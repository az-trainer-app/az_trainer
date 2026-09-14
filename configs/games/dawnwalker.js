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

// --- No Slow-Motion Finishers ------------------------------------------------

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
    return [
        0xf3, 0x0f, 0x10, 0x81, ...b.slice(12, 16),  // movss xmm0, [rcx+Matinee]
        0xf3, 0x0f, 0x59, 0x81, ...b.slice(20, 24),  // mulss xmm0, [rcx+DemoPlay]
        0xf3, 0x0f, 0x59, 0x81, ...b.slice(28, 32),  // mulss xmm0, [rcx+TimeDilation]
        0xc3,                                        // ret
    ];
}

// --- No Hit Reaction -----------------------------------------------------------
// Two edits, both needed:
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
const HIT_SLOT_PAWN = 0x40;

/**
 * Detour the actor check so it is skipped only when rbx is the pawn in the
 * guard slot. The guard is rewritten every tick, since the pawn moves on load.
 * @param {number} target
 */
function noHitActorHook(target) {
    const h = HOOK.cave(
        target,
        HIT_ACTOR_STEAL,
        (cave, original) =>
            mem.writeBytes(cave + HIT_SLOT_PAWN, HOOK.u64(0)) &&
            mem.writeBytes(cave, [
                0x48, 0x3b, 0x1d, ...HOOK.i32(HIT_SLOT_PAWN - 7), // cmp rbx,[pawn slot]
                0x75, 0x10,                                      // jne normal
                0x31, 0xc0,                                      // xor eax,eax
                ...HOOK.jmpAbs(target + HIT_ACTOR_SKIP),         // past the call
                ...original,                                     // normal:
                ...HOOK.jmpAbs(target + HIT_ACTOR_STEAL),
            ]),
    );
    return (
        h && {
            ...h,
            /** @param {number} addr */
            setGuard: (addr) => mem.writeBytes(h.cave + HIT_SLOT_PAWN, HOOK.u64(addr || 0)),
        }
    );
}

/** @type {ReturnType<typeof noHitActorHook>} */
let noHitActor = null;

// --- Denarius --------------------------------------------------------------------

// Denarius goes through the game's own currency functions, so whatever the
// game does on a balance change still happens. Both take the inventory
// component and a currency type, 0 being Denarius.
const GET_CURRENCY_SIG = '40 53 48 83 EC 20 48 8B D9 44 8A C2 48 8B 89 28 04 00 00';
const ADD_CURRENCY_SIG = '48 89 5C 24 10 57 48 83 EC 30 41 8B D8 48 8B F9 48 8B 89 28 04 00 00';

// An inventory function entered with the component in rcx. The signature
// matches 5 bytes in, after its `mov [rsp+10],rbx` prologue.
const INVENTORY_SIG =
    '55 48 8B EC 48 81 EC 80 00 00 00 48 8B 05 ?? ?? ?? ?? 48 33 C4 48 89 45 F0 48 8B D9 E8 ?? ?? ?? ?? 84 C0 74 ?? E8 ?? ?? ?? ?? 83 65 A4 00';
const INVENTORY_BACK = 5;

// Cave data slots
const SLOT_THREAD = 0xe0; // dword  game thread id; any other thread passes straight through
const SLOT_PLAYER = 0xe8; // qword  owner to match against [rcx+20]; 0 = disarmed
const SLOT_MONEY = 0xf0; // dword  requested balance; the cave zeroes it when done
const SLOT_GET = 0x100;
const SLOT_ADD = 0x108;
const SLOT_RETURN = 0x110;

// Changing currency fires UI notifications that build widgets. Doing that
// off the game thread, or into a world that is loading, corrupts UObjects and
// crashes a later load (UObjectHash.cpp "hash itself may be corrupted"). So
// the cave only acts on the game thread, for an armed player, once.
//
//   mov  eax,gs:[48] / cmp eax,[Thread] / jne original
//   mov  rax,[Player] / test rax,rax / je original
//   cmp  [rcx+20],rax / jne original
//   cmp  dword [Money],0 / jle original
//   save rbx rcx rdx rsi rdi r8-r11, xmm0-5 (rsp ends aligned)
//   esi = [Money]; [Money] = 0
//   eax = GetCurrency(inv, 0)
//   AddCurrency(inv, 0, esi - eax)
//   restore
//   original: mov [rsp+10],rbx / jmp [Return]
const INVENTORY_CAVE = [
    0x65, 0x8b, 0x04, 0x25, 0x48, 0x00, 0x00, 0x00, 0x3b, 0x05, 0xd2, 0x00, 0x00, 0x00, 0x0f, 0x85,
    0xbe, 0x00, 0x00, 0x00, 0x48, 0x8b, 0x05, 0xcd, 0x00, 0x00, 0x00, 0x48, 0x85, 0xc0, 0x0f, 0x84,
    0xae, 0x00, 0x00, 0x00, 0x48, 0x39, 0x41, 0x20, 0x0f, 0x85, 0xa4, 0x00, 0x00, 0x00, 0x83, 0x3d,
    0xbb, 0x00, 0x00, 0x00, 0x00, 0x0f, 0x8e, 0x97, 0x00, 0x00, 0x00, 0x53, 0x51, 0x52, 0x56, 0x57,
    0x41, 0x50, 0x41, 0x51, 0x41, 0x52, 0x41, 0x53, 0x48, 0x81, 0xec, 0x80, 0x00, 0x00, 0x00, 0x0f,
    0x11, 0x44, 0x24, 0x20, 0x0f, 0x11, 0x4c, 0x24, 0x30, 0x0f, 0x11, 0x54, 0x24, 0x40, 0x0f, 0x11,
    0x5c, 0x24, 0x50, 0x0f, 0x11, 0x64, 0x24, 0x60, 0x0f, 0x11, 0x6c, 0x24, 0x70, 0x31, 0xff, 0x8b,
    0x35, 0x7b, 0x00, 0x00, 0x00, 0xc7, 0x05, 0x71, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x48,
    0x89, 0xcb, 0x89, 0xfa, 0x48, 0x8b, 0x05, 0x75, 0x00, 0x00, 0x00, 0xff, 0xd0, 0x29, 0xc6, 0x41,
    0x89, 0xf0, 0x89, 0xfa, 0x48, 0x89, 0xd9, 0x48, 0x8b, 0x05, 0x6a, 0x00, 0x00, 0x00, 0xff, 0xd0,
    0x0f, 0x10, 0x6c, 0x24, 0x70, 0x0f, 0x10, 0x64, 0x24, 0x60, 0x0f, 0x10, 0x5c, 0x24, 0x50, 0x0f,
    0x10, 0x54, 0x24, 0x40, 0x0f, 0x10, 0x4c, 0x24, 0x30, 0x0f, 0x10, 0x44, 0x24, 0x20, 0x48, 0x81,
    0xc4, 0x80, 0x00, 0x00, 0x00, 0x41, 0x5b, 0x41, 0x5a, 0x41, 0x59, 0x41, 0x58, 0x5f, 0x5e, 0x5a,
    0x59, 0x5b, 0x48, 0x89, 0x5c, 0x24, 0x10, 0xff, 0x25, 0x33, 0x00, 0x00, 0x00,
];

// How long a Denarius click waits for the game to run the inventory function
// (open the inventory, pick something up) before giving up.
const DENARIUS_TIMEOUT_MS = 10000;

/**
 * Hook the inventory function with the currency cave.
 *
 * The cave is never freed: the game thread may be inside it (in AddCurrency)
 * when the hook comes out, and returning into a freed page would crash. One
 * page per use is a fair price.
 */
function currencyHook() {
    const hit = SCAN.once(INVENTORY_SIG);
    const get = SCAN.once(GET_CURRENCY_SIG);
    const add = SCAN.once(ADD_CURRENCY_SIG);
    const thread = mem.mainThreadId();
    if (!hit || !get || !add || !thread) return null;
    const target = hit - INVENTORY_BACK;

    const h = HOOK.cave(
        target,
        INVENTORY_BACK,
        (cave) =>
            mem.writeBytes(cave, INVENTORY_CAVE) &&
            mem.writeBytes(cave + SLOT_THREAD, HOOK.i32(thread)) &&
            mem.writeBytes(cave + SLOT_PLAYER, HOOK.u64(0)) && // disarmed
            mem.writeBytes(cave + SLOT_MONEY, HOOK.i32(0)) &&
            mem.writeBytes(cave + SLOT_GET, HOOK.u64(get)) &&
            mem.writeBytes(cave + SLOT_ADD, HOOK.u64(add)) &&
            mem.writeBytes(cave + SLOT_RETURN, HOOK.u64(target + INVENTORY_BACK)),
        { free: false },
    );
    if (!h) return null;

    /** @type {(off: number, bytes: number[]) => boolean} */
    const write = (off, bytes) => mem.writeBytes(h.cave + off, bytes);
    const disarm = () => {
        write(SLOT_MONEY, HOOK.i32(0));
        write(SLOT_PLAYER, HOOK.u64(0));
    };
    return {
        /**
         * Player first: with the amount still 0, a stale player fires nothing.
         * @param {number} player
         * @param {number} amount
         */
        arm: (player, amount) => write(SLOT_PLAYER, HOOK.u64(player)) && write(SLOT_MONEY, HOOK.i32(amount)),
        disarm,
        pending: () => mem.i32(h.cave + SLOT_MONEY) !== 0,
        restore() {
            disarm();
            h.restore();
        },
    };
}

/** @type {ReturnType<typeof currencyHook>} */
let currency = null;
let currencyArmed = false;
let currencyDeadline = 0;

// --- helpers ---------------------------------------------------------------------

/** The player's pawn, once it is safe to write to. */
function pawn() {
    return UE.settled('pawn', UE.pawn());
}

/**
 * A sub-object of the pawn, once it is safe to write to.
 * @param {string} key
 * @param {number} off
 */
function pawnPart(key, off) {
    const p = pawn();
    return p ? UE.settled(key, UE.comp(p, off)) : 0;
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
            const s = pawnPart('human', HUMAN_SET);
            return s ? UE.fmt('Health', UE.hold(s, HEALTH, MAX_HEALTH, on)) : undefined;
        },
    },
    {
        name: 'Infinite Vampiric Blood',
        tick({ on }) {
            // max is segments x per-segment, so it tracks upgrades automatically
            const s = pawnPart('vampire', VAMP_SET);
            return s ? UE.fmt('Blood', UE.holdProduct(s, BLOOD, BLOOD_SEGMENTS, BLOOD_PER_SEGMENT, on)) : undefined;
        },
    },
    {
        name: 'Infinite Stamina',
        tick({ on }) {
            const s = pawnPart('human', HUMAN_SET);
            return s ? UE.fmt('Stamina', UE.hold(s, STAMINA, MAX_STAMINA, on)) : undefined;
        },
    },
    {
        name: 'No Hit Reaction',
        tick({ on }) {
            const applied = OPT.whileOn('nohit', on, () => {
                const branch = SCAN.once(HIT_BRANCH_SIG);
                const actor = SCAN.once(HIT_ACTOR_SIG);
                const jmp = branch && actor ? HOOK.patch(branch + HIT_BRANCH_OFF, [0x90, 0xe9]) : null;
                const hook = jmp && noHitActorHook(actor);
                if (!jmp || !hook) {
                    jmp?.restore();
                    return null;
                }
                noHitActor = hook;
                return {
                    restore() {
                        hook.restore();
                        jmp.restore();
                        noHitActor = null;
                    },
                };
            });
            // the settled pawn, not the raw one: a load frees the old pawn and
            // hands its address to something else, and the cave would then be
            // comparing against whatever moved in
            if (applied) noHitActor?.setGuard(pawn());
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
            //
            // It also comes out while the world is loading. The factor this
            // drops is the game's own, and a load is exactly when the game
            // leans on it to hold the world still - so leaving the patch in
            // lets everything keep ticking through a level load instead of
            // waiting for it. The settled pawn is 0 until the world is up.
            const loaded = pawn();
            const p = UE.pawn();
            const hasted = p ? mem.f32(p + TIME_DILATION) / speedMult > 1.05 : false;
            OPT.whileFound(
                'slowmo',
                on && !!loaded && !hasted,
                () => SCAN.once(EFFECTIVE_DILATION_SIG),
                (at) => HOOK.patch(at, withoutGameSlowmo(at)),
            );
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
        keys: ['Alt+F1', 'Alt+F2', 'Alt+F3'],
        tick({ on, mult }) {
            const p = pawn();
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
        // Sets your Denarius once per click, then switches itself off - always
        // within DENARIUS_TIMEOUT_MS, whether or not it landed, so nothing
        // stays armed into a save load. The cave does the work the next time
        // the game thread runs the inventory function (open the inventory, or
        // pick something up), by calling AddCurrency with the difference.
        name: 'Denarius',
        // the game caps Denarius at 99,999
        levels: [1000, 10000, 99999],
        labels: ['1k', '10k', '99k'],
        once: true,
        tick({ on, mult }) {
            const applied = OPT.whileOn('denarius', on, () => (currency = currencyHook()));
            if (!on) {
                currency = null;
                currencyArmed = false;
                currencyDeadline = 0;
                return;
            }
            if (!applied || !currency) return true; // could not hook this build - give up

            const now = Date.now();
            if (!currencyDeadline) currencyDeadline = now + DENARIUS_TIMEOUT_MS;
            const p = pawn(); // 0 while loading, or until the pawn has settled

            if (!currencyArmed) {
                currencyArmed = !!p && currency.arm(p, mult);
                if (currencyArmed || now <= currencyDeadline) return false;
                log('Denarius: not in the world - nothing changed');
                return true;
            }

            // Every way out disarms, so nothing stays armed into a load.
            const hook = currency;
            /** @param {string} [why] */
            const done = (why) => {
                hook.disarm();
                if (why) log(`Denarius: ${why}`);
                return true;
            };
            if (!hook.pending()) return done(); // applied
            if (!p) return done('cancelled by a load'); // never fire into the next world
            if (now > currencyDeadline) return done('timed out - open the inventory right after clicking');
            return false;
        },
    },
];
