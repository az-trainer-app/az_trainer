// The shape of a game config module.
//
// Annotate a config's exports to get completion and checking:
//
//   /** @type {import('../types/trainer').Entry[]} */
//   export const options = [ ... ];

/** What the engine passes to `tick` every 100 ms. */
export interface TickArgs {
    /** Whether the user has the option switched on. */
    on: boolean;
    /** The selected entry of `levels`, or 1 for a plain toggle. */
    mult: number;
}

/** One row in the trainer window. */
export interface Option {
    /**
     * Label shown in the window. Also the key settings are saved under, so
     * renaming an option forgets what the user had chosen.
     */
    name: string;
    /** Multipliers offered as pills (e.g. `[2, 4, 8]`). Omit for a checkbox. */
    levels?: number[];
    /** Pill captions, one per level. Defaults to `2x`, `4x`, ... */
    labels?: string[];
    /**
     * Global shortcuts, one per level - or one for a checkbox. Pressing one
     * selects that level; pressing it again switches the option off.
     * Modifiers (`Alt`, `Ctrl`, `Shift`, `Win`) plus one key: `F1`-`F24`,
     * `A`-`Z`, `0`-`9` or `Num0`-`Num9`.
     * @example ['Alt+F1', 'Alt+F2', 'Alt+F3']
     */
    keys?: string[];
    /** Reserve a value line under the options; `tick`'s return value fills it. */
    show?: string;
    /**
     * A one-shot action rather than a setting: `tick` returns `true` once it
     * has done its job, and the option switches itself back off. Never saved,
     * so it cannot fire again on a later attach or load.
     */
    once?: boolean;
    /**
     * Called every 100 ms whether the option is on or off, so it can both
     * apply and remove its effect. Keep the handles it creates and restore
     * them when `on` turns false - lib/option.js does this for you.
     *
     * @returns a value line to display when `show` is declared, or `true`
     *   when a `once` option is done
     */
    tick(this: Option, args: TickArgs): string | boolean | undefined | void;
}

/**
 * A divider between groups of options. It has no tick and is not saved, so
 * adding or moving one never disturbs the user's settings.
 *
 * @example
 * { separator: 'Combat' }   // ── Combat ──────────
 * { separator: true }       // ───────────────────
 */
export interface Separator {
    /** Heading drawn in the line, or `true` for a plain line. */
    separator: string | true;
}

/** Anything that can appear in `options`. */
export type Entry = Option | Separator;

/**
 * Everything a config may export. Required: `process`, `title`, `options`.
 *
 * Artwork is found automatically: `games/foo.js` uses `games/foo.jpg` (or
 * .png / .jpeg / .webp) when present.
 */
export interface Config {
    /** Executable name to attach to, e.g. `'Game.exe'`. */
    process: string;
    /** Game name shown in the titlebar. */
    title: string;
    options: Entry[];
    /** Base64 image, for configs shipped as a single file without artwork beside them. */
    art?: string;
    /** Whether the game is in a playable state. Omit if there is no cheap test. */
    live?(): boolean;
}
