# taskboard — design of the Rust port

This is a Rust rewrite of a personal Python "task board" (the original archive is not part of this repo).
The original was tied to one person and one employer: their Jira site and teams,
Bitbucket, Azure DevOps builds, a Slack workspace, a private "Review log" service, emulator devices and
a feature-flag site. This port keeps the general idea and drops or generalizes everything else.

## What it is

A native macOS app (GPUI) over a local daemon (API at `http://127.0.0.1:8792/tasks/api`) that tracks every Claude Code session running
in the Midna terminal app, and holds the work queued for them: tasks, goals (ordered groups of tasks) and
a backlog of issues the agents found along the way. It starts Claude terminals for queued tasks, hands each
one a "handoff" prompt, and the Claude plugin's hooks and the `tb` CLI report progress back.

## Pieces

| Piece | Where |
|---|---|
| Daemon (`taskboardd serve`) | `crates/taskboardd` — axum HTTP server, SQLite (rusqlite), background threads for the runner, Midna sync and jobs. Lib + bin, like Midna's `midnad`. |
| Agent CLI (`tb`, installed as `tb`) and hooks (`tb hook <Event>`, `tb statusline`) | `crates/taskboard-cli` |
| Mac app | `crates/taskboard-app` — GPUI via `gpui-kit` 0.7, a client of the daemon's JSON API (`backend.rs`). `TASKBOARD_BACKEND=fake` runs the board in-process on sample data. |
| Packaging | `packaging/build-app.sh` → `Taskboard.app` (app + daemon + tb; the daemon's LaunchAgent is registered with SMAppService on first launch); `scripts/dev-app.sh` → "Taskboard Dev" |
| Claude plugin | `plugin/` — marketplace + `task-board` plugin whose hooks call `tb hook …` |
| Review profiles | `review-profiles/` — generic code-review reviewer profiles |

## Kept (generalized)

- Tasks, goals, goal notes, backlog issues with history, attachments (links), task log, task context, handoff.
- Sessions synced from Midna (`midna call session.list` / `project.list`), closed-session list and reopen.
- Jobs to Midna: agent, message, close, focus, rename; deliveries to a terminal through hooks or Midna.
- Runner: starts queued tasks (pickup queue/new/attach/manual), goal rules (run in order, max terminals),
  `waits_for` dependencies between tasks, work hours (and "today until"), out-of-5-hour-usage pause,
  lost terminals, spool of offline reports, delivery nudges, alerts.
- Report events from hooks (`hook.*`) and `tb` (`tb.*`).
- Question screening (optional): a headless `claude -p` decides whether the owner's own rules already
  answer an agent's question.
- PRs: any GitHub / GitLab / Bitbucket PR URL is recognized. For GitHub, the board reads state, checks and
  reviews with the `gh` CLI and works out a stage (checks, fix, review, comments, merge, merged, declined),
  waking the task's conversation when there's something to do (optional).
- Jira (optional): REST API only, configured in `config.toml` (site, project, email, token, transitions,
  labels/components per "product"). Off when not configured.
- Waves, with the owner's review stop (set only from the app) and holds (`tb goal wave --hold`).
- A device pool (`devices.rs`): named devices with tags, asked for by tag, lent by the runner, named in the
  handoff, an optional focus command each (`[devices]`).
- Bits (`bits.rs`): feature flags, local or backend; a task waits on its unmade backend bits, a done goal too
  (`[bits]` names the flag tool and its new-flag link).

## Dropped

WD review gate, master-build breaks and Azure DevOps,
stopping PR builds, Slack reviewer nudges and reviewer cycling, the private Review log service, the Jira
desk terminal (replaced by REST), code-comment guard hooks, compact-before-resume guard.

## Generalization rules

- No personal names. The owner's name comes from config `owner` (default: first name from
  `git config user.name`, else "the owner"). UI text speaks to the reader as "you".
- No hard-coded hosts, paths, Jira ids, teams or repos. Everything is in `config.toml` or env.
- Data defaults to `~/.config/taskboard/` (`tasks.db`, `tasks/T<n>.md`, `spool/`, `statusline/`, `server.log`).

## Config

`~/.config/taskboard/config.toml` (or `TASKBOARD_CONFIG`), every key optional; env vars override:
`TASKBOARD_PORT`, `TASKBOARD_DATA`, `TASKBOARD_URL`, `TASKBOARD_MIDNA`, `TASKBOARD_CLAUDE`,
`TASKBOARD_HOOK_TIMEOUT`, `TASKBOARD_RUNNER`, `TASKBOARD_JIRA_EMAIL`, `TASKBOARD_JIRA_TOKEN`. See
`config.example.toml`.

## HTTP

- API at `/tasks/api/…` (documented in `docs/API.md`); the native app and `tb` are its clients.
- Every POST needs `X-Task-Board: 1` (CSRF), else 403. Errors are `{"error": "<sentence>"}`.
- Refs: `T12` task, `G3` goal, `B7` backlog issue, `J5` job. The API accepts either form.
