param(
    [string]$Executable,
    [string]$ZipName,
    [string]$Guide = 'PLAYING.md'
)
# Packages the Windows release executable and checks the file a player would unzip.
# Subsystem 2 is IMAGE_SUBSYSTEM_WINDOWS_GUI: Windows does not open a console.
$ErrorActionPreference = 'Stop'

function Assert-NewPackagePaths([string]$ZipPath, [string]$ChecksumPath) {
    foreach ($path in @($ZipPath, $ChecksumPath)) {
        if (Test-Path -LiteralPath $path) {
            throw "Package output already exists; refusing to replace it: $path"
        }
    }
}

function Get-NewPackagePaths([string]$Name, [string]$OutputDirectory) {
    # Keep both files in the same explicit output directory. The restricted
    # leaf name also excludes Windows device names, streams and path aliases.
    if ($Name -notmatch '^[A-Za-z0-9][A-Za-z0-9._ -]*\.zip$' -or
        $Name -match '^(CON|PRN|AUX|NUL|COM[1-9]|LPT[1-9])(?:\.|$)') {
        throw 'ZipName must be a non-device .zip leaf name using letters, digits, spaces, dots, underscores or hyphens.'
    }
    $resolved = Resolve-Path -LiteralPath $OutputDirectory
    if ($resolved.Provider.Name -ne 'FileSystem' -or
        -not (Get-Item -LiteralPath $resolved.ProviderPath).PSIsContainer) {
        throw "Package output must be an existing filesystem directory: $OutputDirectory"
    }
    $zipPath = Join-Path $resolved.ProviderPath $Name
    $checksumPath = "$zipPath.sha256"
    Assert-NewPackagePaths $zipPath $checksumPath
    return [pscustomobject]@{ Zip = $zipPath; Checksum = $checksumPath }
}

function Resolve-PackageStage([string]$Stage, [string]$TempRoot) {
    $rootPath = Resolve-Path -LiteralPath $TempRoot
    $stagePath = Resolve-Path -LiteralPath $Stage
    if ($rootPath.Provider.Name -ne 'FileSystem' -or $stagePath.Provider.Name -ne 'FileSystem') {
        throw 'Package cleanup requires filesystem paths.'
    }
    $rootItem = Get-Item -LiteralPath $rootPath.ProviderPath -Force
    $stageItem = Get-Item -LiteralPath $stagePath.ProviderPath -Force
    if (-not $rootItem.PSIsContainer -or -not $stageItem.PSIsContainer -or
        ($stageItem.Attributes -band [System.IO.FileAttributes]::ReparsePoint) -ne 0 -or
        $stageItem.Name -notmatch '^strata-win-[0-9a-f]{32}$' -or
        -not [string]::Equals($stageItem.Parent.FullName, $rootItem.FullName, [StringComparison]::OrdinalIgnoreCase)) {
        throw "Refusing package cleanup outside its direct temporary staging directory: $Stage"
    }
    return $stageItem.FullName
}

function Publish-PackageFiles([string]$ValidatedZip, [string]$ZipPath, [string]$ChecksumPath) {
    Assert-NewPackagePaths $ZipPath $ChecksumPath
    $id = [guid]::NewGuid().ToString('N')
    $zipCandidate = "$ZipPath.$id.partial"
    $checksumCandidate = "$ChecksumPath.$id.partial"
    $zipPublished = $false
    $checksumPublished = $false
    try {
        # Copy into uniquely named candidates beside the final outputs. A copy
        # failure leaves an explicitly partial file, never a final-looking zip.
        $source = [System.IO.File]::OpenRead($ValidatedZip)
        try {
            $candidate = [System.IO.File]::Open($zipCandidate, [System.IO.FileMode]::CreateNew, [System.IO.FileAccess]::Write, [System.IO.FileShare]::None)
            try { $source.CopyTo($candidate); $candidate.Flush($true) }
            finally { $candidate.Dispose() }
        }
        finally { $source.Dispose() }
        $hash = (Get-FileHash -Algorithm SHA256 -LiteralPath $zipCandidate).Hash.ToLowerInvariant()
        $leaf = [System.IO.Path]::GetFileName($ZipPath)
        $bytes = [System.Text.UTF8Encoding]::new($false).GetBytes("$hash  $leaf`n")
        $candidate = [System.IO.File]::Open($checksumCandidate, [System.IO.FileMode]::CreateNew, [System.IO.FileAccess]::Write, [System.IO.FileShare]::None)
        try { $candidate.Write($bytes, 0, $bytes.Length); $candidate.Flush($true) }
        finally { $candidate.Dispose() }

        Assert-NewPackagePaths $ZipPath $ChecksumPath
        # The two-argument Move refuses an existing destination, including one
        # created after the preflight. Each rename stays on the output volume.
        # The pair cannot be renamed atomically; retain a completed first file
        # and any candidates if the second rename fails, and report the state.
        [System.IO.File]::Move($zipCandidate, $ZipPath)
        $zipPublished = $true
        [System.IO.File]::Move($checksumCandidate, $ChecksumPath)
        $checksumPublished = $true
        return $hash
    }
    catch {
        throw "Package publication failed without replacing existing outputs. ZIP published by this attempt: $zipPublished; checksum published: $checksumPublished. Candidate files, if created, remain at '$zipCandidate' and '$checksumCandidate'. Cause: $($_.Exception.Message)"
    }
}

function Assert-GuiSubsystem([string]$Path) {
    $bytes = [System.IO.File]::ReadAllBytes($Path)
    if ($bytes.Length -lt 96) { throw "Executable is too small to be a PE file: $Path" }
    $pe = [BitConverter]::ToInt32($bytes, 0x3C)
    if ($pe -lt 0 -or ($pe + 96) -gt $bytes.Length) { throw "Invalid PE header offset: $pe" }
    $signature = [BitConverter]::ToUInt32($bytes, $pe)
    if ($signature -ne 0x00004550) { throw 'Missing PE signature.' }
    $magic = [BitConverter]::ToUInt16($bytes, $pe + 24)
    if ($magic -ne 0x20B) { throw "Expected a PE32+ executable, found magic 0x$($magic.ToString('X'))." }
    $subsystem = [BitConverter]::ToUInt16($bytes, $pe + 24 + 68)
    if ($subsystem -ne 2) {
        throw "Subsystem is $subsystem; expected 2 (Windows GUI, no console window)."
    }
}

function Get-DumpbinPath {
    # DLL imports are not a fixed header field. dumpbin is the Visual Studio
    # tool that reads them; a hand-rolled import parser is not a substitute.
    $installs = @()
    $pfx86 = ${env:ProgramFiles(x86)}
    if ($pfx86) {
    $vswhere = Join-Path $pfx86 'Microsoft Visual Studio\Installer\vswhere.exe'
    if (Test-Path -LiteralPath $vswhere) {
        foreach ($vswhereArgs in @(
                @('-latest', '-products', '*', '-requires', 'Microsoft.VisualStudio.Component.VC.Tools.x86.x64', '-property', 'installationPath'),
                @('-latest', '-property', 'installationPath')
            )) {
            $found = & $vswhere @vswhereArgs
            if ($LASTEXITCODE -eq 0 -and $found) {
                $installs += ($found | Select-Object -First 1).ToString().Trim()
            }
        }
    }
    }
    $seen = @{}
    foreach ($install in $installs) {
        if (-not $install -or $seen.ContainsKey($install)) { continue }
        $seen[$install] = $true
        $msvc = Join-Path $install 'VC\Tools\MSVC'
        if (-not (Test-Path -LiteralPath $msvc)) { continue }
        $preferred = Get-ChildItem -Path $msvc -Recurse -Filter dumpbin.exe -ErrorAction SilentlyContinue |
            Where-Object { $_.FullName -match 'Hostx64\\x64\\dumpbin\.exe$' } |
            Select-Object -First 1
        if ($preferred) { return $preferred.FullName }
        $any = Get-ChildItem -Path $msvc -Recurse -Filter dumpbin.exe -ErrorAction SilentlyContinue |
            Select-Object -First 1
        if ($any) { return $any.FullName }
    }
    $looked = if ($installs.Count) { $installs -join '; ' } else { 'no Visual Studio installation from vswhere' }
    throw "dumpbin.exe was not found. Looked in: $looked"
}

function Assert-NoVcRedist([string]$Path) {
    $dumpbin = Get-DumpbinPath
    Write-Host "Using dumpbin: $dumpbin"
    $listing = & $dumpbin /nologo /dependents $Path 2>&1 | Out-String
    if ($LASTEXITCODE -ne 0) { Write-Host $listing; throw 'dumpbin failed.' }
    $forbidden = @(
        'vcruntime140.dll', 'vcruntime140_1.dll', 'msvcp140.dll', 'msvcp140_1.dll',
        'msvcp140_2.dll', 'concrt140.dll', 'vcomp140.dll'
    )
    $lower = $listing.ToLowerInvariant()
    foreach ($dll in $forbidden) {
        if ($lower.Contains($dll)) {
            Write-Host $listing
            throw "The Windows exe imports $dll. The zip must run without the Visual C++ redistributable."
        }
    }
    Write-Host 'DLL imports (no Visual C++ redistributable):'
    foreach ($line in ($listing -split "`r?`n")) {
        if ($line -match '\.dll') { Write-Host $line.Trim() }
    }
}

function Invoke-PackagedStrata([string]$Exe, [string]$Work, [string[]]$ArgumentList, [int]$ExpectExit) {
    $stdout = Join-Path $Work 'stdout.log'
    $stderr = Join-Path $Work 'stderr.log'
    $proc = Start-Process -FilePath $Exe -WorkingDirectory $Work -ArgumentList $ArgumentList `
        -Wait -PassThru -WindowStyle Hidden -RedirectStandardOutput $stdout -RedirectStandardError $stderr
    $outText = if (Test-Path -LiteralPath $stdout) { Get-Content -LiteralPath $stdout -Raw } else { '' }
    $errText = if (Test-Path -LiteralPath $stderr) { Get-Content -LiteralPath $stderr -Raw } else { '' }
    if ($null -eq $proc.ExitCode -or $proc.ExitCode -ne $ExpectExit) {
        Write-Host $outText
        Write-Host $errText
        throw "strata exited $($proc.ExitCode); expected $ExpectExit."
    }
    return [string]$outText + [string]$errText
}

# Executing the file packages a player zip. Dot-sourcing only defines the
# checks, so a test can call Assert-GuiSubsystem without Visual Studio.
# A return here would leave the caller's scope, so the main path is skipped.
if ($MyInvocation.InvocationName -ne '.') {
if (-not $Executable -or -not $ZipName) { throw 'Pass -Executable and -ZipName.' }
$outputs = Get-NewPackagePaths $ZipName (Get-Location).Path
if (-not (Test-Path -LiteralPath $Executable -PathType Leaf)) { throw "Executable not found: $Executable" }
if (-not (Test-Path -LiteralPath $Guide -PathType Leaf)) { throw "Player guide not found: $Guide" }

$built = (Resolve-Path -LiteralPath $Executable).Path
Assert-GuiSubsystem $built
Assert-NoVcRedist $built

$tempRoot = (Resolve-Path -LiteralPath ([System.IO.Path]::GetTempPath())).ProviderPath
$stage = Join-Path $tempRoot ('strata-win-' + [guid]::NewGuid().ToString('N'))
New-Item -ItemType Directory -Path $stage | Out-Null
$packageFailed = $false
try {
    $stage = Resolve-PackageStage $stage $tempRoot
    Copy-Item -LiteralPath $built -Destination (Join-Path $stage 'strata.exe')
    Copy-Item -LiteralPath $Guide -Destination (Join-Path $stage 'PLAYING.md')
    $validatedZip = Join-Path $stage 'candidate.zip'
    & tar -a -c -f $validatedZip -C $stage strata.exe PLAYING.md
    if ($LASTEXITCODE -ne 0) { throw 'Could not create the Windows zip.' }

    $unpack = Join-Path $stage 'unpack'
    New-Item -ItemType Directory -Path $unpack | Out-Null
    & tar -x -f $validatedZip -C $unpack
    if ($LASTEXITCODE -ne 0) { throw 'Could not read the Windows zip back.' }
    $names = @(Get-ChildItem -LiteralPath $unpack -Force | ForEach-Object { $_.Name } | Sort-Object)
    $expected = @('PLAYING.md', 'strata.exe')
    if (($names -join ',') -ne ($expected -join ',')) {
        throw "Windows zip contains [$($names -join ', ')]; expected only strata.exe and PLAYING.md."
    }
    $packed = Join-Path $unpack 'strata.exe'
    if ((Get-Item -LiteralPath $packed).Length -lt 100000) { throw 'Packaged strata.exe is unexpectedly small.' }
    Assert-GuiSubsystem $packed

    $help = Invoke-PackagedStrata $packed $unpack @('--help') 2
    if ($help -notmatch 'STRATA') { throw 'The packaged exe did not print its STRATA help text.' }
    $export = Join-Path $unpack 'materials'
    $exported = Invoke-PackagedStrata $packed $unpack @('--export-materials', $export) 0
    if ($exported -notmatch 'STRATA_MATERIAL_EXPORT') { throw 'The packaged exe did not finish a material export.' }
    $pngs = @(Get-ChildItem -LiteralPath $export -Filter '*.png' -File)
    if ($pngs.Count -lt 18) { throw "Material export wrote $($pngs.Count) PNGs; expected the 17 runtime tiles and contact sheet." }
    if (-not (Test-Path -LiteralPath (Join-Path $export 'contact-sheet.png') -PathType Leaf)) {
        throw 'Material export did not write contact-sheet.png.'
    }
    if (-not (Test-Path -LiteralPath (Join-Path $export 'manifest.json') -PathType Leaf)) {
        throw 'Material export did not write manifest.json.'
    }
    Write-Host 'Packaged Windows exe launched with no console subsystem and wrote its own materials.'
    $hash = Publish-PackageFiles $validatedZip $outputs.Zip $outputs.Checksum
    Write-Host "$hash  $ZipName"
}
catch {
    $packageFailed = $true
    throw
}
finally {
    try {
        $cleanupPath = Resolve-PackageStage $stage $tempRoot
        Remove-Item -LiteralPath $cleanupPath -Recurse -Force
    }
    catch {
        if (-not $packageFailed) { throw }
        Write-Warning "Packaging failed and its temporary directory was retained: $stage. Cleanup error: $($_.Exception.Message)"
    }
}
}
