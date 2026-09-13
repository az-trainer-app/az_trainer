import { test } from 'node:test';
import assert from 'node:assert/strict';

import { BASE, createMem, install } from './fake-mem.mjs';

const UNREAL = new URL('../../configs/lib/unreal.js', import.meta.url).href;
let fresh = 0;
/** A new copy of unreal.js: it caches GWorld, which must not leak between tests. */
const load = () => import(`${UNREAL}?copy=${fresh++}`);

const le64 = (v) => {
    const out = [];
    let hi = Math.floor(v / 0x100000000);
    let lo = v >>> 0;
    for (let i = 0; i < 4; i++, lo >>>= 8) out.push(lo & 0xff);
    for (let i = 0; i < 4; i++, hi >>>= 8) out.push(hi & 0xff);
    return out;
};
const f32 = (v) => {
    const b = new Uint8Array(4);
    new DataView(b.buffer).setFloat32(0, v, true);
    return [...b];
};

const HEAP = 0x2000_0000_0000;
const GWORLD_HIT = BASE + 0x5000;
const GWORLD = GWORLD_HIT + 7 + 0x1000; // mov rax,[rip+0x1000] is 7 bytes
const WORLD = HEAP + 0x100000;
const GAME_INSTANCE = HEAP + 0x200000;
const LOCAL_PLAYERS = HEAP + 0x300000;
const LOCAL_PLAYER = HEAP + 0x400000;
const CONTROLLER = HEAP + 0x500000;
const PAWN = HEAP + 0x600000;
const LEVEL = HEAP + 0xd00000;
const WORLD_SETTINGS = HEAP + 0xe00000;

/** A fake UE5 process with the whole walk from GWorld to the pawn in place. */
function ue5World({ inMenu = false } = {}) {
    const fake = createMem({ aob: () => GWORLD_HIT });
    fake.poke(GWORLD_HIT + 3, [0x00, 0x10, 0x00, 0x00]);
    fake.poke(GWORLD, le64(WORLD));
    fake.poke(WORLD + 0x1d8, le64(GAME_INSTANCE));
    fake.poke(GAME_INSTANCE + 0x38, le64(LOCAL_PLAYERS));
    fake.poke(LOCAL_PLAYERS, le64(LOCAL_PLAYER));
    fake.poke(LOCAL_PLAYER + 0x30, le64(inMenu ? 0 : CONTROLLER));
    fake.poke(CONTROLLER + 0x2f8, le64(PAWN));
    fake.poke(WORLD + 0x30, le64(LEVEL));
    fake.poke(LEVEL + 0x2b0, le64(WORLD_SETTINGS));
    install(fake);
    return fake;
}

test('walks GWorld to the player controller and pawn', async () => {
    ue5World();
    const UE = await load();
    assert.equal(UE.gworld(), GWORLD);
    assert.equal(UE.playerController(), CONTROLLER);
    assert.equal(UE.pawn(), PAWN);
});

test('finds WorldSettings through the persistent level', async () => {
    ue5World();
    const UE = await load();
    assert.equal(UE.worldSettings(), WORLD_SETTINGS);
});

test('reports no pawn while in a menu', async () => {
    ue5World({ inMenu: true });
    const UE = await load();
    assert.equal(UE.playerController(), 0);
    assert.equal(UE.pawn(), 0);
});

test('reports nothing when GWorld is not found', async () => {
    install(createMem());
    const UE = await load();
    assert.equal(UE.gworld(), 0);
    assert.equal(UE.pawn(), 0);
});

test('chain stops at the first null link', async () => {
    const fake = ue5World();
    const UE = await load();
    fake.poke(PAWN + 0x10, le64(HEAP + 0x700000));
    fake.poke(HEAP + 0x700000 + 0x20, le64(HEAP + 0x800000));
    assert.equal(UE.chain(PAWN, [0x10, 0x20]), HEAP + 0x800000);
    fake.poke(PAWN + 0x10, le64(0));
    assert.equal(UE.chain(PAWN, [0x10, 0x20]), 0);
});

test('hold sets both halves of an attribute to its maximum', async () => {
    const fake = ue5World();
    const UE = await load();
    const set = HEAP + 0x900000;
    fake.poke(set + 0x40 + 0xc, f32(300)); // Health.Current
    fake.poke(set + 0x50 + 0xc, f32(1746)); // MaxHealth.Current

    assert.deepEqual(UE.hold(set, 0x40, 0x50, false), { cur: 300, max: 1746 }, 'read only');
    assert.equal(UE.attrGet(set, 0x40), 300, 'unchanged while off');

    assert.deepEqual(UE.hold(set, 0x40, 0x50, true), { cur: 1746, max: 1746 });
    assert.equal(fake.mem.f32(set + 0x40 + 0x8), 1746, 'BaseValue');
    assert.equal(fake.mem.f32(set + 0x40 + 0xc), 1746, 'CurrentValue');
    assert.equal(UE.fmt('Health', UE.hold(set, 0x40, 0x50, false)), 'Health  1746 / 1746');
});

test('holdProduct uses segments x per-segment as the maximum', async () => {
    const fake = ue5World();
    const UE = await load();
    const set = HEAP + 0xa00000;
    fake.poke(set + 0x30 + 0xc, f32(939)); // Blood
    fake.poke(set + 0x90 + 0xc, f32(4)); // segments
    fake.poke(set + 0xa0 + 0xc, f32(313)); // per segment
    assert.deepEqual(UE.holdProduct(set, 0x30, 0x90, 0xa0, true), { cur: 1252, max: 1252 });
});

test('no reading when the maximum is not there yet', async () => {
    const fake = ue5World();
    const UE = await load();
    const set = HEAP + 0xb00000;
    fake.poke(set + 0x50 + 0x8, f32(0));
    fake.poke(set + 0x50 + 0xc, f32(0));
    assert.equal(UE.hold(set, 0x40, 0x50, true), null);
    assert.equal(UE.fmt('Health', null), undefined);
});

test('poke restores the original value when switched off', async () => {
    const fake = ue5World();
    const UE = await load();
    const addr = HEAP + 0xc00000;
    fake.poke(addr, f32(600));
    UE.poke(addr, 2400, true);
    UE.poke(addr, 2400, true); // repeated ticks must not overwrite the saved original
    assert.equal(fake.mem.f32(addr), 2400);
    UE.poke(addr, 2400, false);
    assert.equal(fake.mem.f32(addr), 600);
});
