param(
    [ValidateScript({ -not $_.Contains('"') })]
    [string]$TracePath = ('evidence/streaming-' + (Get-Date -Format 'yyyyMMdd-HHmmss') + '-' + [Guid]::NewGuid().ToString('N').Substring(0, 8) + '.jsonl'),
    [ValidateScript({ -not $_.Contains('"') })]
    [string]$SaveDirectory = 'saves/emerald-reach',
    [ValidateRange(1, 120)]
    [int]$SmokeSeconds = 30,
    [switch]$SkipBuild
)
$ErrorActionPreference = 'Stop'
$strataPreviousSource = [Environment]::GetEnvironmentVariable('STRATA_SOURCE_REVISION', 'Process')
Push-Location -LiteralPath $PSScriptRoot
try {
    if (-not $SkipBuild) {
        cargo build --release --locked
        if ($LASTEXITCODE -ne 0) { throw 'Release build failed; trace not run.' }
    }
    $strataOutput = if ([IO.Path]::IsPathRooted($TracePath)) {
        [IO.Path]::GetFullPath($TracePath)
    } else { [IO.Path]::GetFullPath((Join-Path $PSScriptRoot $TracePath)) }
    $strataProvenance = "$strataOutput.provenance.json"
    if ((Test-Path -LiteralPath $strataOutput) -or (Test-Path -LiteralPath $strataProvenance)) {
        throw 'Choose a new trace path; existing traces and provenance are preserved.'
    }
    New-Item -ItemType Directory -Path ([IO.Path]::GetDirectoryName($strataOutput)) -Force | Out-Null
    $strataInputs = @('Cargo.toml', 'Cargo.lock', 'rust-toolchain.toml', 'run.ps1', 'trace-streaming.ps1')
    $strataInputs += @(Get-ChildItem -LiteralPath (Join-Path $PSScriptRoot 'src') -File -Recurse |
        ForEach-Object { $_.FullName.Substring($PSScriptRoot.Length + 1).Replace('\', '/') })
    $strataHashes = @($strataInputs | Sort-Object | ForEach-Object {
        [ordered]@{ path = $_; sha256 = (Get-FileHash -LiteralPath (Join-Path $PSScriptRoot $_) -Algorithm SHA256).Hash }
    })
    $strataCommit = git rev-parse --verify --quiet HEAD
    if ($LASTEXITCODE -ne 0) { $strataCommit = $null }
    $strataRecord = [ordered]@{
        recorded_utc = [DateTime]::UtcNow.ToString('o')
        build_profile = 'release'
        build_performed_by_this_invocation = -not $SkipBuild
        compiler = (rustc --version | Out-String).Trim()
        git_commit = $strataCommit
        working_tree_changes = @(git status --porcelain)
        binary_sha256 = (Get-FileHash -LiteralPath 'target/release/strata.exe' -Algorithm SHA256).Hash
        source_inputs = $strataHashes
        hardware = [ordered]@{
            cpu = (Get-CimInstance Win32_Processor | Select-Object Name, NumberOfCores, NumberOfLogicalProcessors)
            ram_bytes = (Get-CimInstance Win32_ComputerSystem).TotalPhysicalMemory
            os = (Get-CimInstance Win32_OperatingSystem | Select-Object Caption, Version, BuildNumber)
            gpu = (Get-CimInstance Win32_VideoController | Select-Object Name, DriverVersion)
            power_scheme = (powercfg /getactivescheme | Out-String).Trim()
        }
        smoke_seconds = $SmokeSeconds
        limits = 'Source hashes are observed after build; SkipBuild requires binary/source agreement. Native title scene unless a human enters the world; record actual screen/camera rows. No automated traversal, GPU timestamps, total-memory measurement or playability acceptance. Buffered trace I/O adds observer overhead.'
    }
    $strataBytes = [Text.Encoding]::UTF8.GetBytes(($strataRecord | ConvertTo-Json -Depth 7))
    $strataFile = [IO.File]::Open($strataProvenance, [IO.FileMode]::CreateNew, [IO.FileAccess]::Write)
    try { $strataFile.Write($strataBytes, 0, $strataBytes.Length); $strataFile.Flush($true) } finally { $strataFile.Dispose() }
    $env:STRATA_SOURCE_REVISION = "See provenance SHA256 $((Get-FileHash -LiteralPath $strataProvenance).Hash)"
    & (Join-Path $PSScriptRoot 'run.ps1') -SkipBuild -SaveDirectory $SaveDirectory -SmokeSeconds $SmokeSeconds -TraceStreaming $strataOutput
    if (-not (Test-Path -LiteralPath $strataOutput -PathType Leaf)) { throw 'Client exited without a trace.' }
    $strataEnd = Get-Content -LiteralPath $strataOutput -Tail 1 | ConvertFrom-Json
    if ($strataEnd.kind -ne 'end' -or $strataEnd.application_clean_exit -ne $true) {
        throw 'Streaming trace is incomplete or records a failed run; retain it for diagnosis.'
    }
    Write-Output "Streaming trace: $strataOutput"
    Write-Output "Provenance: $strataProvenance"
} finally {
    [Environment]::SetEnvironmentVariable('STRATA_SOURCE_REVISION', $strataPreviousSource, 'Process')
    Pop-Location
}
