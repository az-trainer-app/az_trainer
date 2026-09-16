// 007 First Light - IO Interactive's Glacier engine.
//
// Sites from the community cheat table for this game (its God Mode, Instinct
// Always Max and Q-Lens resources scripts), confirmed unique in game version
// 1.2.0. The table finds the player by hooking the "get local player" routine
// and calling the engine's interface lookup from its cave; that routine only
// reads a global, so here the same lookup is done from the script instead and
// no call is ever made from injected code.

import * as HOOK from '../lib/hook.js';
import * as OPT from '../lib/option.js';
import * as SCAN from '../lib/scan.js';

export const process = '007FirstLight.exe';
export const title = '007 First Light';

// lea rax,[ZCLGetLocalPlayerCharacter vftable] / mov [rcx+..],rax / ...
// The vftable's slot 5 is the routine, which opens with mov rax,[rip+player].
const PLAYER_SIG = '48 8D 05 ?? ?? ?? ?? 48 89 41 ?? 48 8D 0D ?? ?? ?? ?? FF 15 ?? ?? ?? ?? 4C';
// imul rdx,rcx,.. / add rbx,rdx / lea rdx,[TTypeIDHelper<ZHumanoidCharacterEntity>::id]
const HUMANOID_SIG = '48 69 D1 ?? ?? ?? ?? 48 03 DA 48 8D 15 ?? ?? ?? ?? 48 8B CB E8 ?? ?? ?? ?? 4C';
// vmovss [rsp+..],xmm0 / call ZActor::YouGotHit
const HIT_SIG = 'C5 FA 11 44 24 ?? E8 ?? ?? ?? ?? 4D ?? ?? ?? ?? ?? ?? 8D 4F 01';
// mov rax,[rax+..] / vcomiss xmm0,[r12+rax+4] - current instinct against the stimulus
const INSTINCT_SIG = '48 8B 80 ?? ?? ?? ?? C4 C1 78 2F 44 04 ?? 76 ?? 32 C0';
const INSTINCT_OFF = 7;
// vmovss xmm2,[rcx+8] - a Q-Lens resource's current value, in its update loop
const QLENS_SIG = 'C5 FA 10 51 ?? 44 38 79 ?? 74 ?? C5';

// Where caves keep their data, and a counter of how often each ran - a hook
// that never fires looks exactly like one that does nothing.
const PLAYER_ID_SLOT = 0x100;
const HITS_SLOT = 0x108;
const BLOCKED_SLOT = 0x110;

/** A live address found through a RIP-relative operand. */
function ripTarget(at, operand, length) {
    return at + length + mem.i32(at + operand);
}

/** The global the local player lives in, or 0. */
function playerGlobal() {
    const at = SCAN.once(PLAYER_SIG);
    if (!at) return 0;
    const vftable = ripTarget(at, 3, 7);
    const routine = mem.u64(vftable + 0x28);
    // mov rax,[rip+player]
    const b = mem.readBytes(routine, 3);
    if (b[0] !== 0x48 || b[1] !== 0x8b || b[2] !== 0x05) return 0;
    return ripTarget(routine, 3, 7);
}

/**
 * The engine's ZEntityRef::QueryInterfacePtr, done with reads: an entity's
 * type lists `{type id, offset}` pairs, and the interface is the entity plus
 * that offset (which can be negative).
 */
function queryInterface(entity, typeId) {
    const type = entity ? mem.u64(entity) : 0;
    const list = type ? mem.u64(type + 0x20) : 0;
    if (!list) return 0;
    const end = mem.u64(list + 8);
    for (let e = mem.u64(list), n = 0; e && e < end && n < 256; e += 16, n++) {
        if (mem.u64(e) !== typeId) continue;
        return entity + mem.i32(e + 12) * 0x100000000 + (mem.i32(e + 8) >>> 0);
    }
    return 0;
}

/** The player's character id - what ZActor::YouGotHit is called with - or 0. */
function playerId() {
    const slot = playerGlobal();
    const humanoid = SCAN.once(HUMANOID_SIG);
    if (!slot || !humanoid) return 0;
    const holder = mem.u64(slot);
    const entity = holder ? mem.u64(holder + 0x18) : 0;
    const character = queryInterface(entity, ripTarget(humanoid + 0xa, 3, 7));
    return character ? mem.i32(character + 0x200) : 0;
}

/**
 * Skip damage to the player: ZActor::YouGotHit returns at once when the
 * struct it is handed carries the player's id. Other characters are hit as
 * usual; with the id not known yet, everyone is.
 */
function godModeHook(target) {
    return HOOK.cave(
        target,
        5,
        (at, original) =>
            mem.writeBytes(at + PLAYER_ID_SLOT, HOOK.u64(0)) &&
            mem.writeBytes(at + HITS_SLOT, HOOK.u64(0)) &&
            mem.writeBytes(at + BLOCKED_SLOT, HOOK.u64(0)) &&
            mem.writeBytes(at, [
                0x48, 0xff, 0x05, ...HOOK.i32(HITS_SLOT - 7), //      inc qword [rip+hits]    ends 7
                0x8b, 0x05, ...HOOK.i32(PLAYER_ID_SLOT - 13), //      mov eax,[rip+id]        ends 13
                0x85, 0xc0, //                                        test eax,eax            ends 15
                0x74, 0x0c, //                                        je original             ends 17
                0x39, 0x01, //                                        cmp [rcx],eax           ends 19
                0x75, 0x08, //                                        jne original            ends 21
                0x48, 0xff, 0x05, ...HOOK.i32(BLOCKED_SLOT - 28), //  inc qword [rip+blocked] ends 28
                0xc3, //                                              ret                     ends 29
                ...original, //                                       mov [rsp+10],rdx        @29
                ...HOOK.jmpAbs(target + original.length),
            ]),
        { free: false },
    );
}

/** Copy maximum instinct over current as the game compares against it. */
function instinctHook(target) {
    return HOOK.cave(
        target,
        7,
        (at, original) =>
            mem.writeBytes(at + HITS_SLOT, HOOK.u64(0)) &&
            mem.writeBytes(at, [
                0x48, 0xff, 0x05, ...HOOK.i32(HITS_SLOT - 7), // inc qword [rip+hits]     ends 7
                0xc4, 0xc1, 0x7a, 0x10, 0x0c, 0x04, //          vmovss xmm1,[r12+rax]    ends 13
                0xc4, 0xc1, 0x7a, 0x11, 0x4c, 0x04, 0x04, //    vmovss [r12+rax+4],xmm1  ends 20
                ...original, //                                 vcomiss xmm0,[r12+rax+4]
                ...HOOK.jmpAbs(target + original.length),
            ]),
        { free: false },
    );
}

/** Copy a Q-Lens resource's maximum over its current value, each update. */
function qlensHook(target) {
    return HOOK.cave(
        target,
        5,
        (at, original) =>
            mem.writeBytes(at + HITS_SLOT, HOOK.u64(0)) &&
            mem.writeBytes(at, [
                0x48, 0xff, 0x05, ...HOOK.i32(HITS_SLOT - 7), // inc qword [rip+hits] ends 7
                0xc5, 0xfa, 0x10, 0x51, 0x18, //                vmovss xmm2,[rcx+18] ends 12
                0xc5, 0xfa, 0x11, 0x51, 0x08, //                vmovss [rcx+8],xmm2  ends 17
                ...original, //                                 vmovss xmm2,[rcx+8]
                ...HOOK.jmpAbs(target + original.length),
            ]),
        { free: false },
    );
}

/** An option that keeps one cave installed while it is on. */
function hooked(name, key, find, install, each) {
    let hook = null;
    return {
        name,
        keys: [key],
        tick({ on }) {
            const applied = OPT.whileOn(name, on, () => {
                const at = find();
                const h = at ? install(at) : null;
                if (!h) {
                    log(at ? `${name}: no code cave within reach` : `${name}: site not found`);
                    return null;
                }
                hook = h;
                // published so the devtools channel can read the counters
                globalThis.__firstlight = { ...(globalThis.__firstlight || {}), [name]: h.cave };
                return {
                    restore() {
                        h.restore();
                        hook = null;
                    },
                };
            });
            if (applied && hook && each) each(hook);
        },
    };
}

/** Ready once the damage routine is there to find. */
export function live() {
    return SCAN.once(HIT_SIG) !== 0;
}

/** @type {import('../types/trainer').Entry[]} */
export const options = [
    hooked(
        'Infinite Health',
        'Alt+F1',
        () => {
            const at = SCAN.once(HIT_SIG);
            return at ? ripTarget(at + 6, 1, 5) : 0;
        },
        godModeHook,
        // the player's id can change with a level load, so it is refreshed
        (h) => mem.writeBytes(h.cave + PLAYER_ID_SLOT, HOOK.i32(playerId())),
    ),
    hooked('Infinite Instinct', 'Alt+F2', () => {
        const at = SCAN.once(INSTINCT_SIG);
        return at ? at + INSTINCT_OFF : 0;
    }, instinctHook),
    hooked('Infinite Q-Lens Resources', 'Alt+F3', () => SCAN.once(QLENS_SIG), qlensHook),
];
