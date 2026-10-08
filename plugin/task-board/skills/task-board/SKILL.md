---
name: task-board
description: How to use the local task board with the tb command. Use when on a task-board task (a prompt or context starting [task-board:T<n>]), when asked to take a task, when the owner asks to add a goal, task, backlog item or attachment (design, proposal, doc) to the board, or when the board brings you back about a PR.
---

# Task board

The task board tracks every Midna Claude session and its task. Hooks report turns and commits; you report the rest. "The owner" is the person who runs the board.

If a bare `tb` isn't on the PATH, use `${CLAUDE_PLUGIN_ROOT}/bin/tb`. The `.md` files named here sit next to this one.

## When you're on a task

You're on a task when your prompt or context starts with `[task-board:T<n>]`, or after `tb take T<n>`. Then:

- **At milestones**, save a checkpoint a new terminal could carry on from:
  `tb checkpoint --done "what's finished" --next "what's next" --decision "a choice to keep"`
  Flags repeat; `--file path` for key files.
- **Notes** worth keeping on the task: `tb note "..."`. For the whole goal: `tb note --goal --kind finding|decision|reference "..."`.
- **Something outside the task**: don't fix it. Report it and carry on:
  `tb found "<short summary, 80 chars max>" --kind bug|gap|follow|clean --detail "what you saw" --output "error text"`
- **Needs another task's unfinished work** (any goal): `tb wait-for T<n> --why "…"`, then end your turn. Never ask the owner; see `questions.md`.
- **Blocked on the owner**: read `questions.md` first; most calls are yours. Still the owner's? `tb question "<the question>"`, then stop.
- **Something made for the task**: `tb attach <url-or-path> --kind design|proposal|doc|evidence|results|other --title "..."` (`--goal G<n>` for the goal).
- **Written results** (measurements, a write-up): attach a link with `--kind results`. Lead with the answer.
- **A design is attached**: open it before you touch UI and build to it; note anything that can't match.
- **At the end**: `tb done "<one-paragraph summary>" --human <time>`, or `tb fail "<why>"` if it can't be done. `--human` is your honest estimate of how long this task would have taken a developer by hand (`3h`, `90m`, `1d`); the Days page compares it with your time. A PR, now or later? Read `pr.md` first.

`tb status` shows this terminal's task. Every command takes `--task T<n>`. If the board is down, `tb` saves reports and sends them later.

## When there's no task

Don't run `tb` unless the owner asks you to take, report or add something. PR work on a done task: `pr.md`.

## Putting things on the board

The owner can't make or change anything in the Taskboard app: new tasks, goals and issues, notes, edits, moves and attachments all go through you when they ask. The app only starts and stops work.

Planning a goal? Read `planning.md` first.

1. Reuse an open goal that fits (`tb goals`; `--project <name>` narrows it).
2. Pick the smallest thing that fits:
   - An effort with an outcome: `tb goal new "<name>" --tldr "…" --outcome "<done when>" --task "title::what to do"` (repeat `--task`). Tasks for an existing goal: `tb propose G<n> --task "title::detail"`.
   - One piece of work: `tb task new "<title>" --detail "..." [--goal G<n>]`.
   - Something to remember, not do now: `tb backlog add "<title>" --kind bug|gap|follow|clean --detail "..." [--goal G<n>]`.
   - Retitle or rewrite an issue (the owner asks you to): `tb backlog set B<n> --title "..." [--detail "..."]`.
   - Move an issue to another goal, or out of its goal: `tb backlog move B<n> G<n>|none`.
   - A design, proposal, doc or link: `tb attach` as above, with `--task T<n>` or `--goal G<n>`.
   - Remove an attachment by its link, path or title: `tb unattach "<link-or-title>"` with `--task T<n>` or `--goal G<n>`. To change one, unattach it and attach it again.
3. Delete a goal only when the owner asks: `tb goal delete G<n>` asks whether to keep its tasks; ask the owner, then run it with `--keep-tasks` or `--delete-tasks`.
4. The project defaults to this terminal's; `--project <name>` picks another.
5. Nothing you add runs until the owner queues or starts it; don't take it unless asked.
6. Give the owner the link `tb` printed.

Titles are short summaries a person reads at a glance (80 characters at most, no test names, paths, commit hashes or error text); all of that goes in `--detail`.

## Work hours

The board starts agents only inside the owner's work hours. `tb hours` shows them; change them only when the owner asks (`tb hours --start 09:00 --end 17:00 --days mon-fri`, `--on`/`--off`, `--today-until 4pm` for today only).
