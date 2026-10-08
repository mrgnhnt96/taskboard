//! tb: the agent CLI (`tb` inside Claude sessions), the Claude Code hooks
//! (`tb hook <Event>`) and the status line (`tb statusline`).
mod client;
mod gitcred;
mod hook;
mod tb;

fn main() {
    let args: Vec<String> = std::env::args().collect();
    std::process::exit(tb::main_with(args));
}
