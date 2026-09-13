// Dragon Age: The Veilguard
//
// Frostbite. Every site is located by signature scan and applied as a byte
// patch or a hook, so it survives builds that move the addresses.

import * as HOOK from '../lib/hook.js';

export const process = 'Dragon Age The Veilguard.exe';
export const title = 'Dragon Age: The Veilguard';

// The is-alive predicate:  xorps xmm0,xmm0 / comiss xmm0,[rcx+40] / setb al
// Health is at [rcx+0x40]. Every entity runs through it, so the hook checks
// the game's own player flag at [[rcx+8]+0xF8] to tell you from companions
// and monsters.
const ALIVE_SIG = '0F ?? ?? 0F ?? ?? ?? 0F ?? ?? C3 ?? ?? ?? ?? ?? 48 ?? ?? ?? 4C';
const ALIVE_STEAL = 7;        // xorps(3) + comiss(4)
const FLAG_PTR_OFF = 0x08;    // rcx -> owner object
const FLAG_OFF = 0xF8;        // nonzero on the player
const HEALTH_OFF = 0x40;      // current
const MAX_HEALTH_OFF = 0x44;  // max, immediately after it

// mulss xmm0, [rip+X] - 15 sites share this shape; the mana one is the single
// site whose RIP-relative operand holds -1.0f.
const MANA_SIG = 'F3 0F 59 05 ?? ?? ?? ?? 48 83 ?? ?? C3 CC';
// sub edx, [r8+rax*4+0xD00]   (the potion decrement)
const POTION_SIG = '41 2B ?? ?? ?? ?? 00 00 48 63 ?? ?? 89 ?? ?? ?? ?? 00 00 E9';
// comiss xmm6, xmm7  /  maxss xmm6, xmm7   (cooldown compare, then clamp)
const CD1_SIG = '0F 2F ?? 76 ?? 0F 28 ?? 48 89 ?? ?? ?? 48 8D';
const CD2_SIG = 'F3 0F 5F ?? 48 8D ?? ?? ?? 00 00 F3 0F 11 ?? ?? ?? 00 00 48 83';
// sub [rcx+1C8], r8d - the ability-point decrement. 7 bytes, NOPed out.
const POINTS_SIG = '44 29 81 C8 01 00 00';
const NOP7 = [0x90, 0x90, 0x90, 0x90, 0x90, 0x90, 0x90];

// mov [r9+4], r14d / mov [r9], edi - the XP store. r14d is the amount, so a
// detour that adds to it first grants a bonus on every XP gain.
const XP_SIG = '45 89 71 04 41 89 39';
const XP_BONUS = 5000;
const ADD_R14D = (n) => [0x41, 0x81, 0xC6, n & 0xff, (n >> 8) & 0xff,
                         (n >> 16) & 0xff, (n >> 24) & 0xff];

// mov [rdx+4], r15d / cmp r15d, r12d - the gold store. Setting r15d first
// makes every write land on the value we choose.
const GOLD_SIG = '44 89 7A 04 45 3B FC';
const GOLD_AMOUNT = 99999;
const MOV_R15D = (n) => [0x41, 0xBF, n & 0xff, (n >> 8) & 0xff,
                         (n >> 16) & 0xff, (n >> 24) & 0xff];

const JMP6 = [0xEB, 0x06];                       // jmp +6 - skip the instruction
const XORPS_XMM1 = [0x0F, 0x57, 0xC9];           // xorps xmm1, xmm1
const XORPS_XMM6_NOP = [0x0F, 0x57, 0xF6, 0x90]; // xorps xmm6, xmm6 ; nop

/** The mana site: the mulss whose RIP-relative operand is -1.0f. */
function manaSite() {
    const st = (globalThis.__vgMana ||= { at: 0, searched: false });
    if (st.searched) return st.at;
    st.searched = true;
    for (const a of mem.aobAll(MANA_SIG, 64)) {
        if (mem.f32(a + 8 + mem.i32(a + 4)) === -1.0) { st.at = a; break; }
    }
    if (!st.at) log('Veilguard: mana site not found among candidates');
    return st.at;
}

/** Cached single-match lookup - a 115MB scan must not run every tick. */
function siteOnce(key, sig) {
    const st = (globalThis.__vgSites ||= {});
    if (st[key] === undefined) st[key] = mem.aob(sig);
    return st[key];
}

/** Apply a patch while `on`, remove it when off. */
function toggle(key, on, findAddr, bytes) {
    const st = (globalThis.__vgPatches ||= {});
    if (on && !st[key]) {
        const addr = findAddr();
        if (!addr) return;
        const h = HOOK.patch(addr, bytes);
        if (!h) { log('Veilguard: failed to patch ' + key); return; }
        st[key] = h;
        log('Veilguard: ' + key + ' patched at 0x' + addr.toString(16));
    } else if (!on && st[key]) {
        st[key].restore();
        st[key] = null;
    }
}

/** Install a hook while `on`, remove it when off. `build` returns a handle. */
function hookToggle(key, on, build) {
    const st = (globalThis.__vgHooks ||= {});
    const s = (st[key] ||= { handle: null, failed: false });
    if (!on) {
        if (s.handle) { s.handle.restore(); s.handle = null; }
        s.failed = false;
        return;
    }
    if (s.handle || s.failed) return;
    s.handle = build();
    if (!s.handle) { s.failed = true; return; }
    log('Veilguard: ' + key + ' hooked');
}

/**
 * Ready once the game's code is mapped and our sites resolve.
 *
 * Frostbite exposes no root object to walk, so there is no cheap "are you in
 * world" test the way UE's pawn gives one. The signature lookup is cached, so
 * this costs nothing after the first hit.
 */
export function live() {
    return siteOnce('alive', ALIVE_SIG) !== 0;
}

export const options = [
    {
        name: 'Infinite Health',
        tick({ on }) {
            hookToggle('health', on, () => {
                const a = siteOnce('alive', ALIVE_SIG);
                if (!a) { log('Veilguard: alive-check not found'); return null; }
                return HOOK.holdFieldAtSibling(a, ALIVE_STEAL, FLAG_PTR_OFF,
                                               FLAG_OFF, HEALTH_OFF,
                                               MAX_HEALTH_OFF);
            });
        },
    },
    {
        name: 'Unlimited Mana',
        tick({ on }) {
            toggle('mana', on, manaSite, JMP6);
        },
    },
    {
        name: 'No Cooldowns',
        tick({ on }) {
            // two sites: skip the compare, then zero the clamp
            toggle('cd1', on, () => {
                const a = siteOnce('cd1', CD1_SIG);
                return a ? a + 5 : 0;   // the patch lands 5 bytes in
            }, XORPS_XMM1);
            toggle('cd2', on, () => siteOnce('cd2', CD2_SIG), XORPS_XMM6_NOP);
        },
    },
    {
        name: 'Unlimited Potions',
        tick({ on }) {
            toggle('potions', on, () => siteOnce('potions', POTION_SIG), JMP6);
        },
    },
    {
        name: 'Ability Points Never Decrease',
        tick({ on }) {
            toggle('points', on, () => siteOnce('points', POINTS_SIG), NOP7);
        },
    },
    {
        // Also feeds vendor strength, which scales with your level.
        name: 'Bonus XP (+5000)',
        tick({ on }) {
            hookToggle('xp', on, () => {
                const a = siteOnce('xp', XP_SIG);
                if (!a) { log('Veilguard: XP site not found'); return null; }
                return HOOK.detour(a, 7, ADD_R14D(XP_BONUS));
            });
        },
    },
    {
        // Forces the stored amount rather than topping up once, so spending
        // does not reduce it. Takes effect on the next gold write.
        name: 'Gold 99,999',
        tick({ on }) {
            hookToggle('gold', on, () => {
                const a = siteOnce('gold', GOLD_SIG);
                if (!a) { log('Veilguard: gold site not found'); return null; }
                return HOOK.detour(a, 7, MOV_R15D(GOLD_AMOUNT));
            });
        },
    },
];
