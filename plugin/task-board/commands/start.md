---
description: Start a task or run a goal on the board, on your word
argument-hint: "[T<n>|G<n>|the goal]"
---

This is the owner's word to start, and the board takes it as that.

Start what the owner named: $ARGUMENTS

- A task (`T<n>`): run `tb start T<n>`.
- A goal (`G<n>`), or "the goal": run `tb start G<n>` for the goal (the one this conversation is about for "the goal"). It runs its tasks in their waves; don't start them one by one.
- Nothing named: start this terminal's own task, or what you make this turn, with `tb start`.

If the board still refuses, say so and stop: it has asked the owner in a notification.
