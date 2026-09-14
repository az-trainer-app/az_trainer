// The Plague Tale config raises the game's own cheat flags. What matters is
// that each flag is found through its own compare, that switching an option
// off clears only what it set, and that the two never write to each other.

import { test } from 'node:test';
import assert from 'node:assert/strict';

import { BASE, createMem, install } from './fake-mem.mjs';

const RESONANCE = new URL('../../configs/games/resonance.js', import.meta.url).href;
let fresh = 0;
/** A new copy of the config, so per-option state does not leak between tests. */
const load = () => import(`${RESONANCE}?copy=${fresh++}`);

const i32 = (v) => {
    const n = v < 0 ? v + 0x100000000 : v;
    return [n & 0xff, (n >>> 8) & 0xff, (n >>> 16) & 0xff, (n >>> 24) & 0xff];
};

const GOD_SITE = BASE + 0x1000; // cmp byte [rip+god], 0
const INV_SITE = BASE + 0x2000; // cmp byte [rip+invisible], 0
const GOD = BASE + 0x3987_2f1; // the flags sit side by side in the exe's data
const INVISIBLE = GOD + 1;

/** A game whose two compares point at the two flags, both clear. */
function game() {
    const fake = createMem({
        // the invisible compare is the one followed by `mov rbx,rcx`
        aob: (sig) => (sig.includes('8B D9') ? INV_SITE : GOD_SITE),
    });
    // `cmp byte [rip+disp32], imm8` is 7 bytes with the disp32 at +2
    fake.poke(GOD_SITE + 2, i32(GOD - (GOD_SITE + 7)));
    fake.poke(INV_SITE + 2, i32(INVISIBLE - (INV_SITE + 7)));
    fake.poke(GOD, [0]);
    fake.poke(INVISIBLE, [0]);
    install(fake);
    return fake;
}

const byName = (options, name) => options.find((o) => o.name === name);
const byte = (fake, addr) => fake.mem.readBytes(addr, 1)[0];

test('finds the game and names it', async () => {
    game();
    const cfg = await load();
    assert.equal(cfg.process, 'Resonance.exe');
    assert.ok(cfg.live(), 'the flag checks are there to find');
});

test('each option raises its own flag and lowers it again', async () => {
    const fake = game();
    const cfg = await load();
    const health = byName(cfg.options, 'Infinite Health');
    const invisible = byName(cfg.options, 'Invisible');

    health.tick({ on: true, mult: 1 });
    assert.equal(byte(fake, GOD), 1, 'god mode on');
    assert.equal(byte(fake, INVISIBLE), 0, 'invisibility untouched');

    invisible.tick({ on: true, mult: 1 });
    assert.equal(byte(fake, INVISIBLE), 1, 'invisible on');

    health.tick({ on: false, mult: 1 });
    assert.equal(byte(fake, GOD), 0, 'god mode off');
    assert.equal(byte(fake, INVISIBLE), 1, 'invisibility still on');
});

test('a flag the game set itself is not cleared by an option that never set it', async () => {
    const fake = game();
    fake.poke(GOD, [1]); // raised through the game's own console
    const cfg = await load();

    byName(cfg.options, 'Infinite Health').tick({ on: false, mult: 1 });
    assert.equal(byte(fake, GOD), 1, 'left as the game had it');
});

test('a flag already set is not rewritten every tick', async () => {
    const fake = game();
    const cfg = await load();
    const health = byName(cfg.options, 'Infinite Health');

    health.tick({ on: true, mult: 1 });
    const before = fake.writes.length;
    health.tick({ on: true, mult: 1 });
    assert.equal(fake.writes.length, before, 'no write while already raised');
});
