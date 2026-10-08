//! Accounts: GitHub, Bitbucket and Slack, so the board and its agents can check PRs, comment, push and
//! assign reviewers.
//!
//! - **GitHub** is the `gh` CLI's own sign-in (`gh auth login`, browser or token), so `gh` in every
//!   terminal and `git push` over HTTPS (`gh auth setup-git`) use the same account.
//! - **Bitbucket** is an Atlassian API token with Bitbucket scopes: email + token for the REST API, and
//!   `x-bitbucket-api-token-auth` + token in git's credential helper for `git push`.
//! - **Slack** is an app's OAuth token (`xoxp-` or `xoxb-`), checked with `auth.test`.
//!
//! Bitbucket's and Slack's tokens live in the login Keychain (`taskboard-bitbucket`, `taskboard-slack`);
//! `accounts.json` in the data folder keeps who each account is, never a secret. The API never returns a
//! secret: `tb token <provider>` reads it from the Keychain (or `gh`) itself.
//!
//! A board on a test config (`cfg.accounts_sandbox`) keeps everything in memory, accepts any token but
//! `bad`, and signs GitHub in a few seconds after "Sign in", so the sample board and the tests never
//! touch the Keychain, `gh`, git or the network.

use std::collections::HashMap;
use std::io::{Read, Write};
use std::path::PathBuf;
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
/// How long `gh auth status` (which asks GitHub) is trusted before it's run again.
const GH_STATUS_TTL: Duration = Duration::from_secs(120);
/// How long a browser sign-in may wait for the code to be entered.
const LOGIN_TIMEOUT: Duration = Duration::from_secs(15 * 60);
const SANDBOX_LOGIN_SECS: u64 = 4;

pub const BITBUCKET_GIT_USER: &str = "x-bitbucket-api-token-auth";
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

    /// The Keychain item holding its token (GitHub's is `gh`'s own).
    fn service(self) -> &'static str {
        match self {
            Provider::Github => "",
            Provider::Bitbucket => "taskboard-bitbucket",
            Provider::Slack => "taskboard-slack",
        }
    }
}

fn provider(id: &str) -> Result<Provider> {
    Provider::parse(id).ok_or_else(|| ApiError::new(404, "There's no such account. Use github, bitbucket or slack."))
}

/// A token and who it belongs to: (email or user, secret).
type Cred = (String, String);

/// What the board knows about the accounts beyond `accounts.json`: GitHub's status and sign-in, and
/// the sandbox's Keychain.
pub struct Accounts {
    sandbox: bool,
    gh: Arc<Mutex<Gh>>,
    mem: Mutex<HashMap<Provider, Cred>>,
}

#[derive(Default)]
struct Gh {
    status: Option<(Instant, Value)>,
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

// ------------------------------------------------------------------ the API

/// `GET accounts`: every account's state. `fresh` asks GitHub again instead of using the last answer.
pub fn status(app: &App, fresh: bool) -> Result<Value> {
    let saved = load_who(&app.cfg);
    let mut list = vec![github_status(app, fresh)];
    for p in [Provider::Bitbucket, Provider::Slack] {
        let w = saved.get(p.id()).cloned().unwrap_or(Value::Null);
        let connected = !w.is_null() && app.accounts.get(p).is_some();
        let mut a = base(p, connected);
        if connected {
            for k in ["user", "name", "detail", "scopes", "checked_at", "error", "git"] {
                a[k] = w.get(k).cloned().unwrap_or(Value::Null);
            }
        }
        list.push(a);
    }
    Ok(json!({"accounts": list}))
}

fn base(p: Provider, connected: bool) -> Value {
    json!({"id": p.id(), "label": p.label(), "connected": connected, "user": null, "name": null, "detail": null,
           "scopes": [], "checked_at": null, "error": null, "git": null, "login": null, "ready": true, "setup": null})
}

/// `POST accounts/<id>`: connect with a token (`{"token"}`, Bitbucket also `{"email"}`).
pub fn connect(app: &App, id: &str, body: &Value) -> Result<Value> {
    let p = provider(id)?;
    let token = body_str(body, "token");
    if token.is_empty() {
        return err(400, "Paste a token first.");
    }
    match p {
        Provider::Github => github_token(app, &token)?,
        Provider::Bitbucket => {
            let email = body_str(body, "email");
            if !email.contains('@') {
                return err(400, "Enter the email of your Atlassian account.");
            }
            let mut w = verify(app, p, &(email.clone(), token.clone())).map_err(|e| ApiError::new(400, e))?;
            app.accounts.set(p, (email, token.clone())).map_err(|e| ApiError::new(500, e))?;
            w["git"] = json!(app.accounts.sandbox || git_credential("approve", "bitbucket.org", BITBUCKET_GIT_USER, &token));
            save_who(&app.cfg, p, Some(w));
            app.info("accounts: Bitbucket connected");
        }
        Provider::Slack => {
            if !token.starts_with("xox") {
                return err(400, "That isn't a Slack token. It starts with xoxp- (a user token) or xoxb- (a bot token).");
            }
            let w = verify(app, p, &(String::new(), token.clone())).map_err(|e| ApiError::new(400, e))?;
            let user = w["user"].as_str().unwrap_or("").to_string();
            app.accounts.set(p, (user, token)).map_err(|e| ApiError::new(500, e))?;
            save_who(&app.cfg, p, Some(w));
            app.info("accounts: Slack connected");
        }
    }
    status(app, false)
}

/// `POST accounts/<id>/check`: ask the service again whether the token still works.
pub fn check(app: &App, id: &str) -> Result<Value> {
    let p = provider(id)?;
    if p == Provider::Github {
        return status(app, true);
    }
    let Some(cred) = app.accounts.get(p) else { return err(400, format!("{} isn't connected.", p.label())) };
    let mut saved = load_who(&app.cfg).get(p.id()).cloned().unwrap_or_else(|| json!({}));
    match verify(app, p, &cred) {
        Ok(mut w) => {
            w["git"] = saved.get("git").cloned().unwrap_or(Value::Null);
            save_who(&app.cfg, p, Some(w));
        }
        Err(e) => {
            saved["error"] = json!(e);
            saved["checked_at"] = json!(now_iso());
            save_who(&app.cfg, p, Some(saved));
        }
    }
    status(app, false)
}

/// `POST accounts/<id>/disconnect`: forget the token (GitHub: sign `gh` out of github.com).
pub fn disconnect(app: &App, id: &str) -> Result<Value> {
    let p = provider(id)?;
    match p {
        Provider::Github => github_logout(app)?,
        _ => {
            if p == Provider::Bitbucket && !app.accounts.sandbox {
                if let Some((_, token)) = app.accounts.get(p) {
                    git_credential("reject", "bitbucket.org", BITBUCKET_GIT_USER, &token);
                }
            }
            app.accounts.remove(p);
            save_who(&app.cfg, p, None);
            app.info(format!("accounts: {} disconnected", p.label()));
        }
    }
    status(app, false)
}

// ------------------------------------------------------------------ verifying tokens

fn verify(app: &App, p: Provider, cred: &Cred) -> std::result::Result<Value, String> {
    if app.accounts.sandbox {
        if cred.1 == "bad" {
            return Err(format!("{} didn't accept that token.", p.label()));
        }
        return Ok(match p {
            Provider::Bitbucket => who("sample", "Sample User", "@sample", vec!["read:repository:bitbucket".into(), "write:pullrequest:bitbucket".into()]),
            _ => who("sample", "sample", "Sample Team · sample.slack.com", vec!["chat:write".into()]),
        });
    }
    let agent = ureq::AgentBuilder::new().timeout(HTTP_TIMEOUT).build();
    let scopes = |r: &ureq::Response| -> Vec<String> {
        r.header("x-oauth-scopes").map(|s| s.split(',').map(|x| x.trim().to_string()).filter(|x| !x.is_empty()).collect()).unwrap_or_default()
    };
    match p {
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
        Provider::Github => Err("GitHub signs in through gh.".into()),
    }
}

// ------------------------------------------------------------------ GitHub (gh)

fn gh_path(app: &App) -> Option<PathBuf> {
    proc::which(&app.cfg.pr.gh)
}

fn gh(app: &App, args: &[&str]) -> std::result::Result<proc::Output, String> {
    let Some(bin) = gh_path(app) else { return Err("The GitHub CLI (gh) isn't installed.".into()) };
    let args: Vec<String> = args.iter().map(|s| s.to_string()).collect();
    proc::run(&bin, &args, None, GH_TIMEOUT).map_err(|e| match e {
        proc::RunError::Spawn(e) => format!("Couldn't run gh: {e}"),
        proc::RunError::TimedOut => "gh took too long to answer.".into(),
    })
}

fn last_line(s: &str) -> String {
    s.lines().map(str::trim).filter(|l| !l.is_empty()).last().unwrap_or("").to_string()
}

fn github_status(app: &App, fresh: bool) -> Value {
    let p = Provider::Github;
    let mut a = base(p, false);
    let (login, error) = {
        let g = app.accounts.gh.lock();
        (g.login.as_ref().map(|l| json!({"code": l.code, "url": l.url})), g.error.clone())
    };
    if app.accounts.sandbox {
        let mut g = app.accounts.gh.lock();
        if g.login.as_ref().is_some_and(|l| !l.code.is_empty() && l.started.elapsed() >= Duration::from_secs(SANDBOX_LOGIN_SECS)) {
            g.login = None;
            app.accounts.mem.lock().insert(p, ("sample-user".into(), "gho_sample".into()));
        }
        let signed_in = app.accounts.mem.lock().get(&p).cloned();
        a["login"] = g.login.as_ref().map(|l| json!({"code": l.code, "url": l.url})).unwrap_or(Value::Null);
        a["error"] = json!(g.error);
        if let Some((user, _)) = signed_in {
            a["connected"] = json!(true);
            a["user"] = json!(user);
            a["name"] = json!(user);
            a["detail"] = json!(format!("@{user} · github.com"));
            a["scopes"] = json!(["repo", "read:org", "workflow"]);
            a["git"] = json!(true);
            a["checked_at"] = json!(now_iso());
        }
        return a;
    }
    a["login"] = login.unwrap_or(Value::Null);
    a["error"] = json!(error);
    if gh_path(app).is_none() {
        a["ready"] = json!(false);
        a["setup"] = json!("brew install gh");
        return a;
    }
    let cached = {
        let g = app.accounts.gh.lock();
        g.status.as_ref().filter(|(at, _)| !fresh && at.elapsed() < GH_STATUS_TTL).map(|(_, v)| v.clone())
    };
    let st = match cached {
        Some(v) => v,
        None => {
            let v = gh_auth_status(app);
            app.accounts.gh.lock().status = Some((Instant::now(), v.clone()));
            v
        }
    };
    for k in ["connected", "user", "name", "detail", "scopes", "checked_at", "git"] {
        a[k] = st[k].clone();
    }
    if a["error"].is_null() {
        a["error"] = st["error"].clone();
    }
    a
}

/// `gh auth status` for github.com, as the account's fields.
fn gh_auth_status(app: &App) -> Value {
    let mut a = json!({"connected": false, "checked_at": now_iso(), "scopes": [], "error": null, "git": null});
    let out = match gh(app, &["auth", "status", "--json", "hosts", "--hostname", "github.com"]) {
        Ok(o) => o,
        Err(e) => {
            a["error"] = json!(e);
            return a;
        }
    };
    let v: Value = serde_json::from_str(&out.stdout).unwrap_or(Value::Null);
    let entries = v["hosts"]["github.com"].as_array().cloned().unwrap_or_default();
    let Some(e) = entries.iter().find(|e| e["active"].as_bool() == Some(true)).or(entries.first()) else { return a };
    let user = e["login"].as_str().unwrap_or("").to_string();
    a["user"] = json!(user);
    a["name"] = json!(user);
    a["detail"] = json!(format!("@{user} · github.com"));
    a["scopes"] = json!(e["scopes"].as_str().unwrap_or("").split(',').map(|s| s.trim()).filter(|s| !s.is_empty()).collect::<Vec<_>>());
    match e["state"].as_str() {
        Some("success") => a["connected"] = json!(true),
        Some("timeout") => {
            // Offline: the token is there, GitHub just didn't answer.
            a["connected"] = json!(true);
            a["error"] = json!("Couldn't reach GitHub to check the sign-in.");
        }
        _ => a["error"] = json!(format!("gh's sign-in for @{user} doesn't work anymore. Sign in again.")),
    }
    a["git"] = json!(git_uses_gh());
    a
}

/// Whether git asks `gh` for github.com passwords (`gh auth setup-git`).
fn git_uses_gh() -> bool {
    Command::new("git")
        .args(["config", "--global", "--get-regexp", r"^credential\..*\.helper$"])
        .output()
        .map(|o| String::from_utf8_lossy(&o.stdout).lines().any(|l| l.contains("github.com") && l.contains("gh auth git-credential")))
        .unwrap_or(false)
}

fn forget_gh_status(app: &App) {
    app.accounts.gh.lock().status = None;
}

/// Signs `gh` in with a pasted token, then points git at it.
fn github_token(app: &App, token: &str) -> Result<()> {
    if app.accounts.sandbox {
        if token == "bad" {
            return err(400, "GitHub didn't accept that token.");
        }
        app.accounts.mem.lock().insert(Provider::Github, ("sample-user".into(), token.into()));
        return Ok(());
    }
    let Some(bin) = gh_path(app) else { return err(400, "Install the GitHub CLI first: brew install gh") };
    let mut child = Command::new(bin)
        .args(["auth", "login", "--with-token", "--hostname", "github.com", "--git-protocol", "https"])
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| ApiError::new(500, format!("Couldn't run gh: {e}")))?;
    if let Some(mut i) = child.stdin.take() {
        let _ = i.write_all(format!("{token}\n").as_bytes());
    }
    let mut stderr = String::new();
    if let Some(mut e) = child.stderr.take() {
        let _ = e.read_to_string(&mut stderr);
    }
    let ok = child.wait().map(|s| s.success()).unwrap_or(false);
    forget_gh_status(app);
    if !ok {
        let why = last_line(&stderr);
        return err(400, if why.contains("401") { "GitHub didn't accept that token.".to_string() } else { format!("gh couldn't sign in: {why}") });
    }
    let _ = gh(app, &["auth", "setup-git", "--hostname", "github.com"]);
    app.accounts.gh.lock().error = None;
    app.info("accounts: GitHub signed in with a token");
    Ok(())
}

fn github_logout(app: &App) -> Result<()> {
    if app.accounts.sandbox {
        app.accounts.mem.lock().remove(&Provider::Github);
        return Ok(());
    }
    let user = github_status(app, false)["user"].as_str().unwrap_or("").to_string();
    let mut args = vec!["auth", "logout", "--hostname", "github.com"];
    if !user.is_empty() {
        args.extend(["--user", user.as_str()]);
    }
    let out = gh(app, &args).map_err(|e| ApiError::new(500, e))?;
    forget_gh_status(app);
    if out.code != Some(0) && !out.stderr.contains("not logged in") {
        return err(500, format!("gh couldn't sign out: {}", last_line(&out.stderr)));
    }
    app.info("accounts: GitHub signed out");
    Ok(())
}

/// `POST accounts/github/login`: starts `gh auth login --web` and answers once gh has shown its
/// one-time code (the app shows it and opens github.com/login/device). gh waits for the code to be
/// entered; a thread watches it finish.
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
    let mut child = Command::new(bin)
        .args(["auth", "login", "--web", "--hostname", "github.com", "--git-protocol", "https"])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| ApiError::new(500, format!("Couldn't run gh: {e}")))?;
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
    let log_path = app.cfg.log_path();
    let gh_bin = gh_path(app);
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
        if status == Some(true) {
            if let Some(bin) = gh_bin {
                let _ = proc::run(&bin, &["auth".into(), "setup-git".into(), "--hostname".into(), "github.com".into()], None, GH_TIMEOUT);
            }
        }
        let mut g = gh_state2.lock();
        g.login = None;
        g.status = None;
        g.error = match status {
            Some(false) if started.elapsed() > LOGIN_TIMEOUT => Some("The sign-in code expired. Try again.".into()),
            Some(false) => Some(format!("gh couldn't sign in: {}", last_line(&said.lock()))),
            _ => None,
        };
        let line = format!("{} accounts: GitHub browser sign-in {}\n", chrono::Local::now().format("%Y-%m-%d %H:%M:%S"), match status {
            Some(true) => "finished",
            Some(false) => "failed",
            None => "cancelled",
        });
        if let Ok(mut f) = std::fs::OpenOptions::new().append(true).open(log_path) {
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

/// The token for `tb token` / `tb api`: (user or email, secret). GitHub's comes from `gh auth token`.
pub fn credentials(cfg: &Config, p: Provider) -> Option<Cred> {
    match p {
        Provider::Github => {
            let bin = proc::which(&cfg.pr.gh)?;
            let o = proc::run(&bin, &["auth".into(), "token".into(), "--hostname".into(), "github.com".into()], None, GH_TIMEOUT).ok()?;
            let t = o.stdout.trim().to_string();
            (o.code == Some(0) && !t.is_empty()).then(|| (String::new(), t))
        }
        _ => keychain_get(p.service()),
    }
}

// ------------------------------------------------------------------ Keychain and git

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

/// `git credential approve|reject` for an HTTPS host: hands the token to (or takes it back from)
/// whatever helper git uses (osxkeychain on a Mac). False when git has no helper to keep it.
fn git_credential(action: &str, host: &str, user: &str, secret: &str) -> bool {
    let helper = Command::new("git").args(["config", "--get-all", "credential.helper"]).output().map(|o| !o.stdout.trim_ascii().is_empty()).unwrap_or(false);
    let Ok(mut child) = Command::new("git").args(["credential", action]).stdin(Stdio::piped()).stdout(Stdio::null()).stderr(Stdio::null()).spawn() else {
        return false;
    };
    if let Some(mut i) = child.stdin.take() {
        let _ = i.write_all(format!("protocol=https\nhost={host}\nusername={user}\npassword={secret}\n\n").as_bytes());
    }
    child.wait().is_ok_and(|s| s.success()) && helper
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
    fn github_signs_in_with_the_browser_or_a_token() {
        let (app, _dir) = board();
        let v = github_login(&app).unwrap();
        let gh = account(&v, "github");
        assert_eq!(gh["login"]["code"], "SAMP-1234");
        assert_eq!(gh["connected"], false);
        let v = github_cancel(&app).unwrap();
        assert!(account(&v, "github")["login"].is_null());

        assert!(connect(&app, "github", &json!({"token": "bad"})).is_err());
        let v = connect(&app, "github", &json!({"token": "ghp_x"})).unwrap();
        assert_eq!(account(&v, "github")["connected"], true);
        let v = disconnect(&app, "github").unwrap();
        assert_eq!(account(&v, "github")["connected"], false);
    }
}
