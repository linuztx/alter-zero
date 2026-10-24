#requires -Version 5.1
# Alter Zero Windows installer. See docs/windows.md.
[CmdletBinding()]
param(
    [string]$Version = $env:ALTER_ZERO_VERSION,
    [string]$InstallDir = $env:ALTER_ZERO_INSTALL_DIR,
    [string]$BaseUrl = $env:ALTER_ZERO_INSTALL_BASE_URL
)

$ErrorActionPreference = 'Stop'
[Net.ServicePointManager]::SecurityProtocol = [Net.SecurityProtocolType]::Tls12
if (-not $BaseUrl) { $BaseUrl = 'https://github.com/linuztx/alter-zero' }
$BaseUrl = $BaseUrl.TrimEnd('/')
if (-not $InstallDir) { $InstallDir = Join-Path $env:LOCALAPPDATA 'Programs\Alter Zero' }

$architecture = $env:PROCESSOR_ARCHITEW6432
if (-not $architecture) { $architecture = $env:PROCESSOR_ARCHITECTURE }
$target = switch ($architecture) {
    'AMD64' { 'x86_64-pc-windows-msvc' }
    'ARM64' { 'aarch64-pc-windows-msvc' }
    default { throw "Unsupported Windows architecture: $architecture (x64 or ARM64 required)" }
}

if (-not $Version) {
    $reply = Invoke-WebRequest -UseBasicParsing -Uri "$BaseUrl/releases/latest"
    $uri = $reply.BaseResponse.ResponseUri
    if (-not $uri) { $uri = $reply.BaseResponse.RequestMessage.RequestUri }
    $Version = ($uri.AbsolutePath.TrimEnd('/') -split '/')[-1]
}
if ($Version -notmatch '^v?(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)(-[0-9A-Za-z.-]+)?$') {
    throw "Invalid release version: $Version"
}
$Version = $Version.TrimStart('v')
$stem = "alter-zero-v$Version-$target"
$asset = "$stem.tar.gz"
$url = "$BaseUrl/releases/download/v$Version"
$scratch = Join-Path ([IO.Path]::GetTempPath()) ("alter-zero-install-" + [guid]::NewGuid())
New-Item -ItemType Directory -Path $scratch | Out-Null
try {
    Write-Host "Installing Alter Zero v$Version ($target)"
    $archive = Join-Path $scratch $asset
    Invoke-WebRequest -UseBasicParsing -Uri "$url/$asset" -OutFile $archive
    $sums = (Invoke-WebRequest -UseBasicParsing -Uri "$url/SHA256SUMS").Content
    if ($sums -is [byte[]]) { $sums = [Text.Encoding]::UTF8.GetString($sums) }
    $pattern = '^([0-9a-fA-F]{64})  ' + [regex]::Escape($asset) + '$'
    $matching = @($sums -split '\r?\n' | Where-Object { $_ -match $pattern })
    if ($matching.Count -ne 1) { throw "SHA256SUMS must name $asset exactly once" }
    $expected = ($matching[0] -split '  ', 2)[0]
    if ((Get-FileHash -Algorithm SHA256 -LiteralPath $archive).Hash -ne $expected) {
        throw "SHA-256 mismatch for $asset; nothing was installed"
    }
    # Refuse extra entries and paths before extracting. The shared release
    # verifier enforces this same one-directory, four-file contract.
    $entries = @(& tar.exe -tzf $archive)
    if ($LASTEXITCODE -ne 0) { throw "Cannot read $asset" }
    $allowed = @($stem, "$stem/alter-zero.exe", "$stem/LICENSE", "$stem/README.md", "$stem/CHANGELOG.md")
    $normal = @($entries | ForEach-Object { $_.TrimEnd('/') })
    if ($normal.Count -ne 5 -or (Compare-Object ($normal | Sort-Object) ($allowed | Sort-Object))) {
        throw "Unexpected archive layout in $asset; nothing was extracted"
    }
    & tar.exe -xzf $archive -C $scratch
    if ($LASTEXITCODE -ne 0) { throw "Cannot extract $asset" }
    $binary = Join-Path $scratch "$stem/alter-zero.exe"
    $reported = & $binary --version
    if ($LASTEXITCODE -ne 0 -or $reported -ne "alter-zero $Version") {
        throw "The downloaded binary did not report alter-zero $Version"
    }
    New-Item -ItemType Directory -Force -Path $InstallDir | Out-Null
    $destination = Join-Path $InstallDir 'alter-zero.exe'
    $pending = Join-Path $InstallDir ("alter-zero.new-" + [guid]::NewGuid() + '.exe')
    # Finish copying on the destination filesystem before touching the install.
    Copy-Item -LiteralPath $binary -Destination $pending
    # Windows locks a running executable against overwrite. Renaming the old
    # file first also lets `alter-zero update` replace its own executable.
    $backup = Join-Path $InstallDir ("alter-zero.old-" + [guid]::NewGuid() + '.exe')
    $previous = Test-Path -LiteralPath $destination
    try {
        if ($previous) { Move-Item -LiteralPath $destination -Destination $backup }
        try { Move-Item -LiteralPath $pending -Destination $destination }
        catch {
            if ($previous) { Move-Item -LiteralPath $backup -Destination $destination }
            throw
        }
    }
    finally { Remove-Item -LiteralPath $pending -ErrorAction SilentlyContinue }
    if ($previous) { Remove-Item -LiteralPath $backup -ErrorAction SilentlyContinue }
    Write-Host "Installed $destination"
    if (-not (($env:PATH -split ';') -contains $InstallDir)) {
        Write-Host "Add $InstallDir to your user PATH, then open a new terminal."
    }
    Write-Host 'Install Git for Windows to enable Bash tools, hooks and ! commands.'
}
finally { Remove-Item -LiteralPath $scratch -Recurse -Force -ErrorAction SilentlyContinue }
