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
$Publisher = if ($env:WINDOWS_PUBLISHER) { $env:WINDOWS_PUBLISHER } else { "CN=Enigmadux" }
$PublisherDisplayName = if ($env:WINDOWS_PUBLISHER_DISPLAY_NAME) { $env:WINDOWS_PUBLISHER_DISPLAY_NAME } else { "Enigmadux" }

if (-not (Test-Path $Binary)) { throw "binary not found: $Binary" }

$parts = $Version -split '\.'
while ($parts.Count -lt 4) { $parts += "0" }
$MsixVersion = ($parts[0..3]) -join '.'

New-Item -ItemType Directory -Force -Path $Dist | Out-Null

$TempDir = Join-Path ([System.IO.Path]::GetTempPath()) ("knotq-windows-" + [System.Guid]::NewGuid().ToString("N"))
New-Item -ItemType Directory -Force -Path $TempDir | Out-Null

function New-ResizedPng {
  param(
    [Parameter(Mandatory = $true)]
    [string]$Source,

    [Parameter(Mandatory = $true)]
    [string]$Destination,

    [Parameter(Mandatory = $true)]
    [int]$Size
  )

  Add-Type -AssemblyName System.Drawing
  $image = [System.Drawing.Image]::FromFile($Source)
  try {
    $bitmap = New-Object System.Drawing.Bitmap $Size, $Size
    try {
      $graphics = [System.Drawing.Graphics]::FromImage($bitmap)
      try {
        $graphics.Clear([System.Drawing.Color]::Transparent)
        $graphics.InterpolationMode = [System.Drawing.Drawing2D.InterpolationMode]::HighQualityBicubic
        $graphics.SmoothingMode = [System.Drawing.Drawing2D.SmoothingMode]::HighQuality
        $graphics.PixelOffsetMode = [System.Drawing.Drawing2D.PixelOffsetMode]::HighQuality
        $graphics.DrawImage($image, 0, 0, $Size, $Size)
        $bitmap.Save($Destination, [System.Drawing.Imaging.ImageFormat]::Png)
      } finally {
        $graphics.Dispose()
      }
    } finally {
      $bitmap.Dispose()
    }
  } finally {
    $image.Dispose()
  }
}

try {
  $LayoutDir = Join-Path $TempDir "layout"
  $AssetsDir = Join-Path $LayoutDir "Assets"
  $AppAssetsDir = Join-Path $LayoutDir "assets"
  New-Item -ItemType Directory -Force -Path $AssetsDir | Out-Null
  New-Item -ItemType Directory -Force -Path $AppAssetsDir | Out-Null
  Copy-Item $Binary (Join-Path $LayoutDir "knotq.exe")
  Copy-Item -Path (Join-Path $Root "desktop\app\assets\*") -Destination $AppAssetsDir -Recurse -Force
  Get-ChildItem $AppAssetsDir -Filter ".DS_Store" -Recurse -ErrorAction SilentlyContinue | Remove-Item -Force -ErrorAction SilentlyContinue

  $IconSource = Join-Path $Root "desktop\app\assets\app-icon\windows.png"
  if (-not (Test-Path $IconSource)) {
    $IconSource = Join-Path $Root "desktop\app\assets\app-icon\512x512.png"
  }
  if (-not (Test-Path $IconSource)) {
    $IconSource = Join-Path $Root "desktop\app\assets\app-icon\256x256.png"
  }
  if (-not (Test-Path $IconSource)) { throw "app icon not found" }

  New-ResizedPng $IconSource (Join-Path $AssetsDir "StoreLogo.png") 50
  New-ResizedPng $IconSource (Join-Path $AssetsDir "Square44x44Logo.png") 44
  New-ResizedPng $IconSource (Join-Path $AssetsDir "Square150x150Logo.png") 150

  $Manifest = @"
<?xml version="1.0" encoding="utf-8"?>
<Package xmlns="http://schemas.microsoft.com/appx/manifest/foundation/windows10"
         xmlns:uap="http://schemas.microsoft.com/appx/manifest/uap/windows10"
         xmlns:rescap="http://schemas.microsoft.com/appx/manifest/foundation/windows10/restrictedcapabilities"
         IgnorableNamespaces="rescap">
  <Identity Name="Enigmadux.KnotQ"
            Publisher="$Publisher"
            Version="$MsixVersion"
            ProcessorArchitecture="x64" />
  <Properties>
    <DisplayName>KnotQ</DisplayName>
    <PublisherDisplayName>$PublisherDisplayName</PublisherDisplayName>
    <Description>A structured task and calendar app.</Description>
    <Logo>Assets\StoreLogo.png</Logo>
  </Properties>
  <Dependencies>
    <TargetDeviceFamily Name="Windows.Desktop" MinVersion="10.0.17763.0" MaxVersionTested="10.0.22621.0" />
  </Dependencies>
  <Resources>
    <Resource Language="en-us" />
  </Resources>
  <Applications>
    <Application Id="KnotQ"
                 Executable="knotq.exe"
                 EntryPoint="Windows.FullTrustApplication">
      <uap:VisualElements DisplayName="KnotQ"
                          Description="A structured task and calendar app."
                          BackgroundColor="transparent"
                          Square150x150Logo="Assets\Square150x150Logo.png"
                          Square44x44Logo="Assets\Square44x44Logo.png" />
    </Application>
  </Applications>
  <Capabilities>
    <rescap:Capability Name="runFullTrust" />
  </Capabilities>
</Package>
"@
  $Manifest | Out-File -Encoding utf8 (Join-Path $LayoutDir "AppxManifest.xml")

  $MakeAppx = Get-ChildItem "C:\Program Files (x86)\Windows Kits\10\bin\*\x64\makeappx.exe" -ErrorAction SilentlyContinue |
    Sort-Object FullName |
    Select-Object -Last 1
  if (-not $MakeAppx) { throw "makeappx.exe not found" }

  $MsixPath = Join-Path $Dist "KnotQ-$Version-windows-x64.msix"
  & $MakeAppx.FullName pack /d "$LayoutDir" /p "$MsixPath" /nv
  if ($LASTEXITCODE -ne 0) { throw "makeappx failed with exit code $LASTEXITCODE" }

  Write-Host "[ok] $MsixPath"
} finally {
  Remove-Item $TempDir -Recurse -Force -ErrorAction SilentlyContinue
}
