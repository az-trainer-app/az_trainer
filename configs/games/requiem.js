// A Plague Tale: Requiem
//
// Asobo's engine, between Innocence and Resonance - and like Resonance it
// still carries the game's own debug cheats as flag bytes. One of them makes
// Amicia undetectable, so nothing is hooked: the option sets that flag. Site
// from the community cheat table for this game.

import * as SCAN from '../lib/scan.js';

export const process = 'APlagueTaleRequiem_x64.exe';
export const title = 'A Plague Tale: Requiem';

// Amicia's update opens by testing the undetectable flag:
//   mov [rsp+18],rsi / mov [rsp+20],rdi / push r14 / sub rsp,20
//   cmp byte [rip+undetectable], 0
const AMICIA_SIG = '48 89 74 24 18 48 89 7C 24 20 41 56 48 83 EC 20 80 3D ?? ?? ?? ?? 00';
const UNDETECTABLE_CMP = 0x10; // `cmp byte [rip+disp32], 0`: 7 bytes, disp32 at +2

/** The undetectable flag's address, or 0. */
function undetectable() {
    const at = SCAN.once(AMICIA_SIG);
    return at ? mem.rip(at + UNDETECTABLE_CMP, 2, 7) : 0;
}

/** Ready once Amicia's update is there to find. */
export function live() {
    return SCAN.once(AMICIA_SIG) !== 0;
}

let setByUs = false;

/** @type {import('../types/trainer').Entry[]} */
export const options = [
    {
        name: 'Invisible',
        keys: ['Alt+F1'],
        tick({ on }) {
            const flag = undetectable();
            if (!flag) return;
            if (on) {
                if (mem.readBytes(flag, 1)[0] !== 1) mem.writeBytes(flag, [1]);
                setByUs = true;
            } else if (setByUs) {
                // only undo what we did: the game's console can set it too
                mem.writeBytes(flag, [0]);
                setByUs = false;
            }
        },
    },
];
