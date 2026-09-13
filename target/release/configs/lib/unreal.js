// Generic Unreal Engine 5 helpers.
//
// Nothing here is game-specific: the GWorld signature and the walk down to the
// player Pawn are the same across UE5 titles. A game config imports this and
// only supplies its own AttributeSet / component offsets.
//
// Host API (provided by the trainer):
//   mem.u64(a) mem.i32(a) mem.f32(a)   reads, 0 / NaN on failure
//   mem.writeF32(a, v)                 write, returns bool
//   mem.aob(sig)                       scan main module, returns hit or 0
//   mem.rip(hit, pos, len)             resolve a RIP-relative operand
//   log(msg)

const GWORLD_SIG = '48 8B 05 ?? ?? ?? ?? 4? 8B ?? ?? 48 39 81 C0 02 00 00';

// UObject graph offsets (UE5, stable across most shipping builds)
const GAME_INSTANCE = 0x1D8;
const LOCAL_PLAYERS = 0x38;
const PLAYER_CTRL = 0x30;
const PAWN = 0x2F8;

let _gworld = 0;

/** Address of the GWorld pointer. Scanned once, then cached. */
export function gworld() {
    if (_gworld) return _gworld;
    const hit = mem.aob(GWORLD_SIG);
    if (!hit) return 0;
    _gworld = mem.rip(hit, 3, 7);
    return _gworld;
}

/** The local APlayerController, or 0 when in a menu / loading. */
export function playerController() {
    const gw = gworld();
    if (!gw) return 0;
    let p = mem.u64(gw);                    // UWorld
    if (p) p = mem.u64(p + GAME_INSTANCE);  // UGameInstance
    if (p) p = mem.u64(p + LOCAL_PLAYERS);  // TArray<ULocalPlayer*>
    if (p) p = mem.u64(p);                  // LocalPlayers[0]
    if (p) p = mem.u64(p + PLAYER_CTRL);    // APlayerController
    return p || 0;
}

/** Walk a chain of dereference offsets from a base address. */
export function chain(base, offsets) {
    let p = base;
    for (const off of offsets) {
        if (!p) return 0;
        p = mem.u64(p + off);
    }
    return p || 0;
}

/** The local player's Pawn, or 0 when in a menu / loading. */
export function pawn() {
    const gw = gworld();
    if (!gw) return 0;
    let p = mem.u64(gw);                    // UWorld
    if (p) p = mem.u64(p + GAME_INSTANCE);  // UGameInstance
    if (p) p = mem.u64(p + LOCAL_PLAYERS);  // TArray<ULocalPlayer*>
    if (p) p = mem.u64(p);                  // LocalPlayers[0]
    if (p) p = mem.u64(p + PLAYER_CTRL);    // APlayerController
    if (p) p = mem.u64(p + PAWN);           // APawn
    return p || 0;
}

/** A component or sub-object hanging off an actor. */
export function comp(actor, off) {
    return actor ? mem.u64(actor + off) : 0;
}

// --- FGameplayAttributeData -------------------------------------------------
// Layout: { ..., float BaseValue @ +0x8, float CurrentValue @ +0xC }

export function attrGet(set, off) {
    return mem.f32(set + off + 0xC);
}

export function attrSet(set, off, v) {
    mem.writeF32(set + off + 0x8, v);
    mem.writeF32(set + off + 0xC, v);
}

/**
 * Hold `attr` at the value of `maxAttr`.
 * Pass apply=false to read without writing (so the UI still shows live values
 * while the option is off). Returns {cur, max} or null.
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

/** Like hold(), but max is a product of two attributes (segments x per-segment). */
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

export function fmt(label, v) {
    return v ? label + '  ' + Math.round(v.cur) + ' / ' + Math.round(v.max) : undefined;
}

// --- constants with automatic restore ---------------------------------------

const _saved = new Map();

/**
 * Write a constant while `on`, remembering the original so it can be put back
 * when the option is switched off. Use for fields the game does not
 * recompute on its own (speeds, gravity, FOV).
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
 * Write both halves of an FGameplayAttributeData, remembering the originals.
 * Base and Current are both set because the ability system recomputes Current
 * from Base, so writing only one gets undone on the next recalculation.
 */
export function pokeAttr(set, attr, value, on) {
    if (!set) return;
    poke(set + attr + 0x8, value, on);
    poke(set + attr + 0xC, value, on);
}

/** Drop remembered originals (call when the pawn changes, e.g. after a load). */
export function forget() {
    _saved.clear();
}
