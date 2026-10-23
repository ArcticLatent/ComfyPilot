# First-boot setup for the build VM (see ../vm.sh): SSH access, then the
# tools for the Windows wheel builds and the app's Windows code. Downloads are
# pinned and checked against their published SHA-256 where one exists.
# Writes C:\OEM\setup-done.txt when everything succeeded.
$ErrorActionPreference = 'Stop'
$ProgressPreference = 'SilentlyContinue'  # Invoke-WebRequest is very slow with progress on
$Tools = 'C:\tools'
$Downloads = 'C:\OEM\downloads'
New-Item -ItemType Directory -Force $Tools, $Downloads, 'C:\w' | Out-Null

function Step($message) { Write-Output "=== $(Get-Date -Format 'HH:mm:ss') $message" }

function Download($url, $name, $sha256) {
  $path = Join-Path $Downloads $name
  if (-not (Test-Path $path)) { Invoke-WebRequest $url -OutFile $path -UseBasicParsing }
  if ($sha256 -and (Get-FileHash $path -Algorithm SHA256).Hash -ne $sha256.ToUpper()) {
    Remove-Item $path
    throw "SHA-256 mismatch for $name"
  }
  $path
}

function Run($file, [string[]]$arguments, [int[]]$okCodes = @(0)) {
  $process = Start-Process $file -ArgumentList $arguments -Wait -PassThru -NoNewWindow
  if ($okCodes -notcontains $process.ExitCode) { throw "$file exited with $($process.ExitCode)" }
}

function Add-MachinePath([string[]]$dirs) {
  $path = [Environment]::GetEnvironmentVariable('Path', 'Machine')
  foreach ($dir in $dirs) { if (($path -split ';') -notcontains $dir) { $path = "$path;$dir" } }
  [Environment]::SetEnvironmentVariable('Path', $path, 'Machine')
  $env:Path = "$path;$env:Path"
}

try {
  Step 'OpenSSH server'
  $msi = Download 'https://github.com/PowerShell/Win32-OpenSSH/releases/download/10.4.0.0p1-Preview/OpenSSH-Win64-v10.4.0.0.msi' `
    'OpenSSH-Win64.msi' '788d8643464a01eb63640a2fd746f9ed23d106b5a901a35f88dc8cd40e446ac9'
  Run 'msiexec.exe' @('/i', $msi, '/qn', 'ADDLOCAL=Server')
  New-Item -ItemType Directory -Force 'C:\ProgramData\ssh' | Out-Null
  $keys = 'C:\ProgramData\ssh\administrators_authorized_keys'
  Copy-Item 'C:\OEM\authorized_key.pub' $keys -Force
  icacls.exe $keys /inheritance:r /grant 'Administrators:F' /grant 'SYSTEM:F' | Out-Null
  New-ItemProperty 'HKLM:\SOFTWARE\OpenSSH' -Name DefaultShell -PropertyType String -Force `
    -Value 'C:\Windows\System32\WindowsPowerShell\v1.0\powershell.exe' | Out-Null
  if (-not (Get-NetFirewallRule -Name 'arctic-sshd' -ErrorAction SilentlyContinue)) {
    New-NetFirewallRule -Name 'arctic-sshd' -DisplayName 'OpenSSH Server' -Direction Inbound `
      -Protocol TCP -LocalPort 22 -Action Allow | Out-Null
  }
  Set-Service sshd -StartupType Automatic
  Start-Service sshd

  Step 'Defender exclusions for build directories'
  Add-MpPreference -ExclusionPath $Tools, 'C:\w'
  Add-MpPreference -ExclusionProcess 'cl.exe', 'link.exe', 'nvcc.exe', 'cicc.exe', 'ptxas.exe', `
    'cargo.exe', 'rustc.exe', 'python.exe', 'uv.exe', 'ninja.exe'

  Step 'Git'
  $git = Download 'https://github.com/git-for-windows/git/releases/download/v2.56.0.windows.3/Git-2.56.0.3-64-bit.exe' `
    'Git-64-bit.exe' 'dc6d3db061999136e139f31376c5582a9ed6f36a4ea12f15e54ad7fa2bfc0bf2'
  Run $git @('/VERYSILENT', '/NORESTART', '/NOCANCEL', '/SP-')
  Add-MachinePath @('C:\Program Files\Git\cmd')
  git config --system core.longpaths true

  Step 'uv and Python 3.12'
  $uvZip = Download 'https://github.com/astral-sh/uv/releases/download/0.13.5/uv-x86_64-pc-windows-msvc.zip' `
    'uv.zip' 'a38b8d1902b7cfbc76b423ba1bb456f623f42064cdc72628b6075b89afd1019a'
  Expand-Archive $uvZip "$Tools\uv" -Force
  [Environment]::SetEnvironmentVariable('UV_PYTHON_INSTALL_DIR', "$Tools\python", 'Machine')
  [Environment]::SetEnvironmentVariable('UV_CACHE_DIR', "$Tools\uv-cache", 'Machine')
  $env:UV_PYTHON_INSTALL_DIR = "$Tools\python"
  $env:UV_CACHE_DIR = "$Tools\uv-cache"
  Add-MachinePath @("$Tools\uv")
  uv python install 3.12
  if ($LASTEXITCODE -ne 0) { throw 'uv python install failed' }

  Step 'Rust'
  [Environment]::SetEnvironmentVariable('RUSTUP_HOME', "$Tools\rustup", 'Machine')
  [Environment]::SetEnvironmentVariable('CARGO_HOME', "$Tools\cargo", 'Machine')
  $env:RUSTUP_HOME = "$Tools\rustup"
  $env:CARGO_HOME = "$Tools\cargo"
  $rustup = Download 'https://static.rust-lang.org/rustup/dist/x86_64-pc-windows-msvc/rustup-init.exe' 'rustup-init.exe' $null
  Run $rustup @('-y', '--no-modify-path', '--profile', 'minimal', '--default-toolchain', 'stable',
    '--component', 'clippy', '--component', 'rustfmt')
  Add-MachinePath @("$Tools\cargo\bin")

  Step 'Visual Studio 2022 Build Tools (C++)'
  $vs = Download 'https://aka.ms/vs/17/release/vs_BuildTools.exe' 'vs_BuildTools.exe' $null
  # 3010: success, reboot required.
  Run $vs @('--quiet', '--wait', '--norestart', '--nocache',
    '--add', 'Microsoft.VisualStudio.Workload.VCTools', '--includeRecommended') @(0, 3010)

  Step 'CUDA 13.0 toolkit'
  $cuda = Download 'https://developer.download.nvidia.com/compute/cuda/13.0.3/network_installers/cuda_13.0.3_windows_network.exe' `
    'cuda_network.exe' $null
  $parts = 'nvcc', 'crt', 'nvvm', 'nvptxcompiler', 'nvjitlink', 'cudart', 'thrust', 'nvtx',
    'nvrtc', 'nvrtc_dev', 'cublas', 'cublas_dev', 'cusparse', 'cusparse_dev', 'cusolver',
    'cusolver_dev', 'curand', 'curand_dev', 'cufft', 'cufft_dev', 'cuda_profiler_api' |
    ForEach-Object { "${_}_13.0" }
  Run $cuda (@('-s') + $parts)
  if (-not (Test-Path 'C:\Program Files\NVIDIA GPU Computing Toolkit\CUDA\v13.0\bin\nvcc.exe')) {
    throw 'nvcc.exe not found after the CUDA install'
  }

  # sshd hands new sessions the environment it started with.
  Restart-Service sshd
  Step 'Done'
  Set-Content 'C:\OEM\setup-done.txt' (Get-Date -Format o)
} catch {
  Write-Output "SETUP FAILED: $_"
  Write-Output $_.ScriptStackTrace
  exit 1
}
