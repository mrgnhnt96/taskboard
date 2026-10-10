---
name: task-board
description: How to use the local task board with the tb command. Use when on a task-board task (a prompt or context starting [task-board:T<n>]), when you are the board's Jira desk (a job starting [task-board:J<n>]), when asked to take a task, when the owner asks to add a goal, task, backlog item or attachment (design, proposal, doc) to the board, or when the board brings you back about a PR.
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
- **The owner asks for something else**: the board asks on each of their prompts whether it's part of this task. Small follow-ups to the same change are; a separate fix or feature (its own commit or PR) isn't. Split separate work off before you start on it, without asking:
  - `tb task new "<title>" --detail "<the ask>" --here --next` queues it in this terminal after the current task; carry on with the current one.
  - `--here --now` switches to it now; the current task waits here to resume. Commit or stash its changes first when they share the checkout.
  - `tb switch T<n>` moves to a task in this terminal's line; `tb line` lists the line; `tb line drop T<n>` sends one back to the board.
- **Tests**: run only the tests that cover what you changed (the crate, package, module or test file you touched, e.g. `cargo test -p <crate>`, `flutter test test/<area>`), never the whole suite. CI runs the rest; when a CI check fails, run the failing tests and the ones covering your fix.
- **Something outside the task**: don't fix it. Report it and carry on:
  `tb found "<short summary, 80 chars max>" --kind bug|gap|follow|clean --detail "what you saw" --output "error text"`
- **Needs another task's unfinished work** (any goal): `tb wait-for T<n> --why "…"` (add `--merged` when you need its PR merged first), then end your turn. Never ask the owner; see `questions.md`.
- **Blocked on the owner**: read `questions.md` first; most calls are yours. Still the owner's? `tb question "<the question>"`, then stop.
- **Something made for the task**: `tb attach <url-or-path> --kind design|proposal|doc|evidence|results|other --title "..."` (`--goal G<n>` for the goal). Don't attach writing as a loose file (.md, .txt, .rst, .html): publish it as a brief artifact and attach that link; the board refuses those files.
- **Written results**: attach a link with `--kind results`. Lead with the answer.
- **A design is attached**: open it before you touch UI and build to it; note anything that can't match.
- **No code comments** when your handoff says so: the owner's comment guard is on. Make the code say it (names, small functions) and put the why in the commit or PR; pragmas the tools read (`# noqa`, `// eslint-disable-next-line`, …) are fine. The board refuses an edit that adds a comment, and the PR, `tb pr wait` and `tb done` while the branch adds any.
- **The owner's steps** (in your handoff, or `tb steps`): work the owner wants on every task in this project, before the PR or before `tb done`, in order. Each says how to get past it:
  - agent work: do it as its prompt says, then `tb step done "<name>" --note "what came of it"` (it runs the step's check first; fix what fails);
  - a script: `tb step run "<name>"`, fix what it reports and run it again (it can take a while: give it time);
  - the owner's: `tb step ask "<name>"`, then end your turn; the board brings you back once it's done.
  A step that can't pass: `tb step fail "<name>" --why "…"`, then end your turn. The board refuses the PR and `tb done` while one is left.
  A review step may report findings: `tb steps` lists them with their ids. Fix each, then `tb step triage "<name>" F2 --state fixed --commit <sha>` (or `answered`/`dismissed` with `--note "why"`), and `tb step again "<name>"` for another round (add `--branch <b>` or `--worktree <dir>`, and/or `--commit <ref>`, to aim it at something other than this checkout's head; `tb step aim` with the same flags saves the aim so later rounds and the gates follow it, and `tb steps` shows it; a pinned `--commit` is dropped once its branch gets a new commit, and a detached checkout takes `--worktree <dir> --branch <b>` together, judged on the branch's tip, which must contain the checkout's head). After a rebase's push (once the task has a PR), `tb step publish "<name>"` republishes a review step's result for the new head (`tb pr status` lists it in the rebase commands). A step that runs per push needs a round on each new commit; rounds may need a gap between them (the refusal says when).
- **Review findings** (a review step, a reviewer, a check): fix only what this task's change caused. A finding on code the change didn't touch goes to the backlog (`tb backlog add … --kind follow`) and is dismissed in the review with a line saying so. Never reverse an earlier decision (a goal note, a checkpoint `--decision`, an owner's answer) to satisfy a finding.
- **Jira** (when the board has it on: your handoff has a `Jira:` line or names a ticket): read `jira.md`. Never ask the owner which ticket.
- **At the end**: `tb done "<one-paragraph summary>" --human <time>`, or `tb fail "<why>"` if it can't be done. When this terminal has a line, `tb done`, `tb fail` and a `tb wait-for` that parks the task move it on to the next task and print its handoff: carry straight on with it. Outside work hours or with the 5-hour usage used up nothing moves on; the next task is sent here once they open or the usage resets. `--human` is your honest estimate of how long this task would have taken a developer by hand (`3h`, `90m`, `1d`); the Days page compares it with your time. A PR, now or later? Read `pr.md` first.

`tb status` shows this terminal's task and its line. Every command takes `--task T<n>`. If the board is down, `tb` saves reports and sends them later.

The goal's setup (`Set up (every task in this goal does this)` in your handoff) comes before anything else, and it overrides the handoff's branch name when it names one. Set it only when the owner asks: `tb goal setup G<n> "<what every task does first>"` ({task}, {n}, {wave}, {goal} and {jira} (the ticket key, else the task ref) are filled in per task, {device} and {target} ({device2}, {target2}…) with the devices lent to it; `none` clears it).

## When you're the Jira desk

Your first prompt says "You are the task board's Jira desk", and each job starts `[task-board:J<n>]`. Follow `jira.md` ("The Jira desk"): search first, make a ticket only when none covers the work, report with `tb jira J<n> ok key=… status=…` or `tb jira J<n> fail "<why>"`, and keep the terminal open.

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
   - An effort with an outcome: `tb goal new "<name>" --tldr "…" --outcome "<done when>" --task "title::what to do"` (repeat `--task`). Tasks for an existing goal: `tb propose G<n> --task "title::detail"`. Tasks that run side by side in a wave can name the files each will edit, so its wave mates leave them alone: `--task "title::detail::<wave>::::src/a.rs, src/b.rs"` (or `tb task new --goal G<n> --wave 2 --file src/a.rs`).
   - One piece of work: `tb task new "<title>" --detail "..." [--goal G<n>]`.
   - Only a human starts a task: it waits for the owner to press Start. When the owner tells you in this conversation to start or queue one ("start T4", "make a task and queue it"), run `tb start T<n>` (`--queue` to start it once its project has a free terminal); never on your own. "Start the goal" means `tb start G<n>`, which runs its tasks in their waves: never `tb start` a goal's tasks one by one. A goal runs from here only on the owner's word: a prompt that names it ("start G2", "run G2"), wherever it was made, or, when it's this conversation's goal (your task is in it, you made it or a task in it, or the board opened this terminal to plan it or handed it to you), "start the goal" or "run the goal". A word is good for one run: run it again only on a new one. "Run it" counts right after they asked you for the goal you then made ("make a goal for X and run it"), or when it's this conversation's only goal, and a plain "yes" or "run it" counts when your last message ended on asking whether to run it: ask plainly, naming the goal ("Want me to run G2?"), with no other choice in the same question. A plain "yes" (or "start it") to your "Should I start T4?" or "Should I start T4 and T5?" starts them the same way, and so does "yes, don't start T5" for T4 alone. "It" in your question is what your message spoke of last: the goal after "The plan has two tasks." or "Made G2 with T4 and T5.", but the script after "Wrote seed.sh." or "The goal's migration is written.", the dev server after "I fixed the dev server.", and the task after "I made T5 for the follow-up.", so name what you mean (a README, a ticket, a branch or the docs is never what "it" means). A task that waits for unfinished work (`tb wait-for`, a stack parent) can't start until that's done. The board checks the owner's prompts in this terminal and refuses without their word: a prompt that asks for the start now and names the task, or (the latest prompt only) "make a task and queue it" for the first task you made with `tb task new` after it ("make tasks for A and B and queue them": every one), or a bare "queue it" / "queue them" after a prompt that asked you to make the task(s) you then made (with only a "thanks" or "looks good" between). The latest prompt about it wins, so "don't start T4", "wait", "nope", "never mind that" or "start T5 instead" takes an earlier ask back, and so does a take-back later in the same prompt ("start T4. jk"). A question, a no, a time or condition on the start ("don't start T4", "start T4 tomorrow", "once T3 lands"), and quoted or pasted text are no word: then wait for the owner. One about something else ("start T4 and tell me when it's done", "no rush", "but don't merge it") still is. Never read the board's `app-token` file or its `app.sock`; they're only for the app.
   - Another goal's task does it: `tb task set T<n> --also G<n>`.
   - Waves, waits, locks, running alone, worktrees, devices and bits (feature flags): `planning.md`.
   - Something to remember, not do now: `tb backlog add "<title>" --kind bug|gap|follow|clean --detail "..." [--goal G<n>]`.
   - Retitle or rewrite an issue (the owner asks you to): `tb backlog set B<n> --title "..." [--detail "..."]`.
   - Move issues to another goal, or out of their goal: `tb backlog move B<n> [B<n> …] G<n>|none` (several move together or not at all).
   - On the owner's word, act on an issue: `tb backlog task B<n>` (a planned task in its goal; `--board` queues it on the board), `tb backlog ticket B<n>` (a Jira ticket), `tb backlog drop B<n> --reason "..."` (won't do), `tb backlog reopen B<n>`. Each takes several issues at once (`tb backlog task B4 B5`): all change or none do.
   - Run, hold back or bring back a goal when the owner says so: `tb start G<n>` or `tb goal set G<n> --run` (queues its planned tasks; refused without their word to start or run it in this terminal, and outside a Midna terminal, where only Run on the board runs a goal; a refused `--run` saves none of that command's other changes), `--deprioritize`, `--prioritize`, `--paused on|off`.
   - Something the owner must see that isn't a question on your task (a watcher or script you were asked to set up finds main red): `tb alert raise "<what>" --key <name> [--urgent] [--task T<n>]`, and `tb alert clear <name>` once it's fixed. `--urgent` only for what can't wait: it repeats outside the work hours and can't be dismissed.
   - A red default branch the board watches is a break, `M<n>`: `tb master` lists them and `tb master M<n>` shows the failing checks and suspect commits. The board decides whose it is; `tb master M<n> ours|not-ours|unsure --why "<…>"` changes that only on the owner's word (`not-ours` needs `--proof <link>`: the build or issue that shows it; repeat for more). A fix task it made for one is an ordinary task: fix the branch, and the board closes the break when it's green.
   - Whether a project's work ends in PRs, and its PR rules: `tb project show [<name>]`; on the owner's word, `tb project set <name> --pr-flow auto|on|off`, `--approvals N` (approvals a PR needs; 0 = the host decides), `--expected-check "<check>"` (repeat; `none` for none), `--expected-wait <mins>` `--ask-stage on|off` (after the owner's review, the PR waits until you ask its reviewers with `tb pr reviewers`) `--swap on|off` (a reviewer who hasn't reviewed in time is replaced), `--review on|off` (off: its PRs go to merge without reviewers) and `--agents-merge on|off` (you merge its approved, green PRs with `tb pr merge`), `--waits-on <other>[,…]|none` (its tasks may wait for those projects' tasks); `default` puts a rule back to config.toml's. For every project that doesn't say: `tb project agents-merge [on|off|default]`.
   - A design, proposal, doc or link: `tb attach` as above, with `--task T<n>` or `--goal G<n>`.
   - Remove an attachment by its link, path or title: `tb unattach "<link-or-title>"` with `--task T<n>` or `--goal G<n>`. To change one, unattach it and attach it again.
   - A tester's Jira comment the owner gave you their word on (when QA comments are on): `tb qa task Q<n> --note "<what they said>"` or `tb qa ignore Q<n>`; `tb qa waiting` lists the ones waiting.
3. Delete a goal only when the owner asks: `tb goal delete G<n>` asks whether to keep its tasks (a task that also finishes another goal moves there either way); ask the owner, then run it with `--keep-tasks` or `--delete-tasks`.
4. The project defaults to this terminal's; `--project <name>` picks another.
5. Nothing you add runs until the owner queues or starts it; don't take it unless asked.
6. Give the owner the link `tb` printed.

Titles are short summaries a person reads at a glance (80 characters at most, no test names, paths, commit hashes or error text); all of that goes in `--detail`.

## Context limits

The board compacts its agents' conversations at a token window, compacts a cold conversation (idle over an hour) before resuming it, and resumes a task or PR conversation only while it's small and recent (otherwise it starts fresh from the handoff). It also keeps generated files out of agents' diffs (a `-diff` block in `.git/info/attributes`). `tb limits` shows the limits; change them only when the owner asks: `--compact-window 150000`, `--cold-idle-mins 60`, `--warm-tokens 60000`, `--warm-idle-mins 60` (0 turns one off), `--generated "*.g.dart,Cargo.lock"` (`--project <name>` for one project's own; `none` clears), `--reset`.

## Work hours

The board starts agents only inside the owner's work hours. `tb hours` shows them; change them only when the owner asks (`tb hours --start 09:00 --end 17:00 --days mon-fri`, `--on`/`--off`, `--today-until 4pm` for today only, `--alert-every 10` for how often unanswered alerts repeat, 0 for never).

Midna keeps the Mac awake during the work hours so agents keep running while the owner is away (idle sleep only: the display still sleeps and closing the lid still sleeps). The work hours are its schedule: `tb hours` changes both, and `--today-until` ends both early today. `tb keep-awake` shows whether it's held and why. Change it only when the owner asks: `--on`/`--off`, `--mode with-work|always`, `--min-battery 20` (on battery it lets the Mac sleep below this), `--linger 5`, `--day fri=9am-3pm` / `--day sat=off` / `--day fri=default` for one day's own keep-awake hours, `--today off` / `--today clear`.

Midna's status bar shows the selected terminal's goal, task and line (the app's `plugin/task-board/bin/midna-status`, which runs `tb peek --midna`; `tb peek` alone prints the terminal's whoami as JSON and only reads). `tb status-bar` shows what it includes; change it only when the owner asks: `--goal on|off`, `--title on|off` (off shows refs only), `--reset`.
