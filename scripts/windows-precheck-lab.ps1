[CmdletBinding()]
param(
    [Parameter(Mandatory = $true)]
    [string] $Exe,
    [ValidateRange(1, 100000)]
    [int] $Iterations = 50,
    [ValidateRange(1, 64)]
    [int] $Concurrency = 4,
    [ValidateRange(0, 10000)]
    [int] $ConcurrentRuns = 8,
    [switch] $IoLoad,
    [switch] $AllowIncomplete,
    [string] $Baseline,
    [switch] $UpdateBaseline,
    [string] $OutputDirectory = ".\precheck-lab"
)

$ErrorActionPreference = "Stop"
$exePath = (Resolve-Path -LiteralPath $Exe).Path
$outputPath = [IO.Path]::GetFullPath($OutputDirectory)
[IO.Directory]::CreateDirectory($outputPath) | Out-Null

function Get-Sha256([string] $Text) {
    $sha = [Security.Cryptography.SHA256]::Create()
    try {
        return ([BitConverter]::ToString($sha.ComputeHash([Text.Encoding]::UTF8.GetBytes($Text)))).Replace("-", "").ToLowerInvariant()
    } finally { $sha.Dispose() }
}

function Start-Precheck {
    $start = New-Object Diagnostics.ProcessStartInfo
    $start.FileName = $exePath
    $start.Arguments = "--precheck-self-test"
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

function Get-CanonicalProbe($Probe) {
    $disks = @($Probe.disks | Sort-Object deviceId | ForEach-Object {
        $disk = $_
        [ordered]@{
            deviceId = [string]$disk.deviceId
            sizeBytes = [uint64]$disk.sizeBytes
            partitionStyle = [string]$disk.partitionStyle
            bus = [string]$disk.bus
            isBoot = [bool]$disk.isBoot
            isRst = [bool]$disk.isRst
            isDynamic = [bool]$disk.isDynamic
            isStorageSpaces = [bool]$disk.isStorageSpaces
            maxShrinkBytes = $disk.maxShrinkBytes
            partitions = @($disk.partitions | Sort-Object offsetBytes | ForEach-Object {
                [ordered]@{
                    gptGuid = [string]$_.gptGuid
                    typeGuid = [string]$_.typeGuid
                    letter = [string]$_.letter
                    label = [string]$_.label
                    sizeBytes = [uint64]$_.sizeBytes
                    offsetBytes = [uint64]$_.offsetBytes
                    fs = [string]$_.fs
                }
            })
        }
    })
    $bitlocker = @($Probe.bitlocker | Sort-Object deviceId,mount | ForEach-Object {
        [ordered]@{
            deviceId = [string]$_.deviceId
            diskId = [string]$_.diskId
            mount = [string]$_.mount
            protectionStatus = [uint32]$_.protectionStatus
            conversionStatus = [uint32]$_.conversionStatus
            fullyDecrypted = [bool]$_.fullyDecrypted
        }
    })
    [ordered]@{
        host = $Probe.host
        uefi = [bool]$Probe.uefi
        secureBoot = [bool]$Probe.secureBoot
        efiVarsWritable = [bool]$Probe.efiVarsWritable
        ramInstalledBytes = [uint64]$Probe.ramInstalledBytes
        ramTotalPhysBytes = [uint64]$Probe.ramTotalPhysBytes
        ramOkForCopytoram = [bool]$Probe.ramOkForCopytoram
        tpmPresent = [bool]$Probe.tpmPresent
        recommendedDiskId = [string]$Probe.recommendedDiskId
        targetEsp = $Probe.targetEsp
        linuxById = [string]$Probe.linuxById
        bitlocker = $bitlocker
        disks = $disks
    }
}

function Get-CanonicalBlockers($Probe) {
    @($Probe.blockingReasons | ForEach-Object { $_ | ConvertTo-Json -Depth 8 -Compress } | Sort-Object)
}

function Complete-Precheck($Running, [int] $Ordinal, [string] $Mode) {
    $Running.Process.WaitForExit()
    $Running.Clock.Stop()
    $stdout = $Running.StdoutTask.GetAwaiter().GetResult()
    $stderr = $Running.StderrTask.GetAwaiter().GetResult().Trim()
    $result = [ordered]@{
        ordinal = $Ordinal
        mode = $Mode
        wallMs = $Running.Clock.Elapsed.TotalMilliseconds
        exitCode = $Running.Process.ExitCode
        succeeded = $false
        elapsedMs = $null
        identityHash = $null
        blockerHash = $null
        blockerTypes = @()
        incompleteComponents = @()
        timings = @()
        error = $null
        stderr = $stderr
        response = $null
    }
    try {
        $response = $stdout | ConvertFrom-Json
        $result.response = $response
        if ($response.protocolVersion -ne 1) { throw "Unsupported protocol $($response.protocolVersion)" }
        if ($null -ne $response.error) { throw [string]$response.error }
        if ($null -eq $response.probe) { throw "Response contained no probe" }
        $identity = Get-CanonicalProbe $response.probe
        $blockers = @(Get-CanonicalBlockers $response.probe)
        $result.identityHash = Get-Sha256 ($identity | ConvertTo-Json -Depth 12 -Compress)
        $result.blockerHash = Get-Sha256 ($blockers -join "`n")
        $result.blockerTypes = @($response.probe.blockingReasons | ForEach-Object { [string]$_.type } | Sort-Object)
        $result.incompleteComponents = @($response.probe.blockingReasons | Where-Object type -eq "probeIncomplete" | ForEach-Object { [string]$_.component } | Sort-Object)
        $result.timings = @($response.diagnostics.timings)
        $result.elapsedMs = [double]$response.elapsedMs
        $result.succeeded = ($Running.Process.ExitCode -eq 0)
    } catch {
        $result.error = $_.Exception.Message
    } finally {
        $Running.Process.Dispose()
    }
    [pscustomobject]$result
}

function Get-Percentile([double[]] $Values, [double] $Percentile) {
    if ($Values.Count -eq 0) { return $null }
    $sorted = @($Values | Sort-Object)
    $index = [Math]::Ceiling(($Percentile / 100.0) * $sorted.Count) - 1
    $sorted[[Math]::Max(0, $index)]
}

$beforeProcesses = @(Get-CimInstance Win32_Process -Filter "Name = 'OmarchyInstaller.exe'" | Where-Object { $_.ExecutablePath -eq $exePath }).Count
$loadJob = $null
if ($IoLoad) {
    $loadJob = Start-Job -ScriptBlock {
        $stopAt = [DateTime]::UtcNow.AddMinutes(30)
        while ([DateTime]::UtcNow -lt $stopAt) {
            Get-ChildItem "$env:SystemRoot\System32" -File -ErrorAction SilentlyContinue |
                Select-Object -First 250 | Get-FileHash -Algorithm SHA256 -ErrorAction SilentlyContinue | Out-Null
        }
    }
}

$runs = New-Object Collections.Generic.List[object]
try {
    for ($i = 1; $i -le $Iterations; $i++) {
        $runs.Add((Complete-Precheck (Start-Precheck) $i "sequential"))
    }
    $ordinal = $Iterations
    $remaining = $ConcurrentRuns
    while ($remaining -gt 0) {
        $batchSize = [Math]::Min($Concurrency, $remaining)
        $batch = @(1..$batchSize | ForEach-Object { Start-Precheck })
        foreach ($running in $batch) {
            $ordinal++
            $runs.Add((Complete-Precheck $running $ordinal "concurrent"))
        }
        $remaining -= $batchSize
    }
} finally {
    if ($null -ne $loadJob) {
        Stop-Job $loadJob -ErrorAction SilentlyContinue
        Remove-Job $loadJob -Force -ErrorAction SilentlyContinue
    }
}

Start-Sleep -Milliseconds 200
$afterProcesses = @(Get-CimInstance Win32_Process -Filter "Name = 'OmarchyInstaller.exe'" | Where-Object { $_.ExecutablePath -eq $exePath }).Count
$successful = @($runs | Where-Object succeeded)
$failed = @($runs | Where-Object { -not $_.succeeded })
$identityHashes = @($successful.identityHash | Sort-Object -Unique)
$blockerHashes = @($successful.blockerHash | Sort-Object -Unique)
$incomplete = @($successful | Where-Object { $_.incompleteComponents.Count -gt 0 })
$stageSummary = @($successful.timings | Group-Object stage | Sort-Object Name | ForEach-Object {
    $values = @($_.Group | ForEach-Object { [double]$_.elapsedMs })
    [ordered]@{
        stage = $_.Name
        count = $values.Count
        failures = @($_.Group | Where-Object { -not $_.succeeded }).Count
        p50Ms = Get-Percentile $values 50
        p95Ms = Get-Percentile $values 95
        p99Ms = Get-Percentile $values 99
        maxMs = ($values | Measure-Object -Maximum).Maximum
    }
})

$observedBlockerTypes = New-Object Collections.Generic.List[string]
if ($successful.Count -gt 0) {
    foreach ($blockerType in @($successful[0].blockerTypes)) {
        $observedBlockerTypes.Add([string]$blockerType)
    }
}
$observed = [ordered]@{
    identityHash = @($identityHashes)[0]
    blockerHash = @($blockerHashes)[0]
    blockerTypes = @($observedBlockerTypes)
}
$baselineErrors = New-Object Collections.Generic.List[string]
if (-not [string]::IsNullOrWhiteSpace($Baseline)) {
    $baselinePath = [IO.Path]::GetFullPath($Baseline)
    if ($UpdateBaseline) {
        if ($failed.Count -gt 0 -or $identityHashes.Count -ne 1 -or $blockerHashes.Count -ne 1 -or (-not $AllowIncomplete -and $incomplete.Count -gt 0)) {
            $baselineErrors.Add("refusing to update a baseline from unstable, failed, or incomplete runs")
        } else {
            $observed | ConvertTo-Json -Depth 6 | Set-Content -LiteralPath $baselinePath -Encoding UTF8
        }
    } elseif (Test-Path -LiteralPath $baselinePath) {
        $expected = Get-Content -LiteralPath $baselinePath -Raw | ConvertFrom-Json
        if ($expected.identityHash -ne $observed.identityHash) { $baselineErrors.Add("machine identity changed") }
        if ($expected.blockerHash -ne $observed.blockerHash) { $baselineErrors.Add("blocking result changed") }
    } else {
        $baselineErrors.Add("baseline does not exist: $baselinePath")
    }
}

$summary = [ordered]@{
    generatedAtUtc = [DateTime]::UtcNow.ToString("o")
    executable = $exePath
    requested = [ordered]@{ sequential = $Iterations; concurrent = $ConcurrentRuns; concurrency = $Concurrency; ioLoad = [bool]$IoLoad }
    counts = [ordered]@{ total = $runs.Count; succeeded = $successful.Count; failed = $failed.Count; incomplete = $incomplete.Count }
    wallMs = [ordered]@{ p50 = Get-Percentile @($successful.wallMs) 50; p95 = Get-Percentile @($successful.wallMs) 95; p99 = Get-Percentile @($successful.wallMs) 99; max = (@($successful.wallMs) | Measure-Object -Maximum).Maximum }
    probeMs = [ordered]@{ p50 = Get-Percentile @($successful.elapsedMs) 50; p95 = Get-Percentile @($successful.elapsedMs) 95; p99 = Get-Percentile @($successful.elapsedMs) 99; max = (@($successful.elapsedMs) | Measure-Object -Maximum).Maximum }
    stages = $stageSummary
    identityStable = ($identityHashes.Count -eq 1)
    blockersStable = ($blockerHashes.Count -eq 1)
    observed = $observed
    baselineErrors = @($baselineErrors)
    processCountBefore = $beforeProcesses
    processCountAfter = $afterProcesses
    leakedProcesses = [Math]::Max(0, $afterProcesses - $beforeProcesses)
}

$runs | ConvertTo-Json -Depth 20 | Set-Content -LiteralPath (Join-Path $outputPath "runs.json") -Encoding UTF8
$summary | ConvertTo-Json -Depth 12 | Set-Content -LiteralPath (Join-Path $outputPath "summary.json") -Encoding UTF8
$summary | ConvertTo-Json -Depth 12

$failedIncomplete = (-not $AllowIncomplete -and $incomplete.Count -gt 0)
if ($failed.Count -gt 0 -or $identityHashes.Count -ne 1 -or $blockerHashes.Count -ne 1 -or $failedIncomplete -or $baselineErrors.Count -gt 0 -or $summary.leakedProcesses -ne 0) {
    exit 1
}
