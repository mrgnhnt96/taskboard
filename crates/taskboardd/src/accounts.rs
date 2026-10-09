//! Accounts: GitHub, Bitbucket and Slack, so the board and its agents can check PRs, comment, push and
//! assign reviewers.
//!
//! Taskboard keeps its own sign-in for each and never touches the machine's: not `gh`'s login, not
//! `~/.gitconfig`, not git's credential store.
//!
//! - **GitHub** is a token: pasted, copied once from `gh auth token` ("Use my gh sign-in"), or made by
//!   a browser sign-in that runs `gh auth login --web --insecure-storage` in a throwaway
//!   `GH_CONFIG_DIR`, so gh's own login and Keychain item are left alone.
//! - **Bitbucket** is an Atlassian API token with Bitbucket scopes (email + token).
//! - **Slack** is an app's OAuth token (`xoxp-` or `xoxb-`), checked with `auth.test`.
//!
//! The tokens live in the login Keychain (`taskboard-github`, `taskboard-bitbucket`, `taskboard-slack`);
//! `accounts.json` in the data folder keeps who each account is and its scopes, never a secret. The API
//! never returns a secret: `tb token <provider>` reads it from the Keychain itself, and agents on a task
//! `git push` with it through `tb git-credential` (see `tb hook SessionStart`).
//!
//! [`required_scopes`] is what Taskboard needs from each account. A connected account whose checked
//! scopes miss one answers `reauth: true` with the `missing` scopes, and the app asks the owner to sign
//! in again; raising the list flags every older sign-in.
//!
//! A board on a test config (`cfg.accounts_sandbox`) keeps everything in memory, accepts any token but
//! `bad` (and `narrow`, which lacks scopes), and signs GitHub in a few seconds after "Sign in", so the
//! sample board and the tests never touch the Keychain, `gh`, git or the network.

use std::collections::HashMap;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::Arc;
use std::time::{Duration, Instant};

use parking_lot::Mutex;
use serde_json::{json, Map, Value};

use crate::app::App;
use crate::config::Config;
use crate::jira::base64_lite::encode as b64;
use crate::proc;
use crate::util::*;

const HTTP_TIMEOUT: Duration = Duration::from_secs(15);
const GH_TIMEOUT: f64 = 20.0;
/// How long a browser sign-in may wait for the code to be entered.
const LOGIN_TIMEOUT: Duration = Duration::from_secs(15 * 60);
const SANDBOX_LOGIN_SECS: u64 = 4;

pub const BITBUCKET_GIT_USER: &str = "x-bitbucket-api-token-auth";
/// GitHub takes any user name with a token over HTTPS; this is the one its docs use.
pub const GITHUB_GIT_USER: &str = "x-access-token";
pub const BITBUCKET_TOKEN_URL: &str = "https://id.atlassian.com/manage-profile/security/api-tokens";
pub const SLACK_APPS_URL: &str = "https://api.slack.com/apps";
pub const GITHUB_DEVICE_URL: &str = "https://github.com/login/device";

#[derive(Clone, Copy, PartialEq, Eq, Debug, Hash)]
pub enum Provider {
    Github,
    Bitbucket,
    Slack,
}

pub const PROVIDERS: [Provider; 3] = [Provider::Github, Provider::Bitbucket, Provider::Slack];

impl Provider {
    pub fn parse(s: &str) -> Option<Provider> {
        PROVIDERS.into_iter().find(|p| p.id() == s.trim().to_lowercase())
    }

    pub fn id(self) -> &'static str {
        match self {
            Provider::Github => "github",
            Provider::Bitbucket => "bitbucket",
            Provider::Slack => "slack",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Provider::Github => "GitHub",
            Provider::Bitbucket => "Bitbucket",
            Provider::Slack => "Slack",
        }
    }

    /// The Keychain item holding its token.
    fn service(self) -> &'static str {
        match self {
            Provider::Github => "taskboard-github",
            Provider::Bitbucket => "taskboard-bitbucket",
            Provider::Slack => "taskboard-slack",
        }
    }

    /// The git HTTPS host its token pushes to, and the user name git sends with it.
    fn git(self) -> Option<(&'static str, &'static str)> {
        match self {
            Provider::Github => Some(("github.com", GITHUB_GIT_USER)),
            Provider::Bitbucket => Some(("bitbucket.org", BITBUCKET_GIT_USER)),
            Provider::Slack => None,
        }
    }
}

/// The scopes Taskboard needs from each account. Add one here when a feature needs it: every account
/// signed in without it then shows "Sign in again".
pub fn required_scopes(p: Provider) -> &'static [&'static str] {
    match p {
        Provider::Github => &["repo", "read:org", "workflow"],
        Provider::Bitbucket => &["read:user:bitbucket", "read:repository:bitbucket", "write:repository:bitbucket", "read:pullrequest:bitbucket", "write:pullrequest:bitbucket"],
        Provider::Slack => &[],
    }
}

/// The required scopes `scopes` lacks. Empty when the service didn't list any (a fine-grained GitHub
/// token, or a Bitbucket answer in another vocabulary): those can't be judged. `write:x` and
/// `admin:x` cover `read:x`.
pub fn missing_scopes(p: Provider, scopes: &[String]) -> Vec<String> {
    let judged = match p {
        Provider::Bitbucket => scopes.iter().any(|s| s.ends_with(":bitbucket")),
        _ => !scopes.is_empty(),
    };
    if !judged {
        return vec![];
    }
    let has = |s: &str| scopes.iter().any(|x| x == s);
    required_scopes(p)
        .iter()
        .filter(|need| !(has(need) || need.strip_prefix("read:").is_some_and(|rest| has(&format!("write:{rest}")) || has(&format!("admin:{rest}")))))
        .map(|s| s.to_string())
        .collect()
}

fn provider(id: &str) -> Result<Provider> {
    Provider::parse(id).ok_or_else(|| ApiError::new(404, "There's no such account. Use github, bitbucket or slack."))
}

/// A token and who it belongs to: (email or user, secret).
pub type Cred = (String, String);

/// What the board knows about the accounts beyond `accounts.json`: GitHub's browser sign-in, and the
/// sandbox's Keychain.
pub struct Accounts {
    sandbox: bool,
    gh: Arc<Mutex<Gh>>,
    mem: Mutex<HashMap<Provider, Cred>>,
}

#[derive(Default)]
struct Gh {
    login: Option<Login>,
    /// Why the last sign-in failed, until the next one starts.
    error: Option<String>,
}

struct Login {
    code: String,
    url: String,
    started: Instant,
    cancel: bool,
}

impl Accounts {
    pub fn new(cfg: &Config) -> Accounts {
        Accounts { sandbox: cfg.accounts_sandbox, gh: Arc::new(Mutex::new(Gh::default())), mem: Mutex::new(HashMap::new()) }
    }

    fn get(&self, p: Provider) -> Option<Cred> {
        if self.sandbox {
            return self.mem.lock().get(&p).cloned();
        }
        keychain_get(p.service())
    }

    fn set(&self, p: Provider, cred: Cred) -> std::result::Result<(), String> {
        if self.sandbox {
            self.mem.lock().insert(p, cred);
            return Ok(());
        }
        keychain_set(p.service(), &cred.0, &cred.1)
    }

    fn remove(&self, p: Provider) {
        if self.sandbox {
            self.mem.lock().remove(&p);
        } else {
            keychain_delete(p.service());
        }
    }
}

// ------------------------------------------------------------------ who (accounts.json)

fn who_path(cfg: &Config) -> PathBuf {
    cfg.data.join("accounts.json")
}

fn load_who(cfg: &Config) -> Map<String, Value> {
    std::fs::read_to_string(who_path(cfg)).ok().and_then(|t| serde_json::from_str::<Value>(&t).ok()).and_then(|v| v.as_object().cloned()).unwrap_or_default()
}

fn save_who(cfg: &Config, p: Provider, who: Option<Value>) {
    let mut all = load_who(cfg);
    match who {
        Some(w) => all.insert(p.id().into(), w),
        None => all.remove(p.id()),
    };
    let _ = std::fs::write(who_path(cfg), serde_json::to_string_pretty(&Value::Object(all)).unwrap_or_default());
}

fn who(user: &str, name: &str, detail: &str, scopes: Vec<String>) -> Value {
    json!({"user": user, "name": name, "detail": detail, "scopes": scopes, "checked_at": now_iso(), "error": null})
}

fn scopes_of(w: &Value) -> Vec<String> {
    w["scopes"].as_array().map(|s| s.iter().filter_map(|x| x.as_str().map(|x| x.to_string())).collect()).unwrap_or_default()
}

// ------------------------------------------------------------------ the API

/// `GET accounts`: every account's state. `fresh` asks the services again instead of using the last
/// answer.
pub fn status(app: &App, fresh: bool) -> Result<Value> {
    finish_sandbox_login(app);
    if fresh {
        for p in PROVIDERS {
            recheck(app, p);
        }
    }
    let saved = load_who(&app.cfg);
    let list: Vec<Value> = PROVIDERS.into_iter().map(|p| account(app, p, &saved)).collect();
    Ok(json!({"accounts": list}))
}

fn account(app: &App, p: Provider, saved: &Map<String, Value>) -> Value {
    let w = saved.get(p.id()).cloned().unwrap_or(Value::Null);
    let connected = !w.is_null() && app.accounts.get(p).is_some();
    let mut a = base(p, connected);
    if connected {
        for k in ["user", "name", "detail", "scopes", "checked_at", "error"] {
            a[k] = w.get(k).cloned().unwrap_or(Value::Null);
        }
        let missing = missing_scopes(p, &scopes_of(&w));
        a["reauth"] = json!(!missing.is_empty());
        a["missing"] = json!(missing);
    }
    if p == Provider::Github {
        let g = app.accounts.gh.lock();
        a["login"] = g.login.as_ref().map(|l| json!({"code": l.code, "url": l.url})).unwrap_or(Value::Null);
        if a["error"].is_null() {
            a["error"] = json!(g.error);
        }
        drop(g);
        // gh is only needed for the browser sign-in and "Use my gh sign-in"; a token works without it.
        if !app.accounts.sandbox && gh_path(app).is_none() {
            a["gh"] = json!(false);
            a["setup"] = json!("brew install gh");
        }
    }
    a
}

fn base(p: Provider, connected: bool) -> Value {
    json!({"id": p.id(), "label": p.label(), "connected": connected, "user": null, "name": null, "detail": null,
           "scopes": [], "required": required_scopes(p), "missing": [], "reauth": false, "checked_at": null, "error": null,
           "login": null, "gh": true, "setup": null})
}

/// Accounts that need the owner, for the board's status bar: `[{id, label, reason}]`. Reads only
/// `accounts.json`, so it's cheap enough for every `/state`.
pub fn attention(cfg: &Config) -> Value {
    let saved = load_who(cfg);
    let mut out = vec![];
    for p in PROVIDERS {
        let Some(w) = saved.get(p.id()) else { continue };
        let missing = missing_scopes(p, &scopes_of(w));
        let reason = if !missing.is_empty() {
            format!("Sign in again: Taskboard now needs {}.", missing.join(", "))
        } else if let Some(e) = w["error"].as_str() {
            e.to_string()
        } else {
            continue;
        };
        out.push(json!({"id": p.id(), "label": p.label(), "reason": reason, "reauth": !missing.is_empty()}));
    }
    json!(out)
}

/// `POST accounts/<id>`: connect with a token (`{"token"}`, Bitbucket also `{"email"}`).
pub fn connect(app: &App, id: &str, body: &Value) -> Result<Value> {
    let p = provider(id)?;
    let token = body_str(body, "token");
    if token.is_empty() {
        return err(400, "Paste a token first.");
    }
    let user = match p {
        Provider::Github => String::new(),
        Provider::Bitbucket => {
            let email = body_str(body, "email");
            if !email.contains('@') {
                return err(400, "Enter the email of your Atlassian account.");
            }
            email
        }
        Provider::Slack => {
            if !token.starts_with("xox") {
                return err(400, "That isn't a Slack token. It starts with xoxp- (a user token) or xoxb- (a bot token).");
            }
            String::new()
        }
    };
    keep(app, p, user, token).map_err(|e| ApiError::new(400, e))?;
    status(app, false)
}

/// Checks a token with its service, then keeps it: the Keychain gets the secret, `accounts.json` who
/// it is. GitHub and Slack are kept under the user the service names.
fn keep(app: &App, p: Provider, user: String, token: String) -> std::result::Result<(), String> {
    let w = verify(app, p, &(user.clone(), token.clone()))?;
    let user = if user.is_empty() { w["user"].as_str().unwrap_or("").to_string() } else { user };
    app.accounts.set(p, (user, token))?;
    save_who(&app.cfg, p, Some(w));
    app.info(format!("accounts: {} connected", p.label()));
    Ok(())
}

/// `POST accounts/<id>/check`: ask the service again whether the token still works.
pub fn check(app: &App, id: &str) -> Result<Value> {
    let p = provider(id)?;
    if app.accounts.get(p).is_none() {
        return err(400, format!("{} isn't connected.", p.label()));
    }
    recheck(app, p);
    status(app, false)
}

/// Asks the service about a connected account's token and saves the answer (or why it failed).
fn recheck(app: &App, p: Provider) {
    let Some(cred) = app.accounts.get(p) else { return };
    let mut saved = load_who(&app.cfg).get(p.id()).cloned().unwrap_or_else(|| json!({}));
    match verify(app, p, &cred) {
        Ok(w) => save_who(&app.cfg, p, Some(w)),
        Err(e) => {
            saved["error"] = json!(e);
            saved["checked_at"] = json!(now_iso());
            save_who(&app.cfg, p, Some(saved));
        }
    }
}

/// `POST accounts/<id>/disconnect`: forget Taskboard's token. Nothing else on the Mac changes.
pub fn disconnect(app: &App, id: &str) -> Result<Value> {
    let p = provider(id)?;
    app.accounts.remove(p);
    save_who(&app.cfg, p, None);
    app.info(format!("accounts: {} disconnected", p.label()));
    status(app, false)
}

// ------------------------------------------------------------------ verifying tokens

fn verify(app: &App, p: Provider, cred: &Cred) -> std::result::Result<Value, String> {
    if app.accounts.sandbox {
        return sandbox_who(p, &cred.1);
    }
    ask_service(p, cred)
}

/// Asks the service who a token belongs to and what it may do.
fn ask_service(p: Provider, cred: &Cred) -> std::result::Result<Value, String> {
    let agent = ureq::AgentBuilder::new().timeout(HTTP_TIMEOUT).build();
    let scopes = |r: &ureq::Response| -> Vec<String> {
        r.header("x-oauth-scopes").map(|s| s.split(',').map(|x| x.trim().to_string()).filter(|x| !x.is_empty()).collect()).unwrap_or_default()
    };
    match p {
        Provider::Github => match agent.get("https://api.github.com/user").set("Authorization", &format!("Bearer {}", cred.1)).set("Accept", "application/vnd.github+json").call() {
            Ok(r) => {
                let sc = scopes(&r);
                let v: Value = r.into_json().unwrap_or(Value::Null);
                let user = v["login"].as_str().unwrap_or("").to_string();
                let name = v["name"].as_str().filter(|n| !n.is_empty()).unwrap_or(&user).to_string();
                Ok(who(&user, &name, &format!("@{user} · github.com"), sc))
            }
            Err(ureq::Error::Status(401, _)) => Err("GitHub didn't accept that token.".into()),
            Err(ureq::Error::Status(code, _)) => Err(format!("GitHub answered {code}.")),
            Err(_) => Err("Can't reach GitHub.".into()),
        },
        Provider::Bitbucket => {
            let auth = format!("Basic {}", b64(format!("{}:{}", cred.0, cred.1).as_bytes()));
            match agent.get("https://api.bitbucket.org/2.0/user").set("Authorization", &auth).set("Accept", "application/json").call() {
                Ok(r) => {
                    let sc = scopes(&r);
                    let v: Value = r.into_json().unwrap_or(Value::Null);
                    let user = v["username"].as_str().or(v["nickname"].as_str()).unwrap_or("").to_string();
                    let name = v["display_name"].as_str().unwrap_or(&user).to_string();
                    Ok(who(&user, &name, &format!("@{user} · {}", cred.0), sc))
                }
                Err(ureq::Error::Status(401, _)) => {
                    Err("Bitbucket didn't accept that email and token. Use an API token with Bitbucket scopes (not an app password), made by the same account.".into())
                }
                Err(ureq::Error::Status(403, _)) => Err("Bitbucket accepted the token but it can't read your account. Add the read:user:bitbucket scope.".into()),
                Err(ureq::Error::Status(code, _)) => Err(format!("Bitbucket answered {code}.")),
                Err(_) => Err("Can't reach Bitbucket.".into()),
            }
        }
        Provider::Slack => match agent.post("https://slack.com/api/auth.test").set("Authorization", &format!("Bearer {}", cred.1)).call() {
            Ok(r) => {
                let sc = scopes(&r);
                let v: Value = r.into_json().unwrap_or(Value::Null);
                if v["ok"].as_bool() != Some(true) {
                    let why = v["error"].as_str().unwrap_or("unknown");
                    return Err(match why {
                        "invalid_auth" | "not_authed" | "token_revoked" | "account_inactive" => "Slack didn't accept that token.".to_string(),
                        "token_expired" => "That Slack token has expired.".to_string(),
                        other => format!("Slack said {other}."),
                    });
                }
                let team = v["team"].as_str().unwrap_or("");
                let host = v["url"].as_str().unwrap_or("").trim_start_matches("https://").trim_end_matches('/');
                let kind = if cred.1.starts_with("xoxb-") { "bot" } else { "user" };
                let detail = [team, host].iter().filter(|s| !s.is_empty()).cloned().collect::<Vec<_>>().join(" · ");
                Ok(who(v["user"].as_str().unwrap_or(""), &format!("{} ({kind})", v["user"].as_str().unwrap_or("")), &detail, sc))
            }
            Err(_) => Err("Can't reach Slack.".into()),
        },
    }
}

/// The sandbox's answer for a token: `bad` is refused, `narrow` lacks scopes Taskboard needs.
fn sandbox_who(p: Provider, token: &str) -> std::result::Result<Value, String> {
    if token == "bad" {
        return Err(format!("{} didn't accept that token.", p.label()));
    }
    let mut scopes: Vec<String> = required_scopes(p).iter().map(|s| s.to_string()).collect();
    if token == "narrow" {
        scopes.truncate(1);
    }
    Ok(match p {
        Provider::Github => who("sample-user", "sample-user", "@sample-user · github.com", scopes),
        Provider::Bitbucket => who("sample", "Sample User", "@sample", scopes),
        Provider::Slack => who("sample", "sample", "Sample Team · sample.slack.com", vec!["chat:write".into()]),
    })
}

// ------------------------------------------------------------------ GitHub (gh, never its own login)

fn gh_path(app: &App) -> Option<PathBuf> {
    proc::which(&app.cfg.pr.gh)
}

fn last_line(s: &str) -> String {
    s.lines().map(str::trim).filter(|l| !l.is_empty()).last().unwrap_or("").to_string()
}

/// The environment for a `gh` that must not see or change the Mac's own sign-in: its config in
/// `dir`, git's global config there too, and no token from the environment.
fn isolated_gh(cmd: &mut Command, dir: &Path) {
    cmd.env("GH_CONFIG_DIR", dir).env("GIT_CONFIG_GLOBAL", dir.join("gitconfig")).env("GH_PROMPT_DISABLED", "1");
    for k in ["GH_TOKEN", "GITHUB_TOKEN", "GH_ENTERPRISE_TOKEN", "GITHUB_ENTERPRISE_TOKEN"] {
        cmd.env_remove(k);
    }
}

/// `POST accounts/github/import`: copies the token `gh` already uses (`gh auth token`, read only) into
/// Taskboard's Keychain item. gh's own sign-in isn't changed, and later changes to it don't follow.
pub fn github_import(app: &App) -> Result<Value> {
    let token = if app.accounts.sandbox {
        "gho_sample".to_string()
    } else {
        let Some(bin) = gh_path(app) else { return err(400, "Install the GitHub CLI first: brew install gh") };
        let o = proc::run(&bin, &["auth".into(), "token".into(), "--hostname".into(), "github.com".into()], None, GH_TIMEOUT)
            .map_err(|_| ApiError::new(500, "gh didn't answer."))?;
        let t = o.stdout.trim().to_string();
        if o.code != Some(0) || t.is_empty() {
            return err(400, "gh isn't signed in to github.com on this Mac. Sign in with GitHub here instead.");
        }
        t
    };
    keep(app, Provider::Github, String::new(), token).map_err(|e| ApiError::new(400, e))?;
    status(app, false)
}

/// `POST accounts/github/login`: starts `gh auth login --web` in a throwaway config folder and answers
/// once gh has shown its one-time code (the app shows it and opens github.com/login/device). gh waits
/// for the code to be entered; a thread watches it finish, takes the token into Taskboard's Keychain
/// item and deletes the folder. It asks for [`required_scopes`].
pub fn github_login(app: &App) -> Result<Value> {
    {
        let mut g = app.accounts.gh.lock();
        if g.login.is_some() {
            drop(g);
            return status(app, false);
        }
        g.error = None;
        if app.accounts.sandbox {
            g.login = Some(Login { code: "SAMP-1234".into(), url: GITHUB_DEVICE_URL.into(), started: Instant::now(), cancel: false });
            drop(g);
            return status(app, false);
        }
    }
    let Some(bin) = gh_path(app) else { return err(400, "Install the GitHub CLI first: brew install gh") };
    let dir = app.cfg.data.join("gh-login");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).map_err(|e| ApiError::new(500, format!("Couldn't make a folder for the sign-in: {e}")))?;
    let scopes = required_scopes(Provider::Github).join(",");
    let mut cmd = Command::new(&bin);
    cmd.args(["auth", "login", "--web", "--hostname", "github.com", "--insecure-storage", "--scopes", &scopes]);
    isolated_gh(&mut cmd, &dir);
    let mut child = cmd.stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::piped()).spawn().map_err(|e| ApiError::new(500, format!("Couldn't run gh: {e}")))?;
    let mut stderr = child.stderr.take().unwrap();
    let gh_state = app.accounts.gh.clone();
    gh_state.lock().login = Some(Login { code: String::new(), url: GITHUB_DEVICE_URL.into(), started: Instant::now(), cancel: false });
    // gh's stderr: the code, the URL, then why it failed (if it does).
    let said = Arc::new(Mutex::new(String::new()));
    {
        let (said, gh_state) = (said.clone(), gh_state.clone());
        std::thread::spawn(move || {
            let mut buf = [0u8; 1024];
            while let Ok(n) = stderr.read(&mut buf) {
                if n == 0 {
                    break;
                }
                let text = {
                    let mut s = said.lock();
                    s.push_str(&String::from_utf8_lossy(&buf[..n]));
                    s.clone()
                };
                if let Some((code, url)) = device_prompt(&text) {
                    if let Some(l) = gh_state.lock().login.as_mut() {
                        l.code = code;
                        l.url = url;
                    }
                }
            }
        });
    }
    let (said, gh_state2) = (said.clone(), gh_state.clone());
    let cfg = app.cfg.clone();
    std::thread::spawn(move || {
        let started = Instant::now();
        let status = loop {
            match child.try_wait() {
                Ok(Some(s)) => break Some(s.success()),
                Ok(None) => {}
                Err(_) => break None,
            }
            let cancel = gh_state2.lock().login.as_ref().is_none_or(|l| l.cancel);
            if cancel || started.elapsed() > LOGIN_TIMEOUT {
                let _ = child.kill();
                let _ = child.wait();
                break if cancel { None } else { Some(false) };
            }
            std::thread::sleep(Duration::from_millis(200));
        };
        let error = match status {
            Some(true) => take_login_token(&cfg, &bin, &dir).err(),
            Some(false) if started.elapsed() > LOGIN_TIMEOUT => Some("The sign-in code expired. Try again.".into()),
            Some(false) => Some(format!("gh couldn't sign in: {}", last_line(&said.lock()))),
            None => None,
        };
        let _ = std::fs::remove_dir_all(&dir);
        let line = format!("{} accounts: GitHub browser sign-in {}\n", chrono::Local::now().format("%Y-%m-%d %H:%M:%S"), match (status, &error) {
            (Some(true), None) => "finished",
            (None, _) => "cancelled",
            _ => "failed",
        });
        let mut g = gh_state2.lock();
        g.login = None;
        g.error = error;
        drop(g);
        if let Ok(mut f) = std::fs::OpenOptions::new().append(true).open(cfg.log_path()) {
            let _ = f.write_all(line.as_bytes());
        }
    });
    // Answer once the code is known (gh prints it within a second or two).
    let until = Instant::now() + Duration::from_secs(10);
    while Instant::now() < until {
        let shown = gh_state.lock().login.as_ref().is_none_or(|l| !l.code.is_empty());
        if shown {
            break;
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    status(app, false)
}

/// After a browser sign-in: the token gh wrote into the throwaway folder, checked with GitHub and kept
/// as Taskboard's.
fn take_login_token(cfg: &Config, bin: &Path, dir: &Path) -> std::result::Result<(), String> {
    let mut cmd = Command::new(bin);
    cmd.args(["auth", "token", "--hostname", "github.com"]);
    isolated_gh(&mut cmd, dir);
    let o = cmd.stdin(Stdio::null()).output().map_err(|e| format!("Couldn't run gh: {e}"))?;
    let token = String::from_utf8_lossy(&o.stdout).trim().to_string();
    if !o.status.success() || token.is_empty() {
        return Err("gh signed in but didn't hand over a token. Try again.".into());
    }
    let probe = ask_service(Provider::Github, &(String::new(), token.clone()))?;
    keychain_set(Provider::Github.service(), probe["user"].as_str().unwrap_or(""), &token)?;
    save_who(cfg, Provider::Github, Some(probe));
    Ok(())
}

/// The sandbox's browser sign-in finishes a few seconds after the code shows.
fn finish_sandbox_login(app: &App) {
    if !app.accounts.sandbox {
        return;
    }
    let mut g = app.accounts.gh.lock();
    if g.login.as_ref().is_some_and(|l| !l.code.is_empty() && l.started.elapsed() >= Duration::from_secs(SANDBOX_LOGIN_SECS)) {
        g.login = None;
        drop(g);
        let _ = keep(app, Provider::Github, String::new(), "gho_sample".into());
    }
}

/// `POST accounts/github/cancel`: stops a browser sign-in.
pub fn github_cancel(app: &App) -> Result<Value> {
    let mut g = app.accounts.gh.lock();
    if app.accounts.sandbox {
        g.login = None;
    } else if let Some(l) = g.login.as_mut() {
        l.cancel = true;
    }
    drop(g);
    status(app, false)
}

/// gh's "First copy your one-time code: ABCD-1234" and "…in your web browser: <url>".
fn device_prompt(text: &str) -> Option<(String, String)> {
    let code = regex::Regex::new(r"one-time code:\s*([A-Z0-9]{4}-[A-Z0-9]{4})").unwrap().captures(text)?[1].to_string();
    let url = regex::Regex::new(r"(https://\S+/login/device)").unwrap().captures(text).map(|c| c[1].to_string()).unwrap_or_else(|| GITHUB_DEVICE_URL.into());
    Some((code, url))
}

// ------------------------------------------------------------------ tokens for agents (tb)

/// The token for `tb token` / `tb api`: (user or email, secret), from Taskboard's Keychain item.
pub fn credentials(_cfg: &Config, p: Provider) -> Option<Cred> {
    keychain_get(p.service())
}

/// The account the board itself calls a service with (the PR hosts in `prhost.rs`): (user or email,
/// secret), when that account is connected. On a sandbox board, the in-memory one.
pub fn board_credentials(app: &App, p: Provider) -> Option<Cred> {
    if !load_who(&app.cfg).contains_key(p.id()) {
        return None;
    }
    app.accounts.get(p)
}

/// What `tb git-credential` hands git for an HTTPS host: (user name, token), when Taskboard has that
/// host's account.
pub fn git_credentials(cfg: &Config, host: &str) -> Option<(String, String)> {
    let p = PROVIDERS.into_iter().find(|p| p.git().is_some_and(|(h, _)| h == host))?;
    let (_, user) = p.git()?;
    credentials(cfg, p).map(|(_, secret)| (user.to_string(), secret))
}

/// Whether Taskboard has an account git can push with (GitHub or Bitbucket).
pub fn has_git_account(cfg: &Config) -> bool {
    let saved = load_who(cfg);
    PROVIDERS.into_iter().any(|p| p.git().is_some() && saved.contains_key(p.id()))
}

/// `GH_TOKEN` for a `gh` the board runs itself, when Taskboard has a GitHub account (otherwise gh uses
/// the Mac's own sign-in, read only).
pub fn gh_env(cfg: &Config) -> Vec<(String, String)> {
    if cfg.accounts_sandbox || !load_who(cfg).contains_key(Provider::Github.id()) {
        return vec![];
    }
    credentials(cfg, Provider::Github).map(|(_, t)| vec![("GH_TOKEN".to_string(), t)]).unwrap_or_default()
}

// ------------------------------------------------------------------ Keychain

fn keychain_get(service: &str) -> Option<Cred> {
    let o = Command::new("security").args(["find-generic-password", "-s", service, "-w"]).output().ok()?;
    if !o.status.success() {
        return None;
    }
    let secret = String::from_utf8_lossy(&o.stdout).trim().to_string();
    let attrs = Command::new("security").args(["find-generic-password", "-s", service]).output().ok()?;
    let text = String::from_utf8_lossy(&attrs.stdout);
    let account = regex::Regex::new(r#""acct"<blob>="([^"]*)""#).unwrap().captures(&text).map(|c| c[1].to_string()).unwrap_or_default();
    (!secret.is_empty()).then_some((account, secret))
}

/// Saves through `security -i` so the token never shows in a process list.
fn keychain_set(service: &str, account: &str, secret: &str) -> std::result::Result<(), String> {
    let bad = |s: &str| s.chars().any(|c| c == '"' || c == '\\' || c.is_control());
    if bad(account) || bad(secret) {
        return Err("That token has characters the Keychain can't be given this way.".into());
    }
    keychain_delete(service);
    let mut child = Command::new("security")
        .arg("-i")
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| format!("Couldn't run security: {e}"))?;
    if let Some(mut i) = child.stdin.take() {
        let _ = i.write_all(format!("add-generic-password -s \"{service}\" -a \"{account}\" -l \"{service}\" -w \"{secret}\"\n").as_bytes());
    }
    let out = child.wait_with_output().map_err(|e| format!("Couldn't run security: {e}"))?;
    if keychain_get(service).is_none_or(|(_, s)| s != secret) {
        return Err(format!("Couldn't save the token in the Keychain: {}", last_line(&String::from_utf8_lossy(&out.stderr))));
    }
    Ok(())
}

fn keychain_delete(service: &str) {
    while Command::new("security").args(["delete-generic-password", "-s", service]).output().is_ok_and(|o| o.status.success()) {}
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Config;

    fn board() -> (Arc<App>, tempfile::TempDir) {
        let dir = tempfile::tempdir().unwrap();
        (App::for_tests(Config::for_tests(dir.path())), dir)
    }

    fn account(v: &Value, id: &str) -> Value {
        v["accounts"].as_array().unwrap().iter().find(|a| a["id"] == id).cloned().unwrap()
    }

    #[test]
    fn reads_gh_device_prompt() {
        let text = "\n! First copy your one-time code: 1445-0A8C\nOpen this URL to continue in your web browser: https://github.com/login/device\n";
        assert_eq!(device_prompt(text), Some(("1445-0A8C".into(), "https://github.com/login/device".into())));
        assert_eq!(device_prompt("! First copy your one-time"), None);
    }

    #[test]
    fn connects_and_disconnects_without_saving_secrets_on_disk() {
        let (app, dir) = board();
        let v = status(&app, false).unwrap();
        assert_eq!(v["accounts"].as_array().unwrap().len(), 3);
        assert!(PROVIDERS.iter().all(|p| account(&v, p.id())["connected"] == false));

        let e = connect(&app, "bitbucket", &json!({"email": "a@b.co", "token": "bad"})).unwrap_err();
        assert_eq!(e.status, 400);
        assert!(connect(&app, "bitbucket", &json!({"token": "x"})).is_err(), "needs an email");
        let v = connect(&app, "bitbucket", &json!({"email": "a@b.co", "token": "ATATT-secret"})).unwrap();
        let bb = account(&v, "bitbucket");
        assert_eq!(bb["connected"], true);
        assert_eq!(bb["user"], "sample");
        assert!(!v.to_string().contains("ATATT-secret"), "the API never answers with a token");
        assert!(!std::fs::read_to_string(dir.path().join("accounts.json")).unwrap().contains("ATATT-secret"));

        assert!(connect(&app, "slack", &json!({"token": "nope"})).is_err(), "not a slack token");
        let v = connect(&app, "slack", &json!({"token": "xoxp-1"})).unwrap();
        assert_eq!(account(&v, "slack")["connected"], true);

        let v = disconnect(&app, "bitbucket").unwrap();
        assert_eq!(account(&v, "bitbucket")["connected"], false);
        assert_eq!(account(&v, "slack")["connected"], true);
        assert!(connect(&app, "jira", &json!({"token": "x"})).is_err_and(|e| e.status == 404));
    }

    #[test]
    fn github_signs_in_with_the_browser_a_token_or_gh() {
        let (app, _dir) = board();
        let v = github_login(&app).unwrap();
        let gh = account(&v, "github");
        assert_eq!(gh["login"]["code"], "SAMP-1234");
        assert_eq!(gh["connected"], false);
        let v = github_cancel(&app).unwrap();
        assert!(account(&v, "github")["login"].is_null());

        assert!(connect(&app, "github", &json!({"token": "bad"})).is_err());
        let v = connect(&app, "github", &json!({"token": "ghp_x"})).unwrap();
        let gh = account(&v, "github");
        assert_eq!(gh["connected"], true);
        assert_eq!(gh["user"], "sample-user");
        assert_eq!(gh["reauth"], false);
        let v = disconnect(&app, "github").unwrap();
        assert_eq!(account(&v, "github")["connected"], false);

        let v = github_import(&app).unwrap();
        assert_eq!(account(&v, "github")["connected"], true, "gh's token is copied, not gh's login changed");
    }

    #[test]
    fn missing_scopes_ask_for_a_new_sign_in() {
        let (app, dir) = board();
        let v = connect(&app, "github", &json!({"token": "narrow"})).unwrap();
        let gh = account(&v, "github");
        assert_eq!(gh["connected"], true);
        assert_eq!(gh["reauth"], true);
        assert_eq!(gh["missing"], json!(["read:org", "workflow"]));
        let cfg = Config::for_tests(dir.path());
        let a = attention(&cfg);
        assert_eq!(a[0]["id"], "github");
        assert_eq!(a[0]["reauth"], true);
        assert!(a[0]["reason"].as_str().unwrap().contains("workflow"));

        let v = connect(&app, "bitbucket", &json!({"email": "a@b.co", "token": "narrow"})).unwrap();
        assert_eq!(account(&v, "bitbucket")["reauth"], true);

        let v = connect(&app, "github", &json!({"token": "ghp_full"})).unwrap();
        assert_eq!(account(&v, "github")["reauth"], false, "a new sign-in with every scope clears it");
        assert_eq!(attention(&cfg).as_array().unwrap().len(), 1, "only Bitbucket still needs the owner");
    }

    #[test]
    fn judges_scopes_only_when_the_service_lists_them() {
        let s = |v: &[&str]| v.iter().map(|x| x.to_string()).collect::<Vec<_>>();
        assert!(missing_scopes(Provider::Github, &[]).is_empty(), "a fine-grained token lists none");
        assert_eq!(missing_scopes(Provider::Github, &s(&["repo", "admin:org"])), vec!["workflow"], "admin:org covers read:org");
        assert!(missing_scopes(Provider::Github, &s(&["repo", "read:org", "workflow", "gist"])).is_empty());
        assert!(missing_scopes(Provider::Bitbucket, &s(&["repository:write", "account"])).is_empty(), "an older vocabulary isn't judged");
        let bb = s(&["read:user:bitbucket", "write:repository:bitbucket", "write:pullrequest:bitbucket"]);
        assert!(missing_scopes(Provider::Bitbucket, &bb).is_empty(), "write covers read");
        assert!(missing_scopes(Provider::Slack, &s(&["chat:write"])).is_empty());
    }
}
