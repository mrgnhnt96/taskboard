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
  "master": [break],                            // open master breaks (GET /master): the "Master is red" banner lines
  "pr_builds": {"stopped", "by", "at", "reason", …},  // PR builds stopped (GET /pr-builds): the "PR builds stopped" pill
  "pr_feed": {"on", "healthy", "problem", "why", …},  // the PR feed's health (GET /prs/feed); the app shows a pill while it's unhealthy
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
{"id": str, "at": iso, "text": str, "task": "T12"|null, "goal": "G3"|null, "urgent"?: true, "review"?: true, "key"?: str,
 "snoozed_until"?: iso}
```
Something that needs the reader (a task that couldn't start, an answer that didn't arrive…). `task`/`goal` give
the "Open T12" button. Dismissed with `POST /alerts/:id/dismiss`, except an alert with `"review": true` (a PR waiting
for your review): it can't be dismissed (409), still snoozes, isn't replaced by other alerts for its task or pushed out
by the 20-alert cap, and clears once the PR is reviewed ("I reviewed it"). An `"urgent": true` alert (raised with
`POST /alerts`, `tb alert raise --urgent`) stays the same way, comes first in `state.alerts`, and keeps repeating
outside the work hours; it clears when what raised it clears it (`POST /alerts/:key/clear`) or its task moves on,
never with the rest of its task's alerts. Raising it leaves the task's other alerts up; while it's up, new plain
alerts for that task aren't raised (`POST /alerts` answers 409), though a PR-review alert still is. The app shows
each urgent alert first, in a red row of its own; only the other alerts fold into "N tasks need your attention."

Each alert's desktop notification (Midna `notify.send`) carries the id `taskboard-alert-<alert id>` and the snooze
buttons from config.toml's `[alerts] snooze_mins` ("Snooze 15 min", "Snooze 30 min", "Snooze 1 hour" by default).
The board keeps one waiter per alert on the owner's pick (`notify.response`, 600 s at a time) for as long as the
alert is up, so a late Snooze still counts; each notification's pick counts once (`answered` on the alert holds the
`notified_at` it answered), and a repeat moves the same waiter on to the new notification. A snooze button snoozes
the alert; a click opens the app on it. When an alert clears (dismissed, resolved, pushed out) its notification is withdrawn (`notify.withdraw`).

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
  "today_until": "HH:MM"|null, // today's end overridden by "Today until", null when not set or not today
  "week_days": ["sun", …]      // all seven days in week order, from config.toml's first_weekday (Sunday by default);
                               // the hours menu lays its day buttons out in this order
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
  "compacting": iso|null,      // compacting its conversation since then: from PreCompact to its next hook (SessionStart "compact")
  "task_ref": "T12"|null,      // the task currently on this terminal (status != done)
  "task_title": str|null,
  "last_activity": iso|null,
  "seen_at": iso|null,         // fallback for idle time when last_activity is null
  "can_take": bool,            // idle, no task and nothing in its line: offered in New task → "In an idle terminal"
  "line": [{"ref": "T14", "id": 14, "title": str, "kind": "queued"|"resume"}],  // tasks waiting their turn in this terminal, first first
  "branch": str|null,          // git branch Midna reports (shown in the Sessions list subline)
  "closing": bool,             // a close job is pending/running for it
  "close": "close"|"force"|null, // how it can be closed: idle → "close", busy → "force" (press-and-hold), gone → null
  "renaming": str|null,        // a rename job to this name is pending/running (UI shows the new name greyed)
  "rename_error": str|null,    // the last rename failed within ~10 min: first line of Midna's error
  "role": str                  // only on the Jira desk's terminal: "Handles Jira for the board" (shown instead of "No task");
                               // its close is null, so it's never closed from the app
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
  "worktree_base": str|null,   // each task starts in its own git worktree detached at this branch (`tb goal set --worktrees`)
  "setup": str|null,           // what every task in the goal does first (`tb goal setup`); its handoff shows it with {task} {n} {wave} {goal} filled, {jira} with the ticket key (else the task ref), and {device} {target} ({device2} {target2}…) for the devices lent to the task
  "total": int,        // tasks in the goal, planned included
  "done": int,         // tasks with status done (failed included)
  "active": int,       // working + needs
  "needs": int,        // status needs
  "queued": int,       // status queued
  "starting": int,     // queued with a live start job
  "blocked": int,      // queued and waiting on another task (waits_for)
  "held": int,         // queued, not blocked, but held back by its waves or order, a lock or the device pool
                       // (the goal shows Blocked when blocked + held covers every queued task)
  "bits_waiting": int, // its backend bits not made in the flag tool yet; a done goal waits on them ("Waiting on N bits")
  "planned": int,      // status planned
  "open_issues": int,  // open backlog issues in the goal
  "prs_open": [int],   // PR numbers of done, not-failed tasks whose PR is still open (the goal isn't finished until they merge)
  "prs_planned": int,  // tasks that end in a PR (`ships_pr`), not counting canceled ones
  "prs_canceled": int, // tasks that finished without their PR (`tb done --no-pr`)
  "finished_at": iso|null,  // latest finished_at once EVERY task is done, prs_open is empty and no backend bit waits; else null
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
  "qa": [qa_comment],               // QA comments on the goal's tickets that need or needed something, waiting ones first; [] while QA is off
  "waves": [wave],                  // [] when no task has a wave; see below
  "shared": [task_card],            // tasks from other goals that also finish this one (`tb task set T<n> --also G<n>`); they count in the totals
  "notes": [{"id": int, "kind": "finding"|"decision"|"reference", "text": str, "source": str|null, "pinned": bool, "at": iso}],
                                    // source: "you", a task ref like "T4", or free text; shown as the note's byline
  "backlog": [issue],               // the goal's issues that are NOT dropped, newest first (full issue shape, see GET /backlog/:id, history optional)
  "closed_count": int,              // the goal's dropped ("won't do") issues
  "attachments": [attachment],      // attached to the goal itself
  "bits": {"list": [bit], "backend": int, "made": int, "waiting": int, "tool": str},
                                    // its bits (linked to it or to a task counted in it); "N of M created" counts backend ones
  "devices": {"needs": [{"tag", "n"}], "needs_text": "2 android", "devices": [device], "lent_here": int} | null
                                    // what each task asks for (unless it asks for its own) and the pool; null with no pool and no asks
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
  "goal_attachments": [attachment], // on its goal (omit or [] when no goal)
  "origin": {"from": str, "by": str, "url": str|null} | null,  // where the task came from; the task panel's "From" row
  "step_results": [step_result]     // the latest round of each of its steps that ran (see "The PR plan and flow")
}
```
On this detail, `pr.bar.wd` is the `step_result` of the step with a `bar` name (else null).

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

### `GET /backlog/triage`

Query: `project=<name|all>`. The Backlog page's list: open issues in no goal (untriaged), newest first.
```
{"issues": [issue_card + {"area": str, "impact": "high"|"med"|"low", "priority": "p1"|"p2"|"p3",
                          "group": str, "group_about": str, "grouped": bool, "detail", "said"}],
 "untriaged": int, "triaged": int, "total": int,   // triaged = in a goal, deferred, dropped, a task or a ticket
 "projects": [{"name": str, "untriaged": int}],
 "grouping": bool,  // Claude is sorting new issues now
 "ai": bool}        // Claude is on ([backlog] ai and `claude` installed)
```
Asking for it starts Claude on any issues it hasn't sorted yet. Until then (or without Claude) an issue's
fields are guesses from its kind: bugs p2 / medium, the rest p3 / low, grouped by kind, area = project.
`GET /state`'s `counts.untriaged` is the same count for every project.

### `GET /backlog/plan`

The latest plan asked for: `{"state": "none"}`, or `{id, state: "planning"|"ready", ids: ["B1", …],
waves: [{why, items: [{ref, after: [ref]}]}], name, by: "claude"|"rules", note?}`. `ids` are the issues it
was asked for; `note` says why Claude's plan was replaced by the priority rules. Making a goal from it clears it.

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
  "compacting": iso|null,        // as session_row
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

### `GET /days`

Query: `date=YYYY-MM-DD` (default today; a later day is read as today), `hide=a,b` (projects left out of
every number). Everything the Days page shows: the day, its week (Monday to Sunday) and the week before.
A past day is worked out once from `task_states` (every status change, kept by a trigger), the task log and
the backlog, and kept in `day_stats` (one row per project), so it outlives the cleanup of its events.
```
{"date": "YYYY-MM-DD", "today": "YYYY-MM-DD", "is_today": bool, "now": iso, "day_start": iso,
 "from_hour": int, "to_hour": int,          // the timeline's hours: work hours, widened to cover the day's work
 "projects": [str],                         // every project the page knows, hidden ones too (palette order)
 "hidden": [str], "oldest": "YYYY-MM-DD"|null,
 "day": totals + {
   "peak_slots": int, "peak_first": int|null,      // ten-minute slots at the peak, and the first one
   "running": [int; 144], "hourly": [int; 24],     // terminals working per ten minutes; marks per hour
   "lanes": [{"project": str,
              "bars": [{"task": int, "ref": "T12", "title": str, "from": iso, "to": iso, "open": bool,
                        "kind": "working"|"needs"|"done"|"failed"|"lost"|"stopped"}],
              "marks": [{"at": iso, "kind": "commit"|"question"|"pr"|"done"|"found", "task": int, "ref": str, "text": str}]}],
   "waits": [{"task": int, "ref": str, "at": iso, "min": float, "open": bool, "reason": str, "text": str, "title": str, "project": str}],
   "tasks": [{"task": int, "ref": str, "title": str, "work_min": float, "wait_min": float, "state": str}]},
 "week": [totals + {"date": "YYYY-MM-DD", "future": bool}],      // 7, from config.toml's first_weekday (Sunday by default)
 "first_weekday": "sun"|"mon"|…,
 "last_week": [same],
 "week_tasks": [{"task", "ref", "title", "project", "work_min", "wait_min", "state", "date"}],   // ≤ 12, longest first
 "week_task_median": float,                 // minutes, finished tasks of the week
 "usual": {"days": int, "agent_min", "done", "prs", "wait_min", "human_min", "wait_each": float}}   // medians of the 14 days before, days with work only
totals = {"agent_min", "human_min", "est_agent_min", "done", "prs", "commits", "wait_min", "questions": float,
          "peak": int, "by_project": {project: agent_min}}
```
`human_min` adds up the `tb done --human` estimates of the tasks finished that day; `est_agent_min` is the
agents' whole working time on those same tasks, so hours saved is `human_min - est_agent_min`.

### `GET /history`

How long the board keeps its history: `{"detail_days": 90, "summary_days": 365, "detail_choices": [30, 90, 180, 365],
"summary_choices": [180, 365, 730, 0], "events": int, "events_bytes": int, "summaries": int, "summaries_bytes": int,
"last_cleanup": iso|null, "oldest": "YYYY-MM-DD"|null}`. `summary_days` 0 keeps day summaries always. Sizes are estimates.

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
  "also": [{"id": int, "ref": "G4", "name": str}],   // other goals this task also finishes (shared task; its home goal runs it)
  "jira": {"key": str|null, "status": str|null, "url": str|null, "desk"?: bool, "failed"?: bool} | null,
          // null = no ticket. key null: status says where it is: "Ticket asked for" (or "Ticket asked for · Jira desk")
          // while it's found or made; "No ticket yet" (desk: "No ticket yet. The Jira desk finds or makes one…") while a
          // PR task waits for one ([jira] auto_ticket); "Couldn't make the ticket: <why>" with failed true.
          // url = the ticket's browse URL, built by the server from the configured Jira site (the UI never builds Jira URLs).
  "pr": pr | null,
  "position": number|null,        // order in the goal (unused by the UI apart from sorting done server-side)
  "starting": bool,               // queued and a start job is pending/running ("Starting")
  "line": {"session": str, "name": str, "kind": "queued"|"resume", "pos": int, "label": str, "after": "T12"|null} | null,
                                  // queued in a terminal's line (`tb task new --here --next`, or switched away from): it starts
                                  // there by itself, after `after`; label "Queued in Term 3" / "To resume in Term 3" (resume = started before).
                                  // Like any start it waits for work hours and the 5-hour usage (then `waiting` says so); Start runs it now
  "waiting": str|null,            // queued only: why it isn't starting yet, one plain line
                                  // ("Waits for T4 to finish", "Waits for work hours (tomorrow 6am)", "Waits for the 5-hour usage to reset (3pm)")
  "blocked": bool,                // queued and waiting on another task (waits_for), shown as "Blocked"
  "waits_for": ["T14"],           // tasks it starts after
  "waits_for_state": [{"ref": "T14", "done": bool, "stack"?: true}],   // the same plus the task it stacks on (`stack: true`), each with whether it's done (for a `tb wait-for --merged` wait: done and its PR merged, or it ships no PR); the goal page's "Waits for" chip
  "locks": ["local-core"],        // named locks it holds while it runs; tasks sharing a lock never run together
  "alone": "goal"|"board"|null,   // nothing else in its goal (or on the board) runs while it does
  "compacting": iso|null          // working/needs and its terminal is compacting since then ("Compacting since 3:05 PM" chip)
  "devices": {"needs": [{"tag": "android", "n": 2}], "needs_text": "2 android", "lent": ["pixel-7"]} | null,
                                  // what it asks for from the device pool (its own, else its goal's) and what it has now
  "bits": [{"name": str, "kind": "backend"|"local", "made": bool, "waiting": bool}],  // its bits; waiting = backend, not made
  "ships_pr": bool,               // it ends in a PR: its own setting, else its project's (`pr_flow` / a git remote)
  "ships_pr_set": bool|null,      // its own setting (`tb task set --pr yes|no`); null = the project's default
  "no_pr": str|null,              // why it finished without its PR (`tb done --no-pr`): "PR canceled: <why>"
  "no_evidence": str|null,        // why it finished without evidence (`tb done --no-evidence`)
  "stack_on": stack_on|null,      // the task whose PR this one's builds on (`--stack-on`)
  "wd": step_result|null          // before the PR opens: the `bar` review step's latest round (after, it's `pr.bar.wd`);
                                  // {"name", "bar", "pending": true} before its first round; null on a task that doesn't end in a PR
}
```
`stack_on`: `{"ref": "T3", "title": str, "num": int|null, "url": str|null, "branch": str|null, "merged": bool, "line": "Stacks on T3's PR #12"}`.
The card is draggable to Working when it's queued/planned, not starting, and not in a goal unless it waits in a terminal's line (drop = start with mode `new`).
`POST /tasks/T<n>/start` (the owner's Start, or `tb start` on their word) takes a task out of the terminal's line it waits in and runs it as asked.

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
    "phase": "checks"|"fix"|"ask"|"review"|"rereview"|"comments"|"merge"|"merged"|"declined",
                                 // rereview: changes were asked and are pushed; waiting for that reviewer to look again ("Awaiting re-review")
                                 // ask ([reviewers] ask_stage): the owner reviewed it; reviewers are being asked ("Asking for reviews")
    "label": str,                // plain words, e.g. "Watching checks", "Fixing checks", "Awaiting reviews", "Answering comments", "Merging", "Merged"
    "session": str|null,         // optional: id of the terminal the board woke for fix/comments/merge/ask (links to it)
    "asked": {"at": iso, "names": [str], "by": str} | null,   // who was last asked to review, when, and by whom (the agent, tb, "Task board")
    "stopped": {"asked": bool, "message": str} | null
                                 // optional: that terminal stopped before finishing (asked = it asked you a question).
                                 // Shows "Needs you" and an answer box on the done task; the answer goes through POST /tasks/:id/answer.
    "open_threads": int,         // review threads (and Bitbucket PR tasks) waiting on us: someone other than the board's own host account (the PR's author when that isn't known) spoke last
    "not_ours": {"checks": [str], "title": str, "reason": str, "proof": [url], "links": [{url, label}], "who": str, "at": iso} | null
                                 // this push's failed checks cleared with `tb pr not-ours`; the task panel's PR bar shows
                                 // a "Failed, but not because of this PR" box with the title, checks, reason, the proof links by their labels and "Checked … ago" (`at`)
  } | null,
  "bar": {                       // on cards from this board (absent in the frozen web fixtures)
    "build_url": str|null,       // the first failed check's link, else the newest by its updated/created time (running, else any), else the PR's checks page
    "checks": "not_needed"|"skipped"|"not_ours"|null,   // no checks at all / this push's checks skipped (a hook, tb pr skip-checks) / its failures cleared (`tb pr not-ours`)
    "checks_why": str|null,      // why they were skipped
    "you": "waiting"|"reviewed"|"skipped"|null,   // the owner's own look: a green PR waits for them / they marked it / review skipped
    "approvals": int, "reviewers": int,           // "1 of 2": approvals of (approvals + reviewers still asked)
    "new_comments": int,         // open threads waiting on the author (older reads: comments since the agent last handled them)
    "comments_url": str|null,   // where "N new comments" goes: the unread thread waiting longest, else the PR
    "waits_on_base": bool,       // phase `waits`: a stacked PR waits for the PR it builds on to merge
    "stacks_on": stack_on|null,
    "retargeted": str|null,      // the base the board pointed it at once its parent merged
    "retarget_error": str|null,  // why the last try to point it there failed (tried again with backoff)
    "wd": step_result|null,      // the author-side review step (a step with `bar`): its latest round, or {"name", "bar", "pending": true}
                                 // before the first ("Not reviewed by WD yet"); null when the task doesn't end in a PR or the PR is merged or closed
    "reviewer_rows": [{"name": str, "user": str, "state": "approved"|"changes"|"rereview"|"waiting"|"commented", "swaps": int, "asked_at": iso?}]
                                 // swaps: how many swaps led to this reviewer (the ask ledger); asked_at: when the board or tb asked them
                                 // one pill per reviewer still on the PR, from the host's reviewer states
  }
}
```
With `bar`, the task panel shows five steps: the review step (named by its `bar`, e.g. WD), Checks (linked to
`build_url`; "Not needed", "Not this PR's"), You ("Waiting on you" with "Waiting on your review · Mark reviewed ›",
"Reviewed", "Skipped"), Review ("x of N", "New comments", linked to the PR) and Merge ("Waits on base"), plus a
"Stacks on T3 · PR #12" line. Without it, the three steps below.

`checks` is `pass` when every failure on the head was cleared as not this PR's.

The task panel shows three steps (Checks, Review, Merge) from `checks`, `review`, `state` and `stage.phase`. A done
task whose PR is still OPEN shows "Awaiting merge" (or `stage.label`) instead of Done. The original's `build`,
`review` free text, `review_log`, reviewer lists (now `bar.reviewer_rows`) and
`reviewed_at` are gone; `asked` is `stage.asked`.

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
| `POST /tasks` | `{title, detail, project, priority: "normal"\|"high", goal_id: int\|null, auto_close: bool, pickup: {mode: "queue"\|"new"\|"attach"\|"manual", session_id?}, status?: "planned", jira?: {mode: "create"\|"link"\|"none", key?}}` | `tb task new`. `status: "planned"` only when it has a goal ("Add it to the goal's plan"). `jira` only sent when `state.jira.enabled`. **Response read:** the task (`ref` or `id`), then the caller shows it. |
| `POST /tasks/:id` | `{status: "queued"}` | "Queue it now" on a planned task (planned → queued only). |
| `POST /tasks/:id` | `{jira_key: "PROJ-1"\|"new"\|"none"}` | `tb task set --jira`: link a ticket, ask for one (again, after a failure), or no ticket (a PR task then stops waiting for one). |
| `POST /tasks/:id/start` | `{mode: "new"\|"queue", via_session?}` | Start / Start when the repo's free / New Midna terminal / Queue in Midna / drag to Working (`new`). A goal with a `worktree_base` makes the task's worktree first; one that can't be made answers 409. Without `via_session` it is the app's Start only (the `X-Task-Board-From: app` header, see "The app's own requests" below); any other caller gets 403. With `via_session` (`tb start T<n>`) it starts only when a prompt the owner typed in that terminal, since its conversation began, asks for it now: a clause that asks for the start ("start T4", "queue T4", "kick off T4", "run T4", "pick up T4", "start both T4 and T5", "start them: T4 and T5", "T4 is ready, start it") and names the task, or, in the latest prompt only, an unnamed ask: in a prompt that asks for new tasks, for the tasks that terminal made with `tb task new` in reply to it ("make a task and queue it", "queue the first one": the first; "make tasks for A and B and queue them", "both", "these": every one); in a prompt that is only the ask ("queue it", "ok, start them", "start the new task", "queue it and let me know when it's done"), for the task the prompt just before asked for and the terminal made in reply ("it" only when it made one, "the first one" the first). A prompt that says more than the ask ("the dev server won't come up; start it") may mean something else by "it", so it is no word for a task made before or after it. The latest prompt that speaks of the task's start decides, so a later "don't start T4", "hold off on T4" or "wait" takes an earlier ask back, as does a later prompt that names no task but takes back every start ("no, don't", "nope", "never mind that", "forget I said that", "I changed my mind") or asks for another one in its place ("start T5 instead"). Within a prompt the last word on the task decides: a take-back after the ask ("start T4. jk", "start T4. On second thought, don't.", "start T4, scratch that", "start T4. Not now though.") cancels it. The check reads the prompt as typed (newlines kept, stored in `session_events.full`; the history shows it on one line). The hook sends a prompt whole up to 20000 characters; a longer one keeps its start and its end with `[… cut …]` between, and the check reads both: an ask at the start counts unless the end takes it back. (A prompt the hook before beta.16 clipped, its first 7999 characters and "…", is no word, but takes back only what it says.) A no, a time or a condition holds only the start it is about. No ask: a no in the ask's clause ("don't start T4", "no need to start T4") or a bare one just before it ("never, ever, start T4", "I forbid you: start T4"), even with an aside between ("don't (like T5), start T4"); a time or condition in the ask's clause ("start T4 on Monday", "start T4 when T3 lands", "start T4 then"), just before it ("wait until 6am, then start T4", "once T3 lands, start T4"), or alone after it ("start T4, first thing", "start T4, at six", "start T4, the moment T3 lands", "start T4. Do it tomorrow."); a question ("start T4?"), quoted or pasted text (quotes, code, the rest of a line after "says:", the lines after a line ending in ":", indented or log-like lines), prompts with any `[task-board:…]` marker, and reports ("T4 started"). A no, time or condition about something else leaves the ask: "start T4 and tell me when it's done", "start T4. let me know if it fails", "start T4, no rush", "start T4 but don't merge it", "start T4 without the migration", "start T4, then wait for review", "start T4, the Monday report fix", "start T4 now, not later", "no, start T4" (an answer to the agent), "start T4. once it's done, start T5" (T4 starts, T5 waits). 403 otherwise, naming `tb start G<n>` when the task is in a goal. A task whose dependencies (`waits_for`, a stack parent) aren't done answers 409 from any caller; one ahead of its wave or turn starts. |
| `POST /tasks/:id/answer` | `{text, when: "now"\|"morning"}` | `morning` = hold it until work hours open ("Send at <when>"). Also answers a stopped PR visit. |
| `POST /tasks/:id/resume` | `{mode: "fresh"\|"reopen"}` | Lost terminal: new terminal with the handoff, or `--resume` the old conversation. |
| `POST /tasks/:id/requeue` | `{}` | Try again (failed) / Queue again (done). |
| `POST /tasks/:id/detach` | `{}` | Take it off its terminal; back to queued. |
| `POST /tasks/:id/done` | `{summary?}` | Mark done by hand (the app has no button; `tb done` reports through `/report`). |
| `POST /tasks/:id/fail` | `{reason?}` | Mark failed by hand (the app has no button; `tb fail` reports through `/report`). |
| `POST /tasks/:id/close-terminal` | `{force: bool}` | `force` false from the done box; true from Manage when the terminal is busy. |
| `POST /tasks/:id/focus` | `{}` | Bring its terminal to the front in Midna. |
| `POST /tasks/:id/meta` | `{meta: [[name, value], …]}` | Edit/remove Details fields. |
| `POST /tasks` / `POST /tasks/:id` | `{stack_on: "T3"\|"none", ships_pr: true\|false\|"yes"\|"no"\|"auto"}` | `tb task new\|set --stack-on`, `--pr`, `--no-pr`. `stack_on` must be a task in the same project that doesn't (transitively) build on this one; it blocks like `waits_for`, in any goal. `ships_pr` "auto" (or null) goes back to the project's default. |
| `POST /tasks/:id/pr/reviewed` | `{}` | "I reviewed it": the owner looked at a green PR that waited for them (`pr.stage.awaiting_you`); clears its alerts. |
| `POST /done/close-terminals` | `{}` | Done column menu: close every done task's still-open terminal (no force). |

**The app's own requests.** `X-Task-Board-From: app` marks a request as the owner's own click in Taskboard.app
(Start, a wave's review stop). The header alone is anyone's say-so: a request with it that the board can't tie to the
app is refused with 403 before it reaches the API. How it ties a request to the app is decided at each launch from
the daemon's own code signature (the board log's first lines say which, `crates/taskboardd/src/apporigin.rs`):

- **A signed daemon** (a release, or `packaging/build-app.sh` with a Developer ID): besides its port, the daemon
  listens on a unix socket, `app.sock` in its data folder (mode 0600), and the app sends its posts there. For each
  connection the kernel hands the daemon the peer's audit token (`LOCAL_PEERTOKEN`, taken at `connect`), and
  Security.framework checks that running process against
  `anchor apple generic and identifier "<bundle id>" and certificate leaf[subject.OU] = "<team>"`, the bundle id
  being the daemon's own signing identifier without `.daemon` and the team its own. The process also has to be
  validly signed right now with the hardened runtime on and no debugger attached. Only such a request is the app's;
  the port never is, and there is no token. config.toml can't loosen this.
- **An ad-hoc or unsigned daemon** (`cargo run`, `scripts/dev-app.sh`, tests): there's no signature to check, so the
  daemon writes a fresh random token at each launch to `app-token` in its data folder (mode 0600); the app reads it
  and sends it as `X-Task-Board-Token`, on the socket or the port. `[app_origin] token = false` turns that off (then
  no request is the app's).

What the signature check stops: any other program on the account saying it's the app (curl, a script, `tb`, a copy
of the app built or re-signed by someone else, the real app binary started under a debugger or with injected
libraries, a process that connected and then `exec`'d the app, a reused pid). What it doesn't: a process that can
drive the real app's window (Accessibility or AppleScript UI scripting, which macOS gates behind the Accessibility
permission), root, or anyone who can sign code as the owner's team. Agents can still change the board's data
directly (its SQLite file, config.toml) as the same user; the board's rules for agents live in the API. In an
unsigned build the token is a file the same user can read; the hook guard (HOOKS.md) catches the plain ways an agent
would, not every way.

### PRs (`tb pr …`)
The board watches GitHub PRs (through `gh`) and Bitbucket Cloud PRs (REST 2.0, with Taskboard's Bitbucket account);
`prhost.rs` documents the host interface. Each route below reads the PR from its host first and judges what it says
now. A host that can't be reached answers 502; a refusal answers 409 with the reasons.

| Path | Body | Notes |
|---|---|---|
| `GET /tasks/:id/pr` | `?full=1` | The card, the last read (`record`) and `watched`. `full=1` (`tb pr status`) reads it now and adds `live`: `base_moved`, `builds_note`, `rebase: [str]` (when the base moved: the commands to rebase onto it, test, pass each `per_head` step (the owner's review gate) on the new commit, push the branch to the remote and republish each such step the PR shows (`tb step publish "<step>"` when it has a `publish` script, else, for one with a `bar`, "publish <step>'s round for the pushed head where the PR shows it"), ending "Don't push only to rebase."), `failures: [{check, url, steps, tests, source, error?, base_fails, base_steps, base_tests, base_compared, cleared}]` (failed steps and tests from GitHub Actions, Bitbucket Pipelines, Azure Pipelines (with the token from `tb ci-token set`, else `$pr.azure_token_env`) or the project's `failures_cmd`; the base branch's last 5 commits' runs of the same check are read too, and `base_steps` / `base_tests` are this PR's failed steps and tests that fail there too; `base_fails` when all of them do; `base_compared`: `steps` (compared one by one), `check` (only the check's name could be compared) or null (the base doesn't fail it)), `not_ours`, `expected_missing`, `expected_wait_mins`, `expected_waited_out` (the wait is over: the missing ones no longer hold it), `reviewers: [{user, name, state: approved\|changes\|commented\|pending, requested, swapped_off}]`, `approvals: {have, need}`, `open_threads: [thread]`, `tasks_open` (null when they couldn't be read), `tasks_error` (why; the merge waits until they can be), `blockers: [str]` (why `tb pr merge` would refuse), `read_error`. |
| `POST /tasks/:id/pr/reply` | `{thread, text, resolve?: bool, who?}` | `tb pr reply`: answers the thread on the host (a GitHub comment that has no thread gets a quoting comment); `resolve` resolves it too. |
| `POST /tasks/:id/pr/ack` | `{thread, who?}` | `tb pr ack`: a thread that asks for nothing is resolved without a reply (on the board only, where the host can't resolve it). The ack holds until someone writes on the thread again. |
| `POST /tasks/:id/pr/addressed` | `{who?}` | `tb pr addressed`: 409 while threads are open; then asks each reviewer with a standing request for changes (not one swapped off) to review again, records an ask for each (`why: "rereview"`, unless they have one open, so a slow re-review is swapped like any ask), and ends the visit (stage `rereview`). **Response:** `{asked: [name]}`. |
| `POST /tasks/:id/pr/reviewers` | `{ask?: [who], replace?: who, with?: who, drop?: who, count?, dry_run?: bool, who?}` | `tb pr reviewers`: sets the PR's reviewers through its host. With none of `ask`, `replace` and `drop`, the picker chooses (`count` more, else enough to have `[reviewers] count` on the PR; `dry_run` answers `{picks: [{user, name, why: "pinned"\|"main"\|"turn"}]}` and asks nobody); 409 when nobody on the roster can review. `replace` without `with` takes the picker's choice. `ask` requests each (a roster name, alias, email or host id; someone on the PR's list by name; else a host id as given); `replace` takes one off and asks `with` in their place; `drop` takes one off (either closes their ask as `dropped`, which says nothing about their speed). 409 for the PR's author or someone removed from the roster. Each ask is recorded (`review_asks`), the people taken off go into `pr_flow.swapped_off` (their requests for changes stop holding), and `pr_flow.asked` notes who was asked. **Response:** `{asked: [{user, name}], dropped, replaced: {old, new}?, asks: [ask], pr}`. |
| `POST /tasks/:id/pr/merge` | `{who?, agent?: bool}` | `tb pr merge`: 409 unless it's open, its checks passed (failures cleared as not-ours aside; expected checks posted, until their `expected_wait_mins` is over; none running or stopped), it has the approvals it needs, nobody (still on it) asks for changes, no thread or PR task is open (and its PR tasks could be read), and a stacked base PR has merged. Then points every open PR that goes into its branch at its base (a PR that can't be moved stops the merge: 502), merges with the project's `merge_strategy` (else the repository's default) and deletes the source branch. 403 from an agent (`agent: true`) while agents don't merge the task's project's PRs (its `agents_merge`, else the board's `POST /projects/agents-merge`, else `pr.agents_merge`; `taskboardd import` turns it on for every project that came over with a PR, unless the project already says, and turns the board-wide switch on unless the board already says: the old board's agents always merged their own). |
| `POST /tasks/:id/pr/not-ours` | `{reason, title, proof: [url], checks?: [str], who?}` | `tb pr not-ours`: clears failed checks of the current head (all of them, or `checks`) that aren't the PR's fault. `reason` 20–300 characters, `title` up to 80, at least one http(s) `proof` link. 409 when nothing failed on this push or a named check didn't fail. A new push has to pass on its own. |
| `POST /tasks/:id/pr/skip-checks` | `{reason, all?: bool, who?}` | Counts this push's checks (or every push's) that were stopped, are running or never posted as passed: for builds a hook cancelled. 400 without a `reason`; 409 while a check failed on this push (use `not-ours`), and a later push's failure still counts. The card's `pr.bar.checks` is `skipped`. |
| `POST /tasks/:id/pr/wait` | `{}` | `tb pr wait`: the agent finished this visit. |
| `POST /tasks/:id/pr/merged` | `{who?}` | `tb pr merged`: the PR was merged outside the board. |

`thread = {id, kind: "review"|"comment"|"summary"|"task", resolvable, resolved, author, author_name, last_author, last_id,
last_at, path?, line?, text, url?, outdated?, replies?: [{author, author_name, text, at}]}` (`replies`: the comments after the first, oldest first). A thread is open while it's unresolved and someone other than the
board's own account on the host (the account it reads and posts as; the PR's author when that isn't known) spoke last
(a PR task: until it's resolved), unless it was acknowledged at its last comment.

**Per-project rules** (`[pr.projects.<name>]` in config.toml): `approvals` (needed to be ready to merge; unset =
`pr.approvals`, default 2; 0 = the host's verdict or any approval), `expected` (check names that must post on every
push; checks count as running until they do, for up to `expected_wait_mins`, default 90; `[]` = don't wait at all;
unset = the `no_checks_after_mins` grace), `failures_cmd` and `merge_strategy`. `approvals`, `expected` and
`expected_wait_mins` can be changed on the board with `tb project set` (`POST /projects/:name`), over config.toml's.

**`failures_cmd`** runs with `/bin/sh -c` for each failed check, with `TB_PR_URL`, `TB_PR_REPO`, `TB_PR_NUM`,
`TB_HEAD`, `TB_CHECK` and `TB_CHECK_URL` set, and prints `{"steps": [...], "tests": [...]}` or one failed step per
line (`test: <name>` for a failing test). It takes over from the built-in CI readers for that project.
### The PR feed (`tb feed`)
PR activity can come as events instead of the poll: any listener posts one event per PR change or build, and the
board reads that one PR again and steps it (`feed.rs` documents the health rules). Events work whether or not
`[feed] on` is set; with it, the board watches the feed's health.

| Path | Body | Notes |
|---|---|---|
| `POST /prs/event` | `{url?, repo?, num?, task?: "T12", kind?: "pr"\|"build"\|"heartbeat", state?, head?, branch?, provider?, build_url?, author?, mine?: bool, source?}` | `tb feed event`. The PR is found by `task`, its link, or `repo` + `num`. Notes the event for the feed's health, then reads a GitHub or Bitbucket PR again. A PR event for no task whose `author` is in `owner_emails` (or with `mine: true`, `tb feed event --mine`) notes it as one of the owner's open PRs (until a `merged`, `declined` or `closed` state), for PR builds. **Response:** `{ok, kind, task: "T12"\|null, refreshed: bool, phase?, read_error?, owner_pr?: {owners, open?}}`. |
| `POST /prs/heartbeat` | `{active?: bool, idle_until?: iso, connected_at?: iso}` | `tb feed heartbeat [--active\|--idle] [--idle-until <time>] [--connected-at <time>]`: the feed is alive. Once a feed has sent one, missing them for `stuck_secs` makes it stuck. `active` (`activeNow` is read too) and `idle_until` are the listener's own hours: while it says it's idle (until `idle_until`, if given) it's neither stuck nor silent, and once `idle_until` comes it's timed from then. `connected_at`: when the listener last connected. The first heartbeat, a later `connected_at`, and `active` after idle each start the settle window. 400 for a time that isn't one. **Response:** the health. |
| `GET /prs/feed` | | `tb feed`. **Response:** `{on, healthy, problem: "stuck"\|"silent"\|null, why, last_event_at, last_event, last_heartbeat_at, unhealthy_since, restarts: [{at, ok, error?}], listener: bool\|null, holding: str\|null, settling_secs: number\|null, connected_at, active: bool\|null, idle_until, off_hours: str\|null}` (`settling_secs`: what's left of the settle window; `off_hours`: why the feed is outside its hours); also `state.pr_feed`. |

While the feed is unhealthy (only with `[feed] on`): `feed::feed_healthy` is false and `feed::holding` holds
reviewer asks, nudges and swaps. A stuck feed (down or stale) holds even a PR's first ask, at any hour. Outside the
feed's hours (`feed::off_hours`: the listener says it's idle, or, for a listener that doesn't send `active`, the work
hours are closed) it may miss events, so `holding` holds everything but a PR's first ask. Once the feed is healthy
again, and after every connect (the first heartbeat, a later `connected_at`, the listener waking from idle, the board
starting its own `listener`), `holding` keeps holding for `settle_secs` (300) while the events it missed catch up
(`pr_feed.healthy_at`, `pr_feed.connected_at`; it holds the first ask too). The PR poll runs (while
healthy it rests unless `poll_while_healthy`); the board restarts the feed at `restart_mins` (1, 5, 15) after it went
bad with `restart_cmd` (or by restarting its own `listener`), and raises the `pr-feed` alert if a restart fails or it's
still bad 5 minutes after the last one. The alert clears when the feed is healthy again.

A request for changes only counts (moves the PR to "Addressing comments", blocks the merge, is asked again by `tb pr
addressed`) when the reviewer also wrote on the PR: started or spoke on a thread, a review summary included.

### PR builds (`tb pr-builds`)
A board-wide switch for when CI time is scarce, set only on the owner's word (`prbuilds.rs` documents it).

| Path | Body | Notes |
|---|---|---|
| `GET /pr-builds` | | `tb pr-builds`. **Response:** `{stopped, by, at, reason, resumed_by, resumed_at, cancelling: int, recent: [{at, ok, what, build, task, repo, num, branch, head, follow_up}], owner_prs: [{repo, num, branch, url}]}` (`recent`: the last 20, newest first: one per build stopped, `build` its pipeline's name, else one per cancel or give-up; `owner_prs`: the owner's open PRs off the board, opened by hand); also `state.pr_builds` (the app's "PR builds stopped" pill). |
| `POST /pr-builds` | `{stopped: bool, who?, reason?}` | `tb pr-builds stop [--reason] [--who]` / `resume`. `who` defaults to the owner. Stopping cancels what's running now; resuming drops the cancels still waiting. Both take down the `pr-builds:*` give-up alerts. **Response:** as `GET`. |

While stopped: a build event (`POST /prs/event` with `kind: build`, a running `state` such as `started`) on one of the
board's PRs, a running check seen on a poll, or a push build whose `author` is in `owner_emails` or whose `branch` is
the branch of one of the board's open PRs in that `repo` (merges, rebases, others' commits; cancelled as that PR's),
queues a cancel (once per push). So does a build on one of the owner's open PRs the board didn't open (opened by hand),
by its link or `repo` + `num`, or its `branch`; it's cancelled by the PR's number too (`TB_PR_NUM`; Bitbucket's
pull-request pipelines). Those PRs are read from the host on each repo the board knows (`gh pr list --author @me`,
Bitbucket's open PRs by the board's account) every `owner_prs_secs` (120 s; 0: never) while builds are stopped, and
noted from the feed's PR events. After a cancel the push is swept again after each of `follow_up_secs` (10, 30, 60,
120 s) for builds queued just after it. It runs `[pr_builds.cancel].<provider>` (the event's `provider`, else read from `build_url`: github,
bitbucket or azure, else the PR's host), else the PR host's own (`gh run cancel`, `stopPipeline`). A failed cancel is
tried again after each of `retry_secs` (0, 10, 30, 60, 120 s), then raises an alert keyed `pr-builds:<…>`; a CI with no
way to cancel alerts at once. A cancel command may write the builds it stopped, one name per line, to the file named by
`$TB_CANCELLED`. A cancelled push is logged on its task and kept in `recent`; a follow-up only when it stopped a build (a
cancel command's follow-up that writes nothing to `$TB_CANCELLED` is left out of both, and so is any round, the first
too, that writes it empty, or a host cancel that stops none: it stopped nothing; the push is still marked cancelled). An entry with no builds of its
own names the pipeline the build event gave (`pipeline`, or Azure's `definition.name`). A build or PR event matches a
board task's PR, or one the owner opened by hand, by its repo in any case, or by the repo's short name (after the last
`/`) when one side gives no org, so `ACME/repo`, `repo` and `acme/repo` are one; only among the board's PRs with that
number on the event's host, and `globex/repo` is never `acme/repo` (when the board has both, `repo` names neither). The PRs' checks count as passed: the build
reads "Builds stopped" and the PR moves on to review.

A build event for a PR on a host the board doesn't read (GitLab, …) asks the owner's `pr.checks` hooks (a build
started) or `pr.fix` hooks (a build failed), once per push; a skip counts that push's checks as passed. The response's
`builds: {state, cancel?: "queued"|"waiting", hook?: "go"|"skip"|"block", owners?, task?}` says what happened.

### Master breaks (`tb master`)
An optional watch on each project's default branch (`[master.projects.<name>]`); `breaks.rs` documents the flow and
the `breaks` table.

| Path | Body | Notes |
|---|---|---|
| `GET /master` | | `tb master`. **Response:** `{open: [break], closed: [break] (the last 10), watched: [{project, branch, checked_at, green_head, error}]}`. `branch` is `[master.projects.<name>] branch`, else the one the import carried over (the old board's), else the repo's default branch read from the host (else the clone's `origin/HEAD`); null before the first read. |
| `GET /master/:ref` | | `tb master M3`. **Response:** the break. |
| `POST /master/:ref` | `{verdict: "ours"\|"not-ours"\|"unsure", why?, who?, proof?: [url]}` | `tb master M3 ours\|not-ours\|unsure [--proof <url>]…`: the owner's word, never decided again. `not-ours` needs at least one http(s) `proof` link (a build or an issue that shows it; 400 without). `ours` makes the fix task (if it has none) and raises the urgent alert; the others take the alert down. 409 once it's closed. **Response:** the break. |
| `POST /master/check` | `{}` | `tb master check`: read every watched branch now. **Response:** as `GET /master`. |

`break = {id, ref: "M3", project, host, repo, branch, state: "open"|"closed", head, last_head, green_head, fixed_head,
checks: [str], evidence: {checks: [{name, url, steps, tests}]}, suspects: [{sha, name, email, message, ours}], verdict:
"ours"|"not_ours"|"unsure"|null, verdict_label, verdict_by: "commits"|"claude"|"fallback"|<who>, verdict_why,
verdict_at, proof: [url], task: {ref, title, status}|null, opened_at, closed_at, checked_at}`. `state.master` lists the
open ones that are `ours`: the app's one "Master is red" banner line each, with Open T<n> for the fix task (the app
doesn't show the break's urgent alert as a row of its own beside it).

A failed check on the branch's head opens a break; a head whose checks all passed closes it. Suspects are the commits
since the last green head the board saw: the whole `green..head` range from the host (GitHub's compare, Bitbucket's
`commits?exclude=`) when it's further back than the `commits` read, every commit read when the board never saw it
green; each `ours` when its author's email is in `owner_emails`.
No suspect of the owner's: `not_ours`. Otherwise a headless `claude -p` decides from the evidence (`[master]
fault_check`), or without it: every suspect the owner's makes it `ours`, else `unsure`. Only `ours` gets a fix task
(high priority, with a new Jira ticket when Jira is on (`[master] fix_ticket`), started at once in a new terminal
(`[master] start_fix`; when it can't start it waits in the queue)) and an urgent alert keyed `master:M<n>`; it repeats outside the work hours and clears when the
branch is green. A new head brings new suspects and decides again (unless a person set the verdict); `unsure` is decided
again after `recheck_mins`; a fix task that finished while the branch is still red raises the alert again.
A fix already in flight on the same project and branch covers an `ours` break (its own task first, then the other
breaks'), so no new fix task starts: the task isn't done, or its PR is still open, or its PR merged after the failing
build was queued (the failed checks' queue time from the host, else when the board first saw that head red). Only when
nothing covers it does it get a new fix task, so a build queued after the fix merged that still fails gets one. A fix
that finished without a PR, or whose PR was declined, hands its break to a covering task (logged on both) and raises no
alert again.

**CI token** (`tb ci-token`): `GET /ci-token` → `{set, source: "board"|"env"|null, env}` (never the token); `POST /ci-token {token}`
keeps an Azure DevOps personal access token in the Keychain (`taskboard-azure-devops`), which the board reads Azure
Pipelines steps and tests with; `POST /ci-token/clear` forgets it. Without one, the daemon's `$pr.azure_token_env` is used.

### Projects
| Path | Body | Notes |
|---|---|---|
| `GET /projects` | | Every known project with `remote: bool\|null`, `pr_flow: "auto"\|"on"\|"off"`, `ships_prs: bool`, `pr_rules: {approvals, expected: [str]\|null, expected_wait_mins, ask_stage: bool, swap: bool, review: bool, agents_merge: bool, set: {…what tb project set changed}}` (`tb project show`). |
| `POST /projects/:name` | `{pr_flow?: "auto"\|"on"\|"off", approvals?: int\|null, expected?: [str]\|null, expected_wait_mins?: number\|null, ask_stage?: bool\|"on"\|"off"\|null, swap?: bool\|"on"\|"off"\|null, review?: bool\|"on"\|"off"\|null, agents_merge?: bool\|"on"\|"off"\|null}` | `pr_flow`: whether the project's work ends in PRs; `auto` follows its git remote (`tb project set --pr-flow`). The PR rules (`tb project set --approvals N`, `--expected-check NAME` (repeat; `none` = `[]`), `--expected-wait MINS`, `--ask-stage on|off`, `--swap on|off`, `--review on|off`, `--agents-merge on|off`; `default` sends null) change the project's rules on the board; null goes back to config.toml's. At least one key. **Response read:** the project as in `GET /projects`. |
| `GET /projects/agents-merge` | | `tb project agents-merge`: `{agents_merge: bool, set: bool\|null, config: bool}`: whether agents merge on projects whose own rules don't say, what the board set (null: nothing), and config.toml's `pr.agents_merge`. |
| `POST /projects/agents-merge` | `{agents_merge: bool\|"on"\|"off"\|null}` | `tb project agents-merge on\|off\|default`: the board-wide switch, over config.toml's `pr.agents_merge` and under a project's own `agents_merge`; null goes back to config.toml's. `taskboardd import` sets it on unless the board already says. **Response read:** as `GET /projects/agents-merge`. |

### Goals
| Path | Body | Notes |
|---|---|---|
| `POST /goals` | `{name, tldr, outcome, project, run_in_order: bool, max_terminals: int, auto_close: bool, epic: {mode: "create"\|"link"\|"none", key?}}` | `tb goal new`. `epic.mode` is always `none` without Jira. **Response read:** the goal (`ref`/`id`, or `{goal: {...}}`); the caller shows it. |
| `POST /goals/:id` | any subset of `{name, tldr, outcome, project, run_in_order, max_terminals, auto_close, epic_key: str\|null, paused: bool, deprioritized: bool, setup: str\|"none"}` | `tb goal set` (`epic_key` only with Jira; `--deprioritize`/`--prioritize` set `deprioritized`), the goal page's "How this goal runs" (`max_terminals`, `run_in_order`, `auto_close`), Pause/Resume, Deprioritize/Bring it back. |
| `POST /goals/:id/run` | `{via_session?}` or `{now: true}` | The board's Run, `tb start G<n>` and `tb goal set --run`. Without `via_session` (the app's Run) it runs as asked. With `via_session` it runs only on the owner's word in that terminal, read as for `POST /tasks/:id/start`: a prompt since the conversation began that asks for the run now ("start G2", "run G2", "kick off goal G2"), or an unnamed "start the goal" / "run the goal" when the goal is that conversation's (the terminal's task is in it, the conversation made a task in it with `tb task new`, or, in prompts after it, a `[task-board:G<n>]` prompt handed it the goal); "it" is never a goal. The latest prompt about the goal's run decides; a no, a time or condition, a question, pasted text and board prompts are no word; 403 otherwise. Queue the planned tasks (and re-queue `start_failed` ones), clear paused/deprioritized. `now: true` outside work hours = let this goal run until the hours next open (`goals.hours_until`). **Response read:** `queued_now: int` (how many planned tasks it queued). |
| `POST /goals/:id/plan` | `{mode: "edit"}` | "Plan in Claude": open a Claude terminal in Midna on the goal's plan (no prompt; goal context as system prompt). |
| `POST /goals/:id/notes` | `{kind: "finding"\|"decision"\|"reference", text, source: "you"}` | Add a goal note (no app button; `tb note --goal`). |

### Backlog
| Path | Body | Notes |
|---|---|---|
| `POST /backlog` | `{title, kind, goal_id: int\|null, project, said?, detail?, source?: "answer"\|"review_log"}` | Add an issue (source `you` unless it says `answer` or `review_log`, the external PR feed, which the app shows as "From the Review log"; no app form, `tb backlog add`). **Response read:** the issue (`ref`, or `{issue: {...}}`). |
| `POST /backlog/:id/promote` | `{where: "board"\|"goal", who?}` | Make it a task (`--board` for `board`): `board` = queued task; `goal` = planned task at the end of its goal. Only an open issue: 409 when it's already a task, dropped or otherwise closed. **Response read:** `{task: {ref\|id}}` (or `task_id`) so the board opens the new task. |
| `POST /backlog/:id/ticket` | `{who?}` | Create a Jira ticket for it (only offered with Jira; only an open issue). |
| `POST /backlog/:id/drop` | `{reason?, who?}` | Won't do; only an open issue (409 otherwise). |
| `POST /backlog/:id/reopen` | `{who?}` | Open it again. |
| `POST /backlog/:id/move` | `{goal_id: int\|null}` | Move to another goal or none (`tb backlog move` with one issue; `goal_id` also takes a ref like `"G2"`). |
| `POST /backlog/:id/note` | `{text}` | Add a note to its history (no app button). |
| `POST /backlog/bulk` | `{ids: ["B1", …], action: "task"\|"ticket"\|"drop"\|"move"\|"reopen"\|"defer"\|"priority"\|"goal", where?: "goal", goal_id?: int\|null, priority?: "p1"\|"p2"\|"p3", reason?, who?}` | `tb backlog task\|ticket\|drop\|reopen B4 B5 …` and `tb backlog move B4 B5 G2\|none` (one or more issues; all change or none do), the Goal page bulk bar (Make tasks, Create tickets, Won't do) and the Backlog page's selection bar. `task` sends `where: "goal"`; `move` (no app button) sends `goal_id`. `defer` sets state `defer` (not for now; triaged). `priority` sets the issues' priority. `goal` puts them in that goal as planned tasks, in its last wave. `who` (tb sends the terminal) is credited in each issue's history and the new tasks' origin; without it, the owner. **Response read:** `count: int`, `tasks` (the tasks made). |
| `POST /backlog/plan` | `{ids: ["B1", …]}` | Plan waves for these open issues (Backlog page). With Claude it answers `state: "planning"` at once and plans on its own thread; poll `GET /backlog/plan`. **Response read:** the plan (below). |
| `POST /backlog/goal` | `{name, waves: [{why, items: [{ref: "B1", after: ["B2"]}]}]}` | A new goal from a plan: one project's open issues, none in a goal yet. Each becomes a planned task in its wave (`tasks.wave`); `after` becomes the task's wait-for; the waves go in a pinned goal note. The goal doesn't run in order: a wave starts once every task in the waves before it is done. **Response read:** the goal detail. |

### Attachments
| Path | Body | Notes |
|---|---|---|
| `POST /attachments/:id` | any of `{title, url, kind}` | Edit. |
| `POST /attachments/:id/remove` | `{}` | Remove. |

### Work hours, alerts
| Path | Body | Notes |
|---|---|---|
| `POST /hours` | `{on: bool, start: "HH:MM", end: "HH:MM", days: ["mon", …], today_until?: "HH:MM"\|"off", alert_every_mins?: int}` | Sent on every change in the hours menu (and by `tb hours`; `--alert-every` sets `alert_every_mins`, 0 = alerts don't repeat). `today_until` only when it changed (`off` clears it). **Response read:** the new `work_hours` object (replaces `state.work_hours` at once). Errors (e.g. "4pm has already passed today.") show in the menu. |
| `POST /alerts/:id/dismiss` | `{}` | 409 for a review or urgent alert. |
| `POST /alerts` | `{text, urgent?: bool, key?: str, task?: "T12", goal?: "G3"}` | Raise an alert (`tb alert raise`). With a `key`, raising it again while it's up returns the one that's up. 409 for a plain alert on a task that has an urgent one up. **Response read:** `{alert}`. |
| `POST /alerts/:id/clear` | `{}` | Clear an alert by its id or key, urgent ones too (`tb alert clear`). **Response read:** `{alerts}`. |
| `POST /alerts/:id/snooze` | `{mins}` | One of `[alerts] snooze_mins` (or 15, 30, 60). |

### Context limits (`tb limits`)
`GET /limits` → `{compact_window, cold_idle_mins, warm_tokens, warm_idle_mins, generated: [glob], project_generated: {project: [glob]}, defaults: {…the same, from config.toml}, line: str}`.
`POST /limits` takes any of those numbers (0 turns one off, null puts config.toml's back), `generated` (a list, a
comma-separated string, or `"none"`) with an optional `project` for that project's own globs, and `reset: true`.
It answers like `GET`, and the board rewrites the `.git/info/attributes` blocks at once (else every 5 minutes),
and takes the block out of a repo it no longer looks after (its project off Midna's list with no open goal, task, PR or terminal
there; its work done). The Python board's block (`# task-board: generated files …` to `# task-board: end`) is replaced
by the board's, and once after an upgrade every repo the board has known is swept for blocks from before they were tracked (and again after `taskboardd import`, for the repos it brings).

- `compact_window`: board terminals' Claude gets `--settings '{"autoCompactWindow": n}'` (unless the job brings its own `settings`).
- `cold_idle_mins`: a conversation idle longer is compacted before it carries on: a headless `claude -p /compact --resume <id>
  --output-format json --setting-sources ""` (no user settings or hooks; a nonzero exit or `is_error` is a failure, and the
  resume goes ahead uncompacted) before a new terminal resumes it, or `/compact` queued ahead of an agent job's prompt or a message job's text in its live terminal
  (once per job, logged on the job's task: "Its conversation has been idle N min (Xk tokens), so the board compacts it before
  sending it anything.").
- `warm_tokens`, `warm_idle_mins`: a task (after its wait-for) or a PR visit resumes its conversation only while it's under
  both; otherwise it starts fresh from the handoff and the history says why. A live terminal is always typed into.
  Size and idle time come from the conversation's transcript (its last reply's context, or the last compact
  boundary's `postTokens` when that's newer; its last line's time).

### History
| Path | Body | Notes |
|---|---|---|
| `POST /history` | `{detail_days?: 30\|90\|180\|365, summary_days?: 180\|365\|730\|0}` | Answers `GET /history`. |
| `POST /history/cleanup` | `{}` | Runs the nightly cleanup now: events, status changes and terminal lines of finished tasks older than `detail_days` (each day's summary is kept first), day summaries older than `summary_days`. Answers `{"removed": {"events", "status_changes", "terminal_lines", "day_rows"}, "history": …}`. |

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
| `POST /sessions/:id/close` | `{force: bool}` | `force` from the press-and-hold Force close. 409 for the Jira desk's terminal. |
| `POST /sessions/close` | `{ids: [str]}` | Close several idle terminals with no task. **Response read:** `{closing: [id], skipped: [id]}`. |
| `POST /sessions/:id/reopen` | `{}` | Reopen a closed terminal's conversation in a new Midna terminal. |

---

## QA comments (optional)

Off until Settings ▸ QA switches it on; needs Jira. Every `[jira] qa_poll_mins` (0: 5 minutes over REST, 30 through
Claude) the board makes one search, `key in (<its tickets>) AND updated >= -<N>m`, and reads the new comments of the
tickets it finds; with `via = "claude"` that whole check is one `claude -p`, told the current UTC time. `qa_comment`:
```
{"id": int, "ref": "Q3", "jira_key": "PROJ-7", "comment_id": str, "url": str, "author": str|null,
 "verdict": "task"|"flag"|"none"|null,   // null: still being read
 "title": str|null, "ask": str|null, "text": str|null, "pr": 0|1|null,
 "task": "T9"|null, "task_status": str|null, "task_title": str|null,   // the follow-up task (goal detail adds status and title)
 "source_task": "T1", "handled_at": iso|null, "handled_by": str|null, "created_at": iso,
 "waiting": bool}                         // a flag the owner hasn't answered
```

| Request | Body | What |
|---|---|---|
| `GET /qa` | | `{on, jira, since, checked_at, comments, waiting}` for Settings ▸ QA. |
| `POST /qa` | `{on: bool}` | Switch it on (409 without Jira) or off. On reads only comments from then on; off clears its alerts. |
| `POST /jira/comment` | `{key, comment_id, author?, text?}` | A comment from a forwarder. Ignored while off or when the ticket isn't on the board. |
| `GET /qa-comments` | `?limit&waiting=1` | `{on, comments: [qa_comment]}`, newest first. |
| `GET /qa-comments/:id` | | One `qa_comment`. |
| `POST /qa-comments/:id` | `{action: "task"\|"ignore", note?, pr?, who?}` | The owner's word on a flag (`tb qa task Q3`). Answers the comment plus `started: "T9"\|null`. |

## Jira

Optional: off while `[jira] site` or `project` is empty. Jobs (`J<n>`) run through the REST API with a token, or with
`via = "claude"` through a headless `claude -p` limited to the Atlassian connector's tools (`claude_tools`), one at a
time. A new ticket is always searched for first: an open ticket of the type that already covers the work is linked
instead of making another. With `auto_ticket`, every queued or working task in a project that ships PRs asks for a
ticket and waits (`waiting`: "Waits for its Jira ticket…") until it has one, it says `--jira none`, or the ticket is
linked by hand; a failed ask waits for `tb task set T<n> --jira new` (try again) or `--jira KEY` (link one), which its
`waiting` line says, and raises an alert that says it too. A goal with no epic first takes an open epic that already
covers its work (over REST: the open epic sharing the most of its name's words, stopwords aside, when they're at least
half of either's; through Claude or the desk, the one it judges covers it), and only gets a new one when none fits.
With `desk`, new tickets go to the Jira desk: one Claude terminal the board opens in Midna's Background group (an
`agent` job, purpose `jira_desk`; take `jira_desk` out of `[terminals] background` to open it with the project tabs; an
older `init`'s `background = []                      # example: ["plan"]` line is rewritten to `["jira_desk"]` on load)
and never closes. Its `--allowedTools` are `claude_tools` plus `Bash(tb jira:*)` and
`Bash(<tb path> jira:*)` for the path it's told to run tb by, so its reports don't wait on a prompt. It gets one job
at a time as a message starting `[task-board:J<n>]` and reports with `tb jira`.

| Request | Body | What |
|---|---|---|
| `GET /jira` | | `{on, site, project, via: "rest"\|"claude", auto_ticket, desk, desk_session, products: [{name, what}], jobs: [job]}` (`tb jira`). |
| `GET /jira/jobs/:id` | | One Jira job (`tb jira J12`). |
| `POST /jira/jobs/:id` | `{ok: bool, key?, status?, found?: bool, product?, message?}` | The desk's report (`tb jira J12 ok key=PROJ-1 status="To Do"`, `tb jira J12 fail "<why>"`). A new ticket's `ok` needs `key` (400); a `fail` needs `message`. `product` (picked from the products' `what`) goes on a goal that has none. |

## Waves

A goal's tasks can be grouped in waves (`tasks.wave`); a wave starts once every wave before it has passed.
`wave`:
```
{"wave": int, "name": str, "stop_after": bool, "released_at": iso|null, "passed": bool,
 "held": bool, "held_at": iso|null,                                  // held with tb goal wave --hold
 "state": "done"|"stopped"|"failed"|"running"|"held"|"ready"|"waiting",   // stopped: done, waiting for your review
 "blocked": bool, "held_by": state|null, "hold": str|null,           // the earlier wave that holds it, and why
 "tasks": ["T3"], "total": int, "done": int, "failed": ["T4"], "active": int, "starting": int, "planned": int}
```
`goal_summary.stopped` is a one-line note while the goal waits at a wave for you. Task cards carry `wave: int|null`.

| Request | Body | What |
|---|---|---|
| `POST /goals/:id/waves/:n` | `{name?, stop_after?}` | Name a wave (`tb goal wave --name`) or make it a review stop. `stop_after` is the owner's own word: only Taskboard.app sets it (it sends `X-Task-Board-From: app`, see "The app's own requests"); from anyone else it's 403. Answers the goal detail. |
| `POST /goals/:id/waves/:n/hold` | `{on?: bool (true), who?}` | Hold a wave (`tb goal wave --hold`): its tasks that haven't started don't, nor any later wave, until it's continued. `on: false` lifts it. 409 on a done wave. Answers the goal detail. |
| `POST /goals/:id/waves/:n/continue` | `{who?}` | "Continue to wave N": go on past a review stop or a failed task; on a held wave that isn't done, let it start. Answers the goal detail, with `let_start: true` when it let a held wave start. |
| `POST /tasks/:id` | `{wave: int\|null}` | A task's wave (only in a goal). |

## Locks, running alone and worktrees

All opt-in (`tb task set --lock/--alone`, `tb goal set --worktrees`). A task holds its locks while it's active: working
or needing the owner (not a failed start), or queued with a start job. The next in start order waits ("Waits for
local-core (T12 has it)"). A task with `alone` waits until its goal (or the board) has nothing active ("Waits to run
alone (T1 and T2 are working)"), queued tasks behind it wait for it ("Waits for T13 to run alone first"), and while it
runs nothing else in its scope starts ("Waits while T13 runs alone").

| Request | Body | What |
|---|---|---|
| `POST /tasks/:id` | `{locks: [str]\|"a, b"\|"none", alone: "goal"\|"board"\|"none"\|bool}` | Lock names are lowercase letters, digits, `.`, `_`, `:` and `-` (400 otherwise). A name no other task uses comes back in `warnings` (a spelling hint). Also on `POST /tasks` and `tb.new_task` / `tb.propose` items. |
| `GET /locks` | | `{locks: [{name, held_by: "T12"\|null, tasks: ["T13"]}], alone: [{ref, title, scope, goal, running}]}` (`tb locks`). |
| `POST /goals/:id` | `{worktree_base: "origin/main"\|"off"}` | Each task starts in `<repo>/.claude/worktrees/T<n>`, made with `git fetch` and `git worktree add --detach` at the base. Once the task is finished (failed, or no open PR) and its terminal is gone, the board removes the worktree, or keeps one with uncommitted changes. |

`tb propose` and `--task` items take a fourth `::` field, what the task waits for: `"title::detail::2::#1, T14"`, where
`#k` is the k-th task in the same request (400 when it doesn't point at an earlier one). A fifth field is the files
the wave plans for the task, comma-separated: `"title::detail::2::::src/form.rs, src/form.css"` (an object item, and
`tb.new_task` with a goal, take `files: [str]`; `tb task new --goal G3 --file src/form.rs`). They're kept in the task's
context as `plan_files`; its handoff names them, and each wave mate's handoff lists them as the files that task owns
(else the files it has touched). The handoff's wave mates are only the ones still queued or working.

## Devices

One pool of devices for every project (`tb devices`, `tb device add|set|remove|focus`). A task asks for devices by
tag or name (`{devices: "android:2 ios"}` on `POST /tasks`, `POST /tasks/:id`, or `POST /goals/:id` for the goal's
tasks that don't ask for their own; `"none"` clears). On a task, `"none"` (or `[]`) is its own "needs none", over
its goal's needs, and `"goal"` drops its own so it asks for its goal's again. The runner starts it only once that many
are free, lends them when it starts or an agent takes it with `tb take` (before the handoff is built, which names
them), and takes them back once the task isn't active
(done, or a failed start), the same rule as locks. A queued task's `waiting` line says why ("Waits for a android
device (T4 has them)", "Needs 2 ios devices, and the pool has 1 (pixel-8 is reserved for G3)"); a device asked for by
name says so first, whatever the goal's pool ("Waiting for a free dev-c (dev-c is reserved for G2)", "Waiting for a free dev-c (dev-c is with T4)", "Waiting for dev-c
(it's off)"), and a need no device has as its name or tag says "No dev-zz yet (tb device add)". A task started
without all it asks for (by hand, or with nothing free) logs "<why>; it started without".

A goal can keep devices of its own (`tb goal devices`): its tasks are lent only those, as on the Python board
(`[devices] goal_pool_only = false`: those first, then the rest of the pool; a task short of them waits with "Needs
2 android devices, and G3's own devices have 1"), a device it reserves is never lent to other goals' tasks (nor to
tasks in no goal), and the purposes it gives a device count as that device's tags for the goal's tasks (a device for
`measure` answers a `measure` need there; `measure,demo` is two). More than one goal may reserve a device: their
tasks share it. An archived goal reserves nothing and its pool lends nothing; its page still lists its devices.
The goal detail's `devices` aside lists the goal's own devices first, each with `in_pool`, `purpose` and
`reserved`, and `pool` (how many it has).

A device can say what it is: `kind` (a label, never matched against needs) and `target` (the emulator's serial or
the simulator's UDID, which its commands use as `{target}`), shown with its name as `label` ("dev-a (Android
emulator, emulator-5554)"). Known kinds are named in words, as the Python board named them: `android` → "Android
emulator", `ios` → "iOS simulator", `device` → "Phone or tablet", `other` → "Device" (others show as given; a
device with no kind has `kind_label` "Device" and no kind in its `label`). Its `start_cmd` and `stop_cmd` go in
the handoff of the task that's lent it, as "Start it: …" and "Stop it when you're done: …" (under the device's name
when it has more than one), filled like step commands: the task's `{task}`, `{n}`, `{title}`, `{branch}`, `{base}`,
`{repo}`, `{pr}`, `{wave}`, `{goal}`, `{jira}` (the task's ref when it has no ticket)… and the device's `{device}`
(also `{name}`), `{kind}` and `{target}`. That device paragraph comes early in the handoff and is never cut to fit
its length (a device note in it is clipped to 300), and it ends with "Other devices in use, don't touch them: …":
every device the task must leave alone, with its target and why (`dev-b emulator-5556 (with T5)`, `reserved for G3`,
`kept for …` for one turned off), joined with "; " and clipped to 500. A task card's `devices` has `lent` (names) and `lent_labels` (each `label`).

`device`: `{id, name, label, kind: str|null, kind_label: str, target: str|null, tags: [str], note, off: bool,
focus: str|null, can_focus: bool, start_cmd: str|null, stop_cmd: str|null, held_by: {ref, title, goal}|null,
goals: [{goal, purpose, reserved}], reserved_for: "G3"|"G3 and G4"|null}` (`goals` and `reserved_for`: goals not archived).

| Request | Body | What |
|---|---|---|
| `GET /devices` | | `{devices: [device], waiting: [{ref, title, goal, needs, why}]}`. |
| `POST /devices` | `{name, tags?, kind?, target?, start_cmd?, stop_cmd?, focus?, note?}` | Add one. Names and tags are lowercase letters, digits, `.`, `_`, `-` (names also `:`). 409 when the name is taken. |
| `GET /devices/:name` | | The device. |
| `POST /devices/:name` | `{name?, tags?, kind?, target?, start_cmd?, stop_cmd?, focus?, note?, off?}` | Change it; `off` keeps it from being lent; `none` (or empty) clears `kind`, `target`, `start_cmd`, `stop_cmd` and `focus`. |
| `POST /devices/:name/remove` | | Take it out of the pool (409 while it's lent). |
| `POST /devices/:name/focus` | | Raise its window: runs its `focus` command, else `[devices] focus`, with `sh -c` (`{name}` and `$TASKBOARD_DEVICE` are its name). 409 when there's neither. |
| `GET /goals/:id/devices` | | `tb goal devices G3`. `{goal, devices: [device + {purpose, reserved}]}`: the goal's own devices. |
| `POST /goals/:id/devices` | `{device, purpose?, reserved?}` | `tb goal devices G3 --add rig [--purpose measure] [--reserve\|--unreserve]`. Put a device in the goal's pool, or change its purpose (a tag word, or a comma list of them, stored as `measure,demo`; `none` drops it) or `reserved` (unchanged when left out; other goals may reserve it too). Same response as GET. |
| `POST /goals/:id/devices/:name/remove` | | `tb goal devices G3 --remove rig`. 404 when it isn't one of the goal's. |

## Bits (feature flags)

A bit is `backend` (it has to be made in the flag tool) or `local` (in the code only), linked to tasks and goals.
Tasks start and build behind a flag whether or not it's made; only a goal whose tasks are all done waits on its
unmade backend bits ("Waiting on 1 bit"). The handoff lists a task's bits, and says an unmade one doesn't hold up
the work.
`[bits]` in config.toml names the tool and its "new flag" link.

`bit`: `{id, name, kind, project, note, made: bool, made_at, made_by, waiting: bool, create_url: str|null, tasks: ["T4"], goals: ["G2"], created_at}`.

| Request | Body | What |
|---|---|---|
| `GET /bits` | query `goal`, `task` or `project` | `{bits: [bit], tool, create_url}`. |
| `POST /bits` | `{name, kind: "backend"\|"local", tasks?: ["T4"], goals?: ["G2"], project?, note?, who?}` | Add one (`tb bit add`). The project defaults to its first task's or goal's. |
| `GET /bits/:name` | | The bit. |
| `POST /bits/:name` | `{name?, kind?, note?, project?, tasks?, not_tasks?, goals?, not_goals?, who?}` | Change it or its links (`tb bit set`). |
| `POST /bits/:name/made` | `{undo?: bool, who?}` | It's made in the flag tool, or with `undo` it isn't (`tb bit made`, or the app's one-click "Mark created" on a backend bit). 409 on a local bit, which isn't made anywhere; changing a bit to local clears its made. |
| `POST /bits/:name/remove` | `{who?}` | Remove it and its links. |
| `POST /tasks/:id` | `{bits: ["newCheckout"]\|"none", not_bits: [...]}` | Link a task to bits (404 for a bit that isn't there). `tb task new --bit` sends `bits` (and `devices`) with the new task, so an unknown bit adds no task. |

## Reviewers

Each project has a roster of reviewers (`reviewers.rs` documents the tables). One row per person: commit emails,
host accounts and spellings fold into one, and any of them names the reviewer. A removed reviewer is never asked
(not by the board, nor by `tb pr reviewers --ask`) until they're back; a pinned one comes first for the main-contributor pick. Every
ask is a row in the ledger (`review_asks`).

`reviewer`: `{id, project, name, user: str|null (host id), emails, aliases, slack, source: "tb"|"git"|"host"|"import",
commits, removed, removed_at, removed_why, pinned, automation, bot: {every_h, mark, last_run, next_run}|null,
median_work_mins, open_asks, asks, swaps, last_asked}`. `asks`, `swaps` and `last_asked` count the old board's too
when the reviewer came over with `taskboardd import`.

`ask`: `{id, user, name, why: "pick"|"ask"|"replace"|"swap"|"fill_in"|"stage"|"rereview", by, state:
"open"|"answered"|"swapped"|"came_back"|"dropped"|"closed", asked_at, answered_at, answer, work_mins, replaces, nudged_at,
replied_at, reply}` (the last three from an imported ask: when it was nudged and the reviewer's reply to the nudge).

Every POST takes `project` (or `cwd`, the folder it's run in) and `reviewer` (any name of theirs), plus `who`.

| Request | Body | What |
|---|---|---|
| `GET /reviewers` | query `project` (`all` for every project) or `cwd` | `{project, projects: [{name, reviewers: [reviewer]}]}`. |
| `POST /reviewers` | `{reviewer: name, user?, emails?: [], aliases?: [], slack?}` | `tb reviewers add`: adds them, or folds what's new into the reviewer they already are (a shared host id, email or alias, or the same name unless both have their own, different host ids; two rows that both match become one). Commit authors from a sync fold on a shared name only when one side is just a name. |
| `POST /reviewers/remove` | `{reason?}` | Never ask them (`tb reviewers remove`). |
| `POST /reviewers/back` | | Ask them again. |
| `POST /reviewers/pin` | `{on?: bool}` | First in line for the main-contributor pick (`tb reviewers pin`/`unpin`). |
| `POST /reviewers/auto` | `{level: "off"\|"low"\|"normal"\|"high"\|number}` | How automated their reviewing is (0.25, 0.5, 1, 2, or 0.1–10): a weight on their turn. |
| `POST /reviewers/bot` | `{every_h, mark}` or `{off: true}` | Their review bot's schedule and the text its comments carry. |
| `POST /reviewers/alias` | `{aliases?: [], user?}` | More names, emails or host ids of theirs (409 for one that names someone else). For someone without a host account, an alias that's clearly one (`@login` or `{uuid}`) becomes it; a bare word (a nickname or a login) stays a name. `user` (`tb reviewers alias <who> --user <login>`) sets their host account outright, and one they had stays as an alias. |
| `POST /reviewers/merge` | `{other}` | Fold `other` into them: names, accounts, asks and bot runs. |
| `POST /reviewers/sync` | | `tb reviewers sync`: commit authors of the last `history_months` with at least `min_commits` commits (added up per person: emails of one reviewer, or one author name with one work email plus noreply ones) join the roster now (a GitHub noreply email gives their login), and the host's members (`PrHost::members`) give reviewers without an account theirs, matched by name. The picker does this itself at most every `sync_every_hours`. **Response:** `{project, joined, matched, reviewers: [reviewer]}`. |

**The picker** (`picker.rs`, `[reviewers]` in config.toml) asks one main contributor (whoever's turn comes first of
`main_contributors` people: pinned reviewers first, then those with the most commits to the files the PR changes,
then to the project; only people it could ask, on the PR already or not (not removed, not the author, not a bot
that isn't due), at a weight of `not_a_main_below` (0.05) or more and not out on Slack, take a place; skipped when
one of them is already on the PR), then the rest in turn, `count` in all. It never picks
the PR's author (nor `[reviewers] me`, nor the repo's `git config user.email`), anyone removed, anyone without a
host account, or anyone already on the PR or swapped off it. Turns: a reviewer is due at their last ask + (1 + open
asks) × `turn_gap_hours` / weight, earliest first (ties: fewest asks, then pinned, then most commits to the changed
files, then to the project); weight = automation × speed, where speed comes from the median work minutes they took
to review (`speed_by_minutes`, `slow_speed`, `no_speed_yet`, and `too_slow` for a median of `slow_cap_mins` or
more) over their last `speed_asks` asks of the last `speed_days`: an ask swapped off counts as `slow_cap_mins`, and
one still open counts its time so far (up to the cap) once it's past `swap_after_mins` or slower than the rest; until
then it takes none of the `speed_asks` places. While an ask has been open past `swap_after_mins`, their speed is
`slow_speed` at most. Each review sweep the ledger
marks an ask answered when its reviewer has reviewed (`answer`, `work_mins`: minutes inside the work hours, or every
minute with the hours off), and closed when the PR merged or closed first. The sweep runs every `[intervals] reviews`
seconds (60) on its own timer, whether or not the PRs are polled: with a healthy feed the poll rests, the sweep doesn't.

**Availability** (`presence.rs`, optional: `[reviewers] availability = "slack"`). During the board's work hours the
picker checks candidates, in turn order and at most `pick_tries` per pick, with Taskboard's Slack account: tiers
online (active, or posted today, or away before `quiet_from` where they are) > quiet > off (outside
`local_start`–`local_end` in their Slack time zone, or a weekend). A status matching `out_pattern` is out and never
picked. People are found by their `slack` id or email, then their other emails (not noreply ones), then by full
name, then by their host login or the part of an email (or a noreply email's login) before the @, matched to
Slack's emails and handles (`users.list`). Someone none of those finds leaves the roster ("not on Slack") with
`drop_not_on_slack`; when Slack refuses the user list, or a name or prefix fits more than one person, they're
unknown and nobody is dropped.
It takes the first one online, else the best tier it saw. Outside work hours, or without a provider, nobody is
checked, but an out status seen within `out_keeps_hours` still keeps them from being picked. The client only
calls `users.lookupByEmail`, `users.list`, `users.info`, `users.getPresence` and `search.messages`: the board never
messages anyone.

**Review bots** (`botrun.rs`). A reviewer with `bot: {every_h, mark}` runs their own review bot. Each review sweep the
board reads every comment (whole: a marker in a footer counts) on the repo's `bot_scan_prs` (20) most recently updated
PRs, whoever opened them (`PrHost::recent_comments`, at most every `bot_scan_mins`, 10, counted from the last read
that worked: a failed read is tried again on the next sweep), and records each comment of theirs that carries `mark`
(case-insensitive) from the last `bot_window_hours` as a run in `reviewer_bot_runs`. A comment within
`bot_run_gap_mins` of another comment of a run is part of it (they chain gap to gap, so a slow run that keeps
commenting stays one run, and a comment between two runs joins them); a run is at its earliest comment's time,
whatever order the host lists them in (Bitbucket lists the newest first). Until a run is seen, that person isn't asked.
Then the bot is timed: its next run is the last + `every_h`, rolled forward by `every_h` until it's in the future
(the reviewer's `bot.next_run`); the picker asks that person only when it's at most `bot_due_mins` away, and their
pace is the fastest in `speed_by_minutes`.

**The sweep** (`asks.rs`, every `[intervals] reviews` seconds) applies the stand-in rules to asks swapped off an open PR (by a swap): someone swapped off who reviews anyway is `came_back` (their review counts again: they
leave `pr_flow.swapped_off`), and a stand-in who hasn't reviewed yet is taken off the PR (`dropped`). Someone swapped
off who asks for changes doesn't block the PR (`prflow::review_of` waives anyone in `swapped_off`), and one more
reviewer is asked (`fill_in`, once per ask) while the PR has fewer than `[reviewers] count` on it. An open ask whose
reviewer was taken off the PR on the host (a read after the ask no longer lists them) is closed as `dropped`, so
nobody stands in for them. With `[reviewers] swap = true` (per project: `[pr.projects.<name>] swap`, or `tb project
set <name> --swap on|off|default`, which `taskboardd import` turns on for every project that came over with a PR or a
review ask: the old board always swapped), an ask still open after
`swap_after_mins` work minutes on a PR waiting for review is replaced through the host (`PrHost::replace_reviewer`)
by the picker's choice (`swap`): only inside work hours and never while `feed::holding` (the event feed's
health gate) says to hold; the stand-in rules wait for it too, and so does the board's own ask at the `ask` stage,
except a PR's first ask outside the feed's hours. Each change is logged on the task and the PR is read again. The
sweep doesn't take a reviewer's state from a PR read made before their ask (`pr_flow.checked_at` earlier than
`asked_at`), nor a `rereview` ask's request for changes while the PR still shows the one `tb pr addressed` answered: that
reviewer's own (`reviewers[].changes_at` against `pr_flow.answered_changes_by`, `{host id: when}`), else the PR's
latest (`pr_flow.answered_changes`). The old review isn't an answer to the new ask, and another reviewer's new
request for changes doesn't make it one.

**The `ask` stage** (`[reviewers] ask_stage`, off by default; per project `[pr.projects.<name>] ask_stage` or
`tb project set <name> --ask-stage on|off|default`, which `taskboardd import` turns on for a project where the old
board used the stage). Once the owner has marked a green PR reviewed
(`POST /tasks/:id/pr/reviewed`, "I reviewed it"), its phase is `ask` until reviewers are asked (`pr_flow.asked`).
In work hours, with `pr.wake` on, the agent is brought back to run `tb pr reviewers` (which finishes the visit);
otherwise the board picks and asks them itself through the host (asks with `why: "stage"`, by "Task board"),
retrying after each of `ask_retry_waits` seconds (`pr_flow.ask_tries`, `ask_retry_at`) and alerting once they're
spent. Then the phase moves on to `review`. While the feed holds (`feed::holding`), the agent isn't brought back to
ask and the board doesn't ask; the sweep brings the agent back once the feed has settled. `tb pr reviewers` (besides
`--dry-run`) answers 409 while the feed holds, and, with the stage on, when it would ask someone before the owner
has reviewed the PR and before anyone was asked on it, whatever the task's status (a reopened task waits too); a
later `--replace` or `--drop` doesn't wait for the owner. Asked on it counts the host too: anyone on the PR's
reviewer list or who reviewed it (not the board's own account). Nor does it wait once the PR is past asking
(`rereview`, `merge`, `waits`, `merged`, `declined`).

The app's Settings ▸ Reviewers lists each project's roster; it changes nothing.

## The PR plan and flow

**Stacked PRs** (`tb task new|set --stack-on T<n>`, stored as `tasks.pr_after`). The task waits for its parent like
`waits_for` (any goal, same project, no cycles). While the parent's PR is unmerged: its worktree starts from
`origin/<parent branch>`, the handoff says to cut its branch from there and open the PR into it, `{base}` is the
parent's branch, and once its PR is approved and green its phase is `waits` ("Waits on base") instead of `merge`.
When the parent's PR merges (the watcher sees it, or `POST /tasks/:id/pr/merged`), the board points each open stacked
PR at the parent's base through its host (`PrHost::retarget`, GitHub or Bitbucket) once, and logs it. A failed move
is kept in `pr_flow.retarget_error` and tried again on later refreshes after 1, 5, 15, then every 60 minutes
(`retarget_at`); its alert (key `retarget:T<n>`) is raised once and clears when the move works. The stacked task is told to
rebase as for any `waits_for`. A task others still stack on (open, or done with their PR open) can't be set to end
without a PR (`--pr no`, `tb done --no-pr`): the 409 names them.

**The PR plan** (`tasks.ships_pr`): `tb task new --pr|--no-pr`, `tb task set --pr yes|no|auto`. The handoff and steps
use it instead of the project's default.

**`tb done --no-pr "<why>"`** (a task that would end in a PR, with none linked): stores `tasks.no_pr`, logs "PR
canceled: <why>", skips the before-the-PR steps, and with `[jira] canceled` set moves the ticket there with the reason
as a comment. The why can't be blank (400) and is one line of at most 100 characters (400 past that). **`--no-evidence
"<why>"`** stores `tasks.no_evidence`. The app shows both with task refs as buttons and ticket keys linked to Jira; the
goal row says "PR canceled: <why>" (its task and ticket refs linked) and the wave rail's "PR canceled" chip carries
the why.

**`tb done` refusals** (409, before anything opens): a task that ends in a PR (`ships_pr`), unless its repo has
no remote at all (one the board can't read counts), with no PR linked and no `pr`, `pr_body` or `no_pr` in the report; and a task with a `design` attachment and
no `evidence`/`results` attachment, without `no_evidence`. **Evidence on the PR**: each PR refresh adds the task's
evidence and results links (web links) that its open PR's description lacks, under `## Context`, through
`PrHost::description` / `set_description`; `pr_flow.evidence_added` keeps the ones added, and a host error waits 30
minutes (`evidence_retry_at`). A failed fetch in `--pr-body`'s branch check shows the whole of the error.

**`tb done "<summary>" --pr-body FILE [--title …]`**: the report carries `pr_body` (and `pr_title`). Before finishing,
the board checks the description against `[pr_body]` (sections in order, bullet lists, paragraph length, no board
refs, `forbid` patterns), the before-the-PR and before-done steps, and the branch (pushed as it is, rebased on
`<remote>/<base>`, no merge commits, a ticket if `require_ticket`), then opens the PR into the real base with the
title after the ticket key and a `## Context` section (the ticket, the PR it stacks on, the evidence and results: web
links, and local files by name; then the design links under **Design**), links it and finishes. Errors come back as 400/409 with what
to fix; nothing is spooled. It opens on GitHub or Bitbucket Cloud by the checkout's remote, through the `prhost` interface (`propen::host`; a remote naming neither falls back to `gh pr create`); `propen::host` is the one place that talks to the
host. `tb pr body-check FILE` runs the description check alone.

**Steps with rounds** (`[[steps]]`): `per_head = true` passes only for the head commit it ran on (`GET
/steps?head=<sha>` judges `done` on that commit; the `PreToolUse` hook also holds `git push` to an open PR until
it passes on the commit being pushed); `min_gap_mins` keeps rounds apart (`next_round_at` in `GET /steps`, and `tb`
refuses an earlier round); `bar = "WD"` puts it in the PR bar. A check or script may write
`{"verdict": "pass"|"fail"|"skip", "headline": str, "findings": [{"id", "title", "severity", "state", "file", "line",
"detail", "url"}]}` to `$TASKBOARD_RESULT`; the verdict overrides the exit code, and `skip` (a round that couldn't
review or didn't finish) never blocks, and neither it nor a round stopped at its timeout counts for `min_gap_mins`.
`tb step triage "<step>" F2 --state fixed|answered|dismissed|open [--note …] [--commit <ref>]` answers a finding
(report `tb.step_triage`; `tb` resolves the ref to its sha); `tb step done|again|run "<step>" [--branch B | --worktree DIR]
[--commit REF]` runs another round, on what it names (the report's `head` is the resolved sha; a commit other than the
checkout's head runs on a throwaway checkout of it). `tb step aim --branch B | --worktree DIR [--commit REF]` (report
`tb.step_aim` with `worktree`, `branch`, `sha`, `tip`; `clear: true` drops it) saves the aim on the task: `GET /steps` returns it
as `aim` (null when unaimed) and judges `done` and `head` on its commit, and so do the `tb done` and PR-opening gates. A
pinned `sha` keeps its branch's `tip` at the time (`tb` sends it; the board reads it when left out): once the branch's tip
moves, the pin no longer counts (`aim` has no `sha`, and `dropped: <sha>`; the aim follows the branch), and the next
`tb.step` report drops it from the task with a "Rounds no longer pinned at …" line. `tb` refuses a detached
`--worktree` (and a `--commit` from a detached checkout, and, with no aim, any round from a detached checkout) unless `--branch` names its branch (`--worktree` and `--branch` go together), and the commit must be
on it. A detached checkout aimed with `--branch` is judged on that branch's tip (refused when the branch is missing
or doesn't contain the checkout's head). A pin with no `tip` (saved before the board kept it) is dropped once the branch's tip isn't its sha. Prompts'
`{branch}` is the aimed branch; a placeholder with no value yet is empty (in a script, each value is one shell word,
`''` when empty, and `tb steps`, refusals and the handoff show `run`, `check` and `publish` quoted that way), and refusals, the handoff and `tb step ask`'s question show the steps filled, `{head}` and `{worktree}` included (`GET /steps`'s
`vars` carries them too). `tb.step_ask` takes `head`, `branch` and `worktree` (what `tb` resolved, as for `tb.step`): the
question is filled from them, and they're kept on the task (`context.step_asked`) as the placeholders' fallback when the
board has no aim, recorded branch or head, so a later take's handoff isn't blank. A step's
`publish` script (`tb step publish "<step>"`, which needs the step passed on the head and the task's PR; report
`tb.step_publish` with `name`, `head`, `ok`, `output`, logged as "Published <step> for <sha>" or "Couldn't publish …")
republishes its passing round for the PR, with that round (the one that passed on the head, from `passed_rounds` in
`GET /steps`: `{<step name>: step_result}`, each step's latest passing round on the judged head) as `$TASKBOARD_RESULT`.
A `tb.*` report's `name` is the step or goal it's about, never the terminal's name. A `[[steps]]` entry
that can't be done is left out alone, with an alert keyed `steps:<name>` until it's fixed. `pr.bar.wd` is the `bar`
step's latest `step_result` on cards as well as in the task detail.

`step_result`:
```
{"name": str, "bar": str|null, "at": iso, "head": str|null, "passed": bool, "verdict": "pass"|"fail"|"skip"|null,
 "stale": bool,               // a per-head step whose head has moved since this round ("Moved since")
 "headline": str,             // the tool's, else "No findings" / "2 open findings" / "Answered, not approved" / "Couldn't review this round" / "Didn't finish"
 "open": int, "rounds": int,
 "findings": [{"id": "F2", "title": str, "severity": str|null, "state": str|null, "file": str|null, "line": int?,
               "detail": str|null, "url": str|null, "note": str?, "commit": str?}]}   // open first, then by severity
```
