# Answer a system file dialog of ONE process (a test instance of Pindot)
# with window messages: put a path in its name box and press its button.
# No pointer and no keys, so it works on a desktop nobody sees: run it there
# with scripts\hidden_desktop.py, because a window can only be found from
# its own desktop.
#
#   answer-file-dialog.ps1 -ProcId <pid> -Path <file>            "Attach files", "Import passwords..."
#   answer-file-dialog.ps1 -ProcId <pid> -Path <folder> -Folder  the brain folder's "Choose..."
#   answer-file-dialog.ps1 -ProcId <pid> -Cancel
#
# Only for a process the test started itself. Never aim it at a dialog of
# an app the user has open.
param([int]$ProcId, [string]$Path, [switch]$Folder, [switch]$Cancel, [int]$WaitMs = 10000)

Add-Type @"
using System; using System.Text; using System.Runtime.InteropServices; using System.Collections.Generic;
public static class FileDlg {
  delegate bool EnumProc(IntPtr h, IntPtr l);
  [DllImport("user32.dll")] static extern bool EnumWindows(EnumProc f, IntPtr l);
  [DllImport("user32.dll")] static extern bool EnumChildWindows(IntPtr parent, EnumProc f, IntPtr l);
  [DllImport("user32.dll")] static extern uint GetWindowThreadProcessId(IntPtr h, out uint pid);
  [DllImport("user32.dll", CharSet = CharSet.Unicode)] static extern int GetClassName(IntPtr h, StringBuilder s, int n);
  [DllImport("user32.dll", CharSet = CharSet.Unicode)] static extern int GetWindowText(IntPtr h, StringBuilder s, int n);
  [DllImport("user32.dll")] static extern int GetDlgCtrlID(IntPtr h);
  [DllImport("user32.dll")] public static extern bool IsWindow(IntPtr h);
  [DllImport("user32.dll", CharSet = CharSet.Unicode)] public static extern IntPtr SendMessage(IntPtr h, uint msg, IntPtr w, string l);
  [DllImport("user32.dll")] public static extern bool PostMessage(IntPtr h, uint msg, IntPtr w, IntPtr l);
  static string Class(IntPtr h) { var s = new StringBuilder(128); GetClassName(h, s, 128); return s.ToString(); }
  public static string Title(IntPtr h) { var s = new StringBuilder(256); GetWindowText(h, s, 256); return s.ToString(); }
  // The process's dialog (class #32770), front one first.
  public static IntPtr Find(uint pid) {
    IntPtr found = IntPtr.Zero;
    EnumWindows((h, l) => { uint p; GetWindowThreadProcessId(h, out p); if (p == pid && Class(h) == "#32770") { found = h; return false; } return true; }, IntPtr.Zero);
    return found;
  }
  // Every control of the dialog, in order, as "class|id|handle".
  public static List<string> Controls(IntPtr dlg) {
    var all = new List<string>();
    EnumChildWindows(dlg, (h, l) => { all.Add(Class(h) + "|" + GetDlgCtrlID(h) + "|" + h.ToInt64()); return true; }, IntPtr.Zero);
    return all;
  }
}
"@

function Handle($entry) { [IntPtr][long]($entry.Split('|')[2]) }
function Button($dlg, $id) { [FileDlg]::Controls($dlg) | Where-Object { $_ -match "^Button\|$id\|" } | Select-Object -First 1 }

$deadline = (Get-Date).AddMilliseconds($WaitMs)
$dlg = [IntPtr]::Zero
while ($dlg -eq [IntPtr]::Zero -and (Get-Date) -lt $deadline) {
  $dlg = [FileDlg]::Find($ProcId)
  if ($dlg -eq [IntPtr]::Zero) { Start-Sleep -Milliseconds 150 }
}
if ($dlg -eq [IntPtr]::Zero) { "NO DIALOG of process $ProcId within $WaitMs ms"; exit 1 }
"dialog: `"$([FileDlg]::Title($dlg))`""

$BM_CLICK = 0x00F5; $WM_SETTEXT = 0x000C; $WM_COMMAND = 0x0111
if ($Cancel) {
  $b = Button $dlg 2
  if ($b) { [void][FileDlg]::PostMessage((Handle $b), $BM_CLICK, [IntPtr]::Zero, [IntPtr]::Zero) }
  "pressed Cancel"; exit 0
}

# The name box: the edit inside combo box 1148 ("File name:"), or in a
# folder picker the edit 1152 ("Folder:"). The dialog builds its controls a
# moment after it shows.
$edit = $null
$until = (Get-Date).AddSeconds(6)
while (-not $edit -and (Get-Date) -lt $until) {
  $all = [FileDlg]::Controls($dlg)
  if ($Folder) {
    $edit = $all | Where-Object { $_ -match '^Edit\|1152\|' } | Select-Object -First 1
  } else {
    for ($i = 0; $i -lt $all.Count -and -not $edit; $i++) {
      if ($all[$i] -match '^ComboBoxEx32\|1148\|') {
        $edit = $all[($i + 1)..([Math]::Min($all.Count - 1, $i + 3))] | Where-Object { $_ -match '^Edit\|' } | Select-Object -First 1
      }
    }
  }
  if (-not $edit) { Start-Sleep -Milliseconds 250 }
}
if (-not $edit) { "NO NAME BOX in the dialog"; exit 1 }
[void][FileDlg]::SendMessage((Handle $edit), $WM_SETTEXT, [IntPtr]::Zero, $Path)
"name box set"

# A click on the button (Open, Select Folder). The folder picker ignores
# the dialog's own "OK" command, so that comes second, for a file dialog
# whose button did not take the click.
$b = Button $dlg 1
if ($b) { [void][FileDlg]::PostMessage((Handle $b), $BM_CLICK, [IntPtr]::Zero, [IntPtr]::Zero); "clicked its button" }
Start-Sleep -Milliseconds 1200
if ([FileDlg]::IsWindow($dlg)) {
  [void][FileDlg]::PostMessage($dlg, $WM_COMMAND, [IntPtr]1, [IntPtr]::Zero)
  "sent the dialog its OK command"
  Start-Sleep -Milliseconds 1200
}
$open = [FileDlg]::IsWindow($dlg)
"dialog still open: $open"
if ($open) { exit 1 }
