// The Blood of Dawnwalker - UE5, build dw1-pc-258504-shipping-patch2

import * as HOOK from '../lib/hook.js';
import * as OPT from '../lib/option.js';
import * as SCAN from '../lib/scan.js';
import * as UE from '../lib/unreal.js';

export const process = 'Dawnwalker.exe';
export const title = 'The Blood of Dawnwalker';

// Pawn sub-objects
const HUMAN_SET = 0x940; // UHumanAttributeSet
const VAMP_SET = 0xcd8; // UVampireAttributeSet

// Attribute offsets within a set
const HEALTH = 0x40;
const MAX_HEALTH = 0x50;
const STAMINA = 0x80;
const MAX_STAMINA = 0x90;
const BLOOD = 0x30;
const BLOOD_SEGMENTS = 0x90;
const BLOOD_PER_SEGMENT = 0xa0;

// AActor::CustomTimeDilation - the field the game's Haste mechanic drives.
const TIME_DILATION = 0x68;

/** The multiplier Speed is applying, so other options can see past it. */
let speedMult = 1;

// AWorldSettings::GetEffectiveTimeDilation, as this build has it. UWorld::Tick
// multiplies every frame's delta by the result.
//   movss xmm0, [rcx+4F0]    the game's own factor - finishers lower it
//   movss xmm1, [rcx+3F4]    MatineeTimeDilation
//   mulss xmm0, [rcx+3F8]    DemoPlayTimeDilation
//   mulss xmm1, [rcx+3F0]    TimeDilation
//   mulss xmm0, xmm1 / ret
const EFFECTIVE_DILATION_SIG =
    'F3 0F 10 81 ?? ?? 00 00 F3 0F 10 89 ?? ?? 00 00 F3 0F 59 81 ?? ?? 00 00 F3 0F 59 89 ?? ?? 00 00 F3 0F 59 C1 C3';

/**
 * The same function without the game's factor: Matinee x DemoPlay x
 * TimeDilation, so the engine's own time dilation keeps working. The field
 * offsets are copied from the site rather than assumed.
 * @param {number} site
 */
function withoutGameSlowmo(site) {
    const b = mem.readBytes(site, 32);
    // prettier-ignore
    return [
        0xf3, 0x0f, 0x10, 0x81, ...b.slice(12, 16),  // movss xmm0, [rcx+Matinee]
        0xf3, 0x0f, 0x59, 0x81, ...b.slice(20, 24),  // mulss xmm0, [rcx+DemoPlay]
        0xf3, 0x0f, 0x59, 0x81, ...b.slice(28, 32),  // mulss xmm0, [rcx+TimeDilation]
        0xc3,                                        // ret
    ];
}

// No Hit Reaction, ported from DawnwalkerTrainer 1.1.0. Two edits, both needed:
//
// 1. A `test rax,rax / je rel32` on the hit path becomes `nop / jmp rel32`.
const HIT_BRANCH_SIG =
    'E8 ?? ?? ?? ?? ?? 8D ?? 24 ?? ?? ?? ?? E8 ?? ?? ?? ?? 48 85 C0 0F 84 ?? ?? 00 00 ?? 63 ?? 08';
const HIT_BRANCH_OFF = 21;

// 2. A per-actor check, entered with the actor in rbx:
//   +0   mov rcx,[rbx+10]      \ 7 stolen bytes
//   +4   mov rdx,rax           /
//   +7   call <check>          skipped for the player, as if it returned 0
//   +12  test al,al / je ...
const HIT_ACTOR_SIG =
    '48 8B 4B 10 48 8B D0 E8 ?? ?? ?? ?? 84 C0 0F 84 ?? ?? 00 00 ?? 8D ?? ?? ?? ?? 8B ?? E8 ?? ?? ?? ?? ?? 8B ?? E8';
const HIT_ACTOR_STEAL = 7;
const HIT_ACTOR_SKIP = 12;

/**
 * Detour the actor check so it is skipped only when rbx is the pawn in the
 * guard slot. The guard is rewritten every tick, since the pawn moves on load.
 * @param {number} target
 */
function noHitActorHook(target) {
    const original = mem.readBytes(target, HIT_ACTOR_STEAL);
    if (!original || original.length !== HIT_ACTOR_STEAL) return null;
    const cave = mem.alloc(0x1000, target);
    if (!cave) return null;
    const rel = cave - (target + 5);
    if (rel > 0x7ffffff0 || rel < -0x7ffffff0) {
        mem.free(cave);
        return null;
    }
    const slotPawn = cave + 0x40;

    // prettier-ignore
    const code = [
        0x48, 0x3b, 0x1d, ...HOOK.i32(slotPawn - (cave + 7)), // cmp rbx,[slotPawn]
        0x75, 0x10,                                          // jne normal
        0x31, 0xc0,                                          // xor eax,eax
        ...HOOK.jmpAbs(target + HIT_ACTOR_SKIP),             // past the call
        ...original,                                         // normal:
        ...HOOK.jmpAbs(target + HIT_ACTOR_STEAL),
    ];
    mem.writeBytes(slotPawn, HOOK.u64(0));
    mem.writeBytes(cave, code);
    if (!mem.writeBytes(target, [0xe9, ...HOOK.i32(rel), ...HOOK.nops(HIT_ACTOR_STEAL - 5)])) {
        mem.free(cave);
        return null;
    }
    return {
        setGuard(/** @type {number} */ addr) {
            mem.writeBytes(slotPawn, HOOK.u64(addr || 0));
        },
        restore() {
            mem.writeBytes(target, original);
            mem.free(cave);
        },
    };
}

/** @type {ReturnType<typeof noHitActorHook>} */
let noHitActor = null;

// Denarius is an inventory item (internal id "Coin"), not a standalone counter.
//   PlayerController -> +0x368 -> +0x3C0 -> +0x2F8 -> +0x570 -> +0x318 -> +0x88C
const MONEY_CHAIN = [0x368, 0x3c0, 0x2f8, 0x570, 0x318];
const MONEY_OFF = 0x88c;

function humanSet() {
    const p = UE.pawn();
    return p ? UE.comp(p, HUMAN_SET) : 0;
}

/** In-world once the pawn resolves. */
export function live() {
    return UE.pawn() !== 0;
}

/** @type {import('../types/trainer').Entry[]} */
export const options = [
    { separator: 'Survival' },
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
            return UE.fmt('Blood', UE.holdProduct(s, BLOOD, BLOOD_SEGMENTS, BLOOD_PER_SEGMENT, on));
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
        name: 'No Hit Reaction',
        tick({ on }) {
            const applied = OPT.whileOn('nohit', on, () => {
                const branch = SCAN.once(HIT_BRANCH_SIG);
                const actor = SCAN.once(HIT_ACTOR_SIG);
                if (!branch || !actor) return null;
                const jmp = HOOK.patch(branch + HIT_BRANCH_OFF, [0x90, 0xe9]);
                if (!jmp) return null;
                noHitActor = noHitActorHook(actor);
                if (!noHitActor) {
                    jmp.restore();
                    return null;
                }
                return {
                    restore() {
                        noHitActor?.restore();
                        noHitActor = null;
                        jmp.restore();
                    },
                };
            });
            if (applied) noHitActor?.setGuard(UE.pawn());
        },
    },
    { separator: true },
    {
        name: 'No Slow-Motion Finishers',
        tick({ on }) {
            // Haste uses the same factor: it slows the world and raises the
            // pawn's CustomTimeDilation by the reciprocal. With the factor
            // gone the player would run several times too fast, so the
            // original code is put back while Haste is active. Finishers
            // never touch the pawn's dilation.
            const p = UE.pawn();
            const hasted = p ? mem.f32(p + TIME_DILATION) / speedMult > 1.05 : false;
            OPT.whileOn('slowmo', on && !hasted, () => {
                const at = SCAN.once(EFFECTIVE_DILATION_SIG);
                return at ? HOOK.patch(at, withoutGameSlowmo(at)) : null;
            });
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
            speedMult = on && p ? mult : 1;
            if (!on || !p) {
                mem.clearHolds(); // the game restores it on its next frame
                return;
            }
            mem.holdScale(p + TIME_DILATION, mult);
            return 'Speed  ' + mult + 'x';
        },
    },
    {
        // Reads your live balance; clicking an amount writes it once, so you
        // can still spend normally afterwards (it is not locked).
        name: 'Denarius',
        levels: [1000, 10000, 100000],
        labels: ['1k', '10k', '100k'],
        tick({ on, mult }) {
            const pc = UE.playerController();
            const obj = pc ? UE.chain(pc, MONEY_CHAIN) : 0;
            if (!obj) return;
            const addr = obj + MONEY_OFF;
            OPT.writeOnce('denarius', on, mult, (v) => mem.writeBytes(addr, HOOK.i32(v)));
            return 'Denarius  ' + mem.i32(addr);
        },
    },
];
