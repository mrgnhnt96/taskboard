//! The UI's colors (the web board's light and dark tokens) and font families.
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
    pub jira: Hsla,
    pub backlog_dot: Hsla,
    pub on_accent: Hsla,
    pub backdrop: Hsla,
    /// TextField reads these two (placeholder and cursor/selection).
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
                up: hex(0x1f8a4c),
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
                jira: hex(0x0c66e4),
                backlog_dot: hex(0xb8c0cc),
                on_accent: hex(0xffffff),
                backdrop: alpha(0x1c2129, 0.45),
                dim: hex(0x8a95a3),
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
                accent_btn: hex(0x4d72f0),
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
                jira: hex(0x579dff),
                backlog_dot: hex(0x56606d),
                on_accent: hex(0xffffff),
                backdrop: alpha(0x000000, 0.6),
                dim: hex(0x6f7a88),
                ui_font,
                mono_font,
            },
        }
    }

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
        eprintln!("taskboard-app: could not load bundled fonts: {e:#}");
    }
    let names = cx.text_system().all_font_names();
    let has = |n: &str| names.iter().any(|x| x == n);
    let ui = if has(UI_FONT) { UI_FONT } else { UI_FALLBACK };
    let mono = if has(MONO_FONT) { MONO_FONT } else { MONO_FALLBACK };
    (ui.into(), mono.into())
}
