//! The board's health: the sidebar footer (one summary line of what's wrong, the usage block,
//! work hours and Settings), the summary's popover, and the banners over the page while the board
//! is down or setup needs a hand (its login item, its port, linking `tb`, another board in the way).
use crate::app::{MainWindow, UsageView};
use crate::fmt::{self, s};
use crate::hooks::Hooks;
use crate::install::LoginItem;
use crate::theme::Theme;
use crate::ui::kit;
use crate::updater::State as Update;
use gpui_kit::prelude::*;
use gpui_kit::*;
use serde_json::Value;

/// The summary popover's key in [`MainWindow::menu`].
pub const MENU: &str = "footer-summary";

/// Red when something failed, orange when the owner has to act, blue for news.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Tone {
    Bad,
    Act,
    Info,
}

/// What a button in the summary does.
#[derive(Clone, Debug, PartialEq)]
pub enum Act {
    TryBoard,
    InstallHooks,
    DismissHooks,
    OpenAccounts,
    CheckUpdate,
    DismissUpdate,
    RestartToUpdate,
    WhatsNew(String),
    DismissLink,
    OpenLoginItems,
    Copy(String),
}

/// One thing wrong (or new), as the summary line and its popover show it.
#[derive(Clone, Debug, PartialEq)]
pub struct Item {
    pub tone: Tone,
    pub title: String,
    pub body: String,
    /// The buttons, the first one the main one.
    pub acts: Vec<(String, Act)>,
}

fn item(tone: Tone, title: impl Into<String>, body: impl Into<String>, acts: Vec<(&str, Act)>) -> Item {
    Item { tone, title: title.into(), body: body.into(), acts: acts.into_iter().map(|(l, a)| (l.to_string(), a)).collect() }
}

/// What the footer reads from: the window's state and the app's own checks.
pub struct Inputs<'a> {
    pub st: &'a Value,
    /// Why `GET /state` failed, and since when ("3:05 PM").
    pub down: Option<(&'a str, Option<&'a str>)>,
    pub hooks: &'a Hooks,
    pub hooks_busy: bool,
    pub hooks_failed: Option<crate::hooks::Failed>,
    pub can_install: bool,
    pub update: Update,
    pub restart_reviews: Option<usize>,
    pub link_failed: Option<String>,
    pub login: LoginItem,
    pub sample: bool,
}

/// Everything wrong, worst first: failures, then what's the owner's to do, then news.
pub fn items(i: &Inputs) -> Vec<Item> {
    let mut out = Vec::new();
    if let Some((why, at)) = i.down {
        let since = at.map(|a| format!(" at {a}")).unwrap_or_default();
        out.push(item(
            Tone::Bad,
            "The board isn’t answering",
            format!("It stopped answering{since}. {why} Trying again every few seconds."),
            vec![("Try again", Act::TryBoard), ("Copy details", Act::Copy(why.to_string()))],
        ));
    }
    if let Some(mid) = i.st.get("midna").filter(|v| v.is_object())
        && mid["up"] != true
    {
        let heard = fmt::opt_s(mid, "seen_at").map(|x| format!(" Last heard from {}.", fmt::hhmm(x))).unwrap_or_default();
        out.push(item(Tone::Bad, "Midna isn’t running", format!("Agents can’t start, and messages to terminals wait, until it’s back.{heard}"), vec![]));
    }
    if let Some(f) = &i.hooks_failed {
        out.push(item(
            Tone::Bad,
            format!("Couldn’t install hooks at {}", f.at),
            f.text.clone(),
            vec![("Try again", Act::InstallHooks), ("Copy details", Act::Copy(f.text.clone())), ("Dismiss", Act::DismissHooks)],
        ));
    }
    if let Update::Failed { stage, error, at } = &i.update {
        let detail = format!("{stage}: {error}");
        out.push(item(
            Tone::Bad,
            format!("{} at {at}", Update::failure_title(stage)),
            detail.clone(),
            vec![("Try again", Act::CheckUpdate), ("Copy details", Act::Copy(detail)), ("Dismiss", Act::DismissUpdate)],
        ));
    }
    if let Some(e) = &i.link_failed {
        out.push(item(
            Tone::Bad,
            "Couldn’t link the tb command",
            format!("{e}. Agents’ shells won’t find tb until it’s linked."),
            vec![("Copy details", Act::Copy(e.clone())), ("Dismiss", Act::DismissLink)],
        ));
    }
    if let LoginItem::Failed(e) = &i.login {
        let detail = format!("Registering the board as a login item: {e}");
        out.push(item(Tone::Bad, "The board couldn’t be set to start on its own", detail.clone(), vec![("Copy details", Act::Copy(detail))]));
    }
    if let Some(p) = i.st.get("pr_builds").and_then(crate::ui::prwatch::builds_pill) {
        out.push(item(Tone::Act, p.label, p.title, vec![]));
    }
    if let Some(p) = i.st.get("pr_feed").and_then(crate::ui::prwatch::feed_pill) {
        out.push(item(Tone::Act, p.label, p.title, vec![]));
    }
    if let LoginItem::RequiresApproval = i.login {
        out.push(item(
            Tone::Act,
            "Switch the board on in Login Items",
            "The board starts on its own as a login item. Switch on Taskboard in System Settings ▸ Login Items.",
            vec![("Open Login Items", Act::OpenLoginItems)],
        ));
    }
    if !i.hooks_busy && i.hooks_failed.is_none() {
        let by_app = if i.can_install { "" } else { " Install them from Taskboard.app." };
        let install = |label: &'static str| if i.can_install { vec![(label, Act::InstallHooks)] } else { vec![] };
        match i.hooks {
            Hooks::NotInstalled => out.push(item(
                Tone::Act,
                "Hooks aren’t installed",
                format!("Sessions in Midna terminals don’t report to the board until they are.{by_app}"),
                install("Install hooks"),
            )),
            Hooks::NeedsReinstall(why) => out.push(item(
                Tone::Act,
                "Hooks are out of date",
                format!("Sessions may not report to the board. {why}{by_app}"),
                install("Reinstall hooks"),
            )),
            _ => {}
        }
    }
    if let Some(list) = i.st.get("accounts").and_then(Value::as_array).filter(|l| !l.is_empty()) {
        let title = match list.as_slice() {
            [one] => format!("{}: {}", s(one, "label"), if one["reauth"] == true { "sign in again" } else { "check sign-in" }),
            many => format!("{} accounts need you", many.len()),
        };
        let body = list.iter().map(|a| format!("{}: {}", s(a, "label"), s(a, "reason"))).collect::<Vec<_>>().join("\n");
        out.push(item(Tone::Act, title, body, vec![("Open Accounts", Act::OpenAccounts)]));
    }
    if i.hooks_busy {
        out.push(item(Tone::Info, "Installing hooks…", "", vec![]));
    }
    if let Update::Ready { version, page } = &i.update {
        out.push(item(
            Tone::Info,
            format!("Taskboard {version} is ready"),
            crate::updater::restart_note(i.restart_reviews),
            vec![("Restart to update", Act::RestartToUpdate), ("What’s new", Act::WhatsNew(page.clone()))],
        ));
    }
    if i.sample {
        out.push(item(Tone::Info, "Sample board", "This window shows sample data, not your board.", vec![]));
    }
    out.sort_by_key(|x| x.tone);
    out
}

/// The summary line: the worst item's title and "+N more", or the all-clear.
pub fn summary(items: &[Item], midna_up: bool) -> (Option<Tone>, String) {
    match items {
        [] if midna_up => (None, "Midna running".into()),
        [] => (None, "The board running".into()),
        [one] => (Some(one.tone), one.title.clone()),
        [first, rest @ ..] => (Some(first.tone), format!("{} · +{} more", first.title, rest.len())),
    }
}

fn tone_colors(t: &Theme, tone: Option<Tone>) -> (Hsla, Hsla) {
    match tone {
        Some(Tone::Bad) => (t.down, t.down),
        Some(Tone::Act) => (t.warn, t.warn_fg),
        Some(Tone::Info) => (t.accent, t.accent),
        None => (t.up, t.muted),
    }
}

fn inputs(m: &MainWindow) -> (Value, Option<(String, Option<String>)>) {
    let st = (*m.state_rc()).clone();
    (st, m.down.clone().map(|d| (d, m.down_at.clone())))
}

fn current(m: &MainWindow) -> (Vec<Item>, bool) {
    let (st, down) = inputs(m);
    let midna_up = st.get("midna").is_some_and(|v| v["up"] == true);
    let i = Inputs {
        st: &st,
        down: down.as_ref().map(|(d, at)| (d.as_str(), at.as_deref())),
        hooks: &m.hooks,
        hooks_busy: m.hooks_busy,
        hooks_failed: crate::hooks::failed(),
        can_install: crate::hooks::target().is_some_and(|t| t.can_install),
        update: crate::updater::state(),
        restart_reviews: crate::updater::restart_reviews(),
        link_failed: crate::install::link_failed(),
        login: crate::install::status(),
        sample: m.backend.label() == "fake",
    };
    (items(&i), midna_up)
}

/// Ask the board which code reviews a restart would start again, for the ready update's note.
fn learn_restart(m: &MainWindow, cx: &mut Context<MainWindow>) {
    let backend = m.backend.clone();
    cx.spawn(async move |this, cx| {
        let v = cx.background_executor().spawn(async move { backend.get("restart", &[]).ok() }).await;
        crate::updater::learn_restart(v.as_ref());
        let _ = this.update(cx, |_, cx| cx.notify());
    })
    .detach();
}

/// The sidebar's foot: the summary line, the usage block and work hours with Settings. Three rows at most.
pub fn footer(m: &mut MainWindow, t: &Theme, cx: &mut Context<MainWindow>) -> Div {
    let (items, midna_up) = current(m);
    let (tone, text) = summary(&items, midna_up);
    let (dot, fg) = tone_colors(t, tone);
    let tip = items.iter().map(|x| x.title.clone()).collect::<Vec<_>>().join("\n");
    let open = m.menu_open(MENU).is_some();
    let hover = t.panel_2;
    let row1 = div()
        .id("footer-summary")
        .relative()
        .flex()
        .items_center()
        .gap(px(8.))
        .w_full()
        .min_h(px(28.))
        .px(px(6.))
        .rounded(px(6.))
        .text_size(px(12.5))
        .text_color(fg)
        .when(open, |d| d.bg(hover))
        .child(kit::dot(dot, 8.))
        .child(div().flex_1().min_w_0().truncate().child(text))
        .child(kit::anchor(MENU))
        .when(!items.is_empty(), |d| {
            d.cursor_pointer()
                .hover(move |s| s.bg(hover))
                .tooltip(kit::tip(tip))
                .on_click(cx.listener(|m, e: &ClickEvent, _, cx| {
                    if m.menu_open(MENU).is_none() && matches!(crate::updater::state(), Update::Ready { .. }) {
                        learn_restart(m, cx);
                    }
                    m.toggle_menu(MENU, e.position(), cx)
                }))
        });
    let streams = crate::ui::prwatch::streams_pill(&m.state(), t);
    let row1 = div().flex().items_center().gap(px(6.)).child(div().flex_1().min_w_0().child(row1)).children(streams);
    let mut col = div().flex().flex_col().gap(px(6.)).pt(px(8.)).border_t_1().border_color(t.border).child(row1);
    if let Some(u) = m.state().get("usage").and_then(crate::app::usage_view) {
        let seen = fmt::opt_s(&m.state()["usage"], "seen_at").map(fmt::hhmm);
        col = col.child(usage_block(t, &u, seen));
    }
    let text = t.text;
    col.child(
        div()
            .flex()
            .items_center()
            .gap(px(6.))
            .child(div().flex_1().min_w_0().flex().child(crate::ui::hours::pill(m, t, cx)))
            .child(
                div()
                    .id("open-settings")
                    .flex()
                    .flex_none()
                    .items_center()
                    .gap(px(6.))
                    .h(px(25.))
                    .px(px(4.))
                    .text_size(px(12.5))
                    .text_color(t.muted)
                    .cursor_pointer()
                    .hover(move |s| s.text_color(text))
                    .tooltip(kit::tip("Settings (⌘,)"))
                    .on_click(|_, window, cx| window.dispatch_action(Box::new(crate::OpenSettings), cx))
                    .child("Settings")
                    .child(div().font_family(t.mono_font.clone()).text_size(px(11.)).text_color(t.faint).child("⌘,")),
            ),
    )
}

/// The usage reading as a full-width block: a line per window and how old the reading is. Opens
/// Settings › Usage.
fn usage_block(t: &Theme, u: &UsageView, seen: Option<String>) -> Stateful<Div> {
    let line = t.border_2;
    let mut b = div()
        .id("usage-block")
        .flex()
        .flex_col()
        .gap(px(4.))
        .w_full()
        .p(px(8.))
        .rounded(px(8.))
        .bg(t.seg)
        .border_1()
        .border_color(gpui_kit::transparent_black())
        .cursor_pointer()
        .hover(move |s| s.border_color(line))
        .tooltip(kit::tip(u.title.clone()))
        .on_click(|_, window, cx| window.dispatch_action(Box::new(crate::OpenUsageSettings), cx));
    for w in &u.windows {
        let (bar_c, val_c) = match w.level {
            "lifted" => (t.accent, t.accent),
            "hot" => (t.down, t.down),
            "warm" => (t.warn, t.warn_fg),
            _ => (t.up, t.text_2),
        };
        b = b.child(
            div()
                .flex()
                .items_center()
                .gap(px(8.))
                .text_size(px(11.5))
                .child(div().w(px(44.)).flex_none().font_weight(FontWeight::MEDIUM).text_color(t.faint).child(w.label.clone()))
                .child(div().flex_1().h(px(5.)).rounded_full().overflow_hidden().bg(t.border_2).child(div().h_full().rounded_full().bg(bar_c).w(relative((w.width / 100.) as f32))))
                .child(div().min_w(px(30.)).flex().justify_end().text_color(val_c).when(w.level == "hot", |d| d.font_weight(FontWeight::SEMIBOLD)).child(w.value.clone())),
        );
    }
    if let Some(at) = seen.filter(|a| !a.is_empty()) {
        b = b.child(div().text_size(px(11.)).text_color(if u.stale { t.warn_fg } else { t.faint }).child(if u.stale { format!("Old reading, as of {at}") } else { format!("As of {at}") }));
    }
    b
}

/// The summary's popover: every item with its words and buttons.
pub fn render_menu(m: &mut MainWindow, cx: &mut Context<MainWindow>) -> Option<AnyElement> {
    let at = m.menu_open(MENU)?;
    let (items, _) = current(m);
    if items.is_empty() {
        m.menu = None;
        return None;
    }
    let t = cx.global::<Theme>().clone();
    let head = if items.len() == 1 { "1 thing needs a look".to_string() } else { format!("{} things need a look", items.len()) };
    let mut list = kit::menu_box(&t, 320.)
        .id("footer-summary-menu")
        .max_h(px(420.))
        .overflow_y_scroll()
        .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
        .child(div().px(px(8.)).py(px(6.)).text_size(px(11.)).font_weight(FontWeight::SEMIBOLD).text_color(t.faint).child(head.to_uppercase()));
    for (n, it) in items.into_iter().enumerate() {
        let (dot, fg) = tone_colors(&t, Some(it.tone));
        let has_btns = !it.acts.is_empty();
        let mut btns = div().flex().flex_wrap().gap(px(6.)).pt(px(4.));
        for (k, (label, act)) in it.acts.into_iter().enumerate() {
            let id = SharedString::from(format!("health-{n}-{k}"));
            let b = if k == 0 && matches!(act, Act::RestartToUpdate | Act::TryBoard | Act::InstallHooks) { kit::btn_primary(&t, id, label) } else { kit::btn_small(&t, id, label) };
            btns = btns.child(b.on_click(cx.listener(move |m, _, window, cx| run(m, &act, window, cx))));
        }
        list = list.child(
            div()
                .flex()
                .flex_col()
                .gap(px(2.))
                .px(px(8.))
                .py(px(8.))
                .when(n > 0, |d| d.border_t_1().border_color(t.border))
                .child(div().flex().items_center().gap(px(8.)).text_size(px(13.)).font_weight(FontWeight::MEDIUM).text_color(fg).child(kit::dot(dot, 8.)).child(it.title))
                .when(!it.body.is_empty(), |d| d.child(div().pl(px(16.)).text_size(px(12.)).text_color(t.muted).child(it.body)))
                .when(has_btns, |d| d.child(btns.pl(px(16.)))),
        );
    }
    Some(kit::menu_under(MENU, kit::Align::Left, at, list))
}

/// A summary button.
pub fn run(m: &mut MainWindow, act: &Act, window: &mut Window, cx: &mut Context<MainWindow>) {
    match act {
        Act::TryBoard => m.refresh(cx),
        Act::InstallHooks => {
            crate::hooks::dismiss();
            m.install_hooks(cx);
        }
        Act::DismissHooks => crate::hooks::dismiss(),
        Act::OpenAccounts => window.dispatch_action(Box::new(crate::OpenSettings), cx),
        Act::CheckUpdate => {
            crate::updater::dismiss();
            crate::updater::check_now();
        }
        Act::DismissUpdate => crate::updater::dismiss(),
        Act::RestartToUpdate => restart_to_update(m, cx),
        Act::WhatsNew(page) => cx.open_url(page),
        Act::DismissLink => crate::install::dismiss_link_failure(),
        Act::OpenLoginItems => crate::install::open_login_items(),
        Act::Copy(text) => crate::app::copy_text(m, text, cx),
    }
    if !matches!(act, Act::Copy(_) | Act::WhatsNew(_)) {
        m.menu = None;
    }
    cx.notify();
}

/// Restart to update: swap the new app in and quit; the new app opens itself. A failure stays in
/// the footer (the updater keeps it).
fn restart_to_update(m: &mut MainWindow, cx: &mut Context<MainWindow>) {
    m.menu = None;
    cx.spawn(async move |this, cx| {
        let r = cx.background_executor().spawn(async move { crate::updater::apply() }).await;
        let _ = this.update(cx, |_, cx| match r {
            Ok(()) => cx.quit(),
            Err(_) => cx.notify(),
        });
    })
    .detach();
}

// ------------------------------------------------------------------ banners

/// The banners over the page: the board down (with when, and that what shows is from then), then
/// setup that needs a hand.
pub fn banners(m: &MainWindow, t: &Theme, cx: &mut Context<MainWindow>) -> Vec<AnyElement> {
    let mut out = Vec::new();
    if let Some(why) = m.down.clone() {
        out.push(down_card(t, &why, m.down_at.as_deref(), m.data.state.is_some(), cx));
    }
    out.extend(setup_rows(t, cx));
    out
}

/// What the board-down card says: its title and its line.
pub fn down_words(at: Option<&str>, have_state: bool) -> (String, String) {
    let line = match (at, have_state) {
        (Some(at), true) => format!("Trying again every few seconds. What you see is from {at}, and actions wait until it’s back."),
        (None, true) => "Trying again every few seconds. What you see is from before it stopped, and actions wait until it’s back.".into(),
        (_, false) => "Trying again every few seconds. There’s nothing to show until it answers.".into(),
    };
    ("The board isn’t answering".into(), line)
}

fn down_card(t: &Theme, why: &str, at: Option<&str>, have_state: bool, cx: &mut Context<MainWindow>) -> AnyElement {
    let (title, line) = down_words(at, have_state);
    let detail = match at {
        Some(at) => format!("The board stopped answering at {at}: {why}"),
        None => format!("The board isn’t answering: {why}"),
    };
    div()
        .flex_none()
        .mx(px(40.))
        .mt(px(8.))
        .mb(px(4.))
        .px(px(16.))
        .py(px(12.))
        .rounded(px(10.))
        .bg(t.down_soft)
        .border_1()
        .border_color(t.down_line)
        .flex()
        .items_center()
        .gap(px(12.))
        .child(
            div()
                .flex_1()
                .min_w_0()
                .flex()
                .flex_col()
                .gap(px(2.))
                .child(div().text_size(px(13.)).font_weight(FontWeight::SEMIBOLD).text_color(t.down).child(title))
                .child(div().text_size(px(12.5)).text_color(t.down).child(line)),
        )
        .child(kit::btn_small(t, "down-retry", "Try again").on_click(cx.listener(|m, _, _, cx| m.refresh(cx))))
        .child(kit::btn_small(t, "down-system", "Open System").on_click(|_, window, cx| window.dispatch_action(Box::new(crate::OpenSystemSettings), cx)))
        .child(kit::btn_small(t, "down-copy", "Copy details").on_click(cx.listener(move |m, _, _, cx| crate::app::copy_text(m, &detail, cx))))
        .into_any_element()
}

/// A setup row's words: (orange when it's the owner's to do, else red; the line).
pub fn setup_words(login: &LoginItem, port: Option<String>) -> Option<(bool, String)> {
    match login {
        LoginItem::RequiresApproval => Some((true, "The board starts on its own as a login item. Switch on Taskboard in System Settings ▸ Login Items.".into())),
        LoginItem::Blocked => {
            let held = port.unwrap_or_else(|| "Another program holds the board’s port.".into());
            Some((true, format!("{held} The board can’t start until it’s free. Quit it; the app looks again on its own.")))
        }
        LoginItem::Failed(e) => Some((false, format!("The board couldn’t be set to start on its own. Registering it as a login item: {e}"))),
        _ => None,
    }
}

fn setup_rows(t: &Theme, cx: &mut Context<MainWindow>) -> Vec<AnyElement> {
    let mut out = Vec::new();
    let login = crate::install::status();
    if let Some((act, text)) = setup_words(&login, crate::install::port_line()) {
        let row = if act { crate::app::bar(t.warn_soft, t.warn_text, t.warn_line).child(kit::dot(t.warn, 8.)) } else { crate::app::bar(t.down_soft, t.down, t.down_line).child(kit::dot(t.down, 8.)) };
        let copy = text.clone();
        out.push(
            row.child(div().flex_1().min_w_0().child(text))
                .when(login == LoginItem::RequiresApproval, |d| {
                    d.child(kit::btn_small(t, "open-login-items", "Open Login Items").on_click(|_, _, _| crate::install::open_login_items()))
                })
                .child(crate::app::banner_btn(t, "setup-copy", "Copy details", false).on_click(cx.listener(move |m, _, _, cx| crate::app::copy_text(m, &copy, cx))))
                .into_any_element(),
        );
    }
    if let Some(e) = crate::install::link_failed() {
        let copy = e.clone();
        out.push(
            crate::app::bar(t.down_soft, t.down, t.down_line)
                .child(kit::dot(t.down, 8.))
                .child(div().flex_1().min_w_0().child(format!("Couldn’t link the tb command. {e}.")))
                .child(crate::app::banner_btn(t, "link-copy", "Copy details", false).on_click(cx.listener(move |m, _, _, cx| crate::app::copy_text(m, &copy, cx))))
                .child(crate::app::banner_btn(t, "link-dismiss", "Dismiss", false).on_click(cx.listener(|_, _, _, cx| {
                    crate::install::dismiss_link_failure();
                    cx.notify();
                })))
                .into_any_element(),
        );
    }
    for p in crate::install::in_the_way() {
        out.push(crate::app::bar(t.warn_soft, t.warn_text, t.warn_line).child(kit::dot(t.warn, 8.)).child(div().flex_1().min_w_0().child(p)).into_any_element());
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn base(st: &Value) -> Inputs<'_> {
        Inputs {
            st,
            down: None,
            hooks: &Hooks::Current,
            hooks_busy: false,
            hooks_failed: None,
            can_install: true,
            update: Update::Idle,
            restart_reviews: None,
            link_failed: None,
            login: LoginItem::Enabled,
            sample: false,
        }
    }

    #[::core::prelude::v1::test]
    fn all_clear_names_what_is_running() {
        let st = json!({"midna": {"up": true}});
        let list = items(&base(&st));
        assert!(list.is_empty());
        assert_eq!(summary(&list, true), (None, "Midna running".into()));
    }

    #[::core::prelude::v1::test]
    fn a_stopped_board_reads_as_stopped_and_comes_first() {
        let st = json!({"midna": {"up": true}, "accounts": [{"label": "GitHub", "reauth": true, "reason": "token expired"}]});
        let mut i = base(&st);
        i.down = Some(("The board isn’t answering.", Some("3:05 PM")));
        i.update = Update::Ready { version: "1.2.0".into(), page: "https://x".into() };
        let list = items(&i);
        assert_eq!(list[0].tone, Tone::Bad);
        assert_eq!(list[0].title, "The board isn’t answering");
        assert!(list[0].body.contains("at 3:05 PM"));
        assert_eq!(list[1].title, "GitHub: sign in again");
        assert_eq!(list[1].tone, Tone::Act);
        assert_eq!(list[2].title, "Taskboard 1.2.0 is ready");
        assert_eq!(summary(&list, true).1, "The board isn’t answering · +2 more");
    }

    #[::core::prelude::v1::test]
    fn failures_stay_with_their_stage_and_copy_details() {
        let st = json!({});
        let mut i = base(&st);
        i.hooks_failed = Some(crate::hooks::Failed { text: "Finding Claude Code: the claude command isn’t on this Mac.".into(), at: "3:05 PM".into() });
        i.hooks = &Hooks::NotInstalled;
        i.update = Update::Failed { stage: "Downloading 1.2.0".into(), error: "timed out".into(), at: "2:51 PM".into() };
        i.link_failed = Some("Linking ~/.local/bin/tb: permission denied".into());
        let list = items(&i);
        let titles: Vec<&str> = list.iter().map(|x| x.title.as_str()).collect();
        assert_eq!(titles, vec!["Couldn’t install hooks at 3:05 PM", "Couldn’t download 1.2.0 at 2:51 PM", "Couldn’t link the tb command"]);
        assert!(list.iter().all(|x| x.tone == Tone::Bad));
        assert!(list.iter().all(|x| x.acts.iter().any(|(l, a)| l == "Copy details" && matches!(a, Act::Copy(_)))));
        assert_eq!(list[1].body, "Downloading 1.2.0: timed out");
        assert!(list[0].acts.iter().any(|(l, _)| l == "Dismiss"));
    }

    #[::core::prelude::v1::test]
    fn an_update_shows_only_once_it_is_ready_and_says_what_keeps_running() {
        let st = json!({});
        let mut i = base(&st);
        i.update = Update::Downloading { version: "9.0.0".into() };
        assert!(items(&i).is_empty(), "not while it downloads");
        i.update = Update::Ready { version: "9.0.0".into(), page: String::new() };
        i.restart_reviews = Some(2);
        let list = items(&i);
        assert_eq!(list[0].tone, Tone::Info);
        assert!(list[0].body.starts_with("Agents keep running in Midna."), "{}", list[0].body);
        assert!(list[0].body.contains("2 code reviews start again"), "{}", list[0].body);
        assert_eq!(list[0].acts[0], ("Restart to update".to_string(), Act::RestartToUpdate));
    }

    #[::core::prelude::v1::test]
    fn midna_down_and_hooks_to_install_say_what_they_hold_up() {
        let st = json!({"midna": {"up": false}});
        let mut i = base(&st);
        i.hooks = &Hooks::NotInstalled;
        let list = items(&i);
        assert_eq!(list[0].title, "Midna isn’t running");
        assert_eq!(list[0].tone, Tone::Bad);
        assert_eq!(list[1].title, "Hooks aren’t installed");
        assert_eq!(list[1].acts, vec![("Install hooks".to_string(), Act::InstallHooks)]);
        assert_eq!(summary(&list, false).0, Some(Tone::Bad));
    }

    #[::core::prelude::v1::test]
    fn the_down_card_says_when_and_that_what_shows_is_from_then() {
        let (title, line) = down_words(Some("3:09 PM"), true);
        assert_eq!(title, "The board isn’t answering");
        assert_eq!(line, "Trying again every few seconds. What you see is from 3:09 PM, and actions wait until it’s back.");
        assert!(down_words(None, false).1.contains("nothing to show"));
    }

    #[::core::prelude::v1::test]
    fn setup_rows_say_the_board_and_name_the_port_holder() {
        let (act, text) = setup_words(&LoginItem::Blocked, Some("Port 8792 is held by python3 (pid 812).".into())).unwrap();
        assert!(act);
        assert!(text.starts_with("Port 8792 is held by python3 (pid 812). The board can’t start"));
        let (act, text) = setup_words(&LoginItem::Failed("denied".into()), None).unwrap();
        assert!(!act);
        assert!(text.contains("Registering it as a login item: denied"));
        for l in [LoginItem::RequiresApproval, LoginItem::Blocked, LoginItem::Failed("x".into())] {
            let text = setup_words(&l, None).unwrap().1;
            assert!(!text.contains("background service") && !text.contains("daemon"), "{text}");
        }
        assert_eq!(setup_words(&LoginItem::Enabled, None), None);
    }
}
