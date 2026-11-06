# The owner's steps and code review

Read this when your handoff or `tb steps` lists steps, when a review step reports findings, or when `tb submit` says the task is in code review.

## The owner's steps

Steps are work the owner wants on every task in this project, before the PR or before `tb submit`, in order. Each says how to get past it:

- Agent work: do it as its prompt says, then `tb step done "<name>" --note "what came of it"`. It runs the step's check first; fix what fails.
- A script: `tb step run "<name>"`, fix what it reports and run it again. It can take a while: give it time.
- The owner's: `tb step ask "<name>"`, then end your turn; the board brings you back once it's done.

A step that can't pass: `tb step fail "<name>" --why "…"`, then end your turn. The board refuses the PR and `tb submit` while one is left.

## Review steps

A review step may report findings: `tb steps` lists them with their ids.

- Fix each, then `tb step triage "<name>" F2 --state fixed --commit <sha>`, or `answered`/`dismissed` with `--note "why"`.
- `tb step again "<name>"` runs another round. Add `--branch <b>` or `--worktree <dir>`, and/or `--commit <ref>`, to aim it at something other than this checkout's head.
- `tb step aim` with the same flags saves the aim so later rounds and the gates follow it; `tb steps` shows it. A pinned `--commit` is dropped once its branch gets a new commit. A detached checkout takes `--worktree <dir> --branch <b>` together, judged on the branch's tip, which must contain the checkout's head.
- After a rebase's push (once the task has a PR), `tb step publish "<name>"` republishes a review step's result for the new head. `tb pr status` lists it in the rebase commands.
- A step that runs per push needs a round on each new commit. Rounds may need a gap between them; the refusal says when.

## Review findings

From a review step, a reviewer or a check: fix only what this task's change caused. A finding on code the change didn't touch goes to the backlog (`tb backlog add … --kind follow`) and is dismissed in the review with a line saying so. Never reverse an earlier decision (a goal note, a checkpoint `--decision`, an owner's answer) to satisfy a finding.

## Code review

When the board has a reviewer for the project, never run a reviewer yourself. `tb submit` may answer that the task is in code review: end your turn and leave the worktree as it is. The board runs the reviewer on it, a separate triage agent sorts the findings (fix, backlog or dismiss), and the submit carries on by itself when nothing is left to fix.

When a round found things your change caused, the board sends you the fix list. Fix only those, mark each once it's fixed with `tb finding T<n> F<id> fixed "<what you changed>"` (`tb finding T<n>` lists them), then commit, push and run `tb submit` again with the same summary and `--pr-body`. A changed worktree gets another round, up to the board's cap (5 by default); an unchanged one goes on to its PR, and the owner hears of any finding not marked fixed.
