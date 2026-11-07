// Excerpt of crates/taskboard-app/src/app.rs, lines 1931-2089
fn banner_btn(t: &Theme, id: impl Into<ElementId>, label: impl Into<SharedString>, soft: bool) -> Stateful<Div> {
    let text = t.text;
    div()
        .id(id)
        .flex()
        .flex_none()
        .items_center()
        .justify_center()
        .h(px(32.))
        .px(px(10.))
        .rounded(px(7.))
        .text_size(px(12.5))
        .whitespace_nowrap()
        .cursor_pointer()
        .when(soft, |d| d.bg(t.accent_soft).text_color(t.accent).font_weight(FontWeight::SEMIBOLD).hover(|s| s.opacity(0.9)))
        .when(!soft, |d| d.text_color(t.muted).hover(move |s| s.text_color(text)))
        .child(label.into())
}

/// The banner lines: the board down, the login item (native), master and PR-build problems. The
/// alerts are the home page's Needs you list.
fn banner(m: &MainWindow, t: &Theme, cx: &mut Context<MainWindow>) -> Vec<AnyElement> {
    let mut out = Vec::new();
    let rows = banner_view(m.down.as_deref(), &[]);
    for r in rows.iter().filter(|r| r.kind == "down") {
        let text = r.text.clone();
        out.push(
            bar(t.down_soft, t.down, t.down_line)
                .child(kit::dot(t.down, 8.))
                .child(div().flex_1().min_w_0().child(r.text.clone()))
                .child(banner_btn(t, "down-copy", "Copy", false).on_click(cx.listener(move |m, _, _, cx| copy_text(m, &text, cx))))
                .into_any_element(),
        );
    }
    match crate::install::status() {
        crate::install::LoginItem::RequiresApproval => out.push(
            bar(t.warn_soft, t.warn_text, t.warn_line)
                .child(kit::dot(t.warn, 8.))
                .child(div().flex_1().min_w_0().child("The board runs in the background as a login item. Switch on Taskboard in System Settings ▸ Login Items."))
                .child(kit::btn_small(t, "open-login-items", "Open Login Items").on_click(|_, _, _| crate::install::open_login_items()))
                .into_any_element(),
        ),
        crate::install::LoginItem::Blocked => out.push(
            bar(t.warn_soft, t.warn_text, t.warn_line)
                .child(kit::dot(t.warn, 8.))
                .child(div().flex_1().min_w_0().child("Another board holds the port, so this one didn't start its background service or link tb. Stop the other board, then open Taskboard again."))
                .into_any_element(),
        ),
        crate::install::LoginItem::Failed(e) => out.push(
            bar(t.warn_soft, t.warn_text, t.warn_line)
                .child(kit::dot(t.warn, 8.))
                .child(div().flex_1().min_w_0().child(format!("The board's background service couldn't be registered: {e}")))
                .into_any_element(),
        ),
        _ => {}
    }
    for p in crate::install::in_the_way() {
        out.push(bar(t.warn_soft, t.warn_text, t.warn_line).child(kit::dot(t.warn, 8.)).child(div().flex_1().min_w_0().child(p)).into_any_element());
    }
    out.extend(ui::prwatch::banner(m, t, cx));
    out
}

/// The alert's text, then its age in `ago` (the muted color, or the row's own on a solid red one).
fn alert_text(r: &BannerRow, ago: Hsla) -> Div {
    div()
        .flex_1()
        .min_w_0()
        .flex()
        .flex_wrap()
        .items_baseline()
        .gap(px(4.))
        .child(r.text.clone())
        .children(r.ago.clone().map(|a| div().ml(px(4.)).text_size(px(10.8)).text_color(ago).child(a)))
}

/// One alert: its text and age, "Open T12" and Dismiss. Also used by the alerts dialog.
pub fn alert_row(t: &Theme, a: &Value, cx: &mut Context<MainWindow>) -> Div {
    let id = s(a, "id").to_string();
    let r = alert_row_view(a);
    let (task, goal) = (fmt::opt_s(a, "task").map(str::to_string), fmt::opt_s(a, "goal").map(str::to_string));
    let fixes = alert_actions(a).into_iter().enumerate().map(|(n, (label, command))| {
        let aid = id.clone();
        banner_btn(t, SharedString::from(format!("alert-act-{id}-{n}")), label.clone(), true)
            .when(a["urgent"] == true, |d| d.text_color(t.on_accent))
            .on_click(cx.listener(move |m, _, _, cx| run_alert_action(m, &aid, n, &label, &command, cx)))
    });
    let fixes: Vec<_> = fixes.collect();
    // An urgent alert is solid red; the others are red on the soft red.
    let urgent = r.kind == "urgent";
    let row = if urgent { bar(t.down, t.on_accent, t.down).font_weight(FontWeight::SEMIBOLD) } else { bar(t.down_soft, t.down, t.down_line) };
    row.child(kit::dot(if urgent { t.on_accent } else { t.down }, 8.))
        .child(alert_text(&r, if urgent { t.on_accent } else { t.muted }))
        .children(r.buttons.iter().filter(|(act, _)| *act == "alert-open").map(|(_, label)| {
            let (task, goal) = (task.clone(), goal.clone());
            banner_btn(t, SharedString::from(format!("alert-open-{id}")), label.clone(), true).on_click(cx.listener(move |m, _, window, cx| {
                open_alert(m, task.as_deref(), goal.as_deref(), window, cx);
            }))
        }))
        .children(fixes)
        .child({
            let text = r.text.clone();
            banner_btn(t, SharedString::from(format!("alert-copy-{id}")), "Copy", false)
                .when(urgent, |d| d.text_color(t.on_accent))
                .on_click(cx.listener(move |m, _, _, cx| copy_text(m, &text, cx)))
        })
        .when(r.buttons.iter().any(|(act, _)| *act == "alert-dismiss"), |d| {
            d.child(banner_btn(t, SharedString::from(format!("alert-dismiss-{id}")), "Dismiss", false).on_click(cx.listener(move |m, _, _, cx| dismiss_alert(m, &id, cx))))
        })
}

/// The banner's Copy: its text on the clipboard, and a toast.
pub fn copy_text(m: &mut MainWindow, text: &str, cx: &mut Context<MainWindow>) {
    cx.write_to_clipboard(ClipboardItem::new_string(text.to_string()));
    m.toast("Copied".to_string(), false, cx);
}

/// ⌘⇧C: what's open on the clipboard, to hand to a session. The side panel's task or issue wins
/// over the goal page under it; elsewhere there's nothing to copy.
pub fn open_ref(m: &MainWindow) -> Option<&str> {
    match (&m.panel, &m.page) {
        (Some(Panel::Task { r, .. } | Panel::Issue { r }), _) | (None, Page::Goal(r)) => Some(r),
        _ => None,
    }
}

fn copy_ref(m: &mut MainWindow, cx: &mut Context<MainWindow>) {
    let Some(r) = open_ref(m).map(str::to_string) else { return };
    cx.write_to_clipboard(ClipboardItem::new_string(r.clone()));
    m.toast(format!("Copied {r}"), false, cx);
}

/// `alert-open`: close the alerts dialog, then the task (or else the goal).
pub fn open_alert(m: &mut MainWindow, task: Option<&str>, goal: Option<&str>, window: &mut Window, cx: &mut Context<MainWindow>) {
    if matches!(m.modal, Some(ui::modals::Modal::Alerts)) {
        m.set_modal(None, window, cx);
    }
    if let Some(t) = task {
        m.open_task(t.to_string(), cx);
    } else if let Some(g) = goal {
        m.go(Page::Goal(g.to_string()), cx);
    }
}

/// `alert-dismiss`: `POST /alerts/:id/dismiss {}` (no toast).
pub fn dismiss_alert(m: &mut MainWindow, id: &str, cx: &mut Context<MainWindow>) {
    m.post(format!("alerts/{id}/dismiss"), json!({}), cx, |_, _, _| {});
}

/// `alerts-dismiss-all`: dismiss every current alert; the dialog closes once none are left.
/// (For the dialog in modals.rs.)
pub fn dismiss_all_alerts(m: &mut MainWindow, cx: &mut Context<MainWindow>) {
    let ids: Vec<String> = arr(m.state(), "alerts").iter().filter(|a| !alert_stays(a)).map(|a| s(a, "id").to_string()).collect();
    for id in ids {
        dismiss_alert(m, &id, cx);
    }
}

/// `.conn`: 12.5px, 3px 10px, fully rounded, 6px gaps (line height 1.5).
