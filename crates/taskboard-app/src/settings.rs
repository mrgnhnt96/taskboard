//! Settings: its own standard macOS window, laid out like Midna's. A sidebar of sections beside the
//! selected section's rows, grouped in cards; a row is its name, what it does and its control.
//! "Agent commands" adds under the rows the exact command an agent would run (copyable).
//!
//! Accounts is the one section so far: GitHub, Bitbucket and Slack, so the board and its agents can
//! check PRs, comment, push and assign reviewers. The daemon owns them (`taskboardd::accounts`):
//! GitHub signs in through `gh` (browser code or a pasted token), Bitbucket and Slack take a token
//! that the daemon checks with the service and keeps in the Keychain. Tokens go to the daemon once
//! and never come back to the window.
use crate::backend::Backend;
use crate::theme::Theme;
use crate::ui::kit::{self, Input, KeyOutcome};
use gpui_kit::prelude::*;
use gpui_kit::*;
use serde_json::{Value, json};
use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

struct SettingsHandle(Option<WindowHandle<SettingsWindow>>);
impl Global for SettingsHandle {}

/// Open (or bring forward) the Settings window.
pub fn open(backend: Arc<dyn Backend>, cx: &mut App) -> Option<WindowHandle<SettingsWindow>> {
    if let Some(h) = cx.try_global::<SettingsHandle>().and_then(|g| g.0)
        && h.update(cx, |_, w, _| w.activate_window()).is_ok()
    {
        return Some(h);
    }
    let w: f32 = std::env::var("TASKBOARD_SETTINGS_W").ok().and_then(|v| v.parse().ok()).unwrap_or(900.);
    let h: f32 = std::env::var("TASKBOARD_SETTINGS_H").ok().and_then(|v| v.parse().ok()).unwrap_or(680.);
    let opts = WindowOptions {
        window_bounds: Some(WindowBounds::Windowed(Bounds::centered(None, size(px(w), px(h)), cx))),
        titlebar: Some(TitlebarOptions { title: Some("Settings".into()), appears_transparent: true, traffic_light_position: Some(point(px(16.), px(17.))) }),
        window_min_size: Some(size(px(720.), px(460.))),
        app_id: Some("com.mrgnhnt.taskboard".into()),
        focus: std::env::var("TASKBOARD_NO_ACTIVATE").is_err(),
        ..Default::default()
    };
    let h = cx.open_window(opts, |window, cx| cx.new(|cx| SettingsWindow::new(backend, window, cx))).ok()?;
    cx.set_global(SettingsHandle(Some(h)));
    Some(h)
}

/// The sidebar's sections, top to bottom.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Sec {
    Accounts,
}

const SECS: [Sec; 1] = [Sec::Accounts];

impl Sec {
    fn label(self) -> &'static str {
        match self {
            Sec::Accounts => "Accounts",
        }
    }

    fn about(self) -> &'static str {
        match self {
            Sec::Accounts => "Sign in once. The board and its agents use these to check PRs, comment, push and assign reviewers.",
        }
    }

    fn glyph(self) -> &'static str {
        match self {
            Sec::Accounts => "@",
        }
    }
}

/// Each account's card: id, a short mark and its color, and what it's used for.
const CARDS: [(&str, &str, u32, &str); 3] = [
    ("github", "GH", 0x24292f, "PR checks and reviews, comments, reviewers and git push, through the GitHub CLI"),
    ("bitbucket", "BB", 0x2684ff, "Pull requests, comments, reviewers and git push over HTTPS"),
    ("slack", "S", 0x4a154b, "Posting and reading messages for the board and its agents"),
];

/// What a row's agent line shows, per account.
fn agent_cli(id: &str) -> &'static str {
    match id {
        "github" => "gh pr view 12 --comments   # or: tb api github repos/OWNER/REPO/pulls/12/requested_reviewers -d '{\"reviewers\":[\"sam\"]}'",
        "bitbucket" => "tb api bitbucket repositories/WORKSPACE/REPO/pullrequests/12/comments -d '{\"content\":{\"raw\":\"Looks good\"}}'",
        _ => "tb api slack chat.postMessage -d '{\"channel\":\"#dev\",\"text\":\"PR is ready\"}'",
    }
}

const POLL: Duration = Duration::from_secs(2);
/// Polls between asks when no sign-in is waiting (`gh auth status` is cached by the daemon anyway).
const IDLE_TICKS: u32 = 15;

pub struct SettingsWindow {
    backend: Arc<dyn Backend>,
    focus: FocusHandle,
    scroll: ScrollHandle,
    view: Sec,
    accounts: Vec<Value>,
    loaded: bool,
    /// Why the daemon couldn't be asked.
    error: Option<String>,
    /// The account with a call in flight.
    busy: Option<&'static str>,
    /// A line under an account's card: (text, is an error).
    notes: HashMap<&'static str, (String, bool)>,
    /// Show the commands agents run.
    cli: bool,
    copied: Option<(String, Instant)>,
    /// GitHub's "Use a token" form is open.
    gh_form: bool,
    gh_token: Input,
    bb_email: Input,
    bb_token: Input,
    slack_token: Input,
    /// The sign-in code already copied (so a poll doesn't copy it again).
    opened_code: Option<String>,
    /// GitHub's device page was opened for this code.
    browser_opened: bool,
}

impl SettingsWindow {
    fn new(backend: Arc<dyn Backend>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let focus = cx.focus_handle();
        window.focus(&focus, cx);
        let s = SettingsWindow {
            backend,
            focus,
            scroll: ScrollHandle::new(),
            view: Sec::Accounts,
            accounts: vec![],
            loaded: false,
            error: None,
            busy: None,
            notes: HashMap::new(),
            cli: false,
            copied: None,
            gh_form: false,
            gh_token: Input::secret(cx, "ghp_… or github_pat_…"),
            bb_email: Input::new(cx, "you@company.com", false),
            bb_token: Input::secret(cx, "ATATT…"),
            slack_token: Input::secret(cx, "xoxp-… or xoxb-…"),
            opened_code: None,
            browser_opened: false,
        };
        s.load(false, cx);
        cx.spawn(async move |this, cx| {
            let mut tick = 0u32;
            loop {
                cx.background_executor().timer(POLL).await;
                tick += 1;
                let Ok(waiting) = this.update(cx, |s, _| s.login().is_some()) else { break };
                if waiting || tick % IDLE_TICKS == 0 {
                    if this.update(cx, |s, cx| s.load(false, cx)).is_err() {
                        break;
                    }
                }
            }
        })
        .detach();
        s
    }

    fn account(&self, id: &str) -> Value {
        self.accounts.iter().find(|a| a["id"] == id).cloned().unwrap_or(Value::Null)
    }

    /// GitHub's browser sign-in, while it waits: (code, url).
    fn login(&self) -> Option<(String, String)> {
        let l = &self.account("github")["login"];
        Some((l["code"].as_str()?.to_string(), l["url"].as_str().unwrap_or(taskboardd::accounts::GITHUB_DEVICE_URL).to_string()))
    }

    fn load(&self, fresh: bool, cx: &mut Context<Self>) {
        let backend = self.backend.clone();
        cx.spawn(async move |this, cx| {
            let q: Vec<(&str, String)> = if fresh { vec![("fresh", "1".into())] } else { vec![] };
            let r = cx.background_executor().spawn(async move { backend.get("accounts", &q) }).await;
            let _ = this.update(cx, |s, cx| {
                match r {
                    Ok(v) => s.got(v, cx),
                    Err(e) => s.error = Some(e.message),
                }
                cx.notify();
            });
        })
        .detach();
    }

    /// A fresh `accounts` answer: notes what changed and copies a new sign-in code. GitHub opens only
    /// from the code panel's button, so the code is seen (and known to be copied) before the browser
    /// takes over.
    fn got(&mut self, v: Value, cx: &mut Context<Self>) {
        let before = self.account("github")["connected"] == true;
        self.accounts = v["accounts"].as_array().cloned().unwrap_or_default();
        self.loaded = true;
        self.error = None;
        let gh = self.account("github");
        if !before && gh["connected"] == true && self.opened_code.take().is_some() {
            self.notes.insert("github", (format!("Signed in as @{}.", gh["user"].as_str().unwrap_or("")), false));
        }
        if let Some((code, _)) = self.login().filter(|(c, _)| !c.is_empty())
            && self.opened_code.as_deref() != Some(code.as_str())
        {
            cx.write_to_clipboard(ClipboardItem::new_string(code.clone()));
            self.opened_code = Some(code);
            self.browser_opened = false;
        }
    }

    /// `POST accounts/<path>` for an account; the answer is every account's state.
    fn post(&mut self, id: &'static str, path: String, body: Value, cx: &mut Context<Self>, ok: impl FnOnce(&mut Self, &mut Context<Self>) + 'static) {
        if self.busy.is_some() {
            return;
        }
        self.busy = Some(id);
        self.notes.remove(id);
        cx.notify();
        let backend = self.backend.clone();
        cx.spawn(async move |this, cx| {
            let r = cx.background_executor().spawn(async move { backend.post(&path, body) }).await;
            let _ = this.update(cx, |s, cx| {
                s.busy = None;
                match r {
                    Ok(v) => {
                        s.got(v, cx);
                        ok(s, cx);
                    }
                    Err(e) => {
                        s.notes.insert(id, (e.message, true));
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }

    fn sign_in(&mut self, cx: &mut Context<Self>) {
        self.opened_code = None;
        self.browser_opened = false;
        self.post("github", "accounts/github/login".into(), json!({}), cx, |_, _| {});
    }

    fn connect(&mut self, id: &'static str, cx: &mut Context<Self>) {
        let body = match id {
            "github" => json!({"token": self.gh_token.text(cx)}),
            "bitbucket" => json!({"email": self.bb_email.text(cx), "token": self.bb_token.text(cx)}),
            _ => json!({"token": self.slack_token.text(cx)}),
        };
        self.post(id, format!("accounts/{id}"), body, cx, move |s, cx| {
            let field = match id {
                "github" => &s.gh_token,
                "bitbucket" => &s.bb_token,
                _ => &s.slack_token,
            };
            field.clear(cx);
            s.gh_form = false;
            let a = s.account(id);
            s.notes.insert(id, (format!("Connected as {}.", a["detail"].as_str().or(a["user"].as_str()).unwrap_or("you")), false));
        });
    }

    fn copy(&mut self, text: String, cx: &mut Context<Self>) {
        cx.write_to_clipboard(ClipboardItem::new_string(text.clone()));
        self.copied = Some((text, Instant::now()));
        cx.notify();
        cx.spawn(async move |this, cx| {
            cx.background_executor().timer(Duration::from_millis(1500)).await;
            let _ = this.update(cx, |s, cx| {
                if s.copied.as_ref().is_some_and(|(_, at)| at.elapsed() >= Duration::from_millis(1400)) {
                    s.copied = None;
                    cx.notify();
                }
            });
        })
        .detach();
    }

    /// ↩ connects, esc clears the field.
    fn field_keys(&self, id: &'static str, field: &Input, cx: &mut Context<Self>) -> impl Fn(&KeyDownEvent, &mut Window, &mut App) + 'static {
        let entity = cx.entity().downgrade();
        let input = field.field.clone();
        move |ev, _, cx| {
            let ks = &ev.keystroke;
            let outcome = match ks.key.as_str() {
                "enter" => KeyOutcome::Submit,
                "escape" => KeyOutcome::Cancel,
                _ => KeyOutcome::Ignored,
            };
            match outcome {
                KeyOutcome::Submit => {
                    let _ = entity.update(cx, |s, cx| s.connect(id, cx));
                }
                KeyOutcome::Cancel => input.update(cx, |f, cx| f.clear(cx)),
                KeyOutcome::Ignored => cx.propagate(),
            }
        }
    }
}

// ------------------------------------------------------------------ render

const SIDEBAR_W: f32 = 228.;

/// Drags the window from empty space; a double click zooms, like a title bar.
fn drag_window(ev: &MouseDownEvent, window: &mut Window, _: &mut App) {
    if ev.click_count >= 2 {
        window.titlebar_double_click();
    } else {
        window.start_window_move();
    }
}

impl Render for SettingsWindow {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = cx.global::<Theme>().clone();
        let body = match self.view {
            Sec::Accounts => self.accounts_page(&t, window, cx).into_any_element(),
        };
        div()
            .id("settings-root")
            .track_focus(&self.focus)
            .key_context("TaskboardSettings")
            .size_full()
            .flex()
            .bg(t.bg)
            .text_color(t.text)
            .font_family(t.ui_font.clone())
            .text_size(px(13.))
            .line_height(px(13. * 1.45))
            .child(self.sidebar(&t, cx))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .h_full()
                    .flex()
                    .flex_col()
                    .child(self.page_header(&t, cx))
                    .when_some(self.error.clone(), |d, e| {
                        d.child(div().mx(px(28.)).mb(px(10.)).px(px(12.)).py(px(8.)).rounded(px(8.)).bg(t.warn_soft).text_color(t.warn_fg).text_size(px(12.)).child(format!("taskboardd: {e}")))
                    })
                    .child(body),
            )
    }
}

impl SettingsWindow {
    fn sidebar(&self, t: &Theme, cx: &mut Context<Self>) -> impl IntoElement {
        let mut list = div().id("sections").flex_1().min_h_0().overflow_y_scroll().flex().flex_col().gap(px(1.)).px(px(10.));
        for sec in SECS {
            let on = self.view == sec;
            // amber count: accounts whose sign-in stopped working
            let n = self.accounts.iter().filter(|a| a["connected"] == true && a["error"].is_string()).count();
            let hover = t.panel_2;
            list = list.child(
                div()
                    .id(SharedString::from(format!("sec-{}", sec.label().to_lowercase())))
                    .flex()
                    .flex_none()
                    .items_center()
                    .gap(px(9.))
                    .h(px(30.))
                    .px(px(9.))
                    .rounded(px(7.))
                    .cursor_pointer()
                    .when(on, |d| d.bg(t.panel_2).font_weight(FontWeight::SEMIBOLD))
                    .when(!on, |d| d.hover(move |s| s.bg(hover.opacity(0.6))))
                    .child(div().w(px(15.)).flex().justify_center().font_weight(FontWeight::BOLD).text_color(if on { t.accent } else { t.muted }).child(sec.glyph()))
                    .child(div().flex_1().min_w_0().truncate().child(sec.label()))
                    .when(n > 0, |d| {
                        d.child(
                            div().min_w(px(18.)).h(px(18.)).px(px(5.)).rounded(px(9.)).flex().items_center().justify_center().text_size(px(11.)).font_weight(FontWeight::BOLD).bg(t.warn).text_color(t.on_accent).child(n.to_string()),
                        )
                    })
                    .on_click(cx.listener(move |s, _, _, cx| {
                        s.view = sec;
                        cx.notify();
                    })),
            );
        }
        let live = self.error.is_none() && self.loaded;
        div()
            .w(px(SIDEBAR_W))
            .flex_none()
            .h_full()
            .flex()
            .flex_col()
            .bg(t.tint)
            .border_r_1()
            .border_color(t.border)
            // room for the traffic lights; drags the window like a title bar
            .child(div().id("sidebar-drag").h(px(48.)).flex_none().on_mouse_down(MouseButton::Left, drag_window))
            .child(list)
            .child(
                div()
                    .flex()
                    .flex_none()
                    .items_center()
                    .gap(px(7.))
                    .px(px(19.))
                    .py(px(12.))
                    .text_size(px(12.))
                    .text_color(t.muted)
                    .child(kit::dot(if live { t.up } else { t.warn }, 7.))
                    .child(div().flex_1().child(if live { "Live" } else if self.loaded { "Not connected" } else { "Connecting…" }))
                    .when(self.backend.label() == "fake", |d| d.child(kit::tone_pill(t, "goal", "Sample"))),
            )
    }

    fn page_header(&self, t: &Theme, cx: &mut Context<Self>) -> impl IntoElement {
        let on = self.cli;
        let text = t.text;
        div()
            .id("page-header")
            .flex()
            .flex_none()
            .items_end()
            .gap(px(16.))
            .px(px(28.))
            .pt(px(22.))
            .pb(px(14.))
            .on_mouse_down(MouseButton::Left, drag_window)
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .flex()
                    .flex_col()
                    .gap(px(3.))
                    .child(div().text_size(px(20.)).line_height(px(26.)).font_weight(FontWeight::BOLD).truncate().child(self.view.label()))
                    .child(div().text_size(px(12.5)).line_height(px(17.)).text_color(t.muted).child(self.view.about())),
            )
            .child(
                div()
                    .id("agent-commands")
                    .flex()
                    .flex_none()
                    .items_center()
                    .gap(px(6.))
                    .h(px(28.))
                    .px(px(10.))
                    .rounded(px(7.))
                    .border_1()
                    .text_size(px(12.))
                    .cursor_pointer()
                    .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                    .map(|d| if on { d.border_color(t.accent).bg(t.accent_soft).text_color(t.text) } else { d.border_color(t.border).bg(t.card).text_color(t.muted).hover(move |s| s.text_color(text)) })
                    .tooltip(kit::tip("Show the commands agents run with these accounts"))
                    .on_click(cx.listener(|s, _, _, cx| {
                        s.cli = !s.cli;
                        cx.notify();
                    }))
                    .child(div().font_family(t.mono_font.clone()).text_color(if on { t.accent } else { t.muted }).child("</>"))
                    .child("Agent commands"),
            )
    }

    fn accounts_page(&self, t: &Theme, window: &Window, cx: &mut Context<Self>) -> impl IntoElement {
        let mut list = div().id("settings-rows").flex_1().min_h_0().overflow_y_scroll().track_scroll(&self.scroll).flex().flex_col().gap(px(20.)).px(px(28.)).pt(px(4.)).pb(px(28.));
        if !self.loaded && self.error.is_none() {
            return list.child(kit::empty(t, "Asking the board…"));
        }
        for (id, mark, color, about) in CARDS {
            let a = self.account(id);
            let rows: Vec<AnyElement> = match id {
                "github" => self.github_rows(t, &a, window, cx),
                "bitbucket" => self.bitbucket_rows(t, &a, window, cx),
                _ => self.slack_rows(t, &a, window, cx),
            };
            let mut card = kit::card(t).overflow_hidden();
            for r in rows {
                card = card.child(r);
            }
            if let Some((text, bad)) = self.notes.get(id) {
                card = card.child(row(t, &format!("{id}-note"), false).bg(if *bad { t.down_soft } else { t.up_soft }).text_color(if *bad { t.down } else { t.up_fg }).text_size(px(12.)).child(text.clone()));
            }
            if self.cli {
                card = card.child(self.cli_line(t, id, "tb accounts", cx)).child(self.cli_line(t, &format!("{id}-use"), agent_cli(id), cx));
            }
            let label = a["label"].as_str().unwrap_or(id).to_string();
            let heading = div()
                .flex()
                .items_center()
                .gap(px(8.))
                .px(px(4.))
                .text_size(px(12.))
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(t.muted)
                .child(
                    div()
                        .size(px(18.))
                        .flex()
                        .flex_none()
                        .items_center()
                        .justify_center()
                        .rounded(px(5.))
                        .bg(rgb(color))
                        .text_color(t.on_accent)
                        .text_size(px(9.))
                        .font_weight(FontWeight::BOLD)
                        .child(mark),
                )
                .child(div().text_color(t.text).child(label))
                .child(div().font_weight(FontWeight::NORMAL).text_size(px(11.5)).truncate().child(format!("· {about}")));
            list = list.child(div().flex().flex_col().gap(px(8.)).child(heading).child(card));
        }
        list.child(
            div()
                .px(px(4.))
                .text_size(px(11.5))
                .line_height(px(16.))
                .text_color(t.muted)
                .child("Tokens are checked with the service, then kept in your login Keychain (GitHub's by the GitHub CLI). The board never shows them again; agents get them with tb token."),
        )
    }

    // -------------------------------------------------------------- accounts

    fn github_rows(&self, t: &Theme, a: &Value, window: &Window, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let busy = self.busy == Some("github");
        let mut rows = vec![];
        if a["ready"] == false {
            let setup = a["setup"].as_str().unwrap_or("brew install gh").to_string();
            let copied = self.copied.as_ref().is_some_and(|(c, _)| *c == setup);
            rows.push(
                line(t, "gh-missing", true, true, "The GitHub CLI isn't installed", "Taskboard signs in through gh, the same tool agents use for pull requests.", None)
                    .child(controls().child(kit::btn_small(t, "gh-recheck", "Check again").on_click(cx.listener(|s, _, _, cx| s.load(true, cx)))).child(
                        kit::btn_primary(t, "gh-copy-install", if copied { "Copied".to_string() } else { format!("Copy “{setup}”") })
                            .h(px(26.))
                            .text_size(px(12.))
                            .on_click(cx.listener(move |s, _, _, cx| s.copy(setup.clone(), cx))),
                    ))
                    .into_any_element(),
            );
            return rows;
        }
        if let Some((code, url)) = self.login() {
            rows.push(self.github_code(t, code, url, cx));
            return rows;
        }
        if a["connected"] == true {
            rows.push(self.signed_in_row(t, "github", a, "Signed in with the GitHub CLI", "Sign out", cx));
            rows.push(git_row(t, "github", a["git"] == true, "git push over HTTPS asks gh for github.com passwords.", "git doesn't use gh for github.com yet. Run gh auth setup-git, or sign in again."));
        } else {
            let sign_in = if busy { kit::disabled(kit::btn_primary(t, "gh-sign-in", "Starting…").h(px(26.)).text_size(px(12.))) } else {
                kit::btn_primary(t, "gh-sign-in", "Sign in with GitHub").h(px(26.)).text_size(px(12.)).on_click(cx.listener(|s, _, _, cx| s.sign_in(cx)))
            };
            rows.push(
                line(t, "gh-out", true, false, "Not signed in", "Opens GitHub in your browser with a one-time code. gh keeps the token in your Keychain.", None)
                    .child(
                        controls()
                            .when(!self.gh_form, |d| {
                                d.child(kit::btn_small(t, "gh-use-token", "Use a token").on_click(cx.listener(|s, _, window, cx| {
                                    s.gh_form = true;
                                    window.focus(&s.gh_token.focus, cx);
                                    cx.notify();
                                })))
                            })
                            .child(sign_in),
                    )
                    .into_any_element(),
            );
            if self.gh_form {
                rows.push(self.token_row(t, "github", "Token", "A classic token with repo, read:org and workflow, or a fine-grained one with Contents and Pull requests (read and write).", &self.gh_token, window, cx, true));
            }
        }
        rows.extend(error_row(t, "github", a));
        rows
    }

    fn bitbucket_rows(&self, t: &Theme, a: &Value, window: &Window, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let mut rows = vec![];
        if a["connected"] == true {
            rows.push(self.signed_in_row(t, "bitbucket", a, "API token in the Keychain (taskboard-bitbucket)", "Disconnect", cx));
            rows.push(git_row(t, "bitbucket", a["git"] == true, "git push over HTTPS uses the token (x-bitbucket-api-token-auth).", "git has no credential helper to keep the token, so git push will still ask."));
        } else {
            let email = self.bb_email.render(t, "bb-email", window).w(px(260.)).on_key_down(self.field_keys("bitbucket", &self.bb_email, cx));
            rows.push(line(t, "bb-email-row", true, false, "Atlassian email", "The email you sign in to Bitbucket with.", None).child(email).into_any_element());
            rows.push(self.token_row(
                t,
                "bitbucket",
                "API token",
                "An Atlassian API token with Bitbucket scopes: account (read), repositories and pull requests (read and write).",
                &self.bb_token,
                window,
                cx,
                false,
            ));
            rows.push(link_row(t, "bb-create", "Create an API token", taskboardd::accounts::BITBUCKET_TOKEN_URL, cx));
        }
        rows.extend(error_row(t, "bitbucket", a));
        rows
    }

    fn slack_rows(&self, t: &Theme, a: &Value, window: &Window, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let mut rows = vec![];
        if a["connected"] == true {
            rows.push(self.signed_in_row(t, "slack", a, "Token in the Keychain (taskboard-slack)", "Disconnect", cx));
        } else {
            rows.push(self.token_row(
                t,
                "slack",
                "Token",
                "A user (xoxp-) or bot (xoxb-) token from your Slack app's OAuth & Permissions page.",
                &self.slack_token,
                window,
                cx,
                true,
            ));
            rows.push(link_row(t, "slack-apps", "Open your Slack apps", taskboardd::accounts::SLACK_APPS_URL, cx));
        }
        rows.extend(error_row(t, "slack", a));
        rows
    }

    /// GitHub's browser sign-in: the code (already on the clipboard, and it says so), then a button
    /// that opens github.com/login/device, then what happens next.
    fn github_code(&self, t: &Theme, code: String, url: String, cx: &mut Context<Self>) -> AnyElement {
        // 1 is done as soon as the code is copied; 2 is what's left.
        let done = div().size(px(22.)).flex().flex_none().items_center().justify_center().rounded_full().bg(t.up_soft).text_color(t.up_fg).text_size(px(12.)).font_weight(FontWeight::BOLD).child("✓");
        let step = |n: &str, on: bool| {
            div()
                .size(px(22.))
                .flex()
                .flex_none()
                .items_center()
                .justify_center()
                .rounded_full()
                .text_size(px(12.))
                .font_weight(FontWeight::BOLD)
                .map(|d| if on { d.bg(t.accent_btn).text_color(t.on_accent) } else { d.bg(t.card).border_1().border_color(t.accent_line).text_color(t.accent_fg) })
                .child(n.to_string())
        };
        let head = div()
            .flex()
            .items_center()
            .gap(px(12.))
            .child(div().flex_1().text_size(px(14.)).font_weight(FontWeight::SEMIBOLD).child("Finish signing in on GitHub"))
            .child(kit::btn_small(t, "gh-cancel", "Cancel").on_click(cx.listener(|s, _, _, cx| {
                s.opened_code = None;
                s.browser_opened = false;
                s.post("github", "accounts/github/cancel".into(), json!({}), cx, |_, _| {});
            })));
        let panel = div().id("gh-code").flex().flex_col().gap(px(14.)).px(px(16.)).py(px(14.)).bg(t.accent_soft).child(head);
        if code.is_empty() {
            return panel.child(div().text_size(px(12.5)).text_color(t.muted).child("Getting a code from GitHub…")).into_any_element();
        }
        let flashed = self.copied.as_ref().is_some_and(|(c, _)| *c == code);
        let c = code.clone();
        let chip = div()
            .id("gh-code-chip")
            .px(px(14.))
            .py(px(6.))
            .rounded(px(9.))
            .border_1()
            .border_color(t.accent_line)
            .bg(t.card)
            .font_family(t.mono_font.clone())
            .text_size(px(24.))
            .line_height(px(30.))
            .font_weight(FontWeight::SEMIBOLD)
            .cursor_pointer()
            .tooltip(kit::tip("Copy it again"))
            .on_click(cx.listener(move |s, _, _, cx| s.copy(c.clone(), cx)))
            .child(code.clone());
        let copied = kit::pill(t.up_fg, t.up_soft, if flashed { "✓ Copied again" } else { "✓ Copied to your clipboard" }).h(px(26.)).px(px(11.)).text_size(px(12.5));
        let opened = self.browser_opened;
        let open = kit::btn_primary(t, "gh-open", if opened { "Open GitHub again ↗" } else { "Open GitHub and paste it ↗" }).on_click(cx.listener(move |s, _, _, cx| {
            if let Some((code, _)) = s.login() {
                cx.write_to_clipboard(ClipboardItem::new_string(code));
            }
            cx.open_url(&url);
            s.browser_opened = true;
            cx.notify();
        }));
        let line1 = div().flex().items_center().gap(px(12.)).child(done).child(chip).child(copied);
        let line2 = div()
            .flex()
            .items_center()
            .gap(px(12.))
            .child(step("2", true))
            .child(open)
            .child(div().flex_1().min_w_0().text_size(px(12.5)).text_color(t.text_2).child("Press ⌘V in the code boxes, then Continue and Authorize."));
        let after = if opened {
            "Waiting for you to approve on GitHub. This updates by itself. Page didn't load? Open it again: the code works for 15 minutes."
        } else {
            "Nothing to type: the code is already copied, so you only paste it."
        };
        panel.child(line1).child(line2).child(div().text_size(px(11.5)).line_height(px(15.)).text_color(t.muted).child(after)).into_any_element()
    }

    /// "Name · detail · scopes" with Check and Sign out / Disconnect.
    fn signed_in_row(&self, t: &Theme, id: &'static str, a: &Value, how: &str, out: &str, cx: &mut Context<Self>) -> AnyElement {
        let busy = self.busy == Some(id);
        let name = a["name"].as_str().or(a["user"].as_str()).unwrap_or("Connected").to_string();
        let scopes: Vec<&str> = a["scopes"].as_array().map(|s| s.iter().filter_map(|x| x.as_str()).collect()).unwrap_or_default();
        let detail = a["detail"].as_str().unwrap_or("");
        let note = [detail, how].iter().filter(|s| !s.is_empty()).cloned().collect::<Vec<_>>().join(" · ");
        let check = kit::btn_small(t, SharedString::from(format!("{id}-check")), if busy { "Checking…" } else { "Check" });
        let check = if busy { kit::disabled(check) } else { check.tooltip(kit::tip("Ask the service whether the token still works")).on_click(cx.listener(move |s, _, _, cx| s.post(id, format!("accounts/{id}/check"), json!({}), cx, |_, _| {}))) };
        let out_btn = kit::btn_small(t, SharedString::from(format!("{id}-out")), out.to_string()).text_color(t.down);
        let out_btn = if busy { kit::disabled(out_btn) } else { out_btn.on_click(cx.listener(move |s, _, _, cx| s.post(id, format!("accounts/{id}/disconnect"), json!({}), cx, |_, _| {}))) };
        let tip = if id == "github" { "Signs the GitHub CLI out of github.com on this Mac" } else { "Forgets the token on this Mac" };
        line(t, &format!("{id}-in"), true, false, "", &note, Some((&name, true)))
            .when(!scopes.is_empty(), |d| d.child(div().flex().flex_wrap().gap(px(4.)).max_w(px(260.)).justify_end().children(scopes.into_iter().take(6).map(|s| kit::chip(t, s.to_string())))))
            .child(controls().child(check).child(out_btn.tooltip(kit::tip(tip))))
            .into_any_element()
    }

    /// A secret field with Connect (↩).
    #[allow(clippy::too_many_arguments)]
    fn token_row(&self, t: &Theme, id: &'static str, label: &str, note: &str, field: &Input, window: &Window, cx: &mut Context<Self>, first: bool) -> AnyElement {
        let busy = self.busy == Some(id);
        let input = field.render(t, SharedString::from(format!("{id}-token")), window).w(px(260.)).on_key_down(self.field_keys(id, field, cx));
        let btn = kit::btn_primary(t, SharedString::from(format!("{id}-connect")), if busy { "Checking…" } else { "Connect" }).h(px(32.)).text_size(px(12.5));
        let btn = if busy { kit::disabled(btn) } else { btn.on_click(cx.listener(move |s, _, _, cx| s.connect(id, cx))) };
        line(t, &format!("{id}-token-row"), first, false, label, note, None).child(controls().child(input).child(btn)).into_any_element()
    }

    fn cli_line(&self, t: &Theme, id: &str, cli: &str, cx: &mut Context<Self>) -> impl IntoElement {
        let copied = self.copied.as_ref().is_some_and(|(c, _)| c == cli);
        let text = cli.to_string();
        let (raised, fg) = (t.panel_2, t.text);
        div().px(px(16.)).pb(px(8.)).child(
            div()
                .flex()
                .items_center()
                .gap(px(8.))
                .pl(px(10.))
                .pr(px(4.))
                .py(px(3.))
                .rounded(px(6.))
                .bg(t.col)
                .font_family(t.mono_font.clone())
                .text_size(px(11.5))
                .child(div().flex_none().text_color(t.accent).child("$"))
                .child(div().flex_1().min_w_0().truncate().child(cli.to_string()))
                .child(
                    div()
                        .id(SharedString::from(format!("copy-{id}")))
                        .flex_none()
                        .h(px(20.))
                        .px(px(6.))
                        .flex()
                        .items_center()
                        .rounded(px(5.))
                        .font_family(t.ui_font.clone())
                        .text_size(px(11.))
                        .text_color(if copied { t.up } else { t.muted })
                        .cursor_pointer()
                        .hover(move |s| s.bg(raised).text_color(fg))
                        .on_click(cx.listener(move |s, _, _, cx| s.copy(text.clone(), cx)))
                        .child(if copied { "Copied" } else { "Copy" }),
                ),
        )
    }
}

// ------------------------------------------------------------------ rows

/// A row's shell: padding and the line above every row but the first.
fn row(t: &Theme, id: &str, first: bool) -> Stateful<Div> {
    div().id(SharedString::from(id.to_string())).flex().items_center().gap(px(16.)).px(px(16.)).py(px(9.)).min_h(px(46.)).when(!first, |d| d.border_t_1().border_color(t.border))
}

/// A row with a name (or `title`, bold) and what it means; the caller adds its controls.
fn line(t: &Theme, id: &str, first: bool, warn: bool, label: &str, note: &str, title: Option<(&str, bool)>) -> Stateful<Div> {
    let label = title.map_or(label.to_string(), |(s, _)| s.to_string());
    row(t, id, first).when(warn, |d| d.bg(t.warn_soft)).child(
        div()
            .flex_1()
            .min_w(px(150.))
            .flex()
            .flex_col()
            .child(div().when(title.is_some_and(|(_, b)| b), |d| d.font_weight(FontWeight::SEMIBOLD)).child(label))
            .when(!note.is_empty(), |d| d.child(div().text_size(px(11.5)).line_height(px(15.)).text_color(if warn { t.warn_fg } else { t.muted }).child(note.to_string()))),
    )
}

fn controls() -> Div {
    div().flex().flex_none().items_center().gap(px(8.))
}

fn git_row(t: &Theme, id: &str, ok: bool, yes: &str, no: &str) -> AnyElement {
    row(t, &format!("{id}-git"), false)
        .when(!ok, |d| d.bg(t.warn_soft))
        .child(kit::dot(if ok { t.up } else { t.warn }, 7.))
        .child(div().flex_1().min_w_0().text_size(px(12.)).text_color(if ok { t.muted } else { t.warn_fg }).child(if ok { yes.to_string() } else { no.to_string() }))
        .into_any_element()
}

fn error_row(t: &Theme, id: &str, a: &Value) -> Option<AnyElement> {
    let e = a["error"].as_str()?;
    Some(row(t, &format!("{id}-error"), false).bg(t.warn_soft).child(kit::dot(t.warn, 7.)).child(div().flex_1().min_w_0().text_size(px(12.)).text_color(t.warn_fg).child(e.to_string())).into_any_element())
}

fn link_row(t: &Theme, id: &str, label: &str, url: &'static str, cx: &mut Context<SettingsWindow>) -> AnyElement {
    row(t, id, false)
        .min_h(px(34.))
        .text_size(px(12.))
        .child(kit::link(t, SharedString::from(format!("{id}-link")), format!("{label} ↗")).on_click(cx.listener(move |_, _, _, cx| cx.open_url(url))))
        .child(div().flex_1().min_w_0().truncate().text_color(t.faint).child(url.trim_start_matches("https://").to_string()))
        .into_any_element()
}

#[cfg(test)]
mod tests {
    use super::SettingsWindow;
    use crate::backend::Backend;
    use crate::parity::{Recording, settle};
    use crate::theme::Theme;
    use gpui_kit::WindowHandle;
    use serde_json::json;
    use std::sync::Arc;

    fn window(cx: &mut gpui_kit::TestAppContext) -> (WindowHandle<SettingsWindow>, Arc<Recording>) {
        let rec = Recording::new();
        cx.update(|cx| cx.set_global(Theme::new(crate::theme::ThemeMode::Light, "Helvetica Neue".into(), "Menlo".into())));
        let be: Arc<dyn Backend> = rec.clone();
        let w = cx.add_window(|window, cx| SettingsWindow::new(be, window, cx));
        settle(cx);
        (w, rec)
    }

    #[gpui_kit::test]
    fn connects_with_a_token_and_clears_the_field(cx: &mut gpui_kit::TestAppContext) {
        let (w, rec) = window(cx);
        w.update(cx, |s, _, cx| {
            assert!(s.loaded);
            assert_eq!(s.accounts.len(), 3);
            s.bb_email.set_text("sam@acme.com", cx);
            s.bb_token.set_text("ATATT-x", cx);
            s.connect("bitbucket", cx);
        })
        .unwrap();
        settle(cx);
        assert_eq!(rec.last("accounts/bitbucket"), Some(json!({"email": "sam@acme.com", "token": "ATATT-x"})));
        w.update(cx, |s, _, cx| {
            assert_eq!(s.account("bitbucket")["connected"], true);
            assert_eq!(s.bb_token.text(cx), "", "the token doesn't stay in the field");
            assert!(s.notes.get("bitbucket").is_some_and(|(_, bad)| !bad));
            s.slack_token.set_text("bad", cx);
            s.connect("slack", cx);
        })
        .unwrap();
        settle(cx);
        w.update(cx, |s, _, cx| {
            assert_eq!(s.account("slack")["connected"], false);
            assert!(s.notes.get("slack").is_some_and(|(_, bad)| *bad), "the board's sentence shows under the card");
            assert_eq!(s.slack_token.text(cx), "bad", "a refused token stays to fix");
        })
        .unwrap();
    }

    #[gpui_kit::test]
    fn github_browser_sign_in_shows_the_code_until_cancelled(cx: &mut gpui_kit::TestAppContext) {
        let (w, rec) = window(cx);
        w.update(cx, |s, _, cx| s.sign_in(cx)).unwrap();
        settle(cx);
        w.update(cx, |s, _, cx| {
            assert_eq!(s.login().map(|(c, _)| c).as_deref(), Some("SAMP-1234"));
            assert_eq!(s.opened_code.as_deref(), Some("SAMP-1234"), "the code is copied once");
            assert!(!s.browser_opened, "GitHub opens from the button, after the code is seen");
            s.post("github", "accounts/github/cancel".into(), json!({}), cx, |_, _| {});
        })
        .unwrap();
        settle(cx);
        assert!(rec.posts().iter().any(|(p, _)| p == "accounts/github/cancel"));
        w.update(cx, |s, _, _| assert!(s.login().is_none())).unwrap();
    }
}
