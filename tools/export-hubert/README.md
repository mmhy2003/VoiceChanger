# export-hubert (dev only)

`export.py` makes `hubert.onnx`, the content encoder the engine feeds RVC v2
models with: RVC's own HuBERT base (`hubert_base.pt`) exported with
`torch.onnx` (opset 17).

- Input `source`: `[1, 1, N]` float32, 16 kHz mono.
- Output `embed`: `[1, T, 768]` float32, the output of transformer layer 12
  (what RVC v2 takes from fairseq's `extract_features(output_layer=12)`), one
  frame per 320 samples. 16080 samples give exactly 50 frames.

**Python is never needed at runtime or in CI**: only to re-export the model,
which is rare.

## Source and licence

`hubert_base.pt` from
[lj1995/VoiceConversionWebUI](https://huggingface.co/lj1995/VoiceConversionWebUI)
(MIT), pinned at revision `e6d0c1a17da07c33557852f9dfa2bd44cc75737d`, SHA-256
`f54b40fd2802423a5643779c4861af1e9ee9c1564dc9d32f54f20b5ffba7db96`. It is the
same repo and revision family as RMVPE. It replaces the GPL-3.0 ContentVec
file that the app used before.

`export.py` reads the checkpoint with a restricted unpickler: only
`OrderedDict`, torch's tensor rebuild functions and torch storages are real
classes; everything else (fairseq's config objects) becomes an inert stub. No
code in the pickle runs.

## Setup

Use a throwaway venv outside the repo (PowerShell, from the repo root).
torch 2.4.1 needs Python 3.8 to 3.12.

```
py -3.11 -m venv $env:TEMP\vc-hubert-venv
& $env:TEMP\vc-hubert-venv\Scripts\python.exe -m pip install -r tools\export-hubert\requirements.txt --extra-index-url https://download.pytorch.org/whl/cpu
```

Without `py -3.11`, `uv venv --python 3.11 $env:TEMP\vc-hubert-venv` works too.
Install with `uv pip install` and add `--index-strategy unsafe-best-match`.

## Export

```
& $env:TEMP\vc-hubert-venv\Scripts\python.exe -I tools/export-hubert/export.py --out $env:TEMP\vc-hubert --reference vendor\models\hubert.onnx
```

It downloads `hubert_base.pt` to `%TEMP%\vc-hubert-cache` (`--cache` to
change), checks its SHA-256 and writes `hubert.onnx` to `--out`. It exits 1
when:

- the download's SHA-256 is wrong;
- a checkpoint key doesn't land in torchaudio's `hubert_base()` and isn't
  listed in `UNUSED` with a reason, or a model weight is missing;
- onnxruntime (CPU) and eager PyTorch differ by more than `1e-3` on a seeded
  synthetic voiced signal (3 s, 16080 samples and 400 samples), or the frame
  counts are wrong;
- with `--reference`, the input/output count, rank or last output dimension
  differs from the reference model's. The max difference from the reference's
  output is printed for information only.

It prints the size and SHA-256 of `hubert.onnx`. The export is reproducible:
the same torch version gives the same bytes.

## Hosting

The app doesn't download from Hugging Face for this file. The hosted
`hubert.onnx` on the `base-models-1` release of this repo, and its SHA-256 in
`assets/base-models.toml`, are authoritative.

To publish a new export, ask the maintainer first. Then create a new
`base-models-N` prerelease (never reuse or overwrite an existing one) and
point the manifest at it:

```
gh release create base-models-N --repo mmhy2003/VoiceChanger --target develop --prerelease --latest=false --title "Base models N" --notes "..." $env:TEMP\vc-hubert\hubert.onnx
```

The `base-models-*` tags don't match `v*`, so the release version guards
ignore them.
