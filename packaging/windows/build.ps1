# Builds ComfyPilot's Windows installer (the release workflow's Windows job):
#
#   pwsh packaging/windows/build.ps1 [-OutputDir dist] [-AllowNoCatalog]
#
# Needs the MSVC toolchain, Rust and Qt 6 (windeployqt on PATH or under
# QT_ROOT_DIR). Inno Setup is installed with Chocolatey when iscc is missing.
# The version comes from app/Cargo.toml; the installer lands in
# <OutputDir>/ComfyPilot-Setup-<version>.exe.
#
# The catalog settings (ARCTIC_SUPABASE_URL / ARCTIC_SUPABASE_PUBLISHABLE_KEY)
# are built into the app, so they're required unless -AllowNoCatalog.
param(
    [string]$OutputDir = "dist",
    [switch]$AllowNoCatalog
)

Set-StrictMode -Version Latest
$ErrorActionPreference = "Stop"

$root = Resolve-Path (Join-Path $PSScriptRoot "..\..")
Set-Location $root

function Invoke-Checked([string]$What, [scriptblock]$Command) {
    & $Command
    if ($LASTEXITCODE -ne 0) {
        throw "$What failed (exit $LASTEXITCODE)"
    }
}

$cargoToml = Get-Content (Join-Path $root "app\Cargo.toml") -Raw
$version = [regex]::Match($cargoToml, '(?m)^version\s*=\s*"([^"]+)"').Groups[1].Value
if (-not $version) {
    throw "No version in app/Cargo.toml"
}

if (-not $AllowNoCatalog -and (-not $env:ARCTIC_SUPABASE_URL -or -not $env:ARCTIC_SUPABASE_PUBLISHABLE_KEY)) {
    throw "ARCTIC_SUPABASE_URL and ARCTIC_SUPABASE_PUBLISHABLE_KEY must be set (or pass -AllowNoCatalog)"
}

Write-Host "Building ComfyPilot $version (Windows)..."
Invoke-Checked "cargo build" { cargo build --release --locked --manifest-path app/Cargo.toml }

$targetDir = if ($env:CARGO_TARGET_DIR) { $env:CARGO_TARGET_DIR } else { Join-Path $root "target" }
$exe = Join-Path $targetDir "release\ComfyPilot.exe"
if (-not (Test-Path $exe)) {
    throw "Missing $exe"
}

# The app with its Qt libraries, plugins and QML modules.
$work = Join-Path $targetDir "windows-installer"
$stage = Join-Path $work "ComfyPilot"
Remove-Item -Recurse -Force $work -ErrorAction SilentlyContinue
New-Item -ItemType Directory -Path $stage -Force | Out-Null
Copy-Item $exe $stage

$windeployqt = (Get-Command windeployqt -ErrorAction SilentlyContinue).Source
if (-not $windeployqt -and $env:QT_ROOT_DIR) {
    $windeployqt = Join-Path $env:QT_ROOT_DIR "bin\windeployqt.exe"
}
if (-not $windeployqt -or -not (Test-Path $windeployqt)) {
    throw "windeployqt not found (put Qt's bin folder on PATH or set QT_ROOT_DIR)"
}
Invoke-Checked "windeployqt" {
    & $windeployqt --release --qmldir (Join-Path $root "app\qml") --no-translations --no-opengl-sw `
        --no-compiler-runtime (Join-Path $stage "ComfyPilot.exe")
}

# The app sets the Basic Controls style at runtime, which windeployqt can't
# see from the QML; make sure it was deployed.
if (-not (Test-Path (Join-Path $stage "qml\QtQuick\Controls\Basic"))) {
    throw "windeployqt did not deploy the QtQuick.Controls Basic style"
}
# Video previews need Qt Multimedia's FFmpeg backend and FFmpeg's DLLs.
if (-not (Get-ChildItem (Join-Path $stage "plugins\multimedia"), (Join-Path $stage "multimedia") -Filter "ffmpegmediaplugin*.dll" -ErrorAction SilentlyContinue)) {
    throw "windeployqt did not deploy the FFmpeg multimedia plugin"
}
if (-not (Get-ChildItem $stage -Filter "avcodec*.dll")) {
    $qtBin = Split-Path $windeployqt
    Copy-Item (Join-Path $qtBin "av*.dll"), (Join-Path $qtBin "sw*.dll") $stage
}

# The Visual C++ runtime, next to the app (no separate redistributable).
foreach ($dll in "msvcp140.dll", "msvcp140_1.dll", "msvcp140_2.dll", "vcruntime140.dll", "vcruntime140_1.dll") {
    Copy-Item (Join-Path $env:SystemRoot "System32\$dll") $stage
}

# [InstallDelete] entries for every folder in the app, so an update clears
# the previous version's Qt files first.
$cleanup = Join-Path $work "cleanup.iss"
$entries = Get-ChildItem $stage -Directory | ForEach-Object {
    "Type: filesandordirs; Name: `"{app}\$($_.Name)`""
}
Set-Content -Path $cleanup -Value $entries -Encoding utf8

$iscc = (Get-Command iscc -ErrorAction SilentlyContinue).Source
if (-not $iscc) {
    $iscc = Join-Path ${env:ProgramFiles(x86)} "Inno Setup 6\ISCC.exe"
}
if (-not (Test-Path $iscc)) {
    Invoke-Checked "choco install innosetup" { choco install innosetup --no-progress -y }
}
if (-not (Test-Path $iscc)) {
    throw "Inno Setup's ISCC.exe not found"
}

$out = Join-Path $root $OutputDir
New-Item -ItemType Directory -Path $out -Force | Out-Null
Invoke-Checked "iscc" {
    & $iscc /Q "/DAppVersion=$version" "/DSourceDir=$stage" "/DOutputDir=$out" "/DCleanupFile=$cleanup" `
        (Join-Path $PSScriptRoot "comfypilot.iss")
}

$installer = Join-Path $out "ComfyPilot-Setup-$version.exe"
if (-not (Test-Path $installer)) {
    throw "Missing $installer"
}
Write-Host "Built $installer ($([math]::Round((Get-Item $installer).Length / 1MB, 1)) MB)"
