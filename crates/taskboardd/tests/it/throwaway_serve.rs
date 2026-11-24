//! A scratch `taskboardd serve` stops on its own (#1168): these start the real binary on a
//! throwaway data folder, under a home of its own so it never reads the machine's config.

use std::path::Path;
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

const BIN: &str = env!("CARGO_BIN_EXE_taskboardd");

fn free_port() -> u16 {
    std::net::TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap().port()
}

fn serve(home: &Path, data: &Path, port: u16, extra: &[String]) -> Command {
    let mut c = Command::new(BIN);
    c.arg("serve").arg("--data").arg(data).arg("--port").arg(port.to_string()).args(extra);
    for (k, _) in std::env::vars_os() {
        if k.to_string_lossy().starts_with("TASKBOARD_") {
            c.env_remove(&k);
        }
    }
    c.env("HOME", home).env_remove("XPC_SERVICE_NAME").stdin(Stdio::null());
    c
}

fn listening(port: u16) {
    let until = Instant::now() + Duration::from_secs(90);
    while std::net::TcpStream::connect(("127.0.0.1", port)).is_err() {
        assert!(Instant::now() < until, "the board never listened on {port}");
        std::thread::sleep(Duration::from_millis(200));
    }
}

fn exits(child: &mut Child, within: Duration) -> Option<std::process::ExitStatus> {
    let until = Instant::now() + within;
    while Instant::now() < until {
        if let Some(s) = child.try_wait().unwrap() {
            return Some(s);
        }
        std::thread::sleep(Duration::from_millis(200));
    }
    None
}

fn pid_alive(pid: u32) -> bool {
    taskboardd::throwaway::alive(pid)
}

/// Stops a board a failed test left running.
struct Reap(u32);

impl Drop for Reap {
    fn drop(&mut self) {
        unsafe { libc::kill(self.0 as i32, libc::SIGKILL) };
    }
}

#[test]
fn stops_when_its_data_folder_is_deleted() {
    let tmp = tempfile::tempdir().unwrap();
    let data = tmp.path().join("scratch");
    let port = free_port();
    let mut child = serve(tmp.path(), &data, port, &[]).stdout(Stdio::null()).stderr(Stdio::null()).spawn().unwrap();
    let _reap = Reap(child.id());
    listening(port);
    assert!(child.try_wait().unwrap().is_none());
    // Moved away in one step: deleting it races the board writing into it.
    std::fs::rename(&data, tmp.path().join("deleted")).unwrap();
    let status = exits(&mut child, Duration::from_secs(30)).expect("still running after its data folder was deleted");
    assert!(status.success(), "{status}");
}

#[test]
fn stops_when_the_shell_that_started_it_exits() {
    let tmp = tempfile::tempdir().unwrap();
    let data = tmp.path().join("scratch");
    let port = free_port();
    let pidfile = tmp.path().join("pid");
    let go = tmp.path().join("go");
    let cmd = serve(tmp.path(), &data, port, &[]);
    let args: Vec<String> = cmd.get_args().map(|a| format!("'{}'", a.to_string_lossy())).collect();
    // The shell stays until the test says go, so the board is listening before it's orphaned.
    let script = format!(
        "'{BIN}' {args} >/dev/null 2>&1 & echo $! > '{pid}.tmp' && mv '{pid}.tmp' '{pid}'; while [ ! -e '{go}' ]; do sleep 0.1; done",
        args = args.join(" "),
        pid = pidfile.display(),
        go = go.display()
    );
    let mut sh = Command::new("sh");
    sh.arg("-c").arg(script).stdin(Stdio::null());
    for (k, v) in cmd.get_envs() {
        match v {
            Some(v) => sh.env(k, v),
            None => sh.env_remove(k),
        };
    }
    let mut sh = sh.spawn().unwrap();
    let until = Instant::now() + Duration::from_secs(10);
    while !pidfile.exists() {
        assert!(Instant::now() < until, "the shell never started the board");
        std::thread::sleep(Duration::from_millis(50));
    }
    let pid: u32 = std::fs::read_to_string(&pidfile).unwrap().trim().parse().unwrap();
    let _reap = Reap(pid);
    listening(port);
    std::thread::sleep(Duration::from_secs(3));
    assert!(pid_alive(pid), "stopped while the shell that started it was running");
    std::fs::write(&go, "").unwrap();
    assert!(sh.wait().unwrap().success());
    let until = Instant::now() + Duration::from_secs(30);
    while pid_alive(pid) {
        assert!(Instant::now() < until, "orphaned scratch board {pid} still running");
        std::thread::sleep(Duration::from_millis(200));
    }
    assert!(data.is_dir());
}

#[test]
fn stops_when_the_exit_with_process_exits() {
    let tmp = tempfile::tempdir().unwrap();
    let data = tmp.path().join("scratch");
    let port = free_port();
    let mut keeper = Command::new("sleep").arg("600").spawn().unwrap();
    let mut child = serve(tmp.path(), &data, port, &["--exit-with".into(), keeper.id().to_string()])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    let _reap = Reap(child.id());
    listening(port);
    std::thread::sleep(Duration::from_secs(3));
    assert!(child.try_wait().unwrap().is_none(), "stopped while its --exit-with process was running");
    keeper.kill().unwrap();
    keeper.wait().unwrap();
    let status = exits(&mut child, Duration::from_secs(30)).expect("still running after its --exit-with process exited");
    assert!(status.success(), "{status}");
    assert!(data.is_dir());
}
