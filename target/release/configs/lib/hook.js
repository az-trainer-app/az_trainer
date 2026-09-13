// x64 code injection helpers.
//
// Host primitives used here:
//   mem.alloc(size, near)    RWX page, placed within +/-2GB of `near` when
//                            possible so a 5-byte E9 jump can reach it
//   mem.free(addr)
//   mem.readBytes(addr, n) -> array
//   mem.writeBytes(addr, array)
//   mem.protect(addr, size)
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

/** Little-endian 4-byte two's complement. */
export function i32(v) {
    const n = v < 0 ? v + 0x100000000 : v;
    return [n & 0xff, (n >>> 8) & 0xff, (n >>> 16) & 0xff, (n >>> 24) & 0xff];
}

/** Little-endian 8-byte address. */
export function u64(v) {
    const out = [];
    let hi = Math.floor(v / 0x100000000);
    let lo = v >>> 0;
    for (let i = 0; i < 4; i++) { out.push(lo & 0xff); lo >>>= 8; }
    for (let i = 0; i < 4; i++) { out.push(hi & 0xff); hi >>>= 8; }
    return out;
}

/** jmp [rip+0]; <absolute 64-bit target> -- 14 bytes, reaches anywhere. */
export function jmpAbs(target) {
    return [0xff, 0x25, 0x00, 0x00, 0x00, 0x00, ...u64(target)];
}

/** Overwrite `len` bytes with NOP. Returns the original bytes. */
export function nopOut(addr, len) {
    const original = mem.readBytes(addr, len);
    mem.writeBytes(addr, new Array(len).fill(0x90));
    return original;
}

/**
 * Install a detour at `target`.
 *
 *   stealLen  bytes to overwrite; must be >= 5 and end on an instruction
 *             boundary
 *   code      your instruction bytes, run before the stolen ones
 *
 * Returns a handle with .restore(), or null if it could not be installed.
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
 * needed. `build(cave)` returns the code bytes; it is called at install time.
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
 * Cave layout:
 *   +0x00  mov r10, [rcx+guardOffset]     (r10 is volatile; safe at entry)
 *   +0x04  cmp r10, [rip+slotAddr]
 *   +0x0B  jne skip
 *   +0x0D  mulss xmm1, [rip+slotMult]
 *   +0x15  <stolen bytes>                 (skip lands here)
 *   +...   jmp back
 *   +0x40  slotAddr (qword)
 *   +0x48  slotMult (float)
 *
 * Returns a handle, or null if it could not be installed.
 */
export function guardedScale(target, stealLen, guardOffset) {
    if (!target || stealLen < 5) return null;

    const original = mem.readBytes(target, stealLen);
    if (!original || original.length !== stealLen) return null;

    const cave = mem.alloc(0x1000, target);
    if (!cave) return null;

    const slotAddr = cave + 0x40;
    const slotMult = cave + 0x48;
    const slotCalls = cave + 0x50;   // times the setter ran at all
    const slotHits = cave + 0x58;    // times the guard matched (we scaled)

    // Layout (offsets within the cave):
    //   0x00 mov   r10,[rcx+G]
    //   0x04 inc   qword [calls]        <- every call through the setter
    //   0x0B cmp   r10,[slotAddr]
    //   0x12 jne   skip -> 0x23
    //   0x14 inc   qword [hits]         <- calls where the guard matched
    //   0x1B mulss xmm1,[slotMult]
    //   0x23 <stolen>                   (skip lands here)
    //   0x28 jmp   back
    const code = [
        0x4c, 0x8b, 0x51, guardOffset & 0xff,             // mov r10,[rcx+G]
        0x48, 0xff, 0x05, ...i32(slotCalls - (cave + 0x0b)),
        0x4c, 0x3b, 0x15, ...i32(slotAddr - (cave + 0x12)),
        0x75, 0x0f,                                        // jne +0x0f
        0x48, 0xff, 0x05, ...i32(slotHits - (cave + 0x1b)),
        0xf3, 0x0f, 0x59, 0x0d, ...i32(slotMult - (cave + 0x23)),
        ...original,
        ...jmpAbs(target + stealLen),
    ];
    if (code.length > 0x40) { mem.free(cave); return null; }

    const rel = cave - (target + 5);
    if (rel > 0x7ffffff0 || rel < -0x7ffffff0) { mem.free(cave); return null; }

    mem.writeBytes(cave, code);
    mem.writeBytes(slotAddr, u64(0));   // no guard yet: scales nothing
    mem.writeF32(slotMult, 1.0);
    mem.writeBytes(slotCalls, u64(0));
    mem.writeBytes(slotHits, u64(0));

    const patch = [0xe9, ...i32(rel)];
    while (patch.length < stealLen) patch.push(0x90);
    if (!mem.writeBytes(target, patch)) { mem.free(cave); return null; }

    return {
        target, cave, original,
        setGuard(addr) { mem.writeBytes(slotAddr, u64(addr || 0)); },
        /** [callsThroughSetter, timesGuardMatched] - proves whether the cave runs. */
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
        setMult(m) { mem.writeF32(slotMult, m); },
        restore() {
            mem.writeBytes(target, original);
            mem.free(cave);
        },
    };
}

/** Compare bytes at `addr` against an expected pattern (null = wildcard). */
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
 * `code` fully substitutes for the `stealLen` bytes at `target`, which must be
 * >= 5 and end on an instruction boundary.
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

/** mov dword ptr [rbx+disp32], imm32  -- the C7 83 form. */
export function movRbxImm(disp32, imm32) {
    return [0xc7, 0x83, ...i32(disp32), ...i32(imm32)];
}

/**
 * Plain byte patch with restore -- no cave, no jump.
 *
 * Most published cheat-table edits are this simple: overwrite an instruction
 * with a branch or a zeroing one of the same length. Use `replace()` only when
 * the substitute does not fit in the original's bytes.
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
 * Assumes RDI as the base register, which is what the Veilguard store uses.
 */
export function guardedLoadMax(target, stealLen, curOff, maxOff) {
    if (!target || stealLen < 5) return null;
    const original = mem.readBytes(target, stealLen);
    if (!original || original.length !== stealLen) return null;

    const cave = mem.alloc(0x1000, target);
    if (!cave) return null;
    const slotAddr = cave + 0x40;

    const code = [
        0x4c, 0x8d, 0x57, curOff & 0xff,              // lea r10, [rdi+curOff]
        0x4c, 0x3b, 0x15, ...i32(slotAddr - (cave + 11)),
        0x75, 0x05,                                    // jne +5 (skip the load)
        0xf3, 0x0f, 0x10, 0x47, maxOff & 0xff,        // movss xmm0, [rdi+maxOff]
        ...original,
        ...jmpAbs(target + stealLen),
    ];
    if (code.length > 0x40) { mem.free(cave); return null; }

    const rel = cave - (target + 5);
    if (rel > 0x7ffffff0 || rel < -0x7ffffff0) { mem.free(cave); return null; }

    mem.writeBytes(cave, code);
    mem.writeBytes(slotAddr, u64(0));      // guards nothing until set
    const patch = [0xe9, ...i32(rel)];
    while (patch.length < stealLen) patch.push(0x90);
    if (!mem.writeBytes(target, patch)) { mem.free(cave); return null; }

    return {
        target, cave, original,
        setGuard(addr) { mem.writeBytes(slotAddr, u64(addr || 0)); },
        restore() { mem.writeBytes(target, original); mem.free(cave); },
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
 */
export function holdFieldAtSibling(target, stealLen, flagPtrOff, flagOff,
                                   curOff, maxOff) {
    if (!target || stealLen < 5) return null;
    const original = mem.readBytes(target, stealLen);
    if (!original || original.length !== stealLen) return null;

    const cave = mem.alloc(0x1000, target);
    if (!cave) return null;

    const copy = [
        0x50,                             // push rax
        0x8B, 0x41, maxOff & 0xff,        // mov eax, [rcx+maxOff]
        0x89, 0x41, curOff & 0xff,        // mov [rcx+curOff], eax
        0x58,                             // pop rax
    ];
    const code = [
        0x51,                             // push rcx
        0x48, 0x8B, 0x49, flagPtrOff & 0xff,
        0x83, 0xB9, ...i32(flagOff), 0x00,
        0x59,                             // pop rcx
        0x74, copy.length,                // je skip
        ...copy,
        ...original,
        ...jmpAbs(target + stealLen),
    ];
    if (code.length > 0xE00) { mem.free(cave); return null; }

    const rel = cave - (target + 5);
    if (rel > 0x7ffffff0 || rel < -0x7ffffff0) { mem.free(cave); return null; }
    mem.writeBytes(cave, code);

    const patch = [0xE9, ...i32(rel)];
    while (patch.length < stealLen) patch.push(0x90);
    if (!mem.writeBytes(target, patch)) { mem.free(cave); return null; }

    return {
        target, cave, original,
        restore() { mem.writeBytes(target, original); mem.free(cave); },
    };
}

/** IEEE-754 bits of a float, for encoding immediates. */
export function f32bits(v) {
    const b = new Uint8Array(4);
    new DataView(b.buffer).setFloat32(0, v, true);
    return b[0] | (b[1] << 8) | (b[2] << 16) | (b[3] << 24);
}

/**
 * Diagnostic twin of forceFieldWhenFlagged: records every entity pointer the
 * flag test accepts, instead of writing anything.
 *
 * Answers "is this flag actually unique to the player" with observation
 * rather than inference. The cave keeps a counter and a 32-slot ring:
 *
 *   cave+0xE00  qword  total matches
 *   cave+0xE08  qword[32]  the most recent rcx values
 *
 *   push  rcx
 *   mov   rcx, [rcx+flagPtrOff]
 *   cmp   dword [rcx+flagOff], 0
 *   pop   rcx
 *   je    skip
 *   push  rax / push rdx
 *   mov   rax, <cave+0xE00>
 *   mov   rdx, [rax]          ; current count
 *   inc   qword [rax]
 *   and   rdx, 31             ; ring slot
 *   mov   [rax+rdx*8+8], rcx
 *   pop   rdx / pop rax
 *   skip: <stolen bytes>
 *   jmp   back
 */
export function logFlaggedEntities(target, stealLen, flagPtrOff, flagOff) {
    if (!target || stealLen < 5) return null;
    const original = mem.readBytes(target, stealLen);
    if (!original || original.length !== stealLen) return null;

    const cave = mem.alloc(0x1000, target);
    if (!cave) return null;
    const slots = cave + 0xE00;

    const record = [
        0x50,                                     // push rax
        0x52,                                     // push rdx
        0x48, 0xB8, ...u64(slots),                // mov rax, slots
        0x48, 0x8B, 0x10,                         // mov rdx, [rax]
        0x48, 0xFF, 0x00,                         // inc qword [rax]
        0x48, 0x83, 0xE2, 0x1F,                   // and rdx, 31
        0x48, 0x89, 0x4C, 0xD0, 0x08,             // mov [rax+rdx*8+8], rcx
        0x5A,                                     // pop rdx
        0x58,                                     // pop rax
    ];
    const code = [
        0x51,                                     // push rcx
        0x48, 0x8B, 0x49, flagPtrOff & 0xff,      // mov rcx, [rcx+off]
        0x83, 0xB9, ...i32(flagOff), 0x00,        // cmp dword [rcx+off], 0
        0x59,                                     // pop rcx
        0x74, record.length,                      // je skip
        ...record,
        ...original,
        ...jmpAbs(target + stealLen),
    ];
    if (code.length > 0xE00) { mem.free(cave); return null; }

    const rel = cave - (target + 5);
    if (rel > 0x7ffffff0 || rel < -0x7ffffff0) { mem.free(cave); return null; }
    mem.writeBytes(cave, code);
    mem.writeBytes(slots, u64(0));

    const patch = [0xE9, ...i32(rel)];
    while (patch.length < stealLen) patch.push(0x90);
    if (!mem.writeBytes(target, patch)) { mem.free(cave); return null; }

    return {
        target, cave, slots, original,
        /** Total matches, and the distinct pointers still in the ring. */
        seen() {
            const n = mem.u64(slots);
            const out = [];
            for (let i = 0; i < 32; i++) {
                const p = mem.u64(slots + 8 + i * 8);
                if (p && !out.includes(p)) out.push(p);
            }
            return { count: n, entities: out };
        },
        restore() { mem.writeBytes(target, original); mem.free(cave); },
    };
}
