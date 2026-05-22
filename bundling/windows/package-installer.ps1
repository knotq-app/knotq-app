param(
  [Parameter(Mandatory = $true)]
  [string]$Version,

  [Parameter(Mandatory = $true)]
  [string]$Target
)

$ErrorActionPreference = "Stop"

$Root = (Resolve-Path (Join-Path $PSScriptRoot "../..")).Path
$Binary = Join-Path $Root "target\$Target\release\knotq.exe"
$Dist = Join-Path $Root "dist\windows"
$Script = Join-Path $PSScriptRoot "KnotQ.iss"

if (-not (Test-Path $Binary)) { throw "binary not found: $Binary" }
if (-not (Test-Path $Script)) { throw "installer script not found: $Script" }

New-Item -ItemType Directory -Force -Path $Dist | Out-Null

$IsccCandidates = @(
  "ISCC.exe",
  "C:\Program Files (x86)\Inno Setup 6\ISCC.exe",
  "C:\Program Files\Inno Setup 6\ISCC.exe"
)

$Iscc = $null
foreach ($Candidate in $IsccCandidates) {
  $Command = Get-Command $Candidate -ErrorAction SilentlyContinue
  if ($Command) {
    $Iscc = $Command.Source
    break
  }
  if (Test-Path $Candidate) {
    $Iscc = $Candidate
    break
  }
}

if (-not $Iscc) { throw "ISCC.exe not found. Install Inno Setup 6 before running this script." }

$env:KNOTQ_VERSION = $Version
$env:KNOTQ_SOURCE_ROOT = $Root
$env:KNOTQ_BINARY = $Binary
$env:KNOTQ_OUTPUT_DIR = $Dist

& $Iscc "$Script"

if ($LASTEXITCODE -ne 0) { throw "ISCC failed with exit code $LASTEXITCODE" }

$SetupPath = Join-Path $Dist "KnotQ-$Version-windows-x64-setup.exe"
if (-not (Test-Path $SetupPath)) { throw "installer output not found: $SetupPath" }

Write-Host "[ok] $SetupPath"
