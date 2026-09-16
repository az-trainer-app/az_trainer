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
    /**
     * `[base, size, protect, type]` for the committed region `addr` lies in,
     * or `[]` if it is not committed.
     *
     * A signature can match bytes that are never executed - a packed game
     * carries plenty of code-shaped data - and a hook placed there installs
     * cleanly and then never fires. Test `protect & 0xF0` before hooking a
     * site found by signature alone.
     */
    region(addr: Address): number[];

    // ---- value and pointer search (research) -----------------------------

    /**
     * Start a value search over writable memory (heap and the module's data):
     * keep every 4-byte value within `[lo, hi]`, read as a float (the
     * default), an int, or `"any"` for both. Returns the candidate count, or
     * -1 for an unknown type.
     *
     * @example mem.scanStart(1, 100000, 'any')   // health, whichever it is
     */
    scanStart(lo: number, hi: number, type?: 'float' | 'int' | 'any'): number;
    /**
     * Narrow the search. Re-reads every candidate and keeps those that
     * `"decreased"`, `"increased"`, stayed `"unchanged"` or `"changed"` since
     * the last pass, are `"equal"` to `a` (within `b`, default 0.01), or lie
     * `"between"` `a` and `b`. Returns the survivors, or -1 for an unknown
     * mode. Change the value in the game between calls.
     */
    scanNext(mode: 'decreased' | 'increased' | 'unchanged' | 'changed' | 'equal' | 'between', a?: number, b?: number): number;
    /** Up to `n` survivors as a flat `[addr, value, isInt, addr, value, isInt, ...]`. */
    scanResults(n: number): number[];
    /**
     * Every 8-byte slot in readable memory holding a value in `[lo, hi)`, as a
     * flat `[at, value, ...]`, up to `limit` (at most 1,000,000). The step of a
     * reverse pointer search - see lib/research.js `staticPaths`.
     */
    findPointers(lo: Address, hi: Address, limit?: number): number[];
    /** Load address of the game's main module. */
    moduleBase(): Address;
    /** Size of the main module in bytes. */
    moduleSize(): number;
    /**
     * Id of the thread the process started on - Unreal's game thread. A cave
     * that calls game functions should check `gs:[48]` against it. 0 if
     * unknown.
     *
     * Picked by thread creation time, not enumeration order: the latter
     * drifts as a game retires and creates workers, so attaching to a game
     * that has been running a while could otherwise name a worker thread.
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

/** The game a script is attached to. */
interface Game {
    /** The executable that was matched, as the config's `process` spells it. */
    exe: string;
    /** The executable's PE header link timestamp. */
    timestamp: number;
    /** The executable's PE header image size. */
    size: number;
    /** Steam's build id, for a Steam install. */
    steamBuild: number | undefined;
    /** The entry of the config's `builds` that fits this game; `null` without `builds`. */
    build: import('./trainer').Build | null;
}

/**
 * The attached game. Set on attach, so read it from `tick` or `live`, never
 * at a config's top level.
 */
declare const game: Game;

/** Print a line to the trainer's console output, prefixed with `[js]`. */
declare function log(message: string): void;
