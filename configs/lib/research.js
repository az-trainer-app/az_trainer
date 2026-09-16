// Research helpers for the devtools channel: finding, in the running game,
// what a config will need. Nothing here belongs in a shipped option.
//
// Value searches are native (mem.scanStart / scanNext / scanResults), since
// they sweep all of the game's memory; these build on them and on
// mem.findPointers.

/**
 * Static pointer paths to `target`: chains starting at a fixed offset inside
 * the game's module, which survive a restart where heap addresses do not.
 *
 * Works backwards: every pointer landing up to `window` bytes before the
 * target is a candidate field; one stored inside the module is a static root,
 * and the rest become the next level's targets. Each level scans all readable
 * memory, so depth 3 is slow - start with 2.
 *
 * @param {number} target
 * @param {{ depth?: number, window?: number, fanout?: number, max?: number }} [opts]
 *   `window` bytes a field may sit past the pointer to its object (0x600),
 *   `fanout` pointers chased per level (40), `max` paths returned (20)
 * @returns {{ rva: number, offsets: number[] }[]} each meaning
 *   `[[module + rva] + offsets[0]] ... + offsets[n-1]`, as `resolve` follows it
 */
export function staticPaths(target, { depth = 2, window = 0x600, fanout = 40, max = 20 } = {}) {
    const base = mem.moduleBase();
    const end = base + mem.moduleSize();
    /** @type {{ rva: number, offsets: number[] }[]} */
    const found = [];
    let level = [{ at: target, offsets: /** @type {number[]} */ ([]) }];
    for (let d = 0; d < depth && level.length && found.length < max; d++) {
        /** @type {typeof level} */
        const next = [];
        const seen = new Set();
        for (const { at, offsets } of level.slice(0, fanout)) {
            const hits = mem.findPointers(at - window, at + 1);
            for (let i = 0; i < hits.length; i += 2) {
                const slot = hits[i];
                const path = [at - hits[i + 1], ...offsets];
                if (slot >= base && slot < end) {
                    found.push({ rva: slot - base, offsets: path });
                } else if (!seen.has(slot)) {
                    seen.add(slot);
                    next.push({ at: slot, offsets: path });
                }
            }
        }
        level = next;
    }
    return found.slice(0, max);
}

/**
 * Follow a path from `staticPaths`: dereference every offset but the last,
 * which is added.
 *
 * @param {{ rva: number, offsets: number[] }} path
 * @returns {number} 0 if a link is null
 */
export function resolve({ rva, offsets }) {
    let p = mem.u64(mem.moduleBase() + rva);
    for (let i = 0; i < offsets.length - 1; i++) {
        if (!p) return 0;
        p = mem.u64(p + offsets[i]);
    }
    return p ? p + offsets[offsets.length - 1] : 0;
}

/**
 * @typedef {{ at: number, span: number, bytes: number[] }} Window
 */

/**
 * Remember the bytes around each address, for `changes` to compare against
 * later - how a value that is hard to search for directly (current health next
 * to a known maximum) is found: capture, change it in the game, compare.
 *
 * @param {number[]} addrs
 * @param {number} [span] bytes either side of each address
 * @returns {Window[]}
 */
export function capture(addrs, span = 0x100) {
    return addrs.map((at) => ({ at, span, bytes: mem.readBytes(at - span, span * 2) }));
}

/**
 * Every 4-byte slot around the captured addresses that holds something else
 * now, read both as an int and as a float - engines use either.
 *
 * @param {Window[]} windows from `capture`
 * @param {(c: Change) => boolean} [keep] e.g. `(c) => c.nowFloat < c.wasFloat`
 * @returns {Change[]}
 */
export function changes(windows, keep = () => true) {
    /** @type {Change[]} */
    const out = [];
    for (const { at, span, bytes } of windows) {
        const now = mem.readBytes(at - span, span * 2);
        if (bytes.length !== span * 2 || now.length !== span * 2) continue;
        const before = new DataView(Uint8Array.from(bytes).buffer);
        const after = new DataView(Uint8Array.from(now).buffer);
        for (let o = 0; o + 4 <= span * 2; o += 4) {
            if (before.getUint32(o, true) === after.getUint32(o, true)) continue;
            const c = {
                at: at - span + o,
                offset: o - span,
                was: before.getInt32(o, true),
                now: after.getInt32(o, true),
                wasFloat: before.getFloat32(o, true),
                nowFloat: after.getFloat32(o, true),
            };
            if (keep(c)) out.push(c);
        }
    }
    return out;
}

/**
 * @typedef {{ at: number, offset: number, was: number, now: number, wasFloat: number, nowFloat: number }} Change
 *   `offset` from the captured address; `was`/`now` as int32, `wasFloat`/`nowFloat` as float32
 */
