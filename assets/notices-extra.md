# Third-party notices

VoiceChanger is MIT-licensed (see LICENSE). It ships with the third-party components below.

## ONNX Runtime 1.24.4 (vendor/runtime/onnxruntime.dll)

Microsoft ONNX Runtime, MIT licence. https://github.com/microsoft/onnxruntime

<!-- licence: onnxruntime -->

## DirectML 1.15.4 (vendor/runtime/DirectML.dll)

Microsoft DirectML, redistributed under the Microsoft DirectML licence. https://www.nuget.org/packages/Microsoft.AI.DirectML

<!-- licence: DirectML -->

## HuBERT base (vendor/models/hubert.onnx)

Content encoder `hubert_base.pt` from lj1995/VoiceConversionWebUI, MIT licence, exported to ONNX by tools/export-hubert. https://huggingface.co/lj1995/VoiceConversionWebUI

## RMVPE (vendor/models/rmvpe.onnx)

Pitch estimator from lj1995/VoiceConversionWebUI, MIT licence. https://huggingface.co/lj1995/VoiceConversionWebUI

## ONNX protocol definition (onnx.proto)

Used to read and write ONNX models in the Convert tab. From the ONNX project, Apache-2.0. https://github.com/onnx/onnx

<!-- licence: onnx-proto -->
