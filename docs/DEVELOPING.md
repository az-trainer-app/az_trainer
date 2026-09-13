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

Put it in `configs/games/` with `somegame.jpg` beside it for artwork.
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

### Libraries

| Library | Contents |
| --- | --- |
| `lib/hook.js` | Patches, detours, replacements, flag-guarded writes, instruction encoders |
| `lib/scan.js` | Cached signature scans: `once`, `onceWhere`, `ripF32` |
| `lib/option.js` | `whileOn`, `patchWhileOn`, `writeOnce` |
| `lib/unreal.js` | UE5 pawn walks, pointer chains, attribute holds |

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

Helpers: `cfgcheck <config.js>`, `finder_test`, `detour_test`.

## Tests

```bash
npm install
npm test
npm run typecheck
npm run format:check
cargo test --release
```

Golden hook bytes, library tests, and app tests (including loading every config in QuickJS).
All run on every pull request.

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
