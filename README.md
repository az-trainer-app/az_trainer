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

An entry `{ separator: 'Heading' }` draws a titled divider between options, and `{ separator: true }` a
plain line. Separators are not options: they have no tick and are never saved, so adding or moving one
does not disturb anyone's settings.

### Editor support

Open the `configs` folder in VS Code (or any editor that runs the TypeScript language service).
`configs/jsconfig.json` loads the definitions in `configs/types/`:

- `host.d.ts` — the `mem` and `log` globals, documented.
- `trainer.d.ts` — the shape of a config: `Option`, `Separator`, `TickArgs`.

The libraries carry JSDoc types, so their imports complete and check too. Annotating `options` as in the
example above flags a misspelled field or a wrong `tick` signature as you type. Run the same check from
the command line with `npm run typecheck`.

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

**Where Cheat Engine is better:** discovery and complex injections. Its scanner, debugger and
disassembler are how the values in these scripts were found in the first place, and Auto Assembler
lets you write a multi-instruction cave as readable assembly instead of bytes. For researching a new
game, use Cheat Engine.

**Where AZ Trainer is better:** shipping the result. A script is short, reviewable in a diff, reuses
libraries instead of repeating boilerplate, is tested automatically, and gives players a focused window
that finds the game, remembers their choices and updates itself.

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

Two habits worth carrying over: prefer a signature to a fixed address, since a table's address list is
usually stale after a game patch while its `aobscan` still works; and when a game runs every character
through one routine, look for the flag the game itself uses to mark the player rather than guessing
from values — `Infinite Health` in the Veilguard script is built on exactly that.

## Updates

The trainer checks GitHub at startup and every six hours.

- **App.** If the latest release is newer than the running version, the trainer downloads
  `az_trainer.exe`, verifies it against the release's `az_trainer.exe.sha256`, and swaps it in. A
  **Restart** button appears; the running copy keeps working until you use it. A release without a
  checksum is never installed.
- **Scripts.** New and changed files under `configs/` on the `main` branch are downloaded and
  hot-reloaded. Files are compared by git blob hash, so unchanged scripts are never re-downloaded.
  A script you edited by hand is left alone — the trainer only replaces files it wrote itself.

Neither runs from a development build (`target/release`) or a git checkout, where it would fight your
working tree. Set `AZ_TRAINER_FORCE_UPDATE=1` to test it there.

While this repository is private, updates need a GitHub token: `AZ_TRAINER_TOKEN`, `GH_TOKEN`,
`GITHUB_TOKEN`, or a logged-in [GitHub CLI](https://cli.github.com/) (`gh auth login`).

## Building

Requires Rust (stable) on Windows, and Node.js 22 or later for the script tooling.

```bash
cargo build --release
```

Run `target/release/az_trainer.exe`. A build run from inside the repository reads the repository's own
`configs/` folder, so script edits are tracked by git and picked up by hot reload.

Run `az_trainer.exe --preview configs/games/<game>.js` to open a config's window without the game —
useful for checking layout and taking screenshots. Nothing is attached and clicks are not saved.

Helper binaries:

- `cfgcheck <config.js>` — load a script with the real loader and list what it exports.
- `finder_test`, `detour_test` — exercise the breakpoint finder and detour code against a throwaway
  process. Never develop injection code against a game you care about.

## Tests

```bash
npm install
npm test
npm run typecheck
npm run format:check
cargo test --release
```

- **Golden hook bytes** (`tests/js/golden/hook.json`). Every `hook.js` builder runs against a fake game
  and its output — each byte written, each cave allocated and freed — is compared with a recording
  made from the version verified in the real games. A change to injected machine code fails the build
  until it is re-verified and the golden file regenerated.
- **Config contracts.** Every script in `configs/games` is loaded and ticked in two fake games: one where
  no signature is found, which must not crash or patch anything, and one where every signature is found,
  where switching every option on and then off must leave the game's code byte-identical and free every
  cave.
- **Libraries.** `scan.js`, `option.js`, the instruction encoders and `unreal.js` pointer walks.
- **App.** Signature parsing, settings persistence, update logic, and loading every config in the real
  QuickJS host.

GitHub Actions runs all of it on every pull request (`.github/workflows/ci.yml`). Scripts are formatted
with Prettier; `npm run format` fixes formatting.

## Releasing

Set `version` in `Cargo.toml`, commit, then tag and push:

```bash
git tag v1.1.0
git push origin v1.1.0
```

The `release` workflow checks that the tag matches `Cargo.toml` — a mismatch would make installed
trainers download the same release forever — then runs the tests, builds, and publishes
`az_trainer.exe`, its checksum, and `az_trainer.zip` (exe plus `configs/`).
