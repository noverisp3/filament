"""Starts the server, sends it a real Ctrl+C, and checks how it died.

A console control event has to be delivered to another process's console, which
PowerShell has no cmdlet for. This creates a console, runs the server in it,
generates the event with GenerateConsoleCtrlEvent, and then waits.

Two things are checked, because either alone can pass while the feature is
broken:

  - the exit code is 0. A Ctrl+C that the handler did not catch terminates the
    process with STATUS_CONTROL_C_EXIT, 0xc000013a, and a Ctrl+C caught by one
    but not honoured by the loop times out here instead.
  - the log says it stopped on purpose. An exit code of 0 from a process that
    simply ran out of road is not the same as one that shut down.
"""

import ctypes
import subprocess
import sys
import time
from pathlib import Path

binary = Path(sys.argv[1]) if len(sys.argv) > 1 else Path("target/debug/filament.exe")
log = Path(`r`n    sys.argv[2] if len(sys.argv) > 2 else Path(f"ctrlc-test-{int(time.time())}.log")`r`n)

CREATE_NEW_CONSOLE = 0x00000010
CTRL_C_EVENT = 0
CTRL_BREAK_EVENT = 1

kernel32 = ctypes.WinDLL("kernel32", use_last_error=True)

kernel32.AllocConsole.restype = ctypes.c_void_p
kernel32.FreeConsole.restype = ctypes.c_bool
kernel32.GenerateConsoleCtrlEvent.argtypes = [
    ctypes.c_uint32,
    ctypes.c_uint32,
    ctypes.c_void_p,
]
kernel32.GenerateConsoleCtrlEvent.restype = ctypes.c_bool
kernel32.AttachConsole.argtypes = [ctypes.c_uint32]
kernel32.SetConsoleCtrlHandler.argtypes = [ctypes.c_void_p, ctypes.c_int]

if log.exists():
    log.unlink()

with log.open("w") as out:
    proc = subprocess.Popen([str(binary)], stdout=out, stderr=subprocess.STDOUT,
                            creationflags=CREATE_NEW_CONSOLE)

time.sleep(2.5)
if proc.poll() is not None:
    print(f"FAIL: the server exited before the signal, code {proc.returncode}")
    sys.exit(1)

print(f"server running as pid {proc.pid}; sending Ctrl+C")

freed = kernel32.FreeConsole()
if not freed:
    print(f"FAIL: FreeConsole: {ctypes.get_last_error()}")
    sys.exit(1)

if not kernel32.AllocConsole():
    print(f"FAIL: AllocConsole: {ctypes.get_last_error()}")
    sys.exit(1)

if not kernel32.SetConsoleCtrlHandler(None, 0):
    print(f"FAIL: SetConsoleCtrlHandler(None, 0): {ctypes.get_last_error()}")
    sys.exit(1)

sent = kernel32.GenerateConsoleCtrlEvent(CTRL_C_EVENT, 0)
if not sent:
    print(f"FAIL: GenerateConsoleCtrlEvent: {ctypes.get_last_error()}")
    sys.exit(1)

kernel32.FreeConsole()
if not kernel32.AttachConsole(proc.pid):
    print("FAIL: AttachConsole back to the parent")
    sys.exit(1)
kernel32.SetConsoleCtrlHandler(None, 0)

try:
    code = proc.wait(timeout=10)
except subprocess.TimeoutExpired:
    proc.kill()
    print("FAIL: still running ten seconds after Ctrl+C; the handler never ran")
    sys.exit(1)

text = log.read_text()

print(f"exit code: {code} (0x{code & 0xFFFFFFFF:08x})")
print("--- log ---")
print(text.rstrip())

problems = []
if code != 0:
    problems.append(f"exited with {code:#x}, not 0")
if "stopped" not in text:
    problems.append("never said it stopped")

if problems:
    for p in problems:
        print(f"FAIL: {p}")
    sys.exit(1)

print("PASS: Ctrl+C was handled and the server stopped on purpose")
