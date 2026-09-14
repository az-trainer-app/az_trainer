// The Wukong config records the player's attribute object off a hooked call
// rather than walking GWorld, so what these cover is that chain: the recorder
// goes in, the recorded pointer is followed to the attributes, the writes land
// on the right slots, and a near-miss object is not mistaken for the player's.

import { test } from 'node:test';
import assert from 'node:assert/strict';

import { BASE, createMem, install } from './fake-mem.mjs';

const WUKONG = new URL('../../configs/games/wukong.js', import.meta.url).href;
let fresh = 0;
/** A new copy of the config, so its cached state does not leak between tests. */
const load = () => import(`${WUKONG}?copy=${fresh++}`);

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

// must match the config
const ATTRS = 0x20;
const SET_SLOT = 0x100;
const ACTOR_SLOT = 0x108;
const GUARD_SLOT = 0x80;
const SEEN_SLOT = 0x90;
const SET_CHAIN = [0x30, 0x38];

const HP_MAX = 1;
const MP_MAX = 2;
const STAMINA_MAX = 8;
const VESSEL_MAX = 16;
const SPIRIT_MAX = 17;
const FOCUS_MAX = 39;
const HP_MAX_BASE = 101;
const STAMINA_MAX_BASE = 108;
const ENERGY_INCREASE_SPEED = 14;
const HP = 151;
const MP = 152;
const ATK = 153;
const DEF = 154;
const STAMINA = 158;
const STAMINA_RECOVER = 159;
const FOCUS = 191;
const VESSEL = 201;
const SPIRIT = 202;

const HEAP = 0x2000_0000_0000;
const SITE = BASE + 0x5000; // where the recorded call lives
const CD_SITE = BASE + 0x9000; // a cooldown countdown
const SET = HEAP + 0x100000; // what the call names (rcx)
const MID = HEAP + 0x200000; // set + 0x30
const ATTRS_OBJ = HEAP + 0x300000; // mid + 0x38
const ACTOR = HEAP + 0x400000;

const slot = (attrs, index) => attrs + ATTRS + index * 4;
const byName = (options, name) => options.find((o) => o.name === name);

/** Lay out an attribute array with a plausible set of stats. */
function attributesAt(fake, at, { hp = 116, hpMax = 380, st = 40, stMax = 220 } = {}) {
    fake.poke(slot(at, HP_MAX), f32(hpMax));
    fake.poke(slot(at, STAMINA_MAX), f32(stMax));
    fake.poke(slot(at, HP_MAX_BASE), f32(hpMax));
    fake.poke(slot(at, STAMINA_MAX_BASE), f32(stMax));
    fake.poke(slot(at, HP), f32(hp));
    fake.poke(slot(at, STAMINA), f32(st));
    fake.poke(slot(at, MP_MAX), f32(220));
    fake.poke(slot(at, MP), f32(25));
    fake.poke(slot(at, FOCUS_MAX), f32(210));
    fake.poke(slot(at, FOCUS), f32(15));
    fake.poke(slot(at, SPIRIT_MAX), f32(171));
    fake.poke(slot(at, SPIRIT), f32(0));
    fake.poke(slot(at, VESSEL_MAX), f32(60));
    fake.poke(slot(at, VESSEL), f32(10));
    // the combat stats the config recognises the object by - it never writes
    // these, so they stay honest even while the bars are flooded
    fake.poke(slot(at, ATK), f32(36));
    fake.poke(slot(at, DEF), f32(59));
    fake.poke(slot(at, STAMINA_RECOVER), f32(50));
    return at;
}

/** A process with the recorded call present but not yet executed. */
function game() {
    const fake = createMem({ aob: () => SITE, aobAll: () => [] });
    install(fake);
    return fake;
}

/** The cave the config just allocated. */
const caveOf = (fake, n = 0) => fake.allocs[n].cave;

/** Pretend the game ran the hooked call, handing us rcx and the actor. */
function recordCall(fake, cave, { set = SET, actor = ACTOR } = {}) {
    fake.poke(cave + SET_SLOT, le64(set));
    fake.poke(cave + ACTOR_SLOT, le64(actor));
    fake.poke(set + SET_CHAIN[0], le64(MID));
    fake.poke(MID + SET_CHAIN[1], le64(ATTRS_OBJ));
}

test('installs the recorder and writes nothing until the call has run', async () => {
    const fake = game();
    const cfg = await load();
    assert.equal(cfg.process, 'b1-Win64-Shipping.exe');
    assert.ok(cfg.live(), 'the site to hook is there');

    byName(cfg.options, 'Infinite Health').tick({ on: true, mult: 1 });
    assert.equal(fake.allocs.length, 1, 'one cave for the recorder');

    // the cave's jump is written at the site, but the game has not run it yet
    const cave = caveOf(fake);
    assert.equal(fake.mem.u64(cave + SET_SLOT), 0, 'nothing recorded');
    const stats = fake.writes.filter((w) => w.addr > 0x1000_0000_0000);
    assert.deepEqual(stats, [], 'no stat writes without a recorded call');
});

test('follows the recorded pointer to the attributes and pins each bar', async () => {
    const fake = game();
    const cfg = await load();
    byName(cfg.options, 'Infinite Health').tick({ on: true, mult: 1 });
    const cave = caveOf(fake);

    attributesAt(fake, ATTRS_OBJ);
    recordCall(fake, cave);

    for (const o of cfg.options) if (o.tick) o.tick({ on: true, mult: 1 });

    // health and stamina are flooded rather than pinned at max, so neither a
    // hit nor a dodge ever shows on the bar
    assert.equal(fake.mem.f32(slot(ATTRS_OBJ, HP)), 999999, 'health');
    assert.equal(fake.mem.f32(slot(ATTRS_OBJ, STAMINA)), 999999, 'stamina');
    assert.equal(fake.mem.f32(slot(ATTRS_OBJ, MP)), 220, 'mana');
    assert.equal(fake.mem.f32(slot(ATTRS_OBJ, SPIRIT)), 171, 'spirit energy');
    assert.equal(fake.mem.f32(slot(ATTRS_OBJ, VESSEL)), 60, 'vessel energy');
    // Focus is asked for one past the cap so the last point is credited
    assert.equal(fake.mem.f32(slot(ATTRS_OBJ, FOCUS)), 211, 'focus');
});

test('an object that is only nearly right is not taken for the attributes', async () => {
    const fake = game();
    const cfg = await load();
    byName(cfg.options, 'Infinite Health').tick({ on: true, mult: 1 });
    const cave = caveOf(fake);

    // every bar reads plausibly, but it carries no combat stats - so it is
    // some other float block, not a character's
    fake.poke(slot(ATTRS_OBJ, HP_MAX), f32(380));
    fake.poke(slot(ATTRS_OBJ, HP), f32(100));
    fake.poke(slot(ATTRS_OBJ, STAMINA_MAX), f32(220));
    fake.poke(slot(ATTRS_OBJ, STAMINA), f32(50));
    fake.poke(slot(ATTRS_OBJ, HP_MAX_BASE), f32(380));
    fake.poke(slot(ATTRS_OBJ, STAMINA_MAX_BASE), f32(220));
    fake.poke(slot(ATTRS_OBJ, MP_MAX), f32(220));
    fake.poke(slot(ATTRS_OBJ, MP), f32(25));
    fake.poke(slot(ATTRS_OBJ, ATK), f32(0));
    fake.poke(slot(ATTRS_OBJ, DEF), f32(0));
    fake.poke(slot(ATTRS_OBJ, STAMINA_RECOVER), f32(0));
    recordCall(fake, cave);

    byName(cfg.options, 'Infinite Health').tick({ on: true, mult: 1 });
    assert.equal(fake.mem.f32(slot(ATTRS_OBJ, HP)), 100, 'left alone');
});

test('a bar already full is not rewritten every tick', async () => {
    const fake = game();
    const cfg = await load();
    byName(cfg.options, 'Infinite Health').tick({ on: true, mult: 1 });
    const cave = caveOf(fake);
    attributesAt(fake, ATTRS_OBJ, { hp: 380, hpMax: 380 });
    recordCall(fake, cave);

    byName(cfg.options, 'Infinite Health').tick({ on: true, mult: 1 });
    const before = fake.writes.length;
    byName(cfg.options, 'Infinite Health').tick({ on: true, mult: 1 });
    assert.equal(fake.writes.length, before, 'already at maximum');
});

test('switching everything off takes the recorder back out', async () => {
    const fake = game();
    const cfg = await load();
    const health = byName(cfg.options, 'Infinite Health');

    health.tick({ on: true, mult: 1 });
    const original = fake.original(SITE, 8);
    assert.notDeepEqual(fake.read(SITE, 8), original, 'hooked');

    health.tick({ on: false, mult: 1 });
    assert.deepEqual(fake.read(SITE, 8), original, 'site restored when the last option goes off');
});

test('No Cooldowns hooks the spell sites and frees the transform gauge', async () => {
    const sites = [CD_SITE, CD_SITE + 0x400];
    const fake = createMem({ aob: () => SITE, aobAll: () => sites });
    install(fake);
    const cfg = await load();
    const cd = byName(cfg.options, 'No Cooldowns');

    cd.tick({ on: true, mult: 1 });
    const cave = caveOf(fake, 0);
    attributesAt(fake, ATTRS_OBJ);
    recordCall(fake, cave);
    cd.tick({ on: true, mult: 1 });

    assert.equal(fake.allocs.length, 1 + sites.length, 'recorder plus one cave per site');
    for (let n = 1; n <= sites.length; n++) {
        const code = fake.read(caveOf(fake, n), 42);
        const derefsRsi = code.some((b, i) => b === 0x8b && (code[i + 1] & 0xc7) === 0x46);
        assert.ok(!derefsRsi, 'the cave never reads through rsi - it is not a pointer on every site');
        assert.equal(fake.mem.u64(caveOf(fake, n) + GUARD_SLOT), 0, 'nothing approved before a cooldown is seen');
    }

    // the player's spell bar parks its component; an enemy's site parks one
    // belonging to someone else, and another parks a value that is no pointer
    const PLAYER_BAR = HEAP + 0x600000;
    const ENEMY_BAR = HEAP + 0x700000;
    fake.poke(PLAYER_BAR + 0x10, le64(ACTOR));
    fake.poke(ENEMY_BAR + 0x10, le64(HEAP + 0x800000));
    fake.poke(caveOf(fake, 1) + SEEN_SLOT, le64(PLAYER_BAR));
    fake.poke(caveOf(fake, 2) + SEEN_SLOT, le64(ENEMY_BAR));
    cd.tick({ on: true, mult: 1 });
    assert.equal(fake.mem.u64(caveOf(fake, 1) + GUARD_SLOT), PLAYER_BAR, "the player's bar is approved");
    assert.equal(fake.mem.u64(caveOf(fake, 2) + GUARD_SLOT), 0, "an enemy's bar is not");

    // a stray value seen later does not unseat the approved bar...
    fake.poke(caveOf(fake, 1) + SEEN_SLOT, le64(0x40000000));
    cd.tick({ on: true, mult: 1 });
    assert.equal(fake.mem.u64(caveOf(fake, 1) + GUARD_SLOT), PLAYER_BAR, 'approval kept');
    // ...but a bar that stops belonging to the player is dropped
    fake.poke(PLAYER_BAR + 0x10, le64(HEAP + 0x900000));
    cd.tick({ on: true, mult: 1 });
    assert.equal(fake.mem.u64(caveOf(fake, 1) + GUARD_SLOT), 0, 'approval dropped');
    // transformation is not a countdown at all, just a refill rate
    assert.equal(fake.mem.f32(slot(ATTRS_OBJ, ENERGY_INCREASE_SPEED)), 10000, 'transform gauge');

    const originals = sites.map((s) => fake.original(s + 8, 9));
    cd.tick({ on: false, mult: 1 });
    sites.forEach((s, i) => {
        assert.deepEqual(fake.read(s + 8, 9), originals[i], 'cooldown site restored');
    });
});

test('Speed scales the pawn the movement site is working on', async () => {
    const SPD_SITE = BASE + 0xb000;
    const PAWN_SITE = BASE + 0xd000;
    const PAWN = HEAP + 0x500000;
    const FACTOR_SLOT = 0x88;
    const PAWN_SLOT = 0x100;
    // three different signatures, three different sites
    const fake = createMem({
        aob: (sig) =>
            sig.startsWith('F3 0F 10 8B') ? SPD_SITE : sig.startsWith('41 8B 87') ? PAWN_SITE : SITE,
        aobAll: () => [],
    });
    install(fake);
    const cfg = await load();
    const speed = byName(cfg.options, 'Speed');

    speed.tick({ on: true, mult: 2 });
    assert.equal(fake.allocs.length, 3, 'actor recorder, speed cave, pawn recorder');
    const cave = caveOf(fake, 1);
    const pawnCave = caveOf(fake, 2);

    // nothing to guard with until the game has run the pawn instruction
    assert.equal(fake.mem.u64(cave + GUARD_SLOT), 0, 'inert while the pawn is unknown');

    recordCall(fake, caveOf(fake, 0));
    fake.poke(pawnCave + PAWN_SLOT, le64(PAWN));
    speed.tick({ on: true, mult: 2 });

    // the movement site works on pawns, not on the actor the Focus call names
    assert.equal(fake.mem.u64(cave + GUARD_SLOT), PAWN, 'guarded by the pawn');
    assert.equal(fake.mem.f32(cave + FACTOR_SLOT), 2, 'scaled 2x');

    speed.tick({ on: true, mult: 4 });
    assert.equal(fake.mem.f32(cave + FACTOR_SLOT), 4, 'switching level rewrites the factor');

    const spdOriginal = fake.original(SPD_SITE, 8);
    const pawnOriginal = fake.original(PAWN_SITE + 7, 5);
    speed.tick({ on: false, mult: 4 });
    assert.deepEqual(fake.read(SPD_SITE, 8), spdOriginal, 'movement site restored');
    assert.deepEqual(fake.read(PAWN_SITE + 7, 5), pawnOriginal, 'pawn site restored');
});

test('falls back to an absolute jump when no cave is within reach', async () => {
    // what a long-running game looks like: the allocator can only hand back a
    // block far outside E9 rel32 range
    const FAR = 0x7000_0000_0000;
    const fake = createMem({ aob: () => SITE, aobAll: () => [], alloc: () => FAR });
    install(fake);
    const cfg = await load();

    byName(cfg.options, 'Infinite Health').tick({ on: true, mult: 1 });
    assert.equal(fake.allocs.length, 1, 'a cave was taken');

    const patch = fake.read(SITE, 16);
    assert.deepEqual(patch.slice(0, 2), [0xff, 0x25], 'jmp [rip+0]');
    assert.deepEqual(patch.slice(6, 14), le64(FAR), 'straight to the far cave');
    assert.deepEqual(patch.slice(14), [0x90, 0x90], 'padded out to all 16 stolen bytes');

    // and the whole thing still works through the far cave
    attributesAt(fake, ATTRS_OBJ);
    recordCall(fake, FAR);
    byName(cfg.options, 'Infinite Health').tick({ on: true, mult: 1 });
    assert.equal(fake.mem.f32(slot(ATTRS_OBJ, HP)), 999999, 'health still pinned');

    byName(cfg.options, 'Infinite Health').tick({ on: false, mult: 1 });
    assert.deepEqual(fake.read(SITE, 16), fake.original(SITE, 16), 'all 16 bytes restored');
});

test('a far cave is refused when the caller named no extra bytes', async () => {
    // the cooldown sites can give up 14, the recorder 16 - but a caller that
    // names none must not get a jump it cannot fit
    const fake = createMem({ aob: () => 0, aobAll: () => [], alloc: () => 0x7000_0000_0000 });
    install(fake);
    const HOOK = await import('../../configs/lib/hook.js');
    assert.equal(HOOK.cave(BASE + 0x2000, 5, () => true), null, 'no farSteal, no far jump');
    assert.deepEqual(fake.read(BASE + 0x2000, 5), fake.original(BASE + 0x2000, 5), 'site untouched');
});
