use std::net::SocketAddr;
use std::path::PathBuf;

use clap::{Parser, Subcommand};

use taskboardd::app::App;
use taskboardd::config::{self, Config};
use taskboardd::util::expand_home;

#[derive(Parser)]
#[command(name = "taskboardd", about = "The task board daemon: the API, the runner and the Midna sync (Taskboard.app runs it as a LaunchAgent)", version)]
struct Cli {
    #[command(subcommand)]
    cmd: Option<Cmd>,
}

#[derive(Subcommand)]
enum Cmd {
    /// Run the board: the page, the API, the runner and Midna sync (the default)
    Serve {
        /// Use this data folder instead of the configured one
        #[arg(long)]
        data: Option<PathBuf>,
        #[arg(long)]
        port: Option<u16>,
        /// Run the runner (start terminals, send messages) even on a non-default data folder
        #[arg(long)]
        runner: bool,
    },
    /// Write a starting config.toml (if there's none) and print where things live
    Init,
    /// Fill an empty data folder with sample data to try the page
    Seed {
        #[arg(long)]
        data: PathBuf,
    },
    /// Write the plain-text copy of every task again
    Md,
    /// Print a launchd plist that runs the board at login
    Launchd {
        /// The label for the launch agent
        #[arg(long, default_value = "dev.taskboard.server")]
        label: String,
    },
    /// Print the config the board would use
    Config,
}

fn load(data: Option<PathBuf>, port: Option<u16>, runner: bool) -> Config {
    let mut cfg = Config::load().unwrap_or_else(|e| {
        eprintln!("taskboardd: {e}");
        std::process::exit(2);
    });
    if let Some(d) = data {
        let default = cfg.data == config::default_data_dir();
        cfg.statusline_dir = if cfg.statusline_dir == cfg.data.join("statusline") { d.join("statusline") } else { cfg.statusline_dir.clone() };
        cfg.data = d;
        if default && std::env::var("TASKBOARD_RUNNER").is_err() {
            cfg.runner = cfg.data == config::default_data_dir();
        }
    }
    if let Some(p) = port {
        cfg.port = p;
    }
    if runner {
        cfg.runner = true;
    }
    cfg
}

/// launchd starts the daemon with a bare PATH (/usr/bin:/bin:/usr/sbin:/sbin), and the bundled
/// LaunchAgent plist can't name the home folder. Put the usual tool folders (midna, gh, claude,
/// git) in front so the runner finds them the way a login shell would.
fn widen_path() {
    let home = expand_home("~");
    let cur = std::env::var("PATH").unwrap_or_default();
    let mut parts: Vec<String> = [".local/bin", ".cargo/bin"].iter().map(|d| home.join(d).to_string_lossy().to_string()).collect();
    parts.extend(["/opt/homebrew/bin", "/usr/local/bin"].map(String::from));
    parts.retain(|p| !cur.split(':').any(|c| c == p));
    if !parts.is_empty() {
        std::env::set_var("PATH", format!("{}:{cur}", parts.join(":")));
    }
}

fn serve(cfg: Config) -> i32 {
    widen_path();
    let addr: SocketAddr = match format!("{}:{}", cfg.host, cfg.port).parse() {
        Ok(a) => a,
        Err(e) => {
            eprintln!("taskboardd: {}:{} isn't an address: {e}", cfg.host, cfg.port);
            return 2;
        }
    };
    let rt = tokio::runtime::Builder::new_multi_thread().enable_all().build().expect("tokio runtime");
    rt.block_on(async move {
        let listener = match tokio::net::TcpListener::bind(addr).await {
            Ok(l) => l,
            Err(e) if e.kind() == std::io::ErrorKind::AddrInUse => {
                eprintln!("taskboardd: something is already listening on {addr}; is the board already running?");
                return 0;
            }
            Err(e) => {
                eprintln!("taskboardd: can't listen on {addr}: {e}");
                return 1;
            }
        };
        let app = match App::new(cfg, true) {
            Ok(a) => a,
            Err(e) => {
                eprintln!("taskboardd: {e}");
                return 1;
            }
        };
        app.info(format!(
            "taskboardd {} on http://{addr}/tasks/ · data {} · runner {}",
            env!("CARGO_PKG_VERSION"),
            app.cfg.data.display(),
            if app.cfg.runner { "on" } else { "off" }
        ));
        taskboardd::start_threads(&app);
        let router = taskboardd::server::router(app.clone());
        let stop = app.clone();
        let shutdown = async move {
            // ctrl-c in a terminal, SIGTERM from launchd (logout, the app unregistering the agent).
            let mut term = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate()).expect("SIGTERM handler");
            tokio::select! {
                _ = tokio::signal::ctrl_c() => {}
                _ = term.recv() => {}
            }
            stop.stop();
        };
        if let Err(e) = axum::serve(listener, router).with_graceful_shutdown(shutdown).await {
            app.info(format!("server stopped: {e}"));
            return 1;
        }
        0
    })
}

fn init() -> i32 {
    let path = config::default_config_path();
    if path.exists() {
        println!("Config: {} (already there; left alone)", path.display());
    } else {
        if let Some(d) = path.parent() {
            let _ = std::fs::create_dir_all(d);
        }
        if let Err(e) = std::fs::write(&path, config::EXAMPLE) {
            eprintln!("taskboardd: couldn't write {}: {e}", path.display());
            return 1;
        }
        println!("Config: {} (written; every key is optional)", path.display());
    }
    let cfg = load(None, None, false);
    println!("Data: {}", cfg.data.display());
    println!("API: {}api", cfg.url());
    println!("Notification links: {}", cfg.page_url);
    println!("Owner: {}", cfg.owner);
    println!("Midna CLI: {}{}", cfg.midna.display(), if cfg.midna.is_file() { "" } else { " (not found)" });
    println!("Jira: {}", if cfg.jira_on() { format!("{} / {}", cfg.jira.site, cfg.jira.project) } else { "off".into() });
    0
}

fn launchd(label: &str) -> i32 {
    let exe = std::env::current_exe().map(|p| p.to_string_lossy().to_string()).unwrap_or_else(|_| "taskboardd".into());
    let cfg = load(None, None, false);
    let log = cfg.log_path();
    let home = expand_home("~");
    let path = format!(
        "{}/.local/bin:{}/.cargo/bin:/opt/homebrew/bin:/usr/local/bin:/usr/bin:/bin:/usr/sbin:/sbin",
        home.display(),
        home.display()
    );
    println!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<!-- Save as ~/Library/LaunchAgents/{label}.plist, then:
     launchctl bootstrap gui/$(id -u) ~/Library/LaunchAgents/{label}.plist -->
<plist version="1.0">
<dict>
  <key>Label</key><string>{label}</string>
  <key>ProgramArguments</key>
  <array>
    <string>{exe}</string>
    <string>serve</string>
  </array>
  <key>EnvironmentVariables</key>
  <dict>
    <key>PATH</key><string>{path}</string>
  </dict>
  <key>RunAtLoad</key><true/>
  <key>KeepAlive</key><dict><key>SuccessfulExit</key><false/></dict>
  <key>ThrottleInterval</key><integer>10</integer>
  <key>StandardOutPath</key><string>{log}</string>
  <key>StandardErrorPath</key><string>{log}</string>
</dict>
</plist>"#,
        log = log.display()
    );
    0
}

fn main() {
    let cli = Cli::parse();
    let code = match cli.cmd.unwrap_or(Cmd::Serve { data: None, port: None, runner: false }) {
        Cmd::Serve { data, port, runner } => serve(load(data, port, runner)),
        Cmd::Init => init(),
        Cmd::Seed { data } => {
            let mut cfg = load(Some(data), None, false);
            cfg.runner = false;
            match App::new(cfg, false).and_then(|a| taskboardd::seed::seed(&a).map(|_| a)) {
                Ok(a) => {
                    for t in a.db.q("SELECT id FROM tasks", taskboardd::p![]).unwrap_or_default() {
                        let _ = taskboardd::mdcopy::write(&a, t["id"].as_i64().unwrap_or(0));
                    }
                    println!("Seeded {}. Try: taskboardd serve --data {} --port 18792", a.cfg.data.display(), a.cfg.data.display());
                    0
                }
                Err(e) => {
                    eprintln!("taskboardd: {e}");
                    1
                }
            }
        }
        Cmd::Md => {
            let cfg = load(None, None, false);
            match App::new(cfg, false) {
                Ok(a) => {
                    let ids = a.db.q("SELECT id FROM tasks", taskboardd::p![]).unwrap_or_default();
                    for t in &ids {
                        let _ = taskboardd::mdcopy::write(&a, t["id"].as_i64().unwrap_or(0));
                    }
                    println!("Wrote {} task files in {}", ids.len(), a.cfg.md_dir().display());
                    0
                }
                Err(e) => {
                    eprintln!("taskboardd: {e}");
                    1
                }
            }
        }
        Cmd::Launchd { label } => launchd(&label),
        Cmd::Config => {
            let mut cfg = load(None, None, false);
            if !cfg.jira.token.is_empty() {
                cfg.jira.token = "(set)".into();
            }
            println!("{cfg:#?}");
            0
        }
    };
    std::process::exit(code);
}
