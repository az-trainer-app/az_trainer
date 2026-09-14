// A stand-in for the trainer's `mem` host, so scripts can be tested without a
// game.
//
// Memory is a sparse byte map. Bytes never written read as a fixed pattern,
// which makes a hook's stolen bytes predictable. Every write, allocation and
// free is logged in order, so a test can compare exactly what a script did.

/** Where the fake main module starts. Matches a typical x64 image base. */
export const BASE = 0x140000000;
export const SIZE = 0x4000000;

/**
 * @param {object} [opts]
 * @param {(sig: string) => number} [opts.aob] address for a signature; default: not found
 * @param {(sig: string) => number[]} [opts.aobAll]
 * @param {(size: number, near: number) => number} [opts.alloc] where a cave
 *   lands; default: within rel32 range. Return something far away to exercise
 *   the absolute-jump fallback, or 0 to refuse.
 */
export function createMem(opts = {}) {
    /** @type {Map<number, number>} */
    const bytes = new Map();
    /** @type {{ addr: number, bytes: number[] }[]} */
    const writes = [];
    /** @type {{ cave: number, size: number, near: number }[]} */
    const allocs = [];
    /** @type {number[]} */
    const frees = [];
    /** @type {{ addr: number, mode: string, value: number }[]} */
    let holds = [];
    let nextCave = BASE + 0x10000000; // in rel32 range of the whole module

    const pattern = (a) => (a * 7 + 3) & 0xff;
    const readByte = (a) => (bytes.has(a) ? bytes.get(a) : pattern(a));
    const read = (a, n) => Array.from({ length: n }, (_, i) => readByte(a + i));
    const view = (arr) => new DataView(Uint8Array.from(arr).buffer);
    const store = (a, data) => data.forEach((b, i) => bytes.set(a + i, b & 0xff));

    const mem = {
        u64(a) {
            const b = read(a, 8);
            let v = 0;
            for (let i = 7; i >= 0; i--) v = v * 256 + b[i];
            return v;
        },
        i32: (a) => view(read(a, 4)).getInt32(0, true),
        f32: (a) => view(read(a, 4)).getFloat32(0, true),
        writeF32(a, v) {
            const b = new Uint8Array(4);
            new DataView(b.buffer).setFloat32(0, v, true);
            store(a, [...b]);
            writes.push({ addr: a, bytes: [...b] });
            return true;
        },
        readBytes: (a, n) => read(a, n),
        writeBytes(a, data) {
            store(a, data);
            writes.push({ addr: a, bytes: [...data] });
            return true;
        },
        aob: (sig) => (opts.aob ? opts.aob(sig) : 0),
        aobAll: (sig, limit) => (opts.aobAll ? opts.aobAll(sig) : []).slice(0, limit),
        rip: (hit, pos, len) => hit + len + mem.i32(hit + pos),
        moduleBase: () => BASE,
        moduleSize: () => SIZE,
        alloc(size, near) {
            let cave;
            if (opts.alloc) {
                cave = opts.alloc(size, near);
            } else {
                cave = nextCave;
                nextCave += 0x10000;
            }
            if (cave) allocs.push({ cave, size, near });
            return cave;
        },
        free(a) {
            frees.push(a);
            return true;
        },
        protect: () => 0x40,
        hold(addr, value) {
            holds = [{ addr, mode: 'set', value }];
        },
        holdScale(addr, value) {
            holds = [{ addr, mode: 'scale', value }];
        },
        clearHolds() {
            holds = [];
        },
        findAccessors: () => [],
    };

    return {
        mem,
        writes,
        allocs,
        frees,
        /** Set bytes without logging a write - for arranging a test. */
        poke: store,
        /** What an address held before anything wrote to it. */
        original: (a, n) => Array.from({ length: n }, (_, i) => pattern(a + i)),
        read,
        holds: () => holds,
        /** True if `a` lies inside a cave handed out by `alloc`. */
        inCave: (a) => allocs.some((c) => a >= c.cave && a < c.cave + c.size),
    };
}

/**
 * Install a fake as the global `mem` and silence `log`.
 * @param {ReturnType<typeof createMem>} fake
 * @returns {string[]} the lines scripts logged
 */
export function install(fake, game = { exe: 'Game.exe', timestamp: 0, size: SIZE, steamBuild: undefined, build: null }) {
    const lines = [];
    globalThis.mem = fake.mem;
    globalThis.game = game;
    globalThis.log = (msg) => lines.push(msg);
    return lines;
}
