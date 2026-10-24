# Native offline npm install: pack fixture packages, serve on loopback,
# install with scripts disabled, then run npm's Windows .cmd launcher.
[CmdletBinding()]
param(
    [Parameter(Mandatory = $true)][string]$BinaryPath,
    [string]$ScratchDir = [IO.Path]::GetTempPath()
)
$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest
if ([Environment]::OSVersion.Platform -ne [PlatformID]::Win32NT) { throw 'Native Windows is required.' }
$Root = Split-Path (Split-Path $PSScriptRoot -Parent) -Parent
$BinaryPath = (Resolve-Path -LiteralPath $BinaryPath).Path
$Npm = (Get-Command npm.cmd -ErrorAction Stop).Source
$Python = (Get-Command python -ErrorAction Stop).Source
$Node = (Get-Command node -ErrorAction Stop).Source
$Work = Join-Path $ScratchDir ('alter-zero-npm-test-' + [guid]::NewGuid().ToString('N'))
$Registry = Join-Path $Work 'registry'
$Prefix = Join-Path $Work 'prefix with spaces'
$Server = $null
$Saved = @{}
foreach ($Name in @('NPM_CONFIG_REGISTRY', 'NPM_CONFIG_CACHE', 'NPM_CONFIG_USERCONFIG', 'NPM_CONFIG_AUDIT', 'NPM_CONFIG_FUND', 'NPM_CONFIG_UPDATE_NOTIFIER', 'NPM_CONFIG_OFFLINE', 'ALTER_ZERO_TELEMETRY', 'ALTER_ZERO_CONFIG_DIR')) {
    $Saved[$Name] = [Environment]::GetEnvironmentVariable($Name, 'Process')
}
try {
    New-Item -ItemType Directory -Path $Registry -Force | Out-Null
    $env:NPM_CONFIG_CACHE = Join-Path $Work 'cache'
    $env:NPM_CONFIG_USERCONFIG = Join-Path $Work 'npmrc'
    [IO.File]::WriteAllText($env:NPM_CONFIG_USERCONFIG, '')
    $env:NPM_CONFIG_AUDIT = 'false'
    $env:NPM_CONFIG_FUND = 'false'
    $env:NPM_CONFIG_UPDATE_NOTIFIER = 'false'
    $env:NPM_CONFIG_OFFLINE = 'false'  # Network is restricted to our loopback stand-in.
    $env:ALTER_ZERO_TELEMETRY = '0'
    $env:ALTER_ZERO_CONFIG_DIR = Join-Path $Work 'config'
    # Fixture staging only, no npm publish. Linux selftest separately drives
    # the production npm.sh staging from all five verified release archives.
    $Stage = @'
const fs = require('fs'), path = require('path');
const [root, binary, work] = process.argv.slice(1);
const launcher = JSON.parse(fs.readFileSync(path.join(root, 'npm/package.json')));
const name = `${launcher.name}-win32-x64`;
if (launcher.optionalDependencies[name] !== launcher.version) throw Error('Windows pin/version mismatch');
const wrapper = path.join(work, 'launcher'), native = path.join(work, 'native');
for (const dir of [wrapper, native]) fs.mkdirSync(path.join(dir, 'bin'), {recursive: true});
delete launcher.scripts;
fs.writeFileSync(path.join(wrapper, 'package.json'), JSON.stringify(launcher));
fs.copyFileSync(path.join(root, 'npm/bin/alter-zero.js'), path.join(wrapper, 'bin/alter-zero.js'));
fs.writeFileSync(path.join(native, 'package.json'), JSON.stringify({name, version: launcher.version, os:['win32'], cpu:['x64'], files:['bin/alter-zero.exe'], license:'Apache-2.0'}));
fs.copyFileSync(binary, path.join(native, 'bin/alter-zero.exe'));
for (const dir of [wrapper, native]) fs.copyFileSync(path.join(root, 'LICENSE'), path.join(dir, 'LICENSE'));
'@
    & $Node -e $Stage $Root $BinaryPath $Work
    if ($LASTEXITCODE -ne 0) { throw 'Could not stage native npm fixtures.' }
    # Packing does not require a registry; point all npm commands at a closed
    # loopback port until the fixture server is ready as an extra safety net.
    $env:NPM_CONFIG_REGISTRY = 'http://127.0.0.1:9/'
    foreach ($Package in @('native', 'launcher')) {
        & $Npm pack (Join-Path $Work $Package) --ignore-scripts --pack-destination $Registry
        if ($LASTEXITCODE -ne 0) { throw "npm pack failed: $Package" }
    }
    $PortFile = Join-Path $Work 'port'
    $ErrorFile = Join-Path $Work 'server.err'
    $ServerScript = Join-Path $Root 'scripts/release/npm_registry.py'
    $Server = Start-Process -FilePath $Python -ArgumentList @(('"' + $ServerScript + '"'), ('"' + $Registry + '"')) -NoNewWindow -PassThru -RedirectStandardOutput $PortFile -RedirectStandardError $ErrorFile
    for ($i = 0; $i -lt 100; $i++) {
        if ($Server.HasExited) { throw "Registry failed: $(Get-Content -LiteralPath $ErrorFile -Raw)" }
        if ((Test-Path -LiteralPath $PortFile) -and (Get-Item -LiteralPath $PortFile).Length -gt 0) { break }
        Start-Sleep -Milliseconds 50
    }
    $Port = (Get-Content -LiteralPath $PortFile -Raw).Trim()
    if ($Port -notmatch '^[0-9]+$') { throw 'Registry did not print its port.' }
    $env:NPM_CONFIG_REGISTRY = "http://127.0.0.1:$Port/"
    $Manifest = Get-Content -LiteralPath (Join-Path $Root 'npm/package.json') -Raw | ConvertFrom-Json
    & $Npm install -g --prefix $Prefix --ignore-scripts --registry $env:NPM_CONFIG_REGISTRY "$($Manifest.name)@$($Manifest.version)"
    if ($LASTEXITCODE -ne 0) { throw 'Native offline npm install failed.' }
    $Command = Join-Path $Prefix 'alter-zero.cmd'
    $Version = @(& $Command --version)
    if ($LASTEXITCODE -ne 0 -or $Version.Count -ne 1 -or $Version[0] -cne "alter-zero $($Manifest.version)") { throw 'npm Windows command shim failed --version.' }
    & $Command --help
    if ($LASTEXITCODE -ne 0) { throw 'npm Windows command shim failed --help.' }
    $NativeBinary = Join-Path $Prefix "node_modules/$($Manifest.name)/node_modules/$($Manifest.name)-win32-x64/bin/alter-zero.exe"
    if ((Get-FileHash -LiteralPath $NativeBinary).Hash -cne (Get-FileHash -LiteralPath $BinaryPath).Hash) { throw 'npm installed bytes differ from the verified native executable.' }
    Write-Host 'Native Windows npm optional dependency, script-free install and .cmd launcher passed (offline).'
} finally {
    if ($null -ne $Server -and -not $Server.HasExited) { Stop-Process -Id $Server.Id -Force }
    foreach ($Name in $Saved.Keys) { [Environment]::SetEnvironmentVariable($Name, $Saved[$Name], 'Process') }
    if (Test-Path -LiteralPath $Work) { Remove-Item -LiteralPath $Work -Recurse -Force }
}
