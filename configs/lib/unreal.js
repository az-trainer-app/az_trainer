// Generic Unreal Engine 5 helpers.
//
// Nothing here is game-specific: the GWorld signature and the walk down to the
// player Pawn are the same across UE5 titles. A game config imports this and
// only supplies its own AttributeSet / component offsets.

/**
 * An attribute's live value against its maximum.
 * @typedef {{ cur: number, max: number }} Reading
 */

const GWORLD_SIG = '48 8B 05 ?? ?? ?? ?? 4? 8B ?? ?? 48 39 81 C0 02 00 00';

// UObject graph offsets (UE5, stable across most shipping builds)
const GAME_INSTANCE = 0x1d8;
const LOCAL_PLAYERS = 0x38;
const PLAYER_CTRL = 0x30;
const PAWN = 0x2f8;
const PERSISTENT_LEVEL = 0x30; // UWorld -> ULevel
const WORLD_SETTINGS = 0x2b0; // ULevel -> AWorldSettings

let _gworld = 0;

/**
 * Address of the GWorld pointer. Scanned once, then cached.
 * @returns {number} 0 if the signature is not found
 */
export function gworld() {
    if (_gworld) return _gworld;
    const hit = mem.aob(GWORLD_SIG);
    if (!hit) return 0;
    _gworld = mem.rip(hit, 3, 7);
    return _gworld;
}

/**
 * The local APlayerController.
 * @returns {number} 0 when in a menu or loading
 */
export function playerController() {
    const gw = gworld();
    if (!gw) return 0;
    let p = mem.u64(gw); // UWorld
    if (p) p = mem.u64(p + GAME_INSTANCE); // UGameInstance
    if (p) p = mem.u64(p + LOCAL_PLAYERS); // TArray<ULocalPlayer*>
    if (p) p = mem.u64(p); // LocalPlayers[0]
    if (p) p = mem.u64(p + PLAYER_CTRL); // APlayerController
    return p || 0;
}

/**
 * Walk a chain of dereference offsets from a base address: `[[[base+a]+b]+c]`.
 * @param {number} base
 * @param {number[]} offsets
 * @returns {number} 0 if any link is null
 */
export function chain(base, offsets) {
    let p = base;
    for (const off of offsets) {
        if (!p) return 0;
        p = mem.u64(p + off);
    }
    return p || 0;
}

/**
 * The local player's Pawn.
 * @returns {number} 0 when in a menu or loading
 */
export function pawn() {
    const pc = playerController();
    return pc ? mem.u64(pc + PAWN) || 0 : 0;
}

/**
 * The persistent level's AWorldSettings: time dilation, gravity and other
 * per-world settings.
 * @returns {number} 0 when no world is loaded
 */
export function worldSettings() {
    const gw = gworld();
    return gw ? chain(mem.u64(gw), [PERSISTENT_LEVEL, WORLD_SETTINGS]) : 0;
}

/** @type {Map<string, { obj: number, vtable: number, since: number }>} */
const _settled = new Map();

/**
 * `obj` once it has stayed the same live object for `ms`, otherwise 0.
 *
 * A load frees the old world's objects and hands their memory to new ones.
 * Writing through a pointer read a moment too early lands in whatever took
 * its place and corrupts it - which surfaces as a crash on a later load. So
 * an object must keep the same address and the same class (its vtable,
 * inside the game module) across ticks before anything writes to it.
 *
 * @param {string} key one per pointer being tracked
 * @param {number} obj
 * @param {number} [ms]
 * @returns {number}
 */
export function settled(key, obj, ms = 1500) {
    const vtable = obj ? mem.u64(obj) : 0;
    const base = mem.moduleBase();
    if (!vtable || vtable < base || vtable >= base + mem.moduleSize()) {
        _settled.delete(key);
        return 0;
    }
    const now = Date.now();
    const seen = _settled.get(key);
    if (!seen || seen.obj !== obj || seen.vtable !== vtable) {
        _settled.set(key, { obj, vtable, since: now });
        return 0;
    }
    return now - seen.since >= ms ? obj : 0;
}

/**
 * A component or sub-object hanging off an actor.
 * @param {number} actor
 * @param {number} off
 * @returns {number}
 */
export function comp(actor, off) {
    return actor ? mem.u64(actor + off) : 0;
}

// --- FGameplayAttributeData -------------------------------------------------
// Layout: { ..., float BaseValue @ +0x8, float CurrentValue @ +0xC }

/**
 * An attribute's CurrentValue.
 * @param {number} set the AttributeSet
 * @param {number} off the attribute's offset within the set
 * @returns {number}
 */
export function attrGet(set, off) {
    return mem.f32(set + off + 0xc);
}

/**
 * Set both BaseValue and CurrentValue, so a recalculation does not undo it.
 * @param {number} set
 * @param {number} off
 * @param {number} v
 */
export function attrSet(set, off, v) {
    mem.writeF32(set + off + 0x8, v);
    mem.writeF32(set + off + 0xc, v);
}

/**
 * Hold `attr` at the value of `maxAttr`.
 *
 * @param {number} set
 * @param {number} attr
 * @param {number} maxAttr
 * @param {boolean} apply false reads without writing
 * @returns {Reading | null} null if the maximum is not readable yet
 */
export function hold(set, attr, maxAttr, apply) {
    let max = attrGet(set, maxAttr);
    if (!(max > 0)) max = mem.f32(set + maxAttr + 0x8);
    if (!(max > 0)) return null;

    let cur = attrGet(set, attr);
    if (apply) {
        attrSet(set, attr, max);
        cur = max;
    }
    return { cur: cur, max: max };
}

/**
 * Like hold(), but the maximum is the product of two attributes
 * (segments x per-segment), so it tracks upgrades.
 *
 * @param {number} set
 * @param {number} attr
 * @param {number} aAttr
 * @param {number} bAttr
 * @param {boolean} apply
 * @returns {Reading | null}
 */
export function holdProduct(set, attr, aAttr, bAttr, apply) {
    const a = attrGet(set, aAttr);
    const b = attrGet(set, bAttr);
    const max = a > 0 && b > 0 ? a * b : attrGet(set, attr);
    if (!(max > 0)) return null;

    let cur = attrGet(set, attr);
    if (apply) {
        attrSet(set, attr, max);
        cur = max;
    }
    return { cur: cur, max: max };
}

/**
 * `"Health  1746 / 1746"`, or undefined when there is no reading.
 * @param {string} label
 * @param {Reading | null} v
 * @returns {string | undefined}
 */
export function fmt(label, v) {
    return v ? label + '  ' + Math.round(v.cur) + ' / ' + Math.round(v.max) : undefined;
}

// --- constants with automatic restore ---------------------------------------

/** @type {Map<number, number>} */
const _saved = new Map();

/**
 * Write a constant while `on`, remembering the original so it can be put back
 * when the option is switched off. Use for fields the game does not
 * recompute on its own (speeds, gravity, FOV).
 *
 * @param {number} addr
 * @param {number} value
 * @param {boolean} on
 */
export function poke(addr, value, on) {
    if (!addr) return;
    if (on) {
        if (!_saved.has(addr)) _saved.set(addr, mem.f32(addr));
        mem.writeF32(addr, value);
    } else if (_saved.has(addr)) {
        mem.writeF32(addr, _saved.get(addr));
        _saved.delete(addr);
    }
}

/**
 * poke() both halves of an FGameplayAttributeData. Base and Current are both
 * set because the ability system recomputes Current from Base, so writing
 * only one gets undone on the next recalculation.
 *
 * @param {number} set
 * @param {number} attr
 * @param {number} value
 * @param {boolean} on
 */
export function pokeAttr(set, attr, value, on) {
    if (!set) return;
    poke(set + attr + 0x8, value, on);
    poke(set + attr + 0xc, value, on);
}

/** Drop remembered originals (call when the pawn changes, e.g. after a load). */
export function forget() {
    _saved.clear();
}
