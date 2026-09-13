"""
Find CURRENT health by capturing before and after taking damage.

The stats panel only shows max health. Current health lives next to it, so
this watches the neighbourhood of every max-health match and reports values
that dropped.

    python find_current_hp.py full     at full health
    <take damage>
    python find_current_hp.py hurt     diffs and reports what fell

    python find_current_hp.py reset

Repeat the pair a couple of times (heal up, take damage again) to drop
coincidences.
"""

import json
import os
import struct
import sys

import numpy as np

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from memlib import attach, make_io, regions

STORE = "hp_captures.json"
PROC = "Dragon Age The Veilguard.exe"
MAX_HP = 760
WIN = 0x100          # bytes either side of a max-health match
CHUNK = 16 * 1024 * 1024


def candidates(rd, h):
    """Addresses holding the max-health value."""
    hits = []
    for base, size in regions(h):
        off = 0
        while off < size:
            n = min(CHUNK, size - off)
            d = rd(base + off, n)
            if d:
                u = len(d) - (len(d) % 4)
                if u >= 4:
                    a = np.frombuffer(d, np.int32, count=u // 4)
                    hits.extend(base + off + int(i) * 4
                                for i in np.flatnonzero(a == MAX_HP))
            off += n
    return hits


def main():
    mode = sys.argv[1].lower() if len(sys.argv) > 1 else "status"
    if mode == "reset":
        if os.path.exists(STORE):
            os.remove(STORE)
        print("cleared")
        return 0

    h, pid, mb, ms = attach(PROC)
    if not h:
        print("! game not running")
        return 1
    rd, wrf, rq, rf = make_io(h)

    if mode == "full":
        cands = candidates(rd, h)
        print(f"{len(cands):,} max-health matches; capturing their surroundings")
        snap = {}
        for a in cands:
            blob = rd(a - WIN, WIN * 2)
            if len(blob) == WIN * 2:
                snap[str(a)] = list(blob)
        json.dump(snap, open(STORE, "w"))
        print(f"captured {len(snap):,} windows")
        print("\nnow TAKE DAMAGE, then:  python find_current_hp.py hurt")
        return 0

    if mode == "watch":
        # Sample repeatedly; a real health field is seen AT max and also in a
        # plausible wounded band. Addresses that hold 760 and also 1 are
        # recycled allocations, not health - which is what dominated a first
        # attempt that only required "below max".
        secs = float(sys.argv[2]) if len(sys.argv) > 2 else 60.0
        cands = candidates(rd, h)
        print("watching " + format(len(cands), ",") + " matches for "
              + format(secs, ".0f") + "s")
        print("go take damage - a solid hit, and survive it")
        import time
        at_max, seen_vals = {}, {}
        end_t = time.time() + secs
        passes = 0
        while time.time() < end_t:
            passes += 1
            for a in cands:
                blob = rd(a - WIN, WIN * 2)
                if len(blob) != WIN * 2:
                    continue
                arr = np.frombuffer(blob, np.int32)
                for i in np.flatnonzero(arr == MAX_HP):
                    at_max[(a, int(i) * 4 - WIN)] = True
                lo_band = int(MAX_HP * 0.15)
                hi_band = int(MAX_HP * 0.97)
                m = (arr >= lo_band) & (arr <= hi_band)
                for i in np.flatnonzero(m):
                    key = (a, int(i) * 4 - WIN)
                    seen_vals.setdefault(key, set()).add(int(arr[i]))
        print(str(passes) + " passes")
        hits = [(k, sorted(seen_vals[k])) for k in at_max if k in seen_vals]
        hits.sort(key=lambda t: -len(t[1]))
        print(str(len(hits)) + " value(s) hit " + str(MAX_HP)
              + " and also sat in a wounded band:")
        for (a, off), vals in hits[:20]:
            shown = ", ".join(str(v) for v in vals[:6])
            more = " ..." if len(vals) > 6 else ""
            print("  0x%X%+d   %d distinct: %s%s" % (a, off, len(vals), shown, more))
        if not hits:
            print("  none - try a bigger hit, or heal and get hit again")
        json.dump({("%X%+d" % k): sorted(v) for k, v in seen_vals.items()},
                  open("hp_observed.json", "w"))
        print("raw observations -> hp_observed.json")
        return 0

    if mode != "hurt":
        print(__doc__)
        return 2

    if not os.path.exists(STORE):
        print("no 'full' capture yet")
        return 1
    before = json.load(open(STORE))
    print(f"comparing {len(before):,} windows\n")

    drops = []
    for key, old_bytes in before.items():
        a = int(key)
        blob = rd(a - WIN, WIN * 2)
        if len(blob) != WIN * 2:
            continue
        old = np.frombuffer(bytes(old_bytes), np.int32)
        new = np.frombuffer(blob, np.int32)
        for i in range(len(old)):
            o, n = int(old[i]), int(new[i])
            # a health value: was at max, dropped, still positive
            if o == MAX_HP and 0 < n < o:
                drops.append((a, (i * 4) - WIN, o, n))

    print(f"{len(drops)} value(s) were {MAX_HP} and are now lower:\n")
    for a, off, o, n in drops[:25]:
        print(f"  0x{a:X}{off:+d}   {o} -> {n}")
    if not drops:
        print("  none - did you actually lose health? try again")
    else:
        print("\nre-run the pair (heal, damage) to confirm which tracks reliably")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
