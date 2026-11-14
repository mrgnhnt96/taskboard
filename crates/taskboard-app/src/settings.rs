//! Settings: its own standard macOS window, laid out like Midna's. A sidebar of sections beside the
//! selected section's rows, grouped in cards; a row is its name, what it does and its control.
//! "Agent commands" adds under the rows the exact command an agent would run (copyable).
//!
//! Worktrees shows each project's slot pool (`GET /slots`), read-only: its settings, each slot with
//! the task holding it, its size and last use, and the tasks waiting for one.
//!
//! Accounts: GitHub, Bitbucket, Azure DevOps and Slack, so the board and its agents can
//! check PRs, comment, push, read failed builds and assign reviewers. The daemon owns them (`taskboardd::accounts`): each
//! is Taskboard's own token, checked with the service and kept in the Keychain, never the Mac's `gh`
//! or git sign-in. GitHub's comes from a browser code, a pasted token or a copy of gh's. Tokens go to
//! the daemon once and never come back to the window. An account missing scopes Taskboard needs shows
//! "Sign in again".
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
    History,
    Usage,
    Qa,
    Questions,
    Reviewers,
    Worktrees,
    System,
}

const SECS: [Sec; 8] = [Sec::Accounts, Sec::History, Sec::Usage, Sec::Qa, Sec::Questions, Sec::Reviewers, Sec::Worktrees, Sec::System];

impl Sec {
    fn label(self) -> &'static str {
        match self {
            Sec::Accounts => "Accounts",
            Sec::History => "History",
            Sec::Usage => "Usage",
            Sec::Qa => "QA",
            Sec::Questions => "Questions",
            Sec::Reviewers => "Reviewers",
            Sec::Worktrees => "Worktrees",
            Sec::System => "System",
        }
    }

    fn about(self) -> &'static str {
        match self {
            Sec::Accounts => "Sign in once. The board and its agents use these to check PRs, comment, push and assign reviewers.",
            Sec::History => "How far back the Days page goes, and when the board cleans up old work.",
            Sec::Usage => "A daily share of Claude's weekly limit. Once today's share is used, the board starts no new work until midnight; running agents carry on.",
            Sec::Qa => "Testers' Jira comments on the board's tickets, turned into follow-up tasks or questions for you. Off until you switch it on.",
            Sec::Questions => "Questions agents ask you in their replies, and whether the board sends back one asked above the reply's last paragraph to ask it properly.",
            Sec::Reviewers => "Who reviews each project's PRs. Agents and tb reviewers change the roster.",
            Sec::Worktrees => "Each project's reusable worktree slots, kept warm between tasks. tb project set sizes a project's pool.",
            Sec::System => "Updates, and the versions of the app and the board.",
        }
    }

    fn glyph(self) -> &'static str {
        match self {
            Sec::Accounts => "@",
            Sec::History => "◷",
            Sec::Usage => "◔",
            Sec::Qa => "✓",
            Sec::Questions => "?",
            Sec::Reviewers => "☺",
            Sec::Worktrees => "⑂",
            Sec::System => "⚙",
        }
    }
}

/// Each account's row: id and a short mark.
const CARDS: [(&str, &str); 4] = [("github", "GH"), ("bitbucket", "BB"), ("azure", "AZ"), ("slack", "SL")];

/// What stops while an account's sign-in is refused.
fn refused_means(id: &str) -> &'static str {
    match id {
        "github" | "bitbucket" => "agents can't push",
        "azure" => "no failed pipeline steps on PR checks",
        _ => "no Slack messages",
    }
}

/// What forgetting an account's sign-in stops, asked before Disconnect does it.
fn forget_means(id: &str) -> &'static str {
    match id {
        "github" => "Agents can't push to github.com or read PR checks until you sign in again.",
        "bitbucket" => "Agents can't push to bitbucket.org or read pull requests until you connect it again.",
        "azure" => "PR checks stop showing failed pipeline steps until you connect it again.",
        _ => "The board and its agents can't post to Slack until you connect it again.",
    }
}

/// An account's status line, and whether the service refused its sign-in: "Signed in as sam ·
/// checked 3:05 PM, works", "GitHub refused the sign-in at 2:51 PM · agents can't push",
/// "Not connected".
fn account_line(id: &str, a: &Value) -> (String, bool) {
    if a["connected"] != true {
        return ("Not connected".into(), false);
    }
    let at = a["checked_at"].as_str().map(crate::fmt::day_time).filter(|s| !s.is_empty());
    if a["error"].is_string() {
        let label = a["label"].as_str().unwrap_or(id);
        let when = at.map(|at| format!(" at {at}")).unwrap_or_default();
        return (format!("{label} refused the sign-in{when} · {}", refused_means(id)), true);
    }
    let org = a["detail"].as_str().and_then(|d| d.split("dev.azure.com/").nth(1)).map(|o| o.trim_end_matches('/').to_string());
    let who = match (id, org) {
        ("azure", Some(org)) => format!("Signed in to {org}"),
        _ => match a["user"].as_str().filter(|u| !u.is_empty()) {
            Some(u) => format!("Signed in as {u}"),
            None => "Signed in".into(),
        },
    };
    (match at { Some(at) => format!("{who} · checked {at}, works"), None => who }, false)
}

/// What a row's agent line shows, per account.
fn agent_cli(id: &str) -> &'static str {
    match id {
        "github" => "tb pr status   # anything else: gh, or tb api github repos/OWNER/REPO/…",
        "bitbucket" => "tb pr status   # anything else: tb api bitbucket repositories/WORKSPACE/REPO/…",
        "azure" => "tb pr status   # anything else: tb api azure PROJECT/_apis/build/builds/ID?api-version=7.1",
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
    /// Bitbucket's token form is open while connected (to replace a token missing scopes).
    bb_form: bool,
    /// Azure DevOps's token form is open while connected (to replace a token missing scopes).
    az_form: bool,
    /// Slack's token form is open.
    slack_form: bool,
    /// The account whose Disconnect is asking first.
    confirm: Option<&'static str>,
    gh_token: Input,
    bb_email: Input,
    bb_token: Input,
    az_org: Input,
    az_token: Input,
    slack_token: Input,
    /// The sign-in code already copied (so a poll doesn't copy it again).
    opened_code: Option<String>,
    /// GitHub's device page was opened for this code.
    browser_opened: bool,
    /// `GET /history`, a call in flight, and the line under its card.
    history: Option<Value>,
    history_busy: bool,
    history_note: Option<(String, bool)>,
    /// A retention change that removes history, waiting for the owner's yes: the change and what it removes.
    history_confirm: Option<(Value, String)>,
    /// `GET /usage`'s `daily` block, a call in flight, and the line under its card.
    daily: Option<Value>,
    daily_busy: bool,
    daily_note: Option<(String, bool)>,
    /// `GET /qa`, a call in flight, and the line under its card.
    qa: Option<Value>,
    qa_busy: bool,
    qa_note: Option<(String, bool)>,
    /// `GET /buried-questions`, a call in flight, and the line under its card.
    questions: Option<Value>,
    questions_busy: bool,
    questions_note: Option<(String, bool)>,
    /// `GET /reviewers?project=all`.
    reviewers: Option<Value>,
    /// `GET /slots`: every project's slots and pool settings.
    slots: Option<Value>,
    /// `GET /info`: the daemon's version, uptime and pid.
    info: Option<Value>,
    /// The daemon is restarting, and the line under the System card.
    sys_busy: bool,
    sys_note: Option<(String, bool)>,
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
            bb_form: false,
            az_form: false,
            slack_form: false,
            confirm: None,
            gh_token: Input::secret(cx, "ghp_… or github_pat_…"),
            bb_email: Input::new(cx, "you@company.com", false),
            bb_token: Input::secret(cx, "ATATT…"),
            az_org: Input::new(cx, "your-org or dev.azure.com/your-org", false),
            az_token: Input::secret(cx, "Personal access token"),
            slack_token: Input::secret(cx, "xoxp-… or xoxb-…"),
            opened_code: None,
            browser_opened: false,
            history: None,
            history_busy: false,
            history_note: None,
            history_confirm: None,
            daily: None,
            daily_busy: false,
            daily_note: None,
            qa: None,
            qa_busy: false,
            qa_note: None,
            questions: None,
            questions_busy: false,
            questions_note: None,
            reviewers: None,
            slots: None,
            info: None,
            sys_busy: false,
            sys_note: None,
        };
        // coming back to the window shows what changed while it was behind others
        cx.observe_window_activation(window, |s, window, cx| {
            if window.is_window_active() {
                s.load(false, cx);
                if s.view != Sec::Accounts {
                    s.reload_page(cx);
                }
            }
        })
        .detach();
        s.load(false, cx);
        s.load_history(cx);
        s.load_daily(cx);
        s.load_qa(cx);
        s.load_questions(cx);
        s.load_reviewers(cx);
        s.load_info(cx);
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
                let _ = this.update(cx, |s, cx| {
                    // a window left open keeps its page current
                    if tick % IDLE_TICKS == 0 && s.view != Sec::Accounts {
                        s.reload_page(cx);
                    }
                    // the updater runs on its own thread: redraw System as its state moves
                    if s.view == Sec::System {
                        cx.notify();
                    }
                });
            }
        })
        .detach();
        s
    }

    /// Asks the board again for what the shown page reads.
    fn reload_page(&self, cx: &mut Context<Self>) {
        match self.view {
            Sec::Accounts => self.load(false, cx),
            Sec::History => self.load_history(cx),
            Sec::Usage => self.load_daily(cx),
            Sec::Qa => self.load_qa(cx),
            Sec::Questions => self.load_questions(cx),
            Sec::Reviewers => self.load_reviewers(cx),
            Sec::Worktrees => self.load_slots(cx),
            Sec::System => self.load_info(cx),
        }
    }

    fn load_history(&self, cx: &mut Context<Self>) {
        let backend = self.backend.clone();
        cx.spawn(async move |this, cx| {
            let r = cx.background_executor().spawn(async move { backend.get("history", &[]) }).await;
            let _ = this.update(cx, |s, cx| {
                if let Ok(v) = r {
                    s.history = Some(v);
                }
                cx.notify();
            });
        })
        .detach();
    }

    fn load_info(&self, cx: &mut Context<Self>) {
        let backend = self.backend.clone();
        cx.spawn(async move |this, cx| {
            let b = backend.clone();
            let r = cx.background_executor().spawn(async move { backend.get("info", &[]) }).await;
            let restart = cx.background_executor().spawn(async move { b.get("restart", &[]) }).await;
            crate::updater::learn_restart(restart.ok().as_ref());
            let _ = this.update(cx, |s, cx| {
                s.info = r.ok();
                cx.notify();
            });
        })
        .detach();
    }

    /// `GET /slots` measures the slots on disk, so it's asked only while Worktrees shows.
    fn load_slots(&self, cx: &mut Context<Self>) {
        let backend = self.backend.clone();
        cx.spawn(async move |this, cx| {
            let r = cx.background_executor().spawn(async move { backend.get("slots", &[]) }).await;
            let _ = this.update(cx, |s, cx| {
                if let Ok(v) = r {
                    s.slots = Some(v);
                }
                cx.notify();
            });
        })
        .detach();
    }

    fn load_reviewers(&self, cx: &mut Context<Self>) {
        let backend = self.backend.clone();
        cx.spawn(async move |this, cx| {
            let r = cx.background_executor().spawn(async move { backend.get("reviewers", &[("project", "all".to_string())]) }).await;
            let _ = this.update(cx, |s, cx| {
                if let Ok(v) = r {
                    s.reviewers = Some(v);
                }
                cx.notify();
            });
        })
        .detach();
    }

    fn load_daily(&self, cx: &mut Context<Self>) {
        let backend = self.backend.clone();
        cx.spawn(async move |this, cx| {
            let r = cx.background_executor().spawn(async move { backend.get("usage", &[]) }).await;
            let _ = this.update(cx, |s, cx| {
                if let Ok(v) = r {
                    s.daily = Some(daily_of(&v));
                }
                cx.notify();
            });
        })
        .detach();
    }

    /// `POST usage/daily {base?, carry?}` or `usage/override {off?}`; both answer with `GET /usage`.
    fn post_daily(&mut self, path: &'static str, body: Value, cx: &mut Context<Self>) {
        if self.daily_busy {
            return;
        }
        self.daily_busy = true;
        self.daily_note = None;
        cx.notify();
        let backend = self.backend.clone();
        cx.spawn(async move |this, cx| {
            let r = cx.background_executor().spawn(async move { backend.post(path, body) }).await;
            let _ = this.update(cx, |s, cx| {
                s.daily_busy = false;
                match r {
                    Ok(v) => s.daily = Some(daily_of(&v)),
                    Err(e) => s.daily_note = Some((e.message, true)),
                }
                cx.notify();
            });
        })
        .detach();
    }

    fn load_qa(&self, cx: &mut Context<Self>) {
        let backend = self.backend.clone();
        cx.spawn(async move |this, cx| {
            let r = cx.background_executor().spawn(async move { backend.get("qa", &[]) }).await;
            let _ = this.update(cx, |s, cx| {
                if let Ok(v) = r {
                    s.qa = Some(v);
                }
                cx.notify();
            });
        })
        .detach();
    }

    fn load_questions(&self, cx: &mut Context<Self>) {
        let backend = self.backend.clone();
        cx.spawn(async move |this, cx| {
            let r = cx.background_executor().spawn(async move { backend.get("buried-questions", &[]) }).await;
            let _ = this.update(cx, |s, cx| {
                if let Ok(v) = r {
                    s.questions = Some(v);
                }
                cx.notify();
            });
        })
        .detach();
    }

    /// `POST buried-questions {on}`: answers with its state.
    fn post_questions(&mut self, on: bool, cx: &mut Context<Self>) {
        if self.questions_busy {
            return;
        }
        self.questions_busy = true;
        self.questions_note = None;
        cx.notify();
        let backend = self.backend.clone();
        cx.spawn(async move |this, cx| {
            let r = cx.background_executor().spawn(async move { backend.post("buried-questions", json!({"on": on})) }).await;
            let _ = this.update(cx, |s, cx| {
                s.questions_busy = false;
                match r {
                    Ok(v) => s.questions = Some(v),
                    Err(e) => s.questions_note = Some((e.message, true)),
                }
                cx.notify();
            });
        })
        .detach();
    }

    /// `POST qa {on}`: answers with the QA state.
    fn post_qa(&mut self, on: bool, cx: &mut Context<Self>) {
        if self.qa_busy {
            return;
        }
        self.qa_busy = true;
        self.qa_note = None;
        cx.notify();
        let backend = self.backend.clone();
        cx.spawn(async move |this, cx| {
            let r = cx.background_executor().spawn(async move { backend.post("qa", json!({"on": on})) }).await;
            let _ = this.update(cx, |s, cx| {
                s.qa_busy = false;
                match r {
                    Ok(v) => s.qa = Some(v),
                    Err(e) => s.qa_note = Some((e.message, true)),
                }
                cx.notify();
            });
        })
        .detach();
    }

    /// A retention choice: the board says first what the next cleanup would remove under it. Nothing
    /// to remove saves it; otherwise the page asks.
    fn preview_history(&mut self, key: &'static str, n: i64, cx: &mut Context<Self>) {
        if self.history_busy {
            return;
        }
        self.history_busy = true;
        self.history_note = None;
        self.history_confirm = None;
        cx.notify();
        let backend = self.backend.clone();
        cx.spawn(async move |this, cx| {
            let r = cx.background_executor().spawn(async move { backend.post("history", json!({key: n, "preview": true})) }).await;
            let _ = this.update(cx, |s, cx| {
                s.history_busy = false;
                match r {
                    Ok(v) => match removal_line(&v["removes"]) {
                        None => s.post_history("history", json!({key: n}), cx),
                        Some(line) => s.history_confirm = Some((json!({key: n}), line)),
                    },
                    Err(e) => s.history_note = Some((e.message, true)),
                }
                cx.notify();
            });
        })
        .detach();
    }

    /// `POST history` (a setting) or `history/cleanup`; both answer with the History state.
    fn post_history(&mut self, path: &'static str, body: Value, cx: &mut Context<Self>) {
        if self.history_busy {
            return;
        }
        self.history_busy = true;
        self.history_note = None;
        self.history_confirm = None;
        cx.notify();
        let backend = self.backend.clone();
        cx.spawn(async move |this, cx| {
            let r = cx.background_executor().spawn(async move { backend.post(path, body) }).await;
            let _ = this.update(cx, |s, cx| {
                s.history_busy = false;
                match r {
                    Ok(v) if path == "history/cleanup" => {
                        let n = &v["removed"];
                        let total = ["events", "status_changes", "terminal_lines"].iter().map(|k| n[k].as_i64().unwrap_or(0)).sum::<i64>();
                        let days = n["day_rows"].as_i64().unwrap_or(0);
                        s.history_note = Some((
                            if total + days == 0 { "Nothing was old enough to remove.".into() } else { format!("Removed {total} old events and {days} day summaries.") },
                            false,
                        ));
                        s.history = Some(v["history"].clone());
                    }
                    Ok(v) => s.history = Some(v),
                    Err(e) => s.history_note = Some((e.message, true)),
                }
                cx.notify();
            });
        })
        .detach();
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
        if !before && gh["connected"] == true {
            self.gh_form = false;
        }
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

    /// Copies the token `gh` already uses; gh's own sign-in isn't changed.
    fn import_gh(&mut self, cx: &mut Context<Self>) {
        self.post("github", "accounts/github/import".into(), json!({}), cx, |s, _| {
            s.gh_form = false;
            let a = s.account("github");
            s.notes.insert("github", (format!("Using gh's sign-in: {}.", a["detail"].as_str().unwrap_or("connected")), false));
        });
    }

    fn connect(&mut self, id: &'static str, cx: &mut Context<Self>) {
        let body = match id {
            "github" => json!({"token": self.gh_token.text(cx)}),
            "bitbucket" => json!({"email": self.bb_email.text(cx), "token": self.bb_token.text(cx)}),
            "azure" => json!({"organization": self.az_org.text(cx), "token": self.az_token.text(cx)}),
            _ => json!({"token": self.slack_token.text(cx)}),
        };
        self.post(id, format!("accounts/{id}"), body, cx, move |s, cx| {
            let field = match id {
                "github" => &s.gh_token,
                "bitbucket" => &s.bb_token,
                "azure" => &s.az_token,
                _ => &s.slack_token,
            };
            field.clear(cx);
            s.gh_form = false;
            s.bb_form = false;
            s.az_form = false;
            s.slack_form = false;
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
pub(crate) fn drag_window(ev: &MouseDownEvent, window: &mut Window, _: &mut App) {
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
            Sec::History => self.history_page(&t, cx).into_any_element(),
            Sec::Usage => self.usage_page(&t, cx).into_any_element(),
            Sec::Qa => self.qa_page(&t, cx).into_any_element(),
            Sec::Questions => self.questions_page(&t, cx).into_any_element(),
            Sec::Reviewers => self.reviewers_page(&t).into_any_element(),
            Sec::Worktrees => self.worktrees_page(&t).into_any_element(),
            Sec::System => self.system_page(&t, cx).into_any_element(),
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
                        d.child(div().mx(px(28.)).mb(px(10.)).px(px(12.)).py(px(8.)).rounded(px(8.)).bg(t.warn_soft).text_color(t.warn_fg).text_size(px(12.)).child(format!("The board didn't answer: {e}")))
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
            // amber count: accounts whose sign-in stopped working or lacks scopes
            let n = match sec {
                Sec::Accounts => self.accounts.iter().filter(|a| needs_owner(a)).count(),
                // an update waiting for a restart
                Sec::System => matches!(crate::updater::state(), crate::updater::State::Ready { .. }) as usize,
                _ => 0,
            };
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
                            div().min_w(px(18.)).h(px(18.)).px(px(5.)).rounded(px(9.)).flex().items_center().justify_center().text_size(px(11.)).font_weight(FontWeight::BOLD).bg(t.warn).text_color(t.on_status).child(n.to_string()),
                        )
                    })
                    .on_click(cx.listener(move |s, _, _, cx| {
                        s.view = sec;
                        s.confirm = None;
                        s.reload_page(cx);
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
                    .child(div().flex_1().child(if live { "The board · Running" } else if self.loaded || self.error.is_some() { "The board · Not answering" } else { "The board · Connecting…" }))
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
            .when(self.view == Sec::Accounts, |d| d.child(
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
            ))
    }

    fn accounts_page(&self, t: &Theme, window: &Window, cx: &mut Context<Self>) -> impl IntoElement {
        let list = div().id("settings-rows").flex_1().min_h_0().overflow_y_scroll().track_scroll(&self.scroll).flex().flex_col().gap(px(10.)).px(px(28.)).pt(px(4.)).pb(px(28.));
        if !self.loaded && self.error.is_none() {
            return list.child(kit::empty(t, "Asking the board…"));
        }
        let mut card = kit::card(t).overflow_hidden();
        for (i, (id, mark)) in CARDS.into_iter().enumerate() {
            let a = self.account(id);
            card = card.child(self.status_row(t, id, mark, &a, i == 0, cx));
            let rows: Vec<AnyElement> = match id {
                "github" => self.github_rows(t, &a, window, cx),
                "bitbucket" => self.bitbucket_rows(t, &a, window, cx),
                "azure" => self.azure_rows(t, &a, window, cx),
                _ => self.slack_rows(t, &a, window, cx),
            };
            for r in rows {
                card = card.child(r);
            }
            if let Some((text, bad)) = self.notes.get(id) {
                card = card.child(row(t, &format!("{id}-note"), false).bg(if *bad { t.down_soft } else { t.up_soft }).text_color(if *bad { t.down } else { t.up_fg }).text_size(px(12.)).child(text.clone()));
            }
            if self.cli {
                card = card.child(self.cli_line(t, id, "tb accounts", cx)).child(self.cli_line(t, &format!("{id}-use"), agent_cli(id), cx));
            }
        }
        list.child(card).child(div().px(px(4.)).text_size(px(11.5)).line_height(px(16.)).text_color(t.muted).child("Tokens stay in your login Keychain."))
    }

    // -------------------------------------------------------------- accounts

    /// An account's one row: its mark, name and status, and what can be done with it. Disconnect asks
    /// first, in the row; a refused sign-in turns the row amber with "Sign in again".
    fn status_row(&self, t: &Theme, id: &'static str, mark: &str, a: &Value, first: bool, cx: &mut Context<Self>) -> AnyElement {
        let busy = self.busy == Some(id);
        let label = a["label"].as_str().unwrap_or(id).to_string();
        let connected = a["connected"] == true;
        let (status, refused) = account_line(id, a);
        let asking = self.confirm == Some(id) && connected;
        let tile = div()
            .size(px(30.))
            .flex()
            .flex_none()
            .items_center()
            .justify_center()
            .rounded(px(7.))
            .bg(t.panel_2)
            .text_color(t.muted)
            .text_size(px(11.))
            .font_weight(FontWeight::BOLD)
            .child(mark.to_string());
        let (name, note) = if asking { (format!("Forget the {label} sign-in?"), forget_means(id).to_string()) } else { (label, status) };
        let warn = refused && !asking;
        let words = div()
            .id(SharedString::from(format!("{id}-status")))
            .flex_1()
            .min_w(px(150.))
            .flex()
            .flex_col()
            .child(div().font_weight(FontWeight::SEMIBOLD).when(warn, |d| d.text_color(t.warn_fg)).child(name))
            .child(div().text_size(px(12.)).line_height(px(16.)).text_color(if warn { t.warn_fg } else { t.muted }).child(note))
            .when_some(a["error"].as_str().filter(|_| warn).map(str::to_string), |d, e| d.tooltip(kit::tip(e)));
        let small = |b: Stateful<Div>| b.h(px(26.)).text_size(px(12.));
        let mut ctl = controls().flex_wrap();
        if asking {
            ctl = ctl
                .child(kit::btn_small(t, SharedString::from(format!("{id}-keep")), "Cancel").on_click(cx.listener(|s, _, _, cx| {
                    s.confirm = None;
                    cx.notify();
                })))
                .child(small(kit::btn_danger(t, SharedString::from(format!("{id}-forget")), "Disconnect")).on_click(cx.listener(move |s, _, _, cx| s.disconnect(id, cx))));
        } else if connected {
            let first = if refused {
                let b = small(kit::btn_primary(t, SharedString::from(format!("{id}-again")), if busy { "Starting…" } else { "Sign in again" }));
                if busy { kit::disabled(b) } else { b.on_click(cx.listener(move |s, _, window, cx| s.sign_in_again(id, window, cx))) }
            } else {
                let b = kit::btn_small(t, SharedString::from(format!("{id}-check")), if busy { "Checking…" } else { "Check" });
                if busy { kit::disabled(b) } else { b.tooltip(kit::tip("Ask the service whether the token still works")).on_click(cx.listener(move |s, _, _, cx| s.post(id, format!("accounts/{id}/check"), json!({}), cx, |_, _| {}))) }
            };
            let out = small(kit::btn_danger(t, SharedString::from(format!("{id}-out")), "Disconnect"));
            let out = if busy { kit::disabled(out) } else { out.tooltip(kit::tip("Forgets the board's token. gh and git on this Mac aren't touched.")).on_click(cx.listener(move |s, _, _, cx| s.ask_disconnect(id, cx))) };
            ctl = ctl.child(first).child(div().w(px(1.)).h(px(18.)).mx(px(2.)).bg(t.border)).child(out);
        } else if self.login().is_none() || id != "github" {
            let open = self.form_open(id);
            ctl = ctl.child(kit::btn_small(t, SharedString::from(format!("{id}-connect-open")), if open { "Cancel" } else { "Connect…" }).on_click(cx.listener(move |s, _, window, cx| {
                if s.form_open(id) { s.close_form(id, cx) } else { s.open_form(id, window, cx) }
            })));
        }
        row(t, &format!("{id}-row"), first)
            .min_h(px(56.))
            .when(warn, |d| d.bg(t.warn_soft))
            .when(asking, |d| d.bg(t.down_soft))
            .child(tile)
            .child(words)
            .child(ctl)
            .into_any_element()
    }

    fn form_open(&self, id: &str) -> bool {
        match id {
            "github" => self.gh_form,
            "bitbucket" => self.bb_form,
            "azure" => self.az_form,
            _ => self.slack_form,
        }
    }

    /// Connect… opens an account's form, its first field ready to type in.
    fn open_form(&mut self, id: &'static str, window: &mut Window, cx: &mut Context<Self>) {
        let focus = match id {
            "github" => {
                self.gh_form = true;
                &self.gh_token.focus
            }
            "bitbucket" => {
                self.bb_form = true;
                &self.bb_email.focus
            }
            "azure" => {
                self.az_form = true;
                &self.az_org.focus
            }
            _ => {
                self.slack_form = true;
                &self.slack_token.focus
            }
        };
        window.focus(focus, cx);
        cx.notify();
    }

    fn close_form(&mut self, id: &str, cx: &mut Context<Self>) {
        match id {
            "github" => self.gh_form = false,
            "bitbucket" => self.bb_form = false,
            "azure" => self.az_form = false,
            _ => self.slack_form = false,
        }
        cx.notify();
    }

    /// A refused sign-in: GitHub signs in with the browser again when gh is here; the rest (and
    /// GitHub without gh) open their form for a new token.
    fn sign_in_again(&mut self, id: &'static str, window: &mut Window, cx: &mut Context<Self>) {
        if id == "github" && self.account("github")["gh"] != false {
            self.sign_in(cx);
        } else {
            self.open_form(id, window, cx);
        }
    }

    /// Disconnect only asks; the row says what forgetting the sign-in stops.
    fn ask_disconnect(&mut self, id: &'static str, cx: &mut Context<Self>) {
        self.confirm = Some(id);
        cx.notify();
    }

    fn disconnect(&mut self, id: &'static str, cx: &mut Context<Self>) {
        self.confirm = None;
        self.post(id, format!("accounts/{id}/disconnect"), json!({}), cx, |_, _| {});
    }

    /// GitHub's rows under its status: the sign-in code while the browser sign-in waits, a sign-in
    /// missing scopes, and the form Connect… opens.
    fn github_rows(&self, t: &Theme, a: &Value, window: &Window, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let busy = self.busy == Some("github");
        let has_gh = a["gh"] != false;
        let connected = a["connected"] == true;
        let mut rows = vec![];
        if let Some((code, url)) = self.login() {
            rows.push(self.github_code(t, code, url, cx));
            return rows;
        }
        if connected && a["reauth"] == true {
            let btn = if !has_gh {
                kit::btn_primary(t, "gh-reauth", "Use a new token").h(px(26.)).text_size(px(12.)).on_click(cx.listener(|s, _, window, cx| s.open_form("github", window, cx)))
            } else if busy {
                kit::disabled(kit::btn_primary(t, "gh-reauth", "Starting…").h(px(26.)).text_size(px(12.)))
            } else {
                kit::btn_primary(t, "gh-reauth", "Sign in again").h(px(26.)).text_size(px(12.)).on_click(cx.listener(|s, _, _, cx| s.sign_in(cx)))
            };
            rows.push(reauth_row(t, "github", a, "Sign in again (or use a token that has them).", btn.into_any_element()));
        }
        if !self.gh_form {
            rows.extend(error_row(t, "github", a));
            return rows;
        }
        if !connected {
            let sign_in = if busy { kit::disabled(kit::btn_primary(t, "gh-sign-in", "Starting…").h(px(26.)).text_size(px(12.))) } else {
                kit::btn_primary(t, "gh-sign-in", "Sign in with GitHub").h(px(26.)).text_size(px(12.)).on_click(cx.listener(|s, _, _, cx| s.sign_in(cx)))
            };
            let note = if has_gh {
                "Opens GitHub in your browser with a one-time code. The board keeps its own token; gh's sign-in isn't changed."
            } else {
                "Paste a token below, or install the GitHub CLI (brew install gh) to sign in with the browser."
            };
            rows.push(
                line(t, "gh-out", false, false, "Sign in with the browser", note, None)
                    .when(has_gh, |d| {
                        d.child(
                            controls()
                                .flex_wrap()
                                .child(
                                    kit::btn_small(t, "gh-import", "Use my gh sign-in")
                                        .tooltip(kit::tip("Copies the token gh already uses to the board. gh isn't changed."))
                                        .on_click(cx.listener(|s, _, _, cx| s.import_gh(cx))),
                                )
                                .child(sign_in),
                        )
                    })
                    .into_any_element(),
            );
        }
        rows.push(self.token_row(t, "github", "Token", "A classic token with repo, read:org and workflow, or a fine-grained one with Contents, Pull requests and Workflows (read and write).", &self.gh_token, window, cx, false));
        rows.extend(error_row(t, "github", a));
        rows
    }

    fn bitbucket_rows(&self, t: &Theme, a: &Value, window: &Window, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let mut rows = vec![];
        if a["connected"] == true {
            if a["reauth"] == true && !self.bb_form {
                let btn = kit::btn_primary(t, "bb-reauth", "Use a new token").h(px(26.)).text_size(px(12.)).on_click(cx.listener(|s, _, window, cx| s.open_form("bitbucket", window, cx)));
                rows.push(reauth_row(t, "bitbucket", a, "Make a new API token with them and connect it.", btn.into_any_element()));
            } else if let Some(note) = a["note"].as_str() {
                rows.push(off_row(t, "bitbucket", a, note));
            }
        }
        if self.bb_form {
            let email = self.bb_email.render(t, "bb-email", window).flex_1().min_w(px(140.)).max_w(px(260.)).on_key_down(self.field_keys("bitbucket", &self.bb_email, cx));
            rows.push(line(t, "bb-email-row", false, false, "Atlassian email", "The email you sign in to Bitbucket with.", None).child(field_box().child(email)).into_any_element());
            rows.push(self.token_row(
                t,
                "bitbucket",
                "API token",
                "An Atlassian API token with Bitbucket scopes: read and write repository, read and write pullrequest, read:user for your name, write:pipeline to cancel builds.",
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

    fn azure_rows(&self, t: &Theme, a: &Value, window: &Window, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let mut rows = vec![];
        if a["connected"] == true {
            if a["reauth"] == true && !self.az_form {
                let btn = kit::btn_primary(t, "az-reauth", "Use a new token").h(px(26.)).text_size(px(12.)).on_click(cx.listener(|s, _, window, cx| s.open_form("azure", window, cx)));
                rows.push(reauth_row(t, "azure", a, "Edit the token in Azure DevOps (or make a new one) to add them, and connect it.", btn.into_any_element()));
            } else if let Some(note) = a["note"].as_str() {
                rows.push(off_row(t, "azure", a, note));
            }
        }
        if self.az_form {
            let org = self.az_org.render(t, "az-org", window).flex_1().min_w(px(140.)).max_w(px(260.)).on_key_down(self.field_keys("azure", &self.az_org, cx));
            rows.push(line(t, "az-org-row", false, false, "Organization", "The organization your pipelines run in.", None).child(field_box().child(org)).into_any_element());
            rows.push(self.token_row(t, "azure", "Personal access token", "A token with Build (Read) and Test Management (Read).", &self.az_token, window, cx, false));
            rows.push(link_row(t, "az-create", "How to make a personal access token", taskboardd::accounts::AZURE_TOKEN_URL, cx));
        }
        rows.extend(error_row(t, "azure", a));
        rows
    }

    fn slack_rows(&self, t: &Theme, a: &Value, window: &Window, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let mut rows = vec![];
        if self.slack_form {
            rows.push(self.token_row(t, "slack", "Token", "A user (xoxp-) or bot (xoxb-) token from your Slack app's OAuth & Permissions page.", &self.slack_token, window, cx, false));
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

    /// A secret field with Connect (↩).
    #[allow(clippy::too_many_arguments)]
    fn token_row(&self, t: &Theme, id: &'static str, label: &str, note: &str, field: &Input, window: &Window, cx: &mut Context<Self>, first: bool) -> AnyElement {
        let busy = self.busy == Some(id);
        let input = field.render(t, SharedString::from(format!("{id}-token")), window).flex_1().min_w(px(140.)).max_w(px(260.)).on_key_down(self.field_keys(id, field, cx));
        let btn = kit::btn_primary(t, SharedString::from(format!("{id}-connect")), if busy { "Checking…" } else { "Connect" }).h(px(32.)).text_size(px(12.5));
        let btn = if busy { kit::disabled(btn) } else { btn.on_click(cx.listener(move |s, _, _, cx| s.connect(id, cx))) };
        line(t, &format!("{id}-token-row"), first, false, label, note, None).child(field_box().child(input).child(btn)).into_any_element()
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

// ------------------------------------------------------------------ history

/// "30 days", "6 months", "1 year", "Always".
fn days_label(n: i64) -> String {
    match n {
        0 => "Always".into(),
        365 => "1 year".into(),
        730 => "2 years".into(),
        n if n % 30 == 0 && n >= 180 => format!("{} months", n / 30),
        n => format!("{n} days"),
    }
}

/// A retention control from `GET /history`: its choices, and which one the board keeps now. The
/// board names them `detail_choices` and `summary_choices` beside `detail_days` and `summary_days`.
fn retention(h: &Value, key: &str) -> (Vec<i64>, Option<usize>) {
    let choices: Vec<i64> = h[format!("{}_choices", key.trim_end_matches("_days"))].as_array().map(|a| a.iter().filter_map(|x| x.as_i64()).collect()).unwrap_or_default();
    let on = h[key].as_i64().and_then(|cur| choices.iter().position(|n| *n == cur));
    (choices, on)
}

/// What a retention change would remove (the board's preview), or None when it removes nothing.
fn removal_line(removes: &Value) -> Option<String> {
    let n = |k: &str| removes[k].as_i64().unwrap_or(0);
    let (events, days) = (n("events_total"), n("days"));
    let mut parts = vec![];
    if events > 0 {
        parts.push(crate::fmt::plural(events, "event", "events"));
    }
    if days > 0 {
        parts.push(format!("the summaries of {}", crate::fmt::plural(days, "day", "days")));
    } else if n("day_rows") > 0 {
        parts.push(crate::fmt::plural(n("day_rows"), "day summary", "day summaries"));
    }
    if parts.is_empty() {
        return None;
    }
    Some(format!("The next cleanup, after 3 AM, removes {}. They can't be brought back.", parts.join(" and ")))
}

/// "4.2 MB", "820 KB".
fn bytes_label(b: i64) -> String {
    if b >= 1_000_000 { format!("{:.1} MB", b as f64 / 1_000_000.0) } else { format!("{} KB", (b as f64 / 1000.0).ceil() as i64) }
}

impl SettingsWindow {
    fn history_page(&self, t: &Theme, cx: &mut Context<Self>) -> impl IntoElement {
        let list = div().id("history-rows").flex_1().min_h_0().overflow_y_scroll().flex().flex_col().gap(px(20.)).px(px(28.)).pt(px(4.)).pb(px(28.));
        let Some(h) = self.history.clone() else {
            return list.child(kit::empty(t, "Asking the board…"));
        };
        let busy = self.history_busy;
        let pick = |key: &'static str, label: &str, note: &str, first: bool, cx: &mut Context<Self>| {
            let (choices, on) = retention(&h, key);
            let labels: Vec<String> = choices.iter().map(|n| days_label(*n)).collect();
            let refs: Vec<&str> = labels.iter().map(|s| s.as_str()).collect();
            let on = on.unwrap_or(usize::MAX);
            let control = kit::seg(t, &format!("hist-{key}"), &refs, on, |i, item| {
                let n = choices[i];
                if busy || i == on { item } else { item.on_click(cx.listener(move |s, _, _, cx| s.preview_history(key, n, cx))) }
            });
            line(t, &format!("hist-{key}-row"), first, false, label, note, None).child(controls().child(control))
        };
        let detail = pick("detail_days", "Keep every event for", "Commits, questions, checkpoints and status changes of finished tasks: the marks on a day's timeline and its task log.", true, cx);
        let summary = pick("summary_days", "Keep day summaries for", "One small record per day and project: the timeline bars and totals the week views use.", false, cx);
        let i = |k: &str| h[k].as_i64().unwrap_or(0);
        let used = line(
            t,
            "hist-used",
            false,
            false,
            "Space used",
            &format!("{} events ({}) and {} day summaries ({})", i("events"), bytes_label(i("events_bytes")), i("summaries"), bytes_label(i("summaries_bytes"))),
            None,
        );
        let last = h["last_cleanup"].as_str().map(crate::fmt::full_time).unwrap_or_else(|| "not yet".into());
        let button = if self.history_busy {
            kit::disabled(kit::btn_small(t, "hist-clean", "Cleaning up…"))
        } else {
            kit::btn_small(t, "hist-clean", "Clean up now").on_click(cx.listener(|s, _, _, cx| s.post_history("history/cleanup", json!({}), cx)))
        };
        let clean = line(t, "hist-clean-row", false, false, "Cleanup", &format!("Runs every night after 3 AM. Last run: {last}."), None).child(controls().child(button));
        let mut card = kit::card(t).overflow_hidden().child(detail).child(summary);
        if let Some((body, text)) = self.history_confirm.clone() {
            let ask = line(t, "hist-confirm", false, true, "Remove this history?", &text, None).child(
                controls()
                    .child(kit::btn_small(t, "hist-keep", "Cancel").on_click(cx.listener(|s, _, _, cx| {
                        s.history_confirm = None;
                        cx.notify();
                    })))
                    .child(kit::btn_danger(t, "hist-apply", "Change and remove").h(px(26.)).text_size(px(12.)).on_click(cx.listener(move |s, _, _, cx| s.post_history("history", body.clone(), cx)))),
            );
            card = card.child(ask);
        }
        card = card.child(used).child(clean);
        if let Some((text, bad)) = &self.history_note {
            card = card.child(row(t, "hist-note", false).bg(if *bad { t.down_soft } else { t.up_soft }).text_color(if *bad { t.down } else { t.up_fg }).text_size(px(12.)).child(text.clone()));
        }
        list.child(card)
    }
}

// ------------------------------------------------------------------ usage

/// `GET /usage` answers `null` while there's no reading and the limit is off: that's the plan switched off.
fn daily_of(usage: &Value) -> Value {
    match &usage["daily"] {
        Value::Null => serde_json::json!({"base": "off", "carry": "next_day", "on": false}),
        d => d.clone(),
    }
}

/// The daily limit's base choices: off, even, the fixed shares, and a fixed share set elsewhere
/// (`tb usage daily`) in its place among them.
fn daily_bases(cur: &Value) -> Vec<Value> {
    let mut pcts: Vec<f64> = vec![10., 15., 20., 25.];
    if let Some(n) = cur.as_f64().filter(|n| !pcts.contains(n)) {
        pcts.push(n);
        pcts.sort_by(f64::total_cmp);
    }
    [json!("off"), json!("even")].into_iter().chain(pcts.into_iter().map(|n| json!(n))).collect()
}

fn daily_base_label(b: &Value) -> String {
    match b.as_f64() {
        Some(n) => format!("{}%", crate::app::js_num(&json!(n))),
        None if b == "even" => "Even".into(),
        None => "Off".into(),
    }
}

impl SettingsWindow {
    fn usage_page(&self, t: &Theme, cx: &mut Context<Self>) -> impl IntoElement {
        let list = div().id("usage-rows").flex_1().min_h_0().overflow_y_scroll().flex().flex_col().gap(px(20.)).px(px(28.)).pt(px(4.)).pb(px(28.));
        let Some(d) = self.daily.clone() else {
            return list.child(kit::empty(t, "Asking the board…"));
        };
        let busy = self.daily_busy;
        let bases = daily_bases(&d["base"]);
        let labels: Vec<String> = bases.iter().map(daily_base_label).collect();
        let refs: Vec<&str> = labels.iter().map(|s| s.as_str()).collect();
        let on = bases.iter().position(|b| b.as_f64().zip(d["base"].as_f64()).map_or(*b == d["base"], |(a, c)| a == c)).unwrap_or(0);
        let base = kit::seg(t, "daily-base", &refs, on, |i, item| {
            let b = bases[i].clone();
            if busy || i == on {
                item
            } else {
                item.on_click(cx.listener(move |s, _, _, cx| s.post_daily("usage/daily", json!({"base": b}), cx)))
            }
        });
        let note = "Even splits what's left of the week over the days to its reset; a percent gives each day that share of the week. A change keeps what today already used.";
        let mut card = kit::card(t).overflow_hidden().child(line(t, "daily-base-row", true, false, "Daily limit", note, None).child(controls().child(base)));
        if d["on"] == true {
            let spread = d["carry"] == "spread";
            let carry = kit::seg(t, "daily-carry", &["Next day", "Spread"], spread as usize, |i, item| {
                if busy || (i == 1) == spread {
                    item
                } else {
                    item.on_click(cx.listener(move |s, _, _, cx| s.post_daily("usage/daily", json!({"carry": if i == 1 { "spread" } else { "next_day" }}), cx)))
                }
            });
            let carry_note = if spread { "What a day leaves is shared over the rest of the week." } else { "What a day leaves goes to the next day." };
            card = card.child(line(t, "daily-carry-row", false, false, "Unused share", carry_note, None).child(controls().child(carry)));
            let lifted = d["overridden"] == true;
            let today = d["line"].as_str().unwrap_or("No 7-day reading yet.");
            let toggle = kit::seg(t, "daily-override", &["Limit", "Lifted"], lifted as usize, |i, item| {
                if busy || (i == 1) == lifted {
                    item
                } else {
                    item.on_click(cx.listener(move |s, _, _, cx| s.post_daily("usage/override", json!({"off": i == 0}), cx)))
                }
            });
            card = card.child(line(t, "daily-today-row", false, d["limited"] == true, "Today", today, None).child(controls().child(toggle)));
        }
        if let Some((text, bad)) = &self.daily_note {
            card = card.child(row(t, "daily-note", false).bg(if *bad { t.down_soft } else { t.up_soft }).text_color(if *bad { t.down } else { t.up_fg }).text_size(px(12.)).child(text.clone()));
        }
        list.child(card)
    }
}

// ------------------------------------------------------------------ QA

impl SettingsWindow {
    fn qa_page(&self, t: &Theme, cx: &mut Context<Self>) -> impl IntoElement {
        let list = div().id("qa-rows").flex_1().min_h_0().overflow_y_scroll().flex().flex_col().gap(px(20.)).px(px(28.)).pt(px(4.)).pb(px(28.));
        let Some(q) = self.qa.clone() else {
            return list.child(kit::empty(t, "Asking the board…"));
        };
        let jira = q["jira"] == true;
        let on = q["on"] == true;
        let note = if jira {
            "The board reads new comments on its tickets every few minutes. A clear direction becomes a high-priority follow-up task; \
             a question waits for you on the goal's QA tab and in the alerts. Comments from before you switch it on are never read."
        } else {
            "Needs Jira: set up [jira] in ~/.config/taskboard/config.toml first."
        };
        let control = kit::seg(t, "qa-on", &["Off", "On"], if on { 1 } else { 0 }, |i, item| {
            if jira && !self.qa_busy {
                item.on_click(cx.listener(move |s, _, _, cx| s.post_qa(i == 1, cx)))
            } else {
                item
            }
        });
        let switch = line(t, "qa-on-row", true, !jira, "Read QA comments", note, None).child(controls().child(control));
        let mut card = kit::card(t).overflow_hidden().child(switch);
        if on {
            let i = |k: &str| q[k].as_i64().unwrap_or(0);
            let checked = q["checked_at"].as_str().map(crate::fmt::full_time).unwrap_or_else(|| "not yet".into());
            card = card.child(line(
                t,
                "qa-state",
                false,
                false,
                "Comments",
                &format!("{} read, {} waiting on you. Last checked Jira: {checked}.", i("comments"), i("waiting")),
                None,
            ));
        }
        if let Some((text, bad)) = &self.qa_note {
            card = card.child(row(t, "qa-note", false).bg(if *bad { t.down_soft } else { t.up_soft }).text_color(if *bad { t.down } else { t.up_fg }).text_size(px(12.)).child(text.clone()));
        }
        list.child(card)
    }
}

// ------------------------------------------------------------------ questions

impl SettingsWindow {
    fn questions_page(&self, t: &Theme, cx: &mut Context<Self>) -> impl IntoElement {
        let list = div().id("questions-rows").flex_1().min_h_0().overflow_y_scroll().flex().flex_col().gap(px(20.)).px(px(28.)).pt(px(4.)).pb(px(28.));
        let Some(q) = self.questions.clone() else {
            return list.child(kit::empty(t, "Asking the board…"));
        };
        let on = q["on"] != false;
        let note = "A task's reply that asks you something above its last paragraph is sent back once, to ask it with tb question or in a final Questions block, so it reaches you as a Question card.";
        let control = kit::seg(t, "buried-on", &["Off", "On"], if on { 1 } else { 0 }, |i, item| {
            if self.questions_busy {
                item
            } else {
                item.on_click(cx.listener(move |s, _, _, cx| s.post_questions(i == 1, cx)))
            }
        });
        let switch = line(t, "buried-on-row", true, false, "Send back buried questions", note, None).child(controls().child(control));
        let mut card = kit::card(t).overflow_hidden().child(switch);
        if let Some((text, bad)) = &self.questions_note {
            card = card.child(row(t, "buried-note", false).bg(if *bad { t.down_soft } else { t.up_soft }).text_color(if *bad { t.down } else { t.up_fg }).text_size(px(12.)).child(text.clone()));
        }
        list.child(card)
    }
}

// ------------------------------------------------------------------ system

impl SettingsWindow {
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
        let channel = match updater::updated() {
            Some((v, at)) => format!("Updated to {v} today at {at}."),
            None if updater::VERSION.contains("-beta.") => "Gets every release, betas included.".into(),
            None => "Gets public releases.".into(),
        };
        let (dot, status, note, button): (Hsla, String, String, Option<Stateful<Div>>) = match updater::state() {
            State::Off(why) => (t.faint, "Off".into(), why, None),
            State::Idle => (t.faint, "Not checked yet".into(), channel, Some(check("Check now", cx))),
            State::Checking => (t.faint, "Checking…".into(), channel, Some(kit::disabled(small(kit::btn_small(t, "sys-check", "Checking…"))))),
            State::UpToDate { at } => (t.up, format!("Up to date · checked at {at}"), channel, Some(check("Check now", cx))),
            State::Downloading { version } => (t.accent, format!("Downloading {version}…"), "Checked before it's offered.".into(), None),
            State::Ready { version, .. } => (
                t.accent,
                format!("{version} ready"),
                updater::restart_note(updater::restart_reviews()),
                Some(small(kit::btn_primary(t, "sys-apply", "Restart to update")).on_click(cx.listener(|s, _, _, cx| s.apply_update(cx)))),
            ),
            State::Applying { version } => (t.accent, format!("Installing {version}…"), "The app reopens in a moment.".into(), None),
            State::Failed { stage, error, at } => (t.down, format!("{} at {at}", State::failure_title(&stage)), format!("{stage}: {error}"), Some(check("Try again", cx))),
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
                let b = if behind { kit::btn_primary(t, "sys-restart", "Restart the board") } else { kit::btn_small(t, "sys-restart", "Restart the board") };
                small(b).on_click(cx.listener(|s, _, _, cx| s.restart_daemon(cx)))
            }
        });
        let daemon = line(t, "sys-daemon", false, behind, "Board version", &daemon_note, None).child(
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

    /// Open on Usage (the sidebar's usage block).
    pub fn show_usage(&mut self, cx: &mut Context<Self>) {
        self.view = Sec::Usage;
        self.load_daily(cx);
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
                    s.sys_note = Some((format!("Couldn't restart the board: {e}"), true));
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
    fn reviewers_page(&self, t: &Theme) -> impl IntoElement {
        let mut list = div().id("reviewer-rows").flex_1().min_h_0().overflow_y_scroll().flex().flex_col().gap(px(20.)).px(px(28.)).pt(px(4.)).pb(px(28.));
        let Some(v) = self.reviewers.clone() else {
            return list.child(kit::empty(t, "Asking the board…"));
        };
        let projects = v["projects"].as_array().cloned().unwrap_or_default();
        if projects.is_empty() {
            return list.child(kit::empty(t, "No reviewers"));
        }
        for p in projects {
            let name = p["name"].as_str().unwrap_or("").to_string();
            let mut card = kit::card(t).overflow_hidden();
            for (i, r) in p["reviewers"].as_array().cloned().unwrap_or_default().iter().enumerate() {
                let id = format!("rev-{}", r["id"]);
                let removed = r["removed"] == true;
                let mut l = line(t, &id, i == 0, false, r["name"].as_str().unwrap_or(""), &reviewer_note(r), None).when(removed, |d| d.text_color(t.muted));
                if r["pinned"] == true {
                    l = l.child(kit::tone_pill(t, "review", "Pinned"));
                }
                if removed {
                    l = l.child(kit::tone_pill(t, "neutral", "Never asked"));
                }
                card = card.child(l);
            }
            let heading = div().px(px(4.)).text_size(px(12.)).font_weight(FontWeight::SEMIBOLD).text_color(t.text).child(name);
            list = list.child(div().flex().flex_col().gap(px(8.)).child(heading).child(card));
        }
        list
    }
}

impl SettingsWindow {
    fn worktrees_page(&self, t: &Theme) -> impl IntoElement {
        let mut list = div().id("worktree-rows").flex_1().min_h_0().overflow_y_scroll().flex().flex_col().gap(px(20.)).px(px(28.)).pt(px(4.)).pb(px(28.));
        let Some(v) = self.slots.as_ref() else {
            return list.child(kit::empty(t, "Asking the board…"));
        };
        let pools = pools_vm(v);
        if pools.is_empty() {
            return list.child(kit::empty(t, "No worktree slots"));
        }
        for p in pools {
            let mut card = kit::card(t).overflow_hidden().child(line(t, &format!("pool-{}", p.project), true, false, "Pool", &p.settings, None));
            for r in &p.slots {
                let pill = match &r.held_by {
                    Some(task) => kit::tone_pill(t, "working", task.clone()).id(SharedString::from(format!("slot-tip-{}", r.key))).when_some(r.tip.clone(), |d, tip| d.tooltip(kit::tip(tip))).into_any_element(),
                    None => kit::tone_pill(t, "neutral", "Free").into_any_element(),
                };
                card = card.child(line(t, &format!("slot-{}", r.key), false, false, &r.name, &r.note, None).child(controls().child(pill)));
            }
            if !p.waiting.is_empty() {
                card = card.child(line(t, &format!("waiting-{}", p.project), false, true, &format!("{} waiting", p.waiting.len()), &p.waiting.join(", "), None));
            }
            let heading = div().px(px(4.)).text_size(px(12.)).font_weight(FontWeight::SEMIBOLD).text_color(t.text).child(format!("{} · {}", p.project, p.count));
            list = list.child(div().flex().flex_col().gap(px(8.)).child(heading).child(card));
        }
        list
    }
}

/// One project's pool in Worktrees.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct PoolVm {
    pub project: String,
    /// "6 slots".
    pub count: String,
    /// "Keeps 2 to 10 slots. Build output is cleared after 2 idle days, the slot removed after 7."
    pub settings: String,
    pub slots: Vec<SlotVm>,
    /// The tasks waiting for a slot of this project.
    pub waiting: Vec<String>,
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct SlotVm {
    pub key: String,
    pub name: String,
    /// "7.7 GB · last used 10:08 AM".
    pub note: String,
    /// "T12", the task holding it; None when it's free.
    pub held_by: Option<String>,
    /// The holder's title.
    pub tip: Option<String>,
}

/// `GET /slots` grouped by project, in the daemon's order (project, then slot number).
pub(crate) fn pools_vm(v: &Value) -> Vec<PoolVm> {
    let mut out: Vec<PoolVm> = vec![];
    for x in v["slots"].as_array().cloned().unwrap_or_default() {
        let project = x["project"].as_str().unwrap_or("").to_string();
        let i = match out.iter().position(|p| p.project == project) {
            Some(i) => i,
            None => {
                out.push(PoolVm { project: project.clone(), count: String::new(), settings: pool_settings(&v["pools"][&project]), slots: vec![], waiting: vec![] });
                out.len() - 1
            }
        };
        let p = &mut out[i];
        let held = x["held_by"].as_str().map(str::to_string);
        let size = x["size_kb"].as_i64().map(disk_size);
        let used = x["used_at"].as_str().map(used_at).filter(|u| !u.is_empty()).map(|u| format!("last used {u}"));
        p.slots.push(SlotVm {
            key: x["path"].as_str().unwrap_or("").to_string(),
            name: x["name"].as_str().unwrap_or("").to_string(),
            note: [size, used].into_iter().flatten().collect::<Vec<_>>().join(" · "),
            tip: held.as_ref().and_then(|_| x["title"].as_str()).filter(|t| !t.is_empty()).map(str::to_string),
            held_by: held,
        });
        // Every slot of a repo lists the same waiting tasks; each shows once.
        for w in x["waiting"].as_array().cloned().unwrap_or_default().iter().filter_map(|w| w.as_str()) {
            if !p.waiting.iter().any(|have| have == w) {
                p.waiting.push(w.to_string());
            }
        }
    }
    for p in &mut out {
        p.count = crate::fmt::plural(p.slots.len() as i64, "slot", "slots");
    }
    out
}

/// "Keeps 2 to 10 slots. Build output is cleared after 2 idle days, the slot removed after 7."
fn pool_settings(c: &Value) -> String {
    let (Some(min), Some(max), Some(trim), Some(remove)) = (c["min_slots"].as_i64(), c["max_slots"].as_i64(), c["idle_trim_days"].as_i64(), c["idle_remove_days"].as_i64()) else {
        return String::new();
    };
    format!("Keeps {min} to {max} slots. Build output is cleared after {}, the slot removed after {remove}.", crate::fmt::plural(trim, "idle day", "idle days"))
}

/// "512 KB", "840 MB", "2.3 GB".
fn disk_size(kb: i64) -> String {
    match kb {
        k if k >= 1024 * 1024 => format!("{:.1} GB", k as f64 / (1024.0 * 1024.0)),
        k if k >= 1024 => format!("{} MB", k / 1024),
        k => format!("{k} KB"),
    }
}

/// "3:05 PM" today, "Oct 7, 3:05 PM" another day.
fn used_at(iso: &str) -> String {
    let Some(t) = crate::fmt::parse(iso) else { return String::new() };
    let l = crate::fmt::local(t);
    if crate::fmt::is_today(t) { l.format("%-I:%M %p").to_string() } else { l.format("%b %-d, %-I:%M %p").to_string() }
}

// ------------------------------------------------------------------ rows

/// A row's shell: padding and the line above every row but the first.
fn row(t: &Theme, id: &str, first: bool) -> Stateful<Div> {
    div().id(SharedString::from(id.to_string())).flex().flex_wrap().items_center().gap_x(px(16.)).gap_y(px(8.)).px(px(16.)).py(px(9.)).min_h(px(46.)).when(!first, |d| d.border_t_1().border_color(t.border))
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

/// Controls holding a text field: they shrink with the window, down to a usable field.
fn field_box() -> Div {
    div().flex().flex_1().min_w(px(220.)).justify_end().items_center().gap(px(8.))
}

/// Whether an account needs the owner: its sign-in stopped working or lacks scopes Taskboard needs.
fn needs_owner(a: &Value) -> bool {
    a["connected"] == true && (a["error"].is_string() || a["reauth"] == true)
}

/// Amber: the scopes Taskboard now needs that this sign-in lacks, and how to give them.
fn reauth_row(t: &Theme, id: &str, a: &Value, how: &str, button: AnyElement) -> AnyElement {
    let missing: Vec<&str> = a["missing"].as_array().map(|m| m.iter().filter_map(|x| x.as_str()).collect()).unwrap_or_default();
    line(t, &format!("{id}-reauth"), false, true, "Needs a new sign-in", &format!("The board now needs {}. {how}", missing.join(", ")), None)
        .child(div().flex().flex_wrap().gap(px(4.)).max_w(px(200.)).justify_end().children(missing.iter().map(|s| kit::chip(t, s.to_string()))))
        .child(controls().child(button))
        .into_any_element()
}

/// What the token's scopes leave off ("Can read PRs; add read:user:bitbucket to show your Bitbucket
/// name."), with the scopes it lacks. Read-only: a new token is made on the service.
fn off_row(t: &Theme, id: &str, a: &Value, note: &str) -> AnyElement {
    let lacks: Vec<&str> = a["missing_scopes"].as_array().map(|m| m.iter().filter_map(|x| x.as_str()).collect()).unwrap_or_default();
    line(t, &format!("{id}-off"), false, false, note, "", None)
        .child(div().flex().flex_wrap().gap(px(4.)).max_w(px(260.)).justify_end().children(lacks.iter().map(|s| kit::chip(t, s.to_string()))))
        .into_any_element()
}

/// Why a sign-in that isn't connected failed (a connected one's refusal is its status row).
fn error_row(t: &Theme, id: &str, a: &Value) -> Option<AnyElement> {
    let e = a["error"].as_str().filter(|_| a["connected"] != true)?;
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

    #[::core::prelude::v1::test]
    fn the_daily_base_choices_keep_a_share_set_from_tb_in_order() {
        let labels = |cur| super::daily_bases(&cur).iter().map(super::daily_base_label).collect::<Vec<_>>();
        assert_eq!(labels(json!("off")), ["Off", "Even", "10%", "15%", "20%", "25%"]);
        assert_eq!(labels(json!(12.5)), ["Off", "Even", "10%", "12.5%", "15%", "20%", "25%"]);
        assert_eq!(labels(json!(15)), ["Off", "Even", "10%", "15%", "20%", "25%"]);
    }

    #[gpui_kit::test]
    fn the_daily_limit_sets_its_base_and_carry_and_lifts_today(cx: &mut gpui_kit::TestAppContext) {
        let (w, rec) = window(cx);
        w.update(cx, |s, _, cx| {
            assert_eq!(s.daily.as_ref().unwrap()["on"], false, "off until switched on");
            s.view = super::Sec::Usage;
            s.post_daily("usage/daily", json!({"base": "even"}), cx);
        })
        .unwrap();
        settle(cx);
        w.update(cx, |s, _, cx| {
            let d = s.daily.clone().unwrap();
            assert_eq!((d["on"].clone(), d["base"].clone(), d["carry"].clone()), (json!(true), json!("even"), json!("next_day")));
            s.post_daily("usage/daily", json!({"carry": "spread"}), cx);
        })
        .unwrap();
        settle(cx);
        w.update(cx, |s, _, cx| {
            assert_eq!(s.daily.as_ref().unwrap()["carry"], "spread");
            s.post_daily("usage/daily", json!({"base": 0}), cx);
        })
        .unwrap();
        settle(cx);
        w.update(cx, |s, _, _| {
            assert_eq!(s.daily.as_ref().unwrap()["base"], "even", "a bad share is refused and kept");
            assert!(s.daily_note.as_ref().is_some_and(|(m, bad)| *bad && m.contains("1 to 100")), "{:?}", s.daily_note);
        })
        .unwrap();
        w.update(cx, |s, _, cx| s.post_daily("usage/override", json!({"off": false}), cx)).unwrap();
        settle(cx);
        w.update(cx, |s, _, cx| s.post_daily("usage/override", json!({"off": true}), cx)).unwrap();
        settle(cx);
        let posts: Vec<_> = rec.posts().into_iter().filter(|(p, _)| p.starts_with("usage/")).collect();
        assert_eq!(posts.len(), 5);
        assert_eq!(posts[3], ("usage/override".to_string(), json!({"off": false})));
    }

    #[gpui_kit::test]
    fn system_shows_the_daemon_s_version_beside_the_app_s(cx: &mut gpui_kit::TestAppContext) {
        let (w, _rec) = window(cx);
        w.update(cx, |s, window, cx| {
            assert_eq!(s.info.as_ref().unwrap()["version"], env!("CARGO_PKG_VERSION"));
            s.view = super::Sec::System;
            cx.notify();
            window.refresh();
        })
        .unwrap();
        settle(cx);
    }

    #[gpui_kit::test]
    fn connects_with_a_token_and_clears_the_field(cx: &mut gpui_kit::TestAppContext) {
        let (w, rec) = window(cx);
        w.update(cx, |s, _, cx| {
            assert!(s.loaded);
            assert_eq!(s.accounts.len(), 4);
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

    #[gpui_kit::test]
    fn a_sign_in_missing_scopes_asks_for_a_new_one(cx: &mut gpui_kit::TestAppContext) {
        let (w, rec) = window(cx);
        w.update(cx, |s, _, cx| {
            s.gh_token.set_text("narrow", cx);
            s.connect("github", cx);
        })
        .unwrap();
        settle(cx);
        w.update(cx, |s, _, _| {
            let gh = s.account("github");
            assert_eq!(gh["connected"], true);
            assert_eq!(gh["reauth"], true);
            assert_eq!(gh["missing"], json!(["read:org", "workflow"]));
            assert!(super::needs_owner(&gh), "the sidebar counts it");
        })
        .unwrap();
        let st = rec.get("state", &[]).unwrap();
        assert_eq!(st["accounts"][0]["id"], "github", "the board's status bar hears about it");
        assert_eq!(st["accounts"][0]["reauth"], true);

        w.update(cx, |s, _, cx| {
            s.gh_token.set_text("ghp_full", cx);
            s.connect("github", cx);
        })
        .unwrap();
        settle(cx);
        w.update(cx, |s, _, _| assert_eq!(s.account("github")["reauth"], false)).unwrap();
        assert_eq!(rec.get("state", &[]).unwrap()["accounts"], json!([]));
    }

    #[gpui_kit::test]
    fn a_bitbucket_token_without_read_user_is_kept_and_says_what_is_off(cx: &mut gpui_kit::TestAppContext) {
        let (w, rec) = window(cx);
        w.update(cx, |s, _, cx| {
            s.bb_email.set_text("a@b.co", cx);
            s.bb_token.set_text("nouser", cx);
            s.connect("bitbucket", cx);
        })
        .unwrap();
        settle(cx);
        w.update(cx, |s, _, _| {
            let bb = s.account("bitbucket");
            assert_eq!((bb["connected"].clone(), bb["reauth"].clone()), (json!(true), json!(false)));
            assert!(bb["missing_scopes"].as_array().unwrap().contains(&json!("read:user:bitbucket")));
            assert!(bb["note"].as_str().unwrap().contains("add read:user:bitbucket to show your Bitbucket name"));
            assert!(!super::needs_owner(&bb), "nothing for the owner to fix");
        })
        .unwrap();
        assert_eq!(rec.get("state", &[]).unwrap()["accounts"], json!([]));
    }

    #[gpui_kit::test]
    fn an_azure_token_without_test_management_asks_for_a_new_one(cx: &mut gpui_kit::TestAppContext) {
        let (w, rec) = window(cx);
        w.update(cx, |s, _, cx| {
            s.az_org.set_text("dev.azure.com/acme", cx);
            s.az_token.set_text("narrow", cx);
            s.connect("azure", cx);
        })
        .unwrap();
        settle(cx);
        assert_eq!(rec.last("accounts/azure"), Some(json!({"organization": "dev.azure.com/acme", "token": "narrow"})));
        w.update(cx, |s, _, cx| {
            let az = s.account("azure");
            assert_eq!((az["connected"].clone(), az["reauth"].clone()), (json!(true), json!(true)));
            assert_eq!(az["missing"], json!([taskboardd::accounts::AZURE_TEST]));
            assert!(super::needs_owner(&az), "the sidebar counts it");
            assert_eq!(s.az_token.text(cx), "");
            s.az_token.set_text("azpat-full", cx);
            s.connect("azure", cx);
        })
        .unwrap();
        settle(cx);
        w.update(cx, |s, _, _| assert_eq!(s.account("azure")["reauth"], false)).unwrap();
    }

    #[gpui_kit::test]
    fn disconnect_asks_in_the_row_before_forgetting_the_sign_in(cx: &mut gpui_kit::TestAppContext) {
        let (w, rec) = window(cx);
        w.update(cx, |s, window, cx| {
            assert!(!s.slack_form, "no form until Connect…");
            s.open_form("slack", window, cx);
            assert!(s.slack_form);
            s.slack_token.set_text("xoxb-1", cx);
            s.connect("slack", cx);
        })
        .unwrap();
        settle(cx);
        w.update(cx, |s, window, cx| {
            assert_eq!(s.account("slack")["connected"], true);
            assert!(!s.slack_form, "the form closes once connected");
            s.ask_disconnect("slack", cx);
            window.refresh();
        })
        .unwrap();
        settle(cx);
        assert!(!rec.posts().iter().any(|(p, _)| p == "accounts/slack/disconnect"), "Disconnect only asks");
        w.update(cx, |s, _, cx| {
            assert_eq!(s.confirm, Some("slack"));
            assert_eq!(s.account("slack")["connected"], true);
            s.disconnect("slack", cx);
        })
        .unwrap();
        settle(cx);
        assert!(rec.posts().iter().any(|(p, _)| p == "accounts/slack/disconnect"));
        w.update(cx, |s, _, _| {
            assert_eq!(s.confirm, None);
            assert_eq!(s.account("slack")["connected"], false);
        })
        .unwrap();
    }

    #[::core::prelude::v1::test]
    fn each_account_says_its_status_in_one_line() {
        let line = |id, a| super::account_line(id, &a);
        assert_eq!(line("github", json!({"connected": false})), ("Not connected".into(), false));
        assert_eq!(line("github", json!({"connected": true, "user": "sam", "checked_at": crate::fmt::now().format("%Y-%m-%dT15:05:00Z").to_string()})), ("Signed in as sam · checked 3:05 PM, works".into(), false));
        assert_eq!(line("azure", json!({"connected": true, "user": "s@acme.com", "detail": "s@acme.com · dev.azure.com/acme"})), ("Signed in to acme".into(), false));
        let refused = json!({"connected": true, "label": "GitHub", "user": "sam", "error": "Bad credentials", "checked_at": crate::fmt::now().format("%Y-%m-%dT14:51:00Z").to_string()});
        assert_eq!(line("github", refused.clone()), ("GitHub refused the sign-in at 2:51 PM · agents can't push".into(), true));
        assert!(super::needs_owner(&refused), "the sidebar counts it");
        assert_eq!(line("slack", json!({"connected": true, "label": "Slack", "error": "token_expired"})).0, "Slack refused the sign-in · no Slack messages");
    }

    #[gpui_kit::test]
    fn history_retention_shows_the_values_the_board_keeps(cx: &mut gpui_kit::TestAppContext) {
        let (w, _rec) = window(cx);
        w.update(cx, |s, _, _| {
            let h = s.history.clone().expect("GET /history read on open");
            for key in ["detail_days", "summary_days"] {
                let (choices, on) = super::retention(&h, key);
                assert_eq!(choices.len(), 4, "{key}: {h}");
                assert!(on.is_some(), "{key} shows the board's value as chosen: {h}");
            }
        })
        .unwrap();
    }

    #[gpui_kit::test]
    fn a_retention_change_asks_the_board_what_it_removes_first(cx: &mut gpui_kit::TestAppContext) {
        let (w, rec) = window(cx);
        w.update(cx, |s, _, cx| s.preview_history("summary_days", 0, cx)).unwrap();
        settle(cx);
        let posts: Vec<_> = rec.posts().into_iter().filter(|(p, _)| p == "history").collect();
        assert_eq!(posts[0].1, json!({"summary_days": 0, "preview": true}), "previewed first");
        assert_eq!(posts.get(1).map(|p| p.1.clone()), Some(json!({"summary_days": 0})), "keeping more removes nothing, so it saves");
        w.update(cx, |s, _, _| assert_eq!(s.history.as_ref().unwrap()["summary_days"], 0)).unwrap();
    }

    #[::core::prelude::v1::test]
    fn the_removal_warning_counts_events_and_days() {
        assert_eq!(super::removal_line(&json!({"events_total": 0, "days": 0, "day_rows": 0})), None);
        assert_eq!(
            super::removal_line(&json!({"events_total": 9, "days": 2, "day_rows": 3})).as_deref(),
            Some("The next cleanup, after 3 AM, removes 9 events and the summaries of 2 days. They can't be brought back.")
        );
        assert_eq!(super::removal_line(&json!({"events_total": 1, "days": 0, "day_rows": 0})).as_deref(), Some("The next cleanup, after 3 AM, removes 1 event. They can't be brought back."));
    }

    #[gpui_kit::test]
    fn uses_gh_sign_in_without_changing_it(cx: &mut gpui_kit::TestAppContext) {
        let (w, rec) = window(cx);
        w.update(cx, |s, _, _| s.gh_form = true).unwrap();
        w.update(cx, |s, _, cx| s.import_gh(cx)).unwrap();
        settle(cx);
        assert!(rec.posts().iter().any(|(p, _)| p == "accounts/github/import"));
        w.update(cx, |s, _, _| {
            assert_eq!(s.account("github")["connected"], true);
            assert!(!s.gh_form, "the form closes once signed in");
            assert!(s.notes.get("github").is_some_and(|(n, bad)| !bad && n.starts_with("Using gh's sign-in")));
        })
        .unwrap();
    }
}

#[cfg(test)]
mod worktree_tests {
    use serde_json::{Value, json};

    fn pool() -> Value {
        let today = crate::fmt::now().format("%Y-%m-%dT14:30:00Z").to_string();
        let slot = |project: &str, n: i64, held: Value, title: &str, used: &str, kb: Value, waiting: Value| {
            json!({"name": format!("slot-{n}"), "n": n, "project": project, "repo": format!("/{project}"), "path": format!("/{project}/slot-{n}"),
                   "held_by": held, "title": title, "used_at": used, "size_kb": kb, "waiting": waiting})
        };
        json!({
            "slots": [
                slot("board", 1, json!("T12"), "Slot pool view", &today, json!(2_411_725), json!(["T13", "T14"])),
                slot("board", 2, Value::Null, "", "2026-10-07T15:05:00Z", json!(860_160), json!(["T13", "T14"])),
                slot("lens", 2, Value::Null, "", "", Value::Null, json!([])),
            ],
            "pools": {
                "board": {"min_slots": 2, "max_slots": 10, "idle_trim_days": 1, "idle_remove_days": 7, "low_disk_gb": 50},
                "lens": {"min_slots": 0, "max_slots": 3, "idle_trim_days": 2, "idle_remove_days": 7, "low_disk_gb": 50},
            },
        })
    }

    #[test]
    fn slots_group_by_project_with_their_pool_and_waiting_tasks() {
        let pools = super::pools_vm(&pool());
        assert_eq!(pools.iter().map(|p| (p.project.as_str(), p.count.as_str())).collect::<Vec<_>>(), [("board", "2 slots"), ("lens", "1 slot")]);
        let board = &pools[0];
        assert_eq!(board.settings, "Keeps 2 to 10 slots. Build output is cleared after 1 idle day, the slot removed after 7.");
        assert_eq!(board.waiting, ["T13", "T14"], "each waiting task once");
        assert_eq!(board.slots[0].note, "2.3 GB · last used 2:30 PM");
        assert_eq!((board.slots[0].held_by.as_deref(), board.slots[0].tip.as_deref()), (Some("T12"), Some("Slot pool view")));
        assert_eq!((board.slots[1].note.as_str(), board.slots[1].held_by.as_deref()), ("840 MB · last used Oct 7, 3:05 PM", None));
        let lens = &pools[1];
        assert_eq!((lens.slots[0].name.as_str(), lens.slots[0].note.as_str()), ("slot-2", ""), "not measured or used yet");
        assert!(lens.waiting.is_empty());
        assert!(super::pools_vm(&json!({"slots": []})).is_empty());
    }
}

#[cfg(test)]
mod reviewer_tests {
    use serde_json::json;

    #[test]
    fn a_reviewer_row_says_their_accounts_bot_pace_and_why_they_are_removed() {
        let r = json!({"name": "Ana", "user": "ana", "emails": ["ana@acme.com"], "aliases": [], "bot": {"every_h": 4.0}, "automation": 2.0,
                       "median_work_mins": 75.0, "open_asks": 1, "removed": true, "removed_why": "left the team"});
        assert_eq!(super::reviewer_note(&r), "ana, ana@acme.com · Bot every 4h · Automation 2 · Reviews in 1h 15m · 1 open · Removed: left the team");
    }
}
