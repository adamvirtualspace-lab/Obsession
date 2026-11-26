# Which threads of a running Obsession used the CPU over a window, counted in
# CPU cycles (QueryThreadCycleTime). Process CPU time is charged in 15.6 ms
# ticks and misses short wakeups, so idle checks use this instead: an idle
# app shows no thread at all.
#   powershell -File bench/scripts/idle-threads.ps1 -ProcessId 1234 -Seconds 60
param([int]$ProcessId, [int]$Seconds = 30)
Add-Type @"
using System; using System.Runtime.InteropServices;
public static class Th {
  [DllImport("kernel32.dll")] public static extern IntPtr OpenThread(uint access, bool inherit, uint id);
  [DllImport("kernel32.dll")] public static extern bool CloseHandle(IntPtr h);
  [DllImport("kernel32.dll")] public static extern bool QueryThreadCycleTime(IntPtr h, out ulong cycles);
  [DllImport("ntdll.dll")] public static extern int NtQueryInformationThread(IntPtr h, int cls, out IntPtr info, int len, IntPtr ret);
  public static ulong Cycles(uint id) {
    IntPtr h = OpenThread(0x0800, false, id); if (h == IntPtr.Zero) return 0;
    ulong c; QueryThreadCycleTime(h, out c); CloseHandle(h); return c;
  }
  public static long Start(uint id) {
    IntPtr h = OpenThread(0x0040 | 0x0800, false, id); if (h == IntPtr.Zero) return -1;
    IntPtr a; int s = NtQueryInformationThread(h, 9, out a, IntPtr.Size, IntPtr.Zero); CloseHandle(h);
    return s == 0 ? a.ToInt64() : -2;
  }
}
"@
$p = Get-Process -Id $ProcessId
$mods = $p.Modules | ForEach-Object { [pscustomobject]@{ Name = $_.ModuleName; Base = $_.BaseAddress.ToInt64(); End = $_.BaseAddress.ToInt64() + $_.ModuleMemorySize } }
function Module-Of([long]$addr) {
  $m = $mods | Where-Object { $addr -ge $_.Base -and $addr -lt $_.End } | Select-Object -First 1
  if ($m) { '{0}+0x{1:X}' -f $m.Name, ($addr - $m.Base) } else { '0x{0:X}' -f $addr }
}
$before = @{}
foreach ($t in $p.Threads) { $before[$t.Id] = [Th]::Cycles([uint32]$t.Id) }
Start-Sleep $Seconds
$p.Refresh()
foreach ($t in $p.Threads) {
  $now = [Th]::Cycles([uint32]$t.Id)
  $was = if ($before.ContainsKey($t.Id)) { $before[$t.Id] } else { 0 }
  if ($now -gt $was) {
    $start = [Th]::Start([uint32]$t.Id)
    '{0,6} {1,14:N0} cycles  {2}' -f $t.Id, ($now - $was), (Module-Of $start)
  }
}
"threads: $($p.Threads.Count)"
