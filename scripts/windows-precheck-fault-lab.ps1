[CmdletBinding()]
param(
    [Parameter(Mandatory = $true)]
    [string] $Exe,
    [ValidateRange(1, 1000)]
    [int] $Iterations = 5,
    [ValidateRange(1, 32)]
    [int] $Concurrency = 4,
    [ValidateRange(1000, 120000)]
    [int] $MaxWallMs = 20000,
    [string] $OutputDirectory = ".\precheck-fault-lab"
)

$ErrorActionPreference = "Stop"
$exePath = (Resolve-Path -LiteralPath $Exe).Path
$outputPath = [IO.Path]::GetFullPath($OutputDirectory)
[IO.Directory]::CreateDirectory($outputPath) | Out-Null

$faults = @(
    [pscustomobject]@{ Name = "storage-timeout"; Component = "native Windows storage inventory"; Stage = "storage" },
    [pscustomobject]@{ Name = "storage-malformed"; Component = "native Windows storage inventory"; Stage = "storage" },
    [pscustomobject]@{ Name = "storage-helper-failure"; Component = "native Windows storage inventory"; Stage = "storage" },
    [pscustomobject]@{ Name = "storage-report-failure"; Component = "native Windows storage inventory"; Stage = "storage" },
    [pscustomobject]@{ Name = "bitlocker-timeout"; Component = "BitLocker WMI"; Stage = "bitlocker-wait" },
    [pscustomobject]@{ Name = "bitlocker-malformed"; Component = "BitLocker WMI"; Stage = "bitlocker-wait" },
    [pscustomobject]@{ Name = "bitlocker-empty"; Component = "BitLocker volume-to-disk association"; Stage = "bitlocker-wait"; StageSucceeded = $true },
    [pscustomobject]@{ Name = "bitlocker-worker-panic"; Component = "BitLocker WMI"; Stage = "bitlocker-wait" }
)

function Start-FaultProbe([string] $Fault) {
    $start = New-Object Diagnostics.ProcessStartInfo
    $start.FileName = $exePath
    $start.Arguments = "--precheck-self-test --inject-provider-fault $Fault"
    $start.UseShellExecute = $false
    $start.CreateNoWindow = $true
    $start.RedirectStandardOutput = $true
    $start.RedirectStandardError = $true
    $process = New-Object Diagnostics.Process
    $process.StartInfo = $start
    $clock = [Diagnostics.Stopwatch]::StartNew()
    if (-not $process.Start()) { throw "Could not start $exePath" }
    [pscustomobject]@{
        Process = $process
        Clock = $clock
        StdoutTask = $process.StandardOutput.ReadToEndAsync()
        StderrTask = $process.StandardError.ReadToEndAsync()
    }
}

function Complete-FaultProbe($Running, $Fault, [int] $Ordinal) {
    $Running.Process.WaitForExit()
    $Running.Clock.Stop()
    $stdout = $Running.StdoutTask.GetAwaiter().GetResult()
    $stderr = $Running.StderrTask.GetAwaiter().GetResult().Trim()
    $errors = New-Object Collections.Generic.List[string]
    $response = $null
    try {
        $response = $stdout | ConvertFrom-Json
        if ($response.protocolVersion -ne 1) { $errors.Add("unsupported protocol $($response.protocolVersion)") }
        if ($null -ne $response.error) { $errors.Add([string]$response.error) }
        if ($null -eq $response.probe) { $errors.Add("response contained no probe") }
        if ($null -eq $response.diagnostics) { $errors.Add("response contained no diagnostics") }
        if ($null -ne $response.probe) {
            $components = @($response.probe.blockingReasons |
                Where-Object type -eq "probeIncomplete" |
                ForEach-Object { [string]$_.component })
            if ($components -notcontains $Fault.Component) {
                $errors.Add("missing probeIncomplete component '$($Fault.Component)'")
            }
        }
        if ($null -ne $response.diagnostics) {
            $stage = @($response.diagnostics.timings | Where-Object stage -eq $Fault.Stage)
            if ($stage.Count -ne 1) { $errors.Add("expected one '$($Fault.Stage)' timing") }
            $expectedStageSucceeded = ($Fault.PSObject.Properties.Name -contains "StageSucceeded" -and [bool]$Fault.StageSucceeded)
            if ($stage.Count -eq 1 -and ([bool]$stage[0].succeeded) -ne $expectedStageSucceeded) {
                $errors.Add("injected stage success was $([bool]$stage[0].succeeded), expected $expectedStageSucceeded")
            }
        }
        if ($Running.Clock.Elapsed.TotalMilliseconds -gt $MaxWallMs) {
            $errors.Add("wall time exceeded ${MaxWallMs}ms")
        }
        if ($Running.Process.ExitCode -ne 0) { $errors.Add("exit code $($Running.Process.ExitCode)") }
    } catch {
        $errors.Add($_.Exception.Message)
    } finally {
        $Running.Process.Dispose()
    }
    [pscustomobject]@{
        ordinal = $Ordinal
        fault = $Fault.Name
        expectedComponent = $Fault.Component
        expectedFailedStage = $Fault.Stage
        wallMs = $Running.Clock.Elapsed.TotalMilliseconds
        succeeded = ($errors.Count -eq 0)
        errors = @($errors)
        stderr = $stderr
        response = $response
    }
}

$processName = [IO.Path]::GetFileName($exePath)
$beforeProcesses = @(Get-CimInstance Win32_Process -Filter "Name = '$processName'" |
    Where-Object { $_.ExecutablePath -eq $exePath }).Count
$runs = New-Object Collections.Generic.List[object]
$work = New-Object Collections.Generic.List[object]
foreach ($fault in $faults) {
    for ($iteration = 1; $iteration -le $Iterations; $iteration++) {
        $work.Add($fault)
    }
}

$ordinal = 0
while ($work.Count -gt 0) {
    $batchSize = [Math]::Min($Concurrency, $work.Count)
    $batch = New-Object Collections.Generic.List[object]
    for ($index = 0; $index -lt $batchSize; $index++) {
        $fault = $work[0]
        $work.RemoveAt(0)
        $batch.Add([pscustomobject]@{ Fault = $fault; Running = (Start-FaultProbe $fault.Name) })
    }
    foreach ($item in $batch) {
        $ordinal++
        $runs.Add((Complete-FaultProbe $item.Running $item.Fault $ordinal))
    }
}

# Invalid injection input must be handled by the CLI protocol, never by opening
# the GUI or silently running a normal probe.
$invalid = Start-FaultProbe "definitely-invalid"
$invalid.Process.WaitForExit()
$invalid.Clock.Stop()
$invalidStdout = $invalid.StdoutTask.GetAwaiter().GetResult()
$invalidStderr = $invalid.StderrTask.GetAwaiter().GetResult().Trim()
$invalidResponse = $null
$invalidRejected = $false
try {
    $invalidResponse = $invalidStdout | ConvertFrom-Json
    $invalidRejected = ($null -ne $invalidResponse.error -and $null -eq $invalidResponse.probe)
} catch {
    $invalidRejected = $false
} finally {
    $invalid.Process.Dispose()
}

Start-Sleep -Milliseconds 200
$afterProcesses = @(Get-CimInstance Win32_Process -Filter "Name = '$processName'" |
    Where-Object { $_.ExecutablePath -eq $exePath }).Count
$failures = @($runs | Where-Object { -not $_.succeeded })
$byFault = @($runs | Group-Object fault | Sort-Object Name | ForEach-Object {
    $wall = @($_.Group | ForEach-Object { [double]$_.wallMs })
    [ordered]@{
        fault = $_.Name
        runs = $_.Count
        failures = @($_.Group | Where-Object { -not $_.succeeded }).Count
        minWallMs = ($wall | Measure-Object -Minimum).Minimum
        averageWallMs = ($wall | Measure-Object -Average).Average
        maxWallMs = ($wall | Measure-Object -Maximum).Maximum
    }
})
$summary = [ordered]@{
    generatedAtUtc = [DateTime]::UtcNow.ToString("o")
    executable = $exePath
    requested = [ordered]@{ iterationsPerFault = $Iterations; concurrency = $Concurrency; maxWallMs = $MaxWallMs }
    counts = [ordered]@{ total = $runs.Count; succeeded = $runs.Count - $failures.Count; failed = $failures.Count }
    faults = $byFault
    invalidFaultRejected = $invalidRejected
    invalidFaultStderr = $invalidStderr
    processCountBefore = $beforeProcesses
    processCountAfter = $afterProcesses
    leakedProcesses = [Math]::Max(0, $afterProcesses - $beforeProcesses)
}

$runs | ConvertTo-Json -Depth 20 | Set-Content -LiteralPath (Join-Path $outputPath "runs.json") -Encoding UTF8
$summary | ConvertTo-Json -Depth 12 | Set-Content -LiteralPath (Join-Path $outputPath "summary.json") -Encoding UTF8
$summary | ConvertTo-Json -Depth 12

if ($failures.Count -gt 0 -or -not $invalidRejected -or $summary.leakedProcesses -ne 0) {
    exit 1
}
