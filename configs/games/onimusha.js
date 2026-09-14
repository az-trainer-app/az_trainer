// Onimusha: Way of the Sword - RE Engine, Steam build 24769601 (game 01.000.001)
//
// Data only: no code is patched. RE Engine's anti-tamper corrupts the game if
// any instruction is modified (only REFramework neutralises it), so instead the
// player and the red-soul wallet are located through static pointer paths
// captured from the running game, and values are written directly - which the
// protection does not watch.

import * as HOOK from '../lib/hook.js';

export const process = 'OnimushaWotS.exe';
export const title = 'Onimusha: Way of the Sword';

// Static pointer -> player character. Captured from the running game and
// confirmed to still resolve after a save reload. Several are kept so that one
// going stale is tolerated: the player is the character the most paths agree on
// and whose health and stamina both resolve.
const PLAYER_PATHS = [
    { rva: 0xcbba6d8, offsets: [0x3a0, 0x0] },
    { rva: 0xcbd8dd0, offsets: [0x3e0, 0x0] },
    { rva: 0xcbba6d8, offsets: [0x220, 0x50, 0x0] },
    { rva: 0xcb9bdd0, offsets: [0x208, 0x80, 0x0] },
    { rva: 0xcb9bdd8, offsets: [0x208, 0x80, 0x0] },
    { rva: 0xcbbf560, offsets: [0x250, 0x50, 0x0] },
    { rva: 0xcbbf568, offsets: [0x250, 0x50, 0x0] },
    { rva: 0xcbd8dd0, offsets: [0x260, 0x50, 0x0] },
];

// From the character to each stat object (int max, then current 4 bytes later).
const HEALTH_CHAIN = [0xf8, 0x50, 0x28, 0x70];
const STAMINA_CHAIN = [0x1f0, 0x70, 0x68];
const ONI_CHAIN = [0xf8, 0x58]; // shares the +F8 component holder with health
const HEALTH_MAX = 0x24;
const HEALTH_CUR = 0x2c;
const STAMINA_MAX = 0x34;
const STAMINA_CUR = 0x38;
const ONI_MAX = 0x128;
const ONI_CUR = 0x124;

/** Resolve a static path: `[[exe+rva]+o0]+o1...`, dereferencing all but the last. */
function resolve(path) {
    let p = mem.u64(mem.moduleBase() + path.rva);
    const o = path.offsets;
    for (let i = 0; i < o.length - 1; i++) {
        if (!p) return 0;
        p = mem.u64(p + o[i]);
    }
    return p ? p + o[o.length - 1] : 0;
}

/** Follow a chain of dereference offsets from `base`. */
function chain(base, offsets) {
    let p = base;
    for (const off of offsets) {
        if (!p) return 0;
        p = mem.u64(p + off);
    }
    return p;
}

/** The stat object at `offsets` from `c`, if its max/current look real. */
function statObject(c, offsets, maxOff, curOff) {
    const obj = chain(c, offsets);
    if (!obj) return 0;
    const max = mem.i32(obj + maxOff);
    const cur = mem.i32(obj + curOff);
    return max >= 10 && max <= 100000 && cur >= 0 && cur <= max ? obj : 0;
}

/**
 * The player character, chosen by majority vote among the paths that resolve to
 * a character with both a valid health and stamina object. The dual-chain check
 * means a stale pointer is ignored rather than acted on.
 */
function findPlayer() {
    const votes = new Map();
    for (const path of PLAYER_PATHS) {
        const c = resolve(path);
        if (c) votes.set(c, (votes.get(c) || 0) + 1);
    }
    let best = 0;
    let bestVotes = 0;
    for (const [c, n] of votes) {
        if (n > bestVotes && statObject(c, HEALTH_CHAIN, HEALTH_MAX, HEALTH_CUR) && statObject(c, STAMINA_CHAIN, STAMINA_MAX, STAMINA_CUR)) {
            best = c;
            bestVotes = n;
        }
    }
    return best;
}

// Resolving walks several pointer chains, so cache the character for the span of
// one tick pass (the engine calls every option within a few ms).
let cache = { at: 0, c: 0 };
function character() {
    const now = Date.now();
    if (now - cache.at > 50) cache = { at: now, c: findPlayer() };
    return cache.c;
}

/** Hold `obj`'s current value at its maximum. */
function holdAtMax(obj, maxOff, curOff) {
    const max = mem.i32(obj + maxOff);
    if (mem.i32(obj + curOff) < max) mem.writeBytes(obj + curOff, HOOK.i32(max));
}

/** In-world once the player resolves. */
export function live() {
    return character() !== 0;
}

/** @type {import('../types/trainer').Entry[]} */
export const options = [
    { separator: 'Survival' },
    {
        name: 'Infinite Health',
        tick({ on }) {
            const c = on ? character() : 0;
            const s = c && statObject(c, HEALTH_CHAIN, HEALTH_MAX, HEALTH_CUR);
            if (s) holdAtMax(s, HEALTH_MAX, HEALTH_CUR);
        },
    },
    {
        name: 'Infinite Stamina',
        tick({ on }) {
            const c = on ? character() : 0;
            const s = c && statObject(c, STAMINA_CHAIN, STAMINA_MAX, STAMINA_CUR);
            if (s) holdAtMax(s, STAMINA_MAX, STAMINA_CUR);
        },
    },
    { separator: 'Combat' },
    {
        name: 'Max Oni Power',
        tick({ on }) {
            const c = on ? character() : 0;
            const s = c && statObject(c, ONI_CHAIN, ONI_MAX, ONI_CUR);
            if (s) holdAtMax(s, ONI_MAX, ONI_CUR);
        },
    },
];
