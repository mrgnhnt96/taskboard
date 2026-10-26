//! What server.log says about the waits inside a slow request: an outside command (gh, git, midna,
//! claude) that took a while or timed out, and a long wait for the database. A slow request alone
//! only shows its total.

use std::path::Path;
use std::sync::{Arc, Weak};
use std::time::Duration;

use parking_lot::RwLock;

use crate::app::App;

/// How long a command or a database wait can take before it's worth a line.
pub const SLOW: Duration = Duration::from_secs(1);

static SINK: RwLock<Option<Weak<App>>> = RwLock::new(None);

/// Sends slow commands and database waits to this board's server.log. Until then they go nowhere.
pub fn install(app: &Arc<App>) {
    *SINK.write() = Some(Arc::downgrade(app));
}

fn write(line: String) {
    let app = SINK.read().as_ref().and_then(Weak::upgrade);
    if let Some(a) = app {
        a.info(line);
    }
}

pub enum Ended<'a> {
    Exited { code: Option<i32>, stderr: &'a str },
    TimedOut { stderr: &'a str },
}

/// A command that ran for `took`: logged when it timed out or took longer than `SLOW`.
pub fn command(program: &Path, args: &[String], cwd: Option<&Path>, took: Duration, ended: Ended) {
    if let Some(line) = command_line(program, args, cwd, took, &ended) {
        write(line);
    }
}

/// A wait of `waited` for the database before running `what`: logged when longer than `SLOW`.
pub fn db_wait(waited: Duration, what: &str) {
    if waited > SLOW {
        let thread = std::thread::current();
        let by = thread.name().map(|n| format!(" in thread '{n}'")).unwrap_or_default();
        write(format!("slow db lock: waited {:.1}s{by} for {}", waited.as_secs_f64(), short(what, 80)));
    }
}

fn command_line(program: &Path, args: &[String], cwd: Option<&Path>, took: Duration, ended: &Ended) -> Option<String> {
    let timed_out = matches!(ended, Ended::TimedOut { .. });
    if !timed_out && took <= SLOW {
        return None;
    }
    let name = program.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| program.display().to_string());
    let mut cmd = name;
    for a in args.iter().take(4) {
        cmd.push(' ');
        let a = short(a, 60);
        if a.contains(' ') || a.is_empty() {
            cmd.push_str(&format!("\"{a}\""));
        } else {
            cmd.push_str(&a);
        }
    }
    if args.len() > 4 {
        cmd.push_str(" …");
    }
    let place = cwd.map(|c| format!(" in {}", c.display())).unwrap_or_default();
    let secs = took.as_secs_f64();
    let (head, stderr) = match ended {
        Ended::TimedOut { stderr } => (format!("command timed out after {secs:.1}s: {cmd}{place}"), *stderr),
        Ended::Exited { code, stderr } => {
            let code = code.map(|c| c.to_string()).unwrap_or_else(|| "none (killed by a signal)".into());
            (format!("slow command: {secs:.1}s, exit {code}: {cmd}{place}"), *stderr)
        }
    };
    Some(match stderr.lines().map(str::trim).find(|l| !l.is_empty()) {
        Some(first) => format!("{head} · stderr: {}", short(first, 200)),
        None => head,
    })
}

/// `s` on one line, cut to `max` characters.
fn short(s: &str, max: usize) -> String {
    let one: String = s.split_whitespace().collect::<Vec<_>>().join(" ");
    if one.chars().count() <= max {
        return one;
    }
    let mut cut: String = one.chars().take(max).collect();
    cut.push('…');
    cut
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(a: &[&str]) -> Vec<String> {
        a.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn quick_commands_leave_no_line() {
        let ended = Ended::Exited { code: Some(0), stderr: "" };
        assert!(command_line(Path::new("/usr/bin/git"), &args(&["status"]), None, Duration::from_millis(900), &ended).is_none());
    }

    #[test]
    fn a_slow_command_says_what_where_how_long_and_how_it_ended() {
        let ended = Ended::Exited { code: Some(1), stderr: "\n  HTTP 502: Bad Gateway\nmore\n" };
        let line = command_line(
            Path::new("/opt/homebrew/bin/gh"),
            &args(&["pr", "view", "12", "--json", "state,mergeable"]),
            Some(Path::new("/repo")),
            Duration::from_millis(3240),
            &ended,
        )
        .unwrap();
        assert_eq!(line, "slow command: 3.2s, exit 1: gh pr view 12 --json … in /repo · stderr: HTTP 502: Bad Gateway");
    }

    #[test]
    fn a_timeout_is_logged_however_short() {
        let ended = Ended::TimedOut { stderr: "" };
        let line = command_line(Path::new("midna"), &args(&["session", "read", "a b"]), None, Duration::from_millis(500), &ended).unwrap();
        assert_eq!(line, "command timed out after 0.5s: midna session read \"a b\"");
    }

    #[test]
    fn a_long_argument_is_cut_to_one_short_line() {
        let ended = Ended::Exited { code: None, stderr: "" };
        let prompt = format!("line one\nline two {}", "x".repeat(100));
        let line = command_line(Path::new("claude"), &args(&["-p", &prompt]), None, Duration::from_secs(2), &ended).unwrap();
        assert!(line.starts_with("slow command: 2.0s, exit none (killed by a signal): claude -p \"line one line two xxx"), "{line}");
        assert!(line.ends_with("…\""), "{line}");
        assert!(!line.contains('\n'));
    }

    #[test]
    fn a_timed_out_command_and_a_long_db_wait_reach_server_log() {
        let dir = tempfile::tempdir().unwrap();
        let app = App::new(crate::config::Config::for_tests(dir.path()), false).unwrap();
        install(&app);
        let sleep = crate::proc::which("sleep").unwrap();
        assert!(crate::proc::run(&sleep, &args(&["5"]), Some(dir.path()), 0.2).is_err());
        let (held, has_it) = std::sync::mpsc::channel();
        let holder = {
            let app = app.clone();
            std::thread::spawn(move || {
                app.db
                    .tx(|| {
                        held.send(()).unwrap();
                        std::thread::sleep(Duration::from_millis(1400));
                        Ok(())
                    })
                    .unwrap()
            })
        };
        has_it.recv().unwrap();
        app.db.count("SELECT 1", vec![]).unwrap();
        holder.join().unwrap();
        let log = std::fs::read_to_string(app.cfg.log_path()).unwrap();
        assert!(log.contains("command timed out after 0.2s: sleep 5 in "), "{log}");
        assert!(log.contains("slow db lock: waited 1.") && log.contains("for SELECT 1"), "{log}");
    }
}
