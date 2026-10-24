# Offline native Windows installer smoke test; no Pester/modules/downloads.
# Supply the MSVC binary already built by CI. Network access is loopback only.
# pwsh -NoProfile -File scripts/windows/test-installer.ps1 -BinaryPath target/x86_64-pc-windows-msvc/debug/alter-zero.exe
[CmdletBinding()]
param(
    [Parameter(Mandatory = $true)][string]$BinaryPath,
    [string]$ScratchDir = [IO.Path]::GetTempPath()
)
$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest
if ([Environment]::OSVersion.Platform -ne [PlatformID]::Win32NT) { throw 'Run this test on native Windows.' }
$Root = Split-Path (Split-Path $PSScriptRoot -Parent) -Parent
$Installer = Join-Path $Root 'install.ps1'
if (-not (Test-Path -LiteralPath $Installer -PathType Leaf)) { throw "Missing installer: $Installer" }
$BinaryPath = (Resolve-Path -LiteralPath $BinaryPath).Path
$VersionOutput = @(& $BinaryPath --version)
if ($LASTEXITCODE -ne 0 -or $VersionOutput.Count -ne 1 -or $VersionOutput[0] -notmatch '^alter-zero ([0-9]+\.[0-9]+\.[0-9]+[^ ]*)$') {
    throw "The supplied native binary failed --version: $VersionOutput"
}
$Version = $Matches[1]
$Stem = "alter-zero-v$Version-x86_64-pc-windows-msvc"
$Work = Join-Path $ScratchDir ('alter-zero-installer-test-' + [guid]::NewGuid().ToString('N'))
$Dist = Join-Path $Work 'dist'
$Archive = Join-Path $Dist "$Stem.zip"
$Shell = (Get-Process -Id $PID).Path
$Server = $null
$Saved = @{}
foreach ($Name in @('ALTER_ZERO_VERSION', 'ALTER_ZERO_INSTALL_DIR', 'ALTER_ZERO_INSTALL_BASE_URL', 'LOCALAPPDATA', 'ALTER_ZERO_CONFIG_DIR', 'ALTER_ZERO_TELEMETRY', 'NO_COLOR')) {
    $Saved[$Name] = [Environment]::GetEnvironmentVariable($Name, 'Process')
}
$Pass = 0
function Assert-True([bool]$Condition, [string]$Label) {
    if (-not $Condition) { throw "FAIL: $Label" }
    $script:Pass++
    Write-Host "ok $Label"
}
function Write-Sums([string]$Hash = '') {
    if (-not $Hash) { $Hash = (Get-FileHash -LiteralPath $Archive -Algorithm SHA256).Hash.ToLowerInvariant() }
    [IO.File]::WriteAllText((Join-Path $Dist 'SHA256SUMS'), "$Hash  $Stem.zip`n", [Text.Encoding]::ASCII)
}
function Run-Installer([string[]]$Options = @(), [bool]$Success = $true) {
    # Separate process exercises actual argument parsing, env defaults and exit
    # status. Capture both streams; a failure must never print Installed.
    $PreviousPreference = $ErrorActionPreference
    $ErrorActionPreference = 'Continue'  # PS 5.1 represents native stderr as ErrorRecords.
    try {
        $Output = @(& $Shell -NoProfile -NonInteractive -File $Installer @Options 2>&1)
        $Code = $LASTEXITCODE
    } finally { $ErrorActionPreference = $PreviousPreference }
    $Text = ($Output | Out-String)
    if ($Success) {
        Assert-True ($Code -eq 0) "installer exits 0: $Text"
        Assert-True ($Text -match 'Installed') 'success is reported after validation'
    } else {
        Assert-True ($Code -ne 0) "installer fails: $Text"
        Assert-True ($Text -notmatch '(?m)^Installed') 'failure never reports success'
    }
    return $Text
}
try {
    New-Item -ItemType Directory -Path $Dist -Force | Out-Null
    Add-Type -AssemblyName System.IO.Compression.FileSystem
    $Zip = [IO.Compression.ZipFile]::Open($Archive, [IO.Compression.ZipArchiveMode]::Create)
    try {
        $null = $Zip.CreateEntry("$Stem/")
        $null = [IO.Compression.ZipFileExtensions]::CreateEntryFromFile($Zip, $BinaryPath, "$Stem/alter-zero.exe")
        foreach ($Name in @('LICENSE', 'README.md', 'CHANGELOG.md')) {
            $null = [IO.Compression.ZipFileExtensions]::CreateEntryFromFile($Zip, (Join-Path $Root $Name), "$Stem/$Name")
        }
    } finally { $Zip.Dispose() }
    Write-Sums
    $PortFile = Join-Path $Work 'port.txt'
    $ServerError = Join-Path $Work 'server.err'
    $Python = (Get-Command python -ErrorAction Stop).Source
    $ServerScript = Join-Path $Root 'scripts/release/release_server.py'
    # Start-Process joins arguments on Windows: quote paths, which may contain spaces.
    $Server = Start-Process -FilePath $Python -ArgumentList @(('"' + $ServerScript + '"'), ('"' + $Dist + '"'), ('v' + $Version), ('"' + (Join-Path $Root 'install.sh') + '"'), ('"' + $Installer + '"')) -PassThru -NoNewWindow -RedirectStandardOutput $PortFile -RedirectStandardError $ServerError
    for ($i = 0; $i -lt 100; $i++) {
        if ($Server.HasExited) { throw "Release server failed: $(Get-Content -LiteralPath $ServerError -Raw)" }
        if ((Test-Path -LiteralPath $PortFile) -and (Get-Item -LiteralPath $PortFile).Length -gt 0) { break }
        Start-Sleep -Milliseconds 50
    }
    $Port = (Get-Content -LiteralPath $PortFile -Raw).Trim()
    Assert-True ($Port -match '^[0-9]+$') 'offline release server starts'
    $env:ALTER_ZERO_INSTALL_BASE_URL = "http://127.0.0.1:$Port"
    $env:ALTER_ZERO_VERSION = ''
    $env:ALTER_ZERO_INSTALL_DIR = ''
    $env:LOCALAPPDATA = Join-Path $Work 'local app data'
    $env:ALTER_ZERO_CONFIG_DIR = Join-Path $Work 'config'
    $env:ALTER_ZERO_TELEMETRY = '0'
    $env:NO_COLOR = '1'
    $ServedInstaller = (Invoke-WebRequest -UseBasicParsing -Uri "$env:ALTER_ZERO_INSTALL_BASE_URL/install.ps1").Content
    Assert-True ($ServedInstaller -match 'ALTER_ZERO_INSTALL_BASE_URL') 'stand-in serves install.ps1'
    $Text = Run-Installer
    $DefaultExe = Join-Path $env:LOCALAPPDATA 'Alter Zero/bin/alter-zero.exe'
    Assert-True (Test-Path -LiteralPath $DefaultExe -PathType Leaf) 'default install is per-user LocalAppData'
    Assert-True ((Get-FileHash -LiteralPath $DefaultExe).Hash -eq (Get-FileHash -LiteralPath $BinaryPath).Hash) 'installed bytes equal the release binary'
    Assert-True ($Text -match 'checksum') 'checksum verification is reported'
    $Text = Run-Installer
    $env:ALTER_ZERO_VERSION = "v$Version"
    $env:ALTER_ZERO_INSTALL_DIR = Join-Path $Work ('custom path ' + [char]0x00FC)
    $Text = Run-Installer
    $CustomExe = Join-Path $env:ALTER_ZERO_INSTALL_DIR 'alter-zero.exe'
    Assert-True (Test-Path -LiteralPath $CustomExe) 'environment overrides honored'
    $Explicit = Join-Path $Work 'explicit dir'
    $Text = Run-Installer -Options @('-Version', $Version, '-InstallDir', $Explicit)
    Assert-True (Test-Path -LiteralPath (Join-Path $Explicit 'alter-zero.exe')) 'parameters override environment'
    $Before = (Get-FileHash -LiteralPath $CustomExe).Hash
    $Lock = [IO.File]::Open($CustomExe, [IO.FileMode]::Open, [IO.FileAccess]::Read, [IO.FileShare]::Read)
    try {
        $Text = Run-Installer -Success $false
        Assert-True ($Text -match 'locked|running|close') 'locked executable gives actionable failure'
    } finally { $Lock.Dispose() }
    Assert-True ((Get-FileHash -LiteralPath $CustomExe).Hash -eq $Before) 'locked executable stays unchanged'
    $Text = Run-Installer
    Write-Sums -Hash ('0' * 64)
    $Text = Run-Installer -Success $false
    Assert-True ($Text -match 'checksum mismatch') 'bad checksum is refused'
    Assert-True ((Get-FileHash -LiteralPath $CustomExe).Hash -eq $Before) 'bad download leaves existing install alone'
    Write-Sums
    $env:ALTER_ZERO_VERSION = 'v999.0.0'
    $Text = Run-Installer -Success $false
    Assert-True ($Text -match 'release|download') 'missing release fails'
    $env:ALTER_ZERO_VERSION = "v$Version"
    $Zip = [IO.Compression.ZipFile]::Open($Archive, [IO.Compression.ZipArchiveMode]::Update)
    try { $null = $Zip.CreateEntry('../escaped.txt') } finally { $Zip.Dispose() }
    Write-Sums
    $Text = Run-Installer -Success $false
    Assert-True ($Text -match 'layout|unsafe') 'unsafe ZIP entry is refused before extraction'
    Assert-True (-not (Test-Path -LiteralPath (Join-Path $Work 'escaped.txt'))) 'unsafe ZIP creates no escaped file'
    Write-Host "$Pass native installer assertions passed (offline)."
} finally {
    if ($null -ne $Server -and -not $Server.HasExited) { Stop-Process -Id $Server.Id -Force }
    foreach ($Name in $Saved.Keys) { [Environment]::SetEnvironmentVariable($Name, $Saved[$Name], 'Process') }
    if (Test-Path -LiteralPath $Work) { Remove-Item -LiteralPath $Work -Recurse -Force }
}
