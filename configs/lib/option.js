// What an option's tick usually does: apply something while the option is
// on, undo it when it goes off.
//
// State is kept per attached process, so a reattach - or a test's fresh fake
// host - starts clean. Keys only need to be unique within one config.

import * as HOOK from './hook.js';

/** @typedef {import('./hook.js').Restorable} Restorable */

/** @type {WeakMap<object, Map<string, { handle: Restorable | null, failed: boolean }>>} */
const installed = new WeakMap();

/**
 * Keep something applied while `on`, and restore it when `on` goes false.
 *
 * `build` runs when the option turns on and returns a handle with
 * `restore()` - anything from lib/hook.js - or null if it could not be
 * applied. A failure is not retried until the option is switched off and on
 * again, so a missing site costs one attempt rather than one per tick.
 *
 * @param {string} key
 * @param {boolean} on
 * @param {() => Restorable | null} build
 * @returns {boolean} whether it is applied now
 */
export function whileOn(key, on, build) {
    let all = installed.get(mem);
    if (!all) installed.set(mem, (all = new Map()));
    let s = all.get(key);
    if (!s) all.set(key, (s = { handle: null, failed: false }));

    if (!on) {
        if (s.handle) {
            s.handle.restore();
            s.handle = null;
        }
        s.failed = false;
        return false;
    }
    if (!s.handle && !s.failed) {
        s.handle = build();
        if (!s.handle) {
            s.failed = true;
            log(`${key}: failed to apply`);
        } else {
            log(`${key}: on`);
        }
    }
    return !!s.handle;
}

/**
 * whileOn() for the usual case: `build(addr)` at the address `find` returns.
 *
 * @param {string} key
 * @param {boolean} on
 * @param {() => number} find the address, or 0 when it is not there
 * @param {(addr: number) => Restorable | null} build
 * @returns {boolean}
 */
export function whileFound(key, on, find, build) {
    return whileOn(key, on, () => {
        const at = find();
        return at ? build(at) : null;
    });
}

/**
 * Byte-patch an address while `on`.
 *
 * @param {string} key
 * @param {boolean} on
 * @param {() => number} find returns the address to patch, or 0
 * @param {number[]} bytes
 * @returns {boolean}
 */
export function patchWhileOn(key, on, find, bytes) {
    return whileFound(key, on, find, (at) => HOOK.patch(at, bytes));
}
