param(
    [string]$ExecutablePath = "target\release\Arctic-ComfyUI-Helper.exe",
    [ValidateRange(30, 86400)]
    [int]$DurationSeconds = 1800,
    [ValidateRange(1, 60)]
    [int]$SampleIntervalSeconds = 1,
    [string]$OutputDir = "dist",
    [string]$FilePrefix = "memory-check",
    [int]$TargetPid,
    [switch]$StopProcessOnExit
)

Set-StrictMode -Version Latest
$ErrorActionPreference = "Stop"

function Get-ResolvedRoot {
    return Resolve-Path (Join-Path $PSScriptRoot "..")
}

function Get-SlopeMbPerMinute {
    param(
        [Parameter(Mandatory = $true)]
        [object[]]$Samples,
        [Parameter(Mandatory = $true)]
        [string]$ValueProperty
    )

    if ($Samples.Count -lt 2) {
        return 0.0
    }

    [double]$sumX = 0
    [double]$sumY = 0
    [double]$sumXY = 0
    [double]$sumX2 = 0
    [double]$n = $Samples.Count

    foreach ($sample in $Samples) {
        [double]$x = [double]$sample.elapsed_seconds
        [double]$y = [double]$sample.$ValueProperty
        $sumX += $x
        $sumY += $y
        $sumXY += ($x * $y)
        $sumX2 += ($x * $x)
    }

    [double]$denominator = ($n * $sumX2) - ($sumX * $sumX)
    if ([Math]::Abs($denominator) -lt 0.0000001) {
        return 0.0
    }

    [double]$slopePerSecond = (($n * $sumXY) - ($sumX * $sumY)) / $denominator
    return $slopePerSecond * 60.0
}

$root = Get-ResolvedRoot
Set-Location $root

$distDir = Join-Path $root $OutputDir
New-Item -ItemType Directory -Path $distDir -Force | Out-Null

$timestamp = Get-Date -Format "yyyyMMdd-HHmmss"
$csvPath = Join-Path $distDir "$FilePrefix-$timestamp.csv"
$summaryPath = Join-Path $distDir "$FilePrefix-$timestamp-summary.txt"

$startedByScript = $false
$targetPid = 0
$processName = ""

try {
    if ($TargetPid -gt 0) {
        $existing = Get-Process -Id $TargetPid -ErrorAction Stop
        $targetPid = $existing.Id
        $processName = $existing.ProcessName
        Write-Host "Attached to PID $targetPid ($processName)."
    } else {
        $exe = $ExecutablePath
        if (-not [System.IO.Path]::IsPathRooted($exe)) {
            $exe = Join-Path $root $exe
        }
        $exe = [System.IO.Path]::GetFullPath($exe)
        if (-not (Test-Path $exe)) {
            throw "Executable not found: $exe"
        }

        $proc = Start-Process -FilePath $exe -PassThru
        $startedByScript = $true
        $targetPid = $proc.Id
        $processName = $proc.ProcessName
        Write-Host "Launched PID $targetPid ($processName)."
    }

    $startAt = Get-Date
    $endAt = $startAt.AddSeconds($DurationSeconds)
    $samples = New-Object System.Collections.Generic.List[object]

    while ((Get-Date) -lt $endAt) {
        $p = Get-Process -Id $targetPid -ErrorAction SilentlyContinue
        if (-not $p) {
            Write-Host "Process exited before capture window completed."
            break
        }

        $now = Get-Date
        $elapsed = ($now - $startAt).TotalSeconds
        $samples.Add([pscustomobject]@{
            timestamp_utc      = $now.ToUniversalTime().ToString("o")
            elapsed_seconds    = [Math]::Round($elapsed, 2)
            pid                = $p.Id
            process_name       = $p.ProcessName
            working_set_mb     = [Math]::Round($p.WorkingSet64 / 1MB, 2)
            private_memory_mb  = [Math]::Round($p.PrivateMemorySize64 / 1MB, 2)
            paged_memory_mb    = [Math]::Round($p.PagedMemorySize64 / 1MB, 2)
            virtual_memory_mb  = [Math]::Round($p.VirtualMemorySize64 / 1MB, 2)
            handles            = $p.HandleCount
            threads            = $p.Threads.Count
        })

        Start-Sleep -Seconds $SampleIntervalSeconds
    }

    if ($samples.Count -eq 0) {
        throw "No samples were collected."
    }

    $samples | Export-Csv -Path $csvPath -NoTypeInformation -Encoding utf8

    $first = $samples[0]
    $last = $samples[$samples.Count - 1]
    $durationCaptured = [double]$last.elapsed_seconds

    $privateDelta = [Math]::Round(([double]$last.private_memory_mb - [double]$first.private_memory_mb), 2)
    $workingDelta = [Math]::Round(([double]$last.working_set_mb - [double]$first.working_set_mb), 2)
    $handleDelta = [int]$last.handles - [int]$first.handles

    $privateSlopePerMinute = [Math]::Round((Get-SlopeMbPerMinute -Samples $samples -ValueProperty "private_memory_mb"), 3)
    $workingSlopePerMinute = [Math]::Round((Get-SlopeMbPerMinute -Samples $samples -ValueProperty "working_set_mb"), 3)

    $assessment = if ($privateSlopePerMinute -ge 2.0) {
        "LIKELY LEAK: sustained private memory growth >= 2.0 MB/min."
    } elseif ($privateSlopePerMinute -ge 0.5) {
        "POSSIBLE LEAK: sustained private memory growth between 0.5 and 2.0 MB/min."
    } else {
        "NO CLEAR LEAK TREND: private memory slope < 0.5 MB/min."
    }

    $lines = @(
        "Memory Leak Check Summary",
        "timestamp: $(Get-Date -Format o)",
        "pid: $targetPid",
        "process_name: $processName",
        "samples: $($samples.Count)",
        "captured_seconds: $durationCaptured",
        "sample_interval_seconds: $SampleIntervalSeconds",
        "private_memory_start_mb: $($first.private_memory_mb)",
        "private_memory_end_mb: $($last.private_memory_mb)",
        "private_memory_delta_mb: $privateDelta",
        "private_memory_slope_mb_per_min: $privateSlopePerMinute",
        "working_set_start_mb: $($first.working_set_mb)",
        "working_set_end_mb: $($last.working_set_mb)",
        "working_set_delta_mb: $workingDelta",
        "working_set_slope_mb_per_min: $workingSlopePerMinute",
        "handles_start: $($first.handles)",
        "handles_end: $($last.handles)",
        "handles_delta: $handleDelta",
        "assessment: $assessment",
        "csv: $csvPath"
    )

    $lines | Set-Content -Path $summaryPath -Encoding utf8

    Write-Host ""
    Write-Host $assessment
    Write-Host "CSV: $csvPath"
    Write-Host "Summary: $summaryPath"
}
finally {
    if ($startedByScript -and $StopProcessOnExit) {
        $running = Get-Process -Id $targetPid -ErrorAction SilentlyContinue
        if ($running) {
            Stop-Process -Id $targetPid -Force
            Write-Host "Stopped PID $targetPid."
        }
    }
}
