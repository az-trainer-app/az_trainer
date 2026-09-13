// Contract tests for every game config in configs/games.
//
// They run each script against the fake host in two worlds - one where no
// signature is found, one where every signature is - and check what a player
// depends on: nothing throws, and switching everything on and then off
// leaves the game's code exactly as it was.

import { test } from 'node:test';
import assert from 'node:assert/strict';
import { readdirSync } from 'node:fs';

import { BASE, SIZE, createMem, install } from './fake-mem.mjs';

const GAMES = new URL('../../configs/games/', import.meta.url);
const files = readdirSync(GAMES).filter((f) => f.endsWith('.js'));

let fresh = 0;
/** A new copy of the config, so one test's module state cannot leak into the next. */
const load = (file) => import(`${new URL(file, GAMES).href}?copy=${fresh++}`);

/** The same shape the Rust scanner accepts: two-character hex or `?` tokens. */
const SIGNATURE = /^[0-9A-Fa-f?]{2}( [0-9A-Fa-f?]{2})*$/;

/** Tick every option `rounds` times with the given switch. */
function tickAll(options, on, rounds = 2) {
    for (let r = 0; r < rounds; r++) {
        for (const o of options) {
            if (typeof o.tick !== 'function') continue; // a separator
            o.tick({ on, mult: on && o.levels?.length ? o.levels[0] : 1 });
        }
    }
}

/**
 * A host where every signature resolves, each to its own well-separated
 * address in the module. For `aobAll`, the candidate is arranged to pass the
 * common "RIP operand holds -1.0f" check.
 */
function worldWhereEverythingIsFound() {
    const assigned = new Map();
    const requested = [];
    const addrFor = (sig) => {
        requested.push(sig);
        if (!assigned.has(sig)) assigned.set(sig, BASE + 0x10000 * (assigned.size + 1));
        return assigned.get(sig);
    };
    let fake;
    fake = createMem({
        plausible: true,
        aob: addrFor,
        aobAll: (sig) => {
            const a = addrFor(sig);
            fake.poke(a + 4, [0x00, 0x01, 0x00, 0x00]); // rel32 = 0x100
            fake.poke(a + 8 + 0x100, [0x00, 0x00, 0x80, 0xbf]); // -1.0f
            return [a];
        },
    });
    return { fake, requested };
}

for (const file of files) {
    test(`${file}: exports what the engine loads`, async () => {
        install(createMem());
        const cfg = await load(file);

        assert.equal(typeof cfg.process, 'string');
        assert.match(cfg.process, /\.exe$/i, 'process is an executable name');
        assert.ok(typeof cfg.title === 'string' && cfg.title.length > 0, 'title');
        assert.ok(Array.isArray(cfg.options) && cfg.options.length > 0, 'options');
        if (cfg.live !== undefined) assert.equal(typeof cfg.live, 'function');

        const names = new Set();
        assert.ok(
            cfg.options.some((o) => typeof o.tick === 'function'),
            'at least one real option, not only separators',
        );
        for (const o of cfg.options) {
            if ('separator' in o) {
                assert.ok(
                    o.separator === true || (typeof o.separator === 'string' && o.separator !== ''),
                    'a separator is a heading or true',
                );
                assert.equal(o.tick, undefined, 'a separator has no tick');
                continue;
            }
            assert.ok(typeof o.name === 'string' && o.name.length > 0, 'option name');
            assert.ok(
                !names.has(o.name),
                `duplicate option "${o.name}" - settings are saved by name`,
            );
            names.add(o.name);
            assert.equal(typeof o.tick, 'function', `${o.name}: tick`);
            if (o.levels !== undefined) {
                assert.ok(o.levels.length > 0 && o.levels.every((l) => l > 0), `${o.name}: levels`);
            }
            if (o.labels !== undefined) {
                assert.equal(o.labels.length, o.levels?.length, `${o.name}: one label per level`);
            }
        }
    });

    test(`${file}: survives a game where nothing is found`, async () => {
        const fake = createMem();
        install(fake);
        const cfg = await load(file);

        tickAll(cfg.options, false);
        tickAll(cfg.options, true, 3);
        tickAll(cfg.options, false);
        if (cfg.live) assert.equal(typeof cfg.live(), 'boolean');

        const codeWrites = fake.writes.filter((w) => w.addr >= BASE && w.addr < BASE + SIZE);
        assert.deepEqual(codeWrites, [], 'nothing patched without a signature hit');
        assert.deepEqual(fake.allocs, [], 'no caves without a signature hit');
    });

    test(`${file}: everything on, then off, leaves the game's code untouched`, async () => {
        const { fake, requested } = worldWhereEverythingIsFound();
        const logs = install(fake);
        const cfg = await load(file);

        tickAll(cfg.options, true);
        // Without this, a script that silently does nothing would pass every check below.
        assert.ok(fake.writes.length > 0, 'switching options on changed something');
        tickAll(cfg.options, false);

        for (const sig of new Set(requested)) {
            assert.match(sig, SIGNATURE, `malformed signature: ${sig}`);
        }
        const problems = logs.filter((l) => /not found|failed/i.test(l));
        assert.deepEqual(problems, [], 'every site resolved and every patch applied');

        // Every byte written inside the module must be back to its original.
        const touched = new Set();
        for (const w of fake.writes) {
            if (w.addr < BASE || w.addr >= BASE + SIZE) continue; // game data, not code
            w.bytes.forEach((_, i) => touched.add(w.addr + i));
        }
        for (const a of touched) {
            assert.deepEqual(
                fake.read(a, 1),
                fake.original(a, 1),
                `code byte at 0x${a.toString(16)} not restored`,
            );
        }

        const freed = new Set(fake.frees);
        for (const { cave } of fake.allocs) {
            assert.ok(freed.has(cave), `cave 0x${cave.toString(16)} leaked`);
        }
        assert.deepEqual(fake.holds(), [], 'high-frequency writer stopped');
    });
}

test('there is at least one config to test', () => {
    assert.ok(files.length > 0);
});
