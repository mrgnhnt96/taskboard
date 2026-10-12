//! taskboard-app: the native macOS app (GPUI) for the task board, a client of taskboardd.
//!
//! Env:
//! - `TASKBOARD_BACKEND=fake` — the real board code in-process on seeded sample data (no daemon,
//!   no runner, no Midna).
//! - `TASKBOARD_URL` — where taskboardd answers (default from `config.toml`: 127.0.0.1:8792).
//! - `TASKBOARD_THEME=dark|light` — override the system appearance.
//! - `TASKBOARD_PAGE=board|backlog|sessions|settings|G3`, `TASKBOARD_OPEN=T2|B1` — initial page / panel.
//! - `TASKBOARD_W` / `TASKBOARD_H` — window size.
//! - `taskboard://task/T12` (also `goal/G3`, `issue/B7`, `session/<id>`) opens that page; the
//!   daemon's notifications link there.
//! - `TASKBOARD_DEV=1` — dev mode even inside a bundle: no install / login item. See `install.rs`.
//! - `TASKBOARD_PICK=B2,B3` — check these issues on the Backlog page.
//! - `TASKBOARD_SNAPSHOT=out.png` (feature `snapshot`) — draw the window offscreen, save, quit.
mod app;
mod backend;
mod fmt;
mod hooks;
mod install;
mod prefs;
mod settings;
#[cfg(test)]
mod parity;
mod theme;
mod ui;

use gpui_kit::*;
use theme::Theme;

actions!(taskboard, [Quit, Hide, HideOthers, ShowAll, CloseWindow, OpenSettings]);

fn main() {
    unsafe { libc::signal(libc::SIGPIPE, libc::SIG_IGN) };
    if let Some(code) = install::headless(&std::env::args().collect::<Vec<_>>()) {
        std::process::exit(code);
    }
    let backend = backend::from_env();
    // The reader's UI state (the web board's localStorage); the sample board keeps it in memory.
    prefs::init(backend.label() != "fake");
    let size_env = |k: &str, d: f32| std::env::var(k).ok().and_then(|v| v.parse().ok()).unwrap_or(d);
    let (w, h) = (size_env("TASKBOARD_W", 1360.), size_env("TASKBOARD_H", 860.));
    // `taskboard://…` links (clicked notifications) can arrive before the window exists, so
    // they wait in a channel.
    let (link_tx, link_rx) = async_channel::unbounded::<Vec<String>>();
    let application = gpui_kit::application();
    application.on_open_urls(move |urls| drop(link_tx.try_send(urls)));
    application.run(move |cx| {
        let (ui_font, mono_font) = theme::load_fonts(cx);
        let system_dark = matches!(cx.window_appearance(), WindowAppearance::Dark | WindowAppearance::VibrantDark);
        cx.set_global(Theme::new(theme::startup_mode(system_dark), ui_font, mono_font));
        app::bind_keys(cx);
        cx.bind_keys([
            KeyBinding::new("cmd-q", Quit, None),
            KeyBinding::new("cmd-h", Hide, None),
            KeyBinding::new("cmd-alt-h", HideOthers, None),
            KeyBinding::new("cmd-w", CloseWindow, None),
            KeyBinding::new("cmd-,", OpenSettings, None),
        ]);
        let b = backend.clone();
        cx.on_action(move |_: &OpenSettings, cx| {
            let _ = settings::open(b.clone(), cx);
        });
        cx.on_action(|_: &Quit, cx| cx.quit());
        cx.on_action(|_: &Hide, cx| cx.hide());
        cx.on_action(|_: &HideOthers, cx| cx.hide_other_apps());
        cx.on_action(|_: &ShowAll, cx| cx.unhide_other_apps());
        cx.on_action(|_: &CloseWindow, cx| {
            if let Some(w) = cx.active_window() {
                let _ = w.update(cx, |_, window, _| window.remove_window());
            }
        });
        cx.set_menus(vec![
            Menu {
                name: "Task board".into(),
                items: vec![
                    MenuItem::action("Settings…", OpenSettings),
                    MenuItem::separator(),
                    MenuItem::action("Hide Task board", Hide),
                    MenuItem::action("Hide Others", HideOthers),
                    MenuItem::action("Show All", ShowAll),
                    MenuItem::separator(),
                    MenuItem::action("Quit Task board", Quit),
                ],
                disabled: false,
            },
            Menu {
                name: "File".into(),
                items: vec![
                    MenuItem::action("Close Window", CloseWindow),
                ],
                disabled: false,
            },
            Menu {
                name: "View".into(),
                items: vec![
                    MenuItem::action("Board", app::GoBoard),
                    MenuItem::action("Backlog", app::GoBacklog),
                    MenuItem::action("Sessions", app::GoSessions),
                    MenuItem::separator(),
                    MenuItem::action("Refresh", app::Refresh),
                ],
                disabled: false,
            },
        ]);
        // Installed: register the daemon's LaunchAgent and link `tb` (off the UI thread).
        install::start(backend.label());
        // The board keeps running in taskboardd; closing the window just quits the app.
        cx.on_window_closed(|cx, _| {
            if cx.windows().is_empty() {
                cx.quit();
            }
        })
        .detach();
        let bounds = Bounds::centered(None, size(px(w), px(h)), cx);
        let opts = WindowOptions {
            window_bounds: Some(WindowBounds::Windowed(bounds)),
            titlebar: Some(TitlebarOptions { title: Some("Task board".into()), appears_transparent: true, traffic_light_position: Some(point(px(16.), px(16.))) }),
            window_min_size: Some(size(px(900.), px(560.))),
            app_id: Some("com.mrgnhnt.taskboard".into()),
            focus: std::env::var("TASKBOARD_NO_ACTIVATE").is_err(),
            ..Default::default()
        };
        let b = backend.clone();
        let handle = cx.open_window(opts, |window, cx| cx.new(|cx| app::MainWindow::new(b, window, cx))).expect("open main window");
        cx.spawn(async move |cx| {
            while let Ok(urls) = link_rx.recv().await {
                cx.update(|cx| {
                    cx.activate(true);
                    for u in &urls {
                        let _ = handle.update(cx, |m, _, cx| m.open_link(u, cx));
                    }
                });
            }
        })
        .detach();
        let settings_page = std::env::var("TASKBOARD_PAGE").as_deref() == Ok("settings");
        let settings = settings_page.then(|| settings::open(backend.clone(), cx)).flatten();
        #[cfg(feature = "snapshot")]
        snapshot(settings.map_or(handle.into(), Into::into), cx);
        let _ = settings;
        let _ = handle;
        if std::env::var("TASKBOARD_NO_ACTIVATE").is_err() {
            cx.activate(true);
        }
    });
}

/// Dev: render the main window offscreen to `TASKBOARD_SNAPSHOT` after
/// `TASKBOARD_SNAPSHOT_DELAY_MS` (default 1500), then quit.
#[cfg(feature = "snapshot")]
fn snapshot(any: AnyWindowHandle, cx: &mut App) {
    let Ok(path) = std::env::var("TASKBOARD_SNAPSHOT") else {
        return;
    };
    let delay: u64 = std::env::var("TASKBOARD_SNAPSHOT_DELAY_MS").ok().and_then(|v| v.parse().ok()).unwrap_or(1500);
    cx.spawn(async move |cx| {
        cx.background_executor().timer(std::time::Duration::from_millis(delay)).await;
        for _ in 0..2 {
            let _ = cx.update_window(any, |_, window, cx| {
                window.refresh();
                window.draw(cx).clear(cx);
            });
            cx.background_executor().timer(std::time::Duration::from_millis(300)).await;
        }
        let _ = cx.update_window(any, |_, window, cx| {
            window.refresh();
            window.draw(cx).clear(cx);
            match window.render_to_image() {
                Ok(img) => match img.save(&path) {
                    Ok(()) => eprintln!("taskboard-app: snapshot saved to {path}"),
                    Err(e) => eprintln!("taskboard-app: snapshot save failed: {e}"),
                },
                Err(e) => eprintln!("taskboard-app: render_to_image failed: {e:#}"),
            }
        });
        let _ = cx.update(|cx| cx.quit());
    })
    .detach();
}
