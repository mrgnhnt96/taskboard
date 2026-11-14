//! The UI's colors (the web board's light and dark tokens), font families and the shared size
//! scales: text sizes, spacing, control heights, corners, shadows, opacity and motion.
use gpui_kit::*;

pub const UI_FONT: &str = "Atkinson Hyperlegible Next";
pub const MONO_FONT: &str = "JetBrains Mono";
pub const UI_FALLBACK: &str = "Helvetica Neue";
pub const MONO_FALLBACK: &str = "Menlo";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ThemeMode {
    Dark,
    Light,
}

#[derive(Clone, Debug)]
pub struct Theme {
    pub mode: ThemeMode,
    pub bg: Hsla,
    pub card: Hsla,
    pub panel_2: Hsla,
    pub col: Hsla,
    pub seg: Hsla,
    pub tint: Hsla,
    pub text: Hsla,
    pub text_2: Hsla,
    pub muted: Hsla,
    pub faint: Hsla,
    pub border: Hsla,
    pub border_2: Hsla,
    pub divider: Hsla,
    pub accent: Hsla,
    pub accent_btn: Hsla,
    pub accent_soft: Hsla,
    pub accent_fg: Hsla,
    pub accent_line: Hsla,
    pub warn: Hsla,
    pub warn_soft: Hsla,
    pub warn_fg: Hsla,
    pub warn_text: Hsla,
    pub warn_line: Hsla,
    pub up: Hsla,
    pub up_soft: Hsla,
    pub up_fg: Hsla,
    pub results: Hsla,
    pub results_soft: Hsla,
    pub down: Hsla,
    pub down_soft: Hsla,
    pub down_line: Hsla,
    pub goal: Hsla,
    pub goal_soft: Hsla,
    pub goal_tint: Hsla,
    pub goal_line: Hsla,
    /// A search match inside a result's text.
    pub mark: Hsla,
    pub mark_fg: Hsla,
    pub jira: Hsla,
    pub backlog_dot: Hsla,
    pub on_accent: Hsla,
    /// The filled button's hover and pressed fills.
    pub accent_hover: Hsla,
    pub accent_down: Hsla,
    /// A label or mark drawn on a solid status colour (warn, up, down, goal, results).
    pub on_status: Hsla,
    /// The edge of a text box or checkbox: 3:1 on the card in both modes.
    pub control_line: Hsla,
    /// The 2px ring drawn outside a focused control.
    pub focus_ring: Hsla,
    /// A faint accent wash behind an open or chosen card.
    #[allow(dead_code)]
    pub accent_tint: Hsla,
    /// Behind images and video, the same in both modes.
    #[allow(dead_code)]
    pub media_bg: Hsla,
    /// A hover fill laid over any surface.
    pub hover_overlay: Hsla,
    /// The shadow colour every elevation uses.
    pub shadow: Hsla,
    pub backdrop: Hsla,
    /// The placeholder grey TextField reads: the readable `muted`, never the decorative `faint`.
    pub dim: Hsla,
    pub ui_font: SharedString,
    pub mono_font: SharedString,
}

impl Global for Theme {}

fn hex(s: u32) -> Hsla {
    rgb(s).into()
}

fn alpha(s: u32, a: f32) -> Hsla {
    let mut c = hex(s);
    c.a = a;
    c
}

impl Theme {
    pub fn new(mode: ThemeMode, ui_font: SharedString, mono_font: SharedString) -> Theme {
        match mode {
            ThemeMode::Light => Theme {
                mode,
                bg: hex(0xf6f7f9),
                card: hex(0xffffff),
                panel_2: hex(0xf0f2f5),
                col: hex(0xeceff3),
                seg: hex(0xe9ecf0),
                tint: hex(0xfbfbfc),
                text: hex(0x1c2129),
                text_2: hex(0x3d4652),
                muted: hex(0x5f6b7a),
                faint: hex(0x8a95a3),
                border: hex(0xe2e6eb),
                border_2: hex(0xcfd5dd),
                divider: hex(0xeceff3),
                accent: hex(0x3563e9),
                accent_btn: hex(0x3563e9),
                accent_soft: hex(0xe8eefd),
                accent_fg: hex(0x2447b8),
                accent_line: hex(0xd5e0fb),
                warn: hex(0xc2410c),
                warn_soft: hex(0xfdece3),
                warn_fg: hex(0x9a3409),
                warn_text: hex(0x7a2a08),
                warn_line: hex(0xf0b695),
                up: hex(0x1a7d45),
                up_soft: hex(0xe3f4ea),
                up_fg: hex(0x17693a),
                results: hex(0x0e7490),
                results_soft: hex(0xe0f3f7),
                down: hex(0xb42318),
                down_soft: hex(0xfde8e7),
                down_line: hex(0xf1c7c3),
                goal: hex(0x5b3fc4),
                goal_soft: hex(0xf1edfd),
                goal_tint: hex(0xf5f3ff),
                goal_line: hex(0xb9a8f0),
                mark: hex(0xfff1b8),
                mark_fg: hex(0x1c2129),
                jira: hex(0x0c66e4),
                backlog_dot: hex(0xb8c0cc),
                on_accent: hex(0xffffff),
                accent_hover: hex(0x2c55d4),
                accent_down: hex(0x2447b8),
                on_status: hex(0xffffff),
                control_line: hex(0x7f8896),
                focus_ring: hex(0x3563e9),
                accent_tint: hex(0xf5f8ff),
                media_bg: hex(0x1c2129),
                hover_overlay: alpha(0x1c2129, 0.05),
                shadow: alpha(0x101828, 0.14),
                backdrop: alpha(0x1c2129, 0.45),
                dim: hex(0x5f6b7a),
                ui_font,
                mono_font,
            },
            ThemeMode::Dark => Theme {
                mode,
                bg: hex(0x0f1216),
                card: hex(0x1a1f26),
                panel_2: hex(0x232932),
                col: hex(0x151a20),
                seg: hex(0x232931),
                tint: hex(0x171c22),
                text: hex(0xe6e9ee),
                text_2: hex(0xc3cad4),
                muted: hex(0xa1abb8),
                faint: hex(0x6f7a88),
                border: hex(0x2a3039),
                border_2: hex(0x3a424d),
                divider: hex(0x252b33),
                accent: hex(0x7c9cff),
                accent_btn: hex(0x466ae6),
                accent_soft: hex(0x1f2a47),
                accent_fg: hex(0xa9bcff),
                accent_line: hex(0x2b3a63),
                warn: hex(0xf59e5b),
                warn_soft: hex(0x3a2415),
                warn_fg: hex(0xf7b98a),
                warn_text: hex(0xf7c9a6),
                warn_line: hex(0x6b3a1c),
                up: hex(0x4cc38a),
                up_soft: hex(0x16301f),
                up_fg: hex(0x7fd9a9),
                results: hex(0x4fc3d9),
                results_soft: hex(0x123139),
                down: hex(0xf47067),
                down_soft: hex(0x3b1a19),
                down_line: hex(0x6a2a27),
                goal: hex(0xa996f5),
                goal_soft: hex(0x2a2347),
                goal_tint: hex(0x1f1b33),
                goal_line: hex(0x54479a),
                mark: hex(0x4a3d12),
                mark_fg: hex(0xffe08a),
                jira: hex(0x579dff),
                backlog_dot: hex(0x56606d),
                on_accent: hex(0xffffff),
                accent_hover: hex(0x3b5bd0),
                accent_down: hex(0x2f4cb8),
                on_status: hex(0x0f1216),
                control_line: hex(0x6f7a88),
                focus_ring: hex(0x7c9cff),
                accent_tint: hex(0x172036),
                media_bg: hex(0x1c2129),
                hover_overlay: alpha(0xffffff, 0.06),
                shadow: alpha(0x000000, 0.55),
                backdrop: alpha(0x000000, 0.6),
                dim: hex(0xa1abb8),
                ui_font,
                mono_font,
            },
        }
    }

    /// A raised control: the chosen segment, a hovered card. 0 1px 2px.
    pub fn shadow_raised(&self) -> Vec<BoxShadow> {
        self.elevation(1., 2.)
    }

    /// Menus, tips and popovers. 0 8px 24px.
    pub fn shadow_overlay(&self) -> Vec<BoxShadow> {
        self.elevation(8., 24.)
    }

    /// Dialogs. 0 24px 64px.
    pub fn shadow_modal(&self) -> Vec<BoxShadow> {
        self.elevation(24., 64.)
    }

    fn elevation(&self, y: f32, blur: f32) -> Vec<BoxShadow> {
        vec![BoxShadow { color: self.shadow, offset: point(px(0.), px(y)), blur_radius: px(blur), spread_radius: px(0.), inset: false }]
    }
}

/// The chart palette for projects and categories, in a fixed order checked for colour-blind
/// separation in both modes. It leaves out the greens, oranges and reds that mean done, waiting
/// and lost. Past the list, `chart_other`.
pub fn chart(mode: ThemeMode, index: usize) -> Hsla {
    const LIGHT: [u32; 5] = [0x2a78d6, 0xa87200, 0x4a3aa7, 0xc94f7c, 0x1f9e89];
    const DARK: [u32; 5] = [0x3987e5, 0xc98500, 0x9085e9, 0xd55181, 0x2bb39c];
    let pal = if mode == ThemeMode::Dark { DARK } else { LIGHT };
    pal.get(index).map_or_else(|| chart_other(mode), |c| hex(*c))
}

/// The grey for "everything else" in a chart.
pub fn chart_other(mode: ThemeMode) -> Hsla {
    hex(if mode == ThemeMode::Dark { 0x6f7a88 } else { 0x8a95a3 })
}

/// Text sizes. Line height is 1.4 for body text and 1.3 for headings.
#[allow(dead_code)]
pub mod text {
    /// Pills, captions and section labels.
    pub const CAPTION: f32 = 11.;
    /// Meta lines, chips, small buttons, tips.
    pub const META: f32 = 12.;
    /// Body text, buttons, menus, fields.
    pub const BODY: f32 = 13.;
    /// A row's title.
    pub const ROW_TITLE: f32 = 14.;
    pub const CARD_TITLE: f32 = 16.;
    pub const PANEL_TITLE: f32 = 20.;
    pub const PAGE_TITLE: f32 = 24.;
    pub const LINE_BODY: f32 = 1.4;
    pub const LINE_HEADING: f32 = 1.3;
}

/// The spacing scale. Gaps inside a row take the lower step; gaps between groups the higher.
#[allow(dead_code)]
pub mod space {
    pub const S1: f32 = 4.;
    pub const S2: f32 = 8.;
    pub const S3: f32 = 12.;
    pub const S4: f32 = 16.;
    pub const S6: f32 = 24.;
    pub const S8: f32 = 32.;
    pub const S10: f32 = 40.;
    pub const S12: f32 = 48.;
    pub const S16: f32 = 64.;
    pub const S20: f32 = 80.;
    pub const S24: f32 = 96.;
    /// A page's side margin and top.
    pub const PAGE_X: f32 = 32.;
    pub const PAGE_TOP: f32 = 24.;
    /// Between a menu or tip and the control it belongs to.
    pub const ANCHOR_GAP: f32 = 4.;
}

/// Control heights.
#[allow(dead_code)]
pub mod height {
    /// Pills and chips.
    pub const TAG: f32 = 20.;
    pub const SMALL: f32 = 24.;
    pub const CONTROL: f32 = 32.;
    pub const ROW: f32 = 40.;
}

/// Corners. Items inside a menu or card take the next step down.
#[allow(dead_code)]
pub mod radius {
    /// Chips, segments' items, small marks.
    pub const INNER: f32 = 4.;
    /// Buttons, fields, menu items.
    pub const CONTROL: f32 = 8.;
    /// Cards, menus, panels.
    pub const CONTAINER: f32 = 12.;
    pub const DIALOG: f32 = 16.;
}

#[allow(dead_code)]
pub mod opacity {
    pub const DISABLED: f32 = 0.45;
}

/// Motion, in milliseconds.
#[allow(dead_code)]
pub mod motion {
    pub const FEEDBACK: u64 = 80;
    pub const ENTER: u64 = 160;
    pub const MOVE: u64 = 240;
}

/// The mode to start in: `TASKBOARD_THEME=dark|light`, else the system appearance.
pub fn startup_mode(system_dark: bool) -> ThemeMode {
    match std::env::var("TASKBOARD_THEME").as_deref() {
        Ok("dark") => ThemeMode::Dark,
        Ok("light") => ThemeMode::Light,
        _ if system_dark => ThemeMode::Dark,
        _ => ThemeMode::Light,
    }
}

/// Load the bundled OFL fonts; returns the (ui, mono) family names to use, falling back to
/// system fonts when embedding fails.
pub fn load_fonts(cx: &mut App) -> (SharedString, SharedString) {
    use std::borrow::Cow;
    let fonts: Vec<Cow<'static, [u8]>> = vec![
        Cow::Borrowed(include_bytes!("../assets/fonts/AtkinsonHyperlegibleNext-Regular.ttf")),
        Cow::Borrowed(include_bytes!("../assets/fonts/AtkinsonHyperlegibleNext-Medium.ttf")),
        Cow::Borrowed(include_bytes!("../assets/fonts/AtkinsonHyperlegibleNext-Bold.ttf")),
        Cow::Borrowed(include_bytes!("../assets/fonts/JetBrainsMono-Regular.ttf")),
        Cow::Borrowed(include_bytes!("../assets/fonts/JetBrainsMono-SemiBold.ttf")),
    ];
    if let Err(e) = cx.text_system().add_fonts(fonts) {
        crate::log::info(format!("could not load bundled fonts: {e:#}"));
    }
    let names = cx.text_system().all_font_names();
    let has = |n: &str| names.iter().any(|x| x == n);
    let ui = if has(UI_FONT) { UI_FONT } else { UI_FALLBACK };
    let mono = if has(MONO_FONT) { MONO_FONT } else { MONO_FALLBACK };
    (ui.into(), mono.into())
}

#[cfg(test)]
mod tests {
    use super::{Theme, ThemeMode};
    use gpui_kit::{Hsla, Rgba};

    fn lum(c: Hsla) -> f32 {
        let c: Rgba = c.into();
        let ch = |v: f32| if v <= 0.03928 { v / 12.92 } else { ((v + 0.055) / 1.055).powf(2.4) };
        0.2126 * ch(c.r) + 0.7152 * ch(c.g) + 0.0722 * ch(c.b)
    }

    fn contrast(a: Hsla, b: Hsla) -> f32 {
        let (x, y) = (lum(a), lum(b));
        (x.max(y) + 0.05) / (x.min(y) + 0.05)
    }

    fn both() -> [Theme; 2] {
        [ThemeMode::Light, ThemeMode::Dark].map(|m| Theme::new(m, "".into(), "".into()))
    }

    fn check(pairs: &[(&str, Hsla, Hsla)], min: f32) {
        let low: Vec<String> = pairs.iter().filter(|(_, a, b)| contrast(*a, *b) < min).map(|(n, a, b)| format!("{n} {:.2}", contrast(*a, *b))).collect();
        assert!(low.is_empty(), "under {min}: {low:?}");
    }

    #[test]
    fn text_reads_at_four_and_a_half_to_one_in_both_modes() {
        for t in both() {
            let m = format!("{:?}", t.mode);
            let mut pairs = Vec::new();
            for (sn, s) in [("bg", t.bg), ("card", t.card), ("panel_2", t.panel_2), ("seg", t.seg)] {
                for (tn, c) in [("text", t.text), ("text_2", t.text_2), ("muted", t.muted), ("placeholder", t.dim)] {
                    pairs.push((format!("{m} {tn} on {sn}"), c, s));
                }
            }
            pairs.push((format!("{m} label on filled button"), t.on_accent, t.accent_btn));
            pairs.push((format!("{m} label on hovered button"), t.on_accent, t.accent_hover));
            pairs.push((format!("{m} link on card"), t.accent, t.card));
            pairs.push((format!("{m} danger on card"), t.down, t.card));
            let pairs: Vec<(&str, Hsla, Hsla)> = pairs.iter().map(|(n, a, b)| (n.as_str(), *a, *b)).collect();
            check(&pairs, 4.5);
        }
    }

    #[test]
    fn marks_on_status_colours_read_in_both_modes() {
        for t in both() {
            let on = t.on_status;
            check(&[("warn", on, t.warn), ("up", on, t.up), ("down", on, t.down), ("goal", on, t.goal), ("results", on, t.results)], 4.5);
        }
    }

    #[test]
    fn status_pills_read_in_both_modes() {
        for t in both() {
            check(
                &[
                    ("working", t.accent_fg, t.accent_soft),
                    ("needs", t.warn_fg, t.warn_soft),
                    ("done", t.up_fg, t.up_soft),
                    ("failed", t.down, t.down_soft),
                    ("planned", t.goal, t.goal_soft),
                    ("results", t.results, t.results_soft),
                    ("neutral", t.muted, t.seg),
                ],
                4.5,
            );
        }
    }

    #[test]
    fn field_edges_and_focus_rings_stand_out_at_three_to_one() {
        for t in both() {
            check(&[("field edge", t.control_line, t.card), ("field edge on page", t.control_line, t.bg), ("focus ring", t.focus_ring, t.card), ("focus ring on page", t.focus_ring, t.bg)], 3.);
        }
    }

    #[test]
    fn chart_colours_stand_out_from_the_card() {
        for t in both() {
            let pairs: Vec<(String, Hsla, Hsla)> = (0..5).map(|i| (format!("{:?} chart {i}", t.mode), super::chart(t.mode, i), t.card)).collect();
            let pairs: Vec<(&str, Hsla, Hsla)> = pairs.iter().map(|(n, a, b)| (n.as_str(), *a, *b)).collect();
            check(&pairs, 3.);
        }
    }
}
