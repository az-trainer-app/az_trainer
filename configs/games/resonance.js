// Resonance: A Plague Tale Legacy
//
// Asobo's own engine, not Unreal: there is no GWorld to walk and no attribute
// enum to index, so lib/unreal.js is no help here.
//
// What the game does have is its own debug cheats, kept as flag bytes one
// after the other - god mode, then invisibility - and tested at the top of
// the damage path:
//   cmp byte [rip+invisible], 0 / mov rbx,rcx / jne ...
//   cmp byte [rip+god], 0       / jne ...
//   lea rdi, [rcx+1C0] / jmp ...
// So neither option needs a hook: each sets the game's own flag. Credit for
// finding them goes to Paul44's cheat table for this game.

import * as SCAN from '../lib/scan.js';

export const process = 'Resonance.exe';
export const title = 'Resonance: A Plague Tale Legacy';

// Each signature lands on the compare that tests its flag, so the flag is that
// instruction's operand. Resolving both independently, rather than taking the
// second flag as the first plus one, means a build that reorders them still
// gets each one right. The `EB` matters: the same lea recurs right after that
// jmp, followed by 48, which is where a looser god-mode signature would land.
const GOD_SIG = '80 3D ?? ?? ?? ?? 00 75 ?? 48 8D B9 C0 01 00 00 EB';
const INVISIBLE_SIG = '80 3D ?? ?? ?? ?? 00 48 8B D9 75 ?? 80 3D';

/**
 * An option that raises one of the game's own cheat flags.
 *
 * Only undoes what it did: the game's console can set the same flag, and
 * switching the option off should not clear a flag it never set.
 *
 * @param {string} name
 * @param {string} sig lands on `cmp byte [rip+flag], 0`
 * @param {string} key global shortcut that toggles it
 */
function cheatFlag(name, sig, key) {
    let setByUs = false;
    return {
        name,
        keys: [key],
        tick({ on }) {
            const at = SCAN.once(sig);
            const flag = at ? mem.rip(at, 2, 7) : 0;
            if (!flag) return;
            if (on) {
                if (mem.readBytes(flag, 1)[0] !== 1) mem.writeBytes(flag, [1]);
                setByUs = true;
            } else if (setByUs) {
                mem.writeBytes(flag, [0]);
                setByUs = false;
            }
        },
    };
}

/** Ready once the damage path's flag checks are there to find. */
export function live() {
    return SCAN.once(GOD_SIG) !== 0;
}

/** @type {import('../types/trainer').Entry[]} */
export const options = [
    // the game's own god mode: nothing is subtracted in the first place
    cheatFlag('Infinite Health', GOD_SIG, 'Alt+F1'),
    cheatFlag('Invisible', INVISIBLE_SIG, 'Alt+F2'),
];
