// scan.js, option.js and the instruction encoders in hook.js.

import { test } from 'node:test';
import assert from 'node:assert/strict';

import { BASE, createMem, install } from './fake-mem.mjs';

const lib = (name) => import(new URL(`../../configs/lib/${name}`, import.meta.url).href);
const HOOK = await lib('hook.js');
const SCAN = await lib('scan.js');
const OPT = await lib('option.js');

// ---- encoders -------------------------------------------------------------

test('encoders reproduce the bytes the Veilguard script was verified with', () => {
    assert.deepEqual(HOOK.movR32Imm('r15d', 99999), [0x41, 0xbf, ...HOOK.i32(99999)]);
    assert.deepEqual(HOOK.addR32Imm('r14d', 5000), [0x41, 0x81, 0xc6, ...HOOK.i32(5000)]);
    assert.deepEqual(HOOK.xorps('xmm1', 'xmm1'), [0x0f, 0x57, 0xc9]);
    assert.deepEqual(HOOK.xorps('xmm6', 'xmm6'), [0x0f, 0x57, 0xf6]);
    assert.deepEqual(HOOK.jmpShort(6), [0xeb, 0x06]);
    assert.deepEqual(HOOK.nops(7), [0x90, 0x90, 0x90, 0x90, 0x90, 0x90, 0x90]);
});

test('encoders: low registers need no REX prefix, high ones do', () => {
    assert.deepEqual(HOOK.movR32Imm('eax', 1), [0xb8, 1, 0, 0, 0]);
    assert.deepEqual(HOOK.movR32Imm('r8d', 1), [0x41, 0xb8, 1, 0, 0, 0]);
    assert.deepEqual(HOOK.addR32Imm('ecx', 2), [0x81, 0xc1, 2, 0, 0, 0]);
    assert.deepEqual(HOOK.xorps('xmm9', 'xmm2'), [0x44, 0x0f, 0x57, 0xca]);
    assert.deepEqual(HOOK.xorps('xmm0', 'xmm12'), [0x41, 0x0f, 0x57, 0xc4]);
    assert.deepEqual(HOOK.jmpShort(-2), [0xeb, 0xfe]);
});

test('encoders reject what they cannot encode', () => {
    assert.throws(() => HOOK.movR32Imm(/** @type {any} */ ('rax'), 1), /unknown register/);
    assert.throws(() => HOOK.jmpShort(200), /out of range/);
});

// ---- scan.js --------------------------------------------------------------

test('once: scans each signature once per process', () => {
    let scans = 0;
    const fake = createMem({ aob: () => (scans++, BASE + 0x100) });
    install(fake);
    assert.equal(SCAN.once('48 8B 05'), BASE + 0x100);
    assert.equal(SCAN.once('48 8B 05'), BASE + 0x100);
    assert.equal(scans, 1);

    install(createMem({ aob: () => (scans++, BASE + 0x200) }));
    assert.equal(SCAN.once('48 8B 05'), BASE + 0x200, 'a new process scans again');
    assert.equal(scans, 2);
});

test('once: a miss is remembered and logged once', () => {
    let scans = 0;
    const logs = install(createMem({ aob: () => (scans++, 0) }));
    assert.equal(SCAN.once('90 90'), 0);
    assert.equal(SCAN.once('90 90'), 0);
    assert.equal(scans, 1);
    assert.equal(logs.length, 1);
});

test('onceWhere: picks the candidate whose RIP operand holds the value', () => {
    const a = BASE + 0x1000;
    const b = BASE + 0x2000;
    const fake = createMem({ aobAll: () => [a, b] });
    install(fake);
    for (const site of [a, b]) fake.poke(site + 4, [0x00, 0x01, 0x00, 0x00]); // rel32 = 0x100
    fake.poke(a + 8 + 0x100, [0x00, 0x00, 0x80, 0x3f]); // 1.0f
    fake.poke(b + 8 + 0x100, [0x00, 0x00, 0x80, 0xbf]); // -1.0f

    const isNegOne = (site) => SCAN.ripF32(site, 4, 8) === -1.0;
    assert.equal(SCAN.onceWhere('F3 0F 59 05', isNegOne), b);
});

// ---- option.js ------------------------------------------------------------

/** A handle that counts restores. */
const handle = () => {
    const h = { restores: 0, restore: () => h.restores++ };
    return h;
};

test('whileOn: builds once while on, restores when off', () => {
    install(createMem());
    const h = handle();
    let builds = 0;
    const build = () => (builds++, h);

    assert.equal(OPT.whileOn('k', true, build), true);
    assert.equal(OPT.whileOn('k', true, build), true);
    assert.equal(builds, 1, 'not rebuilt every tick');
    assert.equal(OPT.whileOn('k', false, build), false);
    assert.equal(h.restores, 1);
    assert.equal(OPT.whileOn('k', false, build), false);
    assert.equal(h.restores, 1, 'restored exactly once');
});

test('whileOn: a failure is not retried until switched off and on', () => {
    install(createMem());
    let builds = 0;
    const failing = () => (builds++, null);
    OPT.whileOn('k', true, failing);
    OPT.whileOn('k', true, failing);
    assert.equal(builds, 1);
    OPT.whileOn('k', false, failing);
    OPT.whileOn('k', true, failing);
    assert.equal(builds, 2);
});

test('whileFound: builds at the found address, and not at all when missing', () => {
    install(createMem());
    const seen = [];
    const build = (at) => (seen.push(at), handle());
    assert.equal(OPT.whileFound('found', true, () => BASE + 0x10, build), true);
    assert.equal(OPT.whileFound('missing', true, () => 0, build), false);
    assert.deepEqual(seen, [BASE + 0x10], 'build never ran for the missing site');
});

test('patchWhileOn: patches while on and puts the bytes back', () => {
    const fake = createMem();
    install(fake);
    const at = BASE + 0x3000;
    OPT.patchWhileOn('p', true, () => at, [0xeb, 0x06]);
    assert.deepEqual(fake.read(at, 2), [0xeb, 0x06]);
    OPT.patchWhileOn('p', false, () => at, [0xeb, 0x06]);
    assert.deepEqual(fake.read(at, 2), fake.original(at, 2));
});

test('patchWhileOn: nothing to patch is a failure, not a crash', () => {
    const fake = createMem();
    install(fake);
    assert.equal(
        OPT.patchWhileOn('p', true, () => 0, [0x90]),
        false,
    );
    assert.deepEqual(fake.writes, []);
});
