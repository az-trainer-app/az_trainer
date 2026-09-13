// Dragon Age: The Veilguard
//
// Frostbite. Every site is located by signature scan and applied as a byte
// patch or a hook, so it survives builds that move the addresses.

import * as HOOK from '../lib/hook.js';
import * as OPT from '../lib/option.js';
import * as SCAN from '../lib/scan.js';

export const process = 'Dragon Age The Veilguard.exe';
export const title = 'Dragon Age: The Veilguard';

// The is-alive predicate:  xorps xmm0,xmm0 / comiss xmm0,[rcx+40] / setb al
// Health is at [rcx+0x40]. Every entity runs through it, so the hook checks
// the game's own player flag at [[rcx+8]+0xF8] to tell you from companions
// and monsters.
const ALIVE_SIG = '0F ?? ?? 0F ?? ?? ?? 0F ?? ?? C3 ?? ?? ?? ?? ?? 48 ?? ?? ?? 4C';
const ALIVE_STEAL = 7; // xorps(3) + comiss(4)
const FLAG_PTR_OFF = 0x08; // rcx -> owner object
const FLAG_OFF = 0xf8; // nonzero on the player
const HEALTH_OFF = 0x40; // current
const MAX_HEALTH_OFF = 0x44; // max, immediately after it

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

// mov [r9+4], r14d / mov [r9], edi - the XP store. r14d is the amount, so a
// detour that adds to it first grants a bonus on every XP gain.
const XP_SIG = '45 89 71 04 41 89 39';
const XP_BONUS = 5000;

// mov [rdx+4], r15d / cmp r15d, r12d - the gold store. Setting r15d first
// makes every write land on the value we choose.
const GOLD_SIG = '44 89 7A 04 45 3B FC';
const GOLD_AMOUNT = 99999;

/** jmp +6 over an 8-byte instruction: skips it entirely. */
const SKIP_8 = HOOK.jmpShort(6);

/**
 * Ready once the game's code is mapped and our sites resolve.
 *
 * Frostbite exposes no root object to walk, so there is no cheap "are you in
 * world" test the way UE's pawn gives one. The scan is cached, so this costs
 * nothing after the first hit.
 */
export function live() {
    return SCAN.once(ALIVE_SIG) !== 0;
}

/** @type {import('../types/trainer').Entry[]} */
export const options = [
    { separator: 'Combat' },
    {
        name: 'Infinite Health',
        tick({ on }) {
            OPT.whileOn('health', on, () => {
                const at = SCAN.once(ALIVE_SIG);
                return at
                    ? HOOK.holdFieldAtSibling(
                          at,
                          ALIVE_STEAL,
                          FLAG_PTR_OFF,
                          FLAG_OFF,
                          HEALTH_OFF,
                          MAX_HEALTH_OFF,
                      )
                    : null;
            });
        },
    },
    {
        name: 'Unlimited Mana',
        tick({ on }) {
            const site = () => SCAN.onceWhere(MANA_SIG, (a) => SCAN.ripF32(a, 4, 8) === -1.0);
            OPT.patchWhileOn('mana', on, site, SKIP_8);
        },
    },
    {
        name: 'No Cooldowns',
        tick({ on }) {
            // two sites: skip the compare, then zero the clamp
            OPT.patchWhileOn(
                'cd1',
                on,
                () => {
                    const at = SCAN.once(CD1_SIG);
                    return at && at + 5; // the patch lands 5 bytes in
                },
                HOOK.xorps('xmm1', 'xmm1'),
            );
            OPT.patchWhileOn('cd2', on, () => SCAN.once(CD2_SIG), [
                ...HOOK.xorps('xmm6', 'xmm6'),
                ...HOOK.nops(1),
            ]);
        },
    },
    {
        name: 'Unlimited Potions',
        tick({ on }) {
            OPT.patchWhileOn('potions', on, () => SCAN.once(POTION_SIG), SKIP_8);
        },
    },
    { separator: 'Progression' },
    {
        name: 'Ability Points Never Decrease',
        tick({ on }) {
            OPT.patchWhileOn('points', on, () => SCAN.once(POINTS_SIG), HOOK.nops(7));
        },
    },
    {
        // Also feeds vendor strength, which scales with your level.
        name: 'Bonus XP (+5000)',
        tick({ on }) {
            OPT.whileOn('xp', on, () => {
                const at = SCAN.once(XP_SIG);
                return at ? HOOK.detour(at, 7, HOOK.addR32Imm('r14d', XP_BONUS)) : null;
            });
        },
    },
    {
        // Forces the stored amount rather than topping up once, so spending
        // does not reduce it. Takes effect on the next gold write.
        name: 'Gold 99,999',
        tick({ on }) {
            OPT.whileOn('gold', on, () => {
                const at = SCAN.once(GOLD_SIG);
                return at ? HOOK.detour(at, 7, HOOK.movR32Imm('r15d', GOLD_AMOUNT)) : null;
            });
        },
    },
];
