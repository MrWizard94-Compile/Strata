param(
    [string]$Executable,
    [string]$ZipName,
    [string]$Guide = 'PLAYING.md'
)
# Packages the Windows release executable and checks the file a player would unzip.
# Subsystem 2 is IMAGE_SUBSYSTEM_WINDOWS_GUI: Windows does not open a console.
$ErrorActionPreference = 'Stop'

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
if (-not (Test-Path -LiteralPath $Executable -PathType Leaf)) { throw "Executable not found: $Executable" }
if (-not (Test-Path -LiteralPath $Guide -PathType Leaf)) { throw "Player guide not found: $Guide" }

$built = (Resolve-Path -LiteralPath $Executable).Path
Assert-GuiSubsystem $built
Assert-NoVcRedist $built

$stage = Join-Path ([System.IO.Path]::GetTempPath()) ('strata-win-' + [guid]::NewGuid().ToString('N'))
New-Item -ItemType Directory -Path $stage | Out-Null
try {
    Copy-Item -LiteralPath $built -Destination (Join-Path $stage 'strata.exe')
    Copy-Item -LiteralPath $Guide -Destination (Join-Path $stage 'PLAYING.md')
    $zipFull = Join-Path (Get-Location) $ZipName
    if (Test-Path -LiteralPath $zipFull) { Remove-Item -LiteralPath $zipFull -Force }
    & tar -a -c -f $zipFull -C $stage strata.exe PLAYING.md
    if ($LASTEXITCODE -ne 0) { throw 'Could not create the Windows zip.' }

    $unpack = Join-Path $stage 'unpack'
    New-Item -ItemType Directory -Path $unpack | Out-Null
    & tar -x -f $zipFull -C $unpack
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
}
finally {
    Remove-Item -LiteralPath $stage -Recurse -Force
}

$hash = (Get-FileHash -Algorithm SHA256 -LiteralPath (Join-Path (Get-Location) $ZipName)).Hash.ToLowerInvariant()
$leaf = Split-Path -Leaf $ZipName
[System.IO.File]::WriteAllText((Join-Path (Get-Location) "$leaf.sha256"), "$hash  $leaf`n")
Write-Host "$hash  $leaf"
}
