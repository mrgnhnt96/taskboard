// Excerpt of crates/taskboard-app/src/main.rs, lines 78-185
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
                items: if backend.label() == "fake" {
                    vec![MenuItem::action("Close Window", CloseWindow)]
                } else {
                    vec![MenuItem::action("Close Window", CloseWindow), MenuItem::separator(), MenuItem::action("Export for Debugging…", ExportForDebugging)]
                },
                disabled: false,
            },
            Menu {
                name: "View".into(),
                items: vec![
                    MenuItem::action("Needs you", app::GoBoard),
                    MenuItem::action("Projects", app::GoProjects),
                    MenuItem::action("Goals", app::GoGoals),
                    MenuItem::action("Backlog", app::GoBacklog),
                    MenuItem::action("Days", app::GoDays),
                    MenuItem::separator(),
                    MenuItem::action("Search…", app::Search),
                    MenuItem::separator(),
                    MenuItem::action("Back", app::NavBack),
                    MenuItem::action("Forward", app::NavForward),
                    MenuItem::separator(),
                    MenuItem::action("Refresh", app::Refresh),
                ],
                disabled: false,
            },
        ]);
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
