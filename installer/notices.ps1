<#
.SYNOPSIS
Writes THIRD_PARTY_NOTICES.md at the repo root: assets\notices-extra.md with each
"<!-- licence: NAME -->" marker replaced by that licence's text, then the Rust crate
licences from cargo-about. Called by installer\build.ps1.
#>
$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest
$root = Split-Path -Parent $PSScriptRoot
$licences = [ordered]@{
    'onnxruntime' = 'vendor\licences\onnxruntime.txt'
    'DirectML'    = 'vendor\licences\DirectML.txt'
    'onnx-proto'  = 'crates\convert\proto\LICENSE'
}

if (-not (Get-Command cargo-about -ErrorAction SilentlyContinue)) {
    cargo install cargo-about --locked --features cli
    if ($LASTEXITCODE -ne 0) { throw 'cargo install cargo-about failed' }
}
$crates = Join-Path $root 'target\notices-crates.md'
cargo about generate --manifest-path (Join-Path $root 'crates\app\Cargo.toml') `
    --config (Join-Path $root 'about.toml') --output-file $crates (Join-Path $root 'about.hbs')
if ($LASTEXITCODE -ne 0) { throw 'cargo about generate failed' }

$text = Get-Content -Raw -Encoding utf8 (Join-Path $root 'assets\notices-extra.md')
foreach ($name in $licences.Keys) {
    $path = Join-Path $root $licences[$name]
    if (-not (Test-Path $path -PathType Leaf)) {
        throw "Missing licence file $($licences[$name]) (run cargo xtask fetch-models)"
    }
    $marker = "<!-- licence: $name -->"
    if (-not $text.Contains($marker)) { throw "assets\notices-extra.md has no '$marker'" }
    $body = (Get-Content -Raw -Encoding utf8 $path).TrimEnd()
    $text = $text.Replace($marker, "~~~text`n$body`n~~~")
}
if ($text -match '<!-- licence: [^>]*-->') { throw "Unknown licence marker in assets\notices-extra.md: $($Matches[0])" }

$out = Join-Path $root 'THIRD_PARTY_NOTICES.md'
$all = $text.TrimEnd() + "`n`n" + (Get-Content -Raw -Encoding utf8 $crates)
[IO.File]::WriteAllText($out, $all, (New-Object Text.UTF8Encoding $false))
Write-Host "wrote $out"
