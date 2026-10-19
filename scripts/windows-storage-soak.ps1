[CmdletBinding()]
param(
    [Parameter(Mandatory = $true)]
    [string] $Exe,
    [ValidateRange(1, 100000)]
    [int] $Iterations = 250,
    [ValidateRange(1, 64)]
    [int] $Concurrency = 8,
    [ValidateRange(0, 10000)]
    [int] $ConcurrentRuns = 32,
    [switch] $IoLoad,
    [string] $OutputDirectory = ".\storage-soak"
)

$ErrorActionPreference = "Stop"
$exePath = (Resolve-Path -LiteralPath $Exe).Path
$outputPath = [IO.Path]::GetFullPath($OutputDirectory)
[IO.Directory]::CreateDirectory($outputPath) | Out-Null

function Invoke-StorageProcess {
    param([string[]] $Arguments)
    $start = New-Object Diagnostics.ProcessStartInfo
    $start.FileName = $exePath
    $start.UseShellExecute = $false
    $start.CreateNoWindow = $true
    $start.RedirectStandardOutput = $true
    $start.RedirectStandardError = $true
    # All callers use fixed, space-free private protocol arguments.
    $start.Arguments = $Arguments -join " "
    $process = New-Object Diagnostics.Process
    $process.StartInfo = $start
    $clock = [Diagnostics.Stopwatch]::StartNew()
    if (-not $process.Start()) { throw "Could not start $exePath" }
    $stdoutTask = $process.StandardOutput.ReadToEndAsync()
    $stderrTask = $process.StandardError.ReadToEndAsync()
    return [pscustomobject]@{
        Process = $process
        Clock = $clock
        StdoutTask = $stdoutTask
        StderrTask = $stderrTask
    }
}

function Complete-StorageProcess {
    param($Running, [int] $Ordinal, [string] $Mode)
    $Running.Process.WaitForExit()
    $Running.Clock.Stop()
    $stdout = $Running.StdoutTask.GetAwaiter().GetResult()
    $stderr = $Running.StderrTask.GetAwaiter().GetResult()
    $result = [ordered]@{
        ordinal = $Ordinal
        mode = $Mode
        wallMs = $Running.Clock.Elapsed.TotalMilliseconds
        exitCode = $Running.Process.ExitCode
        validJson = $false
        succeeded = $false
        reportedMs = $null
        identity = $null
        identityHash = $null
        warningHash = $null
        error = $null
        stderr = $stderr.Trim()
    }
    try {
        $json = $stdout | ConvertFrom-Json
        $result.validJson = $true
        if ($null -ne $json.error) {
            $result.error = [string]$json.error
            $inventory = $null
        } else {
            $inventory = $json.inventory
        }
        if ($null -eq $inventory) {
            if ($null -eq $result.error) { $result.error = [string]$json.inventoryError }
        } elseif ($null -ne $inventory.error) {
            $result.error = [string]$inventory.error
        } elseif ($null -eq $inventory.report) {
            $result.error = "Self-test returned neither a report nor an inventory error"
        } else {
            $report = $inventory.report
            $target = @($report.disks | Where-Object { $_.number -eq $report.bootDiskNumber })
            if ($target.Count -ne 1) { throw "Expected exactly one target disk; got $($target.Count)" }
            $parts = @($target[0].partitions | Sort-Object number | ForEach-Object {
                "$($_.number):$($_.offsetBytes):$($_.sizeBytes):$([string]$_.gptGuid):$([string]$_.typeGuid):$([string]$_.volumeGuid)"
            })
            $identity = @(
                [string]$report.bootVolumeGuid,
                [string]$report.bootDiskNumber,
                [string]$target[0].sizeBytes,
                [string]$target[0].partitionStyle,
                [string]$target[0].diskGuid,
                [string]$target[0].bus,
                [string]$target[0].model,
                [string]$target[0].serial,
                ($parts -join "|")
            ) -join "`n"
            $warnings = @($report.warnings | Sort-Object) -join "`n"
            $sha = [Security.Cryptography.SHA256]::Create()
            $result.identity = $identity
            $result.identityHash = ([BitConverter]::ToString($sha.ComputeHash([Text.Encoding]::UTF8.GetBytes($identity)))).Replace("-", "").ToLowerInvariant()
            $result.warningHash = ([BitConverter]::ToString($sha.ComputeHash([Text.Encoding]::UTF8.GetBytes($warnings)))).Replace("-", "").ToLowerInvariant()
            $result.reportedMs = [double]$report.elapsedMs
            $result.succeeded = ($Running.Process.ExitCode -eq 0)
        }
    } catch {
        $result.error = "Invalid self-test response: $($_.Exception.Message)"
    }
    $Running.Process.Dispose()
    return [pscustomobject]$result
}

function Get-Percentile {
    param([double[]] $Values, [double] $Percentile)
    if ($Values.Count -eq 0) { return $null }
    $sorted = @($Values | Sort-Object)
    $index = [Math]::Ceiling(($Percentile / 100.0) * $sorted.Count) - 1
    return $sorted[[Math]::Max(0, $index)]
}

function Get-ReferenceInventory {
    $script = @'
$ErrorActionPreference = "Stop"
$systemPartition = Get-Partition -DriveLetter $env:SystemDrive.Substring(0, 1) -ErrorAction Stop
$disk = Get-Disk -Number $systemPartition.DiskNumber -ErrorAction Stop
$partitions = @(Get-Partition -DiskNumber $disk.Number -ErrorAction Stop | Sort-Object PartitionNumber)
$volume = Get-Volume -DriveLetter $env:SystemDrive.Substring(0, 1) -ErrorAction Stop
$wmi = Get-CimInstance Win32_DiskDrive -Filter "Index = $($disk.Number)" -ErrorAction Stop
[ordered]@{
    available = $true
    error = $null
    bootVolumeGuid = [string]$volume.UniqueId
    bootFileSystem = [string]$volume.FileSystem
    disk = [ordered]@{
        number = [uint32]$disk.Number
        sizeBytes = [uint64]$disk.Size
        partitionStyle = ([string]$disk.PartitionStyle).ToLowerInvariant()
        diskGuid = ([string]$disk.Guid).Trim("{} ").ToLowerInvariant()
        bus = [string]$disk.BusType
        model = [string]$disk.Model
        serial = [string]$disk.SerialNumber
        wmiSizeBytes = [uint64]$wmi.Size
        wmiDeviceId = [string]$wmi.DeviceID
    }
    partitions = @($partitions | ForEach-Object {
        [ordered]@{
            number = [uint32]$_.PartitionNumber
            offsetBytes = [uint64]$_.Offset
            sizeBytes = [uint64]$_.Size
            gptGuid = ([string]$_.Guid).Trim("{} ").ToLowerInvariant()
            typeGuid = ([string]$_.GptType).Trim("{} ").ToLowerInvariant()
            volumeGuid = [string](@($_.AccessPaths | Where-Object { $_ -like "\\?\Volume{*" })[0])
            driveLetter = [string]$_.DriveLetter
        }
    })
} | ConvertTo-Json -Depth 8 -Compress
'@
    $encoded = [Convert]::ToBase64String([Text.Encoding]::Unicode.GetBytes($script))
    $errors = New-Object Collections.Generic.List[string]
    for ($attempt = 1; $attempt -le 3; $attempt++) {
        $start = New-Object Diagnostics.ProcessStartInfo
        $start.FileName = "$env:SystemRoot\System32\WindowsPowerShell\v1.0\powershell.exe"
        $start.Arguments = "-NoProfile -NonInteractive -EncodedCommand $encoded"
        $start.UseShellExecute = $false
        $start.CreateNoWindow = $true
        $start.RedirectStandardOutput = $true
        $start.RedirectStandardError = $true
        $process = New-Object Diagnostics.Process
        $process.StartInfo = $start
        $process.Start() | Out-Null
        $stdoutTask = $process.StandardOutput.ReadToEndAsync()
        $stderrTask = $process.StandardError.ReadToEndAsync()
        if (-not $process.WaitForExit(15000)) {
            $process.Kill()
            $process.WaitForExit()
            $errors.Add("attempt $attempt timed out")
        } else {
            $stdout = $stdoutTask.GetAwaiter().GetResult()
            $stderr = $stderrTask.GetAwaiter().GetResult()
            if ($process.ExitCode -eq 0) {
                try {
                    $parsed = $stdout | ConvertFrom-Json
                    $process.Dispose()
                    return $parsed
                } catch { $errors.Add("attempt $attempt invalid JSON: $($_.Exception.Message)") }
            } else {
                $errors.Add("attempt $attempt failed: $($stderr.Trim())")
            }
        }
        $process.Dispose()
    }
    return [pscustomobject]@{ available = $false; error = ($errors -join "; "); disk = $null; partitions = @() }
}

function Compare-Reference {
    param($SuccessfulRun, $Reference)
    $mismatches = New-Object Collections.Generic.List[string]
    if (-not $Reference.available) { return @("REFERENCE_UNAVAILABLE: $($Reference.error)") }
    $lines = $SuccessfulRun.identity -split "`n"
    if (-not $lines[0].Equals([string]$Reference.bootVolumeGuid, [StringComparison]::OrdinalIgnoreCase)) { $mismatches.Add("boot volume GUID") }
    if ([uint32]$lines[1] -ne $Reference.disk.number) { $mismatches.Add("boot disk number") }
    if ([uint64]$lines[2] -ne $Reference.disk.sizeBytes) { $mismatches.Add("disk size") }
    if ($lines[3] -ne $Reference.disk.partitionStyle) { $mismatches.Add("partition style") }
    if ($lines[4] -ne $Reference.disk.diskGuid) { $mismatches.Add("disk GUID") }
    $nativeParts = @($lines[8] -split '\|' | ForEach-Object {
        $fields = $_ -split ':', 6
        [pscustomobject]@{ number=[uint32]$fields[0]; offsetBytes=[uint64]$fields[1]; sizeBytes=[uint64]$fields[2]; gptGuid=$fields[3]; typeGuid=$fields[4]; volumeGuid=$fields[5] }
    })
    foreach ($partition in $Reference.partitions) {
        $native = @($nativeParts | Where-Object { $_.number -eq $partition.number })
        if ($native.Count -ne 1) { $mismatches.Add("partition $($partition.number) presence"); continue }
        foreach ($field in @("offsetBytes", "sizeBytes", "gptGuid", "typeGuid", "volumeGuid")) {
            if ([string]$native[0].$field -ne [string]$partition.$field) {
                $mismatches.Add("partition $($partition.number) $field")
            }
        }
    }
    return @($mismatches)
}

$beforeHelpers = @(Get-CimInstance Win32_Process -Filter "Name = 'OmarchyInstaller.exe'" | Where-Object { $_.ExecutablePath -eq $exePath }).Count
$referenceBefore = Get-ReferenceInventory
$loadJob = $null
if ($IoLoad) {
    $loadJob = Start-Job -ScriptBlock {
        $stopAt = [DateTime]::UtcNow.AddMinutes(30)
        while ([DateTime]::UtcNow -lt $stopAt) {
            Get-ChildItem "$env:SystemRoot\System32" -File -ErrorAction SilentlyContinue | Select-Object -First 250 | Get-FileHash -Algorithm SHA256 -ErrorAction SilentlyContinue | Out-Null
        }
    }
}

$results = New-Object Collections.Generic.List[object]
try {
    for ($i = 1; $i -le $Iterations; $i++) {
        $results.Add((Complete-StorageProcess (Invoke-StorageProcess @("--storage-self-test")) $i "sequential"))
    }
    $ordinal = $Iterations
    $remaining = $ConcurrentRuns
    while ($remaining -gt 0) {
        $batchSize = [Math]::Min($Concurrency, $remaining)
        $batch = @(1..$batchSize | ForEach-Object { Invoke-StorageProcess @("--storage-self-test") })
        foreach ($running in $batch) {
            $ordinal++
            $results.Add((Complete-StorageProcess $running $ordinal "concurrent"))
        }
        $remaining -= $batchSize
    }
} finally {
    if ($null -ne $loadJob) { Stop-Job $loadJob -ErrorAction SilentlyContinue; Remove-Job $loadJob -Force -ErrorAction SilentlyContinue }
}

$invalid = Complete-StorageProcess (Invoke-StorageProcess @("--storage-helper", "definitely-invalid")) 0 "invalid-command"
$referenceAfter = Get-ReferenceInventory
Start-Sleep -Milliseconds 200
$afterHelpers = @(Get-CimInstance Win32_Process -Filter "Name = 'OmarchyInstaller.exe'" | Where-Object { $_.ExecutablePath -eq $exePath }).Count
$successes = @($results | Where-Object succeeded)
$failures = @($results | Where-Object { -not $_.succeeded })
$identityHashes = @($successes.identityHash | Sort-Object -Unique)
$warningHashes = @($successes.warningHash | Sort-Object -Unique)
$wall = @($successes.wallMs)
$reported = @($successes.reportedMs)
[string[]]$referenceMismatches = if ($successes.Count -gt 0) { @(Compare-Reference $successes[0] $referenceAfter) } else { @("No successful native run") }
$summary = [ordered]@{
    generatedAtUtc = [DateTime]::UtcNow.ToString("o")
    executable = $exePath
    requested = [ordered]@{ sequential = $Iterations; concurrent = $ConcurrentRuns; concurrency = $Concurrency; ioLoad = [bool]$IoLoad }
    counts = [ordered]@{ total = $results.Count; succeeded = $successes.Count; failed = $failures.Count }
    wallMs = [ordered]@{ p50 = Get-Percentile $wall 50; p95 = Get-Percentile $wall 95; p99 = Get-Percentile $wall 99; max = ($wall | Measure-Object -Maximum).Maximum }
    reportedMs = [ordered]@{ p50 = Get-Percentile $reported 50; p95 = Get-Percentile $reported 95; p99 = Get-Percentile $reported 99; max = ($reported | Measure-Object -Maximum).Maximum }
    distinctIdentityHashes = $identityHashes
    distinctWarningHashes = $warningHashes
    identityStable = ($identityHashes.Count -eq 1)
    warningsStable = ($warningHashes.Count -eq 1)
    referenceBefore = $referenceBefore
    referenceAfter = $referenceAfter
    referenceMismatches = @($referenceMismatches)
    invalidCommandRejected = ($invalid.validJson -and $null -ne $invalid.error -and -not $invalid.succeeded)
    processCountBefore = $beforeHelpers
    processCountAfter = $afterHelpers
    leakedProcesses = [Math]::Max(0, $afterHelpers - $beforeHelpers)
}

$results | ConvertTo-Json -Depth 8 | Set-Content -LiteralPath (Join-Path $outputPath "runs.json") -Encoding UTF8
$summary | ConvertTo-Json -Depth 10 | Set-Content -LiteralPath (Join-Path $outputPath "summary.json") -Encoding UTF8
$summary | ConvertTo-Json -Depth 10

if ($failures.Count -gt 0 -or $identityHashes.Count -ne 1 -or $referenceMismatches.Count -gt 0 -or $summary.leakedProcesses -ne 0 -or -not $summary.invalidCommandRejected) {
    exit 1
}
