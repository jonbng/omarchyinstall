$ErrorActionPreference = 'Stop'
$source = @'
using System;
using System.Runtime.InteropServices;
using Microsoft.Win32.SafeHandles;
public static class NativeProbe {
 [StructLayout(LayoutKind.Sequential, CharSet=CharSet.Auto)]
 public class MEMORYSTATUSEX {
   public uint dwLength=64; public uint dwMemoryLoad; public ulong ullTotalPhys; public ulong ullAvailPhys;
   public ulong ullTotalPageFile; public ulong ullAvailPageFile; public ulong ullTotalVirtual; public ulong ullAvailVirtual; public ulong ullAvailExtendedVirtual;
 }
 [DllImport("kernel32.dll", SetLastError=true)] public static extern bool GetFirmwareType(out uint type);
 [DllImport("kernel32.dll", SetLastError=true)] public static extern bool GetPhysicallyInstalledSystemMemory(out ulong kb);
 [DllImport("kernel32.dll", SetLastError=true)] public static extern bool GlobalMemoryStatusEx([In,Out] MEMORYSTATUSEX status);
 [DllImport("kernel32.dll", SetLastError=true, CharSet=CharSet.Unicode)] public static extern uint GetFirmwareEnvironmentVariable(string name,string guid,byte[] data,uint size);
 [DllImport("kernel32.dll", SetLastError=true, CharSet=CharSet.Unicode)] public static extern bool SetFirmwareEnvironmentVariableEx(string name,string guid,byte[] data,uint size,uint attrs);
 [DllImport("kernel32.dll", SetLastError=true, CharSet=CharSet.Unicode)] public static extern SafeFileHandle CreateFile(string name,uint access,uint share,IntPtr security,uint creation,uint flags,IntPtr template);
 [DllImport("kernel32.dll", SetLastError=true)] public static extern bool ReadFile(SafeFileHandle handle,byte[] buffer,uint count,out uint read,IntPtr overlapped);
 [DllImport("kernel32.dll", SetLastError=true)] public static extern bool SetFilePointerEx(SafeFileHandle handle,long distance,out long newPosition,uint method);
}
'@
Add-Type $source
$firmware = 0
$firmwareOk = [NativeProbe]::GetFirmwareType([ref]$firmware)
$installedKb = [uint64]0
$installedOk = [NativeProbe]::GetPhysicallyInstalledSystemMemory([ref]$installedKb)
$memory = New-Object NativeProbe+MEMORYSTATUSEX
$memoryOk = [NativeProbe]::GlobalMemoryStatusEx($memory)
"firmwareOk=$firmwareOk firmwareType=$firmware"
"installedOk=$installedOk installedBytes=$($installedKb*1024)"
"memoryOk=$memoryOk totalBytes=$($memory.ullTotalPhys) availableBytes=$($memory.ullAvailPhys)"
$secure = New-Object byte[] 1
$secureRead = [NativeProbe]::GetFirmwareEnvironmentVariable('SecureBoot','{8BE4DF61-93CA-11D2-AA0D-00E098032B8C}',$secure,1)
$secureErr = [Runtime.InteropServices.Marshal]::GetLastWin32Error()
"secureBootRead=$secureRead value=$($secure[0]) error=$secureErr"
$h = [NativeProbe]::CreateFile('\\.\C:',[uint32]2147483648,3,[IntPtr]::Zero,3,0x80,[IntPtr]::Zero)
if ($h.IsInvalid) {
  "rawOpen=False error=$([Runtime.InteropServices.Marshal]::GetLastWin32Error())"
} else {
  $sector = New-Object byte[] 512
  $count = [uint32]0
  $rawOk = [NativeProbe]::ReadFile($h,$sector,512,[ref]$count,[IntPtr]::Zero)
  $rawError = [Runtime.InteropServices.Marshal]::GetLastWin32Error()
  "rawOpen=True read=$rawOk error=$rawError count=$count prefix=$([BitConverter]::ToString($sector,0,16)) signature=$([Text.Encoding]::ASCII.GetString($sector,3,8))"
  $h.Dispose()
}
$diskHandle = [NativeProbe]::CreateFile('\\.\PHYSICALDRIVE0',[uint32]2147483648,3,[IntPtr]::Zero,3,0x80,[IntPtr]::Zero)
if ($diskHandle.IsInvalid) {
  "physicalOpen=False error=$([Runtime.InteropServices.Marshal]::GetLastWin32Error())"
} else {
  $position = [int64]0
  $seek = [NativeProbe]::SetFilePointerEx($diskHandle,227540992,[ref]$position,0)
  $diskSector = New-Object byte[] 512
  $diskCount = [uint32]0
  $diskRead = [NativeProbe]::ReadFile($diskHandle,$diskSector,512,[ref]$diskCount,[IntPtr]::Zero)
  $diskError = [Runtime.InteropServices.Marshal]::GetLastWin32Error()
  "physicalOpen=True seek=$seek position=$position read=$diskRead error=$diskError count=$diskCount prefix=$([BitConverter]::ToString($diskSector,0,16)) signature=$([Text.Encoding]::ASCII.GetString($diskSector,3,8))"
  $diskHandle.Dispose()
}
$guid = '{FDCA2A4E-3D8D-4EB7-AE97-80598A4D5DB4}'
$failures = @()
1..10 | ForEach-Object {
  $name = "OmarchyValidation$($_)"
  $data = [BitConverter]::GetBytes([uint32]($_*1000+$PID))
  $write = [NativeProbe]::SetFirmwareEnvironmentVariableEx($name,$guid,$data,$data.Length,7)
  if (-not $write) { $failures += "write $_ error=$([Runtime.InteropServices.Marshal]::GetLastWin32Error())"; return }
  $back = New-Object byte[] 4
  $read = [NativeProbe]::GetFirmwareEnvironmentVariable($name,$guid,$back,4)
  if ($read -ne 4 -or [BitConverter]::ToUInt32($back,0) -ne [BitConverter]::ToUInt32($data,0)) { $failures += "read $_ count=$read error=$([Runtime.InteropServices.Marshal]::GetLastWin32Error())" }
  $delete = [NativeProbe]::SetFirmwareEnvironmentVariableEx($name,$guid,$null,0,7)
  if (-not $delete) { $failures += "delete $_ error=$([Runtime.InteropServices.Marshal]::GetLastWin32Error())" }
  $after = New-Object byte[] 4
  $afterRead = [NativeProbe]::GetFirmwareEnvironmentVariable($name,$guid,$after,4)
  if ($afterRead -ne 0 -or [Runtime.InteropServices.Marshal]::GetLastWin32Error() -ne 203) { $failures += "cleanup-check $_ count=$afterRead error=$([Runtime.InteropServices.Marshal]::GetLastWin32Error())" }
}
"efiCycles=10 failures=$($failures.Count)"
$failures
