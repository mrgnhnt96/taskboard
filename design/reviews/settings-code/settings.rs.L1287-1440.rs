// Excerpt of crates/taskboard-app/src/settings.rs, lines 1287-1440
    fn system_page(&self, t: &Theme, cx: &mut Context<Self>) -> impl IntoElement {
        use crate::updater::{self, State};
        let list = div().id("system-rows").flex_1().min_h_0().overflow_y_scroll().flex().flex_col().gap(px(20.)).px(px(28.)).pt(px(4.)).pb(px(28.));
        let small = |b: Stateful<Div>| b.h(px(26.)).text_size(px(12.));
        let check = |label: &'static str, cx: &mut Context<Self>| {
            small(kit::btn_small(t, "sys-check", label)).on_click(cx.listener(|_, _, _, cx| {
                updater::check_now();
                cx.notify();
            }))
        };
        let channel = if updater::VERSION.contains("-beta.") { "Gets every release, betas included." } else { "Gets public releases." };
        let (dot, status, note, button): (Hsla, String, String, Option<Stateful<Div>>) = match updater::state() {
            State::Off(why) => (t.faint, "Off".into(), why, None),
            State::Idle => (t.faint, "Not checked yet".into(), channel.into(), Some(check("Check now", cx))),
            State::Checking => (t.faint, "Checking…".into(), channel.into(), Some(kit::disabled(small(kit::btn_small(t, "sys-check", "Checking…"))))),
            State::UpToDate { at } => (t.up, format!("Up to date · checked at {at}"), channel.into(), Some(check("Check now", cx))),
            State::Downloading { version } => (t.accent, format!("Downloading {version}…"), "Checked before it's offered.".into(), None),
            State::Ready { version, .. } => (
                t.accent,
                format!("{version} ready"),
                "Downloaded and checked. The board and its agents keep running through the restart.".into(),
                Some(small(kit::btn_primary(t, "sys-apply", "Restart to update")).on_click(cx.listener(|s, _, _, cx| s.apply_update(cx)))),
            ),
            State::Applying { version } => (t.accent, format!("Installing {version}…"), "Taskboard reopens in a moment.".into(), None),
            State::Failed { error, at } => (t.down, format!("Check failed at {at}"), error, Some(check("Retry", cx))),
        };
        let update = line(t, "sys-update", true, false, "Update", &note, None).child(
            controls()
                .child(kit::dot(dot, 7.))
                .child(div().text_size(px(12.)).text_color(if dot == t.faint { t.muted } else { t.text }).child(status))
                .when_some(button, |d, b| d.child(b)),
        );
        let page = match updater::state() {
            State::Ready { page, .. } => page,
            _ => updater::RELEASES_PAGE.to_string(),
        };
        let releases = row(t, "sys-releases", false)
            .min_h(px(34.))
            .text_size(px(12.))
            .child(kit::link(t, "sys-releases-link", "Release notes ↗").on_click(cx.listener(move |_, _, _, cx| cx.open_url(&page))))
            .child(div().flex_1());
        let updates = kit::card(t).overflow_hidden().child(update).child(releases);

        let app = line(t, "sys-app", true, false, "App version", "", None).child(controls().child(kit::dot(t.up, 7.)).child(div().text_size(px(12.)).child(updater::VERSION)));
        let daemon_v = self.info.as_ref().and_then(|i| i["version"].as_str()).map(str::to_string);
        let behind = daemon_v.as_deref().is_some_and(|v| v != updater::VERSION);
        let daemon_note = match (&self.info, behind) {
            (None, _) => "Not connected.".to_string(),
            (Some(_), true) => format!("Doesn't match the app. Restart it to run {}.", updater::VERSION),
            (Some(i), false) => format!("Up {} · pid {}", crate::fmt::span(i["uptime_secs"].as_i64().unwrap_or(0) / 60), i["pid"].as_u64().unwrap_or(0)),
        };
        let restart = crate::install::installed_bundle().is_some().then(|| {
            if self.sys_busy {
                kit::disabled(small(kit::btn_small(t, "sys-restart", "Restarting…")))
            } else {
                // the call to action when it's behind; otherwise there for a daemon that's stuck
                let b = if behind { kit::btn_primary(t, "sys-restart", "Restart daemon") } else { kit::btn_small(t, "sys-restart", "Restart daemon") };
                small(b).on_click(cx.listener(|s, _, _, cx| s.restart_daemon(cx)))
            }
        });
        let daemon = line(t, "sys-daemon", false, behind, "Daemon version", &daemon_note, None).child(
            controls()
                .child(kit::dot(if self.info.is_none() { t.faint } else if behind { t.warn } else { t.up }, 7.))
                .child(div().text_size(px(12.)).text_color(if daemon_v.is_some() { t.text } else { t.muted }).child(daemon_v.unwrap_or_else(|| "not running".into())))
                .when_some(restart, |d, b| d.child(b)),
        );
        let mut versions = kit::card(t).overflow_hidden().child(app).child(daemon);
        if let Some((text, bad)) = &self.sys_note {
            versions = versions.child(row(t, "sys-note", false).bg(if *bad { t.down_soft } else { t.up_soft }).text_color(if *bad { t.down } else { t.up_fg }).text_size(px(12.)).child(text.clone()));
        }
        list.child(updates).child(versions)
    }

    /// Open on System (the status bar's "update ready" pill).
    pub fn show_system(&mut self, cx: &mut Context<Self>) {
        self.view = Sec::System;
        self.load_info(cx);
        cx.notify();
    }

    /// Restart to update: swap the new app in, restart the daemon, quit; the new app opens itself.
    fn apply_update(&mut self, cx: &mut Context<Self>) {
        cx.spawn(async move |this, cx| {
            let r = cx.background_executor().spawn(async move { crate::updater::apply() }).await;
            let _ = this.update(cx, |s, cx| match r {
                Ok(()) => cx.quit(),
                Err(e) => {
                    s.sys_note = Some((e, true));
                    cx.notify();
                }
            });
        })
        .detach();
        cx.notify();
    }

    /// Restart the daemon: to run the binary in the app now (after an update it hadn't picked up), or to unstick it.
    fn restart_daemon(&mut self, cx: &mut Context<Self>) {
        let Some(bundle) = crate::install::installed_bundle() else { return };
        self.sys_busy = true;
        self.sys_note = None;
        cx.notify();
        cx.spawn(async move |this, cx| {
            let r = cx.background_executor().spawn(async move { crate::install::restart_daemon(&bundle) }).await;
            // give launchd a moment to bring it back before asking its version
            cx.background_executor().timer(Duration::from_secs(2)).await;
            let _ = this.update(cx, |s, cx| {
                s.sys_busy = false;
                if let Err(e) = r {
                    s.sys_note = Some((format!("Couldn't restart the daemon: {e}"), true));
                }
                s.load_info(cx);
                cx.notify();
            });
        })
        .detach();
    }
}

// ------------------------------------------------------------------ reviewers

/// What a reviewer's row says about them: their accounts, pin, bot, pace and asks.
pub(crate) fn reviewer_note(r: &Value) -> String {
    let mut bits: Vec<String> = vec![];
    let mut ids: Vec<String> = r["user"].as_str().map(|u| vec![u.to_string()]).unwrap_or_default();
    for k in ["emails", "aliases"] {
        ids.extend(r[k].as_array().cloned().unwrap_or_default().iter().filter_map(|x| x.as_str().map(|s| s.to_string())));
    }
    if !ids.is_empty() {
        bits.push(ids.join(", "));
    }
    if r["bot"].is_object() {
        bits.push(format!("Bot every {}h", r["bot"]["every_h"].as_f64().map(|h| format!("{h}")).unwrap_or_default()));
    }
    if let Some(a) = r["automation"].as_f64().filter(|a| (*a - 1.0).abs() > 1e-9) {
        bits.push(format!("Automation {a}"));
    }
    if let Some(m) = r["median_work_mins"].as_f64() {
        bits.push(format!("Reviews in {}", crate::fmt::span(m.round() as i64)));
    }
    let open = r["open_asks"].as_i64().unwrap_or(0);
    if open > 0 {
        bits.push(format!("{open} open"));
    }
    if let Some(at) = r["last_asked"].as_str() {
        bits.push(format!("Last asked {}", crate::fmt::full_time(at)));
    }
    if r["removed"] == true {
        bits.push(r["removed_why"].as_str().map(|w| format!("Removed: {w}")).unwrap_or_else(|| "Removed".into()));
    }
    bits.join(" · ")
}

impl SettingsWindow {
