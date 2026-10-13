# Work event stream, protocol v1

The board defines this format; a work relay (anything that hears Bitbucket, GitHub, git, CI or Jira) adapts
to it. An event carries every field the board needs, so the board never has to call the host to finish one.

```
POST /stream/events          (tb stream send '<json>'   or   … | tb stream send)
```

## Envelope

```json
{"v": 1, "events": [{"id": "bb-8812", "kind": "pr", "at": "2026-10-12T15:04:00Z", "source": "work-relay", "...": "..."}]}
```

- `v` is required and must be `1`. Any other version is refused (400) and nothing is read.
- `events` holds 1 to 500 events. A body without `events` but with a `kind` is read as a single event:
  `{"v": 1, "id": …, "kind": …}`.
- Every event has these fields:
  - `id`: a string of 1 to 200 characters, unique to that event. Sending the same id again is a duplicate.
  - `kind`: what kind of event it is (see Kinds).
  - `at`: when it happened, as ISO 8601.
  - `source`: the relay or feed it came from.

## Reply

Each event is checked and answered on its own, in the order sent:

```json
{"v": 1, "accepted": 1, "duplicate": 1, "rejected": 1, "results": [
  {"id": "e1", "kind": "pr", "status": "accepted", "complete": true, "result": {"task": "T12", "refreshed": true}},
  {"id": "e1", "kind": "pr", "status": "duplicate"},
  {"id": "e2", "kind": "push", "status": "rejected", "why": "branch is missing."}]}
```

- `accepted`: the event is valid. It's kept and routed (see Routing), and `result` holds what the route said.
- `duplicate`: an event with this id was accepted before, possibly earlier in the same batch. Nothing happens.
- `rejected`: `why` gives the reason. Nothing is kept, so the event can be fixed and sent again.
  An event its route refuses (for example, a 400 from the PR feed) is rejected the same way.

`tb stream send` prints one line per event and exits with 1 when any event was rejected.

## Kinds

A field marked `?` may be left out or set to null. Times are ISO 8601. A PR reference (`pr`) is
`{provider?, repo, num, url?}`; either a `url` the board recognizes (GitHub, GitLab or Bitbucket), or `repo` and
`num`, is enough.

### `pr`: a pull request's whole state

| Field | |
|---|---|
| `provider` | `bitbucket`, `github`, `gitlab` |
| `repo`, `num`, `url` | `acme/webapp`, `9`, its link (`url`, or `repo` + `num`, is required) |
| `title`, `author` | |
| `state` | `open`, `merged`, `declined` |
| `draft`, `mergeable` | bool (`mergeable` is null while unknown) |
| `branch`, `base`, `head` | source branch, target branch, head commit sha |
| `checks` | `[{name, state: passed\|failed\|running\|stopped, url?, started_at?, finished_at?}]` |
| `reviewers` | `[{who, state: approved\|changes\|commented\|pending, at?}]` |
| `threads` | `[{id, path?, line?, resolved?, comments: [{id, author, body, at}]}]` |

A pr event is **complete** only when it carries every field in this table; null counts as present. An
incomplete pr event is still accepted, but it's kept with `complete: false`, and the board reads that PR from its
host instead of trusting the event. In board mode (see Modes) every pr event is only a hint: the board reads the PR
from its host either way.

### `review`

`pr`, `who`, `state` (`approved`, `changes`, `commented`, `dismissed`), and `body?`. The event's `at` is the
time of the review.

A review or comment on a PR the board has is a hint: the board reads that PR from its host, as it does for an
incomplete pr event, so the stage changes without waiting for the next PR poll.

### `comment`: a PR comment

`pr`, `comment_id`, `author`, `body`, and optionally `thread_id?`, `path?` and `line?` (a line number of 1 or more).

### `push`

`repo`, `branch`, `after` (the new head sha), and optionally `before?`, `author?`,
`commits?: [{sha, message?, author?}]`, `forced?: bool` and `mine?: bool`.

### `build`

| Field | |
|---|---|
| `repo`, `head` | the repo and the commit built (required) |
| `state` | `queued`, `started`, `running`, `passed`, `failed`, `stopped` (required) |
| `branch?`, `pipeline?`, `author?` | |
| `mine?` | `true` when the relay knows the build is the owner's |
| `pr?` | the PR the build is for, if any |
| `url?` | the build's link |
| `build_id?` | the CI's id for the build (else the last number in `url`) |
| `provider?` | the CI that ran it |

A `build` or `push` is the owner's when `mine` is `true`, or its `author` is one of `owner_emails` or
`[reviewers] me`, or another name, email or host id of a roster row that names one of those (its aliases), in
any case. A display name, a username or an account id all match once they're in `me` or on the owner's roster row.

### `jira.issue`: an issue changed

`key` (`PROJ-123`) and `status` are required. The rest are optional: `summary?`, `type?`,
`status_category?` (Jira's `new`, `indeterminate` or `done`), `from_status?`, `assignee?`, `reporter?`,
`changed_by?` and `epic?`. The board trusts the status in this payload.

### `jira.comment`

`key`, `comment_id` (Jira's numeric id), `author` and `body` are required. `visibility?` is optional.

### `heartbeat`: the relay is alive

`sources?` (some of `prs`, `builds`, `jira` and `qa`), `active?: bool`, `idle_until?`, `connected_at?` and
`covers?: [{provider, repo?}]`. The first four follow `POST /prs/heartbeat`. While the relay says it's idle
(until `idle_until`, if given), the board doesn't count it as stuck or silent. `covers` says which PRs the relay
hears about: each entry a provider (`github`, `bitbucket`) and, optionally, one repo (`acme/webapp`, or its short
name). Left out, the relay covers every PR. Each heartbeat replaces what the last one said it covers.

## Routing (v1)

| Kind | Source | Goes to |
|---|---|---|
| `pr` | prs | the PR feed, as `POST /prs/event` does: the task's PR is read and stepped (in stream mode a complete event is applied instead, see Modes) |
| `build` | builds | the PR feed, as `POST /prs/event` does, and the owner's PR builds are run; a build on the branch of a project under `[master.projects]` (its repo and branch) also goes to the master-break watch, as `POST /master-builds` does, with `head` as its `sha` and `at` as its `ts` (the watch keeps the last green head itself), and its `result` is `{feed, master}` |
| `heartbeat` | each in `sources` (all when left out) | that source's health, as `POST /prs/heartbeat` does for prs |
| `jira.comment` | qa | QA comments, as `POST /jira/comment` does |
| `jira.issue` | jira | the task, goal epic and backlog issue with that key, see below |
| `review`, `comment` | prs | a hint, like an incomplete pr event: a PR on the board is read from its host and stepped (in either mode), and the result names its task; any other PR only counts for the source's health |
| `push` | builds | the owner's PR builds, as `POST /pr-builds/event` does: a push to the branch of a PR on the board or of one of the owner's open PRs opened by hand, or a push by the owner, is a started build on `after`; other pushes are kept only |

### Ticket statuses

A `jira.issue` event, in either mode, sets the Jira status of every task, goal epic and backlog issue with that
key, and logs who moved it (`changed_by`, else "Jira") in the task's or issue's log. A status that's already the
stored one changes nothing. The board skips a status it caused itself: a Jira job in the last 15 minutes that
moved that ticket to that status. It never changes a task: when an open task's ticket moves to done, or a done
task's ticket is reopened, it raises an alert instead. Done means `status_category` is `done`, or, without one,
a status named like `[jira] done`, `merged` or `canceled`, or Jira's usual done names (Done, Closed, Resolved…).
A done task's ticket counts as reopened only when it moves back to a new status (category `new`, or, without
one, To Do, Open, Backlog…), or from a really done status (`done`, `canceled` or Jira's usual done names) to a
status that isn't done. `merged` hands the ticket on to QA, so QA moving it on from there (In QA, QA Passed…)
raises nothing.

These alerts (keyed `jira:<task>:<status>`) stay until the task and its ticket agree again: a reopen alert
until the task is reopened or the ticket is back in a done status, a "still open" one until the task finishes
or the ticket leaves the done statuses. The task being done, or another Jira move, doesn't clear them; the
owner can still dismiss them.

The board keeps when each stored status was true (`jira_status_at` on tasks, goals and issues): the event's
`at`, or the time of the board's own move or the QA poll's read. An event older than the stored status changes
nothing, so a relay catching up after a reconnect or sending a spooled batch can't move a ticket back; its
`result` is `{"key", "skipped": "older than the stored status"}`.

With the jira source in board mode (or its stream stuck or silent), the QA poll's one JQL search
(`updated >= -Nm`) also reads the statuses of open tasks' tickets and applies them the same way, so changes
people make in Jira are seen without a stream. It runs whenever Jira is set up, even with QA off.

## Modes

Each source, `prs`, `builds`, `jira` and `qa`, has a mode:

- `board` (the default, today's behaviour): the board polls the source itself. Events are hints; a pr event makes
  the board read that PR from its host.
- `stream`: the stream is the source of truth. A complete pr event is turned into the PR's record and stepped
  through the PR flow without reading the host; a jira.comment goes to QA as it is. A pr event missing fields, or
  one for a PR the board hasn't read yet (the board's own account and the base branch's head come from that first
  read), falls back to one host read.

config.toml sets it (`[stream.sources.<source>] mode`); `tb stream mode <source> board|stream|default` overrides it
on the board (`default` drops the override). `tb stream` (`GET /stream`) prints each source's mode and health.

### Health

The board keeps each source's health on its own. A stream-mode source (and prs with `[feed] on`) is *stuck* when
it has sent heartbeats before and none came in `stuck_secs`, and *silent* when nothing came in `silent_secs`; then
the board runs its `restart_cmd` on the `restart_mins` schedule and raises an alert (`pr-feed` for prs,
`stream-<source>` for the others) when that doesn't help. prs takes these settings from `[feed]`, the others from
`[stream.sources.<source>]`.

While a stream-mode source is healthy its own poll rests, and it resumes as soon as the stream is stuck or silent:

| Source | Poll |
|---|---|
| `prs` | the PR poll (`[intervals] prs`) |
| `builds` | the read of the owner's open PRs while PR builds are stopped (`pr_builds.owner_prs_secs`) |
| `qa` | the Jira comment poll (`qa_poll_mins`) |

#### Relays

A source can hear from several relays (each event's `source`), say one for Bitbucket and one for GitHub
webhooks. The board keeps each relay's health on its own under its source: its heartbeats, idle state, last
event and what it covers. `GET /stream` lists them under each source's `relays`. Once a stream relay has reported
for a source, the source's health is its relays':

- the source is stuck or silent while any of its relays is, each timed on its own, so one relay's heartbeat or
  idle state doesn't make another's silence look fine;
- a relay that never sent a heartbeat is never stuck, but counts as silent after `silent_secs` like any other;
- the PR poll rests only for PRs a healthy, active relay covers by its heartbeats (`covers`), and the whole poll
  rests only while one covers every PR. Other PRs are polled as in board mode. A relay that never sent a heartbeat
  covers nothing. `POST /prs/refresh` still reads every PR.

Without stream relays (only the older endpoints below), health and the poll work as before.

Every accepted event's whole payload is kept in `stream_events` for 30 days, with its kind, source, time,
whether it was complete, and what its route answered.

## The older endpoints

`POST /prs/event`, `POST /prs/heartbeat` and `POST /jira/comment` keep working unchanged. Their payloads carry no
ids, so they aren't deduplicated.
