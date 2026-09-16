// The 007 First Light config hooks three sites taken from the community cheat
// table. What matters is that each cave is built and entered correctly, that
// god mode is guarded by the player's id - found through the engine's own
// interface table - and that switching off puts every byte back.

import { test } from 'node:test';
import assert from 'node:assert/strict';

import { BASE, createMem, install } from './fake-mem.mjs';

const FIRSTLIGHT = new URL('../../configs/games/firstlight.js', import.meta.url).href;
let fresh = 0;
const load = () => import(`${FIRSTLIGHT}?copy=${fresh++}`);

const le = (v, n) => {
    const out = [];
    for (let i = 0; i < n; i++) {
        out.push(v % 256);
        v = Math.floor(v / 256);
    }
    return out;
};

const PLAYER_SITE = BASE + 0x1000;
const HUMANOID_SITE = BASE + 0x2000;
const HIT_SITE = BASE + 0x3000;
const INSTINCT_SITE = BASE + 0x4000;
const QLENS_SITE = BASE + 0x5000;

const VFTABLE = BASE + 0x100000;
const ROUTINE = BASE + 0x110000;
const PLAYER_GLOBAL = BASE + 0x120000;
const TYPE_ID = BASE + 0x130000;
const YOU_GOT_HIT = BASE + 0x140000;

const HEAP = 0x2000_0000_0000;
const HOLDER = HEAP + 0x1000;
const ENTITY = HEAP + 0x2000;
const ENTITY_TYPE = HEAP + 0x3000;
const INTERFACES = HEAP + 0x4000;
const PLAYER_ID = 117506110;

/** A game with every site in place and a player whose character sits 8 bytes before its entity. */
function game() {
    const fake = createMem({
        aob: (sig) =>
            sig.startsWith('48 8D 05')
                ? PLAYER_SITE
                : sig.startsWith('48 69 D1')
                  ? HUMANOID_SITE
                  : sig.startsWith('C5 FA 11')
                    ? HIT_SITE
                    : sig.startsWith('48 8B 80')
                      ? INSTINCT_SITE
                      : QLENS_SITE,
    });
    const rip = (at, operand, len, target) => fake.poke(at + operand, le(target - (at + len), 4));
    // lea rax,[vftable]; slot 5 is the routine: mov rax,[rip+player]
    rip(PLAYER_SITE, 3, 7, VFTABLE);
    fake.poke(VFTABLE + 0x28, le(ROUTINE, 8));
    fake.poke(ROUTINE, [0x48, 0x8b, 0x05]);
    rip(ROUTINE, 3, 7, PLAYER_GLOBAL);
    // lea rdx,[TTypeIDHelper<ZHumanoidCharacterEntity>::id] at +0xA
    rip(HUMANOID_SITE + 0xa, 3, 7, TYPE_ID);
    // call ZActor::YouGotHit at +6
    rip(HIT_SITE + 6, 1, 5, YOU_GOT_HIT);
    fake.poke(YOU_GOT_HIT, [0x48, 0x89, 0x54, 0x24, 0x10]); // mov [rsp+10],rdx

    // the player: global -> holder, entity at +0x18, interface list on its type
    fake.poke(PLAYER_GLOBAL, le(HOLDER, 8));
    fake.poke(HOLDER + 0x18, le(ENTITY, 8));
    fake.poke(ENTITY, le(ENTITY_TYPE, 8));
    fake.poke(ENTITY_TYPE + 0x20, le(INTERFACES, 8));
    const entries = HEAP + 0x5000;
    fake.poke(INTERFACES, le(entries, 8));
    fake.poke(INTERFACES + 8, le(entries + 32, 8));
    fake.poke(entries, le(BASE + 0x777, 8)); // some other interface
    fake.poke(entries + 8, le(0xa0, 8));
    fake.poke(entries + 16, le(TYPE_ID, 8));
    fake.poke(entries + 24, [0xf8, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff]); // -8
    fake.poke(ENTITY - 8 + 0x200, le(PLAYER_ID, 4));
    install(fake);
    return fake;
}

const byName = (options, name) => options.find((o) => o.name === name);

test('names the game and finds it', async () => {
    game();
    const cfg = await load();
    assert.equal(cfg.process, '007FirstLight.exe');
    assert.ok(cfg.live());
    assert.deepEqual(
        cfg.options.map((o) => o.name),
        ['Infinite Health', 'Infinite Instinct', 'Infinite Q-Lens Resources'],
    );
});

test('Infinite Health hooks YouGotHit, guards it with the player id, and restores it', async () => {
    const fake = game();
    const cfg = await load();
    const health = byName(cfg.options, 'Infinite Health');
    const original = fake.read(YOU_GOT_HIT, 5);

    health.tick({ on: true, mult: 1 });
    assert.equal(fake.read(YOU_GOT_HIT, 1)[0], 0xe9, 'entered through a jump');
    const cave = fake.allocs[0].cave;
    assert.equal(fake.mem.i32(cave + 0x100), PLAYER_ID, "the player's id, found through the interface table");
    assert.deepEqual(fake.read(cave + 29, 5), original, 'the stolen instruction runs after the guard');

    health.tick({ on: false, mult: 1 });
    assert.deepEqual(fake.read(YOU_GOT_HIT, 5), original, 'restored');
});

test('Instinct and Q-Lens caves copy maximum over current before the original instruction', async () => {
    const fake = game();
    const cfg = await load();
    fake.poke(INSTINCT_SITE + 7, [0xc4, 0xc1, 0x78, 0x2f, 0x44, 0x04, 0x04]);
    fake.poke(QLENS_SITE, [0xc5, 0xfa, 0x10, 0x51, 0x08]);
    const instinctOriginal = fake.read(INSTINCT_SITE + 7, 7);
    const qlensOriginal = fake.read(QLENS_SITE, 5);

    byName(cfg.options, 'Infinite Instinct').tick({ on: true, mult: 1 });
    byName(cfg.options, 'Infinite Q-Lens Resources').tick({ on: true, mult: 1 });
    const [instinct, qlens] = fake.allocs.map((a) => a.cave);
    assert.deepEqual(fake.read(instinct + 7, 13), [0xc4, 0xc1, 0x7a, 0x10, 0x0c, 0x04, 0xc4, 0xc1, 0x7a, 0x11, 0x4c, 0x04, 0x04]);
    assert.deepEqual(fake.read(instinct + 20, 7), instinctOriginal);
    assert.deepEqual(fake.read(qlens + 7, 10), [0xc5, 0xfa, 0x10, 0x51, 0x18, 0xc5, 0xfa, 0x11, 0x51, 0x08]);
    assert.deepEqual(fake.read(qlens + 17, 5), qlensOriginal);

    byName(cfg.options, 'Infinite Instinct').tick({ on: false, mult: 1 });
    byName(cfg.options, 'Infinite Q-Lens Resources').tick({ on: false, mult: 1 });
    assert.deepEqual(fake.read(INSTINCT_SITE + 7, 7), instinctOriginal, 'instinct site restored');
    assert.deepEqual(fake.read(QLENS_SITE, 5), qlensOriginal, 'q-lens site restored');
});
