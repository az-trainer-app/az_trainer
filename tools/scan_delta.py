"""
Find a value you cannot read exactly, by how it CHANGES.

    python scan_delta.py <process.exe> snap [max] [min]  record the candidates
    python scan_delta.py <process.exe> down            keep those that fell
    python scan_delta.py <process.exe> up              keep those that rose
    python scan_delta.py <process.exe> same            keep those unchanged
    python scan_delta.py <process.exe> eq <v> [tol]    keep those now equal to v
    python scan_delta.py <process.exe> band <lo> <hi>  keep those now in a range
    python scan_delta.py <process.exe> show            list what is left

Health, mana, stamina: snap, take damage, `down`, take more, `down` again.
Three passes usually leaves a handful.

Scans int32 and float32 together, since engines differ. Only values in
(0, maxval] are kept -- int32 in a narrow band is rare, which is what makes
the first pass small.
"""

import os
import struct
import sys

import numpy as np

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from memlib import attach, make_io, regions

CHUNK = 16 * 1024 * 1024


def store_for(proc):
    return f"delta_{proc.replace('.exe','').replace(' ','_').lower()}.npz"


def read_at(rd, addrs, kind):
    """Current value at each address. Reads contiguous spans in one call each -
    per-address reads are unusable once candidates run past a few thousand."""
    out = np.full(len(addrs), np.nan, dtype=np.float64)
    if len(addrs) == 0:
        return out
    order = np.argsort(addrs)
    sa = addrs[order]
    gaps = np.flatnonzero(np.diff(sa.astype(np.int64)) > 4096)
    starts = np.concatenate(([0], gaps + 1))
    ends = np.concatenate((gaps + 1, [len(sa)]))
    for s0, e0 in zip(starts, ends):
        lo = int(sa[s0])
        span = int(sa[e0 - 1]) - lo + 4
        if span > 8 * 1024 * 1024:
            for j in range(s0, e0):
                d = rd(int(sa[j]), 4)
                if len(d) == 4:
                    idx = order[j]
                    out[idx] = (struct.unpack("<i", d)[0] if kind[idx] == 0
                                else struct.unpack("<f", d)[0])
            continue
        blob = rd(lo, span)
        if len(blob) < span:
            continue
        buf = np.frombuffer(blob, np.uint8)
        for j in range(s0, e0):
            idx = order[j]
            o = int(sa[j]) - lo
            four = buf[o:o + 4].tobytes()
            out[idx] = (struct.unpack("<i", four)[0] if kind[idx] == 0
                        else struct.unpack("<f", four)[0])
    return out


def main():
    if len(sys.argv) < 3:
        print(__doc__)
        return 2
    proc, mode = sys.argv[1], sys.argv[2].lower()
    store = store_for(proc)

    h, pid, mb, ms = attach(proc)
    if not h:
        print(f"! {proc} not running")
        return 1
    rd, wrf, rq, rf = make_io(h)

    if mode == "snap":
        maxval = float(sys.argv[3]) if len(sys.argv) > 3 else 100000.0
        minval = float(sys.argv[4]) if len(sys.argv) > 4 else 1.0
        addrs, kinds, vals = [], [], []
        scanned = 0
        for base, size in regions(h):
            off = 0
            while off < size:
                n = min(CHUNK, size - off)
                d = rd(base + off, n)
                if d:
                    u = len(d) - (len(d) % 4)
                    if u >= 4:
                        ai = np.frombuffer(d, np.int32, count=u // 4)
                        af = np.frombuffer(d, np.float32, count=u // 4)
                        mi = (ai >= minval) & (ai <= maxval)
                        mf = np.isfinite(af) & (af >= minval) & (af <= maxval)
                        mf &= ~mi                      # do not record twice
                        for i in np.flatnonzero(mi):
                            addrs.append(base + off + int(i) * 4); kinds.append(0)
                            vals.append(float(ai[i]))
                        for i in np.flatnonzero(mf):
                            addrs.append(base + off + int(i) * 4); kinds.append(1)
                            vals.append(float(af[i]))
                    scanned += len(d)
                off += n
        np.savez_compressed(store, addrs=np.array(addrs, np.uint64),
                            kinds=np.array(kinds, np.uint8),
                            vals=np.array(vals, np.float64))
        print(f"scanned {scanned/2**30:.2f} GB")
        print(f"{len(addrs):,} candidates in [{minval:g}, {maxval:g}] saved")
        print(f"\nnow change the value, then: python scan_delta.py \"{proc}\" down")
        return 0

    if not os.path.exists(store):
        print("no snapshot yet - run `snap` first")
        return 1
    z = np.load(store)
    addrs, kinds, vals = z["addrs"], z["kinds"], z["vals"]

    if mode == "show":
        print(f"{len(addrs):,} candidates")
        for a, k, v in list(zip(addrs, kinds, vals))[:30]:
            print(f"  0x{int(a):X}  {'int  ' if k == 0 else 'float'}  {v:g}")
        return 0

    now = read_at(rd, addrs, kinds)
    if mode == "eq":
        want = float(sys.argv[3])
        tol = float(sys.argv[4]) if len(sys.argv) > 4 else 0.5
        keep = np.isfinite(now) & (np.abs(now - want) <= tol)
    elif mode == "band":
        lo_b, hi_b = float(sys.argv[3]), float(sys.argv[4])
        keep = np.isfinite(now) & (now >= lo_b) & (now <= hi_b)
    elif mode == "down":
        keep = np.isfinite(now) & (now < vals)
    elif mode == "up":
        keep = np.isfinite(now) & (now > vals)
    elif mode == "same":
        keep = np.isfinite(now) & (now == vals)
    else:
        print(__doc__)
        return 2

    addrs, kinds, vals = addrs[keep], kinds[keep], now[keep]
    np.savez_compressed(store, addrs=addrs, kinds=kinds, vals=vals)
    print(f"{mode}: {int(keep.sum()):,} remain")
    for a, k, v in list(zip(addrs, kinds, vals))[:25]:
        print(f"  0x{int(a):X}  {'int  ' if k == 0 else 'float'}  {v:g}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
