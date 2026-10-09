# taskboard

A native macOS app (Rust + GPUI) that is a task board for Claude Code sessions running in the Midna terminal app.

It shows what every Claude terminal is doing, and holds the work queued for them: tasks, goals (ordered
groups of tasks) and a backlog of issues the agents found along the way. It starts Claude terminals for
queued tasks, gives each one a *handoff* (everything a fresh terminal needs to pick the task up), and a
Claude Code plugin reports turns, commits, checkpoints, questions and results back to the board. When a
terminal closes before its task is done, the next one carries on from the saved context.

It's a Rust rewrite of a personal Python tool, generalized: no names, hosts, Jira ids or repos are built
in. Everything specific lives in `~/.config/taskboard/config.toml`.

## Pieces

It's built like Midna: a daemon that owns the work, a CLI for agents, and a GPUI app that is only a client.

| Piece | What it is |
|---|---|
| `crates/taskboardd` | The daemon: SQLite, the JSON API on `http://127.0.0.1:8792/tasks/api`, the runner that starts terminals, and the Midna sync. The app runs it as a LaunchAgent, so it keeps working while the window is closed. |
| `crates/taskboard-cli` | `tb`: the agent CLI (`tb` inside Claude sessions), the Claude Code hooks (`tb hook <Event>`) and the status line (`tb statusline`). |
| `crates/taskboard-app` | `Taskboard.app`: the native window (GPUI via `gpui-kit`): board, goals, backlog, sessions, task and issue panels, forms, work hours. |
| `packaging/`, `scripts/` | `build-app.sh` assembles and signs `Taskboard.app`; `install-app.sh` installs it to /Applications; `dev-app.sh` installs a separate "Taskboard Dev". |
| `docs/API.md` | The JSON API the app (and `tb`) rely on. |
| `docs/CUTOVER.md` | Moving over from the old Python board: stop it, `taskboardd import` its `tasks.db`, swap the plugin, start. |
| `docs/PARITY.md` | The audit of the app against the web board it replaced, with the tests that hold each behaviour. |
| `plugin/` | The Claude Code plugin: hooks, the `bin/tb` shim and the `task-board` skill. |
| `review-profiles/` | Generic code-review reviewer profiles. |

## Install

```sh
scripts/install-app.sh           # build, install to /Applications/Taskboard.app, (re)start it and its daemon
                                 # (first launch registers the daemon as a login item and links ~/.local/bin/tb)
```

Then click **Install hooks** at the bottom right of the window. It installs the `task-board` Claude Code plugin
(hooks, skill, `tb` shim) from the copy inside the app. The status bar shows **● Hooks** when they're current, and an
amber **Reinstall hooks** when they're switched off, load from somewhere else (a repo checkout, an app that's gone), or
are an older version than the app ships (Claude Code keeps running its cached copy until you reinstall after an app
update); the tooltip says which.

If macOS asks, switch Taskboard on in System Settings ▸ General ▸ Login Items (the app shows a banner until you do).
`/Applications/Taskboard.app/Contents/MacOS/taskboardd init` writes a starting config.

Coming from the old board? Follow `docs/CUTOVER.md` first: it holds the same port, and
`taskboardd import` brings its tasks, goals and backlog over with their numbers.

Without the app: `./install.sh` builds and copies `taskboardd` and `tb` into `~/.local/bin`; run
`taskboardd serve` yourself (or `taskboardd launchd` prints a LaunchAgent plist).

Optional: point Claude Code's status line at `~/.local/bin/tb statusline`, so the board knows when
the 5-hour usage runs out (Midna's own usage reading works too). See `plugin/README.md`.

## How it works

- **Tasks** move `planned → queued → working → needs → done`. A queued task starts when its pickup allows it
  (`queue`: once the project's busy terminals settle, `new`: right away, `attach`: in a chosen idle terminal,
  `manual`: when you press Start), its goal allows it (run in order, at most N terminals, not paused), what it
  waits for is done (`tb wait-for T<n>`), and the work hours are open.
- **The runner** opens a Claude terminal in Midna with the handoff as its prompt. The prompt starts with
  `[task-board:T12]`; when the plugin's `UserPromptSubmit` hook reports that prompt, the task is claimed by that
  terminal.
- **Agents report** with `tb`: `checkpoint`, `note`, `found` (an issue outside the task goes to the backlog),
  `question` (the task waits for you; optionally screened first against your own rules with a headless
  `claude -p`), `wait-for`, `done`, `fail`.
- **Your answers** reach the terminal through its next hook (or Midna types them in once it's idle at its
  prompt). If its terminal is gone, the board starts a new one with the answer in the handoff.
- **Lost terminals**: a task whose terminal closes goes back in the queue and restarts from its handoff
  (twice an hour at most), then waits for you.
- **Pull requests**: any GitHub, GitLab or Bitbucket PR link an agent reports is linked to its task. GitHub
  PRs are watched with `gh`: failed checks or new review comments bring the task's conversation back
  (`--resume`) with what to do; an approved, green PR is flagged for you (or merged by the agent if
  `pr.agents_merge` is on).
- **Accounts**: Settings (⌘,) ▸ Accounts signs in to GitHub (through `gh`: a browser code or a token), Bitbucket (an
  Atlassian API token with Bitbucket scopes) and Slack (an app token). Tokens are checked, then kept in the Keychain,
  and `git push` over HTTPS is set up for both hosts. Agents use `gh`, `tb api bitbucket|slack <path>` and
  `tb token <provider>` to comment, push and assign reviewers.
- **Jira** (optional): REST API with your token. Tickets for tasks and backlog issues, epics for goals, and
  status moves as work starts, reaches review and merges.
- **Days** (⌘4): any day's timeline (each project's tasks, commits, PRs, questions and how many terminals ran),
  its week next to the week before, how long tasks took, how long they waited on you, and hours saved against the
  agents' own `tb done --human` estimates. Settings ▸ History sets how long it's kept: every event for 90 days, then
  a small summary per day for a year (cleaned up nightly).
- **Work hours** and **usage**: outside the hours (or with the 5-hour usage used up) nothing new starts, and
  idle task terminals are closed so they don't sit on a stale conversation; they restart from their handoff.
- Every task also has a plain-text copy at `~/.config/taskboard/tasks/T<n>.md`.

## Configuration

`taskboardd init` writes `~/.config/taskboard/config.toml` from `config.example.toml`. Every key is optional.
The common ones:

```toml
owner = "Alex"            # who agents ask; defaults to your git user.name
[work_hours]
on = true
start = "09:00"
end = "17:00"
[pr]
agents_merge = false
[jira]
site = "acme.atlassian.net"
project = "PROJ"
email = "alex@acme.com"   # token: TASKBOARD_JIRA_TOKEN or the Keychain item "taskboard-jira"
```

## Developing

```sh
cargo test --workspace                                       # unit, end-to-end and web-parity tests
crates/taskboard-app/parity/regen.sh                         # regenerate the parity goldens from the frozen web UI (node)
TASKBOARD_BACKEND=fake cargo run -p taskboard-app            # the app on an in-process sample board (no daemon, no Midna)
taskboardd seed --data /tmp/tb-dev && taskboardd serve --data /tmp/tb-dev --port 18792
TASKBOARD_URL=http://127.0.0.1:18792 cargo run -p taskboard-app   # the app against that daemon
scripts/dev-app.sh --seed                                    # install "Taskboard Dev" (own bundle id, daemon, port 18792, data)
```

A board on any data folder but the default runs without its runner (it never opens or closes terminals,
moves tickets or sends notifications) unless you pass `--runner`. Screenshots without a screen:
`cargo build -p taskboard-app --features snapshot`, then run with `TASKBOARD_BACKEND=fake TASKBOARD_SNAPSHOT=out.png`
(`TASKBOARD_PAGE=backlog|sessions|G1`, `TASKBOARD_OPEN=T3|B1`, `TASKBOARD_THEME=dark`).

## Removing it

1. `claude plugin uninstall task-board@taskboard` and `claude plugin marketplace remove taskboard`.
2. `/Applications/Taskboard.app/Contents/MacOS/taskboard-app --uninstall` (stops and unregisters the daemon, removes the `tb` link), then delete the app.
3. If you don't want the data, delete `~/.config/taskboard`.
