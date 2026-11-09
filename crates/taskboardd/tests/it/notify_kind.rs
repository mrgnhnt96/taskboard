//! Board notifications go as the `board` kind, which Midna refuses until it has it: a send adds the
//! kind and sends again, once.

use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

use serde_json::json;
use taskboardd::midna;

/// A `midna` that refuses `notify.send` with no `board` kind added yet (as Midna does), adds it on
/// `notify.kinds.add`, and logs each method it's called with to `calls`.
fn fake_midna(dir: &Path, has_kind: bool) -> PathBuf {
    let (exe, kind, calls) = (dir.join("midna"), dir.join("kind"), dir.join("calls"));
    if has_kind {
        std::fs::write(&kind, "").unwrap();
    }
    let (kind, calls) = (kind.display(), calls.display());
    let script = format!(
        "#!/bin/sh\necho \"$2\" >> '{calls}'\ncase \"$2\" in\n  notify.kinds.add) touch '{kind}'; echo '{{}}' ;;\n  notify.send) if [ -e '{kind}' ]; then echo '{{\"id\":\"n1\"}}'; else echo 'midna: no kind `board` to send as; kinds you can send to: agent, none added yet (notify.kinds.add)' >&2; exit 2; fi ;;\n  *) echo '{{}}' ;;\nesac\n"
    );
    std::fs::write(&exe, script).unwrap();
    std::fs::set_permissions(&exe, std::fs::Permissions::from_mode(0o755)).unwrap();
    exe
}

fn calls(dir: &Path) -> Vec<String> {
    std::fs::read_to_string(dir.join("calls")).unwrap_or_default().lines().map(str::to_string).collect()
}

fn params() -> serde_json::Value {
    json!({"title": "Start T1?", "category": midna::NOTIFY_KIND, "id": "taskboard-start-T1", "actions": ["Start", "Not now"]})
}

#[test]
fn a_send_with_no_board_kind_adds_it_and_sends_again() {
    let dir = tempfile::tempdir().unwrap();
    let exe = fake_midna(dir.path(), false);
    let r = midna::notify_send(&exe, &params()).expect("sent once the kind was added");
    assert_eq!(r["id"], "n1");
    assert_eq!(calls(dir.path()), ["notify.send", "notify.kinds.add", "notify.send"]);
}

#[test]
fn a_send_with_the_kind_there_sends_once() {
    let dir = tempfile::tempdir().unwrap();
    let exe = fake_midna(dir.path(), true);
    midna::notify_send(&exe, &params()).unwrap();
    assert_eq!(calls(dir.path()), ["notify.send"]);
}

#[test]
fn another_refusal_is_not_retried() {
    let dir = tempfile::tempdir().unwrap();
    let exe = dir.path().join("midna");
    std::fs::write(&exe, format!("#!/bin/sh\necho \"$2\" >> '{}'\necho 'midna: too many notifications' >&2; exit 1\n", dir.path().join("calls").display())).unwrap();
    std::fs::set_permissions(&exe, std::fs::Permissions::from_mode(0o755)).unwrap();
    let e = midna::notify_send(&exe, &params()).unwrap_err();
    assert_eq!(e.to_string(), "too many notifications");
    assert_eq!(calls(dir.path()), ["notify.send"]);
}

#[test]
fn a_start_ask_with_no_ask_kind_adds_one_that_stays_until_answered() {
    let dir = tempfile::tempdir().unwrap();
    let exe = dir.path().join("midna");
    let (kind, log, added) = (dir.path().join("kind"), dir.path().join("calls"), dir.path().join("added"));
    let script = format!(
        "#!/bin/sh\necho \"$2\" >> '{calls}'\ncase \"$2\" in\n  notify.kinds.add) touch '{kind}'; echo \"$3\" > '{added}'; echo '{{}}' ;;\n  notify.send) if [ -e '{kind}' ]; then echo '{{\"id\":\"n2\"}}'; else echo 'midna: no kind `board_ask` to send as; kinds you can send to: agent, board' >&2; exit 2; fi ;;\n  *) echo '{{}}' ;;\nesac\n",
        calls = log.display(),
        kind = kind.display(),
        added = added.display()
    );
    std::fs::write(&exe, script).unwrap();
    std::fs::set_permissions(&exe, std::fs::Permissions::from_mode(0o755)).unwrap();
    let ask = json!({"title": "Start T1?", "category": midna::NOTIFY_ASK_KIND, "id": "taskboard-start-T1", "actions": ["Start", "Not now"]});
    assert_eq!(midna::notify_send(&exe, &ask).unwrap()["id"], "n2");
    assert_eq!(calls(dir.path()), ["notify.send", "notify.kinds.add", "notify.send"]);
    let made: serde_json::Value = serde_json::from_str(std::fs::read_to_string(&added).unwrap().trim()).unwrap();
    assert_eq!(made["key"], midna::NOTIFY_ASK_KIND);
    assert_eq!(made["settings"]["stay"], 0, "it stays on screen until answered: {made}");
}
