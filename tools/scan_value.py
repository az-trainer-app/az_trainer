"""
Find a value by successive scans. Works for any game and any int/float.

    python scan_value.py <process.exe> <value>        first pass
    python scan_value.py <process.exe> <new-value>    narrow, after it changes

Change the value in game between passes. Two or three passes usually leaves a
handful of addresses; the real one tracks every change.

Candidates live in scan_<process>.npy between runs; delete it to start over.
"""

import os
import struct
import sys

import numpy as np

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from memlib import attach, make_io, regions

CHUNK = 16 * 1024 * 1024


def main():
    if len(sys.argv) < 3:
        print(__doc__)
        return 2
    proc = sys.argv[1]
    want = int(sys.argv[2])
    store = f"scan_{proc.replace('.exe', '').replace(' ', '_').lower()}.npy"

    h, pid, mb, ms = attach(proc)
    if not h:
        print(f"! {proc} is not running")
        return 1
    rd, wrf, rq, rf = make_io(h)
    print(f"pid {pid}")

    if os.path.exists(store):
        prev = np.load(store)
        keep = []
        for a in prev:
            d = rd(int(a), 4)
            if len(d) == 4 and struct.unpack("<i", d)[0] == want:
                keep.append(int(a))
        out = np.array(keep, dtype=np.uint64)
        np.save(store, out)
        print(f"narrowed {len(prev):,} -> {len(out):,} now reading {want}")
        for a in out[:25]:
            print(f"  0x{int(a):X}")
        if len(out) == 0:
            print(f"\nnone left - delete {store} and start over")
        elif len(out) <= 8:
            print("\nfew enough to test")
        return 0

    hits = []
    scanned = 0
    for base, size in regions(h):
        off = 0
        while off < size:
            n = min(CHUNK, size - off)
            data = rd(base + off, n)
            if data:
                usable = len(data) - (len(data) % 4)
                if usable >= 4:
                    arr = np.frombuffer(data, dtype=np.int32, count=usable // 4)
                    idx = np.flatnonzero(arr == want)
                    if idx.size:
                        hits.extend((base + off + i * 4) for i in idx.tolist())
                scanned += len(data)
            off += n
    out = np.array(hits, dtype=np.uint64)
    np.save(store, out)
    print(f"scanned {scanned/2**30:.2f} GB, {len(out):,} matches -> {store}")
    print(f"\nchange the value in game, then:")
    print(f"    python scan_value.py \"{proc}\" <new-value>")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
