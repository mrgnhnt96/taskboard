# Planning a goal

Read this before `tb goal new` or `tb propose`.

## Write the TLDR

Every goal gets a TLDR for the owner: `tb goal new … --tldr "…"`. It sits at the top of the goal page.

- Two to four short sentences in plain words: what the problem is, what's going on, and what will change.
- No file names, function names, flags or jargon. Say what a user or developer would notice.
- The outcome (`--outcome`, or `tb goal set G<n> --outcome "…"`) is where "done when" goes, for the agents. Keep it testable; it doesn't need to read well.

## Say which product

If the owner's board has products configured, give the goal one: `--product <name>` on `tb goal new`, or `tb goal set G<n> --product <name>`. Decide from the work, not the repo. With Jira set up, the product picks the ticket's labels and components; link an existing epic with `tb goal set G<n> --epic KEY`.

## Slice the work

- Each task that changes code ends in one PR. It holds finished code that builds, breaks nothing that works today, and can be merged on its own. It can be several commits.
- Aim for PRs that are easy to review and have a small impact, not for as many PRs as possible.
- A goal runs its tasks in order, so list them in the order they should run; a task that needs an earlier one's code goes after it.
- A task that needs work from another goal: say so in its detail. Mid-task, the agent runs `tb wait-for T<n>` (see `questions.md`). Never assume the other goal will have it merged in time.
- A task that changes no code (an investigation, a measurement): say in its detail that the agent attaches its write-up or numbers with `tb attach <link> --kind results`.
- A task that builds or changes UI from a design gets the design attached: `tb attach <url-or-path> --kind design --task T<n>` (or `--goal G<n>` for every task).

## Waves

A goal with no waves runs its tasks one after another (or in any order). When tasks can run at the same time without touching the same files, put them in one wave: a wave's tasks run side by side, up to the goal's terminal limit, and the next wave waits until every task in it is done and none failed.

- A new task's wave: `tb task new "<title>" --goal G<n> --wave 2`, or `--task "title::what to do::2"` on `tb propose` and `tb goal new`.
- An existing task: `tb task set T<n> --wave 3` (`--wave none` takes it out; tasks with no wave run after the waves, in the group called Post).
- Name a wave: `tb goal wave G<n> 2 --name "API"`.
- Stop the goal after a wave for the owner's review: `tb goal wave G<n> 2 --stop on`, only when the owner asks. The goal page then shows "Continue to wave 3"; `tb goal continue G<n> 2` does the same when the owner says so, and also goes on past a wave whose task failed.
- `tb goal show G<n>` shows each task's wave and the waves' states.

## Waits, locks and running alone

All opt-in: a goal that sets none of it runs as before.

- A task whose PR builds on another's unmerged PR: `tb task new … --stack-on T<m>` (or `tb task set T<n> --stack-on T<m>`), in any goal of the same project. It waits for T<m>, starts from its branch, and its PR goes into it. A task that won't end in a PR (notes, an investigation): `--no-pr` on `tb task new`, or `tb task set T<n> --pr no`.
- Within a wave, a task that needs one other task's work can wait for just that one: `tb task set T<n> --waits-for T<m>` (or `--waits-for` on `tb task new`), or a fourth `::` field on `--task`: `"title::what to do::2::#1"` waits for the first `--task` in the same command, `"title::what to do::::T14"` for T14 (no wave). Separate several with commas.
- Tasks that would get in each other's way without touching the same files (one emulator, a local server, a build cache) share a lock, in any goal: `tb task set T<n> --lock local-core` (several: `--lock emulator-5554,local-core`; `--lock none` clears them). Only one task that names a lock runs at a time; the others say "Waits for local-core (T12 has it)". Lock names are short lowercase words; `tb locks` shows who holds each and who waits.
- A task that needs nothing else running, like a measurement: `tb task set T<n> --alone` waits until the rest of its goal is quiet, then nothing else in the goal starts until it's done. `--alone board` does the same for the whole board; `--alone none` undoes it.
- A goal whose tasks run side by side in one repo: `tb goal set G<n> --worktrees origin/<base>` starts each task in its own git worktree, `.claude/worktrees/T<n>`, detached at that base. The agent makes its branch there with `git switch -c`. The board removes the worktree once the task is done, its PR is merged or closed (or it has none) and its terminal is closed; one with uncommitted changes is kept. `--worktrees off` goes back to the shared folder.
- `tb goal show G<n>` marks each task with "waits for T12", "holds local-core" or "runs alone in its goal".

## Work that also finishes another goal

Before you add a task, check the project's other open goals (`tb goals --project <name>`, then `tb goal show G<n>`) for work that overlaps: the same files, the same change. When one task's PR finishes both, keep it one task and reference the other goal; never add a second task for the same work, and don't wait on the owner to decide.

- An existing task: `tb task set T<n> --also G<n>` (`--not-also G<n>` takes it back).
- A new one: `tb task new "<title>" --goal G<home> --also G<n>`.
- The home goal runs it: its order, pause and terminals. The other goal counts it toward done and lists it under "From other goals". A paused other goal doesn't stop it.
- `tb task set T<n> --goal G<n>` moves it to another home. If its home goal is deleted, it moves to the latest goal that references it.

## Tell the owner

Give the owner the goal's link (`tb goal new` and `tb goal show` print it) and a line or two on anything that needs them. Don't paste the plan into the terminal: the goal page shows it.
