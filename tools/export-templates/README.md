# export-templates (dev only)

`export.py` makes the weight-free ONNX templates that the Convert tab fills
with a user's PyTorch RVC v2 weights:

- `assets/templates/rvc-v2-f0-32k.onnx`, `-40k.onnx`, `-48k.onnx`: RVC's ONNX
  generator (`SynthesizerTrnMsNSFsidM`, F0, 768-dim features) exported with
  `torch.onnx` (opset 13). Every initializer that is a state-dict key keeps its
  name and shape but has no data. `vc-convert` fills them by name, so each file
  is under 1 MB.
- `assets/templates/templates.toml`: one `[[template]]` per file, with its
  SHA-256, the RVC commit, the torch version and the architecture config that
  a checkpoint must match.

`vc-convert` embeds these files with `include_bytes!`. **Python is never needed
at runtime or in CI**: only to re-export the templates, which is rare.

## Pinned RVC commit

`d60cd3d009b7e438f92f8624fa89b710d10c6a39` of
[RVC-Project/Retrieval-based-Voice-Conversion-WebUI](https://github.com/RVC-Project/Retrieval-based-Voice-Conversion-WebUI),
the head of its `20240604` branch. In July 2026 `main` was replaced by a new
history without `infer/lib/infer_pack/models_onnx.py`, so `main` can't export
these templates. The `20240604` branch keeps the code that RVC v2 models were
trained and exported with.

## Setup

Use a throwaway venv and checkout outside the repo (PowerShell, from the repo
root). torch 2.4.1 needs Python 3.8 to 3.12.

```
py -3.11 -m venv $env:TEMP\vc-export-venv
& $env:TEMP\vc-export-venv\Scripts\python.exe -m pip install -r tools\export-templates\requirements.txt --extra-index-url https://download.pytorch.org/whl/cpu
git clone https://github.com/RVC-Project/Retrieval-based-Voice-Conversion-WebUI $env:TEMP\rvc
git -C $env:TEMP\rvc checkout d60cd3d009b7e438f92f8624fa89b710d10c6a39
```

Without `py -3.11`, `uv venv --python 3.11 $env:TEMP\vc-export-venv` works too.
Install with `uv pip install` and add `--index-strategy unsafe-best-match`.

## Re-export

```
& $env:TEMP\vc-export-venv\Scripts\python.exe -I tools/export-templates/export.py --rvc $env:TEMP\rvc --out assets/templates
```

It writes the 3 templates and `templates.toml`. The export is reproducible:
the same RVC commit and torch version give the same bytes. Self-checks stop it
if `emb_g.weight` wasn't stripped, or if a state-dict key outside `enc_q.` was
neither stripped nor listed in `UNUSED` with a reason. A renamed or merged
parameter would otherwise keep the export's random weights.

## Check against a real model

```
& $env:TEMP\vc-export-venv\Scripts\python.exe -I tools/export-templates/export.py --rvc $env:TEMP\rvc --out assets/templates --check vendor/test-models/<m>.pth
```

It loads the model with `torch.load(weights_only=True)`, picks the template
whose config matches, fills it as `vc-convert` does, and runs it in
onnxruntime (CPU) and in torch on the same seeded input. The NSF noise is
zeroed on both sides for this comparison only, because the two runtimes draw
different random numbers. It prints the max absolute difference and exits 1 if
that is above `1e-3 × max(1, peak)`. Extract `.pth` files from `.zip` models
into a temporary directory outside the repo first. `cargo xtask
fetch-test-models` downloads the public test models.
