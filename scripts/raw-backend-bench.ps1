param(
    [Parameter(Mandatory)][string[]]$Sources,
    [string]$OutputDirectory = 'tests/perf/.reports/raw-backend',
    [ValidateRange(1, 30)][int]$Runs = 5,
    [ValidateRange(30, 1200)][int]$TimeoutSeconds = 300,
    [switch]$DecodeOnly,
    [switch]$BufferedJpeg
)
$ErrorActionPreference = 'Stop'
$binary = (Resolve-Path 'target/release/raw_backend_bench.exe').Path
$outputRoot = [System.IO.Path]::GetFullPath($OutputDirectory)
New-Item -ItemType Directory -Force -Path $outputRoot | Out-Null
if (Test-Path -LiteralPath (Join-Path $outputRoot 'results.json')) {
    throw 'An earlier benchmark report exists; choose a new output directory.'
}
$results = [System.Collections.Generic.List[object]]::new()
foreach ($source in $Sources) {
    $sourcePath = (Resolve-Path -LiteralPath $source).Path
    $sourceName = [System.IO.Path]::GetFileName($sourcePath)
    for ($run = 1; $run -le $Runs; $run++) {
        # Alternate the first backend, but never overlap decodes or benchmarks.
        $backends = if ($run % 2) { @('wic-frame', 'libraw') } else { @('libraw', 'wic-frame') }
        foreach ($backend in $backends) {
            $stem = "$sourceName-$backend-$run"
            $start = [System.Diagnostics.ProcessStartInfo]::new($binary)
            $start.UseShellExecute = $false
            $start.CreateNoWindow = $true
            $start.RedirectStandardOutput = $true
            $start.RedirectStandardError = $true
            $start.ArgumentList.Add($backend)
            $start.ArgumentList.Add($sourcePath)
            $start.ArgumentList.Add((Join-Path $outputRoot "$stem.jpg"))
            if ($DecodeOnly) { $start.ArgumentList.Add('--decode-only') }
            if ($BufferedJpeg) { $start.ArgumentList.Add('--buffered-jpeg') }
            $process = [System.Diagnostics.Process]::new()
            $process.StartInfo = $start
            $timer = [System.Diagnostics.Stopwatch]::StartNew()
            $null = $process.Start()
            $stdout = $process.StandardOutput.ReadToEndAsync()
            $stderr = $process.StandardError.ReadToEndAsync()
            $peak = 0L
            try {
                while (-not $process.HasExited) {
                    $process.Refresh()
                    try { $peak = [Math]::Max($peak, $process.PeakWorkingSet64) } catch {}
                    if ($timer.Elapsed.TotalSeconds -gt $TimeoutSeconds) {
                        $process.Kill($true)
                        throw "Backend timed out: $stem"
                    }
                    Start-Sleep -Milliseconds 25
                }
                $text = $stdout.GetAwaiter().GetResult()
                $errorText = $stderr.GetAwaiter().GetResult()
                if ($process.ExitCode -ne 0) { throw "$stem failed: $text $errorText" }
                $result = $text | ConvertFrom-Json
                $result | Add-Member -NotePropertyName source -NotePropertyValue $sourceName
                $result | Add-Member -NotePropertyName run -NotePropertyValue $run
                $result | Add-Member -NotePropertyName processMs -NotePropertyValue $timer.Elapsed.TotalMilliseconds
                $result | Add-Member -NotePropertyName sampledPeakWorkingSetBytes -NotePropertyValue $peak
                $results.Add($result)
                $results | ConvertTo-Json -Depth 12 | Set-Content -LiteralPath (Join-Path $outputRoot 'results.json') -Encoding utf8
                $result | ConvertTo-Json -Depth 12 -Compress
            } finally {
                if (-not $process.HasExited) { $process.Kill($true); $process.WaitForExit() }
                $process.Dispose()
            }
        }
    }
}
