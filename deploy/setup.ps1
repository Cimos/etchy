<#
  etchy demo — Windows serve-side launcher.

  Build happens on WSL/Linux (deploy/setup.sh); this script SERVES an already-
  staged bundle on Windows and writes feedback into the repo at
  deploy\feedback\<hostname>.jsonl (preserved + collectable).

  Usage:
    deploy\setup.ps1 [-Port 8080] [-ServeDir <dir>]

    -ServeDir  folder holding etchy-gui.js, etchy-gui_bg.wasm, index.html,
               etchy-server.py (default: deploy\.serve next to this script).
#>
param(
  [int]$Port = 8080,
  [string]$ServeDir = ""
)
$ErrorActionPreference = "Stop"
$repo = (Resolve-Path (Join-Path $PSScriptRoot "..")).Path
if (-not $ServeDir) { $ServeDir = Join-Path $repo "deploy\.serve" }

$server = Join-Path $ServeDir "etchy-server.py"
if (-not (Test-Path $server)) {
  throw "nothing staged in $ServeDir. Build+stage on WSL/Linux first: deploy/setup.sh --build-only"
}

$py = (Get-Command python.exe -ErrorAction SilentlyContinue) ?? (Get-Command python -ErrorAction SilentlyContinue)
if (-not $py) { throw "python not found on PATH." }

$fb = Join-Path $repo ("deploy\feedback\{0}.jsonl" -f $env:COMPUTERNAME)
$env:ETCHY_FEEDBACK = $fb
Write-Host "• feedback -> $fb" -ForegroundColor Cyan
Write-Host "• serving $ServeDir on 0.0.0.0:$Port  (Ctrl+C to stop)" -ForegroundColor Cyan
Write-Host "• commit deploy\feedback\$($env:COMPUTERNAME).jsonl to share feedback." -ForegroundColor Cyan
& $py.Source $server $Port $ServeDir
