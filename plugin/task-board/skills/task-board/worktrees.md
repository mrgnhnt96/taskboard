# Worktrees and the shared checkout

Read this when your task holds a slot or wants one, when `tb slot` or `tb stage` comes up, or when you commit in a checkout other tasks share.

## Slots

- Every worktree a task works in is a slot from the board's pool, `<repo>/.claude/worktrees/slot-<n>`, reused from task to task so builds stay warm and the disk doesn't fill. Don't make your own: from a task's terminal the board refuses `git worktree add`, `EnterWorktree` and subagents with `isolation: worktree` outside the pool.
- A goal that runs its tasks in worktrees starts each in a slot, detached at the base; make your branch there with `git switch -c`.
- The task holds its slot until it submits, fails or parks with `tb wait-for`, and gives it back then. That's refused while the slot has uncommitted changes or unpushed commits: commit and push them, or discard them.
- `tb slots` lists each slot, who holds it, who waits, when it was last used and its size.
- `tb slot claim` takes one for review fixes, a rebase or a failing check (its own again when free, on the pushed branch) and prints its path. `tb slot release` gives it back early. `tb slot discard [path]` saves a slot's uncommitted changes to a backup ref and clears them.
- When the task starts again in its slot, the board clears files it never edited that match its base (saved to a backup ref first) and rebases a clean, unpushed branch onto the base; its log says what it did.
- While the pool is full the task waits for a slot (`tb locks` lists `wt:slot-<n>`).
- Subagents work in the task's checkout: give each its own files.

## The shared checkout

A task with no worktree works in the shared checkout. Commit only your own files, by name; the board refuses a commit of another task's edits there.

In a file another task edits too, only your own hunks may go in: `tb stage [files]` stages just those from your recorded edits (a file only you edited goes whole; a hunk you both changed is refused naming the other task), then `git commit` without `-a` or a `git add` of that file.

## Moving to a slot

Any task (one made with `--here`, or outside a goal) can move to a slot when it wants its own:

- `tb slot claim --bring <files>` takes a slot, puts the task on the slot lane and moves its uncommitted edits in those files there from the shared checkout.
- `--bring` on its own takes the files the task edited.
- In a file another task is editing too, only your own hunks go and theirs stay; a hunk you both changed is refused naming the other task.
- Hunks no task's edits made (shell edits) are listed with ids: pick yours with `--bring-hunk <file>:<id>`, or leave them with `--bring-hunk none`.
- A refusal or clash changes nothing.
- Then `cd` to the path it prints and carry on there. If it says the slot is detached at your branch's tip, make a new branch with `git switch -c`.

`tb task new "<title>" --here --lane slot --bring <files>` does the same for a new task.
