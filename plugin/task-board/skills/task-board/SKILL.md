---
name: task-board
description: How to use the local task board with the tb command. Use when on a task-board task (a prompt or context starting [task-board:T<n>]), when you are the board's Jira desk (a job starting [task-board:J<n>]), when asked to take a task, when the owner asks to add a goal, task, backlog item or attachment (design, proposal, doc) to the board, or when the board brings you back about a PR.
---

# Task board

The task board tracks every Midna Claude session and its task. Hooks report turns and commits; you report the rest. "The owner" is the person who runs the board.

If a bare `tb` isn't on the PATH, use `${CLAUDE_PLUGIN_ROOT}/bin/tb`. The `.md` files named here sit next to this one. `tb <cmd> --help` has each command's flags. When the board refuses something, the refusal says what to do next: do that.

## Which file for which role

- On a task: "When you're on a task", below.
- Brought back about a PR: `pr.md`.
- No task on this terminal, or looking for work on the board (`tb search`): `no-task.md`.
- Adding to the board, or acting on it for the owner (goals, tasks, backlog, attachments, starting work, Needs you, projects, releases): `board.md`. Planning a goal: `planning.md` too.
- Context limits, work hours, keep-awake, the status bar and the terminal cap: `board.md`.
- The Jira desk: `jira.md`.
- A code review's triage agent: `triage.md`.

## When you're on a task

You're on a task when your prompt or context starts with `[task-board:T<n>]`, or after `tb take T<n>`. The goal's setup (`Set up (every task in this goal does this)` in your handoff) comes before anything else, and it overrides the handoff's branch name when it names one. Then:

- **At milestones**, save a checkpoint a new terminal can resume from: `tb checkpoint --done "what's finished" --next "what's next" --decision "a choice to keep"`.
- **Notes** worth keeping: `tb note "..."`. For the whole goal: `tb note --goal --kind finding|decision|reference "..."`.
- **Lessons** that should outlast the goal (a trap in the code, a step that keeps failing, a plan that had to be redone): `tb lesson add "<what to do differently>" --kind planning|code|process|tooling`. Read `lessons.md` first.
- **The owner asks for something else**: the board asks on each of their prompts whether it's part of this task. Small follow-ups to the same change are; a separate fix or feature (its own commit or PR) isn't. Split separate work off before you start on it, without asking:
  - `tb task new "<title>" --detail "<the ask>" --here --next` queues it in this terminal after the current task; carry on with the current one.
  - `--here --now` switches to it now; the current task waits here to resume. Commit or stash its changes first when they share the checkout.
  - `tb line`, `tb switch T<n>` and `tb line drop T<n>` work this terminal's line.
  - `tb move` hands the task you're on to a new terminal, only on the owner's word; leave its work to that terminal and carry on with their other asks here.
- **Tests**: run only the tests that cover what you changed (the crate, package, module or test file you touched, e.g. `cargo test -p <crate>`, `flutter test test/<area>`), never the whole suite. CI runs the rest; when a CI check fails, run the failing tests and the ones covering your fix.
- **Long builds and tests**: anything that may run past a couple of minutes goes in the background (`run_in_background: true`); Claude Code tells you when it exits. Never poll it with a `sleep` loop or wrap it in a foreground command that can hit the 10-minute timeout. Run `cargo check` before `cargo test`.
- **Something outside the task**: don't fix it. Report it and carry on: `tb found "<short summary, 80 chars max>" --kind bug|gap|follow|clean --detail "what you saw" --output "error text"`.
- **The goal's own done-when isn't met** (you see its outcome miss, e.g. on one platform): `tb found "<what misses>" --misses-goal`.
- **Another repository**: a task's work stays in its own project's repository. The board refuses the rest and says how to file it.
- **Worktrees**: never make your own; a task's worktree is a slot from the board's pool. Holding or wanting a slot, or committing in a shared checkout: `worktrees.md`.
- **Needs another task's unfinished work** (any goal): `tb wait-for T<n> --why "…"` (add `--merged` when you need its PR merged first), then end your turn. Never ask the owner.
- **Blocked on the owner**: read `questions.md` first; most calls are yours. Still the owner's? `tb question "<the question>"`, then stop. Answered another way since? `tb withdraw "<how>"`.
- **Needs something only the owner can do** (make an account, sign an agreement, buy something, get a key from a site you can't reach): `tb task new "<what to do>" --human --detail "<numbered steps, each link written out, and what to send back>"`, then `tb wait-for H<n> --why "…"` and end your turn. When the owner tells you they did it ("H4's done", "mark them done"), `tb needs done H4` marks it done for them (several at once: `tb needs done H3 H4`); never on your own judgment. The steps hold everything they need and nothing for you; one H task per thing they do. Anything an agent can run (a release cut, a deploy, a `tb` command) is never a human task, even if the plan says not to do it yourself: make it an agent task (`--no-pr`) and `tb wait-for T<n>` on that. The board screens every new human task against the owner's rules and refuses one an agent could do (a setting behind a sign-in Claude in Chrome already has, a lookup through an MCP connector, a fact in the code) or the rules already settle: do what the refusal says instead.
- **Something made for the task**: `tb attach <url-or-path> --kind design|proposal|doc|evidence|results|other --title "..." --summary "..."` (`--goal G<n>` for the goal). A claude.ai artifact link takes the artifact's own title as `--title`; the board can't open it to read one. Writing is published as a brief artifact and attached as a link; written results go in with `--kind results`, leading with the answer.
- **Several questions to chase at once**: they go on the board, not to a batch of subagents: a research task each (`tb task new "<the question>" --kind research --detail "…" --goal G<n>`) in the goal this work belongs to, or a new one (`tb goal new`). Run it with `tb start G<n>` only on the owner's word; otherwise tell them it's ready and ask. The board stops a batch of subagents or a workflow once to say so, unless you're on a research task. When it isn't research (reading code for the work you're on), run the same call again and it goes through.
- **A design is attached**: open it before you touch UI and build to it; note anything that can't match.
- **Default branch**: commit on a branch of your own and land it through a PR.
- **No code comments** (Python docstrings count too) when your handoff says so; pragmas the tools read are fine.
- **The owner's steps** (in your handoff, or `tb steps`) come before the PR or `tb submit`, in order. Review rounds, findings and code review: `steps.md`.
- **Review findings** (a review step, a reviewer, a check): fix only what this task's change caused; the rest goes to the backlog. Never reverse an earlier decision to satisfy a finding.
- **Code review**: never run a reviewer yourself. When `tb submit` says the task is in code review, end your turn and leave the worktree as it is.
- **Jira** (when the board has it on: your handoff has a `Jira:` line or names a ticket): read `jira.md`. Never ask the owner which ticket.
- **At the end**: `tb submit "<summary>" --human <time>`, or `tb fail "<why>"` if it can't be done. The summary is for the owner, not another agent: one or two short plain sentences on what's different now or what was found, with little to no jargon (no file, function, query or header names, no task and backlog refs). The details belong in the PR, a note or an attached doc. `--human` is your honest estimate of how long this would have taken a developer by hand (`3h`, `90m`, `1d`). A PR, now or later? Read `pr.md` first: the board opens it (`tb submit "<summary>" --pr-body <file>`); `--pr <link>` is only for a PR that was open before the task. When this terminal has a line, `tb submit`, `tb fail` and a `tb wait-for` that parks the task print the next task's handoff: carry straight on with it.

`tb status` shows this terminal's task and its line (with the board down it says so and fails; it saves nothing). A handoff section marked "Shortened to fit" is whole in `tb handoff`. Every command takes `--task T<n>`. If the board is down, `tb` saves reports and sends them later.
