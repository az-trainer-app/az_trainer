import { test } from 'node:test';
import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';

import { BASE, createMem, install } from './fake-mem.mjs';
import { runScenarios } from './hook-scenarios.mjs';

const HOOK_URL = new URL('../../configs/lib/hook.js', import.meta.url).href;
const golden = JSON.parse(readFileSync(new URL('./golden/hook.json', import.meta.url), 'utf8'));
const actual = JSON.parse(JSON.stringify(await runScenarios(HOOK_URL)));

// The machine code each builder injects must not change by accident. If a
// change is intended, verify it in a game, then regenerate golden/hook.json.
for (const name of Object.keys(golden)) {
    test(`golden bytes: ${name}`, () => {
        assert.deepEqual(actual[name], golden[name]);
    });
}

test('every scenario has a golden record', () => {
    assert.deepEqual(Object.keys(actual).sort(), Object.keys(golden).sort());
});

// Invariants, stated directly rather than only through the golden bytes.

const H = await import(HOOK_URL);

/** Builders that leave a jump in the game, with arguments for each. */
const detours = {
    detour: (t) => H.detour(t, 7, [0x90]),
    replace: (t) => H.replace(t, 7, [0x90]),
    guardedScale: (t) => H.guardedScale(t, 5, 0x20),
    guardedLoadMax: (t) => H.guardedLoadMax(t, 5, 0x08, 0x0c),
    holdFieldAtSibling: (t) => H.holdFieldAtSibling(t, 7, 0x08, 0xf8, 0x40, 0x44),
    logFlaggedEntities: (t) => H.logFlaggedEntities(t, 7, 0x08, 0xf8),
};

for (const [name, build] of Object.entries(detours)) {
    test(`${name}: jumps to its cave, then restores without a trace`, () => {
        const fake = createMem();
        install(fake);
        const target = BASE + 0x1000;
        const steal = fake.original(target, 7);

        const h = build(target);
        assert.ok(h, 'installed');

        const site = fake.read(target, 5);
        assert.equal(site[0], 0xe9, 'E9 rel32 at the hook site');
        const rel = new DataView(Uint8Array.from(site.slice(1)).buffer).getInt32(0, true);
        assert.equal(target + 5 + rel, h.cave, 'the jump lands on the cave');

        h.restore();
        assert.deepEqual(fake.read(target, 7), steal, 'original bytes back');
        assert.deepEqual(fake.frees, [h.cave], 'cave released');
    });

    test(`${name}: refuses a steal shorter than a jump`, () => {
        install(createMem());
        assert.equal(
            name === 'guardedScale' || name === 'guardedLoadMax'
                ? H[name](BASE, 4, 0, 0)
                : H[name](BASE, 4, 0, 0, 0, 0),
            null,
        );
    });
}

test('detour: cave runs your code, then the stolen bytes, then jumps back', () => {
    const fake = createMem();
    install(fake);
    const target = BASE + 0x2000;
    const stolen = fake.original(target, 7);

    const h = H.detour(target, 7, [0xcc, 0xcc]);
    const cave = fake.read(h.cave, 2 + 7 + 14);
    assert.deepEqual(cave.slice(0, 2), [0xcc, 0xcc]);
    assert.deepEqual(cave.slice(2, 9), stolen);
    assert.deepEqual(cave.slice(9), H.jmpAbs(target + 7));
    assert.deepEqual(fake.read(target + 5, 2), [0x90, 0x90], 'NOP padding after the jump');
});

test('patch: restore puts back exactly what was there', () => {
    const fake = createMem();
    install(fake);
    const at = BASE + 0x3000;
    const before = fake.original(at, 3);
    const h = H.patch(at, [0xeb, 0x06, 0x90]);
    assert.deepEqual(fake.read(at, 3), [0xeb, 0x06, 0x90]);
    h.restore();
    assert.deepEqual(fake.read(at, 3), before);
});

test('i32 and u64 encode little-endian', () => {
    assert.deepEqual(H.i32(0x12345678), [0x78, 0x56, 0x34, 0x12]);
    assert.deepEqual(H.i32(-1), [0xff, 0xff, 0xff, 0xff]);
    assert.deepEqual(H.u64(0x140001234), [0x34, 0x12, 0x00, 0x40, 0x01, 0x00, 0x00, 0x00]);
});
