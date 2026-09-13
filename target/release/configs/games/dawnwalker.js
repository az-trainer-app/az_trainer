// The Blood of Dawnwalker - UE5, build dw1-pc-258504-shipping-patch2

import * as UE from '../lib/unreal.js';
import * as HOOK from '../lib/hook.js';

export const process = 'Dawnwalker.exe';
export const title = 'The Blood of Dawnwalker';

// Pawn sub-objects
const HUMAN_SET = 0x940; // UHumanAttributeSet
const VAMP_SET = 0xCD8;  // UVampireAttributeSet

// Attribute offsets within a set
const HEALTH = 0x40, MAX_HEALTH = 0x50;
const STAMINA = 0x80, MAX_STAMINA = 0x90;
const BLOOD = 0x30, BLOOD_SEGMENTS = 0x90, BLOOD_PER_SEGMENT = 0xA0;

// AActor::CustomTimeDilation - the field the game's Haste mechanic drives.
const TIME_DILATION = 0x68;

// Denarius is an inventory item (internal id "Coin"), not a standalone counter.
//   PlayerController -> +0x368 -> +0x3C0 -> +0x2F8 -> +0x570 -> +0x318 -> +0x88C
const MONEY_CHAIN = [0x368, 0x3C0, 0x2F8, 0x570, 0x318];
const MONEY_OFF = 0x88C;

function humanSet() {
    const p = UE.pawn();
    return p ? UE.comp(p, HUMAN_SET) : 0;
}

/** In-world once the pawn resolves. */
export function live() {
    return UE.pawn() !== 0;
}

export const options = [
    {
        name: 'Infinite Human Health',
        tick({ on }) {
            const s = humanSet();
            if (!s) return;
            return UE.fmt('Health', UE.hold(s, HEALTH, MAX_HEALTH, on));
        },
    },
    {
        name: 'Infinite Vampiric Blood',
        tick({ on }) {
            const p = UE.pawn();
            const s = p ? UE.comp(p, VAMP_SET) : 0;
            if (!s) return;
            // max is segments x per-segment, so it tracks upgrades automatically
            return UE.fmt(
                'Blood',
                UE.holdProduct(s, BLOOD, BLOOD_SEGMENTS, BLOOD_PER_SEGMENT, on)
            );
        },
    },
    {
        name: 'Infinite Stamina',
        tick({ on }) {
            const s = humanSet();
            if (!s) return;
            return UE.fmt('Stamina', UE.hold(s, STAMINA, MAX_STAMINA, on));
        },
    },
    {
        // Scales CustomTimeDilation, the field Haste (human form, Shift twice)
        // raises to 3.25. Scaling rather than setting keeps every tier
        // proportional: at 2x, walk and sprint become 2.0 and Haste 6.5.
        //
        // The game rewrites the field every frame, so mem.holdScale() hands it
        // to a ~1kHz writer thread; a 10Hz write only flickers.
        name: 'Speed',
        levels: [2, 4, 8],
        tick({ on, mult }) {
            const p = UE.pawn();
            if (!on || !p) {
                mem.clearHolds();      // the game restores it on its next frame
                return;
            }
            mem.holdScale(p + TIME_DILATION, mult);
            return 'Speed  ' + mult + 'x';
        },
    },
    {
        // Reads your live balance; clicking an amount writes it ONCE so you
        // can still spend normally afterwards (it is not locked).
        name: 'Denarius',
        levels: [1000, 10000, 100000],
        labels: ['1k', '10k', '100k'],
        tick({ on, mult }) {
            const pc = UE.playerController();
            const obj = pc ? UE.chain(pc, MONEY_CHAIN) : 0;
            if (!obj) return;
            const addr = obj + MONEY_OFF;

            const st = (globalThis.__money ||= { applied: null });
            if (!on) {
                st.applied = null;                 // re-arm for the next click
                return 'Denarius  ' + mem.i32(addr);
            }
            if (st.applied !== mult) {
                mem.writeBytes(addr, HOOK.i32(mult));
                st.applied = mult;
            }
            return 'Denarius  ' + mem.i32(addr);
        },
    },
];
