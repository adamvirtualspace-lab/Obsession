param([int]$ProcessId, [int]$Seconds = 6, [string]$Burst = "")
# The UI thread's load while something happens: samples its CPU cycles every
# 100 ms while -Burst (a command) runs in the background, e.g. a script that
# writes 2,000 notes into the vault. About 300 Mcycles per 100 ms is one core
# flat out.
#   powershell -File bench/scripts/ui-load.ps1 -ProcessId 1234 -Seconds 8 -Burst "python make-notes.py"
Add-Type @"
using System; using System.Runtime.InteropServices;
public static class Ml {
  [DllImport("kernel32.dll")] public static extern IntPtr OpenThread(uint access, bool inherit, uint id);
  [DllImport("kernel32.dll")] public static extern bool QueryThreadCycleTime(IntPtr h, out ulong cycles);
}
"@
$p = Get-Process -Id $ProcessId
$main = $p.Threads[0].Id
$h = [Ml]::OpenThread(0x0800, $false, [uint32]$main)
if ($Burst) { $job = Start-Job -ScriptBlock ([scriptblock]::Create($Burst)) }
$prev = [uint64]0; [Ml]::QueryThreadCycleTime($h, [ref]$prev) | Out-Null
$rows = @()
$sw = [Diagnostics.Stopwatch]::StartNew()
$last = 0
while ($sw.ElapsedMilliseconds -lt $Seconds * 1000) {
  Start-Sleep -Milliseconds 100
  $c = [uint64]0; [Ml]::QueryThreadCycleTime($h, [ref]$c) | Out-Null
  $now = $sw.ElapsedMilliseconds
  $rows += [pscustomobject]@{ ms = $now; mcycles = [math]::Round(($c - $prev) / 1e6, 1); window = $now - $last }
  $prev = $c; $last = $now
}
if ($job) { Receive-Job $job -Wait | Out-Null; Remove-Job $job }
# ~2.9 GHz: 290 Mcycles in a 100 ms window is one core flat out.
$busy = $rows | Where-Object { $_.mcycles -gt 0.5 }
"main thread $main; windows with work: $($busy.Count) of $($rows.Count)"
"peak {0} Mcycles in 100 ms; total {1} Mcycles" -f ($rows | Measure-Object mcycles -Maximum).Maximum, ($rows | Measure-Object mcycles -Sum).Sum
($rows | ForEach-Object { $_.mcycles }) -join ' '
