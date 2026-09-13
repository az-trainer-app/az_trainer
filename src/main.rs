#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod art;
mod engine;
mod finder;
mod hold;
mod js;
mod mem;

use std::path::PathBuf;
use std::time::Duration;

use engine::Engine;
use gpui::{
    div, img, prelude::*, px, rgb, size, App,
    Application, Bounds, Context, FontWeight, MouseButton, ObjectFit, SharedString,
    TitlebarOptions, Window, WindowBounds, WindowOptions,
};

/// Displayed as "AZ Trainer v1.0"; the value comes from Cargo.toml.
const VERSION: &str = env!("CARGO_PKG_VERSION");

fn app_title() -> String {
    let mut it = VERSION.split('.');
    match (it.next(), it.next()) {
        (Some(maj), Some(min)) => format!("AZ Trainer v{maj}.{min}"),
        _ => format!("AZ Trainer v{VERSION}"),
    }
}

const BG: u32 = 0x11131a;
const PANEL: u32 = 0x181b23;
const TEXT: u32 = 0xd6dae3;
const DIM: u32 = 0x7c8595;
const ACCENT: u32 = 0xc23b3b;

struct Row {
    name: SharedString,
    levels: Vec<f32>,
    labels: Vec<String>,
    level: usize,
}

// layout metrics, also used to compute the window height
const PAD: f32 = 14.0; // outer padding on every side
const BANNER_H: f32 = 108.0; // full-width art stripe, title overlaid on it
const SEARCH_H: f32 = 170.0; // window height while no game is attached
const DOTS: usize = 8;       // spinner dots
const LINE_H: f32 = 24.0; // a live value line
const ROW_H: f32 = 28.0; // a toggle row
const GAP: f32 = 6.0;
const SECTION_GAP: f32 = 12.0;
const WIDTH: f32 = 420.0;

struct Trainer {
    engine: Engine,
    title: SharedString,
    status: SharedString,
    rows: Vec<Row>,
    values: Vec<SharedString>,
    art: Option<PathBuf>,
    /// false while no game is attached: the UI shows the spinner instead
    ready: bool,
    /// spinner phase, advanced once per UI tick
    frame: usize,
    /// last titlebar text we set, so we only touch the window on change
    titled: String,
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
            engine: Engine::start(),
            title: "AZ Trainer".into(),
            status: "starting...".into(),
            rows: Vec::new(),
            values: Vec::new(),
            art: None,
            ready: false,
            frame: 0,
            titled: String::new(),
        };
        t.pull();
        t
    }

    fn pull(&mut self) {
        let Ok(s) = self.engine.shared.lock() else { return };
        self.status = s.status.clone().into();
        self.title = if s.title.is_empty() {
            "AZ Trainer".into()
        } else {
            s.title.clone().into()
        };
        self.ready = s.ready;
        self.art = s.art.clone();
        if s.ready {
            self.rows = s
                .names
                .iter()
                .enumerate()
                .map(|(i, n)| Row {
                    name: n.clone().into(),
                    levels: s.levels.get(i).cloned().unwrap_or_default(),
                    labels: s.labels.get(i).cloned().unwrap_or_default(),
                    level: s.level.get(i).copied().unwrap_or(0),
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
        if !self.ready {
            return SEARCH_H;
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
            h += rows * ROW_H + (rows - 1.0).max(0.0) * GAP;
            blocks += 1;
        }
        if blocks == 2 {
            h += SECTION_GAP;
        }
        h
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
    fn render(&mut self, _w: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
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
            _w.set_window_title(&want_title);
            self.titled = want_title;
        }

        let wanted = self.wanted_height();
        if (wanted - f32::from(_w.viewport_size().height)).abs() > 1.0 {
            _w.resize(size(px(WIDTH), px(wanted)));
        }

        if !self.ready {
            return div()
                .flex()
                .flex_col()
                .items_center()
                .justify_center()
                .gap_4()
                .size_full()
                .bg(rgb(BG))
                .font_family("Segoe UI")
                .text_sm()
                .child(spinner(self.frame / 2))
                .child(div().text_color(rgb(DIM)).child(self.status.clone()))
                .into_any_element();
        }

        div()
            .flex()
            .flex_col()
            .size_full()
            .bg(rgb(BG))
            .text_color(rgb(TEXT))
            .font_family("Segoe UI")
            .text_sm()
            // Full-bleed art stripe. The artwork already carries the game's
            // name, so the text is a fallback for configs shipped without an
            // image rather than a label drawn on top of one.
            .child(
                div()
                    .relative()
                    .flex_none()
                    .w_full()
                    .h(px(BANNER_H))
                    .bg(rgb(PANEL))
                    .overflow_hidden()
                    .map(|d| match self.art.clone() {
                        Some(p) => d.child(
                            img(p)
                                .absolute()
                                .inset_0()
                                .size_full()
                                .object_fit(ObjectFit::Cover),
                        ),
                        None => d
                            .flex()
                            .items_center()
                            .justify_center()
                            .px(px(PAD))
                            .child(
                                div()
                                    .text_xl()
                                    .text_color(rgb(0xffffff))
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .child(self.title.clone()),
                            ),
                    }),
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
            // toggles
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap(px(GAP))
                    .children(self.rows.iter().enumerate().map(|(i, row)| {
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
                                            .child(
                                                div()
                                                    .w(px(14.))
                                                    .h(px(14.))
                                                    .rounded_sm()
                                                    .border_1()
                                                    .border_color(rgb(if on { ACCENT } else { DIM }))
                                                    .bg(rgb(if on { ACCENT } else { BG })),
                                            )
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
                                        div()
                                            .id(("lvl", (i * 16 + k) as u64))
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
                                            .child(match row.labels.get(k) {
                                                Some(l) => l.clone(),
                                                None => format!("{}x", *mult as i32),
                                            })
                                    }),
                                ))
                            })
                    })),
            ),
            )
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

fn main() {
    #[cfg(windows)]
    apply_window_icon();

    Application::new().run(|cx: &mut App| {
        let bounds = Bounds::centered(None, size(px(WIDTH), px(BANNER_H + PAD * 2.0)), cx);
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
