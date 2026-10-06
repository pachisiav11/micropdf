<#
.SYNOPSIS
M0 memory benchmark. Runs micropdf with each Slint renderer on each file (scroll top to
bottom, then settle), optionally opens the same files in Acrobat (open, then idle), and writes
bench/results/<timestamp>.md.

.EXAMPLE
./bench/run.ps1                     # micropdf only
./bench/run.ps1 -Acrobat            # also Acrobat; close Acrobat first
#>
param(
    [string] $Exe = "target/bench/release/micropdf.exe",
    [string[]] $Backends = @("winit-software"),
    [string[]] $Files = @("", "fixtures/hello.pdf", "fixtures/external/text-300.pdf", "fixtures/external/scan-1000.pdf"),
    [switch] $Acrobat,
    [double] $AcrobatSeconds = 20
)
$ErrorActionPreference = 'Stop'
$root = Split-Path -Parent $PSScriptRoot
Set-Location $root
$results = Join-Path $root "bench/results"
New-Item -ItemType Directory -Force $results | Out-Null
$rows = @()

foreach ($file in $Files) {
    foreach ($backend in $Backends) {
        $out = Join-Path $results "run.json"
        Remove-Item $out -ErrorAction SilentlyContinue
        $env:SLINT_BACKEND = $backend
        $arguments = @("--bench-scroll", "`"$out`"")
        if ($file) { $arguments = @("`"$file`"") + $arguments }
        $proc = Start-Process -FilePath $Exe -ArgumentList $arguments -PassThru
        $mem = & "$PSScriptRoot/measure.ps1" -Names micropdf
        $proc.WaitForExit()
        $run = Get-Content $out -ErrorAction SilentlyContinue | ConvertFrom-Json
        $rows += [pscustomobject]@{
            app        = "micropdf"
            renderer   = $backend
            file       = if ($file) { Split-Path $file -Leaf } else { "(none)" }
            peak_mb    = $mem.peak_mb
            settled_mb = $mem.last_mb
            scroll_s   = $run.scroll_seconds
            ticks_s    = $run.ticks_per_second
            tiles      = $run.tiles_rendered
        }
    }
}
Remove-Item Env:SLINT_BACKEND -ErrorAction SilentlyContinue

if ($Acrobat) {
    $acrobatExe = "C:\Program Files\Adobe\Acrobat DC\Acrobat\Acrobat.exe"
    $names = @("Acrobat", "AcroCEF", "AdobeCollabSync")
    if (Get-Process Acrobat -ErrorAction SilentlyContinue) {
        Write-Warning "Acrobat is already running. Close it so the numbers cover only the test file. Skipping Acrobat."
    } else {
        foreach ($file in ($Files | Where-Object { $_ })) {
            $p = Start-Process $acrobatExe -ArgumentList "`"$((Resolve-Path $file).Path)`"" -PassThru
            $mem = & "$PSScriptRoot/measure.ps1" -Names $names -Seconds $AcrobatSeconds
            $null = $p.CloseMainWindow()
            $rows += [pscustomobject]@{
                app = "Acrobat"; renderer = "-"; file = Split-Path $file -Leaf
                peak_mb = $mem.peak_mb; settled_mb = $mem.last_mb
                scroll_s = $null; ticks_s = $null; tiles = $null
            }
            if (-not $p.WaitForExit(20000)) {
                # Acrobat was not running before this script started it, so stopping it is safe.
                Write-Warning "Acrobat did not close after 20 s; stopping it."
                Stop-Process -Name Acrobat, AcroCEF -Force -ErrorAction SilentlyContinue
                Start-Sleep -Seconds 3
            }
        }
    }
}

$stamp = Get-Date -Format "yyyy-MM-dd_HHmm"
$md = @(
    "# Benchmark $stamp",
    "",
    "micropdf: open, scroll top to bottom, settle 3 s. Acrobat: open, idle $AcrobatSeconds s (no scrolling).",
    "Memory is private working set summed over all processes of the app, in MB.",
    "",
    "| app | renderer | file | peak MB | settled MB | scroll s | ticks/s | tiles |",
    "|---|---|---|---|---|---|---|---|"
) + ($rows | ForEach-Object {
    "| $($_.app) | $($_.renderer) | $($_.file) | $($_.peak_mb) | $($_.settled_mb) | $($_.scroll_s) | $($_.ticks_s) | $($_.tiles) |"
})
$path = Join-Path $results "$stamp.md"
$md | Set-Content -Encoding utf8 $path
$md -join "`n"
"`nSaved $path"
