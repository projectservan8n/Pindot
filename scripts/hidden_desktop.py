#!/usr/bin/env python3
"""Run a command on a separate, invisible Windows desktop.

For tests of the app on a PC somebody is using: a Pindot started this way
has all its windows (the main window, the overlay, system file dialogs) on
a desktop nobody sees, so nothing shows up on the screen and nothing can
take the keyboard focus. Its pages are still reachable over the WebView2
debugging port, and its MCP server over its port.

  python scripts\\hidden_desktop.py "<command line>"

The command runs through `cmd /c` with this process's environment, so set
PINDOT_DATA_DIR, PINDOT_PORT, WEBVIEW2_USER_DATA_FOLDER and
WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS first, and redirect inside the
quotes ("pindot.exe --exit-after 600 --setup > app.log 2>&1"). The script
waits for the command and ends with its exit code. Always give the app
`--exit-after`: there is no window to close it with.

Windows on that desktop can only be found from a process that runs there
too: start `scripts\\answer-file-dialog.ps1` the same way. The microphone,
the speakers and the credential store are NOT separate: they are the
user's. The desktop goes away with its last process.
"""
import ctypes
import os
import sys
from ctypes import wintypes

DESKTOP = "PindotTest"


class STARTUPINFOW(ctypes.Structure):
    _fields_ = [
        ("cb", wintypes.DWORD), ("lpReserved", wintypes.LPWSTR), ("lpDesktop", wintypes.LPWSTR),
        ("lpTitle", wintypes.LPWSTR), ("dwX", wintypes.DWORD), ("dwY", wintypes.DWORD),
        ("dwXSize", wintypes.DWORD), ("dwYSize", wintypes.DWORD), ("dwXCountChars", wintypes.DWORD),
        ("dwYCountChars", wintypes.DWORD), ("dwFillAttribute", wintypes.DWORD), ("dwFlags", wintypes.DWORD),
        ("wShowWindow", wintypes.WORD), ("cbReserved2", wintypes.WORD), ("lpReserved2", ctypes.c_void_p),
        ("hStdInput", wintypes.HANDLE), ("hStdOutput", wintypes.HANDLE), ("hStdError", wintypes.HANDLE),
    ]


class PROCESS_INFORMATION(ctypes.Structure):
    _fields_ = [("hProcess", wintypes.HANDLE), ("hThread", wintypes.HANDLE),
                ("dwProcessId", wintypes.DWORD), ("dwThreadId", wintypes.DWORD)]


def main():
    if os.name != "nt" or len(sys.argv) != 2:
        sys.exit(__doc__)
    user32 = ctypes.WinDLL("user32", use_last_error=True)
    kernel32 = ctypes.WinDLL("kernel32", use_last_error=True)
    user32.CreateDesktopW.restype = wintypes.HANDLE
    user32.CreateDesktopW.argtypes = [wintypes.LPCWSTR, wintypes.LPCWSTR, ctypes.c_void_p, wintypes.DWORD,
                                      wintypes.DWORD, ctypes.c_void_p]
    kernel32.CreateProcessW.argtypes = [wintypes.LPCWSTR, wintypes.LPWSTR, ctypes.c_void_p, ctypes.c_void_p,
                                        wintypes.BOOL, wintypes.DWORD, ctypes.c_void_p, wintypes.LPCWSTR,
                                        ctypes.POINTER(STARTUPINFOW), ctypes.POINTER(PROCESS_INFORMATION)]
    kernel32.WaitForSingleObject.argtypes = [wintypes.HANDLE, wintypes.DWORD]

    # Opens the desktop when it exists already (GENERIC_ALL).
    if not user32.CreateDesktopW(DESKTOP, None, None, 0, 0x10000000, None):
        sys.exit(f"CreateDesktop failed: error {ctypes.get_last_error()}")
    si = STARTUPINFOW()
    si.cb = ctypes.sizeof(si)
    si.lpDesktop = f"WinSta0\\{DESKTOP}"
    pi = PROCESS_INFORMATION()
    line = ctypes.create_unicode_buffer(f'cmd.exe /d /s /c "{sys.argv[1]}"')
    if not kernel32.CreateProcessW(None, line, None, None, False, 0, None, None, ctypes.byref(si), ctypes.byref(pi)):
        sys.exit(f"CreateProcess failed: error {ctypes.get_last_error()}")
    kernel32.WaitForSingleObject(pi.hProcess, 0xFFFFFFFF)
    code = wintypes.DWORD()
    kernel32.GetExitCodeProcess(pi.hProcess, ctypes.byref(code))
    sys.exit(code.value)


if __name__ == "__main__":
    main()
