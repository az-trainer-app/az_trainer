# AZ Trainer

A small, fast game trainer for Windows. Start it, start your game, click what you want.

| The Blood of Dawnwalker | Dragon Age: The Veilguard |
| :---: | :---: |
| ![AZ Trainer attached to The Blood of Dawnwalker](docs/dawnwalker.png) | ![AZ Trainer attached to Dragon Age: The Veilguard](docs/veilguard.png) |

## Features

- Detects the running game automatically
- Remembers your settings per game
- Updates itself, including new games and fixes

## Supported games

| Game | Options |
| --- | --- |
| The Blood of Dawnwalker | Infinite Human Health, Infinite Vampiric Blood, Infinite Stamina, Speed (2x/4x/8x), Denarius (1k/10k/100k) |
| Dragon Age: The Veilguard | Infinite Health, Unlimited Mana, No Cooldowns, Unlimited Potions, Ability Points Never Decrease, Bonus XP (+5000), Gold 99,999 |

Single-player only. Do not use with online or competitive games.

## Usage

1. Download `az_trainer.zip` from the [latest release](../../releases/latest) and extract it.
2. Run `az_trainer.exe` and start the game.
3. Click the options you want.
4. Close the trainer with **X** when done.

If it says "cannot open process", run it as Administrator.

## Compared with Cheat Engine tables

| | Cheat Engine table (`.CT`) | AZ Trainer |
| --- | --- | --- |
| **Needs** | Cheat Engine installed | Nothing else |
| **Interface** | Generic address list | Toggles and levels for each game |
| **Attaching** | Pick the process by hand | Automatic |
| **Settings** | Forgotten between sessions | Remembered per game |
| **Updates** | Download a newer table | Automatic |
| **Finding new cheats** | Built-in scanner and debugger | Not included |

Cheat Engine is the tool for researching a game; AZ Trainer is for playing with the result.

## Adding games

Each game is a single JavaScript file. See [docs/DEVELOPING.md](docs/DEVELOPING.md).
