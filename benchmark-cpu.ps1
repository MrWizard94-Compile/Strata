param(
    [ValidateScript({ -not $_.Contains('"') })]
    [string]$ReportPath = ('evidence/cpu-' + (Get-Date -Format 'yyyyMMdd-HHmmss') + '-' + [Guid]::NewGuid().ToString('N').Substring(0, 8) + '.json'),
    [switch]$SkipBuild
)
$ErrorActionPreference = 'Stop'
Push-Location -LiteralPath $PSScriptRoot
$strataLabelNames = @('STRATA_BUILD_PROFILE', 'STRATA_RUSTC_VERSION', 'STRATA_SOURCE_REVISION', 'STRATA_HARDWARE_LABEL')
$strataPreviousLabels = @{}
foreach ($strataLabelName in $strataLabelNames) {
    $strataPreviousLabels[$strataLabelName] = [Environment]::GetEnvironmentVariable($strataLabelName, 'Process')
}
try {
    if (-not $SkipBuild) {
        cargo build --release --locked --target x86_64-pc-windows-msvc
        if ($LASTEXITCODE -ne 0) { throw 'Release build failed; benchmark not run.' }
    }
    $strataOutput = if ([IO.Path]::IsPathRooted($ReportPath)) {
        [IO.Path]::GetFullPath($ReportPath)
    } else { [IO.Path]::GetFullPath((Join-Path $PSScriptRoot $ReportPath)) }
    $strataProvenance = "$strataOutput.provenance.json"
    if ((Test-Path -LiteralPath $strataOutput) -or (Test-Path -LiteralPath $strataProvenance)) {
        throw 'Choose a new report path; existing results and provenance are preserved.'
    }
    New-Item -ItemType Directory -Path ([IO.Path]::GetDirectoryName($strataOutput)) -Force | Out-Null
    $strataBinary = Join-Path $PSScriptRoot 'target/x86_64-pc-windows-msvc/release/strata.exe'
    $strataInputs = @('Cargo.toml', 'Cargo.lock', 'rust-toolchain.toml')
    $strataInputs += @(Get-ChildItem -LiteralPath (Join-Path $PSScriptRoot 'src') -File -Recurse |
        ForEach-Object { $_.FullName.Substring($PSScriptRoot.Length + 1).Replace('\', '/') })
    $strataHashes = @($strataInputs | Sort-Object | ForEach-Object {
        [ordered]@{ path = $_; sha256 = (Get-FileHash -LiteralPath (Join-Path $PSScriptRoot $_) -Algorithm SHA256).Hash }
    })
    $strataCommit = git rev-parse --verify HEAD 2>$null
    if ($LASTEXITCODE -ne 0) { $strataCommit = $null }
    $strataChanges = @(git status --porcelain)
    $strataHardware = [ordered]@{
        cpu = (Get-CimInstance Win32_Processor | Select-Object Name, NumberOfCores, NumberOfLogicalProcessors)
        ram_bytes = (Get-CimInstance Win32_ComputerSystem).TotalPhysicalMemory
        os = (Get-CimInstance Win32_OperatingSystem | Select-Object Caption, Version, BuildNumber)
        gpu = (Get-CimInstance Win32_VideoController | Select-Object Name, DriverVersion)
        power_scheme = (powercfg /getactivescheme | Out-String).Trim()
    }
    $strataCompiler = (rustc --version | Out-String).Trim()
    $strataRecord = [ordered]@{
        recorded_utc = [DateTime]::UtcNow.ToString('o')
        build_profile = 'release'
        build_performed_by_this_invocation = -not $SkipBuild
        compiler = $strataCompiler
        git_commit = $strataCommit
        working_tree_changes = $strataChanges
        binary_sha256 = (Get-FileHash -LiteralPath $strataBinary -Algorithm SHA256).Hash
        source_inputs = $strataHashes
        hardware = $strataHardware
        limits = 'Source inputs are observed after build; SkipBuild requires the caller to establish binary/source agreement. No process isolation, CPU affinity, thermal normalization or GPU/frame-time measurement.'
    }
    $strataBytes = [Text.Encoding]::UTF8.GetBytes(($strataRecord | ConvertTo-Json -Depth 7))
    $strataFile = [IO.File]::Open($strataProvenance, [IO.FileMode]::CreateNew, [IO.FileAccess]::Write)
    try { $strataFile.Write($strataBytes, 0, $strataBytes.Length); $strataFile.Flush($true) } finally { $strataFile.Dispose() }
    $env:STRATA_BUILD_PROFILE = 'release'
    $env:STRATA_RUSTC_VERSION = $strataCompiler
    $env:STRATA_SOURCE_REVISION = "See provenance SHA256 $((Get-FileHash -LiteralPath $strataProvenance).Hash)"
    $env:STRATA_HARDWARE_LABEL = "$($strataHardware.cpu.Name); $($strataHardware.ram_bytes) RAM bytes; Windows $($strataHardware.os.Version); $($strataHardware.power_scheme)"
    $strataProcess = Start-Process -FilePath $strataBinary -WorkingDirectory $PSScriptRoot `
        -ArgumentList @('--benchmark-cpu', ('"' + $strataOutput.Replace('\', '/') + '"')) `
        -WindowStyle Hidden -RedirectStandardOutput "$strataOutput.stdout.log" `
        -RedirectStandardError "$strataOutput.stderr.log" -PassThru -Wait
    if ($strataProcess.ExitCode -ne 0) {
        throw "CPU benchmark failed: $(Get-Content -LiteralPath "$strataOutput.stderr.log" -Raw)"
    }
    if (-not (Test-Path -LiteralPath $strataOutput -PathType Leaf)) { throw 'Benchmark exited without a report.' }
    Write-Output "CPU benchmark: $strataOutput"
    Write-Output "Provenance: $strataProvenance"
} finally {
    foreach ($strataLabelName in $strataLabelNames) {
        [Environment]::SetEnvironmentVariable($strataLabelName, $strataPreviousLabels[$strataLabelName], 'Process')
    }
    Pop-Location
}
