# AZ Trainer

A lightweight trainer for single-player PC games on Windows. AZ Trainer detects the game you are playing, applies the options you choose, and restores everything when you close it.

**Open Source. No Ads, No Spyware, No Viruses.**

**Website:** https://az-trainer-app.github.io

<p align="center"><img src="docs/dawnwalker.png" alt="AZ Trainer attached to The Blood of Dawnwalker"></p>

## Supported games

Newest first; each links to its options and shortcuts on the [website](https://az-trainer-app.github.io).

- [Onimusha: Way of the Sword](https://az-trainer-app.github.io/games/onimusha/)
- [The Blood of Dawnwalker](https://az-trainer-app.github.io/games/dawnwalker/)
- [Resonance: A Plague Tale Legacy](https://az-trainer-app.github.io/games/resonance/)
- [007 First Light](https://az-trainer-app.github.io/games/firstlight/)
- [Dragon Age: The Veilguard](https://az-trainer-app.github.io/games/veilguard/)
- [Black Myth: Wukong](https://az-trainer-app.github.io/games/wukong/)
- [A Plague Tale: Requiem](https://az-trainer-app.github.io/games/requiem/)
- [A Plague Tale: Innocence](https://az-trainer-app.github.io/games/innocence/)

## Features

- **Version-aware detection:** recognises the running game and its exact build, and never modifies a version a script was not written for
- **Global shortcuts:** toggle options with hotkeys such as Alt+F1, confirmed by a brief on-screen notice in game
- **Automatic updates:** new releases, newly supported games and fixes are installed in the background
- **Saved preferences:** your selections are remembered for each game
- **Full transparency:** the active script's update date, checksum and source are shown in the trainer window

For single-player games only. Not intended for online or competitive play.

## Usage

1. Download `az_trainer.exe` from the [latest release](../../releases/latest) and save it to a folder of its own.
2. Run it and launch your game. On first launch it retrieves the latest game scripts.
3. Enable the options you want, from the window or with their shortcuts.
4. Close the trainer when you are done. Every change is reverted.

> **Windows SmartScreen warning.** On first launch Windows may report that *Microsoft Defender SmartScreen prevented an unrecognized app from starting*. AZ Trainer is not code-signed, so new releases have not yet built up download reputation. Select **More info**, then **Run anyway**. To confirm the file is genuine, compare the output of `Get-FileHash az_trainer.exe` with `az_trainer.exe.sha256` on the release page.

## Adding games

Each game is a single JavaScript file. See [docs/DEVELOPING.md](docs/DEVELOPING.md).

## License

[MIT](LICENSE)
