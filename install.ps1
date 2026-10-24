# Alter Zero's native Windows x64 installer (PowerShell 5.1+).
# irm https://raw.githubusercontent.com/linuztx/alter-zero/main/install.ps1 | iex
# Or save it, then: .\install.ps1 -Version 0.15.0 -InstallDir C:\Tools\AlterZero
# No administrator rights, machine PATH changes, or execution-policy changes.
[CmdletBinding()]
param(
    [string]$Version = $env:ALTER_ZERO_VERSION,
    [Alias('Dir')][string]$InstallDir = $env:ALTER_ZERO_INSTALL_DIR,
    [string]$BaseUrl = $env:ALTER_ZERO_INSTALL_BASE_URL,
    [switch]$Help
)

function Get-AlterZeroTag([string]$Base) {
    $Request = [Net.HttpWebRequest]::Create("$Base/releases/latest")
    $Request.AllowAutoRedirect = $true
    $Request.UserAgent = 'alter-zero-installer'
    $Request.Timeout = 30000
    $Response = $Request.GetResponse()
    try {
        $Path = $Response.ResponseUri.AbsolutePath
        if ($Path -cnotmatch '/releases/tag/(v[^/]+)/*$') { throw "The latest release redirect is not a release tag: $Path" }
        return $Matches[1]
    } finally { $Response.Dispose() }
}

function Get-AlterZeroFile([string]$Url, [string]$Destination) {
    $Client = New-Object Net.WebClient
    try {
        $Client.Headers.Add('User-Agent', 'alter-zero-installer')
        $Client.DownloadFile($Url, $Destination)
    } catch {
        throw "Could not download release file $Url : $($_.Exception.Message)"
    } finally { $Client.Dispose() }
}

function Expand-AlterZeroZip([string]$Archive, [string]$Stem, [string]$Destination) {
    Add-Type -AssemblyName System.IO.Compression.FileSystem
    $Expected = New-Object 'System.Collections.Generic.HashSet[string]' ([StringComparer]::Ordinal)
    $null = $Expected.Add("$Stem/")
    foreach ($Name in @('alter-zero.exe', 'LICENSE', 'README.md', 'CHANGELOG.md')) { $null = $Expected.Add("$Stem/$Name") }
    $Seen = New-Object 'System.Collections.Generic.HashSet[string]' ([StringComparer]::Ordinal)
    $Zip = [IO.Compression.ZipFile]::OpenRead($Archive)
    try {
        if ($Zip.Entries.Count -ne 5) { throw 'Unsafe ZIP layout: expected the stem directory and four release files.' }
        foreach ($Entry in $Zip.Entries) {
            # Exact case-sensitive allowlist: rejects ../, absolute/drive/UNC
            # paths, backslashes, alternate streams, duplicates and aliases.
            if (-not $Expected.Contains($Entry.FullName) -or -not $Seen.Add($Entry.FullName)) {
                throw "Unsafe ZIP layout: unexpected or duplicate entry '$($Entry.FullName)'."
            }
            $Kind = ($Entry.ExternalAttributes -shr 16) -band 0xF000
            $ExpectedKind = if ($Entry.FullName -ceq "$Stem/") { 0x4000 } else { 0x8000 }
            if (($Kind -ne 0 -and $Kind -ne $ExpectedKind) -or ($Entry.ExternalAttributes -band 0x400) -ne 0 -or $Entry.Length -gt 536870912) {
                throw "Unsafe ZIP layout: link, special file or oversized entry '$($Entry.FullName)'."
            }
        }
        # Validate ALL names before creating files; never call ExtractToDirectory.
        $null = [IO.Directory]::CreateDirectory($Destination)
        foreach ($Entry in $Zip.Entries) {
            if ($Entry.FullName -ceq "$Stem/") { continue }
            $Name = $Entry.FullName.Substring($Stem.Length + 1)
            $Source = $Entry.Open()
            try {
                $Target = [IO.File]::Open((Join-Path $Destination $Name), [IO.FileMode]::CreateNew, [IO.FileAccess]::Write, [IO.FileShare]::None)
                try { $Source.CopyTo($Target) } finally { $Target.Dispose() }
            } finally { $Source.Dispose() }
        }
    } finally { $Zip.Dispose() }
}

function Test-AlterZeroPE([string]$Path) {
    # Numeric DOS + COFF + PE32+ fields; never trust file(1)'s description.
    $Stream = [IO.File]::Open($Path, [IO.FileMode]::Open, [IO.FileAccess]::Read, [IO.FileShare]::Read)
    $Reader = New-Object IO.BinaryReader($Stream)
    try {
        if ($Stream.Length -lt 64 -or $Reader.ReadUInt16() -ne 0x5A4D) { throw 'PE image lacks a complete MZ DOS header.' }
        $Stream.Position = 0x3C
        [long]$Offset = $Reader.ReadUInt32()
        if ($Offset -lt 64 -or $Offset + 24 -gt $Stream.Length) { throw 'PE header offset is out of bounds.' }
        $Stream.Position = $Offset
        if ($Reader.ReadUInt32() -ne 0x00004550) { throw 'PE signature is invalid.' }
        if ($Reader.ReadUInt16() -ne 0x8664) { throw 'PE machine is not native x64 (0x8664).' }
        $Sections = $Reader.ReadUInt16()
        $Stream.Position = $Offset + 20
        $OptionalSize = $Reader.ReadUInt16()
        $Flags = $Reader.ReadUInt16()
        if (($Flags -band 2) -eq 0 -or ($Flags -band 0x2000) -ne 0) { throw 'PE must be an executable image, not a DLL.' }
        if ($Sections -lt 1 -or $Sections -gt 96 -or $OptionalSize -lt 112 -or $OptionalSize -gt 4096) { throw 'PE section count or optional header size is invalid.' }
        [long]$Table = $Offset + 24 + $OptionalSize
        if ($Table + $Sections * 40 -gt $Stream.Length) { throw 'PE optional header or section table is truncated.' }
        if ($Reader.ReadUInt16() -ne 0x20B) { throw 'PE optional header must be PE32+ (0x020b).' }
        $Stream.Position = $Offset + 24 + 60
        [long]$HeadersSize = $Reader.ReadUInt32()
        if ($HeadersSize -lt $Table + $Sections * 40 -or $HeadersSize -gt $Stream.Length) { throw 'PE SizeOfHeaders is invalid.' }
        $Stream.Position = $Offset + 24 + 68
        if ($Reader.ReadUInt16() -ne 3) { throw 'PE subsystem must be Windows console (3).' }
        $Stream.Position = $Offset + 24 + 108
        $Directories = $Reader.ReadUInt32()
        if ($Directories -gt [Math]::Floor(($OptionalSize - 112) / 8)) { throw 'PE data-directory table is truncated.' }
        for ($i = 0; $i -lt $Sections; $i++) {
            $Stream.Position = $Table + $i * 40 + 16
            [long]$Size = $Reader.ReadUInt32()
            [long]$Start = $Reader.ReadUInt32()
            if ($Size -gt 0 -and ($Start -lt $HeadersSize -or $Start + $Size -gt $Stream.Length)) { throw 'PE raw section is truncated/out of bounds.' }
        }
    } finally { $Reader.Dispose(); $Stream.Dispose() }
}

function Test-AlterZeroVersion([string]$Path, [string]$Want) {
    $Output = @(& $Path --version 2>&1)
    if ($LASTEXITCODE -ne 0 -or $Output.Count -ne 1 -or [string]$Output[0] -cne "alter-zero $Want") {
        throw "The executable failed --version (expected 'alter-zero $Want'): $($Output -join ' ')"
    }
}

function Install-AlterZero {
    $ErrorActionPreference = 'Stop'
    Set-StrictMode -Version Latest
    if ($Help) {
        Write-Output @'
Alter Zero - native Windows x64 installer (Windows 10 1809+, Git for Windows required)

  irm https://raw.githubusercontent.com/linuztx/alter-zero/main/install.ps1 | iex
  .\install.ps1 [-Version X.Y.Z] [-InstallDir DIR] [-BaseUrl URL]

Environment: ALTER_ZERO_VERSION, ALTER_ZERO_INSTALL_DIR, ALTER_ZERO_INSTALL_BASE_URL.
Default directory: %LOCALAPPDATA%\Alter Zero\bin. No administrator rights required.
Close running Alter Zero instances before reinstalling. No execution policy is changed.
'@
        return
    }
    if ([Environment]::OSVersion.Platform -ne [PlatformID]::Win32NT) { throw 'Use install.ps1 on native Windows; use install.sh on Linux/macOS.' }
    try { $Architecture = [Runtime.InteropServices.RuntimeInformation]::OSArchitecture.ToString() }
    catch { $Architecture = if ($env:PROCESSOR_ARCHITEW6432) { $env:PROCESSOR_ARCHITEW6432 } else { $env:PROCESSOR_ARCHITECTURE } }
    if ($Architecture -notin @('X64', 'AMD64')) { throw "Native Windows supports x64 only, not $Architecture. Windows ARM64 and 32-bit builds are not shipped." }
    $Build = [int](Get-ItemProperty -LiteralPath 'HKLM:\SOFTWARE\Microsoft\Windows NT\CurrentVersion' -Name CurrentBuildNumber).CurrentBuildNumber
    if ($Build -lt 17763) { throw 'Alter Zero needs Windows 10 version 1809 (build 17763) or newer for ConPTY.' }
    if (-not $InstallDir) {
        if (-not $env:LOCALAPPDATA) { throw 'LOCALAPPDATA is missing; set ALTER_ZERO_INSTALL_DIR to a per-user writable directory.' }
        $InstallDir = Join-Path $env:LOCALAPPDATA 'Alter Zero\bin'
    }
    $InstallDir = [IO.Path]::GetFullPath($InstallDir)
    if (-not $BaseUrl) { $BaseUrl = 'https://github.com/linuztx/alter-zero' }
    $BaseUrl = $BaseUrl.TrimEnd('/')
    $Uri = $null
    if (-not [Uri]::TryCreate($BaseUrl, [UriKind]::Absolute, [ref]$Uri) -or $Uri.Scheme -notin @('https', 'http') -or $Uri.Query -or $Uri.Fragment) {
        throw 'ALTER_ZERO_INSTALL_BASE_URL must be an absolute HTTP(S) repository URL without a query or fragment.'
    }
    [Net.ServicePointManager]::SecurityProtocol = [Net.ServicePointManager]::SecurityProtocol -bor [Net.SecurityProtocolType]::Tls12
    if (-not $Version) { $Version = Get-AlterZeroTag $BaseUrl }
    $Version = $Version -creplace '^v', ''
    if ($Version -cnotmatch '^(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)(-[0-9A-Za-z.-]+)?(\+[0-9A-Za-z.-]+)?$') { throw "Not a release version: '$Version'." }
    $Stem = "alter-zero-v$Version-x86_64-pc-windows-msvc"
    $Asset = "$Stem.zip"
    $Work = Join-Path ([IO.Path]::GetTempPath()) ('alter-zero-install-' + [guid]::NewGuid().ToString('N'))
    $null = [IO.Directory]::CreateDirectory($Work)
    try {
        Write-Host "Downloading Alter Zero v$Version (Windows x64)..."
        $Archive = Join-Path $Work $Asset
        $Sums = Join-Path $Work 'SHA256SUMS'
        Get-AlterZeroFile "$BaseUrl/releases/download/v$Version/$Asset" $Archive
        Get-AlterZeroFile "$BaseUrl/releases/download/v$Version/SHA256SUMS" $Sums
        $Hashes = @()
        foreach ($Line in [IO.File]::ReadAllLines($Sums)) {
            if (-not $Line.Trim()) { continue }
            if ($Line -cnotmatch '^([0-9a-fA-F]{64})  ([^\s/\\:]+)$') { throw 'Invalid SHA256SUMS: checksum and plain asset file name required.' }
            if ($Matches[2] -ceq $Asset) { $Hashes += $Matches[1] }
        }
        if ($Hashes.Count -ne 1) { throw "SHA256SUMS must list $Asset exactly once. Nothing was installed." }
        if ((Get-FileHash -LiteralPath $Archive -Algorithm SHA256).Hash -ine $Hashes[0]) { throw 'Release checksum mismatch. Nothing was installed.' }
        Write-Host 'SHA256 checksum matches the published value.'
        $Stage = Join-Path $Work 'stage'
        Expand-AlterZeroZip $Archive $Stem $Stage
        $Binary = Join-Path $Stage 'alter-zero.exe'
        Test-AlterZeroPE $Binary
        Test-AlterZeroVersion $Binary $Version
        $null = [IO.Directory]::CreateDirectory($InstallDir)
        $Destination = Join-Path $InstallDir 'alter-zero.exe'
        $Pending = Join-Path $InstallDir ('.alter-zero-' + [guid]::NewGuid().ToString('N') + '.exe')
        $Backup = Join-Path $InstallDir ('.alter-zero-backup-' + [guid]::NewGuid().ToString('N') + '.exe')
        $HadOld = [IO.File]::Exists($Destination)
        $Replaced = $false
        try {
            if ($HadOld) {
                if (((Get-Item -LiteralPath $Destination -Force).Attributes -band [IO.FileAttributes]::ReparsePoint) -ne 0) { throw 'Refusing to replace an executable symlink/reparse point.' }
                try {
                    # Windows cannot rename over a running or locked exe. Probe
                    # explicitly, and still catch Replace's race-time failure.
                    $Probe = [IO.File]::Open($Destination, [IO.FileMode]::Open, [IO.FileAccess]::ReadWrite, [IO.FileShare]::None)
                    $Probe.Dispose()
                } catch { throw "The installed executable is locked, running, or not writable. Close all Alter Zero instances and rerun the installer. $($_.Exception.Message)" }
            }
            [IO.File]::Copy($Binary, $Pending, $false)
            try {
                if ($HadOld) { [IO.File]::Replace($Pending, $Destination, $Backup) }
                else { [IO.File]::Move($Pending, $Destination) }
            } catch { throw "Could not replace $Destination. It may be locked or running; close all Alter Zero instances and retry. $($_.Exception.Message)" }
            $Replaced = $true
            Test-AlterZeroVersion $Destination $Version
            if ((Get-FileHash -LiteralPath $Destination).Hash -cne (Get-FileHash -LiteralPath $Binary).Hash) { throw 'Installed executable failed its final checksum check.' }
        } catch {
            $Failure = $_
            if ($Replaced) {
                try {
                    if ($HadOld) { [IO.File]::Replace($Backup, $Destination, $null) }
                    else { [IO.File]::Delete($Destination) }
                } catch { [Console]::Error.WriteLine("Could not roll back; old binary, if present, is at $Backup. $($_.Exception.Message)") }
            }
            throw $Failure
        } finally {
            if ([IO.File]::Exists($Pending)) { [IO.File]::Delete($Pending) }
        }
        if ([IO.File]::Exists($Backup)) { [IO.File]::Delete($Backup) }
        Write-Host "Installed Alter Zero v$Version to $Destination"
        Write-Host 'Git for Windows is required (not WSL). Windows Terminal is recommended.'
        $QuotedDir = $InstallDir.Replace("'", "''")
        if (($env:Path -split ';') -notcontains $InstallDir) {
            Write-Host "This directory is not on your PATH. Add it to your user PATH, or for this session:"
            Write-Host "  `$env:Path = '$QuotedDir;' + `$env:Path"
        }
        Write-Host "Start: & '$QuotedDir\alter-zero.exe'"
        Write-Host "Uninstall: Remove-Item -LiteralPath '$QuotedDir\alter-zero.exe'"
    } finally {
        if (Test-Path -LiteralPath $Work) { Remove-Item -LiteralPath $Work -Recurse -Force }
    }
}

try { Install-AlterZero } catch { [Console]::Error.WriteLine("alter-zero: $($_.Exception.Message)"); exit 1 }
