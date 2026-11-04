param(
    [UInt64]$Seed = 728491,
    [ValidateScript({ -not $_.Contains('"') })]
    [string]$SaveDirectory = 'saves/emerald-reach',
    [switch]$SkipBuild,
    [ValidateScript({ -not $_.Contains('"') })]
    [string]$TraceStreaming,
    [ValidateRange(1, 120)]
    [int]$SmokeSeconds
)
$ErrorActionPreference = 'Stop'
Push-Location -LiteralPath $PSScriptRoot
try {
    if (-not $SkipBuild) {
        cargo build --release --locked --target x86_64-pc-windows-msvc
        if ($LASTEXITCODE -ne 0) { throw 'STRATA build failed. See Cargo diagnostics above.' }
    }
    $strataExecutable = Join-Path $PSScriptRoot 'target\x86_64-pc-windows-msvc\release\strata.exe'
    if (-not (Test-Path -LiteralPath $strataExecutable -PathType Leaf)) {
        throw 'No release build found. Run without -SkipBuild first.'
    }
    if ([IO.Path]::IsPathRooted($SaveDirectory)) {
        $strataSavePath = [IO.Path]::GetFullPath($SaveDirectory)
    } else {
        $strataSavePath = [IO.Path]::GetFullPath((Join-Path $PSScriptRoot $SaveDirectory))
    }
    $strataLogRoot = Join-Path $PSScriptRoot 'logs'
    New-Item -ItemType Directory -Path $strataLogRoot -Force | Out-Null
    $strataRunId = (Get-Date -Format 'yyyyMMdd-HHmmss') + '-' + [Guid]::NewGuid().ToString('N').Substring(0, 8)
    $strataStdout = Join-Path $strataLogRoot "$strataRunId-stdout.log"
    $strataStderr = Join-Path $strataLogRoot "$strataRunId-stderr.log"
    $strataArguments = @('--seed', $Seed.ToString(), '--save-dir', ('"' + $strataSavePath.Replace('\', '/') + '"'))
    if ($PSBoundParameters.ContainsKey('SmokeSeconds')) {
        $strataArguments += @('--smoke-seconds', $SmokeSeconds.ToString())
    }
    if ($PSBoundParameters.ContainsKey('TraceStreaming')) {
        $strataTracePath = if ([IO.Path]::IsPathRooted($TraceStreaming)) {
            [IO.Path]::GetFullPath($TraceStreaming)
        } else {
            [IO.Path]::GetFullPath((Join-Path $PSScriptRoot $TraceStreaming))
        }
        $strataArguments += @('--trace-streaming', ('"' + $strataTracePath.Replace('\', '/') + '"'))
    }
    Write-Host "STRATA logs: $strataStdout"
    # The native game is intentionally visible. Redirected handles also preserve
    # startup diagnostics from the Windows GUI-subsystem release executable.
    $strataProcess = Start-Process -FilePath $strataExecutable -WorkingDirectory $PSScriptRoot `
        -ArgumentList $strataArguments -RedirectStandardOutput $strataStdout `
        -RedirectStandardError $strataStderr -PassThru -Wait
    if ($strataProcess.ExitCode -ne 0) {
        $strataErrorText = Get-Content -LiteralPath $strataStderr -Raw
        throw "STRATA exited with code $($strataProcess.ExitCode). $strataErrorText`nDiagnostics: $strataStderr. Preserve save files before recovery."
    }
} finally {
    Pop-Location
}
