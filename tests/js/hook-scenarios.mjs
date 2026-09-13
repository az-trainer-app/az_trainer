// Every hook.js builder exercised with fixed inputs against a fresh fake host.
//
// The recorded output - return values plus every byte written, allocated and
// freed - is the golden file golden/hook.json. It was generated from the
// hook.js that was verified in the real games, so a change here means the
// machine code a script injects has changed.

import { BASE, createMem, install } from './fake-mem.mjs';

/** Handle fields worth comparing: data, not functions. */
function data(handle) {
    if (!handle) return handle;
    return Object.fromEntries(Object.entries(handle).filter(([, v]) => typeof v !== 'function'));
}

/**
 * @param {string} hookUrl module URL of the hook.js under test
 */
export async function runScenarios(hookUrl) {
    const H = await import(hookUrl);
    const out = {};

    /** Run `fn` against a fresh fake and record everything it did. */
    const scenario = (name, fn) => {
        const fake = createMem();
        install(fake);
        const result = fn(fake);
        out[name] = {
            result,
            writes: fake.writes.map((w) => ({ addr: w.addr, bytes: w.bytes })),
            allocs: fake.allocs,
            frees: fake.frees,
        };
    };

    scenario('encoders', () => ({
        i32: [H.i32(0), H.i32(5000), H.i32(-1), H.i32(-0x7ffffff0), H.i32(0x12345678)],
        u64: [H.u64(0), H.u64(BASE + 0x1234), H.u64(0x7ffe12345678)],
        jmpAbs: H.jmpAbs(BASE + 0x1000),
        movRbxImm: H.movRbxImm(0x1c8, 99),
        f32bits: [H.f32bits(1), H.f32bits(-1), H.f32bits(1.5), H.f32bits(100000)],
    }));

    scenario('patch + restore', () => {
        const h = H.patch(BASE + 0x100, [0x90, 0x90, 0x90]);
        const before = data(h);
        h.restore();
        return before;
    });

    scenario('patch refuses empty input', () => [H.patch(0, [0x90]), H.patch(BASE, [])]);

    scenario('nopOut', () => H.nopOut(BASE + 0x200, 4));

    scenario('bytesMatch', (fake) => {
        fake.poke(BASE + 0x300, [0x48, 0x8b, 0x05]);
        return [
            H.bytesMatch(BASE + 0x300, [0x48, 0x8b, 0x05]),
            H.bytesMatch(BASE + 0x300, [0x48, null, 0x05]),
            H.bytesMatch(BASE + 0x300, [0x48, 0x8b, 0x06]),
        ];
    });

    scenario('detour + restore', () => {
        const h = H.detour(BASE + 0x400, 7, [0x41, 0x81, 0xc6, ...H.i32(5000)]);
        const before = data(h);
        h.restore();
        return before;
    });

    scenario('detour refuses short steal', () => [
        H.detour(BASE + 0x400, 4, [0x90]),
        H.detour(0, 7, [0x90]),
    ]);

    scenario('replace + restore', () => {
        const h = H.replace(BASE + 0x500, 6, H.movRbxImm(0x1c8, 7));
        const before = data(h);
        h.restore();
        return before;
    });

    scenario('toggle on/off', () => {
        const state = { handle: null };
        const on = H.toggle(state, true, BASE + 0x600, 5, () => [0x90]);
        const again = H.toggle(state, true, BASE + 0x600, 5, () => [0xcc]);
        const off = H.toggle(state, false, BASE + 0x600, 5, () => [0x90]);
        return [on, again, off, state.handle];
    });

    scenario('guardedScale', () => {
        const h = H.guardedScale(BASE + 0x700, 5, 0x20);
        const before = data(h);
        h.setGuard(BASE + 0x9000);
        h.setMult(2);
        const counts = h.counts();
        h.restore();
        return { before, counts };
    });

    scenario('guardedLoadMax', () => {
        const h = H.guardedLoadMax(BASE + 0x800, 5, 0x08, 0x0c);
        const before = data(h);
        h.setGuard(BASE + 0xa000);
        h.restore();
        return before;
    });

    scenario('holdFieldAtSibling', () => {
        const h = H.holdFieldAtSibling(BASE + 0x900, 7, 0x08, 0xf8, 0x40, 0x44);
        const before = data(h);
        h.restore();
        return before;
    });

    scenario('logFlaggedEntities', () => {
        const h = H.logFlaggedEntities(BASE + 0xa00, 7, 0x08, 0xf8);
        const before = data(h);
        const seen = h.seen();
        h.restore();
        return { before, seen };
    });

    return out;
}
