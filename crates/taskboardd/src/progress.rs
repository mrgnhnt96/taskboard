//! What a long request says as it goes: a handler calls `stage` as it finishes each stage (and runs a
//! long host call in `waiting`), and a caller that asked for the progress stream (`tb`, with the
//! `X-Task-Board-Progress` header; `server.rs`) gets each line as it's said, so its time limit is per
//! stage rather than for the whole request. Without a sink (the app, hooks, tests) these do nothing.

use std::cell::RefCell;
use std::sync::Arc;
use std::time::Duration;

/// Where a request's progress lines go.
pub type Sink = Arc<dyn Fn(&str) + Send + Sync>;

/// How often `waiting` says it's still waiting.
pub const WAITING_EVERY: Duration = Duration::from_secs(2);

thread_local! {
    static SINK: RefCell<Option<Sink>> = const { RefCell::new(None) };
}

/// Runs `f` with its progress lines going to `sink`.
pub fn with<T>(sink: Sink, f: impl FnOnce() -> T) -> T {
    let before = SINK.with(|s| s.replace(Some(sink)));
    let out = f();
    SINK.with(|s| *s.borrow_mut() = before);
    out
}

fn sink() -> Option<Sink> {
    SINK.with(|s| s.borrow().clone())
}

/// One finished stage of the request.
pub fn stage(line: &str) {
    if let Some(s) = sink() {
        s(line);
    }
}

/// Runs `f` (one call that can run long, on a PR host), saying `still waiting on <what>` every
/// `every` until it returns.
pub fn waiting<T>(what: &str, every: Duration, f: impl FnOnce() -> T) -> T {
    let Some(s) = sink() else { return f() };
    let (done, wait) = std::sync::mpsc::channel::<()>();
    let line = format!("still waiting on {what}");
    let ticker = std::thread::spawn(move || {
        while let Err(std::sync::mpsc::RecvTimeoutError::Timeout) = wait.recv_timeout(every) {
            s(&line);
        }
    });
    let out = f();
    drop(done);
    let _ = ticker.join();
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use parking_lot::Mutex;

    fn collect() -> (Sink, Arc<Mutex<Vec<String>>>) {
        let lines = Arc::new(Mutex::new(vec![]));
        let l = lines.clone();
        (Arc::new(move |s: &str| l.lock().push(s.to_string())), lines)
    }

    #[test]
    fn stages_go_to_the_sink_only_inside_with() {
        stage("nobody hears this");
        let (sink, lines) = collect();
        with(sink, || {
            stage("read the PR");
            stage("merged it");
        });
        stage("nor this");
        assert_eq!(*lines.lock(), ["read the PR", "merged it"]);
    }

    #[test]
    fn waiting_says_so_until_the_call_returns() {
        let (sink, lines) = collect();
        let v = with(sink, || waiting("GitHub to merge PR #3", Duration::from_millis(20), || {
            std::thread::sleep(Duration::from_millis(110));
            7
        }));
        assert_eq!(v, 7);
        let n = lines.lock().len();
        assert!((3..=6).contains(&n), "{:?}", lines.lock());
        assert!(lines.lock().iter().all(|l| l == "still waiting on GitHub to merge PR #3"));
        std::thread::sleep(Duration::from_millis(50));
        assert_eq!(lines.lock().len(), n, "nothing after it returns");
        // With no sink it just runs.
        assert_eq!(waiting("x", Duration::from_millis(1), || 1), 1);
    }
}
