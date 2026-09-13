# AZ Trainer

A small, fast game trainer for Windows. Start it, start your game, click what you want.

| The Blood of Dawnwalker | Dragon Age: The Veilguard |
| :---: | :---: |
| ![AZ Trainer attached to The Blood of Dawnwalker](docs/dawnwalker.png) | ![AZ Trainer attached to Dragon Age: The Veilguard](docs/veilguard.png) |

## Features

- Detects the running game automatically
- Remembers your settings per game
- Updates itself, including new games and fixes
- Shows which script is loaded: its update date, checksum and a link to its source

Single-player only. Do not use with online or competitive games.

## Usage

1. Download `az_trainer.zip` from the [latest release](../../releases/latest) and extract it.
2. Run `az_trainer.exe` and start the game.
3. Click the options you want.
4. Close the trainer with **X** when done.

If it says "cannot open process", run it as Administrator.

## Adding games

Each game is a single JavaScript file. See [docs/DEVELOPING.md](docs/DEVELOPING.md).
