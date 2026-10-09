//! What makes a request the owner's own click in Taskboard.app (`X-Task-Board-From: app`): Start, a
//! wave's review stop and the app's other signals.
//!
//! The daemon listens on a unix socket (`app.sock` in its data folder) next to its TCP port, and the
//! app sends its posts there. For a connection on the socket the kernel gives the daemon the peer's
//! audit token (`LOCAL_PEERTOKEN`, taken at `connect`; it names one process instance, so a reused
//! pid or an `exec` after connecting doesn't pass for it), and Security.framework checks that running
//! process against a code requirement: Apple-anchored, the app's bundle id, signed by the same team as
//! the daemon itself, valid right now, with the hardened runtime on and no debugger attached.
//!
//! Which way the daemon trusts is decided once, at launch, from its own signature (`Trust::detect`):
//! - Signed with a team's certificate (a release, or `packaging/build-app.sh` with a Developer ID):
//!   only the signature counts. No token file is written, and a token is refused. Nothing in
//!   config.toml loosens this, since an agent could edit that file.
//! - Ad-hoc or unsigned (`cargo run`, `scripts/dev-app.sh`, tests): there is no signature to check,
//!   so the per-launch token file of `apptoken` counts, as before, unless `[app_origin] token = false`.
//!   Agents run as the same user and can read that file; the PreToolUse guard
//!   (`apptoken::tool_reaches`) only catches the plain ways to.

use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::time::Duration;

use serde::Deserialize;

use crate::config::Config;

/// The socket's file in the data folder.
pub const SOCKET: &str = "app.sock";

pub fn socket_path(cfg: &Config) -> PathBuf {
    cfg.data.join(SOCKET)
}

/// `[app_origin]` in config.toml.
#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct AppOriginConfig {
    /// In a build with no team signature, take the app token file as the app's (false: no request is
    /// the app's there). A team-signed daemon never takes the token, whatever this says.
    pub token: bool,
}

impl Default for AppOriginConfig {
    fn default() -> Self {
        AppOriginConfig { token: true }
    }
}

/// Who the daemon takes for the app this launch.
#[derive(Debug, Clone, PartialEq)]
pub struct Trust {
    /// The code requirement the process on the other end of the app socket has to meet.
    pub requirement: Option<String>,
    /// That process also has to run with the hardened runtime and no debugger.
    pub runtime: bool,
    /// The token file (`apptoken`) counts.
    pub token: bool,
}

/// A daemon's own signature: its code identifier and its certificate's team.
#[derive(Debug, Clone, PartialEq)]
pub struct Signature {
    pub identifier: String,
    pub team: String,
}

impl Trust {
    /// No signature to go by: the token, when `token` allows it.
    pub fn unsigned(token: bool) -> Trust {
        Trust { requirement: None, runtime: false, token }
    }

    /// This launch's trust, from the running daemon's own signature.
    pub fn detect(cfg: &Config) -> Trust {
        Trust::from_signature(own_signature(), cfg.app_origin.token)
    }

    /// `build-app.sh` signs the daemon as `<bundle id>.daemon` and the app as `<bundle id>`. A team
    /// signature on anything else (the CLI, a renamed build) trusts nobody rather than guessing.
    pub fn from_signature(sig: Option<Signature>, token: bool) -> Trust {
        let Some(sig) = sig else { return Trust::unsigned(token) };
        let app = sig.identifier.strip_suffix(".daemon").filter(|a| safe_word(a) && safe_word(&sig.team));
        Trust { requirement: app.map(|a| requirement(a, &sig.team)), runtime: true, token: false }
    }

    /// The board log's line about it.
    pub fn describe(&self) -> String {
        match (&self.requirement, self.token) {
            (Some(r), _) => format!("app requests: signed peers on {SOCKET} only ({r})"),
            (None, true) => "app requests: the app token (this build has no team signature)".into(),
            (None, false) => "app requests: none (no team signature, and [app_origin] token = false)".into(),
        }
    }
}

fn safe_word(s: &str) -> bool {
    !s.is_empty() && s.chars().all(|c| c.is_ascii_alphanumeric() || c == '.' || c == '-')
}

/// The requirement for the app `app_id` signed by `team`.
pub fn requirement(app_id: &str, team: &str) -> String {
    format!("anchor apple generic and identifier \"{app_id}\" and certificate leaf[subject.OU] = \"{team}\"")
}

/// What the server knows of a connection on the app socket.
#[derive(Clone, Debug, Default)]
pub struct Peer {
    /// The peer's audit token, when the kernel gave one.
    pub audit_token: Option<[u32; 8]>,
}

impl axum::extract::connect_info::Connected<axum::serve::IncomingStream<'_, tokio::net::UnixListener>> for Peer {
    fn connect_info(stream: axum::serve::IncomingStream<'_, tokio::net::UnixListener>) -> Self {
        use std::os::fd::AsRawFd;
        Peer { audit_token: peer_audit_token(stream.io().as_raw_fd()) }
    }
}

/// The audit token of the process that connected `fd`, a unix socket.
#[cfg(target_os = "macos")]
pub fn peer_audit_token(fd: std::os::fd::RawFd) -> Option<[u32; 8]> {
    let mut token = [0u32; 8];
    let mut len = std::mem::size_of_val(&token) as libc::socklen_t;
    let r = unsafe { libc::getsockopt(fd, libc::SOL_LOCAL, libc::LOCAL_PEERTOKEN, token.as_mut_ptr().cast(), &mut len) };
    (r == 0 && len as usize == std::mem::size_of_val(&token)).then_some(token)
}

#[cfg(not(target_os = "macos"))]
pub fn peer_audit_token(_fd: std::os::fd::RawFd) -> Option<[u32; 8]> {
    None
}

/// Whether a request that says it's from the app is: its connection's peer meets the requirement, or
/// (when this build takes it) it carries the token.
pub fn is_app(trust: &Trust, peer: Option<&Peer>, given_token: Option<&str>, token: &str) -> bool {
    if let (Some(req), Some(audit)) = (&trust.requirement, peer.and_then(|p| p.audit_token)) {
        if check_peer(&audit, req, trust.runtime).is_ok() {
            return true;
        }
    }
    trust.token && crate::apptoken::matches(given_token, token)
}

// The CS_* status bits of a running process (<kern/cs_blobs.h>).
const CS_VALID: u32 = 0x1;
const CS_RUNTIME: u32 = 0x10000;
const CS_DEBUGGED: u32 = 0x1000_0000;

/// Whether the process with audit token `audit` meets `requirement` (and, with `runtime`, runs with
/// the hardened runtime and no debugger), checked by Security.framework against the running code.
#[cfg(target_os = "macos")]
pub fn check_peer(audit: &[u32; 8], requirement: &str, runtime: bool) -> Result<(), String> {
    use core_foundation::base::TCFType;
    use core_foundation::data::CFData;
    use security_framework::os::macos::code_signing::{Flags, GuestAttributes, SecCode, SecRequirement};

    let bytes: Vec<u8> = audit.iter().flat_map(|w| w.to_ne_bytes()).collect();
    let data = CFData::from_buffer(&bytes);
    let mut attrs = GuestAttributes::new();
    attrs.set_audit_token(data.as_concrete_TypeRef());
    let code = SecCode::copy_guest_with_attribues(None, &attrs, Flags::NONE).map_err(|e| format!("no running code for that peer: {e}"))?;
    let req: SecRequirement = requirement.parse().map_err(|e| format!("bad requirement: {e}"))?;
    code.check_validity(Flags::NONE, &req).map_err(|e| format!("the peer doesn't meet the requirement: {e}"))?;
    if runtime {
        let status = ffi::status(&code).ok_or("the peer's signing status can't be read")?;
        if status & CS_VALID == 0 || status & CS_RUNTIME == 0 || status & CS_DEBUGGED != 0 {
            return Err(format!("the peer's signing status is {status:#x} (it needs the hardened runtime and no debugger)"));
        }
    }
    Ok(())
}

#[cfg(not(target_os = "macos"))]
pub fn check_peer(_audit: &[u32; 8], _requirement: &str, _runtime: bool) -> Result<(), String> {
    Err("code signatures are only checked on macOS".into())
}

/// The running daemon's own signature, when it's signed with a certificate that chains to Apple and
/// names a team (ad-hoc and linker signatures have no team).
#[cfg(target_os = "macos")]
pub fn own_signature() -> Option<Signature> {
    use security_framework::os::macos::code_signing::{Flags, SecCode, SecRequirement};
    let me = SecCode::for_self(Flags::NONE).ok()?;
    let (identifier, team) = ffi::identity(&me)?;
    if !safe_word(&team) {
        return None;
    }
    let anchored: SecRequirement = format!("anchor apple generic and certificate leaf[subject.OU] = \"{team}\"").parse().ok()?;
    me.check_validity(Flags::NONE, &anchored).ok()?;
    Some(Signature { identifier, team })
}

#[cfg(not(target_os = "macos"))]
pub fn own_signature() -> Option<Signature> {
    None
}

/// This process's code identifier, signed or ad-hoc (tests).
#[cfg(all(test, target_os = "macos"))]
pub(crate) fn own_identifier() -> Option<String> {
    use security_framework::os::macos::code_signing::{Flags, SecCode};
    ffi::identifier(&SecCode::for_self(Flags::NONE).ok()?)
}

/// SecCodeCopySigningInformation, which the security-framework crate doesn't wrap.
#[cfg(target_os = "macos")]
mod ffi {
    use core_foundation::base::{CFType, TCFType};
    use core_foundation::dictionary::{CFDictionary, CFDictionaryRef};
    use core_foundation::number::CFNumber;
    use core_foundation::string::{CFString, CFStringRef};
    use security_framework::os::macos::code_signing::SecCode;

    const SIGNING_INFORMATION: u32 = 1 << 1;
    const DYNAMIC_INFORMATION: u32 = 1 << 3;

    #[link(name = "Security", kind = "framework")]
    unsafe extern "C" {
        fn SecCodeCopySigningInformation(code: *const std::ffi::c_void, flags: u32, info: *mut CFDictionaryRef) -> i32;
        static kSecCodeInfoIdentifier: CFStringRef;
        static kSecCodeInfoTeamIdentifier: CFStringRef;
        static kSecCodeInfoStatus: CFStringRef;
    }

    fn info(code: &SecCode, flags: u32) -> Option<CFDictionary<CFString, CFType>> {
        let mut dict: CFDictionaryRef = std::ptr::null();
        let r = unsafe { SecCodeCopySigningInformation(code.as_concrete_TypeRef() as *const _, flags, &mut dict) };
        (r == 0 && !dict.is_null()).then(|| unsafe { CFDictionary::wrap_under_create_rule(dict) })
    }

    fn string(d: &CFDictionary<CFString, CFType>, key: CFStringRef) -> Option<String> {
        let key = unsafe { CFString::wrap_under_get_rule(key) };
        d.find(&key)?.downcast::<CFString>().map(|s| s.to_string())
    }

    /// (identifier, team) of signed code; None without a team.
    pub fn identity(code: &SecCode) -> Option<(String, String)> {
        let d = info(code, SIGNING_INFORMATION)?;
        Some((string(&d, unsafe { kSecCodeInfoIdentifier })?, string(&d, unsafe { kSecCodeInfoTeamIdentifier })?))
    }

    /// The running code's CS_* status bits.
    pub fn status(code: &SecCode) -> Option<u32> {
        let d = info(code, DYNAMIC_INFORMATION)?;
        let key = unsafe { CFString::wrap_under_get_rule(kSecCodeInfoStatus) };
        d.find(&key)?.downcast::<CFNumber>()?.to_i64().map(|n| n as u32)
    }

    /// The identifier of signed code, team or not (tests).
    #[cfg(test)]
    pub fn identifier(code: &SecCode) -> Option<String> {
        string(&info(code, SIGNING_INFORMATION)?, unsafe { kSecCodeInfoIdentifier })
    }
}

/// Binds the app socket at `path`, replacing a stale one, connectable by this user only. A path
/// macOS can't bind (over 103 bytes) is an error.
pub fn bind(path: &Path) -> std::io::Result<tokio::net::UnixListener> {
    use std::os::unix::fs::{FileTypeExt, PermissionsExt};
    if path.as_os_str().len() > 103 {
        return Err(std::io::Error::other(format!("{} is too long for a socket", path.display())));
    }
    match std::fs::symlink_metadata(path) {
        Ok(m) if m.file_type().is_socket() => std::fs::remove_file(path)?,
        Ok(_) => return Err(std::io::Error::other(format!("{} is there and isn't a socket", path.display()))),
        Err(_) => {}
    }
    let l = tokio::net::UnixListener::bind(path)?;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))?;
    Ok(l)
}

/// A blocking `POST /tasks/api/<api_path>` over the app socket: (status, body). `Connection: close`,
/// so the answer is everything up to EOF.
pub fn post(socket: &Path, api_path: &str, headers: &[(&str, &str)], body: &[u8], timeout: Duration) -> std::io::Result<(u16, Vec<u8>)> {
    let mut s = std::os::unix::net::UnixStream::connect(socket)?;
    s.set_read_timeout(Some(timeout))?;
    s.set_write_timeout(Some(timeout))?;
    let mut req = format!("POST /tasks/api/{api_path} HTTP/1.1\r\nHost: taskboard\r\nContent-Type: application/json\r\nAccept: application/json\r\nContent-Length: {}\r\nConnection: close\r\n", body.len());
    for (k, v) in headers {
        if k.contains(['\r', '\n']) || v.contains(['\r', '\n']) {
            return Err(std::io::Error::other("a header can't hold a line break"));
        }
        req.push_str(&format!("{k}: {v}\r\n"));
    }
    req.push_str("\r\n");
    s.write_all(req.as_bytes())?;
    s.write_all(body)?;
    let mut out = vec![];
    s.read_to_end(&mut out)?;
    parse_response(&out).ok_or_else(|| std::io::Error::other("the board's answer isn't HTTP"))
}

/// (status, body) of a whole HTTP/1.1 response.
fn parse_response(raw: &[u8]) -> Option<(u16, Vec<u8>)> {
    let end = raw.windows(4).position(|w| w == b"\r\n\r\n")?;
    let head = std::str::from_utf8(&raw[..end]).ok()?;
    let status = head.lines().next()?.split_whitespace().nth(1)?.parse().ok()?;
    let body = &raw[end + 4..];
    let chunked = head.lines().any(|l| {
        let l = l.to_ascii_lowercase();
        l.starts_with("transfer-encoding:") && l.contains("chunked")
    });
    if !chunked {
        return Some((status, body.to_vec()));
    }
    let (mut out, mut rest) = (vec![], body);
    loop {
        let nl = rest.windows(2).position(|w| w == b"\r\n")?;
        let size = usize::from_str_radix(std::str::from_utf8(&rest[..nl]).ok()?.split(';').next()?.trim(), 16).ok()?;
        rest = &rest[nl + 2..];
        if size == 0 {
            return Some((status, out));
        }
        out.extend_from_slice(rest.get(..size)?);
        rest = rest.get(size + 2..)?;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_team_signed_daemon_trusts_the_signed_app_only() {
        let sig = Signature { identifier: "com.mrgnhnt.taskboard.daemon".into(), team: "U2G2XV3688".into() };
        let t = Trust::from_signature(Some(sig), true);
        assert_eq!(
            t.requirement.as_deref(),
            Some("anchor apple generic and identifier \"com.mrgnhnt.taskboard\" and certificate leaf[subject.OU] = \"U2G2XV3688\"")
        );
        assert!(t.runtime);
        assert!(!t.token, "a team-signed daemon never takes the token, whatever the config says");
        // The dev app's daemon trusts the dev app.
        let dev = Trust::from_signature(Some(Signature { identifier: "com.mrgnhnt.taskboard.dev.daemon".into(), team: "U2G2XV3688".into() }), true);
        assert!(dev.requirement.unwrap().contains("identifier \"com.mrgnhnt.taskboard.dev\""));
    }

    #[test]
    fn a_team_signature_on_something_else_trusts_nobody() {
        for (id, team) in [("com.mrgnhnt.taskboard.tb", "U2G2XV3688"), ("x\" or anchor apple generic or identifier \"y.daemon", "U2G2XV3688"), ("a.daemon", "T\"")] {
            let t = Trust::from_signature(Some(Signature { identifier: id.into(), team: team.into() }), true);
            assert_eq!(t, Trust { requirement: None, runtime: true, token: false }, "{id}");
            assert!(!is_app(&t, Some(&Peer { audit_token: Some([0; 8]) }), Some("tok"), "tok"));
        }
    }

    #[test]
    fn an_unsigned_daemon_takes_the_token_unless_told_not_to() {
        let t = Trust::from_signature(None, true);
        assert!(is_app(&t, None, Some("abc"), "abc"));
        assert!(!is_app(&t, None, Some("abd"), "abc"));
        assert!(!is_app(&t, None, None, "abc"));
        let off = Trust::from_signature(None, false);
        assert!(!is_app(&off, None, Some("abc"), "abc"));
        let signed = Trust::from_signature(Some(Signature { identifier: "com.mrgnhnt.taskboard.daemon".into(), team: "U2G2XV3688".into() }), true);
        assert!(!is_app(&signed, None, Some("abc"), "abc"), "the token doesn't count for a signed daemon");
    }

    #[test]
    fn a_test_build_has_no_team_signature() {
        assert_eq!(own_signature(), None);
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(Trust::detect(&Config::for_tests(dir.path())), Trust::unsigned(true));
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn the_peer_is_checked_by_its_running_code() {
        use std::os::fd::AsRawFd;
        // A socket pair: each end's peer is this test process.
        let (a, _b) = std::os::unix::net::UnixStream::pair().unwrap();
        let audit = peer_audit_token(a.as_raw_fd()).expect("audit token");
        assert_eq!(audit[5] as i32, std::process::id() as i32, "the token's pid");
        let me = own_identifier().unwrap();
        check_peer(&audit, &format!("identifier \"{me}\""), false).unwrap();
        // A test build has no hardened runtime.
        assert!(check_peer(&audit, &format!("identifier \"{me}\""), true).unwrap_err().contains("hardened runtime"));
        assert!(check_peer(&audit, &requirement("com.mrgnhnt.taskboard", "U2G2XV3688"), false).is_err());
        assert!(check_peer(&audit, &format!("identifier \"{me}\" and anchor apple generic"), false).is_err(), "not Apple-anchored");
        // A made-up token names no process.
        let mut gone = audit;
        gone[5] = 0x7fff_fff0;
        assert!(check_peer(&gone, &format!("identifier \"{me}\""), false).is_err());
    }

    /// Apple's own `nc` connects from outside: its running code passes for itself and not for the app.
    #[cfg(target_os = "macos")]
    #[tokio::test]
    async fn another_process_on_the_socket_is_checked_too() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(SOCKET);
        let l = bind(&path).unwrap();
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(std::fs::metadata(&path).unwrap().permissions().mode() & 0o777, 0o600);
        let mut nc = std::process::Command::new("/usr/bin/nc").arg("-U").arg(&path).stdin(std::process::Stdio::piped()).spawn().unwrap();
        let (s, _) = tokio::time::timeout(Duration::from_secs(10), l.accept()).await.unwrap().unwrap();
        use std::os::fd::AsRawFd;
        let audit = peer_audit_token(s.as_raw_fd()).unwrap();
        assert_eq!(audit[5], nc.id());
        let r = (
            check_peer(&audit, "anchor apple and identifier \"com.apple.nc\"", false),
            check_peer(&audit, "anchor apple and identifier \"com.apple.nc\"", true),
            check_peer(&audit, &requirement("com.mrgnhnt.taskboard", "U2G2XV3688"), false),
        );
        let _ = nc.kill();
        let _ = nc.wait();
        assert_eq!(r.0, Ok(()));
        assert_eq!(r.1, Ok(()), "Apple runs its own tools with the hardened runtime");
        assert!(r.2.is_err());
        // A stale socket is replaced; a file that isn't one is left alone.
        drop((s, l));
        bind(&path).unwrap();
        let file = dir.path().join("plain");
        std::fs::write(&file, "x").unwrap();
        assert!(bind(&file).is_err());
        assert_eq!(std::fs::read_to_string(&file).unwrap(), "x");
    }

    #[test]
    fn answers_are_read_plain_or_chunked() {
        assert_eq!(parse_response(b"HTTP/1.1 200 OK\r\ncontent-length: 2\r\n\r\n{}"), Some((200, b"{}".to_vec())));
        assert_eq!(
            parse_response(b"HTTP/1.1 403 Forbidden\r\nTransfer-Encoding: chunked\r\n\r\n4\r\n{\"a\"\r\n3\r\n:1}\r\n0\r\n\r\n"),
            Some((403, b"{\"a\":1}".to_vec()))
        );
        assert_eq!(parse_response(b"garbage"), None);
    }
}
