// The Innocence config keeps enemy awareness from rising at the three places
// the game stores it. What matters is that all three are changed together,
// in the right way each, and that switching off puts every byte back.

import { test } from 'node:test';
import assert from 'node:assert/strict';

import { BASE, createMem, install } from './fake-mem.mjs';

const INNOCENCE = new URL('../../configs/games/innocence.js', import.meta.url).href;
let fresh = 0;
const load = () => import(`${INNOCENCE}?copy=${fresh++}`);

const SIGHTED = BASE + 0x717a4;
const HEARD = BASE + 0x7174c;
const CLOSING = BASE + 0x718c9;

/** A game with the three awareness stores, each told apart by its tail. */
function game() {
    const fake = createMem({
        aob: (sig) => (sig.startsWith('76') ? CLOSING : sig.includes('41 0F') ? HEARD : SIGHTED),
    });
    install(fake);
    return fake;
}

const XORPS_XMM0 = [0x0f, 0x57, 0xc0];

test('names the game and finds it', async () => {
    game();
    const cfg = await load();
    assert.equal(cfg.process, 'APlagueTaleInnocence_x64.exe');
    assert.ok(cfg.live());
});

test('Invisible zeroes both stores, skips the third, and puts everything back', async () => {
    const fake = game();
    const cfg = await load();
    const invisible = cfg.options.find((o) => o.name === 'Invisible');
    const sightedBefore = fake.original(SIGHTED, 8);
    const heardBefore = fake.original(HEARD, 8);
    const closingBefore = fake.original(CLOSING, 1);

    invisible.tick({ on: true, mult: 1 });

    assert.equal(fake.allocs.length, 2, 'one cave per store that is zeroed');
    assert.equal(fake.read(SIGHTED, 1)[0], 0xe9, 'sighted store jumps to its cave');
    assert.equal(fake.read(HEARD, 1)[0], 0xe9, 'heard store jumps to its cave');
    for (const { cave } of fake.allocs) {
        assert.deepEqual(fake.read(cave, 3), XORPS_XMM0, 'each cave zeroes xmm0 first');
    }
    assert.deepEqual(fake.read(CLOSING, 1), [0xeb], 'closing store is jumped over');

    invisible.tick({ on: false, mult: 1 });

    assert.deepEqual(fake.read(SIGHTED, 8), sightedBefore, 'sighted restored');
    assert.deepEqual(fake.read(HEARD, 8), heardBefore, 'heard restored');
    assert.deepEqual(fake.read(CLOSING, 1), closingBefore, 'closing restored');
});
