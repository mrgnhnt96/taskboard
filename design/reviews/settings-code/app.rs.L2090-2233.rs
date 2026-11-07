// Excerpt of crates/taskboard-app/src/app.rs, lines 2090-2233
pub fn pill_shell(t: &Theme, id: &'static str, fg: Hsla, bg: Hsla) -> Stateful<Div> {
    let _ = t;
    div().id(id).flex().flex_none().items_center().gap(px(6.)).py(px(3.)).px(px(10.)).rounded_full().text_size(px(12.5)).line_height(px(18.75)).whitespace_nowrap().text_color(fg).bg(bg)
}

/// Amber "GitHub: sign in again" when an account lacks scopes Taskboard needs (or stopped working);
/// opens Settings. `state.accounts` is the daemon's `accounts::attention`.
pub(crate) fn accounts_pill(st: &Value, t: &Theme) -> Option<AnyElement> {
    let list = st.get("accounts")?.as_array().filter(|l| !l.is_empty())?;
    let label = match list.as_slice() {
        [one] => format!("{}: {}", one["label"].as_str().unwrap_or("Account"), if one["reauth"] == true { "sign in again" } else { "check sign-in" }),
        many => format!("{} accounts need you", many.len()),
    };
    let tip = list.iter().map(|a| format!("{}: {}", a["label"].as_str().unwrap_or(""), a["reason"].as_str().unwrap_or(""))).collect::<Vec<_>>().join("\n");
    Some(
        pill_shell(t, "accounts-pill", t.warn_fg, t.warn_soft)
            .cursor_pointer()
            .child(kit::dot(t.warn, 7.))
            .child(label)
            .tooltip(kit::tip(tip))
            .on_click(|_, window, cx| window.dispatch_action(Box::new(crate::OpenSettings), cx))
            .into_any_element(),
    )
}

/// "Taskboard 1.2.0 is ready" once a newer release is downloaded and checked; opens Settings › System,
/// where Restart to update installs it.
pub(crate) fn update_pill(t: &Theme) -> Option<AnyElement> {
    let crate::updater::State::Ready { version, .. } = crate::updater::state() else { return None };
    Some(
        pill_shell(t, "update-pill", t.accent, t.accent_soft)
            .cursor_pointer()
            .child(kit::dot(t.accent, 7.))
            .child(format!("Taskboard {version} is ready"))
            .tooltip(kit::tip(format!("A new release, downloaded and checked. Restart to update from {} to {version}.", crate::updater::VERSION)))
            .on_click(|_, window, cx| window.dispatch_action(Box::new(crate::OpenSystemSettings), cx))
            .into_any_element(),
    )
}

/// "Install hooks", "● Hooks" or an amber "Reinstall hooks", as in Midna's status bar.
pub(crate) fn hooks_item(m: &MainWindow, t: &Theme, cx: &mut Context<MainWindow>) -> Option<AnyElement> {
    use crate::hooks::Hooks;
    let can_install = crate::hooks::target().is_some_and(|t| t.can_install);
    let text = t.text;
    let base = div().id("hooks").flex().items_center().gap(px(5.)).text_size(px(12.5)).whitespace_nowrap().text_color(t.muted);
    let clickable = |d: Stateful<Div>| -> Stateful<Div> {
        if can_install {
            d.cursor_pointer().hover(move |s| s.text_color(text)).on_click(cx.listener(|m, _, _, cx| m.install_hooks(cx)))
        } else {
            d
        }
    };
    let by_app = if can_install { "" } else { "\nInstall them from Taskboard.app." };
    let el = if m.hooks_busy {
        base.child("Installing hooks…")
    } else {
        match &m.hooks {
            Hooks::Hidden | Hooks::Checking => return None,
            Hooks::Current => base
                .child(kit::dot(t.up, 7.))
                .child("Hooks")
                .tooltip(kit::tip("The board's hooks are in Claude Code's settings, so sessions in Midna terminals report to the board.")),
            Hooks::NotInstalled => clickable(base.text_color(t.accent).child("Install hooks")).tooltip(kit::tip(format!(
                "Install the board's hooks and plugin in Claude Code so sessions in Midna terminals report to the board.{by_app}"
            ))),
            Hooks::NeedsReinstall(why) => clickable(base.text_color(t.warn_fg).child(kit::dot(t.warn, 7.)).child("Reinstall hooks"))
                .tooltip(kit::tip(format!("The hooks are out of date, so sessions may not report to the board.\n{why}{by_app}"))),
        }
    };
    Some(el.into_any_element())
}

/// `.conn.usage`: per window `.u-k` label, a 34×5 bar and the value; warm/hot recolour both.
pub(crate) fn usage_pill(t: &Theme, u: &UsageView) -> AnyElement {
    let mut row = pill_shell(t, "usage-pill", t.muted, t.seg)
        .gap(px(0.))
        .px(px(4.))
        .border_1()
        .border_color(gpui_kit::transparent_black())
        .when(u.stale, |d| d.opacity(0.6))
        .tooltip(kit::tip(u.title.clone()));
    for (n, w) in u.windows.iter().enumerate() {
        let (bar_c, val_c) = match w.level {
            "lifted" => (t.accent, t.accent),
            "hot" => (t.down, t.down),
            "warm" => (t.warn, t.warn_fg),
            _ => (t.up, t.text_2),
        };
        row = row.child(
            div()
                .flex()
                .items_center()
                .gap(px(6.))
                .px(px(7.))
                .when(n > 0, |d| d.border_l_1().border_color(t.border_2))
                .child(div().text_size(px(11.5)).font_weight(FontWeight::MEDIUM).text_color(t.faint).child(w.label.clone()))
                .child(div().w(px(34.)).h(px(5.)).rounded_full().overflow_hidden().bg(t.border_2).child(div().h_full().rounded_full().bg(bar_c).w(relative((w.width / 100.) as f32))))
                .child(div().min_w(px(22.)).flex().justify_end().text_color(val_c).when(w.level == "hot", |d| d.font_weight(FontWeight::SEMIBOLD)).child(w.value.clone())),
        );
    }
    row.into_any_element()
}

/// `.toast`: centred 24px above the bottom, 9px 16px, radius 9, 13px, `--shadow-modal`.
fn toasts(m: &MainWindow, t: &Theme, cx: &mut Context<MainWindow>) -> Option<AnyElement> {
    if m.toasts.is_empty() {
        return None;
    }
    let mut col = div().absolute().bottom(px(24.)).left_0().right_0().px(px(16.)).flex().flex_col().items_center().gap(px(6.));
    // (One at a time: `toast` replaces the last.)
    for (i, x) in m.toasts.iter().enumerate() {
        col = col.child(
            div()
                .id(("toast", i))
                .px(px(16.))
                .py(px(9.))
                .rounded(px(9.))
                .shadow(ui::kit::shadow_modal(t))
                .text_size(px(13.))
                .bg(if x.err { t.down } else { t.text })
                .text_color(if x.err { gpui_kit::white() } else { t.bg })
                .flex()
                .items_center()
                .gap(px(14.))
                .child(x.text.clone())
                .children(x.undo.clone().map(|path| {
                    let at = x.at;
                    div()
                        .id("toast-undo")
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(t.accent_soft)
                        .cursor_pointer()
                        .child("Undo")
                        .on_click(cx.listener(move |m, _, _, cx| {
                            m.toasts.retain(|t| t.at != at);
                            m.post(path.clone(), serde_json::json!({}), cx, |_, _, _| {});
                        }))
                })),
        );
    }
    Some(deferred(col).with_priority(4).into_any_element())
}

