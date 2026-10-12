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
host instead of trusting the event.

### `review`

`pr`, `who`, `state` (`approved`, `changes`, `commented`, `dismissed`), and `body?`. The event's `at` is the
time of the review.

### `comment`: a PR comment

`pr`, `comment_id`, `author`, `body`, and optionally `thread_id?`, `path?` and `line?` (a line number of 1 or more).

### `push`

`repo`, `branch`, `after` (the new head sha), and optionally `before?`, `author?`,
`commits?: [{sha, message?, author?}]` and `forced?: bool`.

### `build`

| Field | |
|---|---|
| `repo`, `head` | the repo and the commit built (required) |
| `state` | `queued`, `started`, `running`, `passed`, `failed`, `stopped` (required) |
| `branch?`, `pipeline?`, `author?` | |
| `pr?` | the PR the build is for, if any |
| `url?` | the build's link |
| `provider?` | the CI that ran it |

### `jira.issue`: an issue changed

`key` (`PROJ-123`) and `status` are required. The rest are optional: `summary?`, `type?`,
`status_category?` (Jira's `new`, `indeterminate` or `done`), `from_status?`, `assignee?`, `reporter?`,
`changed_by?` and `epic?`. The board trusts the status in this payload.

### `jira.comment`

`key`, `comment_id` (Jira's numeric id), `author` and `body` are required. `visibility?` is optional.

### `heartbeat`: the relay is alive

`sources?` (some of `prs`, `builds`, `jira` and `qa`), `active?: bool`, `idle_until?` and `connected_at?`.
These follow `POST /prs/heartbeat`. While the relay says it's idle (until `idle_until`, if given), the board
doesn't count it as stuck or silent.

## Routing (v1)

| Kind | Goes to |
|---|---|
| `pr`, `build` | the PR feed, as `POST /prs/event` does: the task's PR is read and stepped, and the owner's PR builds are run |
| `heartbeat` | the PR feed's health, as `POST /prs/heartbeat` does |
| `jira.comment` | QA comments, as `POST /jira/comment` does |
| `review`, `comment`, `push`, `jira.issue` | kept only, for now |

Every accepted event's whole payload is kept in `stream_events` for 30 days, with its kind, source, time,
whether it was complete, and what its route answered.

## The older endpoints

`POST /prs/event`, `POST /prs/heartbeat` and `POST /jira/comment` keep working unchanged. Their payloads carry no
ids, so they aren't deduplicated.
