# Cross-platform, offline tests of the REAL installer's pure ZIP/PE helpers.
# pwsh -NoProfile -File scripts/windows/test-installer-functions.ps1
[CmdletBinding()]
param([string]$ScratchDir = [IO.Path]::GetTempPath())
$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest
$Root = Split-Path (Split-Path $PSScriptRoot -Parent) -Parent
$Tokens = $null
$Errors = $null
$Ast = [Management.Automation.Language.Parser]::ParseFile((Join-Path $Root 'install.ps1'), [ref]$Tokens, [ref]$Errors)
if ($Errors.Count) { throw ($Errors | Out-String) }
# Load only function ASTs; never invoke the platform-specific installation
# entry point or introduce a production bypass of its Windows OS checks.
foreach ($Function in $Ast.FindAll({ param($Node) $Node -is [Management.Automation.Language.FunctionDefinitionAst] }, $false)) {
    . ([scriptblock]::Create($Function.Extent.Text))
}
$Work = Join-Path $ScratchDir ('alter-zero-installer-functions-' + [guid]::NewGuid().ToString('N'))
$Pass = 0
function Assert-True([bool]$Condition, [string]$Label) {
    if (-not $Condition) { throw "FAIL: $Label" }
    $script:Pass++
    Write-Host "ok $Label"
}
function Must-Fail([scriptblock]$Action, [string]$Label) {
    $Failed = $false
    try { & $Action } catch { $Failed = $true }
    Assert-True $Failed $Label
}
try {
    $null = [IO.Directory]::CreateDirectory($Work)
    $Image = New-Object byte[] 1024
    $Memory = New-Object IO.MemoryStream(,$Image)
    $Writer = New-Object IO.BinaryWriter($Memory)
    $Writer.Write([uint16]0x5A4D)
    $Memory.Position = 0x3C; $Writer.Write([uint32]0x80)
    $Memory.Position = 0x80; $Writer.Write([uint32]0x4550)
    $Writer.Write([uint16]0x8664); $Writer.Write([uint16]1)
    $Memory.Position = 0x94; $Writer.Write([uint16]240); $Writer.Write([uint16]0x22)
    $Writer.Write([uint16]0x20B)
    $Memory.Position = 0xD4; $Writer.Write([uint32]512)
    $Memory.Position = 0xDC; $Writer.Write([uint16]3)
    $Memory.Position = 0x198; $Writer.Write([uint32]512); $Writer.Write([uint32]512)
    $Writer.Flush()
    $Image = $Memory.ToArray()
    $Writer.Dispose(); $Memory.Dispose()
    $Exe = Join-Path $Work 'fixture.exe'
    [IO.File]::WriteAllBytes($Exe, $Image)
    Test-AlterZeroPE $Exe
    Assert-True $true 'valid numeric PE32+ x64 console image'
    foreach ($Mutation in @(
        @(0, [uint16]0), @(0x84, [uint16]0x14C), @(0x84, [uint16]0xAA64),
        @(0x96, [uint16]0x2022), @(0x96, [uint16]0), @(0x98, [uint16]0x10B),
        @(0xDC, [uint16]2), @(0xDC, [uint16]0)
    )) {
        [byte[]]$Bad = $Image.Clone()
        [BitConverter]::GetBytes([uint16]$Mutation[1]).CopyTo($Bad, [int]$Mutation[0])
        [IO.File]::WriteAllBytes($Exe, $Bad)
        Must-Fail { Test-AlterZeroPE $Exe } "PE mutation at $($Mutation[0]) with $($Mutation[1]) refused"
    }
    [IO.File]::WriteAllBytes($Exe, $Image[0..149])
    Must-Fail { Test-AlterZeroPE $Exe } 'truncated PE refused'
    Add-Type -AssemblyName System.IO.Compression.FileSystem
    $ZipPath = Join-Path $Work 'fixture.zip'
    $Stem = 'alter-zero-v0.15.0-x86_64-pc-windows-msvc'
    foreach ($BadName in @('', '../escaped', '/absolute', 'C:/drive', '..\escape', "$Stem/README.md", "$Stem/README.md:stream")) {
        if ([IO.File]::Exists($ZipPath)) { [IO.File]::Delete($ZipPath) }
        $Zip = [IO.Compression.ZipFile]::Open($ZipPath, [IO.Compression.ZipArchiveMode]::Create)
        try {
            $null = $Zip.CreateEntry("$Stem/")
            foreach ($Name in @('alter-zero.exe', 'LICENSE', 'README.md', 'CHANGELOG.md')) {
                $Entry = $Zip.CreateEntry("$Stem/$Name")
                $Stream = $Entry.Open()
                try { $Stream.Write($Image, 0, $Image.Length) } finally { $Stream.Dispose() }
            }
            if ($BadName) { $null = $Zip.CreateEntry($BadName) }
        } finally { $Zip.Dispose() }
        $Destination = Join-Path $Work ([guid]::NewGuid().ToString('N'))
        if (-not $BadName) {
            Expand-AlterZeroZip $ZipPath $Stem $Destination
            Assert-True ((Get-ChildItem -LiteralPath $Destination -File).Count -eq 4) 'safe ZIP extracts exactly four files'
            $Hasher = [Security.Cryptography.SHA256]::Create()
            try { $ExpectedHash = [BitConverter]::ToString($Hasher.ComputeHash($Image)).Replace('-', '') } finally { $Hasher.Dispose() }
            Assert-True ((Get-FileHash -LiteralPath (Join-Path $Destination 'alter-zero.exe')).Hash -eq $ExpectedHash) 'ZIP binary bytes preserved'
        } else {
            Must-Fail { Expand-AlterZeroZip $ZipPath $Stem $Destination } "unsafe or duplicate ZIP entry '$BadName' refused"
            Assert-True (-not (Test-Path -LiteralPath $Destination)) 'invalid ZIP creates no extraction directory'
        }
    }
    Write-Host "$Pass PowerShell installer helper assertions passed (offline)."
} finally {
    if (Test-Path -LiteralPath $Work) { Remove-Item -LiteralPath $Work -Recurse -Force }
}
