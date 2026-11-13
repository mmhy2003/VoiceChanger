"""Export RVC's MIT HuBERT base content encoder to hubert.onnx (dev only).

Downloads hubert_base.pt from lj1995/VoiceConversionWebUI at a pinned
revision, checks its SHA-256, reads the weights with a restricted unpickler
(nothing in the pickle runs), maps them into torchaudio's HuBERT base and
exports layer 12 (768-dim, what RVC v2 uses) with torch.onnx. The result is
checked against eager PyTorch in onnxruntime (CPU).

    python -I export.py --out <dir>
    python -I export.py --out <dir> --reference vendor/models/hubert.onnx

Python is only needed to (re)make hubert.onnx, never at runtime or in CI.
See README.md.
"""

import argparse
import hashlib
import os
import pickle
import sys
import tempfile
import types
import urllib.request
from collections import OrderedDict
from pathlib import Path

import numpy as np
import onnxruntime
import torch
import torch._utils
import torchaudio
from torchaudio.models.wav2vec2.utils.import_fairseq import _convert_state_dict

REVISION = "e6d0c1a17da07c33557852f9dfa2bd44cc75737d"
URL = f"https://huggingface.co/lj1995/VoiceConversionWebUI/resolve/{REVISION}/hubert_base.pt"
SHA256 = "f54b40fd2802423a5643779c4861af1e9ee9c1564dc9d32f54f20b5ffba7db96"

INPUT_NAME = "source"  # [1, 1, N] float32, 16 kHz
OUTPUT_NAME = "embed"  # [1, T, 768] float32, one frame per 320 samples
OPSET = 17
TOLERANCE = 1e-3
SEED = 1234
SAMPLE_RATE = 16000

# Checkpoint keys that the content encoder does not use, each with the reason.
# They are dropped before conversion. Any other key that does not land in the
# model is an error: a renamed parameter would silently keep random weights.
UNUSED: dict[str, str] = {
    "label_embs_concat": "pre-training cluster label embeddings (training target only)",
    "final_proj.weight": "pre-training projection to label space; RVC v2 reads layer 12 before it",
    "final_proj.bias": "pre-training projection to label space; RVC v2 reads layer 12 before it",
    "mask_emb": "pre-training mask embedding (no masking at inference)",
}


class _Stub:
    """Inert stand-in for every class the checkpoint names outside the allowlist.

    Fairseq stores its config next to the weights (argparse namespaces,
    omegaconf objects, enums). Those are never needed, so they are read into
    stubs that accept and ignore everything instead of being imported.
    """

    def __new__(cls, *args, **kwargs):
        return super().__new__(cls)

    def __init__(self, *args, **kwargs):
        pass

    def __call__(self, *args, **kwargs):
        return _Stub()

    def __setstate__(self, state):
        pass

    def __setitem__(self, key, value):
        pass

    def append(self, value):
        pass

    def extend(self, values):
        pass


ALLOWED = {
    ("collections", "OrderedDict"): OrderedDict,
    ("torch._utils", "_rebuild_tensor_v2"): torch._utils._rebuild_tensor_v2,
    ("torch._utils", "_rebuild_parameter"): torch._utils._rebuild_parameter,
}


class RestrictedUnpickler(pickle.Unpickler):
    def find_class(self, module, name):
        if (module, name) in ALLOWED:
            return ALLOWED[(module, name)]
        if module == "torch" and name.endswith("Storage") and hasattr(torch, name):
            return getattr(torch, name)
        return _Stub


def _restricted_load(file, **kwargs):
    return RestrictedUnpickler(file, **kwargs).load()


restricted = types.ModuleType("restricted_pickle")
restricted.Unpickler = RestrictedUnpickler
restricted.load = _restricted_load


class Encoder(torch.nn.Module):
    def __init__(self, hubert):
        super().__init__()
        self.hubert = hubert

    def forward(self, source):
        return self.hubert.extract_features(source.squeeze(1), num_layers=12)[0][-1]


def sha256_of(path: Path) -> str:
    h = hashlib.sha256()
    with path.open("rb") as f:
        for chunk in iter(lambda: f.read(1 << 20), b""):
            h.update(chunk)
    return h.hexdigest()


def download(cache: Path) -> Path:
    cache.mkdir(parents=True, exist_ok=True)
    path = cache / "hubert_base.pt"
    if path.exists() and sha256_of(path) == SHA256:
        print(f"{path}: cached, SHA-256 ok")
        return path
    part = path.with_suffix(".part")
    print(f"downloading {URL}")
    with urllib.request.urlopen(URL) as resp, part.open("wb") as f:
        while chunk := resp.read(1 << 20):
            f.write(chunk)
    got = sha256_of(part)
    if got != SHA256:
        part.unlink()
        raise SystemExit(f"hubert_base.pt: SHA-256 {got}, expected {SHA256}")
    os.replace(part, path)
    print(f"{path}: SHA-256 ok")
    return path


def load_model(path: Path):
    ckpt = torch.load(path, map_location="cpu", weights_only=False, pickle_module=restricted)
    state = {k: v for k, v in ckpt["model"].items() if k not in UNUSED}
    dropped = len(ckpt["model"]) - len(state)
    converted = _convert_state_dict(state)
    model = torchaudio.models.hubert_base()
    result = model.load_state_dict({k: v.float() for k, v in converted.items()}, strict=False)
    if result.missing_keys or result.unexpected_keys:
        raise SystemExit(
            f"weights don't match torchaudio's hubert_base: missing {result.missing_keys}, "
            f"unexpected {result.unexpected_keys} (add unused keys to UNUSED with a reason)"
        )
    print(f"loaded {len(converted)} tensors, {dropped} unused keys dropped")
    return Encoder(model).eval()


def voiced_signal() -> np.ndarray:
    """3 s of a seeded synthetic voice: harmonics of a 120-220 Hz glide plus noise."""
    rng = np.random.default_rng(SEED)
    n = 3 * SAMPLE_RATE
    f0 = np.linspace(120.0, 220.0, n)
    phase = 2.0 * np.pi * np.cumsum(f0) / SAMPLE_RATE
    x = sum(np.sin(k * phase) / k for k in range(1, 11))
    x = 0.5 * x / np.max(np.abs(x)) + 0.01 * rng.standard_normal(n)
    return x.astype(np.float32)


def export(encoder, path: Path) -> None:
    torch.onnx.export(
        encoder,
        (torch.zeros(1, 1, 16080),),
        str(path),
        input_names=[INPUT_NAME],
        output_names=[OUTPUT_NAME],
        dynamic_axes={INPUT_NAME: {2: "samples"}, OUTPUT_NAME: {1: "frames"}},
        opset_version=OPSET,
    )


def check(encoder, path: Path) -> bool:
    session = onnxruntime.InferenceSession(str(path), providers=["CPUExecutionProvider"])
    signal = voiced_signal()
    # (name, samples, expected frames or None for "at least one")
    cases = [
        ("3 s voiced", signal, None),
        ("16080 samples", signal[:16080], 50),
        ("400 samples", signal[:400], None),
    ]
    ok = True
    for name, x, frames in cases:
        source = x.reshape(1, 1, -1)
        with torch.no_grad():
            want = encoder(torch.from_numpy(source)).numpy()
        (got,) = session.run([OUTPUT_NAME], {INPUT_NAME: source})
        good = got.shape == want.shape and got.shape[0] == 1 and got.shape[2] == 768
        good = good and (got.shape[1] == frames if frames else got.shape[1] >= 1)
        diff = float(np.max(np.abs(got - want))) if got.shape == want.shape else float("inf")
        good = good and diff <= TOLERANCE
        ok = ok and good
        print(
            f"{name}: shape {list(got.shape)}, max diff vs torch {diff:.3e} "
            f"(limit {TOLERANCE:.0e}) {'ok' if good else 'FAILED'}"
        )
    return ok


def signature(session) -> tuple:
    inputs, outputs = session.get_inputs(), session.get_outputs()
    return (len(inputs), len(outputs), len(inputs[0].shape), len(outputs[0].shape), outputs[0].shape[-1])


def compare_reference(path: Path, reference: Path) -> bool:
    new = onnxruntime.InferenceSession(str(path), providers=["CPUExecutionProvider"])
    old = onnxruntime.InferenceSession(str(reference), providers=["CPUExecutionProvider"])
    labels = ("inputs", "outputs", "input rank", "output rank", "output last dim")
    ok = True
    for label, a, b in zip(labels, signature(new), signature(old)):
        same = a == b
        ok = ok and same
        print(f"reference {label}: {a} vs {b} {'ok' if same else 'MISMATCH'}")
    if not ok:
        return False

    # For information only: how far the MIT model's features are from the old one's.
    x = voiced_signal()
    (got,) = new.run(None, {new.get_inputs()[0].name: x.reshape(1, 1, -1)})
    rank = len(old.get_inputs()[0].shape)
    (ref,) = old.run(None, {old.get_inputs()[0].name: x.reshape((1,) * (rank - 1) + (-1,))})
    if got.shape == ref.shape:
        print(f"reference max abs diff (information): {float(np.max(np.abs(got - ref))):.3e}")
    else:
        print(f"reference output shape {list(ref.shape)} vs {list(got.shape)} (information)")
    return True


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument("--out", type=Path, required=True, help="output directory for hubert.onnx")
    parser.add_argument("--reference", type=Path, help="old hubert.onnx to compare the signature with")
    parser.add_argument(
        "--cache",
        type=Path,
        default=Path(tempfile.gettempdir()) / "vc-hubert-cache",
        help="download cache for hubert_base.pt",
    )
    args = parser.parse_args()

    encoder = load_model(download(args.cache))
    args.out.mkdir(parents=True, exist_ok=True)
    path = args.out / "hubert.onnx"
    export(encoder, path)

    if not check(encoder, path):
        return 1
    if args.reference and not compare_reference(path, args.reference):
        return 1
    size = path.stat().st_size / (1024 * 1024)
    print(f"{path}: {size:.1f} MB, SHA-256 {sha256_of(path)}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
