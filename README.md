# AZ Trainer

A small, fast game trainer for Windows. Start it, start your game, click what you want.

**Website:** https://az-trainer-app.github.io

<p align="center"><img src="docs/dawnwalker.png" alt="AZ Trainer attached to The Blood of Dawnwalker"></p>

## Supported games

Newest first. Screenshots of every trainer are on the [website](https://az-trainer-app.github.io).

- Onimusha: Way of the Sword
- The Blood of Dawnwalker
- Resonance: A Plague Tale Legacy
- Dragon Age: The Veilguard
- Black Myth: Wukong
- A Plague Tale: Requiem
- A Plague Tale: Innocence

## Features

- Detects the running game automatically, and its version: a script never touches a build it was not made for
- Remembers your settings per game
- Keyboard shortcuts (e.g. Alt+F1-F3 for speed), with a brief on-screen notice in game
- Updates itself, including new games and fixes
- Shows which script is loaded: its update date, checksum and a link to its source

Single-player only. Do not use with online or competitive games.

## Usage

1. Download `az_trainer.exe` from the [latest release](../../releases/latest) and put it in a folder of its own.
2. Run it and start the game. On first start it downloads the game scripts next to itself.
3. Click the options you want.
4. Close the trainer with **X** when done.

## Adding games

Each game is a single JavaScript file. See [docs/DEVELOPING.md](docs/DEVELOPING.md).
