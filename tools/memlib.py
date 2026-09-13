"""
Shared memory primitives for the trainer's diagnostic scripts.

    from memlib import attach, make_io, scan_sig

Every script that pokes at a running game uses these: attach by process name,
read/write helpers, and an AOB scanner with per-nibble wildcards ("4?" and
"??" both work).
"""

import ctypes
import ctypes.wintypes as wt
import struct

import numpy as np

k = ctypes.WinDLL("kernel32", use_last_error=True)
k.OpenProcess.restype = wt.HANDLE
k.OpenProcess.argtypes = [wt.DWORD, wt.BOOL, wt.DWORD]

PROCESS_ALL = 0x043A  # query | vm_read | vm_write | vm_operation


class PROCESSENTRY32(ctypes.Structure):
    _fields_ = [
        ("dwSize", wt.DWORD), ("cntUsage", wt.DWORD), ("th32ProcessID", wt.DWORD),
        ("th32DefaultHeapID", ctypes.POINTER(ctypes.c_ulong)),
        ("th32ModuleID", wt.DWORD), ("cntThreads", wt.DWORD),
        ("th32ParentProcessID", wt.DWORD), ("pcPriClassBase", ctypes.c_long),
        ("dwFlags", wt.DWORD), ("szExeFile", ctypes.c_char * 260),
    ]


class MODULEENTRY32(ctypes.Structure):
    _fields_ = [
        ("dwSize", wt.DWORD), ("th32ModuleID", wt.DWORD),
        ("th32ProcessID", wt.DWORD), ("GlblcntUsage", wt.DWORD),
        ("ProccntUsage", wt.DWORD), ("modBaseAddr", ctypes.POINTER(ctypes.c_byte)),
        ("modBaseSize", wt.DWORD), ("hModule", wt.HMODULE),
        ("szModule", ctypes.c_char * 256), ("szExePath", ctypes.c_char * 260),
    ]


class MEMORY_BASIC_INFORMATION(ctypes.Structure):
    _fields_ = [
        ("BaseAddress", ctypes.c_ulonglong), ("AllocationBase", ctypes.c_ulonglong),
        ("AllocationProtect", wt.DWORD), ("__a1", wt.DWORD),
        ("RegionSize", ctypes.c_ulonglong), ("State", wt.DWORD),
        ("Protect", wt.DWORD), ("Type", wt.DWORD), ("__a2", wt.DWORD),
    ]


MEM_COMMIT = 0x1000
MEM_PRIVATE = 0x20000
PAGE_GUARD = 0x100
WRITABLE = {0x04, 0x08, 0x40, 0x80}


def attach(name="dawnwalker.exe"):
    """Open a process by exe name. Returns (handle, pid, module_base, module_size)."""
    name = name.lower()
    snap = k.CreateToolhelp32Snapshot(2, 0)
    pe = PROCESSENTRY32()
    pe.dwSize = ctypes.sizeof(PROCESSENTRY32)
    pid = 0
    if k.Process32First(snap, ctypes.byref(pe)):
        while True:
            if pe.szExeFile.decode(errors="ignore").lower() == name:
                pid = pe.th32ProcessID
                break
            if not k.Process32Next(snap, ctypes.byref(pe)):
                break
    k.CloseHandle(snap)
    if not pid:
        return None, 0, 0, 0

    h = k.OpenProcess(PROCESS_ALL, False, pid)
    snap = k.CreateToolhelp32Snapshot(8 | 16, pid)
    me = MODULEENTRY32()
    me.dwSize = ctypes.sizeof(MODULEENTRY32)
    base = size = 0
    if k.Module32First(snap, ctypes.byref(me)):
        while True:
            if me.szModule.decode(errors="ignore").lower() == name:
                base = ctypes.cast(me.modBaseAddr, ctypes.c_void_p).value
                size = me.modBaseSize
                break
            if not k.Module32Next(snap, ctypes.byref(me)):
                break
    k.CloseHandle(snap)
    return h, pid, base, size


def make_io(h):
    """Returns (read_bytes, write_f32, read_u64, read_f32) bound to a handle."""

    def rd(addr, n):
        buf = ctypes.create_string_buffer(n)
        got = ctypes.c_size_t(0)
        k.ReadProcessMemory(h, ctypes.c_void_p(addr), buf, n, ctypes.byref(got))
        return buf.raw[: got.value]

    def wrf(addr, v):
        n = ctypes.c_size_t(0)
        return bool(k.WriteProcessMemory(h, ctypes.c_void_p(addr),
                                         struct.pack("<f", v), 4, ctypes.byref(n)))

    def rq(addr):
        d = rd(addr, 8)
        return struct.unpack("<Q", d)[0] if len(d) == 8 else 0

    def rf(addr):
        d = rd(addr, 4)
        return struct.unpack("<f", d)[0] if len(d) == 4 else None

    return rd, wrf, rq, rf


def write_bytes(h, addr, data):
    n = ctypes.c_size_t(0)
    return bool(k.WriteProcessMemory(h, ctypes.c_void_p(addr),
                                     bytes(data), len(data), ctypes.byref(n)))


def parse_sig(sig):
    """'48 8B 4? ?? ' -> (values, mask) arrays, per-nibble wildcards."""
    val, mask = [], []
    for tok in sig.split():
        v = m = 0
        for c in tok:
            v <<= 4
            m <<= 4
            if c != "?":
                m |= 0xF
                v |= int(c, 16)
        val.append(v & m)
        mask.append(m)
    return np.array(val, np.uint8), np.array(mask, np.uint8)


def scan_sig(rd, base, size, sig, chunk=8 * 1024 * 1024):
    """Scan a module for a signature; returns the address or None."""
    val, mask = parse_sig(sig)
    L = len(val)
    p, end = base, base + size
    while p < end:
        n = min(chunk, end - p)
        data = rd(p, n)
        if len(data) >= L:
            a = np.frombuffer(data, np.uint8)
            for i in np.flatnonzero((a[: len(a) - L + 1] & mask[0]) == val[0]):
                if np.all((a[i:i + L] & mask) == val):
                    return p + int(i)
        if n < L:
            break
        p += n - (L - 1)
    return None


def regions(h):
    """Yield (base, size) for committed, writable, private regions."""
    k.VirtualQueryEx.argtypes = [wt.HANDLE, wt.LPCVOID,
                                 ctypes.POINTER(MEMORY_BASIC_INFORMATION),
                                 ctypes.c_size_t]
    mbi = MEMORY_BASIC_INFORMATION()
    addr = 0
    while addr < 0x7FFFFFFF0000:
        if not k.VirtualQueryEx(h, ctypes.c_void_p(addr), ctypes.byref(mbi),
                                ctypes.sizeof(mbi)):
            break
        if mbi.RegionSize == 0:
            break
        if (mbi.State == MEM_COMMIT and mbi.Type == MEM_PRIVATE
                and mbi.Protect in WRITABLE and not (mbi.Protect & PAGE_GUARD)):
            yield mbi.BaseAddress, int(mbi.RegionSize)
        addr = mbi.BaseAddress + mbi.RegionSize
