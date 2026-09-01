$ErrorActionPreference = 'Stop'
$diskNumber = 0
$espGuid = '{b7c42ce2-2c4d-4f50-a5b4-5811a9970f4c}'
$p = Get-Partition -DiskNumber $diskNumber | Where-Object { ([string]$_.Guid).Trim('{}') -eq $espGuid.Trim('{}') }
if (-not $p) { throw 'ESP not found' }
$assigned = $false
if (-not $p.DriveLetter) {
  $p | Add-PartitionAccessPath -AssignDriveLetter
  $assigned = $true
  $p = Get-Partition -DiskNumber $diskNumber -PartitionNumber $p.PartitionNumber
}
"espLetter=$($p.DriveLetter) assigned=$assigned"

function Test-Entry([string]$strategy) {
  $description = "Omarchy $strategy Validation $([guid]::NewGuid())"
  $id = $null
  try {
    if ($strategy -eq 'BOOTAPP') {
      $created = (& bcdedit /create /d $description /application BOOTAPP | Out-String)
    } elseif ($strategy -eq 'BOOTMGR') {
      $created = (& bcdedit /create /d $description /application BOOTMGR | Out-String)
    } else {
      $created = (& bcdedit /copy '{bootmgr}' /d $description | Out-String)
    }
    "$strategy createExit=$LASTEXITCODE output=$($created.Trim())"
    $match = [regex]::Match($created,'\{[0-9a-fA-F-]+\}')
    if (-not $match.Success) { return }
    $id = $match.Value
    "$strategy id=$id"
    $device = "partition=$($p.DriveLetter):"
    $setDevice = (& bcdedit /set $id device $device | Out-String)
    "$strategy setDeviceExit=$LASTEXITCODE output=$($setDevice.Trim())"
    $setPath = (& bcdedit /set $id path '\EFI\OmarchyValidation\BOOTX64.EFI' | Out-String)
    "$strategy setPathExit=$LASTEXITCODE output=$($setPath.Trim())"
    $entry = (& bcdedit /enum $id /v | Out-String)
    "$strategy enumExit=$LASTEXITCODE entry=$($entry.Trim())"
    $firmware = (& bcdedit /enum firmware /v | Out-String)
    "$strategy visibleInFirmware=$($firmware -match [regex]::Escape($id))"
    $setSequence = (& bcdedit /set '{fwbootmgr}' bootsequence $id | Out-String)
    "$strategy setSequenceExit=$LASTEXITCODE output=$($setSequence.Trim())"
    $fw = (& bcdedit /enum '{fwbootmgr}' /v | Out-String)
    "$strategy bootSequenceVisible=$($fw -match [regex]::Escape($id))"
    if ($fw -match [regex]::Escape($id)) {
      $clearSequence = (& bcdedit /deletevalue '{fwbootmgr}' bootsequence | Out-String)
      "$strategy clearSequenceExit=$LASTEXITCODE output=$($clearSequence.Trim())"
    }
  } finally {
    if ($id) {
      $deleted = (& bcdedit /delete $id /cleanup | Out-String)
      "$strategy deleteEntryExit=$LASTEXITCODE output=$($deleted.Trim())"
    }
  }
}

try {
  Test-Entry 'BOOTAPP'
  Test-Entry 'BOOTMGR'
  Test-Entry 'COPY_BOOTMGR'
} finally {
  $fw = (& bcdedit /enum '{fwbootmgr}' /v | Out-String)
  if ($fw -match '(?im)^bootsequence') {
    & bcdedit /deletevalue '{fwbootmgr}' bootsequence | Out-Null
  }
  if ($assigned) {
    Remove-PartitionAccessPath -DiskNumber $diskNumber -PartitionNumber $p.PartitionNumber -AccessPath ($p.DriveLetter + ':\') -ErrorAction Continue
  }
}
$espAfter = Get-Partition -DiskNumber $diskNumber -PartitionNumber $p.PartitionNumber
"espLetterAfter=$([string]$espAfter.DriveLetter)"
