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
    for (let i = 0; i < 4; i++) {
        out.push(lo & 0xff);
        lo >>>= 8;
    }
    for (let i = 0; i < 4; i++) {
        out.push(hi & 0xff);
        hi >>>= 8;
    }
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
 * Overwrite `len` bytes with NOP.
 * @param {number} addr
 * @param {number} len
 * @returns {number[]} the original bytes
 */
export function nopOut(addr, len) {
    const original = mem.readBytes(addr, len);
    mem.writeBytes(addr, new Array(len).fill(0x90));
    return original;
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
    if (!target || stealLen < 5) return null;

    const original = mem.readBytes(target, stealLen);
    if (!original || original.length !== stealLen) return null;

    const cave = mem.alloc(0x1000, target);
    if (!cave) return null;

    // cave: [code][stolen][jmp back to after the patch]
    const body = [...code, ...original, ...jmpAbs(target + stealLen)];
    if (!mem.writeBytes(cave, body)) {
        mem.free(cave);
        return null;
    }

    // target: E9 rel32 -> cave, NOP padding
    const rel = cave - (target + 5);
    if (rel > 0x7ffffff0 || rel < -0x7ffffff0) {
        mem.free(cave); // allocation landed out of jump range
        return null;
    }
    const patch = [0xe9, ...i32(rel)];
    while (patch.length < stealLen) patch.push(0x90);
    if (!mem.writeBytes(target, patch)) {
        mem.free(cave);
        return null;
    }

    return {
        target,
        cave,
        original,
        restore() {
            mem.writeBytes(target, original);
            mem.free(cave);
        },
    };
}

/**
 * Keep a detour in step with an on/off switch, installing and removing it as
 * needed.
 *
 * @param {{ handle: Detour | null }} state holds the installed detour between calls
 * @param {boolean} on
 * @param {number} target
 * @param {number} stealLen
 * @param {() => number[]} build returns the code bytes; called at install time
 * @returns {boolean} whether the detour is installed
 */
export function toggle(state, on, target, stealLen, build) {
    if (on && !state.handle) {
        state.handle = detour(target, stealLen, build());
        return !!state.handle;
    }
    if (!on && state.handle) {
        state.handle.restore();
        state.handle = null;
    }
    return !!state.handle;
}

/**
 * Guarded scale hook -- generic, reusable across games.
 *
 * Many engines funnel float writes through a shared setter of the shape:
 *
 *     movss [rcx+A], xmm1     <- 5-byte entry, the hook site
 *     ...
 *     mov  rax, [rcx+G]       <- G = guardOffset, holds the destination
 *     movss [rax], xmm1
 *     ret
 *
 * Scaling xmm1 blindly would corrupt every value the setter handles, so the
 * cave compares [rcx+guardOffset] against an address you supply and only
 * scales on a match. Both the guarded address and the multiplier live in data
 * slots you can rewrite at any time without reinstalling the hook -- which
 * matters because the object usually moves when the game reloads.
 *
 * @param {number} target the setter's 5-byte entry
 * @param {number} stealLen
 * @param {number} guardOffset offset of the destination pointer from rcx
 * @returns {(Detour & {
 *   setGuard(addr: number): void,
 *   setMult(multiplier: number): void,
 *   counts(): [calls: number, matches: number],
 * }) | null}
 */
export function guardedScale(target, stealLen, guardOffset) {
    if (!target || stealLen < 5) return null;

    const original = mem.readBytes(target, stealLen);
    if (!original || original.length !== stealLen) return null;

    const cave = mem.alloc(0x1000, target);
    if (!cave) return null;

    const slotAddr = cave + 0x40;
    const slotMult = cave + 0x48;
    const slotCalls = cave + 0x50; // times the setter ran at all
    const slotHits = cave + 0x58; // times the guard matched (we scaled)

    // Layout (offsets within the cave):
    //   0x00 mov   r10,[rcx+G]
    //   0x04 inc   qword [calls]        <- every call through the setter
    //   0x0B cmp   r10,[slotAddr]
    //   0x12 jne   skip -> 0x23
    //   0x14 inc   qword [hits]         <- calls where the guard matched
    //   0x1B mulss xmm1,[slotMult]
    //   0x23 <stolen>                   (skip lands here)
    //   ...  jmp   back
    // prettier-ignore
    const code = [
        0x4c, 0x8b, 0x51, guardOffset & 0xff,                     // mov r10,[rcx+G]
        0x48, 0xff, 0x05, ...i32(slotCalls - (cave + 0x0b)),      // inc qword [calls]
        0x4c, 0x3b, 0x15, ...i32(slotAddr - (cave + 0x12)),       // cmp r10,[slotAddr]
        0x75, 0x0f,                                               // jne skip
        0x48, 0xff, 0x05, ...i32(slotHits - (cave + 0x1b)),       // inc qword [hits]
        0xf3, 0x0f, 0x59, 0x0d, ...i32(slotMult - (cave + 0x23)), // mulss xmm1,[slotMult]
        ...original,
        ...jmpAbs(target + stealLen),
    ];
    if (code.length > 0x40) {
        mem.free(cave);
        return null;
    }

    const rel = cave - (target + 5);
    if (rel > 0x7ffffff0 || rel < -0x7ffffff0) {
        mem.free(cave);
        return null;
    }

    mem.writeBytes(cave, code);
    mem.writeBytes(slotAddr, u64(0)); // no guard yet: scales nothing
    mem.writeF32(slotMult, 1.0);
    mem.writeBytes(slotCalls, u64(0));
    mem.writeBytes(slotHits, u64(0));

    const patch = [0xe9, ...i32(rel)];
    while (patch.length < stealLen) patch.push(0x90);
    if (!mem.writeBytes(target, patch)) {
        mem.free(cave);
        return null;
    }

    return {
        target,
        cave,
        original,
        setGuard(addr) {
            mem.writeBytes(slotAddr, u64(addr || 0));
        },
        /** Calls through the setter, and how many the guard matched - proves the cave runs. */
        counts() {
            const rd8 = (a) => {
                const b = mem.readBytes(a, 8);
                if (!b || b.length < 8) return -1;
                let v = 0;
                for (let i = 7; i >= 0; i--) v = v * 256 + b[i];
                return v;
            };
            return [rd8(slotCalls), rd8(slotHits)];
        },
        setMult(m) {
            mem.writeF32(slotMult, m);
        },
        restore() {
            mem.writeBytes(target, original);
            mem.free(cave);
        },
    };
}

/**
 * Compare bytes at `addr` against an expected pattern.
 * @param {number} addr
 * @param {(number | null)[]} expected null entries match any byte
 * @returns {boolean}
 */
export function bytesMatch(addr, expected) {
    const got = mem.readBytes(addr, expected.length);
    if (!got || got.length !== expected.length) return false;
    return expected.every((b, i) => b === null || b === got[i]);
}

/**
 * Replace an instruction instead of prepending to it.
 *
 * detour() runs your code and THEN the original instruction. Sometimes the
 * original must be gone -- turning `mov [rbx+1C8], eax` into a constant store,
 * for instance, where re-running the original would immediately overwrite you.
 *
 * @param {number} target
 * @param {number} stealLen bytes `code` substitutes for; >= 5, ending on an instruction boundary
 * @param {number[]} code
 * @returns {Detour | null}
 */
export function replace(target, stealLen, code) {
    if (!target || stealLen < 5) return null;

    const original = mem.readBytes(target, stealLen);
    if (!original || original.length !== stealLen) return null;

    const cave = mem.alloc(0x1000, target);
    if (!cave) return null;

    // cave: [your code][jmp back past the replaced instruction]
    if (!mem.writeBytes(cave, [...code, ...jmpAbs(target + stealLen)])) {
        mem.free(cave);
        return null;
    }

    const rel = cave - (target + 5);
    if (rel > 0x7ffffff0 || rel < -0x7ffffff0) {
        mem.free(cave);
        return null;
    }
    const patch = [0xe9, ...i32(rel)];
    while (patch.length < stealLen) patch.push(0x90);
    if (!mem.writeBytes(target, patch)) {
        mem.free(cave);
        return null;
    }

    return {
        target,
        cave,
        original,
        restore() {
            mem.writeBytes(target, original);
            mem.free(cave);
        },
    };
}

/**
 * `mov dword ptr [rbx+disp32], imm32` -- the C7 83 form.
 * @param {number} disp32
 * @param {number} imm32
 * @returns {number[]}
 */
export function movRbxImm(disp32, imm32) {
    return [0xc7, 0x83, ...i32(disp32), ...i32(imm32)];
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

/**
 * Plain byte patch with restore -- no cave, no jump.
 *
 * Most published cheat-table edits are this simple: overwrite an instruction
 * with a branch or a zeroing one of the same length. Use `replace()` only when
 * the substitute does not fit in the original's bytes.
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
 * Guarded "load max before the store".
 *
 * For shared float stores of the form `movss [reg+curOff], xmm0`, where the
 * same instruction serves every entity in the game. The cave compares the
 * destination address against a guarded one and only substitutes the max
 * value for that entity -- leaving everyone else untouched.
 *
 *   lea   r10, [rdi+curOff]
 *   cmp   r10, [slotAddr]
 *   jne   skip
 *   movss xmm0, [rdi+maxOff]
 *   skip: <stolen store>
 *   jmp   back
 *
 * Assumes RDI as the base register.
 *
 * @param {number} target
 * @param {number} stealLen
 * @param {number} curOff
 * @param {number} maxOff
 * @returns {(Detour & { setGuard(addr: number): void }) | null}
 */
export function guardedLoadMax(target, stealLen, curOff, maxOff) {
    if (!target || stealLen < 5) return null;
    const original = mem.readBytes(target, stealLen);
    if (!original || original.length !== stealLen) return null;

    const cave = mem.alloc(0x1000, target);
    if (!cave) return null;
    const slotAddr = cave + 0x40;

    // prettier-ignore
    const code = [
        0x4c, 0x8d, 0x57, curOff & 0xff,                   // lea r10, [rdi+curOff]
        0x4c, 0x3b, 0x15, ...i32(slotAddr - (cave + 11)),  // cmp r10, [slotAddr]
        0x75, 0x05,                                        // jne skip
        0xf3, 0x0f, 0x10, 0x47, maxOff & 0xff,             // movss xmm0, [rdi+maxOff]
        ...original,
        ...jmpAbs(target + stealLen),
    ];
    if (code.length > 0x40) {
        mem.free(cave);
        return null;
    }

    const rel = cave - (target + 5);
    if (rel > 0x7ffffff0 || rel < -0x7ffffff0) {
        mem.free(cave);
        return null;
    }

    mem.writeBytes(cave, code);
    mem.writeBytes(slotAddr, u64(0)); // guards nothing until set
    const patch = [0xe9, ...i32(rel)];
    while (patch.length < stealLen) patch.push(0x90);
    if (!mem.writeBytes(target, patch)) {
        mem.free(cave);
        return null;
    }

    return {
        target,
        cave,
        original,
        setGuard(addr) {
            mem.writeBytes(slotAddr, u64(addr || 0));
        },
        restore() {
            mem.writeBytes(target, original);
            mem.free(cave);
        },
    };
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
    if (!target || stealLen < 5) return null;
    const original = mem.readBytes(target, stealLen);
    if (!original || original.length !== stealLen) return null;

    const cave = mem.alloc(0x1000, target);
    if (!cave) return null;

    // prettier-ignore
    const copy = [
        0x50,                          // push rax
        0x8b, 0x41, maxOff & 0xff,     // mov eax, [rcx+maxOff]
        0x89, 0x41, curOff & 0xff,     // mov [rcx+curOff], eax
        0x58,                          // pop rax
    ];
    // prettier-ignore
    const code = [
        0x51,                                  // push rcx
        0x48, 0x8b, 0x49, flagPtrOff & 0xff,   // mov rcx, [rcx+flagPtrOff]
        0x83, 0xb9, ...i32(flagOff), 0x00,     // cmp dword [rcx+flagOff], 0
        0x59,                                  // pop rcx
        0x74, copy.length,                     // je skip
        ...copy,
        ...original,
        ...jmpAbs(target + stealLen),
    ];
    if (code.length > 0xe00) {
        mem.free(cave);
        return null;
    }

    const rel = cave - (target + 5);
    if (rel > 0x7ffffff0 || rel < -0x7ffffff0) {
        mem.free(cave);
        return null;
    }
    mem.writeBytes(cave, code);

    const patch = [0xe9, ...i32(rel)];
    while (patch.length < stealLen) patch.push(0x90);
    if (!mem.writeBytes(target, patch)) {
        mem.free(cave);
        return null;
    }

    return {
        target,
        cave,
        original,
        restore() {
            mem.writeBytes(target, original);
            mem.free(cave);
        },
    };
}

/**
 * IEEE-754 bits of a float, for encoding immediates.
 * @param {number} v
 * @returns {number}
 */
export function f32bits(v) {
    const b = new Uint8Array(4);
    new DataView(b.buffer).setFloat32(0, v, true);
    return b[0] | (b[1] << 8) | (b[2] << 16) | (b[3] << 24);
}

/**
 * Diagnostic companion to holdFieldAtSibling: records every entity pointer
 * the flag test accepts, instead of writing anything.
 *
 * Answers "is this flag actually unique to the player" with observation
 * rather than inference. The cave keeps a counter and a 32-slot ring:
 *
 *   cave+0xE00  qword      total matches
 *   cave+0xE08  qword[32]  the most recent rcx values
 *
 * @param {number} target
 * @param {number} stealLen
 * @param {number} flagPtrOff
 * @param {number} flagOff
 * @returns {(Detour & {
 *   slots: number,
 *   seen(): { count: number, entities: number[] },
 * }) | null}
 */
export function logFlaggedEntities(target, stealLen, flagPtrOff, flagOff) {
    if (!target || stealLen < 5) return null;
    const original = mem.readBytes(target, stealLen);
    if (!original || original.length !== stealLen) return null;

    const cave = mem.alloc(0x1000, target);
    if (!cave) return null;
    const slots = cave + 0xe00;

    // prettier-ignore
    const record = [
        0x50,                              // push rax
        0x52,                              // push rdx
        0x48, 0xb8, ...u64(slots),         // mov rax, slots
        0x48, 0x8b, 0x10,                  // mov rdx, [rax]      current count
        0x48, 0xff, 0x00,                  // inc qword [rax]
        0x48, 0x83, 0xe2, 0x1f,            // and rdx, 31         ring slot
        0x48, 0x89, 0x4c, 0xd0, 0x08,      // mov [rax+rdx*8+8], rcx
        0x5a,                              // pop rdx
        0x58,                              // pop rax
    ];
    // prettier-ignore
    const code = [
        0x51,                                  // push rcx
        0x48, 0x8b, 0x49, flagPtrOff & 0xff,   // mov rcx, [rcx+flagPtrOff]
        0x83, 0xb9, ...i32(flagOff), 0x00,     // cmp dword [rcx+flagOff], 0
        0x59,                                  // pop rcx
        0x74, record.length,                   // je skip
        ...record,
        ...original,
        ...jmpAbs(target + stealLen),
    ];
    if (code.length > 0xe00) {
        mem.free(cave);
        return null;
    }

    const rel = cave - (target + 5);
    if (rel > 0x7ffffff0 || rel < -0x7ffffff0) {
        mem.free(cave);
        return null;
    }
    mem.writeBytes(cave, code);
    mem.writeBytes(slots, u64(0));

    const patch = [0xe9, ...i32(rel)];
    while (patch.length < stealLen) patch.push(0x90);
    if (!mem.writeBytes(target, patch)) {
        mem.free(cave);
        return null;
    }

    return {
        target,
        cave,
        slots,
        original,
        seen() {
            const n = mem.u64(slots);
            const out = [];
            for (let i = 0; i < 32; i++) {
                const p = mem.u64(slots + 8 + i * 8);
                if (p && !out.includes(p)) out.push(p);
            }
            return { count: n, entities: out };
        },
        restore() {
            mem.writeBytes(target, original);
            mem.free(cave);
        },
    };
}
