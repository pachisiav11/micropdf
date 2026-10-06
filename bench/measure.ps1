<#
.SYNOPSIS
Samples the private working set (Task Manager's "Memory" column) summed over every process
whose name matches, and reports the peak and the last sample in MB.

.EXAMPLE
./bench/measure.ps1 -Names micropdf -Seconds 10
./bench/measure.ps1 -Names Acrobat,AcroCEF -Seconds 20
#>
param(
    [Parameter(Mandatory)] [string[]] $Names,
    # Stop after this many seconds. 0 = stop once no matching process is left.
    [double] $Seconds = 0,
    [int] $IntervalMs = 250
)

$patterns = $Names | ForEach-Object { "^$([regex]::Escape($_))(#\d+)?$" }
$deadline = if ($Seconds -gt 0) { (Get-Date).AddSeconds($Seconds) } else { [datetime]::MaxValue }
$peak = 0.0
$history = New-Object System.Collections.Generic.List[double]
$samples = 0
$seen = $false

while ((Get-Date) -lt $deadline) {
    $procs = Get-CimInstance Win32_PerfFormattedData_PerfProc_Process |
        Where-Object { $n = $_.Name; $patterns | Where-Object { $n -match $_ } }
    if (-not $procs) {
        if ($seen -and $Seconds -le 0) { break }
    } else {
        $seen = $true
        $mb = ($procs | Measure-Object -Property WorkingSetPrivate -Sum).Sum / 1MB
        $history.Add($mb)
        if ($mb -gt $peak) { $peak = $mb }
        $samples++
    }
    Start-Sleep -Milliseconds $IntervalMs
}

# The last samples can catch the process tearing down; report the one ~0.5 s before the end.
$last = if ($history.Count -ge 3) { $history[$history.Count - 3] } elseif ($history.Count) { $history[0] } else { 0 }

[pscustomobject]@{
    names      = ($Names -join ',')
    peak_mb    = [math]::Round($peak, 1)
    last_mb    = [math]::Round($last, 1)
    samples    = $samples
}
