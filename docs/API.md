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
  "can_take": bool,            // idle and no task: offered in New task → "In an idle terminal"
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
  "setup": str|null,           // what every task in the goal does first (`tb goal setup`); its handoff shows it with {task} {n} {wave} {goal} filled
  "total": int,        // tasks in the goal, planned included
  "done": int,         // tasks with status done (failed included)
  "active": int,       // working + needs
  "needs": int,        // status needs
  "queued": int,       // status queued
  "starting": int,     // queued with a live start job
  "blocked": int,      // queued and waiting on another task (waits_for)
  "held": int,         // queued, not blocked, but held back by its waves or order, a lock, a bit or the device pool
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
  "waiting": str|null,            // queued only: why it isn't starting yet, one plain line
                                  // ("Waits for T4 to finish", "Waits for work hours (tomorrow 6am)", "Waits for the 5-hour usage to reset (3pm)")
  "blocked": bool,                // queued and waiting on another task (waits_for), shown as "Blocked"
  "waits_for": ["T14"],           // tasks it starts after
  "waits_for_state": [{"ref": "T14", "done": bool}],   // the same, each with whether it's done; the goal page's "Waits for" chip
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
  "stack_on": stack_on|null       // the task whose PR this one's builds on (`--stack-on`)
}
```
`stack_on`: `{"ref": "T3", "title": str, "num": int|null, "url": str|null, "branch": str|null, "merged": bool, "line": "Stacks on T3's PR #12"}`.
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
    "phase": "checks"|"fix"|"review"|"rereview"|"comments"|"merge"|"merged"|"declined",
                                 // rereview: changes were asked and are pushed; waiting for that reviewer to look again ("Awaiting re-review")
    "label": str,                // plain words, e.g. "Watching checks", "Fixing checks", "Awaiting reviews", "Answering comments", "Merging", "Merged"
    "session": str|null,         // optional: id of the terminal the board woke for fix/comments/merge (links to it)
    "stopped": {"asked": bool, "message": str} | null
                                 // optional: that terminal stopped before finishing (asked = it asked you a question).
                                 // Shows "Needs you" and an answer box on the done task; the answer goes through POST /tasks/:id/answer.
    "open_threads": int,         // review threads (and Bitbucket PR tasks) waiting on us: someone other than the board's own host account (the PR's author when that isn't known) spoke last
    "not_ours": {"checks": [str], "title": str, "reason": str, "proof": [url], "who": str, "at": iso} | null
                                 // this push's failed checks cleared with `tb pr not-ours`; the task panel's PR bar shows
                                 // a "Failed, but not because of this PR" box with the title, checks, reason and proof links
  } | null,
  "bar": {                       // on cards from this board (absent in the frozen web fixtures)
    "build_url": str|null,       // the failed check's link, else the first check's
    "checks": "not_needed"|"not_ours"|null,   // no checks at all / this push's checks skipped or its failures cleared (`tb pr not-ours`)
    "checks_why": str|null,      // why they were skipped
    "you": "waiting"|"reviewed"|"skipped"|null,   // the owner's own look: a green PR waits for them / they marked it / review skipped
    "approvals": int, "reviewers": int,           // "1 of 2": approvals of (approvals + reviewers still asked)
    "new_comments": int,         // open threads waiting on the author (older reads: comments since the agent last handled them)
    "waits_on_base": bool,       // phase `waits`: a stacked PR waits for the PR it builds on to merge
    "stacks_on": stack_on|null,
    "retargeted": str|null,      // the base the board pointed it at once its parent merged (or "failed: …")
    "wd": step_result|null,      // the author-side review step (a step with `bar`), on GET /tasks/:id only
    "reviewer_rows": [{"name": str, "user": str, "state": "approved"|"changes"|"rereview"|"waiting"|"commented", "swaps": int}]
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
`review` free text, `review_log`, reviewer lists (now `bar.reviewer_rows`),
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
| `POST /tasks` | `{title, detail, project, priority: "normal"\|"high", goal_id: int\|null, auto_close: bool, pickup: {mode: "queue"\|"new"\|"attach"\|"manual", session_id?}, status?: "planned", jira?: {mode: "create"\|"link"\|"none", key?}}` | `tb task new`. `status: "planned"` only when it has a goal ("Add it to the goal's plan"). `jira` only sent when `state.jira.enabled`. **Response read:** the task (`ref` or `id`), then the caller shows it. |
| `POST /tasks/:id` | `{status: "queued"}` | "Queue it now" on a planned task (planned → queued only). |
| `POST /tasks/:id` | `{jira_key: "PROJ-1"\|"new"\|"none"}` | `tb task set --jira`: link a ticket, ask for one (again, after a failure), or no ticket (a PR task then stops waiting for one). |
| `POST /tasks/:id/start` | `{mode: "new"\|"queue"}` | Start / Start when the repo's free / New Midna terminal / Queue in Midna / drag to Working (`new`). A goal with a `worktree_base` makes the task's worktree first; one that can't be made answers 409. |
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

### PRs (`tb pr …`)
The board watches GitHub PRs (through `gh`) and Bitbucket Cloud PRs (REST 2.0, with Taskboard's Bitbucket account);
`prhost.rs` documents the host interface. Each route below reads the PR from its host first and judges what it says
now. A host that can't be reached answers 502; a refusal answers 409 with the reasons.

| Path | Body | Notes |
|---|---|---|
| `GET /tasks/:id/pr` | `?full=1` | The card, the last read (`record`) and `watched`. `full=1` (`tb pr status`) reads it now and adds `live`: `base_moved`, `builds_note`, `failures: [{check, url, steps, tests, source, error?, base_fails, cleared}]` (failed steps and tests from GitHub Actions, Bitbucket Pipelines, Azure Pipelines or the project's `failures_cmd`; `base_fails` when the base branch's last 5 commits fail that check too), `not_ours`, `expected_missing`, `reviewers: [{user, name, state: approved\|changes\|commented\|pending, requested, swapped_off}]`, `approvals: {have, need}`, `open_threads: [thread]`, `tasks_open` (null when they couldn't be read), `tasks_error` (why; the merge waits until they can be), `blockers: [str]` (why `tb pr merge` would refuse), `read_error`. |
| `POST /tasks/:id/pr/reply` | `{thread, text, resolve?: bool, who?}` | `tb pr reply`: answers the thread on the host (a GitHub comment that has no thread gets a quoting comment); `resolve` resolves it too. |
| `POST /tasks/:id/pr/ack` | `{thread, who?}` | `tb pr ack`: a thread that asks for nothing is resolved without a reply (on the board only, where the host can't resolve it). The ack holds until someone writes on the thread again. |
| `POST /tasks/:id/pr/addressed` | `{who?}` | `tb pr addressed`: 409 while threads are open; then asks each reviewer with a standing request for changes (not one swapped off) to review again, and ends the visit (stage `rereview`). **Response:** `{asked: [name]}`. |
| `POST /tasks/:id/pr/merge` | `{who?, agent?: bool}` | `tb pr merge`: 409 unless it's open, its checks passed (failures cleared as not-ours aside; expected checks posted; none running or stopped), it has the approvals it needs, nobody (still on it) asks for changes, no thread or PR task is open (and its PR tasks could be read), and a stacked base PR has merged. Then points every open PR that goes into its branch at its base (a PR that can't be moved stops the merge: 502), merges with the project's `merge_strategy` (else the repository's default) and deletes the source branch. 403 from an agent (`agent: true`) while `pr.agents_merge` is off. |
| `POST /tasks/:id/pr/not-ours` | `{reason, title, proof: [url], checks?: [str], who?}` | `tb pr not-ours`: clears failed checks of the current head (all of them, or `checks`) that aren't the PR's fault. `reason` 20–300 characters, `title` up to 80, at least one http(s) `proof` link. 409 when nothing failed on this push or a named check didn't fail. A new push has to pass on its own. |
| `POST /tasks/:id/pr/skip-checks` | `{reason?, all?: bool, who?}` | Counts this push's checks (or every push's) as passed: for builds a hook cancelled, not for failures (use `not-ours`). |
| `POST /tasks/:id/pr/wait` | `{}` | `tb pr wait`: the agent finished this visit. |
| `POST /tasks/:id/pr/merged` | `{who?}` | `tb pr merged`: the PR was merged outside the board. |

`thread = {id, kind: "review"|"comment"|"summary"|"task", resolvable, resolved, author, author_name, last_author, last_id,
last_at, path?, line?, text, url?, outdated?}`. A thread is open while it's unresolved and someone other than the
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
### Projects
| Path | Body | Notes |
|---|---|---|
| `GET /projects` | | Every known project with `remote: bool\|null`, `pr_flow: "auto"\|"on"\|"off"`, `ships_prs: bool`, `pr_rules: {approvals, expected: [str]\|null, expected_wait_mins, set: {…what tb project set changed}}` (`tb project show`). |
| `POST /projects/:name` | `{pr_flow?: "auto"\|"on"\|"off", approvals?: int\|null, expected?: [str]\|null, expected_wait_mins?: number\|null}` | `pr_flow`: whether the project's work ends in PRs; `auto` follows its git remote (`tb project set --pr-flow`). The PR rules (`tb project set --approvals N`, `--expected-check NAME` (repeat; `none` = `[]`), `--expected-wait MINS`; `default` sends null) change the project's rules on the board; null goes back to config.toml's. At least one key. **Response read:** the project as in `GET /projects`. |

### Goals
| Path | Body | Notes |
|---|---|---|
| `POST /goals` | `{name, tldr, outcome, project, run_in_order: bool, max_terminals: int, auto_close: bool, epic: {mode: "create"\|"link"\|"none", key?}}` | `tb goal new`. `epic.mode` is always `none` without Jira. **Response read:** the goal (`ref`/`id`, or `{goal: {...}}`); the caller shows it. |
| `POST /goals/:id` | any subset of `{name, tldr, outcome, project, run_in_order, max_terminals, auto_close, epic_key: str\|null, paused: bool, deprioritized: bool, setup: str\|"none"}` | `tb goal set` (`epic_key` only with Jira; `--deprioritize`/`--prioritize` set `deprioritized`), the goal page's "How this goal runs" (`max_terminals`, `run_in_order`, `auto_close`), Pause/Resume, Deprioritize/Bring it back. |
| `POST /goals/:id/run` | `{}` or `{now: true}` | `tb goal set --run`. Queue the planned tasks (and re-queue `start_failed` ones), clear paused/deprioritized. `now: true` outside work hours = let this goal run until the hours next open (`goals.hours_until`). **Response read:** `queued_now: int` (how many planned tasks it queued). |
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
| `POST /backlog/:id/move` | `{goal_id: int\|null}` | Move to another goal or none (`tb backlog move`; `goal_id` also takes a ref like `"G2"`). |
| `POST /backlog/:id/note` | `{text}` | Add a note to its history (no app button). |
| `POST /backlog/bulk` | `{ids: ["B1", …], action: "task"\|"ticket"\|"drop"\|"move"\|"reopen"\|"defer"\|"priority"\|"goal", where?: "goal", goal_id?: int\|null, priority?: "p1"\|"p2"\|"p3", reason?, who?}` | `tb backlog task\|ticket\|drop\|reopen B4 B5 …` (one or more issues; all change or none do), the Goal page bulk bar (Make tasks, Create tickets, Won't do) and the Backlog page's selection bar. `task` sends `where: "goal"`; `move` (no app button) sends `goal_id`. `defer` sets state `defer` (not for now; triaged). `priority` sets the issues' priority. `goal` puts them in that goal as planned tasks, in its last wave. `who` (tb sends the terminal) is credited in each issue's history and the new tasks' origin; without it, the owner. **Response read:** `count: int`, `tasks` (the tasks made). |
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
It answers like `GET`, and the board rewrites the `.git/info/attributes` blocks at once (else every 5 minutes).

- `compact_window`: board terminals' Claude gets `--settings '{"autoCompactWindow": n}'` (unless the job brings its own `settings`).
- `cold_idle_mins`: a conversation idle longer is compacted before it carries on: a headless `claude -p /compact --resume <id>`
  before a new terminal resumes it, or `/compact` queued ahead of the prompt in its live terminal.
- `warm_tokens`, `warm_idle_mins`: a task (after its wait-for) or a PR visit resumes its conversation only while it's under
  both; otherwise it starts fresh from the handoff and the history says why. A live terminal is always typed into.
  Size and idle time come from the conversation's transcript (its last reply's context, its last line's time).

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
`agent` job, purpose `jira_desk`) and never closes. Its `--allowedTools` are `claude_tools` plus `Bash(tb jira:*)` and
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
| `POST /goals/:id/waves/:n` | `{name?, stop_after?}` | Name a wave (`tb goal wave --name`) or make it a review stop. `stop_after` is the owner's own word: only Taskboard.app sets it (it sends `X-Task-Board-From: app`); from anyone else it's 403. Answers the goal detail. |
| `POST /goals/:id/waves/:n/hold` | `{on?: bool (true), who?}` | Hold a wave (`tb goal wave --hold`): its tasks that haven't started don't, nor any later wave, until it's continued. `on: false` lifts it. 409 on a done wave. Answers the goal detail. |
| `POST /goals/:id/waves/:n/continue` | `{who?}` | "Continue to wave N": go on past a review stop or a failed task; on a held wave that isn't done, let it start. Answers the goal detail. |
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
tasks that don't ask for their own; `"none"` clears). The runner starts it only once that many are free, lends them
when it starts (before the handoff is built, which names them), and takes them back once the task isn't active
(done, or a failed start), the same rule as locks. A queued task's `waiting` line says why ("Waits for a android
device (T4 has them)", "Needs 2 ios devices, and the pool has 1").

`device`: `{id, name, tags: [str], note, off: bool, focus: str|null, can_focus: bool, held_by: {ref, title, goal}|null}`.

| Request | Body | What |
|---|---|---|
| `GET /devices` | | `{devices: [device], waiting: [{ref, title, goal, needs, why}]}`. |
| `POST /devices` | `{name, tags?, focus?, note?}` | Add one. Names and tags are lowercase letters, digits, `.`, `_`, `-` (names also `:`). 409 when the name is taken. |
| `GET /devices/:name` | | The device. |
| `POST /devices/:name` | `{name?, tags?, focus?, note?, off?}` | Change it; `off` keeps it from being lent. |
| `POST /devices/:name/remove` | | Take it out of the pool (409 while it's lent). |
| `POST /devices/:name/focus` | | Raise its window: runs its `focus` command, else `[devices] focus`, with `sh -c` (`{name}` and `$TASKBOARD_DEVICE` are its name). 409 when there's neither. |

## Bits (feature flags)

A bit is `backend` (it has to be made in the flag tool) or `local` (in the code only), linked to tasks and goals. A
queued task waits while a backend bit linked to it isn't made ("Waits for the bit newCheckout to be made in
Flagsmith"); a goal whose tasks are all done waits on its unmade backend bits. The handoff lists a task's bits.
`[bits]` in config.toml names the tool and its "new flag" link.

`bit`: `{id, name, kind, project, note, made: bool, made_at, made_by, waiting: bool, create_url: str|null, tasks: ["T4"], goals: ["G2"], created_at}`.

| Request | Body | What |
|---|---|---|
| `GET /bits` | query `goal`, `task` or `project` | `{bits: [bit], tool, create_url}`. |
| `POST /bits` | `{name, kind: "backend"\|"local", tasks?: ["T4"], goals?: ["G2"], project?, note?, who?}` | Add one (`tb bit add`). The project defaults to its first task's or goal's. |
| `GET /bits/:name` | | The bit. |
| `POST /bits/:name` | `{name?, kind?, note?, project?, tasks?, not_tasks?, goals?, not_goals?, who?}` | Change it or its links (`tb bit set`). |
| `POST /bits/:name/made` | `{undo?: bool, who?}` | It's made in the flag tool, or with `undo` it isn't (`tb bit made`). Never from the app. |
| `POST /bits/:name/remove` | `{who?}` | Remove it and its links. |
| `POST /tasks/:id` | `{bits: ["newCheckout"]\|"none", not_bits: [...]}` | Link a task to bits (404 for a bit that isn't there). |

## The PR plan and flow

**Stacked PRs** (`tb task new|set --stack-on T<n>`, stored as `tasks.pr_after`). The task waits for its parent like
`waits_for` (any goal, same project, no cycles). While the parent's PR is unmerged: its worktree starts from
`origin/<parent branch>`, the handoff says to cut its branch from there and open the PR into it, `{base}` is the
parent's branch, and once its PR is approved and green its phase is `waits` ("Waits on base") instead of `merge`.
When the parent's PR merges (the watcher sees it, or `POST /tasks/:id/pr/merged`), the board points each open stacked
PR at the parent's base through its host (`PrHost::retarget`, GitHub or Bitbucket) once, logs it, and alerts if it couldn't; the stacked task is told to
rebase as for any `waits_for`.

**The PR plan** (`tasks.ships_pr`): `tb task new --pr|--no-pr`, `tb task set --pr yes|no|auto`. The handoff and steps
use it instead of the project's default.

**`tb done --no-pr "<why>"`** (a task that would end in a PR, with none linked): stores `tasks.no_pr`, logs "PR
canceled: <why>", skips the before-the-PR steps, and with `[jira] canceled` set moves the ticket there with the reason
as a comment. **`--no-evidence "<why>"`** stores `tasks.no_evidence`. The app shows both with task refs as buttons
and ticket keys linked to Jira.

**`tb done "<summary>" --pr-body FILE [--title …]`**: the report carries `pr_body` (and `pr_title`). Before finishing,
the board checks the description against `[pr_body]` (sections in order, bullet lists, paragraph length, no board
refs, `forbid` patterns), the before-the-PR and before-done steps, and the branch (pushed as it is, rebased on
`<remote>/<base>`, no merge commits, a ticket if `require_ticket`), then opens the PR into the real base with the
title after the ticket key and a `## Context` section, links it and finishes. Errors come back as 400/409 with what
to fix; nothing is spooled. It opens on GitHub or Bitbucket Cloud by the checkout's remote, through the `prhost` interface (`propen::host`; a remote naming neither falls back to `gh pr create`); `propen::host` is the one place that talks to the
host. `tb pr body-check FILE` runs the description check alone.

**Steps with rounds** (`[[steps]]`): `per_head = true` passes only for the head commit it ran on (`GET
/steps?head=<sha>` judges `done` on that commit; the `PreToolUse` hook also holds `git push` to an open PR until
it passes on the commit being pushed); `min_gap_mins` keeps rounds apart (`next_round_at` in `GET /steps`, and `tb`
refuses an earlier round); `bar = "WD"` puts it in the PR bar. A check or script may write
`{"verdict": "pass"|"fail"|"skip", "headline": str, "findings": [{"id", "title", "severity", "state", "file", "line",
"detail", "url"}]}` to `$TASKBOARD_RESULT`; the verdict overrides the exit code, and `skip` (a round that couldn't
review or didn't finish) never blocks. `tb step triage "<step>" F2 --state fixed|answered|dismissed|open [--note …]
[--commit <sha>]` answers a finding (report `tb.step_triage`); `tb step again "<step>"` runs another round.

`step_result`:
```
{"name": str, "bar": str|null, "at": iso, "head": str|null, "passed": bool, "verdict": "pass"|"fail"|"skip"|null,
 "stale": bool,               // a per-head step whose head has moved since this round ("Moved since")
 "headline": str,             // the tool's, else "No findings" / "2 open findings" / "Answered, not approved" / "Couldn't review this round" / "Didn't finish"
 "open": int, "rounds": int,
 "findings": [{"id": "F2", "title": str, "severity": str|null, "state": str|null, "file": str|null, "line": int?,
               "detail": str|null, "url": str|null, "note": str?, "commit": str?}]}   // open first, then by severity
```
