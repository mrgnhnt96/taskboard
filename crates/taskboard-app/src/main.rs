//! taskboard-app: the native macOS app (GPUI) for the task board, a client of taskboardd.
//!
//! Env:
//! - `TASKBOARD_BACKEND=fake` — the real board code in-process on seeded sample data (no daemon,
//!   no runner, no Midna).
//! - `TASKBOARD_URL` — where taskboardd answers (default from `config.toml`: 127.0.0.1:8792).
//! - `TASKBOARD_THEME=dark|light` — override the system appearance.
//! - `TASKBOARD_PAGE=board|backlog|sessions|settings|G3|releases:webapp`, `TASKBOARD_OPEN=T2|B1` — initial page / panel.
//! - `TASKBOARD_FAKE_RELEASES=1` — with the fake backend, sample releases and a pending cut too.
//! - `TASKBOARD_FAKE_STOPPED=1` — with the fake backend, two tasks whose terminals were closed by hand.
//! - `TASKBOARD_FAKE_NEEDS=1` — with the fake backend, misses, an alert, to-dos and items put off on Needs you.
//! - `TASKBOARD_FAKE_STOPS=1` — with the fake backend, goals stopped at a failed, held and review wave, a paused goal and a chained one.
//! - `TASKBOARD_FAKE_QUIET=1` — with the fake backend, work gone quiet under Agents waiting.
//! - `TASKBOARD_FAKE_RECAPS=1` — with the fake backend, this week's and this month's recaps, unseen.
//! - `TASKBOARD_W` / `TASKBOARD_H` — window size.
//! - `taskboard://task/T12` (also `goal/G3`, `issue/B7`, `session/<id>`) opens that page; the
//!   daemon's notifications link there. `TASKBOARD_LINK=<url>` follows one at startup.
//! - `TASKBOARD_DEV=1` — dev mode even inside a bundle: no install / login item. See `install.rs`.
//! - `TASKBOARD_PICK=B2,B3` — check these issues on the Backlog page.
//! - `TASKBOARD_SNAPSHOT=out.png` (feature `snapshot`) — draw the window offscreen, save, quit;
//!   `TASKBOARD_SNAPSHOT_MOUSE=x,y` moves the mouse there first (hover cards and tooltips),
//!   `TASKBOARD_SNAPSHOT_CLICK=x,y;x,y` clicks there in turn first (menus and opened rows),
//!   `TASKBOARD_SNAPSHOT_KEYS=tab tab` presses those keys in turn after the clicks (focus rings),
//!   `TASKBOARD_SNAPSHOT_SCROLL=x,y,dy` scrolls what's under x,y down dy px after the clicks.
mod app;
mod backend;
mod fingerprint;
mod fmt;
mod hooks;
mod install;
mod log;
mod menu_bar;
mod prefs;
mod settings;
#[cfg(test)]
mod parity;
mod theme;
mod ui;
mod updater;

use gpui_kit::*;
use theme::Theme;

actions!(taskboard, [Quit, Hide, HideOthers, ShowAll, CloseWindow, OpenSettings, OpenSystemSettings, ExportForDebugging]);

/// Writes the same redacted zip as `tb export` into Downloads and shows it in Finder, off the UI thread.
fn export_for_debugging() {
    std::thread::spawn(|| {
        let made = taskboardd::config::Config::load()
            .map_err(|e| e.to_string())
            .and_then(|cfg| taskboardd::export::run(&cfg, "app", &taskboardd::export::default_dir()));
        let _ = match made {
            Ok(path) => std::process::Command::new("open").arg("-R").arg(path).status(),
            Err(e) => std::process::Command::new("osascript")
                .args(["-e", "on run argv", "-e", "display alert \"Couldn't export the board\" message (item 1 of argv)", "-e", "end run", &e])
                .status(),
        };
    });
}

fn main() {
    unsafe { libc::signal(libc::SIGPIPE, libc::SIG_IGN) };
    log::init();
    if let Some(code) = install::headless(&std::env::args().collect::<Vec<_>>()) {
        std::process::exit(code);
    }
    let backend = backend::from_env();
    log::info(format!("started v{} ({} backend)", env!("CARGO_PKG_VERSION"), backend.label()));
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
        // Entrances and transitions stay still when the Mac asks for less motion.
        gpui_kit::base::apply_system_reduce_motion(cx);
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
        let b = backend.clone();
        cx.on_action(move |_: &OpenSystemSettings, cx| {
            if let Some(h) = settings::open(b.clone(), cx) {
                let _ = h.update(cx, |s, _, cx| s.show_system(cx));
            }
        });
        cx.on_action(|_: &ExportForDebugging, _| export_for_debugging());
        cx.on_action(|_: &Quit, cx| cx.quit());
        cx.on_action(|_: &Hide, cx| cx.hide());
        cx.on_action(|_: &HideOthers, cx| cx.hide_other_apps());
        cx.on_action(|_: &ShowAll, cx| cx.unhide_other_apps());
        cx.on_action(|_: &CloseWindow, cx| {
            if let Some(w) = cx.active_window() {
                let _ = w.update(cx, |_, window, _| window.remove_window());
            }
        });
        menu_bar::bind_keys(cx);
        menu_bar::on_app_actions(backend.label() == "fake", cx);
        cx.set_menus(menu_bar::menus(backend.label() == "fake", &app::Page::Board, "Needs you"));
        // Installed: register the daemon's LaunchAgent and link `tb` (off the UI thread).
        install::start(backend.label());
        // Installed release builds: check GitHub for a newer release now and then.
        updater::start(backend.label());
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
                        let _ = handle.update(cx, |m, window, cx| m.arrive(u, window, cx));
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
        let spot = |v: &str| {
            let (x, y) = v.split_once(',')?;
            Some(point(px(x.trim().parse().ok()?), px(y.trim().parse().ok()?)))
        };
        let clicks = std::env::var("TASKBOARD_SNAPSHOT_CLICK").unwrap_or_default();
        for position in clicks.split(';').filter_map(spot) {
            let _ = cx.update_window(any, |_, window, cx| {
                window.refresh();
                window.draw(cx).clear(cx);
                window.dispatch_event(PlatformInput::MouseMove(MouseMoveEvent { position, pressed_button: None, modifiers: Modifiers::default() }), cx);
                window.refresh();
                window.draw(cx).clear(cx);
                let (button, modifiers, click_count) = (MouseButton::Left, Modifiers::default(), 1);
                window.dispatch_event(PlatformInput::MouseDown(MouseDownEvent { button, position, modifiers, click_count, first_mouse: false }), cx);
                window.dispatch_event(PlatformInput::MouseUp(MouseUpEvent { button, position, modifiers, click_count }), cx);
            });
            cx.background_executor().timer(std::time::Duration::from_millis(400)).await;
        }
        let keys = std::env::var("TASKBOARD_SNAPSHOT_KEYS").unwrap_or_default();
        for key in keys.split_whitespace().filter_map(|k| Keystroke::parse(k).ok()) {
            let _ = cx.update_window(any, |_, window, cx| {
                window.refresh();
                window.draw(cx).clear(cx);
                window.dispatch_keystroke(key, cx);
            });
            cx.background_executor().timer(std::time::Duration::from_millis(200)).await;
        }
        if let Some((at, dy)) = std::env::var("TASKBOARD_SNAPSHOT_SCROLL").ok().and_then(|v| {
            let (xy, dy) = v.rsplit_once(',')?;
            Some((spot(xy)?, dy.trim().parse::<f32>().ok()?))
        }) {
            let _ = cx.update_window(any, |_, window, cx| {
                window.refresh();
                window.draw(cx).clear(cx);
                let delta = ScrollDelta::Pixels(point(px(0.), px(-dy)));
                window.dispatch_event(PlatformInput::ScrollWheel(ScrollWheelEvent { position: at, delta, modifiers: Modifiers::default(), touch_phase: TouchPhase::Moved }), cx);
            });
            cx.background_executor().timer(std::time::Duration::from_millis(400)).await;
        }
        let mouse = std::env::var("TASKBOARD_SNAPSHOT_MOUSE").ok().and_then(|v| spot(&v));
        if let Some(position) = mouse {
            for _ in 0..2 {
                let _ = cx.update_window(any, |_, window, cx| {
                    window.refresh();
                    window.draw(cx).clear(cx);
                    window.dispatch_event(PlatformInput::MouseMove(MouseMoveEvent { position, pressed_button: None, modifiers: Modifiers::default() }), cx);
                });
                cx.background_executor().timer(std::time::Duration::from_millis(200)).await;
            }
            for _ in 0..5 {
                cx.background_executor().timer(std::time::Duration::from_millis(400)).await;
                let _ = cx.update_window(any, |_, window, cx| {
                    window.refresh();
                    window.draw(cx).clear(cx);
                });
            }
        }
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
