"""Export the weight-free RVC v2 generator templates (dev only).

Builds RVC's ONNX generator (SynthesizerTrnMsNSFsidM) for 32k, 40k and 48k,
exports it with torch.onnx, then strips the data of every initializer that is
a state-dict key. What is left is the graph plus the shapes; vc-convert fills
the weights back in by name from a user's .pth.

    python -I export.py --rvc <rvc checkout> --out assets/templates
    python -I export.py --rvc <rvc checkout> --out assets/templates --check <model.pth>

Python is only needed to (re)make the templates, never at runtime or in CI.
See README.md.
"""

import argparse
import hashlib
import json
import subprocess
import sys
import tomllib
from pathlib import Path

import numpy as np
import onnx
import onnxruntime
import torch

# RVC config file per template. 40k has no v2 config; RVC v2 trains 40k from
# the v1 config with 768-dim features.
RATES = {
    "32k": "configs/v2/32k.json",
    "40k": "configs/v1/40k.json",
    "48k": "configs/v2/48k.json",
}

# State-dict keys (outside enc_q.) that the ONNX graph does not use, each with
# the reason. Any other key that is not stripped is an error: a renamed
# parameter would silently keep its random weights.
UNUSED: dict[str, str] = {}

# ArchConfig fields and their index in the checkpoint's config list.
CONFIG_FIELDS = {
    "spec_channels": 0,
    "inter_channels": 2,
    "hidden_channels": 3,
    "filter_channels": 4,
    "n_heads": 5,
    "n_layers": 6,
    "kernel_size": 7,
    "resblock": 9,
    "resblock_kernel_sizes": 10,
    "resblock_dilation_sizes": 11,
    "upsample_rates": 12,
    "upsample_initial_channel": 13,
    "upsample_kernel_sizes": 14,
    "gin_channels": 16,
    "sr": 17,
}

INPUT_NAMES = ["phone", "phone_lengths", "pitch", "pitchf", "ds", "rnd"]
SEED = 1234


def rvc_config(rvc: Path, rate: str) -> list:
    """The config list exactly as RVC's process_ckpt.savee writes it."""
    hps = json.loads((rvc / RATES[rate]).read_text(encoding="utf-8"))
    data, model = hps["data"], hps["model"]
    return [
        data["filter_length"] // 2 + 1,
        32,
        model["inter_channels"],
        model["hidden_channels"],
        model["filter_channels"],
        model["n_heads"],
        model["n_layers"],
        model["kernel_size"],
        model["p_dropout"],
        model["resblock"],
        model["resblock_kernel_sizes"],
        model["resblock_dilation_sizes"],
        model["upsample_rates"],
        model["upsample_initial_channel"],
        model["upsample_kernel_sizes"],
        model["spk_embed_dim"],
        model["gin_channels"],
        data["sampling_rate"],
    ]


def arch_config(config: list) -> dict:
    """The fields vc-convert matches templates on (no training-only values)."""
    out = {field: config[i] for field, i in CONFIG_FIELDS.items()}
    out["resblock"] = str(out["resblock"])
    out["sr"] = int(out["sr"])
    return out


def build_net(config: list):
    from infer.lib.infer_pack.models_onnx import SynthesizerTrnMsNSFsidM

    net = SynthesizerTrnMsNSFsidM(*config, is_half=False, version="v2")
    return net.eval()


def export(rvc: Path, rate: str, path: Path) -> None:
    torch.manual_seed(SEED)
    net = build_net(rvc_config(rvc, rate))
    # torch.onnx merges initializers with equal values, and a fresh net has
    # many (LayerNorm ones and zeros, the zero-initialised flow post layers).
    # Distinct random values keep one initializer per state-dict key; they are
    # stripped below anyway.
    with torch.no_grad():
        for p in net.parameters():
            p.copy_(torch.randn_like(p))
    keys = set(net.state_dict().keys())
    args = (
        torch.rand(1, 200, 768),
        torch.tensor([200]).long(),
        torch.randint(size=(1, 200), low=5, high=255),
        torch.rand(1, 200),
        torch.LongTensor([0]),
        torch.rand(1, 192, 200),
    )
    torch.onnx.export(
        net,
        args,
        str(path),
        input_names=INPUT_NAMES,
        output_names=["audio"],
        dynamic_axes={"phone": [1], "pitch": [1], "pitchf": [1], "rnd": [2]},
        opset_version=13,
        do_constant_folding=False,
    )

    model = onnx.load(str(path))
    stripped = set()
    for init in model.graph.initializer:
        if init.name in keys:
            assert init.data_type == onnx.TensorProto.FLOAT, init.name
            init.ClearField("raw_data")
            del init.float_data[:]
            stripped.add(init.name)

    assert "emb_g.weight" in stripped, "emb_g.weight was not stripped"
    assert stripped <= keys, f"stripped names that are not state-dict keys: {sorted(stripped - keys)}"
    missing = sorted(
        k for k in keys if not k.startswith("enc_q.") and k not in stripped and k not in UNUSED
    )
    assert not missing, f"state-dict keys neither stripped nor in UNUSED: {missing}"

    onnx.save(model, str(path))
    print(f"{path.name}: {len(stripped)} initializers stripped, {path.stat().st_size} bytes")


def write_manifest(rvc: Path, out: Path) -> None:
    commit = subprocess.run(
        ["git", "-C", str(rvc), "rev-parse", "HEAD"], check=True, capture_output=True, text=True
    ).stdout.strip()
    lines = [
        "# Weight-free RVC v2 templates, written by tools/export-templates/export.py.",
        "# Do not edit by hand: re-export instead (see tools/export-templates/README.md).",
    ]
    for rate in RATES:
        file = f"rvc-v2-f0-{rate}.onnx"
        sha = hashlib.sha256((out / file).read_bytes()).hexdigest()
        lines += [
            "",
            "[[template]]",
            f'file = "{file}"',
            f'sha256 = "{sha}"',
            f'rvc_commit = "{commit}"',
            f'torch = "{torch.__version__}"',
            "[template.config]",
        ]
        for field, value in arch_config(rvc_config(rvc, rate)).items():
            # json.dumps writes ints, strings and nested lists as valid TOML.
            lines.append(f"{field} = {json.dumps(value)}")
    (out / "templates.toml").write_text("\n".join(lines) + "\n", encoding="utf-8", newline="\n")


def is_stripped(t) -> bool:
    return int(np.prod(t.dims)) > 0 and not t.raw_data and len(t.float_data) == 0


def check(rvc: Path, out: Path, pth: Path) -> int:
    cpt = torch.load(pth, map_location="cpu", weights_only=True)
    weights = {k: v.float() for k, v in cpt["weight"].items()}
    cfg = arch_config(cpt["config"])

    manifest = tomllib.loads((out / "templates.toml").read_text(encoding="utf-8"))
    matches = [t for t in manifest["template"] if t["config"] == cfg]
    if not matches:
        print(f"{pth.name}: no template for {cfg}")
        return 1
    template = matches[0]

    # Fill the template the way vc-convert does: by name, fp32 raw data,
    # emb_g.weight dims from the checkpoint.
    model = onnx.load(str(out / template["file"]))
    for init in model.graph.initializer:
        if not is_stripped(init):
            continue
        w = weights[init.name].numpy()
        if init.name == "emb_g.weight":
            assert w.shape[1] == init.dims[1], init.name
        else:
            assert list(w.shape) == list(init.dims), init.name
        del init.dims[:]
        init.dims.extend(w.shape)
        init.data_type = onnx.TensorProto.FLOAT
        init.raw_data = w.astype("<f4").tobytes()

    # The NSF source adds Gaussian noise inside the graph, and onnxruntime and
    # torch draw different numbers. Zero it on both sides, only for this check,
    # so the comparison measures the weights alone.
    noise = [n for n in model.graph.node if n.op_type in ("RandomNormal", "RandomNormalLike")]
    assert noise, "expected the NSF noise node in the graph"
    for node in noise:
        for attr in node.attribute:
            if attr.name == "scale":
                attr.f = 0.0
                break
        else:
            node.attribute.append(onnx.helper.make_attribute("scale", 0.0))

    gen = torch.Generator().manual_seed(SEED)
    t = 100
    inputs = {
        "phone": torch.rand(1, t, 768, generator=gen),
        "phone_lengths": torch.tensor([t]).long(),
        "pitch": torch.randint(5, 255, (1, t), generator=gen),
        "pitchf": 100.0 + 200.0 * torch.rand(1, t, generator=gen),
        "ds": torch.LongTensor([0]),
        "rnd": torch.rand(1, 192, t, generator=gen),
    }

    session = onnxruntime.InferenceSession(
        model.SerializeToString(), providers=["CPUExecutionProvider"]
    )
    (ort_out,) = session.run(None, {k: v.numpy() for k, v in inputs.items()})

    config = list(cpt["config"])
    config[-3] = weights["emb_g.weight"].shape[0]
    net = build_net(config)
    result = net.load_state_dict(weights, strict=False)
    assert all(k.startswith("enc_q.") for k in result.missing_keys), result.missing_keys
    assert not result.unexpected_keys, result.unexpected_keys
    randn_like = torch.randn_like
    torch.randn_like = torch.zeros_like
    try:
        with torch.no_grad():
            # The ONNX variant of the model only runs traced (it calls
            # torch.clamp on sizes, which are ints in eager mode), as in export.
            args = tuple(inputs.values())
            traced = torch.jit.trace(net, args, check_trace=False)
            torch_out = traced(*args).numpy()
    finally:
        torch.randn_like = randn_like

    assert ort_out.shape == torch_out.shape, (ort_out.shape, torch_out.shape)
    diff = float(np.max(np.abs(ort_out - torch_out)))
    peak = float(np.max(np.abs(torch_out)))
    limit = 1e-3 * max(1.0, peak)
    ok = diff <= limit
    print(
        f"{pth.name}: template {template['file']}, shape {list(ort_out.shape)}, "
        f"peak {peak:.4f}, max diff {diff:.3e} (limit {limit:.3e}) {'ok' if ok else 'FAILED'}"
    )
    return 0 if ok else 1


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument("--rvc", type=Path, required=True, help="RVC WebUI checkout")
    parser.add_argument("--out", type=Path, default=Path("assets/templates"))
    parser.add_argument("--check", type=Path, metavar="PTH", help="compare a model against torch")
    args = parser.parse_args()
    sys.path.insert(0, str(args.rvc.resolve()))

    if args.check:
        return check(args.rvc, args.out, args.check)
    args.out.mkdir(parents=True, exist_ok=True)
    for rate in RATES:
        export(args.rvc, rate, args.out / f"rvc-v2-f0-{rate}.onnx")
    write_manifest(args.rvc, args.out)
    return 0


if __name__ == "__main__":
    sys.exit(main())
