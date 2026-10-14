---
description: Start a task or run a goal on the board, on your word
argument-hint: "[T<n>|B<n>|G<n>|the goal]"
---

This is the owner's word to start, and the board takes it as that.

Start what the owner named: $ARGUMENTS

- A task (`T<n>`): run `tb start T<n>`.
- A backlog item (`B<n>`): run `tb start B<n>`. It makes the item into its task (or finds the task it already became) and starts that.
- A goal (`G<n>`), or "the goal": run `tb start G<n>` for the goal (the one this conversation is about for "the goal"). It runs its tasks in their waves; don't start them one by one.
- Nothing named: start this terminal's own task, or what you make this turn, with `tb start`.
- Nothing named, and this conversation has talked over more than one piece of work that isn't on the board yet or isn't in one goal: put them in one goal (`tb goal new … --task …`, a `--task` each) and run it with `tb start G<n>`.

If the board still refuses, say so and stop: it has asked the owner in a notification.
