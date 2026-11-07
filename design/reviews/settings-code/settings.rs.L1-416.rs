// Excerpt of crates/taskboard-app/src/settings.rs, lines 1-416
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
    Reviewers,
    Worktrees,
    System,
}

const SECS: [Sec; 7] = [Sec::Accounts, Sec::History, Sec::Usage, Sec::Qa, Sec::Reviewers, Sec::Worktrees, Sec::System];

impl Sec {
    fn label(self) -> &'static str {
        match self {
            Sec::Accounts => "Accounts",
            Sec::History => "History",
            Sec::Usage => "Usage",
            Sec::Qa => "QA",
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
            Sec::Reviewers => "Who reviews each project's PRs. Agents and tb reviewers change the roster.",
            Sec::Worktrees => "Each project's reusable worktree slots, kept warm between tasks. tb project set sizes a project's pool.",
            Sec::System => "Updates, and the versions of the app and the daemon.",
        }
    }

    fn glyph(self) -> &'static str {
        match self {
            Sec::Accounts => "@",
            Sec::History => "◷",
            Sec::Usage => "◔",
            Sec::Qa => "✓",
            Sec::Reviewers => "☺",
            Sec::Worktrees => "⑂",
            Sec::System => "⚙",
        }
    }
}

/// Each account's card: id, a short mark and its color, and what it's used for.
const CARDS: [(&str, &str, u32, &str); 4] = [
    ("github", "GH", 0x24292f, "PR checks and reviews, comments, reviewers and git push"),
    ("bitbucket", "BB", 0x2684ff, "Pull requests, comments, reviewers and git push over HTTPS"),
    ("azure", "AZ", 0x0078d4, "Failed Azure Pipelines steps and tests on PR checks"),
    ("slack", "S", 0x4a154b, "Posting and reading messages for the board and its agents"),
];

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
    /// `GET /usage`'s `daily` block, a call in flight, and the line under its card.
    daily: Option<Value>,
    daily_busy: bool,
    daily_note: Option<(String, bool)>,
    /// `GET /qa`, a call in flight, and the line under its card.
    qa: Option<Value>,
    qa_busy: bool,
    qa_note: Option<(String, bool)>,
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
            daily: None,
            daily_busy: false,
            daily_note: None,
            qa: None,
            qa_busy: false,
            qa_note: None,
            reviewers: None,
            slots: None,
            info: None,
            sys_busy: false,
            sys_note: None,
        };
        s.load(false, cx);
        s.load_history(cx);
        s.load_daily(cx);
        s.load_qa(cx);
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
                // the updater runs on its own thread: redraw System as its state moves
                let _ = this.update(cx, |s, cx| {
                    if s.view == Sec::Worktrees && tick % IDLE_TICKS == 0 {
                        s.load_slots(cx);
                    }
                    if s.view == Sec::System {
                        if tick % IDLE_TICKS == 0 {
                            s.load_info(cx);
                        }
                        cx.notify();
                    }
                });
            }
        })
        .detach();
        s
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
            let r = cx.background_executor().spawn(async move { backend.get("info", &[]) }).await;
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

    /// `POST history` (a setting) or `history/cleanup`; both answer with the History state.
    fn post_history(&mut self, path: &'static str, body: Value, cx: &mut Context<Self>) {
        if self.history_busy {
            return;
        }
        self.history_busy = true;
        self.history_note = None;
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

