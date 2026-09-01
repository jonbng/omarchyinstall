$ErrorActionPreference = 'Stop'
$iso = 'C:\Users\Jonathan\Downloads\omarchy-4.0.2.iso'
$mounted = $false
try {
  $image = Mount-DiskImage -ImagePath $iso -PassThru
  $mounted = $true
  $volume = $image | Get-Volume
  $root = $volume.Path
  $uuidFiles = @(Get-ChildItem -LiteralPath (Join-Path $root 'boot') -Filter '*.uuid' -File -Recurse)
  $efi = Join-Path $root 'EFI\BOOT\BOOTX64.EFI'
  $kernel = Join-Path $root 'arch\boot\x86_64\vmlinuz-linux-t2'
  $initramfs = Join-Path $root 'arch\boot\x86_64\initramfs-linux-t2.img'
  [pscustomobject]@{
    attached = $image.Attached
    filesystem = $volume.FileSystemType
    label = $volume.FileSystemLabel
    root = $root
    uuidCount = $uuidFiles.Count
    uuidPaths = @($uuidFiles | ForEach-Object { $_.FullName.Substring($root.Length - 1).Replace('\', '/') })
    efiExists = Test-Path -LiteralPath $efi -PathType Leaf
    efiBytes = if (Test-Path -LiteralPath $efi -PathType Leaf) { (Get-Item -LiteralPath $efi).Length } else { 0 }
    kernelExists = Test-Path -LiteralPath $kernel -PathType Leaf
    kernelBytes = if (Test-Path -LiteralPath $kernel -PathType Leaf) { (Get-Item -LiteralPath $kernel).Length } else { 0 }
    initramfsExists = Test-Path -LiteralPath $initramfs -PathType Leaf
    initramfsBytes = if (Test-Path -LiteralPath $initramfs -PathType Leaf) { (Get-Item -LiteralPath $initramfs).Length } else { 0 }
  } | ConvertTo-Json -Depth 4 -Compress
} finally {
  if ($mounted) { Dismount-DiskImage -ImagePath $iso }
}
$after = Get-DiskImage -ImagePath $iso
"detachedAfter=$(-not $after.Attached)"
