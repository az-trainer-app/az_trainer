# Developing AZ Trainer

The app is Rust with [GPUI](https://www.gpui.rs/) and an embedded [QuickJS](https://bellard.org/quickjs/)
engine. Each game is an ES module in `configs/games/`; saving one hot-reloads it.

## How it works

```
┌─────────────── az_trainer.exe ───────────────┐
│ UI thread (GPUI)      toggles, art, resizing │
│ Engine thread         finds the game, loads  │──── Read/WriteProcessMemory ───► game.exe
│                       its script, ticks 10Hz │
│ QuickJS               configs/games/*.js     │
│ Hold thread           ~1kHz value writer     │
│ Update thread         GitHub, every 6h       │
└──────────────────────────────────────────────┘
```

A script exports `process`, `title` and `options`. Every 100 ms each option's `tick({ on, mult })` runs.

## Writing a script

```js
import * as HOOK from '../lib/hook.js';
import * as OPT from '../lib/option.js';
import * as SCAN from '../lib/scan.js';

export const process = 'Game.exe';
export const title = 'Some Game';

// sub [rcx+1C8], r8d
const POINTS_SIG = '44 29 81 C8 01 00 00';

/** @type {import('../types/trainer').Entry[]} */
export const options = [
    { separator: 'Progression' },
    {
        name: 'Points Never Decrease',
        tick({ on }) {
            OPT.patchWhileOn('points', on, () => SCAN.once(POINTS_SIG), HOOK.nops(7));
        },
    },
];
```

Put it in `configs/games/` with `somegame.jpg` or `somegame.png` beside it for artwork.
`cargo run --bin artwork -- "Some Game" somegame` fetches that artwork from Steam:
it takes an app id, a store URL or a name, saves the library hero as
`configs/games/somegame.jpg` scaled to 1280 wide, and prints the match it used.
`{ separator: 'Heading' }` draws a titled divider; `{ separator: true }` a plain line.

### Host API

| `mem.*` | Purpose |
| --- | --- |
| `aob(sig)`, `aobAll(sig, limit)` | Find code by signature (`??` and `4?` wildcards) |
| `u64`, `i32`, `f32`, `writeF32`, `readBytes`, `writeBytes` | Read and write memory |
| `alloc(size, near)`, `free`, `protect` | Code caves within ±2 GB |
| `hold(addr, value)`, `holdScale(addr, mult)`, `clearHolds()` | ~1 kHz float writer; one address at a time |
| `moduleBase()`, `moduleSize()`, `rip(hit, pos, len)` | Module bounds; RIP-relative operands |
| `findAccessors(addr, size, access, ms)` | Research: instructions touching an address |
| `scanStart(lo, hi, type)`, `scanNext(mode, a, b)`, `scanResults(n)` | Research: find a value by scanning, then narrowing as it changes (floats, ints, or both) |
| `findPointers(lo, hi, limit)` | Research: every slot pointing into a range |

### Libraries

| Library | Contents |
| --- | --- |
| `lib/hook.js` | Patches, code caves, detours, flag-guarded writes, instruction encoders |
| `lib/scan.js` | Cached signature scans: `once`, `onceWhere`, `ripF32` |
| `lib/option.js` | `whileOn`, `whileFound`, `patchWhileOn` |
| `lib/unreal.js` | UE5 pawn walks, pointer chains, attribute holds |
| `lib/research.js` | Devtools only: `staticPaths` (reverse pointer search), `resolve`, `capture` / `changes` |

### Finding values

With a devtools build (`cargo build --release --features devtools`) attached to
the game, research runs through the eval channel in the config's own context:

```js
mem.scanStart(1, 100000, 'any');   // everything that could be health
// take damage
mem.scanNext('decreased');
// heal, or wait
mem.scanNext('increased');
mem.scanResults(20);               // [addr, value, isInt, ...]
```

`scanNext('equal', 380)` and `scanNext('between', 300, 400)` narrow on a known
reading. Heap addresses change on restart; to reach one from a fixed place,
search backwards from it:

```js
research.staticPaths(0x1ae1be66e0, { depth: 2 }); // [{ rva, offsets }]: [[module+rva]+o0]+o1
```

For a value that is hard to search for directly, capture the memory around
something related, change the value in game, and compare:
`research.changes(snap, (c) => c.nowFloat < c.wasFloat)` after
`snap = research.capture(addrs)`. In a devtools build `research` is
`lib/research.js`, already loaded.

A devtools build can also see and drive the game, for research that needs it
in a particular state: `dev.capture(maxWidth)` saves the game window as a PNG
and returns its path, `dev.press('Shift+W', ms)` holds keys, `dev.down` /
`dev.up` press and release separately, `dev.mouse(dx, dy)` and
`dev.click(button)` move and click, and `dev.focus()` / `dev.sleep(ms)` bring
the game forward and wait. None of it exists in a released build.

### Several builds of a game

A config that finds everything by signature usually survives patches as is.
One that relies on fixed addresses lists the builds it knows, each carrying
whatever differs in it, and shares everything else:

```js
export const process = ['Game.exe', 'Game-WinGDK-Shipping.exe'];

export const builds = [
    { name: 'Steam 24769601', match: { steamBuild: 24769601 }, player: 0xcbba6d8 },
    { name: 'Game Pass 1.0.2', match: { exe: 'Game-WinGDK-Shipping.exe', timestamp: 0x68a1b2c3 }, player: 0xcbbf560 },
];

function player() {
    return mem.u64(mem.moduleBase() + game.build.player);
}
```

On attach, the first entry whose `match` fits becomes `game.build`. `match` can
test `exe`, `timestamp` and `size` (from the executable's PE header) and
`steamBuild` (from the Steam app manifest); every field given must agree. An
entry without `match` fits anything, as a last fallback.

When nothing fits, the trainer does not attach: the status reads *unsupported
game version* and names the build, and the console gets the full identity
(`[engine] Game.exe is Steam build 24769601, timestamp 0x68a1b2c3, size 0xd2e1000`)
to copy into a new entry. The build that did match is named in the status bar.

`game` also holds `exe`, `timestamp`, `size` and `steamBuild` for the running
game. It is set on attach, so read it from `tick` or `live`, not at the top level.

### Two columns

`export const columns = 2` widens the window and lays the options out in two
columns. The split keeps the columns even, breaking before a separator where it
can so each group stays together.

### Porting a Cheat Engine table

| Cheat Engine | AZ Trainer |
| --- | --- |
| `aobscanmodule(site, Game.exe, 44 29 81 ...)` | `SCAN.once('44 29 81 ...')` |
| `[ENABLE]` / `[DISABLE]` | `OPT.whileOn(key, on, build)` |
| `site: db 90 90 90` | `OPT.patchWhileOn(key, on, find, HOOK.nops(3))` |
| `alloc` + `jmp newmem` + `return:` | `HOOK.detour(site, stealLen, code)` |
| `mov r15d, #99999` | `HOOK.movR32Imm('r15d', 99999)` |
| `[[[base+368]+3C0]+2F8]` | `UE.chain(base, [0x368, 0x3c0, 0x2f8])` |
| `readFloat` / `writeFloat` | `mem.f32` / `mem.writeF32` |
| Frozen value | `mem.hold(addr, value)` |

### Editor support

Open `configs/` in VS Code for completion and type checking (`configs/types/`). CLI: `npm run typecheck`.

## Building

Rust (stable) on Windows; Node.js 22+ for script tooling.

```bash
cargo build --release
```

`target/release/az_trainer.exe` reads the repository's `configs/`.
`--preview configs/games/<game>.js` opens a config without the game.

Helpers: `cfgcheck <config.js>`, `artwork <game> <config>`, `finder_test`, `detour_test`.

## Tests

```bash
npm install
npm test
npm run typecheck
cargo test --release
```

Golden hook bytes, library tests, and app tests (including loading every config in QuickJS).
CI runs the script checks when scripts or tests change, and the app tests only when Rust code
changes - run `cargo test` yourself after editing a config. Game scripts themselves are tested in-game.

## Updates

The app updates from GitHub Releases (SHA-256 verified); scripts sync from `main`, skipping files edited
locally. Skipped in development builds and git checkouts (override: `AZ_TRAINER_FORCE_UPDATE=1`).

## Releasing

Set `version` in `Cargo.toml`, commit, then:

```bash
git tag v1.1.0
git push origin v1.1.0
```

The tag must match `Cargo.toml`.

A release is just `az_trainer.exe` and its `.sha256`. Game scripts and libraries are not bundled:
the app downloads them from `main` into `configs\` beside the exe on first start and keeps them
in sync, so script changes need no release.
