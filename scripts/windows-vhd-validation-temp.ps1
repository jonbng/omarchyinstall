$ErrorActionPreference = 'Stop'
$vhd = Join-Path $env:TEMP 'omarchy-partition-validation.vhdx'
$diskpartScript = Join-Path $env:TEMP 'omarchy-partition-validation.txt'
$before = @(Get-Disk | ForEach-Object Number)
try {
  @(
    "create vdisk file=`"$vhd`" maximum=2048 type=expandable",
    "select vdisk file=`"$vhd`"",
    'attach vdisk'
  ) | Set-Content -Encoding ascii $diskpartScript
  $diskpart = (& diskpart /s $diskpartScript | Out-String)
  "diskpart=$($diskpart.Trim())"
  $disk = Get-Disk | Where-Object { $before -notcontains $_.Number } | Select-Object -First 1
  if (-not $disk) { throw 'new VHD disk not found' }
  "diskNumber=$($disk.Number) size=$($disk.Size) bus=$($disk.BusType)"
  Initialize-Disk -Number $disk.Number -PartitionStyle GPT
  $disk = Get-Disk -Number $disk.Number
  $diskGuid = ([string]$disk.Guid).Trim('{}')
  "diskGuid=$diskGuid style=$($disk.PartitionStyle)"

  $om = New-Partition -DiskNumber $disk.Number -Size 1GB -GptType '{ebd0a0a2-b9e5-4433-87c0-68b6b72699c7}'
  Format-Volume -Partition $om -FileSystem NTFS -NewFileSystemLabel OMARCHYINST -Confirm:$false | Out-Null
  Set-Partition -DiskNumber $disk.Number -PartitionNumber $om.PartitionNumber -NoDefaultDriveLetter $true -IsHidden $false
  $om = Get-Partition -DiskNumber $disk.Number -PartitionNumber $om.PartitionNumber
  if ($om.DriveLetter) { Remove-PartitionAccessPath -DiskNumber $disk.Number -PartitionNumber $om.PartitionNumber -AccessPath ($om.DriveLetter + ':\') }

  $ci = New-Partition -DiskNumber $disk.Number -Size 64MB -GptType '{ebd0a0a2-b9e5-4433-87c0-68b6b72699c7}'
  Format-Volume -Partition $ci -FileSystem FAT32 -NewFileSystemLabel cidata -Confirm:$false | Out-Null
  $ciVolumeBefore = Get-Volume -Partition $ci
  $ciId = [string]$ciVolumeBefore.UniqueId
  "cidataBeforeHardening fs=$($ciVolumeBefore.FileSystemType) label=$($ciVolumeBefore.FileSystemLabel) id=$ciId reachable=$(Test-Path -LiteralPath $ciId)"
  Set-Partition -DiskNumber $disk.Number -PartitionNumber $ci.PartitionNumber -NoDefaultDriveLetter $true -IsHidden $false
  $ci = Get-Partition -DiskNumber $disk.Number -PartitionNumber $ci.PartitionNumber
  if ($ci.DriveLetter) { Remove-PartitionAccessPath -DiskNumber $disk.Number -PartitionNumber $ci.PartitionNumber -AccessPath ($ci.DriveLetter + ':\') }
  $ciAfterPartition = Get-Volume -Partition $ci -ErrorAction SilentlyContinue
  $ciAfterUnique = Get-Volume -UniqueId $ciId -ErrorAction SilentlyContinue
  "cidataAfterHardening byPartition=$(@($ciAfterPartition).Count) byUniqueId=$(@($ciAfterUnique).Count) savedReachable=$(Test-Path -LiteralPath $ciId) driveBool=$([bool]$ci.DriveLetter)"

  $results = @(Get-Partition -DiskNumber $disk.Number | ForEach-Object {
    $p=$_; $vol=Get-Volume -Partition $p
    $volumeId=[string]$vol.UniqueId
    [pscustomobject]@{
      PartitionNumber=$p.PartitionNumber; Guid=[string]$p.Guid; Offset=$p.Offset; Size=$p.Size
      IsHidden=$p.IsHidden; NoDefaultDriveLetter=$p.NoDefaultDriveLetter; DriveLetter=[string]$p.DriveLetter
      FileSystem=[string]$vol.FileSystemType; Label=[string]$vol.FileSystemLabel; Volume=$volumeId
      Reachable=if($volumeId){Test-Path -LiteralPath $volumeId}else{$false}; DriveLetterBool=[bool]$p.DriveLetter
    }
  })
  $results | ConvertTo-Json -Depth 4
  $omResult=$results | Where-Object Label -eq 'OMARCHYINST'
  $ciResult=$results | Where-Object Label -eq 'cidata'
  "omValid=$($omResult.FileSystem -eq 'NTFS' -and $omResult.NoDefaultDriveLetter -and -not $omResult.DriveLetterBool -and $omResult.Reachable)"
  "cidataValid=$($ciResult.FileSystem -eq 'FAT32' -and $ciResult.NoDefaultDriveLetter -and -not $ciResult.DriveLetterBool -and -not $ciResult.IsHidden -and $ciResult.Reachable)"
} finally {
  Dismount-DiskImage -ImagePath $vhd -ErrorAction SilentlyContinue | Out-Null
  Remove-Item -LiteralPath $vhd -Force -ErrorAction SilentlyContinue
  Remove-Item -LiteralPath $diskpartScript -Force -ErrorAction SilentlyContinue
  "vhdCleaned=$(-not (Test-Path -LiteralPath $vhd))"
}
