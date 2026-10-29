//! `tb api`: one call to an account's REST API with Taskboard's token, made by the daemon so a GitHub
//! or Bitbucket call goes through the account's [`Gate`](super::limit::Gate) like every other call on
//! it: paced, within the hour's budget, held while the host has the account limited, and counted as
//! the `tb api` reader in the account's hourly split. Its reads go as a `tb pr` command's do
//! ([`priority`](super::limit::priority)), since an agent waits on each.

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

/// `POST api-call`: `{provider, path, method?, data?}` → `{status, body}`, the host's answer as sent.
pub fn call(app: &App, body: &Value) -> Result<Value> {
    let p = Provider::parse(body["provider"].as_str().unwrap_or("")).ok_or_else(|| bad("unknown account"))?;
    let (who, secret) = crate::accounts::board_credentials(app, p).ok_or_else(|| bad(format!("{} isn't connected. Ask {} to connect it in Taskboard ▸ Settings.", p.label(), app.cfg.owner)))?;
    let url = url_for(&base(app, p), p.label(), body["path"].as_str().unwrap_or("")).map_err(bad)?;
    let data = body["data"].as_str();
    if let Some(d) = data {
        serde_json::from_str::<Value>(d).map_err(|e| bad(format!("--data isn't JSON: {e}")))?;
    }
    let method = body["method"].as_str().map(|m| m.to_uppercase()).unwrap_or_else(|| if data.is_some() { "POST".into() } else { "GET".into() });
    let res = super::limit::reader("tb api", || {
        super::limit::priority(|| match p {
            Provider::Bitbucket => super::bitbucket::BasicHttp::new(&who, &secret).raw(&method, &url, data),
            Provider::Github => github(&secret, &method, &url, data),
            Provider::Slack => send(&format!("Bearer {secret}"), "Slack", &method, &url, data).map(|(v, _)| v).map_err(|(e, _)| e),
        })
    });
    res.map_err(|e| ApiError::new(502, e))
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
}
