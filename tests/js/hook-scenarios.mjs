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
    }));

    scenario('patch + restore', () => {
        const h = H.patch(BASE + 0x100, [0x90, 0x90, 0x90]);
        const before = data(h);
        h.restore();
        return before;
    });

    scenario('patch refuses empty input', () => [H.patch(0, [0x90]), H.patch(BASE, [])]);

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

    scenario('holdFieldAtSibling', () => {
        const h = H.holdFieldAtSibling(BASE + 0x900, 7, 0x08, 0xf8, 0x40, 0x44);
        const before = data(h);
        h.restore();
        return before;
    });

    return out;
}
