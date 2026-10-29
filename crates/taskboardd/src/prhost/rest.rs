//! `tb api`: one call to an account's REST API with Taskboard's token, made by the daemon so a GitHub
//! or Bitbucket call goes through the account's [`Gate`](super::limit::Gate) like every other call on
//! it: paced, within the hour's budget, held while the host has the account limited, and counted as
//! the `tb api` reader in the account's hourly split. Its reads go as a `tb pr` command's do
//! ([`priority`](super::limit::priority)), since an agent waits on each, and a read's answer is kept
//! for the same read again ([`keep_for`]): agents ask for the same PR's comments over and over.

use std::time::Duration;

use serde_json::{json, Value};

use crate::accounts::Provider;
use crate::app::App;
use crate::util::*;

/// The URL every path for `p` is under.
pub fn base(app: &App, p: Provider) -> String {
    match p {
        Provider::Github => "https://api.github.com/".into(),
        Provider::Bitbucket => format!("{}/", app.cfg.pr.bitbucket_api.trim_end_matches('/')),
        Provider::Slack => "https://slack.com/api/".into(),
    }
}

/// `path` as a full URL under `base` (a full URL must already be under it).
pub fn url_for(base: &str, label: &str, path: &str) -> std::result::Result<String, String> {
    let url = if path.starts_with("https://") { path.to_string() } else { format!("{base}{}", path.trim_start_matches('/')) };
    if !url.starts_with(base) {
        return Err(format!("{label} requests go to {base}"));
    }
    Ok(url)
}

/// `POST api-call`: `{provider, path, method?, data?, fresh?}` → `{status, body}`, the host's answer as
/// sent; `kept_secs` when it's an answer kept from that long ago ([`keep_for`]), unless `fresh`.
pub fn call(app: &App, body: &Value) -> Result<Value> {
    let p = Provider::parse(body["provider"].as_str().unwrap_or("")).ok_or_else(|| bad("unknown account"))?;
    let (who, secret) = crate::accounts::board_credentials(app, p).ok_or_else(|| bad(format!("{} isn't connected. Ask {} to connect it in Taskboard ▸ Settings.", p.label(), app.cfg.owner)))?;
    let url = url_for(&base(app, p), p.label(), body["path"].as_str().unwrap_or("")).map_err(bad)?;
    let data = body["data"].as_str();
    if let Some(d) = data {
        serde_json::from_str::<Value>(d).map_err(|e| bad(format!("--data isn't JSON: {e}")))?;
    }
    let method = body["method"].as_str().map(|m| m.to_uppercase()).unwrap_or_else(|| if data.is_some() { "POST".into() } else { "GET".into() });
    let key = match p {
        Provider::Bitbucket => Some(super::account_key("bitbucket", &[&who, &secret])),
        Provider::Github => Some(super::account_key("github", &[&secret])),
        Provider::Slack => None,
    };
    let keep = key.as_ref().filter(|_| method == "GET").map(|_| keep_for(app, p, &url));
    let limits = super::limit::limits();
    if let (Some(key), Some((_, mark)), false) = (&key, &keep, body["fresh"] == true) {
        if let Some((mut v, at)) = limits.api_kept(key, &url, mark.as_deref(), now_ts()) {
            v["kept_secs"] = json!((now_ts() - at).round());
            return Ok(v);
        }
    }
    let res = super::limit::reader("tb api", || {
        super::limit::priority(|| match p {
            Provider::Bitbucket => super::bitbucket::BasicHttp::new(&who, &secret).raw(&method, &url, data),
            Provider::Github => github(&secret, &method, &url, data),
            Provider::Slack => send(&format!("Bearer {secret}"), "Slack", &method, &url, data).map(|(v, _)| v).map_err(|(e, _)| e),
        })
    });
    let v = res.map_err(|e| ApiError::new(502, e))?;
    match (&key, keep) {
        (Some(key), Some((secs, mark))) if (200..300).contains(&v["status"].as_u64().unwrap_or(0)) => limits.keep_api(key, &url, v.clone(), secs, mark, now_ts()),
        (Some(key), None) if p == Provider::Github => limits.wrote(key, &url),
        _ => {}
    }
    Ok(v)
}

/// A commit's own files, diff and details never change: kept this long.
const COMMIT_KEEP_SECS: f64 = 3600.0;
/// A read of a PR the board follows: kept until the board's read of that PR changes, at most this long.
const PR_KEEP_SECS: f64 = 7200.0;
/// Any other read: kept this long, so asking again at once doesn't reach the host.
const KEEP_SECS: f64 = 60.0;

/// How long a `tb api` read of `url` is kept, and the mark of its PR when the board follows that PR (a
/// hash of the board's last read of it): the kept answer goes once the mark moves.
pub fn keep_for(app: &App, p: Provider, url: &str) -> (f64, Option<String>) {
    if of_a_commit(url) {
        return (COMMIT_KEEP_SECS, None);
    }
    match (p, super::limit::pr_scope(url).and_then(|s| pr_mark(app, &s))) {
        (Provider::Bitbucket, Some(mark)) => (PR_KEEP_SECS, Some(mark)),
        _ => (KEEP_SECS, None),
    }
}

/// A read of one commit's files, diff or details (not its statuses, which change).
fn of_a_commit(url: &str) -> bool {
    let commit = regex::Regex::new(r"/(?:src|commit|commits|diff|patch|diffstat)/[0-9a-fA-F]{40}(?:[/?]|$)").unwrap();
    commit.is_match(url) && !url.contains("/statuses") && !url.contains("/check-runs")
}

/// A hash of the board's last read of the Bitbucket PR at `scope` (`…/repositories/<ws>/<repo>/pullrequests/<n>`),
/// when a task has that PR.
fn pr_mark(app: &App, scope: &str) -> Option<String> {
    use std::hash::{Hash, Hasher};
    let c = regex::Regex::new(r"/repositories/([^/]+/[^/]+)/pullrequests/(\d+)$").unwrap().captures(scope)?;
    let num: i64 = c[2].parse().ok()?;
    let t = app.db.q1("SELECT * FROM tasks WHERE pr_host = 'bitbucket' AND lower(pr_repo) = ? AND pr_num = ? ORDER BY id DESC LIMIT 1", crate::p![c[1].to_string(), num]).ok()??;
    let rec = crate::prflow::flow(&t).get("rec").filter(|r| r.is_object()).cloned()?;
    let mut h = std::collections::hash_map::DefaultHasher::new();
    rec.to_string().hash(&mut h);
    Some(format!("{:x}", h.finish()))
}

fn bad(msg: impl Into<String>) -> ApiError {
    ApiError::new(400, msg)
}

/// A GitHub call through the gate the board's `gh` calls take on the same token.
fn github(token: &str, method: &str, url: &str, data: Option<&str>) -> std::result::Result<Value, String> {
    let gate = super::limit::Gate::new("github", super::account_key("github", &[token]));
    gate.call(method != "GET", || send(&format!("Bearer {token}"), "GitHub", method, url, data))
}

/// One call: `{status, body}` whatever the status, or an error with the rate limit when the host
/// said the account is limited (a 429, or a 403 with nothing left this hour).
fn send(auth: &str, label: &str, method: &str, url: &str, data: Option<&str>) -> std::result::Result<(Value, Option<super::limit::Hit>), (String, Option<super::limit::Hit>)> {
    let req = ureq::AgentBuilder::new().timeout(Duration::from_secs(30)).build().request(method, url).set("Authorization", auth).set("Accept", "application/json");
    let res = match data {
        Some(d) => req.set("Content-Type", "application/json; charset=utf-8").send_string(d),
        None => req.call(),
    };
    let (code, r) = match res {
        Ok(r) => (r.status(), r),
        Err(ureq::Error::Status(code, r)) => (code, r),
        Err(e) => return Err((format!("couldn't reach {label}: {e}"), None)),
    };
    let num = |h: &str| r.header(h).and_then(|v| v.trim().parse::<f64>().ok());
    if code == 429 || (code == 403 && num("X-RateLimit-Remaining") == Some(0.0)) {
        let hit = super::limit::Hit { retry_after: num("Retry-After"), reset_at: num("X-RateLimit-Reset"), why: format!("{label} answered {code}"), headers: vec![] };
        return Err((format!("{label} answered {code}"), Some(hit)));
    }
    Ok((json!({"status": code, "body": r.into_string().unwrap_or_default()}), None))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_path_goes_under_the_base_and_a_full_url_must_already_be_there() {
        let base = "https://api.bitbucket.org/2.0/";
        assert_eq!(url_for(base, "Bitbucket", "/repositories/w/r").unwrap(), "https://api.bitbucket.org/2.0/repositories/w/r");
        assert_eq!(url_for(base, "Bitbucket", "https://api.bitbucket.org/2.0/user").unwrap(), "https://api.bitbucket.org/2.0/user");
        assert!(url_for(base, "Bitbucket", "https://evil.example/2.0/user").unwrap_err().contains("requests go to"));
    }

    #[test]
    fn a_commit_s_own_reads_never_change_but_its_statuses_do() {
        let sha = "0123456789abcdef0123456789abcdef01234567";
        assert!(of_a_commit(&format!("https://api.bitbucket.org/2.0/repositories/w/r/src/{sha}/lib/a.rs")));
        assert!(of_a_commit(&format!("https://api.bitbucket.org/2.0/repositories/w/r/diff/{sha}")));
        assert!(of_a_commit(&format!("https://api.github.com/repos/o/r/commits/{sha}")));
        assert!(!of_a_commit(&format!("https://api.bitbucket.org/2.0/repositories/w/r/commit/{sha}/statuses")));
        assert!(!of_a_commit("https://api.bitbucket.org/2.0/repositories/w/r/src/main/lib/a.rs"), "a branch moves");
    }
}
