---
name: task-board
description: How to use the local task board with the tb command. Use when on a task-board task (a prompt or context starting [task-board:T<n>]), when asked to take a task, when the owner asks to add a goal, task, backlog item or attachment (design, proposal, doc) to the board, or when the board brings you back about a PR.
---

# Task board

The task board tracks every Midna Claude session and its task. Hooks report turns and commits; you report the rest. "The owner" is the person who runs the board.

If a bare `tb` isn't on the PATH, use `${CLAUDE_PLUGIN_ROOT}/bin/tb`. The `.md` files named here sit next to this one.

## When you're on a task

You're on a task when your prompt or context starts with `[task-board:T<n>]`, or after `tb take T<n>`. Then:

- **At milestones**, save a checkpoint a new terminal can resume from:
  `tb checkpoint --done "what's finished" --next "what's next" --decision "a choice to keep"`
  Flags repeat; `--file path` for key files.
- **Notes** worth keeping: `tb note "..."`. For the whole goal: `tb note --goal --kind finding|decision|reference "..."`.
- **Something outside the task**: don't fix it. Report it and carry on:
  `tb found "<short summary, 80 chars max>" --kind bug|gap|follow|clean --detail "what you saw" --output "error text"`
- **Needs another task's unfinished work** (any goal): `tb wait-for T<n> --why "…"`, then end your turn. Never ask the owner; see `questions.md`.
- **Blocked on the owner**: read `questions.md` first; most calls are yours. Still the owner's? `tb question "<the question>"`, then stop.
- **Something made for the task**: `tb attach <url-or-path> --kind design|proposal|doc|evidence|results|other --title "..."` (`--goal G<n>` for the goal).
- **Written results**: attach a link with `--kind results`. Lead with the answer.
- **A design is attached**: open it before you touch UI and build to it; note anything that can't match.
- **The owner's steps** (in your handoff, or `tb steps`): work the owner wants on every task in this project, before the PR or before `tb done`, in order. Each says how to get past it:
  - agent work: do it as its prompt says, then `tb step done "<name>" --note "what came of it"` (it runs the step's check first; fix what fails);
  - a script: `tb step run "<name>"`, fix what it reports and run it again (it can take a while: give it time);
  - the owner's: `tb step ask "<name>"`, then end your turn; the board brings you back once it's done.
  A step that can't pass: `tb step fail "<name>" --why "…"`, then end your turn. The board refuses the PR and `tb done` while one is left.
- **At the end**: `tb done "<one-paragraph summary>" --human <time>`, or `tb fail "<why>"` if it can't be done. `--human` is your honest estimate of how long this task would have taken a developer by hand (`3h`, `90m`, `1d`); the Days page compares it with your time. A PR, now or later? Read `pr.md` first.

`tb status` shows this terminal's task. Every command takes `--task T<n>`. If the board is down, `tb` saves reports and sends them later.

## When there's no task

The owner's questions, screenshots and asks in this terminal are the task. Once you change code for them, put it on the board as a standalone task on this terminal, without asking:
`tb task new "<short imperative title>" --detail "<what the owner asked for and what you're changing>" --here`.
You're on it at once; from then on follow "When you're on a task". If a turn changes code with no task, the board stops it and asks for this.

Otherwise don't run `tb` unless the owner asks you to take, report or add something. PR work on a done task: `pr.md`.

## Putting things on the board

The owner can't make or change anything in the Taskboard app: new tasks, goals and issues, notes, edits, moves and attachments all go through you when they ask. The app only starts and stops work.

Planning a goal? Read `planning.md` first.

1. Reuse an open goal that fits (`tb goals`; `--project <name>` narrows it).
2. Pick the smallest thing that fits:
   - An effort with an outcome: `tb goal new "<name>" --tldr "…" --outcome "<done when>" --task "title::what to do"` (repeat `--task`). Tasks for an existing goal: `tb propose G<n> --task "title::detail"`.
   - One piece of work: `tb task new "<title>" --detail "..." [--goal G<n>]`.
   - Another goal's task does it: `tb task set T<n> --also G<n>`.
   - Waves, waits, locks, running alone, worktrees, devices and bits (feature flags): `planning.md`.
   - Something to remember, not do now: `tb backlog add "<title>" --kind bug|gap|follow|clean --detail "..." [--goal G<n>]`.
   - Retitle or rewrite an issue (the owner asks you to): `tb backlog set B<n> --title "..." [--detail "..."]`.
   - Move an issue to another goal, or out of its goal: `tb backlog move B<n> G<n>|none`.
   - A design, proposal, doc or link: `tb attach` as above, with `--task T<n>` or `--goal G<n>`.
   - Remove an attachment by its link, path or title: `tb unattach "<link-or-title>"` with `--task T<n>` or `--goal G<n>`. To change one, unattach it and attach it again.
   - A tester's Jira comment the owner gave you their word on (when QA comments are on): `tb qa task Q<n> --note "<what they said>"` or `tb qa ignore Q<n>`; `tb qa waiting` lists the ones waiting.
3. Delete a goal only when the owner asks: `tb goal delete G<n>` asks whether to keep its tasks (a task that also finishes another goal moves there either way); ask the owner, then run it with `--keep-tasks` or `--delete-tasks`.
4. The project defaults to this terminal's; `--project <name>` picks another.
5. Nothing you add runs until the owner queues or starts it; don't take it unless asked.
6. Give the owner the link `tb` printed.

Titles are short summaries a person reads at a glance (80 characters at most, no test names, paths, commit hashes or error text); all of that goes in `--detail`.

## Work hours

The board starts agents only inside the owner's work hours. `tb hours` shows them; change them only when the owner asks (`tb hours --start 09:00 --end 17:00 --days mon-fri`, `--on`/`--off`, `--today-until 4pm` for today only).

Midna keeps the Mac awake during the work hours so agents keep running while the owner is away (idle sleep only: the display still sleeps and closing the lid still sleeps). The work hours are its schedule: `tb hours` changes both, and `--today-until` ends both early today. `tb keep-awake` shows whether it's held and why. Change it only when the owner asks: `--on`/`--off`, `--mode with-work|always`, `--min-battery 20` (on battery it lets the Mac sleep below this), `--linger 5`, `--day fri=9am-3pm` / `--day sat=off` / `--day fri=default` for one day's own keep-awake hours, `--today off` / `--today clear`.
