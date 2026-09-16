// lib/research.js does its sweeping through the host, so what matters here is
// the bookkeeping around it: offsets collected in the right order, static
// roots recognised by the module's bounds, and changes reported at the right
// place with both readings.

import { test } from 'node:test';
import assert from 'node:assert/strict';

import * as R from '../../configs/lib/research.js';

const BASE = 0x140000000;
const SIZE = 0x10000000; // big enough to hold the static slot below

/**
 * A host whose memory is a map of 8-byte slots. findPointers scans them the way
 * the native one scans the process.
 *
 * @param {Map<number, number>} slots address -> u64 value
 */
function host(slots, bytes = new Map()) {
    globalThis.log = () => {};
    globalThis.mem = {
        moduleBase: () => BASE,
        moduleSize: () => SIZE,
        u64: (a) => slots.get(a) ?? 0,
        findPointers(lo, hi) {
            const out = [];
            for (const [at, v] of slots) if (v >= lo && v < hi) out.push(at, v);
            return out;
        },
        readBytes: (a, n) => Array.from({ length: n }, (_, i) => bytes.get(a + i) ?? 0),
    };
}

test('a two-level path from a static root is found and resolves to the target', () => {
    const HEAP = 0x2000_0000_0000;
    const player = HEAP + 0x5000; // object holding the stat
    const target = player + 0x2c; // the stat
    const holder = HEAP + 0x9000; // object pointing at the player
    const staticSlot = BASE + 0xcbba6d8;
    const slots = new Map([
        [holder + 0x3a0, player], // [holder+3A0] -> player
        [staticSlot, holder], // [module+CBBA6D8] -> holder
        [HEAP + 0x100, HEAP + 0x7777], // unrelated noise
    ]);
    host(slots);

    const paths = R.staticPaths(target, { depth: 2 });
    assert.deepEqual(paths, [{ rva: 0xcbba6d8, offsets: [0x3a0, 0x2c] }]);
    assert.equal(R.resolve(paths[0]), target);
});

test('a pointer too far below the target is not taken for its object', () => {
    const HEAP = 0x2000_0000_0000;
    const target = HEAP + 0x5000;
    host(new Map([[BASE + 0x10, target - 0x700]]));
    assert.deepEqual(R.staticPaths(target, { depth: 1, window: 0x600 }), []);
    assert.equal(R.staticPaths(target, { depth: 1, window: 0x800 }).length, 1);
});

test('changes reports what moved around a captured address, as int and float', () => {
    const bytes = new Map();
    const put = (a, arr) => arr.forEach((b, i) => bytes.set(a + i, b));
    const f32 = (v) => { const u = new Uint8Array(4); new DataView(u.buffer).setFloat32(0, v, true); return [...u]; };
    const MAX = 0x3000_0000;
    put(MAX, f32(380)); // the known maximum
    put(MAX + 8, f32(380)); // current health beside it
    host(new Map(), bytes);

    const snap = R.capture([MAX], 0x10);
    put(MAX + 8, f32(311)); // take damage
    const fell = R.changes(snap, (c) => c.nowFloat < c.wasFloat);

    assert.equal(fell.length, 1);
    assert.equal(fell[0].at, MAX + 8);
    assert.equal(fell[0].offset, 8);
    assert.equal(fell[0].wasFloat, 380);
    assert.equal(fell[0].nowFloat, 311);
});
