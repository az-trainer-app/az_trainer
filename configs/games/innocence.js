// A Plague Tale: Innocence
//
// Asobo's engine again, but an older build than Resonance and without that
// game's debug cheat flags - so invisibility here means keeping enemies'
// awareness of you from rising. Each AI stores its awareness at a fixed offset
// from three places: when it sights you, when it hears you, and while it
// closes in. The first two are made to store zero and the third skips its
// store entirely. Sites from the community cheat table for this game (its
// "senseless enemies" and "stealth mod" scripts).

import * as HOOK from '../lib/hook.js';
import * as OPT from '../lib/option.js';
import * as SCAN from '../lib/scan.js';

export const process = 'APlagueTaleInnocence_x64.exe';
export const title = 'A Plague Tale: Innocence';

// movss [rdi+140], xmm0 - the awareness store when the AI sights you...
const SIGHTED_SIG = 'F3 0F 11 ?? ?? ?? 00 00 48 8B ?? ?? ?? 00 00 8B ?? ?? 48 ?? ?? ?? 48';
// ...and when it hears you
const HEARD_SIG = 'F3 0F 11 ?? ?? ?? 00 00 F3 0F 10 ?? ?? ?? 00 00 41 0F ?? ?? 72';
// jbe over the store made while it closes in: a jmp skips it for good
const CLOSING_SIG = '76 ?? F3 0F 11 ?? ?? ?? 00 00 48 81 ?? ?? ?? 00 00 48 ?? ?? ?? 48 ?? ?? 0F 85';

const STORE_LEN = 8; // movss [rdi+disp32], xmm0
// Stolen along with the instruction after the store, when no cave is within a
// 5-byte jump - both whole, rdi-relative, so safe to run from the cave.
const SIGHTED_FAR = 15; // + mov rbx, [rdi+disp32]
const HEARD_FAR = 16; // + movss xmm7, [rdi+disp32]

/**
 * Zero xmm0 ahead of the store, so the awareness written is none - and xmm0
 * is left at zero too, for anything after that still reads it.
 *
 * @param {number} at the store instruction
 * @param {number} far bytes to take for a far jump
 */
function storeNothing(at, far) {
    return HOOK.detour(at, STORE_LEN, HOOK.xorps('xmm0', 'xmm0'), { free: false, farSteal: far });
}

/** Ready once the awareness stores are there to find. */
export function live() {
    return SCAN.once(SIGHTED_SIG) !== 0;
}

/** @type {import('../types/trainer').Entry[]} */
export const options = [
    {
        name: 'Invisible',
        keys: ['Alt+F1'],
        tick({ on }) {
            OPT.whileFound('sighted', on, () => SCAN.once(SIGHTED_SIG), (at) => storeNothing(at, SIGHTED_FAR));
            OPT.whileFound('heard', on, () => SCAN.once(HEARD_SIG), (at) => storeNothing(at, HEARD_FAR));
            OPT.patchWhileOn('closing', on, () => SCAN.once(CLOSING_SIG), [0xeb]);
        },
    },
];
