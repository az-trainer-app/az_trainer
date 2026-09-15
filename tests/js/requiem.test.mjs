// The Requiem config raises the game's own undetectable flag. What matters is
// that the flag is found through the compare that tests it, and that
// switching off clears only what the option set.

import { test } from 'node:test';
import assert from 'node:assert/strict';

import { BASE, createMem, install } from './fake-mem.mjs';

const REQUIEM = new URL('../../configs/games/requiem.js', import.meta.url).href;
let fresh = 0;
/** A new copy of the config, so per-option state does not leak between tests. */
const load = () => import(`${REQUIEM}?copy=${fresh++}`);

const i32 = (v) => {
    const n = v < 0 ? v + 0x100000000 : v;
    return [n & 0xff, (n >>> 8) & 0xff, (n >>> 16) & 0xff, (n >>> 24) & 0xff];
};

const AMICIA_SITE = BASE + 0x1000;
const UNDETECTABLE = BASE + 0x1bc87a2; // in the exe's data, as in the live game

/** A game whose compare points at the flag, clear. */
function game() {
    const fake = createMem({ aob: () => AMICIA_SITE });
    // the compare sits 0x10 in; `cmp byte [rip+disp32], 0` has the disp32 at +2
    const cmp = AMICIA_SITE + 0x10;
    fake.poke(cmp + 2, i32(UNDETECTABLE - (cmp + 7)));
    fake.poke(UNDETECTABLE, [0]);
    install(fake);
    return fake;
}

const byte = (fake, at) => fake.read(at, 1)[0];

test('names the game and finds it', async () => {
    game();
    const cfg = await load();
    assert.equal(cfg.process, 'APlagueTaleRequiem_x64.exe');
    assert.ok(cfg.live());
});

test('Invisible raises the flag and clears it again', async () => {
    const fake = game();
    const cfg = await load();
    const invisible = cfg.options.find((o) => o.name === 'Invisible');

    invisible.tick({ on: true, mult: 1 });
    assert.equal(byte(fake, UNDETECTABLE), 1, 'on');
    invisible.tick({ on: false, mult: 1 });
    assert.equal(byte(fake, UNDETECTABLE), 0, 'off');
});

test('switching off leaves alone a flag the game set by itself', async () => {
    const fake = game();
    const cfg = await load();
    fake.poke(UNDETECTABLE, [1]); // set from the game's console
    cfg.options[0].tick({ on: false, mult: 1 });
    assert.equal(byte(fake, UNDETECTABLE), 1);
});
