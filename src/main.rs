#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod art;
#[cfg(feature = "devtools")]
mod devtools;
mod engine;
mod finder;
mod game;
mod hold;
#[cfg(windows)]
mod hotkey;
mod js;
mod keys;
#[cfg(windows)]
mod toast;
mod mem;
mod update;

use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use engine::Engine;
use gpui::{
    canvas, div, img, linear_color_stop, linear_gradient, point, prelude::*, px, rgb, rgba, size,
    Animation, AnimationExt, App, Application, Bounds, Context, FontWeight, MouseButton,
    ObjectFit, PathBuilder, SharedString, TitlebarOptions, Window, WindowBounds, WindowOptions,
};

/// Displayed as "AZ Trainer v1.0.1"; the value comes from Cargo.toml.
const VERSION: &str = env!("CARGO_PKG_VERSION");

fn app_title() -> String {
    format!("AZ Trainer v{VERSION}")
}

const BG: u32 = 0x11131a;
const PANEL: u32 = 0x181b23;
const LINE: u32 = 0x2a2f3a;
const TEXT: u32 = 0xd6dae3;
const DIM: u32 = 0x7c8595;
const ACCENT: u32 = 0xc23b3b;

struct Row {
    name: SharedString,
    levels: Vec<f32>,
    labels: Vec<String>,
    /// shortcuts, one per level (one for a toggle)
    keys: Vec<String>,
    level: usize,
    /// `Some` when this row is a divider: its heading, possibly empty
    separator: Option<SharedString>,
}

impl Row {
    fn height(&self) -> f32 {
        if self.separator.is_some() {
            SEP_H
        } else {
            ROW_H
        }
    }
}

// layout metrics, also used to compute the window height
const PAD: f32 = 14.0; // outer padding on every side
const BANNER_H: f32 = art::BANNER_H; // full-width art stripe, title overlaid on it
const SEARCH_H: f32 = 170.0; // window height while no game is attached
const DOTS: usize = 8;       // spinner dots
const FOOTER_LINE_H: f32 = 30.0; // one update notice line
const STATUS_H: f32 = 24.0; // config status bar at the very bottom
const LINE_H: f32 = 24.0; // a live value line
const ROW_H: f32 = 28.0; // a toggle row
const SEP_H: f32 = 22.0; // a separator row
const GAP: f32 = 6.0;
const SECTION_GAP: f32 = 12.0;
const COLUMN_GAP: f32 = 20.0; // between the two option columns

struct Trainer {
    engine: Engine,
    title: SharedString,
    status: SharedString,
    rows: Vec<Row>,
    values: Vec<SharedString>,
    art: Option<art::Art>,
    /// option columns the config asks for
    columns: usize,
    /// false while no game is attached: the UI shows the spinner instead
    ready: bool,
    /// spinner phase, advanced once per UI tick
    frame: usize,
    /// last titlebar text we set, so we only touch the window on change
    titled: String,
    updates: Arc<Mutex<update::State>>,
    /// version installed by the updater, waiting for a restart
    staged: Option<SharedString>,
    /// loaded config's file, update date and checksum
    config_info: Option<SharedString>,
    /// the loaded config on GitHub, when the repository has it
    config_url: Option<SharedString>,
    /// shortcut of the control under the mouse, shown in the status bar
    hint: Option<SharedString>,
}

/// Blend two packed RGB colours; `t` runs 0.0 (a) to 1.0 (b).
fn mix(a: u32, b: u32, t: f32) -> u32 {
    let ch = |sh: u32| {
        let (x, y) = (((a >> sh) & 0xff) as f32, ((b >> sh) & 0xff) as f32);
        ((x + (y - x) * t) as u32).min(0xff) << sh
    };
    ch(16) | ch(8) | ch(0)
}

impl Trainer {
    fn new(cx: &mut Context<Self>) -> Self {
        cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor().timer(Duration::from_millis(100)).await;
                if this
                    .update(cx, |t, cx| {
                        t.frame = t.frame.wrapping_add(1);
                        t.pull();
                        cx.notify();
                    })
                    .is_err()
                {
                    break;
                }
            }
        })
        .detach();

        let mut t = Trainer {
            engine: match preview_arg() {
                Some(config) => Engine::preview(config),
                None => Engine::start(),
            },
            title: "AZ Trainer".into(),
            status: "starting...".into(),
            rows: Vec::new(),
            values: Vec::new(),
            art: None,
            columns: 1,
            ready: false,
            frame: 0,
            titled: String::new(),
            updates: Arc::new(Mutex::new(update::State::default())),
            staged: None,
            config_info: None,
            config_url: None,
            hint: None,
        };
        // Closing the window ends the process without dropping Trainer, so
        // Drop for Engine never runs and every patch the config installed
        // stays in the game. Shut the engine down while the entity is still
        // alive, which restores them and detaches.
        // Both, because closing the last window and quitting the app do not
        // reliably run the same path, and a missed shutdown leaves the game
        // patched. shutdown() is idempotent, so running twice is harmless.
        let closed = cx.weak_entity();
        cx.on_window_closed(move |cx| {
            eprintln!("[app] window closed - shutting the engine down");
            if let Some(t) = closed.upgrade() {
                t.update(cx, |t, _| t.engine.shutdown());
            }
        })
        .detach();

        // Context's own on_app_quit hands the entity straight back, so this
        // one needs no handle of its own.
        cx.on_app_quit(|t: &mut Trainer, _cx| {
            eprintln!("[app] quitting - shutting the engine down");
            t.engine.shutdown();
            async {}
        })
        .detach();

        update::start(t.updates.clone());
        t.pull();
        t
    }

    fn pull(&mut self) {
        let rel = self.engine.shared.lock().ok().and_then(|s| s.config_rel.clone());
        if let Ok(u) = self.updates.lock() {
            self.staged = u.staged.clone().map(Into::into);
            self.config_url = rel
                .as_deref()
                .and_then(|r| update::github_url(&u, r))
                .map(Into::into);
        }
        let Ok(s) = self.engine.shared.lock() else { return };
        self.status = s.status.clone().into();
        self.title = if s.title.is_empty() {
            "AZ Trainer".into()
        } else {
            s.title.clone().into()
        };
        self.ready = s.ready;
        self.art = s.art.clone();
        self.columns = s.columns;
        self.config_info = s.config_info.clone().map(Into::into);
        if !s.ready {
            self.hint = None;
        }
        if s.ready {
            self.rows = s
                .names
                .iter()
                .enumerate()
                .map(|(i, n)| Row {
                    name: n.clone().into(),
                    levels: s.levels.get(i).cloned().unwrap_or_default(),
                    labels: s.labels.get(i).cloned().unwrap_or_default(),
                    keys: s.keys.get(i).cloned().unwrap_or_default(),
                    level: s.level.get(i).copied().unwrap_or(0),
                    separator: s.separators.get(i).cloned().flatten().map(Into::into),
                })
                .collect();
            // one line per option that declares `show`, so the layout does not
            // jump as values come and go between menus and gameplay
            self.values = s
                .shows
                .iter()
                .enumerate()
                .filter_map(|(i, label)| {
                    let label = label.as_ref()?;
                    Some(match s.values.get(i).and_then(|v| v.clone()) {
                        Some(v) => v.into(),
                        None => format!("{label}  --").into(),
                    })
                })
                .collect();
        }
    }

    /// Height needed to show everything, with no scrolling and no clipping.
    fn wanted_height(&self) -> f32 {
        let footer = self.footer_lines() as f32 * FOOTER_LINE_H
            + if self.status_bar_shown() { STATUS_H } else { 0.0 };
        if !self.ready {
            return SEARCH_H + footer;
        }
        let values = self.values.len() as f32;
        let rows = self.rows.len() as f32;
        let mut h = BANNER_H + PAD * 2.0;
        let mut blocks = 0;
        if values > 0.0 {
            h += values * LINE_H + (values - 1.0).max(0.0) * GAP;
            blocks += 1;
        }
        if rows > 0.0 {
            h += self
                .row_columns()
                .into_iter()
                .map(|c| stack_height(&self.rows[c].iter().map(Row::height).collect::<Vec<_>>()))
                .fold(0.0, f32::max);
            blocks += 1;
        }
        if blocks == 2 {
            h += SECTION_GAP;
        }
        h + footer
    }

    fn footer_lines(&self) -> usize {
        self.staged.is_some() as usize
    }

    fn status_bar_shown(&self) -> bool {
        self.ready && self.config_info.is_some()
    }

    /// Show `hint` in the status bar while hovered; clear it on leave, unless
    /// another control has taken the bar over in between.
    fn hover_hint(&mut self, hint: SharedString, hovered: bool) {
        if hovered {
            self.hint = Some(hint);
        } else if self.hint.as_ref() == Some(&hint) {
            self.hint = None;
        }
    }

    /// One dim line pinned to the window's bottom edge, naming the loaded
    /// config with its update date and checksum, plus a link to the original
    /// when the repository has it. While a control with a shortcut is
    /// hovered, it names the shortcut instead.
    fn status_bar(&self, cx: &mut Context<Self>) -> Option<gpui::AnyElement> {
        if !self.status_bar_shown() {
            return None;
        }
        Some(
            div()
                .flex_none()
                .h(px(STATUS_H))
                .px(px(PAD))
                .flex()
                .items_center()
                .justify_between()
                .gap_2()
                .border_t_1()
                .border_color(rgb(LINE))
                .bg(rgb(PANEL))
                .text_xs()
                .text_color(rgb(DIM))
                .child(match self.hint.clone() {
                    Some(hint) => div().overflow_hidden().text_color(rgb(TEXT)).child(hint),
                    None => div().overflow_hidden().children(self.config_info.clone()),
                })
                .children(self.config_url.clone().map(|url| {
                    div()
                        .id("github")
                        .flex_none()
                        .cursor_pointer()
                        .text_color(rgb(TEXT))
                        .hover(|d| d.text_color(rgb(ACCENT)))
                        .child("source")
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(move |_this, _e, _w, cx| cx.open_url(&url)),
                        )
                }))
                .into_any_element(),
        )
    }

    /// Update notices, pinned under everything else. Absent when there is
    /// nothing to say, so the window does not carry an empty strip.
    fn footer(&self, cx: &mut Context<Self>) -> Option<gpui::AnyElement> {
        if self.footer_lines() == 0 {
            return None;
        }
        let line = || {
            div()
                .h(px(FOOTER_LINE_H))
                .px(px(PAD))
                .flex()
                .items_center()
                .justify_between()
                .text_xs()
        };
        Some(
            div()
                .flex_none()
                .flex()
                .flex_col()
                .bg(rgb(PANEL))
                .children(self.staged.clone().map(|v| {
                    line()
                        .child(
                            div()
                                .text_color(rgb(TEXT))
                                .child(format!("Version {v} installed")),
                        )
                        .child(
                            div()
                                .id("restart")
                                .px_2()
                                .py(px(2.))
                                .rounded_sm()
                                .cursor_pointer()
                                .bg(rgb(ACCENT))
                                .text_color(rgb(0xffffff))
                                .child("Restart")
                                .on_mouse_down(
                                    MouseButton::Left,
                                    cx.listener(|this, _e, _w, cx| this.restart(cx)),
                                ),
                        )
                }))
                .into_any_element(),
        )
    }

    /// One entry of `rows`: a divider, or an option's name with its checkbox
    /// or level pills.
    fn option_row(&self, i: usize, cx: &mut Context<Self>) -> gpui::AnyElement {
        let row = &self.rows[i];
        if let Some(heading) = &row.separator {
            return separator(heading);
        }
        let on = row.level > 0;
        let has_levels = !row.levels.is_empty();

        div()
            .h(px(ROW_H))
            .flex()
            .items_center()
            .justify_between()
            .gap_2()
            .px_2()
            .rounded_md()
            .child(
                // name + checkbox (checkbox only for plain toggles)
                div()
                    .id(("name", i))
                    .flex()
                    .items_center()
                    .gap_2()
                    .when_some(
                        row.keys.first().filter(|_| !has_levels).cloned(),
                        |d, key| {
                            let hint: SharedString =
                                format!("{}  ·  {key}", row.name).into();
                            d.on_hover(cx.listener(move |this, hovered: &bool, _w, cx| {
                                this.hover_hint(hint.clone(), *hovered);
                                cx.notify();
                            }))
                        },
                    )
                    .when(!has_levels, |d| {
                        d.cursor_pointer()
                            .on_mouse_down(
                                MouseButton::Left,
                                cx.listener(move |this, _e, _w, cx| {
                                    this.engine.cycle(i);
                                    this.pull();
                                    cx.notify();
                                }),
                            )
                            .child(checkbox(on))
                    })
                    .child(
                        div()
                            .text_color(rgb(if on { TEXT } else { DIM }))
                            .child(row.name.clone()),
                    ),
            )
            // level pills: 2x / 4x / 8x
            .when(has_levels, |d| {
                d.child(div().flex().gap_1().children(
                    row.levels.iter().enumerate().map(|(k, mult)| {
                        let sel = row.level == k + 1;
                        let caption = match row.labels.get(k) {
                            Some(l) => l.clone(),
                            None => format!("{}x", *mult as i32),
                        };
                        div()
                            .id(("lvl", (i * 16 + k) as u64))
                            .when_some(row.keys.get(k).cloned(), |d, key| {
                                let hint: SharedString =
                                    format!("{} {caption}  ·  {key}", row.name).into();
                                d.on_hover(cx.listener(move |this, hovered: &bool, _w, cx| {
                                    this.hover_hint(hint.clone(), *hovered);
                                    cx.notify();
                                }))
                            })
                            .px_2()
                            .py(px(1.))
                            .rounded_sm()
                            .border_1()
                            .cursor_pointer()
                            .border_color(rgb(if sel { ACCENT } else { PANEL }))
                            .bg(rgb(if sel { ACCENT } else { PANEL }))
                            .text_color(rgb(if sel { 0xffffff } else { DIM }))
                            .text_xs()
                            .on_mouse_down(
                                MouseButton::Left,
                                cx.listener(move |this, _e, _w, cx| {
                                    this.engine.set_level(i, k + 1);
                                    this.pull();
                                    cx.notify();
                                }),
                            )
                            .child(caption)
                    }),
                ))
            })
            .into_any_element()
    }

    /// The window's width: wider while a two-column config is attached.
    fn width(&self) -> f32 {
        art::window_width(if self.ready { self.columns } else { 1 })
    }

    /// `rows` split into the columns they are shown in, as index ranges.
    fn row_columns(&self) -> Vec<std::ops::Range<usize>> {
        let n = self.rows.len();
        if self.columns < 2 || n < 2 {
            return vec![0..n];
        }
        let heights: Vec<f32> = self.rows.iter().map(Row::height).collect();
        let separators: Vec<bool> = self.rows.iter().map(|r| r.separator.is_some()).collect();
        let at = split_point(&heights, &separators);
        vec![0..at, at..n]
    }

    /// Hand over to the freshly installed exe.
    ///
    /// The engine is shut down first and the new instance is told to wait for
    /// this process to exit, so the game is fully unpatched before anything
    /// tries to patch it again.
    fn restart(&mut self, cx: &mut Context<Self>) {
        self.engine.shutdown();
        if let Ok(exe) = std::env::current_exe() {
            let _ = std::process::Command::new(exe)
                .args(["--wait-pid", &std::process::id().to_string()])
                .spawn();
        }
        cx.quit();
    }
}

/// How often the glints cross the banner art, how much of that cycle one
/// crossing takes, and where in the cycle each pass starts - the rest of the
/// time the band waits off the right edge.
const SWEEP_EVERY: Duration = Duration::from_secs(7);
const SWEEP_CROSSING: f32 = 0.045;
const SWEEP_PASSES: [f32; 2] = [0.0, 0.055];
/// Width of the glint band.
const SWEEP_BAND: f32 = 140.0;
/// Gradient direction in degrees (90 = straight across), so the band leans.
const SWEEP_ANGLE: f32 = 110.0;

/// A soft band of light that crosses the banner art now and then.
///
/// GPUI has no hook for custom shaders, so this is two gradient quads - clear
/// to faint white, then back to clear - slid across by an animation. Only
/// rendered while the window is focused: a repeating animation asks for a
/// frame every vsync, which is not worth spending on a window behind the game.
fn light_sweep(width: f32) -> impl IntoElement {
    // white at zero alpha rather than transparent black, so the fade does not
    // pass through grey on its way in
    let clear = rgba(0xffffff00);
    let glint = rgba(0xffffff1c);
    let half = || div().h_full().w(px(SWEEP_BAND / 2.0));
    div()
        .absolute()
        .top_0()
        .h_full()
        .w(px(SWEEP_BAND))
        .flex()
        .child(half().bg(linear_gradient(SWEEP_ANGLE,linear_color_stop(clear, 0.), linear_color_stop(glint, 1.))))
        .child(half().bg(linear_gradient(SWEEP_ANGLE,linear_color_stop(glint, 0.), linear_color_stop(clear, 1.))))
        .with_animation("banner-sweep", Animation::new(SWEEP_EVERY).repeat(), move |band, t| {
            let x = SWEEP_PASSES
                .iter()
                .find_map(|&start| {
                    let p = (t - start) / SWEEP_CROSSING;
                    (0.0..1.0).contains(&p).then(|| {
                        let eased = p * p * (3.0 - 2.0 * p); // smoothstep: eases in and out
                        -SWEEP_BAND + (width + SWEEP_BAND) * eased
                    })
                })
                // between passes: parked just past the right edge, clipped by the banner
                .unwrap_or(width);
            band.left(px(x))
        })
}

/// The game name over the banner: white with a subtle black drop shadow, drawn
/// as a black copy offset by one pixel behind the white text.
fn title_label(title: SharedString) -> impl IntoElement {
    let text = || div().text_xl().font_weight(FontWeight::SEMIBOLD);
    div()
        .relative()
        .child(
            text()
                .absolute()
                .left(px(1.))
                .top(px(1.))
                .text_color(rgba(0x000000c0))
                .child(title.clone()),
        )
        .child(text().text_color(rgb(0xffffff)).child(title))
}

/// A checkbox. Off: an outlined square. On: filled with the accent, rimmed a
/// shade lighter, with a white tick drawn as a path - no glyph, so it cannot
/// fall back to tofu.
fn checkbox(on: bool) -> impl IntoElement {
    const BOX: f32 = 16.0;
    div()
        .flex_none()
        .w(px(BOX))
        .h(px(BOX))
        .rounded(px(4.))
        .border_1()
        .border_color(rgb(if on { mix(ACCENT, 0xffffff, 0.3) } else { DIM }))
        .bg(rgb(if on { ACCENT } else { BG }))
        .when(on, |d| {
            d.child(
                canvas(
                    |_, _, _| {},
                    |bounds, _, window, _| {
                        // in the 14px inside the border
                        let at = |x: f32, y: f32| point(bounds.origin.x + px(x), bounds.origin.y + px(y));
                        let mut tick = PathBuilder::stroke(px(2.));
                        tick.move_to(at(3.2, 7.2));
                        tick.line_to(at(5.9, 9.9));
                        tick.line_to(at(10.9, 4.3));
                        if let Ok(path) = tick.build() {
                            window.paint_path(path, rgb(0xffffff));
                        }
                    },
                )
                .size_full(),
            )
        })
}

/// Height of rows stacked with `GAP` between them.
fn stack_height(heights: &[f32]) -> f32 {
    heights.iter().sum::<f32>() + heights.len().saturating_sub(1) as f32 * GAP
}

/// Where the second column starts: the split that leaves the taller column
/// shortest. A break just before a separator keeps its group together, so it
/// wins unless splitting inside a group is more than a couple of rows more
/// even. A column never ends on a separator.
fn split_point(heights: &[f32], separators: &[bool]) -> usize {
    let n = heights.len();
    let cost = |at: usize| stack_height(&heights[..at]).max(stack_height(&heights[at..]));
    // on a tie the later split wins, so the left column takes the extra row
    let best = |candidates: Vec<usize>| {
        candidates.into_iter().min_by(|&a, &b| cost(a).total_cmp(&cost(b)).then(b.cmp(&a)))
    };
    let anywhere = best((1..n).filter(|&i| !separators[i - 1]).collect()).unwrap_or(n.div_ceil(2));
    match best((1..n).filter(|&i| separators[i]).collect()) {
        Some(group) if cost(group) <= cost(anywhere) + 2.0 * (ROW_H + GAP) => group,
        _ => anywhere,
    }
}

/// A divider between groups of options: `── Heading ──────`, or a plain line
/// when the heading is empty.
fn separator(heading: &SharedString) -> gpui::AnyElement {
    let line = || div().h(px(1.)).bg(rgb(LINE));
    let row = div().h(px(SEP_H)).flex().items_center().gap_2().px_2();
    if heading.is_empty() {
        row.child(line().flex_1()).into_any_element()
    } else {
        row.child(line().w(px(12.)))
            .child(div().text_xs().text_color(rgb(DIM)).child(heading.clone()))
            .child(line().flex_1())
            .into_any_element()
    }
}

/// A ring of dots with a travelling highlight - no glyphs, so it cannot fall
/// back to tofu on a machine missing the usual spinner characters.
fn spinner(frame: usize) -> impl IntoElement {
    const BOX: f32 = 44.0;
    const R: f32 = 16.0;
    const DOT: f32 = 6.0;

    div()
        .relative()
        .w(px(BOX))
        .h(px(BOX))
        .children((0..DOTS).map(|i| {
            let a = std::f32::consts::TAU * (i as f32) / (DOTS as f32);
            // 0 for the dot currently lit, rising as the tail falls behind
            let age = (frame + DOTS - i) % DOTS;
            let t = 1.0 - (age as f32) / (DOTS as f32);
            div()
                .absolute()
                .left(px(BOX / 2.0 + R * a.sin() - DOT / 2.0))
                .top(px(BOX / 2.0 - R * a.cos() - DOT / 2.0))
                .w(px(DOT))
                .h(px(DOT))
                .rounded_full()
                .bg(rgb(mix(PANEL, ACCENT, t)))
        }))
}

impl Render for Trainer {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        // Grow/shrink so nothing is clipped. Measured against the window's
        // real height rather than the last value we asked for: a request the
        // window manager drops used to leave us convinced we had resized,
        // with no retry, which is how a game switch could strand the window
        // at the previous layout's height.
        // "AZ Trainer v1.0 - The Blood of Dawnwalker", back to the bare app
        // name whenever no game is attached
        let want_title = if self.ready {
            format!("{} - {}", app_title(), self.title)
        } else {
            app_title()
        };
        if want_title != self.titled {
            window.set_window_title(&want_title);
            self.titled = want_title;
        }

        let (width, wanted) = (self.width(), self.wanted_height());
        let viewport = window.viewport_size();
        if (wanted - f32::from(viewport.height)).abs() > 1.0
            || (width - f32::from(viewport.width)).abs() > 1.0
        {
            window.resize(size(px(width), px(wanted)));
        }
        // the banner glint and liquid play whenever there is art
        let sweep = self.art.is_some();

        if !self.ready {
            return div()
                .flex()
                .flex_col()
                .size_full()
                .bg(rgb(BG))
                .font_family("Segoe UI")
                .text_sm()
                .child(
                    div()
                        .flex_1()
                        .flex()
                        .flex_col()
                        .items_center()
                        .justify_center()
                        .gap_4()
                        .child(spinner(self.frame / 2))
                        .child(div().text_color(rgb(DIM)).child(self.status.clone())),
                )
                .children(self.footer(cx))
                .into_any_element();
        }

        let columns: Vec<_> = self
            .row_columns()
            .into_iter()
            .map(|range| {
                let rows: Vec<gpui::AnyElement> = range.map(|i| self.option_row(i, cx)).collect();
                div().flex_1().flex().flex_col().gap(px(GAP)).children(rows)
            })
            .collect();

        div()
            .flex()
            .flex_col()
            .size_full()
            .bg(rgb(BG))
            .text_color(rgb(TEXT))
            .font_family("Segoe UI")
            .text_sm()
            // Full-bleed art stripe with the game's name drawn over it (the
            // hero art carries no text). White with a subtle black drop shadow,
            // sitting on the banner's faded-dark bottom for contrast.
            .child(
                div()
                    .relative()
                    .flex_none()
                    .w_full()
                    .h(px(BANNER_H))
                    .bg(rgb(PANEL))
                    .overflow_hidden()
                    .when_some(self.art.as_ref(), |d, a| {
                        d.child(
                            img(a.banner.clone())
                                .absolute()
                                .inset_0()
                                .size_full()
                                .object_fit(ObjectFit::Cover),
                        )
                    })
                    // the stretched fills either side, stirring like a liquid
                    .when_some(self.art.as_ref().filter(|_| sweep), |d, a| {
                        d.children(a.liquid.iter().enumerate().map(|(i, l)| {
                            img(l.frames.clone())
                                .id(("banner-liquid", i))
                                .absolute()
                                .top_0()
                                .left(px(width * l.left))
                                .w(px(width * l.width))
                                .h_full()
                                .object_fit(ObjectFit::Fill)
                        }))
                    })
                    // over the art, under the title
                    .when(sweep, |d| d.child(light_sweep(width)))
                    .child(
                        div()
                            .absolute()
                            .left(px(PAD))
                            .bottom(px(8.))
                            .child(title_label(self.title.clone())),
                    ),
            )
            .child(
                div()
                    .flex()
                    .flex_col()
                    .p(px(PAD))
                    .gap(px(SECTION_GAP))
            // live values - omitted entirely when empty, otherwise the empty
            // container still consumes a SECTION_GAP and the computed height
            // comes up short, eating the bottom padding
            .when(!self.values.is_empty(), |parent| {
                parent.child(
                    div()
                        .flex()
                        .flex_col()
                        .gap(px(GAP))
                        .children(self.values.iter().map(|v| {
                            div()
                                .h(px(LINE_H))
                                .flex()
                                .items_center()
                                .px_3()
                                .rounded_md()
                                .bg(rgb(PANEL))
                                .child(v.clone())
                        })),
                )
            })
            // toggles, in one or two columns
            .child(div().flex().gap(px(COLUMN_GAP)).children(columns)),
            )
            .children(self.footer(cx))
            .children(self.status_bar(cx))
            .into_any_element()
    }
}

/// GPUI registers its own window class with no icon, so the titlebar falls
/// back to the Windows default even though the exe carries one. Find our
/// window and hand it the embedded icon (resource id 1).
#[cfg(windows)]
fn apply_window_icon() {
    use windows::core::PCWSTR;
    use windows::Win32::Foundation::{BOOL, HWND, LPARAM, WPARAM};
    use windows::Win32::System::LibraryLoader::GetModuleHandleW;
    use windows::Win32::System::Threading::GetCurrentProcessId;
    use windows::Win32::UI::WindowsAndMessaging::{
        EnumWindows, GetWindowLongPtrW, GetWindowThreadProcessId, LoadImageW,
        SendMessageW, SetWindowLongPtrW, SetWindowPos, GWL_STYLE, ICON_BIG, ICON_SMALL,
        IMAGE_ICON, LR_DEFAULTSIZE, LR_SHARED, SWP_FRAMECHANGED, SWP_NOMOVE,
        SWP_NOSIZE, SWP_NOZORDER, WM_SETICON, WS_MAXIMIZEBOX, WS_THICKFRAME,
    };

    unsafe extern "system" fn cb(hwnd: HWND, lparam: LPARAM) -> BOOL {
        let mut pid = 0u32;
        GetWindowThreadProcessId(hwnd, Some(&mut pid));
        if pid == GetCurrentProcessId() {
            let icon = lparam.0;
            SendMessageW(hwnd, WM_SETICON, WPARAM(ICON_SMALL as usize), LPARAM(icon));
            SendMessageW(hwnd, WM_SETICON, WPARAM(ICON_BIG as usize), LPARAM(icon));

            // The window sizes itself to fit its options, so a resize grip and
            // a maximize button would only let the user break that layout.
            let style = GetWindowLongPtrW(hwnd, GWL_STYLE);
            let fixed = style & !((WS_THICKFRAME.0 | WS_MAXIMIZEBOX.0) as isize);
            if fixed != style {
                SetWindowLongPtrW(hwnd, GWL_STYLE, fixed);
                let _ = SetWindowPos(
                    hwnd,
                    None,
                    0,
                    0,
                    0,
                    0,
                    SWP_NOMOVE | SWP_NOSIZE | SWP_NOZORDER | SWP_FRAMECHANGED,
                );
            }
        }
        BOOL(1)
    }

    std::thread::spawn(|| unsafe {
        let Ok(hmod) = GetModuleHandleW(None) else { return };
        let hinst: windows::Win32::Foundation::HINSTANCE = hmod.into();
        let Ok(icon) = LoadImageW(
            hinst,
            PCWSTR(1 as *const u16),
            IMAGE_ICON,
            0,
            0,
            LR_DEFAULTSIZE | LR_SHARED,
        ) else {
            return;
        };
        // the window does not exist immediately; retry briefly
        for _ in 0..40 {
            std::thread::sleep(std::time::Duration::from_millis(100));
            let _ = EnumWindows(Some(cb), LPARAM(icon.0 as isize));
        }
    });
}

/// `--preview <config.js>`: that config's window without a game attached.
fn preview_arg() -> Option<PathBuf> {
    let mut args = std::env::args();
    while let Some(a) = args.next() {
        if a == "--preview" {
            return args.next().map(PathBuf::from);
        }
    }
    None
}

/// After an update, the new instance is started with `--wait-pid <old pid>`
/// and holds off until the old one has exited and restored the game.
fn wait_for_previous_instance() {
    let mut args = std::env::args();
    while let Some(a) = args.next() {
        if a != "--wait-pid" {
            continue;
        }
        let Some(pid) = args.next().and_then(|p| p.parse::<u32>().ok()) else { return };
        #[cfg(windows)]
        unsafe {
            use windows::Win32::Foundation::CloseHandle;
            use windows::Win32::System::Threading::{
                OpenProcess, WaitForSingleObject, PROCESS_SYNCHRONIZE,
            };
            if let Ok(h) = OpenProcess(PROCESS_SYNCHRONIZE, false, pid) {
                WaitForSingleObject(h, 15_000);
                let _ = CloseHandle(h);
            }
        }
    }
}

fn main() {
    wait_for_previous_instance();

    #[cfg(windows)]
    apply_window_icon();

    Application::new().run(|cx: &mut App| {
        let bounds = Bounds::centered(None, size(px(art::window_width(1)), px(BANNER_H + PAD * 2.0)), cx);
        cx.open_window(
            WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(bounds)),
                titlebar: Some(TitlebarOptions {
                    title: Some(app_title().into()),
                    ..Default::default()
                }),
                ..Default::default()
            },
            |_w, cx| cx.new(Trainer::new),
        )
        .unwrap();
        cx.activate(true);
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    const R: f32 = ROW_H;
    const S: f32 = SEP_H;

    #[test]
    fn columns_break_before_a_separator_when_that_is_even_enough() {
        // Combat: 3 options, Movement: 3 options
        let heights = [S, R, R, R, S, R, R, R];
        let seps = [true, false, false, false, true, false, false, false];
        assert_eq!(split_point(&heights, &seps), 4);
    }

    #[test]
    fn a_lopsided_group_is_split_inside_rather_than_left_uneven() {
        // one option, then a group of nine
        let heights = [R, S, R, R, R, R, R, R, R, R, R];
        let seps = [false, true, false, false, false, false, false, false, false, false, false];
        let at = split_point(&heights, &seps);
        assert!((5..=7).contains(&at), "split at {at}");
    }

    #[test]
    fn without_separators_the_left_column_takes_the_extra_row() {
        assert_eq!(split_point(&[R; 5], &[false; 5]), 3);
        assert_eq!(split_point(&[R; 4], &[false; 4]), 2);
    }

    #[test]
    fn a_column_never_ends_on_a_separator() {
        let heights = [R, R, S, R, R];
        let seps = [false, false, true, false, false];
        let at = split_point(&heights, &seps);
        assert!(!seps[at - 1], "left column ends on the separator (split {at})");
    }
}
