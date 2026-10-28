$ErrorActionPreference = 'Stop'
. (Join-Path (Split-Path -Parent $PSScriptRoot) 'package-windows.ps1')

function Assert-Throws([scriptblock]$Action, [string]$MessagePattern) {
    $observed = $null
    try { & $Action | Out-Null }
    catch { $observed = $_.Exception.Message }
    if (-not $observed -or $observed -notlike $MessagePattern) {
        throw "Expected '$MessagePattern', received '$observed'."
    }
}

$tempRoot = (Resolve-Path -LiteralPath ([System.IO.Path]::GetTempPath())).ProviderPath
$testRoot = Join-Path $tempRoot ('strata-package-tests-' + [guid]::NewGuid().ToString('N'))
New-Item -ItemType Directory -Path $testRoot | Out-Null
try {
    foreach ($name in @('', '..\outside.zip', '../outside.zip', 'C:\outside.zip',
            'folder/archive.zip', 'archive.zip:stream', 'CON.zip', 'nul.zip',
            'LPT1.zip', 'archive.zip.', 'archive.zip ', 'archive.tar')) {
        Assert-Throws { Get-NewPackagePaths $name $testRoot } '*leaf name*'
    }
    $paths = Get-NewPackagePaths 'strata safety.zip' $testRoot
    if ([System.IO.Path]::GetDirectoryName($paths.Zip) -ne $testRoot -or
        $paths.Checksum -ne "$($paths.Zip).sha256") {
        throw 'ZIP and checksum did not stay together in the requested output directory.'
    }
    [System.IO.File]::WriteAllText($paths.Zip, 'existing ZIP owner')
    Assert-Throws { Get-NewPackagePaths 'strata safety.zip' $testRoot } '*already exists*'
    if ([System.IO.File]::ReadAllText($paths.Zip) -ne 'existing ZIP owner') { throw 'Existing ZIP was changed.' }
    $checksumOnly = Join-Path $testRoot 'checksum-only.zip.sha256'
    [System.IO.File]::WriteAllText($checksumOnly, 'existing checksum owner')
    Assert-Throws { Get-NewPackagePaths 'checksum-only.zip' $testRoot } '*already exists*'
    if ([System.IO.File]::ReadAllText($checksumOnly) -ne 'existing checksum owner') { throw 'Existing checksum was changed.' }

    $stage = Join-Path $testRoot ('strata-win-' + [guid]::NewGuid().ToString('N'))
    New-Item -ItemType Directory -Path $stage | Out-Null
    if ((Resolve-PackageStage $stage $testRoot) -ne $stage) { throw 'Valid stage did not resolve.' }
    Assert-Throws { Resolve-PackageStage $testRoot $testRoot } '*Refusing package cleanup*'
    $other = Join-Path $testRoot 'other'
    New-Item -ItemType Directory -Path $other | Out-Null
    Assert-Throws { Resolve-PackageStage $stage $other } '*Refusing package cleanup*'
    Assert-Throws { Resolve-PackageStage $other $testRoot } '*Refusing package cleanup*'
    $nested = Join-Path $other ('strata-win-' + [guid]::NewGuid().ToString('N'))
    New-Item -ItemType Directory -Path $nested | Out-Null
    Assert-Throws { Resolve-PackageStage $nested $testRoot } '*Refusing package cleanup*'
    $junction = Join-Path $testRoot ('strata-win-' + [guid]::NewGuid().ToString('N'))
    New-Item -ItemType Junction -Path $junction -Target $other | Out-Null
    try { Assert-Throws { Resolve-PackageStage $junction $testRoot } '*Refusing package cleanup*' }
    finally { Remove-Item -LiteralPath $junction -Force }

    # Exercise publication separately from executable/package validation. No
    # executable is launched: these bytes stand for an already validated ZIP.
    $source = Join-Path $testRoot 'validated-source.bin'
    $sourceBytes = [byte[]](0..255)
    [System.IO.File]::WriteAllBytes($source, $sourceBytes)
    $published = Get-NewPackagePaths 'complete.zip' $testRoot
    $hash = Publish-PackageFiles $source $published.Zip $published.Checksum
    $actual = (Get-FileHash -LiteralPath $published.Zip -Algorithm SHA256).Hash.ToLowerInvariant()
    if ($hash -ne $actual -or
        [Convert]::ToBase64String([System.IO.File]::ReadAllBytes($published.Zip)) -ne [Convert]::ToBase64String($sourceBytes) -or
        [System.IO.File]::ReadAllText($published.Checksum) -ne "$hash  complete.zip`n") {
        throw 'Published ZIP bytes or checksum differ from the validated source.'
    }
    Assert-Throws { Publish-PackageFiles $source $published.Zip $published.Checksum } '*already exists*'
    if ((Get-ChildItem -LiteralPath $testRoot -Filter '*.partial').Count -ne 0) { throw 'Successful publication left partial candidates.' }

    # Deterministically introduce another writer after the initial preflight.
    # It must keep its final file while this attempt retains named candidates.
    $racing = Get-NewPackagePaths 'racing.zip' $testRoot
    function Get-FileHash([string]$Algorithm, [string]$LiteralPath) {
        [System.IO.File]::WriteAllText($racing.Checksum, 'competing checksum owner')
        Microsoft.PowerShell.Utility\Get-FileHash -Algorithm $Algorithm -LiteralPath $LiteralPath
    }
    try {
        Assert-Throws { Publish-PackageFiles $source $racing.Zip $racing.Checksum } '*Candidate files*'
    }
    finally { Remove-Item -LiteralPath Function:Get-FileHash }
    if ((Test-Path -LiteralPath $racing.Zip) -or
        [System.IO.File]::ReadAllText($racing.Checksum) -ne 'competing checksum owner' -or
        (Get-ChildItem -LiteralPath $testRoot -Filter '*.partial').Count -ne 2) {
        throw 'Competing output was changed or partial candidates were not distinguished.'
    }
    Write-Host 'Package safety checks passed: names, existing outputs, stage containment/junction rejection, byte/checksum publication and competing output preservation.'
}
finally {
    $resolvedRoot = Get-Item -LiteralPath (Resolve-Path -LiteralPath $testRoot).ProviderPath -Force
    $resolvedTemp = Get-Item -LiteralPath (Resolve-Path -LiteralPath $tempRoot).ProviderPath -Force
    if ($resolvedRoot.Name -notmatch '^strata-package-tests-[0-9a-f]{32}$' -or
        ($resolvedRoot.Attributes -band [System.IO.FileAttributes]::ReparsePoint) -ne 0 -or
        -not [string]::Equals($resolvedRoot.Parent.FullName, $resolvedTemp.FullName, [StringComparison]::OrdinalIgnoreCase)) {
        throw "Refusing test cleanup outside its temporary directory: $testRoot"
    }
    Remove-Item -LiteralPath $resolvedRoot.FullName -Recurse -Force
}
