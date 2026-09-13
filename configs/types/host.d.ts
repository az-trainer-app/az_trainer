// The API the trainer injects into every script as globals.
//
// Implemented in src/js.rs. Editors load this through configs/jsconfig.json;
// nothing here exists at runtime beyond what the host provides.

/**
 * An address in the game process.
 *
 * A plain number: exact up to 2^53, which covers every user-mode address.
 */
type Address = number;

interface Mem {
    // ---- reading and writing -------------------------------------------

    /** Read 8 bytes, usually a pointer. 0 if the address is unreadable. */
    u64(addr: Address): number;
    /** Read a signed 32-bit integer. 0 if unreadable. */
    i32(addr: Address): number;
    /** Read a 32-bit float. 0 if unreadable. */
    f32(addr: Address): number;
    /** Write a 32-bit float. */
    writeF32(addr: Address, value: number): boolean;
    /** Read `count` bytes. Empty if unreadable. */
    readBytes(addr: Address, count: number): number[];
    /** Write bytes (0-255 each). Works on code pages too. */
    writeBytes(addr: Address, bytes: readonly number[]): boolean;

    // ---- finding code --------------------------------------------------

    /**
     * First match of a byte signature in the game's main module, or 0.
     *
     * Hex bytes separated by spaces. `??` matches any byte and `4?` any byte
     * whose high nibble is 4. Scans the whole module, so cache the result.
     *
     * @example mem.aob('48 8B 05 ?? ?? ?? ?? 4? 8B ?? ?? 48 39 81')
     */
    aob(signature: string): Address;
    /** Every match of a signature, up to `limit`. */
    aobAll(signature: string, limit: number): Address[];
    /**
     * Resolve a RIP-relative operand: the rel32 at `hit + operandOffset`,
     * relative to the end of the `instructionLength`-byte instruction at `hit`.
     *
     * @example
     * // 48 8B 05 <rel32>  mov rax, [rip+rel32]
     * const gworld = mem.rip(hit, 3, 7);
     */
    rip(hit: Address, operandOffset: number, instructionLength: number): Address;
    /** Load address of the game's main module. */
    moduleBase(): Address;
    /** Size of the main module in bytes. */
    moduleSize(): number;
    /**
     * Id of the process's first thread - Unreal's game thread. A cave that
     * calls game functions should check `gs:[48]` against it. 0 if unknown.
     */
    mainThreadId(): number;

    // ---- code injection ------------------------------------------------

    /**
     * Commit an executable, writable block, placed within ±2 GB of `near`
     * when possible so a 5-byte `E9 rel32` jump can reach it. 0 on failure.
     *
     * Prefer the helpers in lib/hook.js, which allocate, write and restore.
     */
    alloc(size: number, near: Address): Address;
    /** Release a block from `alloc`. */
    free(addr: Address): boolean;
    /** Make a range readable, writable and executable. Returns the previous protection flags. */
    protect(addr: Address, size: number): number;

    // ---- high-frequency writer -----------------------------------------

    /**
     * Keep writing `value` to a float about 1000 times a second.
     *
     * For fields the game rewrites every frame, where a write from the 10 Hz
     * tick only flickers. Replaces whatever was held before: the writer holds
     * one address at a time.
     */
    hold(addr: Address, value: number): void;
    /**
     * Keep multiplying whatever the game last wrote to a float.
     *
     * Preserves the game's own tiers - a field that is 1.0 walking and 3.25
     * while hasted stays proportional. Replaces whatever was held before.
     */
    holdScale(addr: Address, multiplier: number): void;
    /** Stop the high-frequency writer. */
    clearHolds(): void;

    // ---- research ------------------------------------------------------

    /**
     * Set a hardware breakpoint on `addr` for `ms` milliseconds and report the
     * instructions that touched it, one line each.
     *
     * Debug-attaches to the game for the duration. Research only - never
     * leave it in a shipped option.
     *
     * @param size 1, 2, 4 or 8 bytes
     * @param access 1 = writes only, 3 = reads and writes
     */
    findAccessors(addr: Address, size: number, access: 1 | 3, ms: number): string[];
}

/** Game memory and code injection, bound to the attached process. */
declare const mem: Mem;

/** Print a line to the trainer's console output, prefixed with `[js]`. */
declare function log(message: string): void;
