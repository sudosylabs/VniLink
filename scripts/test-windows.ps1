param([string]$MsysRoot = 'C:\msys64', [switch]$SkipInstall)
# The full Windows suite (CI and `npm run test:windows`): install, check what
# the tasks need with `npm run doctor`, then the checks, Rust tests and UI tests.
# MinGW is found on PATH, else under -MsysRoot (VNILINK_MSYS_ROOT).
$ErrorActionPreference = 'Stop'
if ($env:OS -ne 'Windows_NT') { throw 'This runner requires native Windows.' }
$Root = Split-Path -Parent $PSScriptRoot
if (-not $env:VNILINK_MSYS_ROOT) { $env:VNILINK_MSYS_ROOT = $MsysRoot }
foreach ($Command in @('node', 'npm', 'rustup')) {
    if (-not (Get-Command $Command -ErrorAction SilentlyContinue)) { throw "Install $Command first; CONTRIBUTING.md lists the Windows prerequisites." }
}
function Invoke-Check([string]$Command, [string[]]$Arguments) {
    & $Command @Arguments
    if ($LASTEXITCODE -ne 0) { throw "$Command $($Arguments -join ' ') failed with exit code $LASTEXITCODE" }
}
Push-Location $Root
try {
    # preinstall refuses an unsupported Node; postinstall installs the Rust
    # toolchain and its GNU target.
    if (-not $SkipInstall) { Invoke-Check 'npm' @('ci') }
    # Use the repository-selected toolchain directly; Windows rustup proxy
    # executables may be unavailable in an SSH session after self-update.
    $CargoPath = & rustup which cargo
    if ($LASTEXITCODE -ne 0) { throw 'Cannot resolve the repository Rust toolchain.' }
    $env:PATH = (Split-Path -Parent $CargoPath) + [IO.Path]::PathSeparator + $env:PATH
    Invoke-Check 'npm' @('run', 'doctor', '--', '--require', 'build,test:rust,test:ui')
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
