# Native packaged executable + installer gate. Called by Windows CI/release.
# pwsh -NoProfile -File scripts/windows/test.ps1 -Dist dist
[CmdletBinding()]
param(
    [string]$Dist = 'dist',
    [string]$ScratchDir = [IO.Path]::GetTempPath()
)
$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest
if ([Environment]::OSVersion.Platform -ne [PlatformID]::Win32NT) { throw 'This gate requires native Windows.' }
$Root = Split-Path (Split-Path $PSScriptRoot -Parent) -Parent
$Dist = (Resolve-Path -LiteralPath $Dist).Path
$Archives = @(Get-ChildItem -LiteralPath $Dist -Filter '*-x86_64-pc-windows-msvc.zip' -File)
if ($Archives.Count -ne 1) { throw 'Expected exactly one Windows x64 release ZIP.' }
$Archive = $Archives[0]
$Hash = (Get-FileHash -LiteralPath $Archive.FullName -Algorithm SHA256).Hash
$Sum = [IO.File]::ReadAllText($Archive.FullName + '.sha256').Trim()
if ($Sum -ine "$Hash  $($Archive.Name)") { throw 'Windows ZIP checksum mismatch.' }
$Work = Join-Path $ScratchDir ('alter-zero-package-test-' + [guid]::NewGuid().ToString('N'))
try {
    & (Join-Path $PSScriptRoot 'test-installer-functions.ps1') -ScratchDir $ScratchDir
    if (-not $?) { throw 'Pure installer helper tests failed.' }
    Add-Type -AssemblyName System.IO.Compression.FileSystem
    [IO.Compression.ZipFile]::ExtractToDirectory($Archive.FullName, $Work)
    $Stem = $Archive.BaseName
    $Binary = Join-Path $Work "$Stem/alter-zero.exe"
    if (-not (Test-Path -LiteralPath $Binary -PathType Leaf)) { throw 'Packaged executable is missing.' }
    $VersionOutput = @(& $Binary --version)
    if ($LASTEXITCODE -ne 0 -or $VersionOutput.Count -ne 1 -or $VersionOutput[0] -notmatch '^alter-zero ([^ ]+)$') { throw 'Packaged --version failed.' }
    if ($Stem -cne "alter-zero-v$($Matches[1])-x86_64-pc-windows-msvc") { throw 'Packaged version does not match its archive name.' }
    & $Binary --help
    if ($LASTEXITCODE -ne 0) { throw 'Packaged --help failed.' }
    & (Join-Path $PSScriptRoot 'test-installer.ps1') -BinaryPath $Binary -ScratchDir $ScratchDir
    if (-not $?) { throw 'Offline installer test failed.' }
    & (Join-Path $PSScriptRoot 'test-npm.ps1') -BinaryPath $Binary -ScratchDir $ScratchDir
    if (-not $?) { throw 'Offline npm install test failed.' }
    # PS 5.1 is included on Windows 10; PS 7 alone does not prove the entry
    # point works in the stock shell named in the installation instructions.
    if ($PSVersionTable.PSVersion.Major -ge 7) {
        $WindowsPowerShell = Join-Path $env:SystemRoot 'System32/WindowsPowerShell/v1.0/powershell.exe'
        & $WindowsPowerShell -NoProfile -NonInteractive -File (Join-Path $PSScriptRoot 'test-installer.ps1') -BinaryPath $Binary -ScratchDir $ScratchDir
        if ($LASTEXITCODE -ne 0) { throw 'Windows PowerShell 5.1 installer test failed.' }
    }
    Write-Host 'Native Windows package and installer checks passed.'
} finally {
    if (Test-Path -LiteralPath $Work) { Remove-Item -LiteralPath $Work -Recurse -Force }
}
