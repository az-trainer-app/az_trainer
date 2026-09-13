"""
Reverse pointer scan: find a static path to a heap address.

    python rscan.py <process.exe> <target-hex> [depth] [window]

Games without a known root object (Frostbite, custom engines) need a path that
starts inside the module, since module+offset survives restarts while heap
addresses do not.

Level 1 finds every pointer landing within `window` bytes before the target.
Any that live inside the module are static hits -- done. The rest become level
2 targets, and so on.

Each level scans all committed memory, so depth 3 is slow. Start with 2.
"""

import os
import struct
import sys

import numpy as np

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from memlib import attach, make_io, regions

CHUNK = 16 * 1024 * 1024


def pointers_into(rd, regs, lo, hi):
    """Every address holding a pointer in [lo, hi). Returns [(at, points_to)]."""
    found = []
    for base, size in regs:
        off = 0
        while off < size:
            n = min(CHUNK, size - off)
            data = rd(base + off, n)
            if len(data) >= 8:
                usable = len(data) - (len(data) % 8)
                arr = np.frombuffer(data, dtype=np.uint64, count=usable // 8)
                m = (arr >= lo) & (arr < hi)
                for i in np.flatnonzero(m):
                    found.append((base + off + int(i) * 8, int(arr[i])))
            off += n
    return found


def main():
    if len(sys.argv) < 3:
        print(__doc__)
        return 2
    proc = sys.argv[1]
    target = int(sys.argv[2], 16)
    depth = int(sys.argv[3]) if len(sys.argv) > 3 else 2
    window = int(sys.argv[4], 0) if len(sys.argv) > 4 else 0x600

    h, pid, mb, ms = attach(proc)
    if not h:
        print(f"! {proc} not running")
        return 1
    rd, wrf, rq, rf = make_io(h)
    print(f"pid {pid}  module 0x{mb:X}+0x{ms:X}")
    print(f"target 0x{target:X}, depth {depth}, window 0x{window:X}\n")

    regs = list(regions(h))
    print(f"{len(regs)} regions to scan per level\n")

    level = [(target, [])]          # (address, offsets collected so far)
    for d in range(1, depth + 1):
        print(f"--- level {d}: {len(level)} target(s) ---")
        nxt = []
        statics = []
        for tgt, trail in level[:40]:      # cap the fan-out
            hits = pointers_into(rd, regs, tgt - window, tgt + 1)
            for at, points_to in hits:
                off = tgt - points_to
                path = [off] + trail
                if mb <= at < mb + ms:
                    statics.append((at - mb, path))
                else:
                    nxt.append((at, path))
        if statics:
            print(f"\nSTATIC PATHS FOUND ({len(statics)}):\n")
            for rva, path in statics[:15]:
                chain = " -> ".join(f"+0x{o:X}" for o in path)
                print(f"  [{proc}+0x{rva:X}] -> {chain}")
            return 0
        print(f"  no static hits; {len(nxt)} pointer(s) to chase\n")
        # dedupe, keep the most promising
        seen = set()
        level = []
        for a, p in nxt:
            if a not in seen:
                seen.add(a)
                level.append((a, p))
        if not level:
            print("dead end - widen the window or raise depth")
            return 1

    print("no static path within that depth")
    return 1


if __name__ == "__main__":
    raise SystemExit(main())
