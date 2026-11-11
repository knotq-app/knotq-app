param(
  [Parameter(Mandatory = $true)]
  [string]$Installer
)

# Installs KnotQ with the release installer the way a user would and fails
# unless the app is still running after 20s and has written its workspace.
# Mirrors bundling/smoke-test-common.sh.

$ErrorActionPreference = "Stop"

$Work = Join-Path ([System.IO.Path]::GetTempPath()) ("knotq-smoke-" + [System.Guid]::NewGuid().ToString("N"))
$InstallDir = Join-Path $Work "install"
$Data = Join-Path $Work "data"
New-Item -ItemType Directory -Force $Work, $Data | Out-Null

$setup = Start-Process -FilePath $Installer -Wait -PassThru `
  -ArgumentList "/VERYSILENT", "/SUPPRESSMSGBOXES", "/NORESTART", "/DIR=`"$InstallDir`""
if ($setup.ExitCode -ne 0) { throw "installer exited with $($setup.ExitCode)" }

$Bin = Join-Path $InstallDir "knotq.exe"
if (-not (Test-Path $Bin)) { throw "installed binary not found: $Bin" }
if (-not (Test-Path (Join-Path $InstallDir "knotq-mcp.exe"))) { throw "knotq-mcp.exe not installed" }

$env:KNOTQ_DATA_DIR = $Data
$out = Join-Path $Work "out.txt"
$err = Join-Path $Work "err.txt"
$secs = 20
Write-Host "[smoke] launching $Bin for ${secs}s"
$app = Start-Process -FilePath $Bin -PassThru -RedirectStandardOutput $out -RedirectStandardError $err
Start-Sleep -Seconds $secs
if ($app.HasExited) {
  Get-Content $err -ErrorAction SilentlyContinue | Write-Host
  throw "the app exited with code $($app.ExitCode) during launch"
}
Stop-Process -Id $app.Id -Force

$workspace = Join-Path $Data "workspace\workspace.json"
if (-not (Test-Path $workspace)) {
  Get-ChildItem -Recurse $Data | Select-Object -First 20 | ForEach-Object { Write-Host $_.FullName }
  Get-Content $err -ErrorAction SilentlyContinue | Write-Host
  throw "the app stayed up but never wrote its workspace"
}
Write-Host "[smoke] app stayed up and wrote its workspace"

$uninstaller = Join-Path $InstallDir "unins000.exe"
if (Test-Path $uninstaller) {
  Start-Process -FilePath $uninstaller -Wait -ArgumentList "/VERYSILENT", "/SUPPRESSMSGBOXES", "/NORESTART"
}
Write-Host "[ok] Windows smoke test passed"
