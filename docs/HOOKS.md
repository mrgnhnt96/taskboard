# Hooks

Your own commands, run by `taskboardd` at each step of the board's flow. A hook can stop or skip a step, unless
the step says why it can't. They're shaped like Claude Code's
hooks: a JSON file maps an event to matcher groups, and each group lists commands. Use them for anything too
specific to build into the board (cancelling CI builds, posting to a channel, a check before a task may finish…).

## The flow

```
task.created ─▶ task.queued ─▶ [task.starting] ─▶ task.working ⇄ task.needs
                                                       │
                                                       ├─▶ task.failed
                                                       └─▶ [task.finishing] ─▶ task.done ─▶ pr.opened
                                                                                              │
     ┌────────────────────────────────────────────────────────────────────────────────────────┘
     ▼
[pr.checks] ⇄ [pr.fix] ─▶ [pr.review] ⇄ [pr.comments] ─▶ [pr.merge] ─▶ pr.merged
                                                                    └▶ pr.declined

goal.created ─▶ goal.paused ⇄ goal.resumed ─▶ goal.finished        goal.archived
```

There are two kinds of event:

- **Steps** (in `[brackets]`): the board is about to do something. Their hooks run *just before* it, and a hook can
  **stop** the step or **skip** it, unless the step says why it can't (below).
- **Announcements** (the rest): something already happened. Their hooks run afterwards and can't change anything.

| Step             | When                                                                       | Skip means                              | Can't                |
|------------------|----------------------------------------------------------------------------|-----------------------------------------|----------------------|
| `task.starting`  | The board is about to open a terminal (on its own, or Start / Resume).     | The task is marked done without running |                      |
| `task.finishing` | The task is about to be marked done (`tb done`, or Done in the app).        |                                         | skip: finishing is the last step before done; let it carry on instead |
| `pr.checks`      | The PR's checks are running (asked once per push).                          | The checks count as passed              | stop: the checks run on the PR's host, not on the board |
| `pr.fix`         | A check failed; the board is about to bring the agent back to fix it.       | The checks count as passed              |                      |
| `pr.review`      | The PR waits for review; the board is about to tell you.                     | It counts as approved                   |                      |
| `pr.comments`    | Review comments; the board is about to bring the agent back for them.       | They count as answered                  |                      |
| `pr.merge`       | Approved and green; the board is about to bring the agent back to merge, or tell you. |                               | skip: the board can't merge by skipping; merge it (a hook may) and the board sees it merged |

| Announcement     | When                                                                                    |
|------------------|-----------------------------------------------------------------------------------------|
| `task.created`   | A task was added (queued or planned).                                                    |
| `task.queued`    | A task went (back) into the queue: planned → queued, put back after its terminal closed, retried. |
| `task.working`   | An agent is working on it: started, picked up, or carried on after an answer.             |
| `task.needs`     | It needs you: a question, a lost terminal, an API error (`task.needs_reason`).            |
| `task.done`      | It was marked done (skipped tasks too).                                                   |
| `task.failed`    | The agent (or you) gave up on it.                                                         |
| `pr.opened`      | A PR was linked to the task.                                                              |
| `pr.merged`      | Merged.                                                                                   |
| `pr.declined`    | Closed without merging.                                                                   |
| `goal.created`   | A goal was made.                                                                          |
| `goal.paused`    | A goal was paused.                                                                        |
| `goal.resumed`   | A paused goal was resumed.                                                                |
| `goal.archived`  | A goal was archived.                                                                      |
| `goal.finished`  | Every task in the goal is done and every PR is merged or closed.                          |

An event fires once per change: a task already `working` doesn't fire `task.working` again, and a PR step is asked
about once per push (and, for `pr.comments`, per new comment). The `pr.*` steps need PR watching (`[pr] watch`,
GitHub only). When a skip moves a PR on, the step it lands on asks its own hooks straight away.

## `hooks.json`

`~/.config/taskboard/hooks.json` (next to `config.toml`; `TASKBOARD_HOOKS` overrides it). It's read fresh on every
event, so edits apply without restarting the board.

```json
{
  "hooks": {
    "pr.checks": [
      {
        "matcher": "work-app|work-api",
        "hooks": [{ "type": "command", "command": "~/bin/cancel-builds.sh", "timeout": 120 }]
      }
    ],
    "task.needs": [
      { "hooks": [{ "type": "command", "command": "say 'A task needs you'" }] }
    ]
  }
}
```

- **`matcher`**: a regex matched against the whole project name (the task's, or the goal's). Empty, `*` or left
  out matches every project.
- **`command`**: run through `sh -c`. With **`args`** (a list), `command` is the program and runs directly.
- **`timeout`**: seconds before the hook is stopped (default 60).

An event name the board doesn't know is an error. The whole file is then ignored and `server.log` says why, so a
typo doesn't fail silently.

## Stopping and skipping a step

As in Claude Code, a hook on a step decides with its exit code or its output:

| The hook…                                                     | Means                                    |
|---------------------------------------------------------------|------------------------------------------|
| exits 0 (no JSON on stdout)                                    | carry on                                 |
| exits 2                                                        | **stop**; stderr is the reason           |
| exits 0 and prints `{"decision": "block", "reason": "…"}`     | **stop**                                 |
| exits 0 and prints `{"decision": "skip", "reason": "…"}`      | **skip**                                 |
| exits anything else, times out, or can't start                 | carry on; the error is noted on the task |

A step's hooks run in file order and the first stop or skip wins; the rest don't run. A decision the step can't take
(a skip on `task.finishing`, any decision on an announcement) is noted on the task with the step's reason, and the
board carries on. The input's `can` field says what the step allows: `{"block": true, "skip": false}`.

Every stop and skip is said out loud, with the hook and its reason ("Stopped from finishing by hook `check.sh`: the
tests are red"), in the task's history and as below:

| Step             | Stop                                                                     | Skip                                  |
|------------------|--------------------------------------------------------------------------|---------------------------------------|
| `task.starting`  | Started by the board: the task moves to Needs you with the reason, and you get an alert. Your Start / Resume: refused with the reason. Starting it again asks again. | The task is done, with "Skipped by hook `x`: reason" as its summary. No terminal opens. |
| `task.finishing` | The task stays open. `tb done` is refused and the agent reads the reason; Done in the app is refused the same way. | — |
| `pr.checks`      | —                                                                        | This push's checks count as passed ("Checks skipped"); the PR moves on. |
| `pr.fix`         | The agent isn't brought back; you get an alert. The PR stays at "Fixing checks" until the next push. | As `pr.checks`. |
| `pr.review`      | You aren't told it's ready for review.                                    | This push counts as approved; the PR moves on to `pr.merge`. |
| `pr.comments`    | The agent isn't brought back; you get an alert.                           | The comments so far count as answered; the PR moves on. |
| `pr.merge`       | The agent isn't brought back to merge, and there's no "ready to merge" alert; you get an alert with the reason instead. | — |

### Skipping checks you cancelled

Hook `pr.checks` (builds started) and `pr.fix` (cancelled builds usually show up as failed checks):

```sh
#!/bin/sh
# ~/bin/cancel-builds.sh
input=$(cat)
pr=$(echo "$input" | jq -r .task.pr.url)
my-ci cancel --pr "$pr" >&2 || exit 1       # a failure carries on as normal
echo '{"decision": "skip", "reason": "Cancelled the CI builds"}'
```

```json
{
  "hooks": {
    "pr.checks": [{ "matcher": "work-app", "hooks": [{ "type": "command", "command": "~/bin/cancel-builds.sh" }] }],
    "pr.fix":    [{ "matcher": "work-app", "hooks": [{ "type": "command", "command": "~/bin/cancel-builds.sh" }] }]
  }
}
```

A skip covers one push; the next push asks `pr.checks` again. From any other hook or by hand, `tb pr skip-checks
[T12] --reason "…"` does the same for the current push, and `--all` for every push from now on.

## What a hook gets

The event as JSON on stdin:

```json
{
  "event": "pr.fix",
  "at": "2026-10-08T15:04:05Z",
  "board_url": "http://127.0.0.1:8792",
  "tb": "/Applications/Taskboard.app/…/bin/tb",
  "can": { "block": true, "skip": true },
  "from": null,
  "task": {
    "id": 12, "ref": "T12", "title": "…", "detail": "…", "project": "work-app",
    "repo_path": "/Users/me/work-app", "branch": "feature/login",
    "status": "done", "needs_reason": null, "question": null, "failed": false, "summary": "…",
    "priority": "normal", "goal": "G3", "session_id": "…", "session_name": "…", "jira_key": "APP-41",
    "pr": { "host": "github", "repo": "acme/work-app", "num": 9, "url": "https://…", "phase": "checks", "state": "OPEN" }
  },
  "head": "4be1…", "checks": [{ "name": "ci", "state": "failed" }], "failed_checks": ["ci"]
}
```

- `from`: the task before the change, for announced task steps (`null` otherwise).
- `task.pr`: `null` until a PR is linked.
- `pr.*` steps add `head`, `checks`, `failed_checks` and `comments`.
- `task.finishing` adds `by` and `done: {"summary", "pr"}` (what the agent passed to `tb done`).
- Goal events have `goal` (as `GET /goals` returns it) and `task` (the task whose change finished it, else `null`).

The common bits are also in the environment: `TASKBOARD_EVENT`, `TASKBOARD_TASK`, `TASKBOARD_GOAL`,
`TASKBOARD_PROJECT`, `TASKBOARD_REPO`, `TASKBOARD_BRANCH`, `TASKBOARD_PR_URL`, `TASKBOARD_URL`. The hook runs in
the task's (or goal's) repo when there is one. It can call `tb` (`tb note --task "$TASKBOARD_TASK" "…"`) and `tb
api` / `tb token` for a connected account.

## How they run

- **Announcements** run one at a time on the board's own `hooks` thread, in the order the steps happened. A slow
  hook delays the hooks after it, not the board.
- **Steps** run on the thread taking the step (the runner, the PR watcher, the request), outside any
  database transaction, so the hook can call `tb`. The step waits for them: keep them quick, and set a `timeout`.
- Every run goes to `~/.config/taskboard/hooks.log` (JSON lines with the decision, output clipped to 4000
  characters) and `server.log`. A hook that fails is also noted in the task's history.

## `tb hooks`

```
tb hooks                               # the flow's events, what each can't do, and the hooks on each
tb hooks test pr.fix --task T12        # run an event's hooks now against a task ("test": true in the input)
tb hooks log [-n 20]                   # the latest runs and their decisions
tb pr skip-checks [T12] [--all] [--reason "…"]
```

`tb hooks test` asks the board for the input and runs the hooks in your shell, printing their output and
decision. It doesn't take the step.
