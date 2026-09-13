// Finding code, once per game.
//
// A signature scan walks the whole main module, so running one every tick is
// far too slow. These cache each result per attached process: a reattach -
// or a test's fresh fake host - scans again.

/** @type {WeakMap<object, Map<string, number>>} */
const hits = new WeakMap();

/** This process's cache. */
function cache() {
    let m = hits.get(mem);
    if (!m) hits.set(mem, (m = new Map()));
    return m;
}

/**
 * First match of a signature in the main module, scanned once.
 *
 * @param {string} sig hex bytes, `??` or `4?` wildcards
 * @returns {number} 0 if not found
 */
export function once(sig) {
    const c = cache();
    let at = c.get(sig);
    if (at === undefined) {
        at = mem.aob(sig);
        if (!at) log(`signature not found: ${sig}`);
        c.set(sig, at);
    }
    return at;
}

/**
 * First match of a signature that also passes `test`, scanned once.
 *
 * For sites that share their bytes with other code and differ only in an
 * operand - fifteen `mulss xmm0, [rip+X]` sites, of which one multiplies by
 * -1.0, say.
 *
 * @param {string} sig
 * @param {(addr: number) => boolean} test
 * @param {number} [limit] how many candidates to consider
 * @returns {number} 0 if no candidate passes
 */
export function onceWhere(sig, test, limit = 64) {
    const c = cache();
    const key = `${sig}\n${test}`;
    let at = c.get(key);
    if (at === undefined) {
        at = mem.aobAll(sig, limit).find(test) ?? 0;
        if (!at) log(`no candidate passed the test: ${sig}`);
        c.set(key, at);
    }
    return at;
}

/**
 * The float a RIP-relative operand points at.
 *
 * @example
 * // F3 0F 59 05 <rel32>   mulss xmm0, [rip+rel32]   (8 bytes, operand at +4)
 * ripF32(site, 4, 8) === -1.0
 *
 * @param {number} hit start of the instruction
 * @param {number} operandOffset where the rel32 sits within it
 * @param {number} instructionLength
 * @returns {number}
 */
export function ripF32(hit, operandOffset, instructionLength) {
    return mem.f32(mem.rip(hit, operandOffset, instructionLength));
}
