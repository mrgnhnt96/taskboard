# Pre-PR review protocol, v1

The contract between the board and a code reviewer. Before a task's PR opens, the board runs the
project's reviewer on the task's worktree, reads what it found, and decides what happens next. Any program
that follows this page can be the reviewer: write one that reads the variables below and writes the result
file, and point `[code_review] command` at it.

## Turning it on

```toml
[code_review]
command = "my-reviewer review"   # run with sh -c; unset or empty: no review stage
timeout_mins = 30                # the reviewer is killed past this
max_rounds = 5                   # rounds per task; past this the task carries on to its PR
```

One reviewer per project. `tb project set <project> --code-review '<command>'` gives a project its own,
`--code-review none` turns review off for it, and `--code-review default` goes back to config.toml's.
`tb project show` lists it. With no command set, tasks behave as they did without this stage.

## Who runs it

The board's daemon, never an agent session. When a task that ends in a PR runs `tb submit` (or the board
opens its PR for it), the submit is held and the task goes into the **in code review** stage. The board
runs the command:

- with `sh -c`, in the task's worktree (the task keeps holding its slot), in its own process group;
- off the agent's session: the agent is told to end its turn and leave the worktree alone;
- killed with its whole process group at `timeout_mins`.

Each round is recorded on the task (its head commit, the tree it read, the result, the reviewer's name and
cost, the tail of its stderr) and shown on the task card as `code_review`.

## Inputs

Environment variables: the `TASKBOARD_*` variables a step's script gets (`TASKBOARD_TITLE`,
`TASKBOARD_PROJECT`, `TASKBOARD_PR_URL`, `TASKBOARD_JIRA`, …), plus these, which win over a step
variable of the same name:

| Variable | Value |
|---|---|
| `TASKBOARD_REVIEW_PROTOCOL` | `1`, the protocol version |
| `TASKBOARD_REPO` | the task's worktree path (also the working directory) |
| `TASKBOARD_BASE` | the base ref the PR goes into, with its remote: `origin/main` |
| `TASKBOARD_HEAD` | the head commit's sha |
| `TASKBOARD_BRANCH` | the task's branch |
| `TASKBOARD_TASK` | the task's ref, `T<n>` |
| `TASKBOARD_ROUND` | this round's number, `1` up to `max_rounds` |
| `TASKBOARD_INTENT` | path to a markdown file with the task's title and detail and its goal's done-when |
| `TASKBOARD_PREVIOUS` | path to the previous round's result JSON, each finding with its triage `outcome` (`fix`, `backlog` or `dismiss`) and `reason` added. Unset on round 1 |
| `TASKBOARD_STATE_DIR` | a directory for this task that the board keeps across rounds and deletes when the task ends |
| `TASKBOARD_RESULT` | the path the reviewer writes its result to |

Uncommitted changes in the worktree are part of what's reviewed.

## Output

JSON written to `$TASKBOARD_RESULT`, a superset of a step's result:

```json
{
  "protocol": 1,
  "reviewer": "my-reviewer",
  "verdict": "fail",
  "headline": "One major finding",
  "findings": [
    {
      "id": "F1",
      "title": "Off-by-one in the page count",
      "severity": "major",
      "file": "src/a.rs",
      "line": 42,
      "detail": "The failure scenario, or why it matters",
      "url": null
    }
  ],
  "cost": {"usd": 0.61}
}
```

- `verdict`: `pass` (nothing worth triage), `fail` (there are findings), or `skip` (nothing to review,
  such as a docs-only change).
- `severity`: `blocker`, `major` or `minor`.
- `id`: keep the same id across rounds for the same finding when the reviewer can tell.
- `cost` is optional.

## Exit

- `0`: the result is written; the verdict carries the outcome.
- Non-zero, or a missing or invalid result (not JSON, or no `pass`/`fail`/`skip` verdict): a reviewer
  error. The board tries once more, then carries on without a review and alerts the owner.
- stderr is logged as progress; stdout is ignored.

## Rules for the reviewer

The reviewer never changes the worktree, commits, pushes or touches the PR host. It writes only to
`$TASKBOARD_STATE_DIR`, `$TASKBOARD_RESULT` and its own caches outside the repository.

## The loop (board side)

1. The code is done: the agent runs `tb submit`, and round *n* runs.
2. Verdict `fail`: a fresh triage agent marks each finding `fix`, `backlog` or `dismiss`, with a reason
   (`backlog` files a backlog item).
3. The task's own agent is resumed with the `fix` findings.
4. On its next `tb submit`: if the worktree changed, round *n+1* runs; otherwise the task carries on.

The loop stops when a round has no `fix` findings, when the code is unchanged, or at `max_rounds` (5). At the
cap with fixes still open, the board opens the PR anyway and alerts the owner.

A submit turned back after its review (a branch that isn't pushed, say) doesn't run the reviewer again
while the code is as it was reviewed.
