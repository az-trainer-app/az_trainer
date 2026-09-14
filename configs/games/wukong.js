// Black Myth: Wukong - Unreal Engine 5.0
//
// Every stat the player has lives in one float array: the game reads a stat
// through GetAttributeFloat(attrs, index), which is `lea rax,[rax+rcx*4+20]`
// - floats at `attrs + 0x20` indexed by the GSE-ProtobufDB.EBGUAttrFloat
// enum. Current and maximum are two entries of that same array, so holding a
// value is a plain write.
//
// Finding the player is the only part that needs code. lib/unreal.js walks
// GWorld, but its signature comes from a much newer UE5 and does not match
// this 5.0 build. Instead one hook records the actor the game is fetching the
// player's Focus for - that routine only ever runs for the player - and the
// attribute object is found underneath it.

import * as HOOK from '../lib/hook.js';
import * as OPT from '../lib/option.js';
import * as SCAN from '../lib/scan.js';

export const process = 'b1-Win64-Shipping.exe';
export const title = 'Black Myth: Wukong';

/** Where the float array starts inside the attribute object. */
const ATTRS = 0x20;

// EBGUAttrFloat indices. The maxima sit low in the enum and the live values
// high, which is why the two halves look unrelated.
const HP_MAX = 1;
const MP_MAX = 2;
const STAMINA_MAX = 8;
const VESSEL_MAX = 16; // FabaoEnergyMax
const SPIRIT_MAX = 17; // VigorEnergyMax
const FOCUS_MAX = 39; // PevalueMax
const HP_MAX_BASE = 101;
const STAMINA_MAX_BASE = 108;
const HP = 151;
const MP = 152;
const ATK = 153;
const DEF = 154;
const STAMINA = 158;
const STAMINA_RECOVER = 159;
const FOCUS = 191; // Pevalue
const VESSEL = 201; // FabaoEnergy
const SPIRIT = 202; // VigorEnergy
// How fast the transformation gauge refills. The game's own trainer makes
// transform instant by feeding this back as 10000 from inside the attribute
// read; writing the same number into the array does it without any code.
const ENERGY_INCREASE_SPEED = 14;
const ENERGY_FLOOD = 10000;

// The player's Focus fetch:
//   call [rax-0x40]          \ 8 stolen bytes
//   movss [rbp-0x2c], xmm0   /
// entered with the player's actor in rsi. Only the player has Focus, so the
// actor seen here is never a companion or an enemy.
const ACTOR_SIG =
    'FF 50 ?? F3 0F 11 45 ?? F3 0F 10 ?? ?? 0F 57 ?? ?? 0F 2F ?? 0F 82 ?? ?? 00 00 E9 ?? ?? ?? ?? 48 8B 86 ?? ?? 00 00';
const ACTOR_STEAL = 8;
const ACTOR_FAR_STEAL = 16; // four whole instructions, for a far jump
const ACTOR_OFF = 0x10; // rsi -> the actor
/** Where in the cave the captured pointers are parked. */
const SET_SLOT = 0x100; // rcx: the attribute object the call is about to read
const ACTOR_SLOT = 0x108; // [rsi+0x10]: the actor it belongs to

/**
 * Record what the game is about to read the player's Focus from.
 *
 * This is a call to GetAttributeFloat(attrs, index): by the x64 convention
 * rcx is the attribute object and edx the index - which is exactly what the
 * game's own trainer reads there. So the object can be taken straight off the
 * register rather than searched for.
 *
 * The cave only copies pointers; it changes nothing the game does. rax is
 * saved and restored around the copies because the stolen `call` goes through
 * it, and rcx/rdx are only read, never touched. The cave is kept allocated
 * after restore, since the game thread may still be inside it.
 *
 * @param {number} target
 */
function actorRecorder(target) {
    return HOOK.cave(
        target,
        ACTOR_STEAL,
        (at, original) =>
            mem.writeBytes(at + SET_SLOT, HOOK.u64(0)) &&
            mem.writeBytes(at + ACTOR_SLOT, HOOK.u64(0)) &&
            mem.writeBytes(at, [
                0x50, // push rax
                0x48, 0x89, 0xc8, // mov rax, rcx
                0x48, 0x89, 0x05, ...HOOK.i32(SET_SLOT - 11), // mov [rip+set], rax
                0x48, 0x8b, 0x46, ACTOR_OFF, // mov rax, [rsi+0x10]
                0x48, 0x89, 0x05, ...HOOK.i32(ACTOR_SLOT - 22), // mov [rip+actor], rax
                0x58, // pop rax
                ...original,
                ...HOOK.jmpAbs(target + original.length),
            ]),
        // If no cave is free within jump range, 16 bytes can go instead of 8:
        //   call [rax-40] / movss [rbp-2c],xmm0 / movss xmm0,[rbp-2c] / xorps xmm1,xmm1
        // which is four whole instructions, none of them position-dependent.
        { free: false, farSteal: ACTOR_FAR_STEAL },
    );
}

// A spell cooldown ticking down:
//   cmp dword [reg],0
//   call ...
//   movss xmm1,[rbp+d]   \ 9 stolen bytes, hooked 8 in
//   subss xmm0,xmm1      /
// Several sites share this shape and only one of them is the player's spell
// bar, so the cave decides at runtime rather than the signature deciding for
// us: it acts only on a value worth shortening and only for the player's
// actor, which leaves it inert everywhere else.
const CD_SIG = '83 ?? 00 E8 ?? ?? ?? ?? F3 0F 10 4D ?? F3 0F 5C C1';
const CD_HOOK_OFF = 8; // past the cmp and the call, onto the movss
const CD_STEAL = 9; // movss(5) + subss(4)
const CD_SITES = 12;
const GUARD_SLOT = 0x80; // the player's spell-bar component (rsi), once the script has vouched for it
const F_FLOOR_SLOT = 0x88; // 2.0  - below this there is nothing worth cutting
const F_LEFT_SLOT = 0x8c; // 0.1  - what a shortened cooldown becomes
const SEEN_SLOT = 0x90; // the last component (rsi) seen with a cooldown worth cutting

/**
 * Cut the player's cooldown short.
 *
 * Like the game's own trainer, short timers are left alone and only the
 * player's are touched - the component in rsi whose actor, at [rsi+0x10], is
 * the player. But the cave never reads [rsi+0x10] itself: on the sites that
 * are not the spell bar rsi is not a pointer at all, and reading through it
 * crashed the game as soon as enemies used their skills. The cave only parks
 * rsi and compares it; the script checks the parked value from outside the
 * game, where a bad pointer just reads as 0, and approves it into the guard.
 * While the guard is zero nothing matches.
 *
 * @param {number} target
 */
function cooldownHook(target) {
    return HOOK.cave(
        target,
        CD_STEAL,
        (at, original) =>
            mem.writeBytes(at + GUARD_SLOT, HOOK.u64(0)) &&
            mem.writeBytes(at + SEEN_SLOT, HOOK.u64(0)) &&
            mem.writeF32(at + F_FLOOR_SLOT, 2.0) &&
            mem.writeF32(at + F_LEFT_SLOT, 0.1) &&
            mem.writeBytes(at, [
                ...original, //                                    @0,  ends 9
                0x0f, 0x2f, 0x05, ...HOOK.i32(F_FLOOR_SLOT - 16), // comiss xmm0,[floor] @9, ends 16
                0x76, 0x18, //                                     jbe done            @16, ends 18
                0x48, 0x89, 0x35, ...HOOK.i32(SEEN_SLOT - 25), //  mov [rip+seen],rsi  @18, ends 25
                0x48, 0x3b, 0x35, ...HOOK.i32(GUARD_SLOT - 32), // cmp rsi,[rip+guard] @25, ends 32
                0x75, 0x08, //                                     jne done            @32, ends 34
                0xf3, 0x0f, 0x10, 0x05, ...HOOK.i32(F_LEFT_SLOT - 42), // movss xmm0,[left] @34, ends 42
                // done:                                                               @42
                ...HOOK.jmpAbs(target + original.length),
            ]),
        { free: false },
    );
}

/**
 * The component a cooldown cave should act on: what it last saw, if that
 * belongs to the player; else what it already had, while that still does.
 * Reads go through the host, so a parked value that is not a pointer is
 * simply not the player's.
 *
 * @param {number} cave
 * @param {number} who the player's actor, or 0
 */
function vouch(cave, who) {
    const owns = (component) => component !== 0 && who !== 0 && mem.u64(component + ACTOR_OFF) === who;
    const seen = mem.u64(cave + SEEN_SLOT);
    const approved = mem.u64(cave + GUARD_SLOT);
    return owns(seen) ? seen : owns(approved) ? approved : 0;
}

/** @type {ReturnType<typeof cooldownHook>[]} */
let cooldownHooks = [];

// The movement speed a character is about to move at:
//   movss xmm1,[rbx+disp32]    8 stolen bytes, hooked at the instruction
// Multiplying xmm1 here scales the speed. Entered with the character in rbx,
// or with rbx owning it at [rbx+0x20] - the game's own trainer accepts either.
const SPEED_SIG =
    'F3 0F 10 8B ?? ?? 00 00 EB ?? F3 0F 10 ?? ?? ?? ?? ?? 48 8B ?? ?? ?? 8B ?? F3 0F 59 ?? ?? ?? ?? 48 8B ?? FF 90';
const SPEED_STEAL = 8;
// Two counters, readable through the devtools channel from the cave address
// published below. They separate the two ways a hook like this goes wrong: a
// signature that matched bytes which never execute leaves `hits` at zero,
// while a guard comparing against the wrong object leaves `scaled` at zero
// with `hits` climbing. Both happened while this was being written.
const SPD_HITS_SLOT = 0x90; // times the cave has run
const SPD_SCALED_SLOT = 0x98; // times the guard matched and the speed was scaled
const SPD_GUARD_SLOT = 0x80; // the player's pawn, rewritten every tick
const SPD_FACTOR_SLOT = 0x88; // what to multiply the speed by

/**
 * Scale the player's movement speed.
 *
 * A signature matching bytes that are never executed installs perfectly and
 * then does nothing, which is easy to mistake for a wrong guard - so the cave
 * counts its own runs. While the guard is zero nothing matches and the hook
 * is inert.
 *
 * @param {number} target
 */
function speedHook(target) {
    return HOOK.cave(
        target,
        SPEED_STEAL,
        (at, original) =>
            mem.writeBytes(at + SPD_GUARD_SLOT, HOOK.u64(0)) &&
            mem.writeBytes(at + SPD_HITS_SLOT, HOOK.u64(0)) &&
            mem.writeBytes(at + SPD_SCALED_SLOT, HOOK.u64(0)) &&
            mem.writeF32(at + SPD_FACTOR_SLOT, 1.0) &&
            mem.writeBytes(at, [
                ...original, //                                              @0,  ends 8
                0x48, 0xff, 0x05, ...HOOK.i32(SPD_HITS_SLOT - 15), //        inc qword [rip+hits]   ends 15
                0x50, //                                                     push rax
                0x48, 0x8b, 0x05, ...HOOK.i32(SPD_GUARD_SLOT - 23), //       mov rax,[rip+guard]    ends 23
                0x48, 0x39, 0xd8, //                                         cmp rax,rbx            ends 26
                0x74, 0x06, //                                               je scale
                0x48, 0x3b, 0x43, 0x20, //                                   cmp rax,[rbx+0x20]     ends 32
                0x75, 0x0f, //                                               jne done
                // scale:
                0x48, 0xff, 0x05, ...HOOK.i32(SPD_SCALED_SLOT - 41), //      inc qword [rip+scaled] ends 41
                0xf3, 0x0f, 0x59, 0x0d, ...HOOK.i32(SPD_FACTOR_SLOT - 49), // mulss xmm1,[rip+factor] ends 49
                // done:
                0x58, //                                                     pop rax
                ...HOOK.jmpAbs(target + original.length),
            ]),
        { free: false },
    );
}

/** @type {ReturnType<typeof speedHook>[]} */
let speedHooks = [];

// The player's pawn, which is a different object from the actor the Focus
// call names - the movement site works on pawns, so that is what its guard
// has to compare against.
//   mov r8d,[r15+0x290] / shr eax,4 / test al,1
// hooked 7 in, where the two flag instructions are exactly five bytes and r15
// holds the pawn.
const PAWN_SIG =
    '41 8B 87 ?? ?? 00 00 C1 E8 04 A8 01 0F 84 ?? ?? 00 00 ?? 8B ?? ?? ?? 00 00 ?? 85 ?? 74';
const PAWN_HOOK_OFF = 7;
const PAWN_STEAL = 5;
const PAWN_SLOT = 0x100;
const PAWN_HITS_SLOT = 0x108; // a site that never runs says so, rather than looking like a bad guard

/** Record the player's pawn. Copies a register and nothing else. */
function pawnRecorder(target) {
    return HOOK.cave(
        target,
        PAWN_STEAL,
        (at, original) =>
            mem.writeBytes(at + PAWN_SLOT, HOOK.u64(0)) &&
            mem.writeBytes(at + PAWN_HITS_SLOT, HOOK.u64(0)) &&
            mem.writeBytes(at, [
                0x48, 0xff, 0x05, ...HOOK.i32(PAWN_HITS_SLOT - 7), // inc qword [rip+hits] ends 7
                0x4c, 0x89, 0x3d, ...HOOK.i32(PAWN_SLOT - 14), //     mov [rip+pawn],r15   ends 14
                ...original,
                ...HOOK.jmpAbs(target + original.length),
            ]),
        { free: false },
    );
}

/** @type {ReturnType<typeof pawnRecorder>} */
let pawnHook = null;

/** The player's pawn, once the game has run the recorded instruction. */
function pawn() {
    return pawnHook ? mem.u64(pawnHook.cave + PAWN_SLOT) || 0 : 0;
}

/** @type {ReturnType<typeof actorRecorder>} */
let recorder = null;

/** Options currently switched on; the hook is installed only while some are. */
const active = new Set();

/** Install or remove the recorder to match what the user has switched on. */
function syncRecorder() {
    OPT.whileOn('actor', active.size > 0, () => {
        const at = SCAN.once(ACTOR_SIG);
        const h = at ? actorRecorder(at) : null;
        if (h) log(`recorder: site ${at.toString(16)} cave ${h.cave.toString(16)}`);
        else if (at) {
            // The site is there, so this is the cave: a 5-byte jump only
            // reaches +/-2GB, and the free space that close to the game's code
            // fragments into sub-64KB slivers the longer it runs. Restarting
            // the game gives it back.
            log(`recorder: found ${at.toString(16)} but no code cave within jump range - restart the game`);
        }
        recorder = h || null;
        return (
            h && {
                restore() {
                    h.restore();
                    recorder = null;
                },
            }
        );
    });
}

/** The attribute object the recorded call was reading, or 0. */
function recordedSet() {
    return recorder ? mem.u64(recorder.cave + SET_SLOT) || 0 : 0;
}

/** The player's actor, once the game has run the recorded routine. */
function actor() {
    return recorder ? mem.u64(recorder.cave + ACTOR_SLOT) || 0 : 0;
}

/** The address of an enum index's float. */
function slot(attrs, index) {
    return attrs + ATTRS + index * 4;
}

/** The float at an enum index. */
function attr(attrs, index) {
    return mem.f32(slot(attrs, index));
}

/**
 * Whether `p` looks like the attribute object.
 *
 * Health and stamina each have to read as a sane current-against-maximum
 * pair, and both "base" entries have to be populated. Vessel and Focus are
 * deliberately not required: with no vessel equipped they read zero.
 */
function isAttributes(p) {
    if (!p) return false;
    // The options flood current values far above their maximum, so a
    // current-against-maximum test would make the object stop recognising
    // itself the moment one is switched on. Recognition therefore rests on
    // the fields nothing here ever writes - the maxima, the base maxima, and
    // the combat stats - with the live values only required to be sane.
    const span = (index, lo, hi) => {
        const v = attr(p, index);
        return v >= lo && v <= hi;
    };
    if (!span(HP_MAX, 10, 1e6) || !span(STAMINA_MAX, 10, 1e6) || !span(MP_MAX, 1, 1e6)) return false;
    if (!span(HP_MAX_BASE, 10, 1e6) || !span(STAMINA_MAX_BASE, 10, 1e6)) return false;
    if (!span(ATK, 1, 1e6) || !span(DEF, 0, 1e6) || !span(STAMINA_RECOVER, 1, 1e6)) return false;
    return attr(p, HP) >= 0 && attr(p, STAMINA) >= 0 && attr(p, MP) >= 0;
}

// From the recorded object to the attribute array, as seen in the live game.
// The layout has moved before - [0x30, 0x38] stopped leading anywhere and the
// array turned up at [0x20, 0x30, 0x10] - so every known chain is tried
// before anything is searched for.
const SET_CHAINS = [
    [0x30, 0x38],
    [0x20, 0x30, 0x10],
];

// How far into an object to look when no known chain works. Two levels are
// searched wide; a third only through the first pointers of each, which is
// where the chains so far have run, to keep the read count sane.
const ACTOR_SPAN = 0x400;
const CHILD_SPAN = 0x400;
const DEEP_SPAN = 0x80;
const DEEP_CHILD_SPAN = 0x100;

/**
 * Find the attribute object under `root`, returning the offsets that reach it.
 *
 * The offset moves between builds, so it is searched for rather than
 * hard-coded: every pointer in the actor, then every pointer in each of
 * those. The result is cached as a chain, so this runs once and every tick
 * after it is a couple of dereferences.
 *
 * @returns {number[] | null} offsets from the actor, or null
 */
function searchAttributes(root) {
    /** @type {number[]} */
    const children = [];
    for (let off = 0; off < ACTOR_SPAN; off += 8) {
        const p = mem.u64(root + off);
        if (!p) continue;
        if (isAttributes(p)) return [off];
        children.push(off);
    }
    for (const off of children) {
        const child = mem.u64(root + off);
        for (let inner = 0; inner < CHILD_SPAN; inner += 8) {
            const p = mem.u64(child + inner);
            if (p && isAttributes(p)) return [off, inner];
        }
    }
    for (let off = 0; off < DEEP_SPAN; off += 8) {
        const child = mem.u64(root + off);
        if (!child) continue;
        for (let mid = 0; mid < DEEP_CHILD_SPAN; mid += 8) {
            const grandchild = mem.u64(child + mid);
            if (!grandchild) continue;
            for (let inner = 0; inner < DEEP_CHILD_SPAN; inner += 8) {
                const p = mem.u64(grandchild + inner);
                if (p && isAttributes(p)) return [off, mid, inner];
            }
        }
    }
    return null;
}

/** Follow a chain of dereference offsets. */
function follow(from, offsets) {
    let p = from;
    for (const off of offsets) {
        if (!p) return 0;
        p = mem.u64(p + off);
    }
    return p || 0;
}

/** @type {{ from: number, chain: number[] } | null} */
let found = null;

// A search reads thousands of pointers, far more than a 100 ms tick should
// do, so a failed one is spaced out. An actor we have not searched yet is
// always tried at once.
const RESEARCH_MS = 1000;
let searched = { from: 0, at: 0 };

/** Keeps the "where the attributes came from" line to one per attach. */
let reported = false;

/** The player's attribute object, or 0. */
function attributes() {
    const set = recordedSet();
    // Published so research over the devtools channel reads the live roots
    // straight from the config, instead of guessing them from a log line that
    // a later reload has already replaced.
    globalThis.__wk = { cave: recorder ? recorder.cave : 0, set, actor: actor() };
    if (!set) return 0;

    // The usual case: a known chain leads straight to the array.
    for (const chain of SET_CHAINS) {
        const direct = follow(set, chain);
        if (!isAttributes(direct)) continue;
        if (!reported) {
            reported = true;
            log(`attributes: ${direct.toString(16)} via the recorded call, chain ${chain.map((o) => '0x' + o.toString(16)).join(' -> ')}`);
        }
        return direct;
    }

    if (found && found.from === set) {
        const at = follow(set, found.chain);
        if (isAttributes(at)) return at;
    }
    const now = Date.now();
    if (searched.from === set && now - searched.at < RESEARCH_MS) return 0;
    searched = { from: set, at: now };

    const chain = searchAttributes(set);
    if (!chain) {
        found = null;
        return 0;
    }
    found = { from: set, chain };
    log(`attributes: ${set.toString(16)} + ${chain.map((o) => '0x' + o.toString(16)).join(' -> ')}`);
    return follow(set, chain);
}

/**
 * Hold one stat at the value of another.
 *
 * @param {number} over added to the maximum. Focus is a bar the game turns
 *   into whole points, and it credits the last one only once the bar is past
 *   full, so that option asks for one more than the cap.
 */
function holdAtMax(attrs, cur, max, over = 0) {
    const ceiling = attr(attrs, max);
    if (!(ceiling > 0)) return;
    const target = ceiling + over;
    if (attr(attrs, cur) < target) mem.writeF32(slot(attrs, cur), target);
}

/** Ready once the routine we record the player from is there to hook. */
export function live() {
    return SCAN.once(ACTOR_SIG) !== 0;
}

/** An option that pins one stat to its maximum. */
function infinite(name, cur, max, over = 0) {
    return {
        name,
        tick({ on }) {
            if (on) active.add(name);
            else active.delete(name);
            // Unconditionally, so that switching the last option off takes the
            // recorder back out instead of leaving it in the game.
            syncRecorder();
            const a = on && attributes();
            if (a) holdAtMax(a, cur, max, over);
        },
    };
}

/**
 * What a flooded bar is set to.
 *
 * Holding a bar at exactly its maximum still lets every hit land and be put
 * back on the next tick, and the HUD plays its damage animation each time -
 * the bar looks restless even though the value is never really down. Set far
 * above the maximum instead and a hit never brings it near full, so nothing
 * animates and a 100 ms tick is fast enough. The game's own trainer does the
 * same. Nothing displays the number: the stat screen reads the maximum.
 */
const FLOOD = 999999;

/** An option that floods one bar so it never visibly moves. */
function flooded(name, cur) {
    return {
        name,
        tick({ on }) {
            if (on) active.add(name);
            else active.delete(name);
            syncRecorder();
            const a = on && attributes();
            if (a && attr(a, cur) < FLOOD) mem.writeF32(slot(a, cur), FLOOD);
        },
    };
}

/** @type {import('../types/trainer').Entry[]} */
export const options = [
    { separator: 'Survival' },
    flooded('Infinite Health', HP),
    infinite('Infinite Mana', MP, MP_MAX),
    flooded('Infinite Stamina', STAMINA),
    { separator: 'Combat' },
    infinite('Infinite Focus', FOCUS, FOCUS_MAX, 1),
    infinite('Infinite Spirit Energy', SPIRIT, SPIRIT_MAX),
    infinite('Infinite Vessel Energy', VESSEL, VESSEL_MAX),
    {
        // Spells and transformation in one: the spell bar is a countdown in
        // code, while transformation is just a refill rate in the attribute
        // array - so one needs a hook and the other only a write.
        name: 'No Cooldowns',
        tick({ on }) {
            if (on) active.add('No Cooldowns');
            else active.delete('No Cooldowns');
            syncRecorder();

            const applied = OPT.whileOn('cooldown', on, () => {
                const sites = mem.aobAll(CD_SIG, CD_SITES);
                const hooks = sites.map((at) => cooldownHook(at + CD_HOOK_OFF)).filter((h) => h);
                if (!hooks.length) {
                    log(
                        sites.length
                            ? `cooldowns: ${sites.length} site(s) found but no cave within jump range`
                            : 'cooldowns: no matching site in this build',
                    );
                    return null;
                }
                cooldownHooks = hooks;
                log(`cooldowns: ${hooks.length}/${sites.length} site(s) hooked`);
                return {
                    restore() {
                        for (const h of hooks) h.restore();
                        cooldownHooks = [];
                    },
                };
            });

            const a = on && attributes();
            if (a) mem.writeF32(slot(a, ENERGY_INCREASE_SPEED), ENERGY_FLOOD);
            if (!applied) return;
            // the player's component can change (a reload, a new area), so it
            // is vouched for again every tick
            const who = actor();
            for (const h of cooldownHooks) {
                const next = vouch(h.cave, who);
                if (next !== mem.u64(h.cave + GUARD_SLOT)) mem.writeBytes(h.cave + GUARD_SLOT, HOOK.u64(next));
            }
        },
    },
    {
        name: 'Speed',
        levels: [2, 4],
        // no `labels`: plain multipliers get 2x/4x captions by default, and
        // that is also what makes the toast read "Speed: 1x" when off rather
        // than "Speed: OFF" - a captioned level has no natural neutral value.
        keys: ['Alt+F1', 'Alt+F2'],
        tick({ on, mult }) {
            if (on) active.add('Speed');
            else active.delete('Speed');
            syncRecorder();

            const applied = OPT.whileOn('speed', on, () => {
                const at = SCAN.once(SPEED_SIG);
                const h = at ? speedHook(at) : null;
                if (!h) {
                    log(at ? `speed: ${at.toString(16)} found but no cave within reach` : 'speed: no site');
                    return null;
                }
                // the guard needs the pawn, which only this second site names
                const pat = SCAN.once(PAWN_SIG);
                const p = pat ? pawnRecorder(pat + PAWN_HOOK_OFF) : null;
                pawnHook = p || null;
                speedHooks = [h];
                log(`speed: hooked ${at.toString(16)}, pawn ${p ? 'recorder at ' + pat.toString(16) : 'site missing'}`);
                return {
                    restore() {
                        h.restore();
                        if (p) p.restore();
                        speedHooks = [];
                        pawnHook = null;
                    },
                };
            });
            if (!applied) return;

            // the movement site works on pawns, so the pawn is the guard
            const who = pawn();
            for (const h of speedHooks) {
                mem.writeBytes(h.cave + SPD_GUARD_SLOT, HOOK.u64(who));
                mem.writeF32(h.cave + SPD_FACTOR_SLOT, mult);
            }
            // published so the devtools channel can read the run counters
            globalThis.__wkSpeed = speedHooks.length ? speedHooks[0].cave : 0;
            globalThis.__wkPawn = pawnHook ? pawnHook.cave : 0;
        },
    },
];
