# The board's JSON API

This is everything Taskboard.app (`crates/taskboard-app`) reads from and sends to `taskboardd`. A field that isn't
listed here isn't used by the app. The app's fake backend (`TASKBOARD_BACKEND=fake`) calls the same handlers
in-process (`taskboardd::api::dispatch`), so it can't drift from these shapes. The agent CLI (`tb`) uses other
routes under the same prefix (see `crates/taskboard-cli`).

## Conventions

- **Base paths.** API at `/tasks/api/…` on `127.0.0.1:8792` (config `port`, `TASKBOARD_PORT`).
- **Requests.** Every GET sends `Accept: application/json` with `cache: no-store`. Every POST sends a JSON body
  (at least `{}`), `Content-Type: application/json` and `X-Task-Board: 1`. A POST without that header gets 403.
- **Errors.** Any non-2xx response should have the body `{"error": "<a sentence for the reader>"}`. The UI shows
  `error` as is (toasts, inline notes, the banner). If it can't parse JSON it says "The board answered <status>."
- **Refs.** Tasks `T12`, goals `G3`, backlog issues `B7`. Path segments are sent as refs (`/tasks/T12`,
  `/goals/G3`, `/backlog/B7`); the server must accept either `T12` or `12`. Body fields named `goal_id` are sent as
  numbers (or `null`). Objects always carry both `id` (number) and `ref` (string) where listed.
- **Times.** ISO 8601 strings. UTC with `Z` is fine everywhere except `work_hours.next_open` (see there). `state.now`
  is used to correct for clock skew between the server and the browser.
- **Booleans** must be real JSON booleans (the UI also tolerates 0/1 for `auto_close` and `run_in_order`).
- **Polling.** The app polls every 3 s: `GET /state` and `GET /goals` always, plus the open
  task, issue, goal, backlog list or sessions pages as below.

---

## GET endpoints

### `GET /state`

Query: `project=<name|all>`, `goal=<goal id number|all>`, `done=24h|7d|all`, and optionally
`keep=B3,B4` (issue refs the board should keep in the Backlog column even if they no longer match, so a card the
reader just changed doesn't vanish from under them).

```
{
  "now": iso,                                   // server clock
  "midna": {"up": bool, "seen_at": iso|null},   // up = a Midna CLI call succeeded in the last ~20 s; seen_at = last success
  "jira": {"enabled": bool},                    // NEW: Jira is configured. false hides every Jira control and label.
  "alerts": [alert],
  "work_hours": work_hours,
  "usage": usage | null,                        // null when no usage reading is known: the pill is hidden
  "accounts": [{"id", "label", "reason", "reauth": bool}],  // accounts needing the owner (missing scopes or a failed check): the amber status-bar pill
  "projects": [{"name": str, "path": str|null}],// every known project (Midna's list + projects on tasks/goals/sessions), sorted by name
  "sessions": [session_row],                    // live (not gone) Claude terminals, filtered by ?project
  "session_projects": [str],                    // sorted project names of all live Claude terminals (unfiltered); seeds the goals rail
  "goals": [goal_summary],                      // non-archived goals in ?project (fallback until GET /goals answers)
  "columns": {
    "backlog": [issue_card],                    // open issues matching the filters, newest first, max 4, plus any ?keep ones
    "backlog_open": int,                        // count of ALL open issues matching the filters
    "queued": [task_card],                      // high priority first, then oldest created first
    "working": [task_card],                     // newest `when` first
    "needs": [task_card],                       // newest `when` first
    "done": [task_card]                         // newest `when` first, only those inside the ?done window (cap ~200)
  },
  "counts": {
    "needs": int,                               // shown in the page title "(3) Task board"
    "open_issues": int,                         // open issues matching the filters (Backlog page header fallback)
    "done_hidden": int,                         // done tasks matching the filters but older than the ?done window
    "queued": int, "working": int               // optional, unused today
  }
}
```

Planned tasks never appear in `columns`; they only show on the goal page. Columns respect `?project` and `?goal`.

#### `alert`
```
{"id": str, "at": iso, "text": str, "task": "T12"|null, "goal": "G3"|null}
```
Something that needs the reader (a task that couldn't start, an answer that didn't arrive…). `task`/`goal` give
the "Open T12" button. Dismissed with `POST /alerts/:id/dismiss`. (The original's `urgent` "Master is red" alerts
are gone.)

#### `work_hours` (from `hours.state`)
```
{
  "on": bool,             // false = no work hours; agents start any time
  "start": "HH:MM",       // 24 h, local time of the server's machine
  "end": "HH:MM",
  "days": ["mon", …],     // three-letter lower-case day names: sun mon tue wed thu fri sat (any order)
  "open": bool,           // agents may start right now (always true when on = false)
  "line": str,            // one plain sentence, used as the pill's tooltip, e.g. "Work hours 6am–3pm: agents start until 3pm"
  "next_open": str|null,  // when the hours next open, null while open. LOCAL time WITHOUT a zone, "2026-10-08T06:00",
                          // because the browser parses it as local time (Python: isoformat(timespec="minutes") of a naive local datetime)
  "today_until": "HH:MM"|null  // today's end overridden by "Today until", null when not set or not today
}
```
The pill reads "Work hours until 4pm today" (open + today_until), "Work hours 6am–3pm" (open), "Agents off until
Thu 6am" (closed, from `next_open`) or "No work hours" (`on` false). `next_open` also drives "Send at <when>" on
answer boxes and "Start tomorrow / Start now" on the goal page. If the 5-hour usage is used up the original also
rewrote `line` ("Out of 5-hour usage: agents start again at …").

#### `usage` (from `usage.state`)
```
{"seen_at": iso, "windows": [{"key": "five_hour"|"seven_day", "label": "5h"|"7d", "pct": int 0-100, "resets_at": iso|null}]}
```
Claude plan usage. `pct` is rounded used percentage (0 once `resets_at` has passed). The pill greys out when
`seen_at` is over an hour old. Omit (null) when nothing is known; an empty `windows` also hides it. A `five_hour`
window at 100 % makes goals show "Queued until agents can start".

#### `session_row` (from `api.session_list`)
```
{
  "id": str,                   // Midna terminal id
  "name": str,                 // never empty: fall back to "Terminal <first 8 of id>"
  "project": str|null,
  "project_path": str|null,
  "status": "idle"|"working"|"needs"|"offline",   // ("gone" never appears in this list)
                               // offline: its last turn ended on a lost connection (StopFailure); needs: on another API error
  "api_error": str|null,       // that error, until the next prompt, turn or session start
  "idle_secs": int|null,       // idle time from last_activity (else seen_at), not counting time the Mac slept
  "task_ref": "T12"|null,      // the task currently on this terminal (status != done)
  "task_title": str|null,
  "last_activity": iso|null,
  "seen_at": iso|null,         // fallback for idle time when last_activity is null
  "can_take": bool,            // idle and no task: offered in New task → "In an idle terminal"
  "branch": str|null,          // git branch Midna reports (shown in the Sessions list subline)
  "closing": bool,             // a close job is pending/running for it
  "close": "close"|"force"|null, // how it can be closed: idle → "close", busy → "force" (press-and-hold), gone → null
  "renaming": str|null,        // a rename job to this name is pending/running (UI shows the new name greyed)
  "rename_error": str|null     // the last rename failed within ~10 min: first line of Midna's error
}
```
The UI also accepts a missing `renaming`/`rename_error`.

### `GET /goals`

No query. Returns `{"goals": [goal_summary]}` for every non-archived goal (all projects). The goals rail, goal
pickers and goal nav use this list. (The UI also accepts a bare array.)

#### `goal_summary` (from `board.goal_dict` + `goal_counts`)
```
{
  "id": int, "ref": "G3", "name": str, "project": str,
  "tldr": str, "outcome": str,              // "" when unset
  "epic_key": str|null, "epic_status": str|null,
  "epic_url": str|null,                     // NEW: browse URL of the epic, built by the server from the Jira site. null without Jira.
  "run_in_order": bool, "max_terminals": int, "auto_close": bool,
  "archived": bool, "paused": bool, "deprioritized": bool,
  "total": int,        // tasks in the goal, planned included
  "done": int,         // tasks with status done (failed included)
  "active": int,       // working + needs
  "needs": int,        // status needs
  "queued": int,       // status queued
  "starting": int,     // queued with a live start job
  "blocked": int,      // queued and waiting on another task (waits_for)
  "planned": int,      // status planned
  "open_issues": int,  // open backlog issues in the goal
  "prs_open": [int],   // PR numbers of done, not-failed tasks whose PR is still open (the goal isn't finished until they merge)
  "finished_at": iso|null,  // latest finished_at once EVERY task is done and prs_open is empty; else null
  "peek": [{"ref": "T12", "title": str, "status": "needs"|"working"|"failed"|"starting"|"blocked"|"queued", "why": str|null}]
            // hover card on the rail: non-planned, not-done tasks plus failed ones; why = the blocker line for blocked ones
}
```
A goal with `finished_at` (or all tasks done and no open PRs) shows as finished; finished goals from the last day
(back to Friday on weekends/Mondays) show under "Recently completed".

### `GET /goals/:id`

`goal_summary` plus:
```
{
  "tasks": [task_card],             // every task in the goal including planned, in goal order (position, then id)
  "notes": [{"id": int, "kind": "finding"|"decision"|"reference", "text": str, "source": str|null, "pinned": bool, "at": iso}],
                                    // source: "you", a task ref like "T4", or free text; shown as the note's byline
  "backlog": [issue],               // the goal's issues that are NOT dropped, newest first (full issue shape, see GET /backlog/:id, history optional)
  "closed_count": int,              // the goal's dropped ("won't do") issues
  "attachments": [attachment]       // attached to the goal itself
}
```
The UI also accepts `{"goal": {...}, ...rest}` and merges them, but a flat object is preferred.

### `GET /tasks/:id`

`task_card` plus:
```
{
  "detail": str,                    // "What to do", plain text with light markdown (lists, indented code)
  "repo_path": str|null,
  "created_at": iso, "updated_at": iso, "started_at": iso|null, "finished_at": iso|null,
  "session": {"id": str, "name": str, "status": "idle"|"working"|"needs"|"gone"} | null,
                                    // the terminal on it now; when the task still names a terminal the board no longer knows, status "gone"
  "terminals": [{"id": str|null, "name": str, "status": "idle"|"working"|"needs"|"gone", "why": str, "at": iso|null}],
                                    // every terminal that worked on it, newest first. why: "Worked on the task" | "Worked on the PR" | free text
  "claude_session_id": str|null,    // enables "Reopen the old conversation"
  "goal": {"id": int, "ref": "G3", "name": str, "position": int, "total": int, "next_title": str|null, "open_issues": int} | null,
                                    // NOTE: replaces task_card.goal here. position is 1-based within the goal; next_title = the next task's title
  "blocked_by": [task_card],        // tasks it waits for that aren't done (waits_for); uses ref, title, status, failed, blocked, starting, pr
  "meta": [[name, value]],          // "Details" fields (strings)
  "context": {
    "where": {"branch": str, "worktree": str, "last_commit": str, "uncommitted": int|str, "conversation": str},  // all optional
    "done": [str], "next": [str], "decisions": [str], "answers": [str], "files": [str],                          // all optional
    "saved_at": iso, "turns": int, "checkpoints": int                                                              // optional
  },
  "log": [{"at": iso, "who": str, "kind": str, "text": str}],   // every event, newest first.
                                    // kind: status | note | turn | checkpoint | question | answer | jira | found | commit | midna | handoff
  "handoff": str,                   // the full handoff text a new terminal gets (or omit it and the UI calls GET /tasks/:id/handoff)
  "found": [{"id": int, "ref": "B7", "title": str}],   // issues this task reported
  "attachments": [attachment],      // on the task
  "goal_attachments": [attachment]  // on its goal (omit or [] when no goal)
}
```

### `GET /tasks/:id/handoff`

`{"text": str}`. Only called when `GET /tasks/:id` has no `handoff` string and the Context tab is open.

### `GET /backlog`

Query: `project=<name|all>`, `goal=all|none|<goal id number>`, `kind=all|bug|gap|follow|clean`,
`state=open|task|ticket|drop|all`, `sort=new|old|kind` (`kind` = by type in the order bug, gap, follow, clean,
newest first within a type).

`{"issues": [issue_card + {"said", "how", "detail"}], "total": int}`. `total` is shown as "Showing N of <total>". The
original returned the count of every issue regardless of filters; the UI only needs it to be ≥ the list length,
so the matched count is fine (the mock does that). The page also calls it once with the default filters
(`project=all&goal=all&kind=all&state=open&sort=new`) to show the "<n> open" pill.

### `GET /backlog/:id`

The full `issue`:
```
issue_card + {
  "detail": str|null,
  "said": str|null,        // what the reporter said, already in quotes when the server wraps it (“…”)
  "how": str|null,         // one sentence: how it was reported ("The API auth terminal reported it while working on T4.")
  "snapshot": {str: any},  // what was happening; keys the UI labels nicely: from task step branch worktree last_commit commit
                           // uncommitted file last_turn output cwd (others are shown with "_" → " "). {} when none.
  "found_by_task": {"id": int, "ref": "T4", "title": str} | null,   // replaces the card's plain id
  "history": [{"at": iso, "who": str|null, "kind": str, "text": str}]
                           // oldest first; kind: report | seen | note | source | ticket | task | move | drop | lost
}
```

#### `issue_card` (from `board.issue_card`)
```
{
  "id": int, "ref": "B7", "kind": "bug"|"gap"|"follow"|"clean",   // labels: Bug, Test gap, Follow-up, Clean-up
  "title": str,
  "goal": {"id": int, "ref": "G3", "name": str} | null,
  "goal_id": int|null,           // optional; the UI uses goal_id, else goal.id
  "project": str,
  "state": "open"|"task"|"ticket"|"drop",   // task = made into a task; ticket = sent to Jira; drop = won't do
  "task_id": int|null,           // the task it became (state task)
  "jira_key": str|null,          // the Jira ticket (state ticket); null while the ticket is being created
  "source": "terminal"|"you"|"answer",
  "found_by_name": str|null,     // terminal name that reported it (or the owner's name for source you)
  "found_by_task": int|null,     // on cards a plain task id is fine; the full issue has an object
  "created_at": iso, "updated_at": iso
}
```

### `GET /sessions`

Query: `project=all`. Returns `{"sessions": [session_row]}` (all live Claude terminals). Used by the Sessions page.

### `GET /sessions/closed`

Query: `project=all`. Returns `{"sessions": [closed_row], "total": int}` (total = all closed rows before any limit;
the UI shows up to 50).
```
closed_row = {"id": str, "name": str, "project": str|null, "project_path": str|null, "closed_at": iso,
              "last_activity": iso|null, "task_ref": "T12"|null, "task_title": str|null,
              "can_reopen": bool}   // the board knows its Claude conversation id and its folder still exists
```

### `GET /sessions/:id`

For a live or a gone terminal:
```
{
  "id": str, "name": str, "project": str|null, "project_path": str|null,
  "status": "idle"|"working"|"needs"|"offline"|"gone", "api_error": str|null, "idle_secs": int|null,   // as session_row
  "status_at": iso|null,         // when it entered this status ("Working for 12 min")
  "last_activity": iso|null, "gone_at": iso|null,
  "branch": str|null,
  "dirty": int|null,             // uncommitted file count; only used when diff is null
  "diff": {"files": int, "added": int, "removed": int, "new": int} | null,   // git diff vs HEAD incl. untracked; null for gone terminals
  "close": "close"|"force"|null, "closing": bool, "renaming": str|null, "rename_error": str|null,   // as session_row
  "task": task_card | null,      // the task on it now
  "last_task": {"ref": "T12", "title": str} | null,   // when no task now: the last one it worked on
  "prompt": {"text": str, "at": iso} | null,   // last prompt (Claude Code tags stripped)
  "reply": {"text": str, "at": iso} | null,    // last reply (markdown; rendered with a small markdown subset)
  "waiting": {"text": str, "at": iso} | null,  // what it's waiting on, only while status is needs
  "stats": {"turns": int, "commits": int, "files": int},   // REQUIRED (the UI reads stats.turns directly)
  "timeline": [{"at": iso, "kind": str, "text": str, "files": int?}]   // REQUIRED array, newest first, ≤ ~80
       // kind: prompt reply turn commit checkpoint found ask wait compact start end take done fail rename close closed close_failed
}
```

### `GET /accounts`

Query: `fresh=1` asks each service again about its token (otherwise the last check is used). Returns
`{"accounts": [account]}` for github, bitbucket and slack, in that order. Never a token.

Every token is Taskboard's own, in the login Keychain (`taskboard-github`, `taskboard-bitbucket`,
`taskboard-slack`). Nothing here changes the Mac's `gh` sign-in, `~/.gitconfig` or git's credential store.
```
account = {"id": "github"|"bitbucket"|"slack", "label": str, "connected": bool,
           "user": str|null, "name": str|null, "detail": str|null,   // "@sam · sam@acme.com", "Acme · acme.slack.com"
           "scopes": [str], "checked_at": iso|null,
           "required": [str],      // the scopes Taskboard needs (accounts::required_scopes)
           "missing": [str],       // required scopes the token lacks; [] when the service lists none (fine-grained GitHub token)
           "reauth": bool,         // missing isn't empty: the app asks the owner to sign in again
           "error": str|null,      // why the last check or sign-in failed
           "login": {"code": "ABCD-1234", "url": str}|null,   // github: a browser sign-in waiting for the code
           "gh": bool, "setup": str|null}                    // github: false + "brew install gh" when gh is missing (only the browser sign-in and import need it)
```

---

## Shared objects

### `task_card` (from `board.task_card`)
```
{
  "id": int, "ref": "T12", "title": str, "project": str,
  "status": "planned"|"queued"|"working"|"needs"|"done",
  "priority": "normal"|"high",
  "failed": bool,                 // done but failed (status stays "done")
  "lost": bool,                   // its terminal ended before it was done (status "needs")
  "needs_reason": "question"|"attention"|"lost"|"start_failed"|null,
                                  // start_failed = Midna couldn't start it after the retries → "Couldn't start in Midna" box
  "question": str|null,           // the agent's question while it needs you
  "latest": str|null,             // one-line latest update
  "summary": str|null,            // the done/fail summary
  "when": iso,                    // done: finished_at; needs/working: updated_at; queued/planned: created_at
  "updated_at": iso,              // optional fallback for when
  "started_at": iso|null,         // "running 2h 5m" on working/needs tasks
  "finished_at": iso|null,        // optional on cards; "Took 1h" on done tasks in the panel
  "who": str|null,                // name of the terminal on it (or that last worked on it)
  "session_id": str|null,
  "goal": {"id": int, "ref": "G3", "name": str} | null,
  "jira": {"key": str|null, "status": str|null, "url": str|null} | null,
          // null = no ticket. key null + status "Ticket asked for" = being created. url = the ticket's browse URL, built
          // by the server from the configured Jira site (the UI never builds Jira URLs).
  "pr": pr | null,
  "position": number|null,        // order in the goal (unused by the UI apart from sorting done server-side)
  "starting": bool,               // queued and a start job is pending/running ("Starting")
  "waiting": str|null,            // queued only: why it isn't starting yet, one plain line
                                  // ("Waits for T4 to finish", "Waits for work hours (tomorrow 6am)", "Waits for the 5-hour usage to reset (3pm)")
  "blocked": bool                 // queued and waiting on another task (waits_for), shown as "Blocked"
}
```
The card is draggable to Working when it's queued/planned, not in a goal and not starting (drop = start with mode `new`).

### `pr`
```
{
  "repo": str,                   // short repo name shown under the PR title
  "num": int,
  "url": str,                    // the PR's web URL (GitHub/GitLab/Bitbucket); the UI links only to this
  "title": str|null,             // PR title (a leading Jira key is trimmed off for display)
  "state": "OPEN"|"MERGED"|"DECLINED",   // upper case; anything not OPEN/MERGED shows as declined/closed
  "checks": "pass"|"fail"|"pending"|"none"|null,   // CI status of the head. none = no checks configured/expected.
  "review": "approved"|"changes"|"pending"|"none"|null,   // approved = enough approvals; changes = changes requested;
                                 // pending = reviewers asked, nobody has decided; none = no review asked yet
  "stage": {
    "phase": "checks"|"fix"|"review"|"comments"|"merge"|"merged"|"declined",
    "label": str,                // plain words, e.g. "Watching checks", "Fixing checks", "Awaiting reviews", "Answering comments", "Merging", "Merged"
    "session": str|null,         // optional: id of the terminal the board woke for fix/comments/merge (links to it)
    "stopped": {"asked": bool, "message": str} | null
                                 // optional: that terminal stopped before finishing (asked = it asked you a question).
                                 // Shows "Needs you" and an answer box on the done task; the answer goes through POST /tasks/:id/answer.
  } | null
}
```
The task panel shows three steps (Checks, Review, Merge) from `checks`, `review`, `state` and `stage.phase`. A done
task whose PR is still OPEN shows "Awaiting merge" (or `stage.label`) instead of Done. The original's `build`,
`review` free text, `review_log`, reviewer lists, `build_url`, `new_comments`, `not_ours`, `awaiting_you`,
`reviewed_at` and `asked` are gone.

### `attachment` (from `board.attachment_dict`)
```
{"id": int, "kind": "design"|"proposal"|"doc"|"evidence"|"results"|"other", "title": str, "url": str,
 "at": iso, "task": "T12"|null, "goal": "G3"|null, "added_by": str?}
```
`url` is an `https://` link or an absolute/`~` file path. For a path, the UI links to `/tasks/files/:id`, which the
server serves (the original served the file inline with a sandboxing CSP). The UI can edit and remove attachments
but not add them (agents add them with `tb`).

---

## POST endpoints

Unless noted, the UI ignores the response body (it re-polls), so any 2xx JSON works. The Python server returned the
updated task detail for task routes, the goal detail for goal routes and the issue for backlog routes.

### Tasks
| Path | Body | Notes |
|---|---|---|
| `POST /tasks` | `{title, detail, project, priority: "normal"\|"high", goal_id: int\|null, auto_close: bool, pickup: {mode: "queue"\|"new"\|"attach"\|"manual", session_id?}, status?: "planned", jira?: {mode: "create"\|"link"\|"none", key?}}` | New task form. `status: "planned"` only when it has a goal ("Add it to the goal's plan"). `jira` only sent when `state.jira.enabled`. **Response read:** the task (`ref` or `id`), then the UI opens it. |
| `POST /tasks/:id` | `{status: "queued"}` | "Queue it now" on a planned task (planned → queued only). |
| `POST /tasks/:id/start` | `{mode: "new"\|"queue"}` | Start / Start when the repo's free / New Midna terminal / Queue in Midna / drag to Working (`new`). |
| `POST /tasks/:id/answer` | `{text, when: "now"\|"morning"}` | `morning` = hold it until work hours open ("Send at <when>"). Also answers a stopped PR visit. |
| `POST /tasks/:id/resume` | `{mode: "fresh"\|"reopen"}` | Lost terminal: new terminal with the handoff, or `--resume` the old conversation. |
| `POST /tasks/:id/requeue` | `{}` | Try again (failed) / Queue again (done). |
| `POST /tasks/:id/detach` | `{}` | Take it off its terminal; back to queued. |
| `POST /tasks/:id/done` | `{summary?}` | Your manual "Mark done". |
| `POST /tasks/:id/fail` | `{reason?}` | Your manual "Mark failed". |
| `POST /tasks/:id/close-terminal` | `{force: bool}` | `force` false from the done box; true from Manage when the terminal is busy. |
| `POST /tasks/:id/focus` | `{}` | Bring its terminal to the front in Midna. |
| `POST /tasks/:id/meta` | `{meta: [[name, value], …]}` | Edit/remove Details fields. |
| `POST /done/close-terminals` | `{}` | Done column menu: close every done task's still-open terminal (no force). |

### Goals
| Path | Body | Notes |
|---|---|---|
| `POST /goals` | `{name, tldr, outcome, project, run_in_order: bool, max_terminals: int, auto_close: bool, epic: {mode: "create"\|"link"\|"none", key?}}` | New goal. `epic.mode` is always `none` without Jira. **Response read:** the goal (`ref`/`id`, or `{goal: {...}}`); the UI navigates to it. |
| `POST /goals/:id` | any subset of `{name, tldr, outcome, project, run_in_order, max_terminals, auto_close, epic_key: str\|null, paused: bool, deprioritized: bool}` | Edit goal form (`epic_key` only with Jira), the goal page's "How this goal runs" (`max_terminals`, `run_in_order`, `auto_close`), Pause/Resume, Deprioritize/Bring it back. |
| `POST /goals/:id/run` | `{}` or `{now: true}` | Queue the planned tasks (and re-queue `start_failed` ones), clear paused/deprioritized. `now: true` outside work hours = let this goal run until the hours next open (`goals.hours_until`). **Response read:** `queued_now: int` (how many planned tasks it queued). |
| `POST /goals/:id/plan` | `{mode: "edit"}` | "Plan in Claude": open a Claude terminal in Midna on the goal's plan (no prompt; goal context as system prompt). |
| `POST /goals/:id/notes` | `{kind: "finding"\|"decision"\|"reference", text, source: "you"}` | Add a goal note. |

### Backlog
| Path | Body | Notes |
|---|---|---|
| `POST /backlog` | `{title, kind, goal_id: int\|null, project, said?, detail?}` | Add an issue (source `you`). **Response read:** the issue (`ref`, or `{issue: {...}}`). |
| `POST /backlog/:id/promote` | `{where: "board"\|"goal"}` | Make it a task: `board` = queued task; `goal` = planned task at the end of its goal. **Response read:** `{task: {ref\|id}}` (or `task_id`) so the board opens the new task. |
| `POST /backlog/:id/ticket` | `{}` | Create a Jira ticket for it (only offered with Jira). |
| `POST /backlog/:id/drop` | `{}` | Won't do. |
| `POST /backlog/:id/move` | `{goal_id: int\|null}` | Move to another goal or none. |
| `POST /backlog/:id/note` | `{text}` | Add a note to its history. |
| `POST /backlog/bulk` | `{ids: ["B1", …], action: "task"\|"ticket"\|"drop"\|"move", where?: "goal", goal_id?: int\|null}` | Goal page bulk bar. `task` sends `where: "goal"`; `move` sends `goal_id`. **Response read:** `count: int`. |

### Attachments
| Path | Body | Notes |
|---|---|---|
| `POST /attachments/:id` | any of `{title, url, kind}` | Edit. |
| `POST /attachments/:id/remove` | `{}` | Remove. |

### Work hours, alerts
| Path | Body | Notes |
|---|---|---|
| `POST /hours` | `{on: bool, start: "HH:MM", end: "HH:MM", days: ["mon", …], today_until?: "HH:MM"\|"off"}` | Sent on every change in the hours menu. `today_until` only when it changed (`off` clears it). **Response read:** the new `work_hours` object (replaces `state.work_hours` at once). Errors (e.g. "4pm has already passed today.") show in the menu. |
| `POST /alerts/:id/dismiss` | `{}` | |

### Accounts
Every one answers with `GET /accounts`'s `{"accounts": […]}`.

| Path | Body | Notes |
|---|---|---|
| `POST /accounts/github/login` | `{}` | Starts `gh auth login --web --insecure-storage --scopes <required>` in a throwaway `GH_CONFIG_DIR` (gh's own sign-in is untouched); answers once gh has shown its one-time code (`login.code`). When it finishes, the token moves into `taskboard-github` and the folder is deleted. Poll `GET /accounts` until `login` is null. |
| `POST /accounts/github/cancel` | `{}` | Stops a browser sign-in. |
| `POST /accounts/github/import` | `{}` | Copies the token `gh auth token` prints into `taskboard-github` (read only; later gh changes don't follow). 400 when gh isn't signed in. |
| `POST /accounts/:id` | `{token}` (bitbucket also `{email}`) | Checks the token with the service, then keeps it in the Keychain. 400 with the reason when it's refused. |
| `POST /accounts/:id/check` | `{}` | Asks the service again whether the token works and what scopes it has. |
| `POST /accounts/:id/disconnect` | `{}` | Forgets Taskboard's token. Nothing else on the Mac changes. |

### Sessions (Midna terminals)
| Path | Body | Notes |
|---|---|---|
| `POST /sessions/:id/rename` | `{name}` | ≤ 80 chars. The UI then expects `renaming` on the session until Midna confirms. |
| `POST /sessions/:id/focus` | `{}` | |
| `POST /sessions/:id/close` | `{force: bool}` | `force` from the press-and-hold Force close. |
| `POST /sessions/close` | `{ids: [str]}` | Close several idle terminals with no task. **Response read:** `{closing: [id], skipped: [id]}`. |
| `POST /sessions/:id/reopen` | `{}` | Reopen a closed terminal's conversation in a new Midna terminal. |

---
