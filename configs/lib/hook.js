// x64 code injection helpers.
//
// A detour works the standard way:
//   1. copy the instruction bytes we are about to overwrite ("stolen bytes")
//   2. build a cave: [your code][stolen bytes][jmp back]
//   3. overwrite the target with `jmp cave`, padding with NOP
// Disabling writes the stolen bytes back and frees the cave.
//
// WARNING: the stolen length must land on an instruction boundary. Cutting an
// instruction in half will crash the game. Use a disassembler (or Cheat
// Engine's view) to pick the length -- do not guess.

/**
 * Undoes a patch.
 * @typedef {object} Restorable
 * @property {() => void} restore Put the original bytes back (and free any cave).
 */

/**
 * A byte patch applied in place.
 * @typedef {Restorable & { addr: number, original: number[] }} Patch
 */

/**
 * An installed jump to a code cave.
 * @typedef {Restorable & { target: number, cave: number, original: number[] }} Detour
 */

/**
 * Little-endian 4-byte two's complement.
 * @param {number} v
 * @returns {number[]}
 */
export function i32(v) {
    const n = v < 0 ? v + 0x100000000 : v;
    return [n & 0xff, (n >>> 8) & 0xff, (n >>> 16) & 0xff, (n >>> 24) & 0xff];
}

/**
 * Little-endian 8-byte address.
 * @param {number} v
 * @returns {number[]}
 */
export function u64(v) {
    const out = [];
    let hi = Math.floor(v / 0x100000000);
    let lo = v >>> 0;
    for (let i = 0; i < 4; i++, lo >>>= 8) out.push(lo & 0xff);
    for (let i = 0; i < 4; i++, hi >>>= 8) out.push(hi & 0xff);
    return out;
}

/**
 * `jmp [rip+0]; <absolute 64-bit target>` -- 14 bytes, reaches anywhere.
 * @param {number} target
 * @returns {number[]}
 */
export function jmpAbs(target) {
    return [0xff, 0x25, 0x00, 0x00, 0x00, 0x00, ...u64(target)];
}

/**
 * Plain byte patch with restore -- no cave, no jump.
 *
 * Most published cheat-table edits are this simple: overwrite an instruction
 * with a branch or a zeroing one of the same length.
 *
 * @param {number} addr
 * @param {number[]} bytes
 * @returns {Patch | null}
 */
export function patch(addr, bytes) {
    if (!addr || !bytes.length) return null;
    const original = mem.readBytes(addr, bytes.length);
    if (!original || original.length !== bytes.length) return null;
    if (!mem.writeBytes(addr, bytes)) return null;
    return {
        addr,
        original,
        restore() {
            mem.writeBytes(addr, original);
        },
    };
}

/**
 * Point the first `stealLen` bytes of `target` at a fresh code cave.
 *
 * `fill(cave, original)` writes the cave - code and any data slots - and
 * returns false if it could not. Only then does the jump go in, so the game
 * never runs a half-written cave.
 *
 * @param {number} target
 * @param {number} stealLen bytes to overwrite; >= 5, ending on an instruction boundary
 * @param {(cave: number, original: number[]) => boolean} fill
 * @param {{ free?: boolean }} [opts] `free: false` keeps the cave allocated
 *   after restore(), for caves the game may still be executing
 * @returns {Detour | null} null if it could not be installed
 */
export function cave(target, stealLen, fill, { free = true } = {}) {
    if (!target || stealLen < 5) return null;
    const original = mem.readBytes(target, stealLen);
    if (!original || original.length !== stealLen) return null;
    const at = mem.alloc(0x1000, target);
    if (!at) return null;

    const rel = at - (target + 5); // E9 rel32 must reach the cave
    const inRange = rel <= 0x7ffffff0 && rel >= -0x7ffffff0;
    if (!inRange || !fill(at, original) || !mem.writeBytes(target, [0xe9, ...i32(rel), ...nops(stealLen - 5)])) {
        mem.free(at);
        return null;
    }
    return {
        target,
        cave: at,
        original,
        restore() {
            mem.writeBytes(target, original);
            if (free) mem.free(at);
        },
    };
}

/**
 * Install a detour: your code runs, then the stolen instruction, then the
 * game continues.
 *
 * @param {number} target where to hook
 * @param {number} stealLen bytes to overwrite; >= 5, ending on an instruction boundary
 * @param {number[]} code your instruction bytes, run before the stolen ones
 * @returns {Detour | null} null if it could not be installed
 */
export function detour(target, stealLen, code) {
    return cave(target, stealLen, (at, original) =>
        mem.writeBytes(at, [...code, ...original, ...jmpAbs(target + stealLen)]),
    );
}

/**
 * Hold a field at a sibling field's value, for the entity a flag identifies.
 *
 * Games routinely funnel every entity through one routine, so the useful
 * question is never "which address is the player" but "how does the game
 * itself tell". Where a flag exists, reading it beats any statistical guess.
 *
 * Copying `maxOff` rather than writing a constant keeps the HUD honest: a
 * flat huge value makes current/max meaningless and the bar renders empty,
 * and it also survives levelling, since max moves on its own.
 *
 *   push  rcx
 *   mov   rcx, [rcx+flagPtrOff]
 *   cmp   dword [rcx+flagOff], 0
 *   pop   rcx
 *   je    skip
 *   push  rax
 *   mov   eax, [rcx+maxOff]
 *   mov   [rcx+curOff], eax
 *   pop   rax
 *   skip: <stolen bytes>
 *   jmp   back
 *
 * The copy is `mov`, which leaves flags alone, and the stolen compare runs
 * afterwards either way.
 *
 * @param {number} target a routine entered with the entity in rcx
 * @param {number} stealLen
 * @param {number} flagPtrOff offset from rcx of the object holding the flag
 * @param {number} flagOff offset of the dword flag in that object; nonzero = act
 * @param {number} curOff field to overwrite
 * @param {number} maxOff field to copy from
 * @returns {Detour | null}
 */
export function holdFieldAtSibling(target, stealLen, flagPtrOff, flagOff, curOff, maxOff) {
    const copy = [
        0x50,                          // push rax
        0x8b, 0x41, maxOff & 0xff,     // mov eax, [rcx+maxOff]
        0x89, 0x41, curOff & 0xff,     // mov [rcx+curOff], eax
        0x58,                          // pop rax
    ];
    return cave(target, stealLen, (at, original) =>
        mem.writeBytes(at, [
            0x51,                                  // push rcx
            0x48, 0x8b, 0x49, flagPtrOff & 0xff,   // mov rcx, [rcx+flagPtrOff]
            0x83, 0xb9, ...i32(flagOff), 0x00,     // cmp dword [rcx+flagOff], 0
            0x59,                                  // pop rcx
            0x74, copy.length,                     // je skip
            ...copy,
            ...original,
            ...jmpAbs(target + stealLen),
        ]),
    );
}

// --- instruction encoders ---------------------------------------------------

/**
 * A 32-bit general-purpose register.
 * @typedef {'eax' | 'ecx' | 'edx' | 'ebx' | 'esp' | 'ebp' | 'esi' | 'edi'
 *   | 'r8d' | 'r9d' | 'r10d' | 'r11d' | 'r12d' | 'r13d' | 'r14d' | 'r15d'} Reg32
 */

/**
 * An SSE register.
 * @typedef {'xmm0' | 'xmm1' | 'xmm2' | 'xmm3' | 'xmm4' | 'xmm5' | 'xmm6' | 'xmm7'
 *   | 'xmm8' | 'xmm9' | 'xmm10' | 'xmm11' | 'xmm12' | 'xmm13' | 'xmm14' | 'xmm15'} Xmm
 */

const REG32 = ['eax', 'ecx', 'edx', 'ebx', 'esp', 'ebp', 'esi', 'edi'];
for (let i = 8; i < 16; i++) REG32.push(`r${i}d`);
const XMM = Array.from({ length: 16 }, (_, i) => `xmm${i}`);

/**
 * @param {string[]} names
 * @param {string} name
 */
function regIndex(names, name) {
    const i = names.indexOf(name);
    if (i < 0) throw new Error(`unknown register: ${name}`);
    return i;
}

/**
 * `mov r32, imm32`
 * @param {Reg32} reg
 * @param {number} imm
 * @returns {number[]}
 */
export function movR32Imm(reg, imm) {
    const r = regIndex(REG32, reg);
    return [...(r >= 8 ? [0x41] : []), 0xb8 + (r & 7), ...i32(imm)];
}

/**
 * `add r32, imm32`
 * @param {Reg32} reg
 * @param {number} imm
 * @returns {number[]}
 */
export function addR32Imm(reg, imm) {
    const r = regIndex(REG32, reg);
    return [...(r >= 8 ? [0x41] : []), 0x81, 0xc0 | (r & 7), ...i32(imm)];
}

/**
 * `xorps dst, src` -- with the same register twice, sets it to 0.0.
 * @param {Xmm} dst
 * @param {Xmm} src
 * @returns {number[]}
 */
export function xorps(dst, src) {
    const d = regIndex(XMM, dst);
    const s = regIndex(XMM, src);
    const rex = (d >= 8 ? 4 : 0) | (s >= 8 ? 1 : 0);
    return [...(rex ? [0x40 | rex] : []), 0x0f, 0x57, 0xc0 | ((d & 7) << 3) | (s & 7)];
}

/**
 * `jmp rel8` -- skip the next `n` bytes.
 * @param {number} n -128 to 127
 * @returns {number[]}
 */
export function jmpShort(n) {
    if (n < -128 || n > 127) throw new Error(`short jump out of range: ${n}`);
    return [0xeb, n & 0xff];
}

/**
 * `n` single-byte NOPs.
 * @param {number} n
 * @returns {number[]}
 */
export function nops(n) {
    return new Array(n).fill(0x90);
}
