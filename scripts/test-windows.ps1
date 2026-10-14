param([string]$MsysRoot = 'C:\msys64', [switch]$SkipInstall)
$ErrorActionPreference = 'Stop'
if ($env:OS -ne 'Windows_NT') { throw 'This runner requires native Windows.' }
$Root = Split-Path -Parent $PSScriptRoot
$NativeBin = Join-Path $MsysRoot 'mingw64\bin'
if (-not (Test-Path $NativeBin)) { throw "Install MSYS2 and its x86_64 MinGW GCC, Clang, NASM and Make packages first: $NativeBin" }
$env:PATH = $NativeBin + [IO.Path]::PathSeparator + $env:PATH
$env:CARGO_BUILD_TARGET = 'x86_64-pc-windows-gnu'
$env:CARGO_TARGET_X86_64_PC_WINDOWS_GNU_LINKER = 'gcc'
$env:CMAKE_GENERATOR = 'MinGW Makefiles'
$env:LIBCLANG_PATH = $NativeBin
if (-not (Test-Path (Join-Path $NativeBin 'libclang.dll'))) { throw "Missing MSYS2 libclang: $NativeBin" }
foreach ($Command in @('node', 'npm', 'cargo', 'rustup', 'gcc', 'clang', 'nasm', 'cmake', 'mingw32-make')) {
    if (-not (Get-Command $Command -ErrorAction SilentlyContinue)) { throw "Missing Windows validation prerequisite: $Command" }
}
function Invoke-Check([string]$Command, [string[]]$Arguments) {
    & $Command @Arguments
    if ($LASTEXITCODE -ne 0) { throw "$Command failed with exit code $LASTEXITCODE" }
}
Push-Location $Root
try {
    # Use the repository-selected toolchain directly; Windows rustup proxy
    # executables may be unavailable in an SSH session after self-update.
    $CargoPath = & rustup which cargo
    if ($LASTEXITCODE -ne 0) { throw 'Cannot resolve the repository Rust toolchain.' }
    $env:PATH = (Split-Path -Parent $CargoPath) + [IO.Path]::PathSeparator + $env:PATH
    Invoke-Check 'node' @('-e', "const [major,minor]=process.versions.node.split('.').map(Number); if(process.arch!=='x64'||major<22||(major===22&&minor<19)) process.exit(1);")
    Invoke-Check 'rustup' @('target', 'add', 'x86_64-pc-windows-gnu')
    if (-not $SkipInstall) { Invoke-Check 'npm' @('ci') }
    Invoke-Check 'npm' @('run', 'check')
    Invoke-Check 'cargo' @('fmt', '--all', '--', '--check')
    $Failures = @()
    foreach ($Arguments in @(@('run', 'test:rust'), @('test'))) {
        try {
            Invoke-Check 'npm' $Arguments
        } catch {
            $Failures += $_.Exception.Message
            Write-Output $_.Exception.Message
        }
    }
    if ($Failures.Count -gt 0) { throw ($Failures -join '; ') }
} finally {
    Pop-Location
}
