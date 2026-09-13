# AZ Trainer

A small, fast game trainer for Windows. One native app, one JavaScript file per game.

The app is written in Rust with [GPUI](https://www.gpui.rs/) for the interface and an embedded
[QuickJS](https://bellard.org/quickjs/) engine for trainer scripts. It notices which supported game
is running, loads that game's script, and shows its options as toggles. Scripts are plain ES modules,
so a new game is a new `.js` file — no rebuild.

| The Blood of Dawnwalker | Dragon Age: The Veilguard |
| :---: | :---: |
| ![AZ Trainer attached to The Blood of Dawnwalker](docs/dawnwalker.png) | ![AZ Trainer attached to Dragon Age: The Veilguard](docs/veilguard.png) |

## Features

- **Auto-detects the game.** Start the trainer before or after the game; it attaches when a supported
  process appears and goes back to waiting when the game closes.
- **Scripts in JavaScript.** Each game is one ES module built on shared libraries: code injection,
  signature scanning, option lifecycles, and Unreal Engine object walks.
- **Hot reload.** Save a script and the trainer reloads it within a second, keeping which options were on.
- **Remembers settings per game.** Toggles and levels (2x / 4x / 8x) are restored the next time that
  game is detected. Nothing is ever enabled unless you enabled it.
- **Grouped options.** Scripts can divide their options with titled or plain separators.
- **Signature-based.** Code is located by byte pattern, not fixed address, so scripts survive most
  game patches.
- **Clean exit.** Closing the window restores every patched instruction in the game.
- **Updates itself.** New app versions come from GitHub Releases; new and changed trainer scripts come
  from this repository. See [Updates](#updates).
- **Editor support.** Type definitions for the whole script API give completion and type checking in
  VS Code and other TypeScript-aware editors.

## Supported games

| Game | Engine | Options |
| --- | --- | --- |
| The Blood of Dawnwalker | Unreal Engine 5 | Infinite Human Health, Infinite Vampiric Blood, Infinite Stamina, Speed (2x/4x/8x), Denarius (set 1k/10k/100k) |
| Dragon Age: The Veilguard | Frostbite | Infinite Health (player and party), Unlimited Mana, No Cooldowns, Unlimited Potions, Ability Points Never Decrease, Bonus XP (+5000), Gold 99,999 |

Single-player games only. Trainers write to another process's memory, which anti-cheat systems treat
as cheating regardless of intent — do not use this with online or competitive games.

## Using it

1. Download `az_trainer.zip` from the [latest release](../../releases/latest) and extract it anywhere.
   Keep `az_trainer.exe` and the `configs` folder together.
2. Run `az_trainer.exe`, then start the game (either order works).
3. Load into the game world and click the options you want.
4. Close the trainer with the window's **X** so it can restore the game's code. Killing the process
   from Task Manager skips that step and leaves patches in place until the game restarts.

If attaching fails with "cannot open process", run the trainer as Administrator.

## How it works

```
┌─────────────── az_trainer.exe ───────────────┐
│ UI thread (GPUI)      toggles, art, resizing │
│        │  shared state                       │
│ Engine thread         finds the game, loads  │──── ReadProcessMemory / WriteProcessMemory ───► game.exe
│        │              its script, ticks 10Hz │
│ QuickJS               configs/games/*.js     │
│ Hold thread           ~1kHz value writer     │
│ Update thread         GitHub, every 6h       │
└──────────────────────────────────────────────┘
```

A script exports the process name, a title, and a list of options. Every 100 ms the engine calls each
option's `tick({ on, mult })`, and the option applies or removes its effect.

### Host API

The trainer gives every script a `mem` global (and `log`):

| `mem.*` | Purpose |
| --- | --- |
| `aob(sig)`, `aobAll(sig, limit)` | Find code by byte signature (`??` and per-nibble `4?` wildcards) |
| `u64`, `i32`, `f32`, `writeF32`, `readBytes`, `writeBytes` | Read and write game memory |
| `alloc(size, near)`, `free`, `protect` | Allocate code caves within ±2 GB of a hook site |
| `hold(addr, value)`, `holdScale(addr, mult)`, `clearHolds()` | Hand a float to the ~1 kHz writer, for fields the game rewrites every frame. One address at a time: a new hold replaces the last. |
| `moduleBase()`, `moduleSize()`, `rip(hit, pos, len)` | Module bounds; resolve the RIP-relative operand at `hit + pos` of a `len`-byte instruction |
| `findAccessors(addr, size, access, ms)` | Research only: report the instructions that touch an address, via a hardware breakpoint |

### Script libraries

| Library | What it gives you |
| --- | --- |
| `lib/hook.js` | Byte patches, detours with stolen bytes, instruction replacement, flag-guarded writes, and encoders (`movR32Imm`, `addR32Imm`, `xorps`, `jmpShort`, `nops`). Every hook returns a handle whose `restore()` puts the original bytes back. |
| `lib/scan.js` | Signature scans cached per process — `once(sig)`, `onceWhere(sig, test)` for sites that differ only in an operand, `ripF32` to read what an operand points at. |
| `lib/option.js` | Option lifecycles — `whileOn` keeps a hook installed only while its option is on, `patchWhileOn` does the same for a byte patch, `writeOnce` sets a value when asked and then leaves it alone. |
| `lib/unreal.js` | Unreal Engine 5: GWorld → player controller → pawn walks, pointer chains, and `FGameplayAttributeData` reads and holds. |

### A minimal script

```js
import * as HOOK from '../lib/hook.js';
import * as OPT from '../lib/option.js';
import * as SCAN from '../lib/scan.js';

export const process = 'Game.exe';
export const title = 'Some Game';

// sub [rcx+1C8], r8d - the decrement we want gone
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

Drop it in `configs/games/`, add `somegame.jpg` beside it for artwork, and start the game.

`{ separator: 'Heading' }` draws a titled divider between options; `{ separator: true }` a plain line.

### Editor support

Open the `configs` folder in VS Code (or any editor that runs the TypeScript language service).
`configs/jsconfig.json` loads the definitions in `configs/types/`:

- `host.d.ts` — the `mem` and `log` globals, documented.
- `trainer.d.ts` — the shape of a config: `Option`, `Separator`, `TickArgs`.

Command line: `npm run typecheck`.

## Compared with Cheat Engine tables

Most community trainers are shared as Cheat Engine tables (`.CT`). AZ Trainer's scripts were largely
ported from them, and the two solve the same problem differently.

| | Cheat Engine table (`.CT`) | AZ Trainer script (`.js`) |
| --- | --- | --- |
| **Format** | XML: an address list of cheat entries, with Auto Assembler and Lua scripts embedded | One JavaScript ES module per game |
| **Runs in** | Cheat Engine — a full memory scanner, debugger and disassembler | A small dedicated app that only runs trainers |
| **Writing hooks** | Auto Assembler: x64 mnemonics, `alloc`, `label`, `jmp` resolved for you | Helpers in `lib/hook.js` that build the cave, plus encoders for common instructions |
| **Reuse** | Copy and paste between tables; Lua can be shared with effort | `import` from shared libraries |
| **Finding values** | Built in: value scans, pointer scans, "find what writes to this address" | Not in the app; use Cheat Engine or the scripts in `tools/` |
| **Player experience** | Generic address list; tick checkboxes, edit values in cells | Purpose-built window: toggles, levels, groups, game artwork |
| **Game detection** | Attach to a process by hand | Automatic, including switching between games |
| **Settings** | Not remembered between sessions | Remembered per game |
| **Iteration** | Edit the script, disable and re-enable the entry | Save the file; hot reload re-applies it |
| **Editor** | Cheat Engine's script editor | Any editor, with completion and type checking |
| **Testing** | Manual, in the game | Automated: every script is checked against a fake game on each pull request |
| **Updates** | Download a newer table manually | Scripts sync from GitHub automatically |

**Use Cheat Engine** to research a game: finding values and writing complex injections.
**Use AZ Trainer** to ship the result to players.

### Porting a CT entry

| Cheat Engine | AZ Trainer |
| --- | --- |
| `aobscanmodule(site, Game.exe, 44 29 81 C8 01 00 00)` | `SCAN.once('44 29 81 C8 01 00 00')` |
| `[ENABLE]` / `[DISABLE]` sections | `OPT.whileOn(key, on, build)` — `build` installs, `restore()` undoes |
| `site: db 90 90 90 90 90 90 90` | `OPT.patchWhileOn(key, on, find, HOOK.nops(7))` |
| `alloc(newmem, $1000, site)` + `jmp newmem` + `return:` | `HOOK.detour(site, stealLen, codeBytes)` |
| `mov r15d, #99999` inside a cave | `HOOK.movR32Imm('r15d', 99999)` |
| Pointer entry `[[[base+368]+3C0]+2F8]` | `UE.chain(base, [0x368, 0x3c0, 0x2f8])` |
| Lua `readFloat` / `writeFloat` | `mem.f32(addr)` / `mem.writeF32(addr, v)` |
| Frozen value (checkbox in the address list) | `mem.hold(addr, value)`, re-written at ~1 kHz |

## Updates

The trainer checks GitHub at startup and every six hours.

- **App.** If the latest release is newer than the running version, the trainer downloads
  `az_trainer.exe`, verifies it against the release's `az_trainer.exe.sha256`, and swaps it in. A
  **Restart** button appears.
- **Scripts.** New and changed files under `configs/` on the `main` branch are downloaded and
  hot-reloaded. Scripts you edited by hand are left alone.

Updates are skipped in development builds and git checkouts (override: `AZ_TRAINER_FORCE_UPDATE=1`).

While this repository is private, updates need a GitHub token: `AZ_TRAINER_TOKEN`, `GH_TOKEN`,
`GITHUB_TOKEN`, or a logged-in [GitHub CLI](https://cli.github.com/) (`gh auth login`).

## Building

Requires Rust (stable) on Windows, and Node.js 22 or later for the script tooling.

```bash
cargo build --release
```

Run `target/release/az_trainer.exe`; it reads the repository's `configs/` folder.
`--preview configs/games/<game>.js` opens a config's window without the game.

Helper binaries:

- `cfgcheck <config.js>` — load a script with the real loader and list what it exports.
- `finder_test`, `detour_test` — exercise the breakpoint finder and detour code against a throwaway
  process.

## Tests

```bash
npm install
npm test
npm run typecheck
npm run format:check
cargo test --release
```

- **Golden hook bytes** — injected machine code must match `tests/js/golden/hook.json`.
- **Config contracts** — every config survives a game where nothing is found, and leaves code
  byte-identical after all options go on and off.
- **Libraries** and **app** unit tests.

All of it runs on every pull request. `npm run format` fixes formatting.

## Releasing

Set `version` in `Cargo.toml`, commit, then tag and push:

```bash
git tag v1.1.0
git push origin v1.1.0
```

The tag must match `Cargo.toml`. The `release` workflow tests, builds and publishes
`az_trainer.exe`, its checksum and `az_trainer.zip`.
