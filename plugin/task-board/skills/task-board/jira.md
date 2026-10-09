# Jira

Only when the owner's board has Jira on: your handoff has a `Jira:` line or names a ticket, or `tb jira` says so. With Jira off, skip this file.

## Which tasks get a ticket

- A task whose work ends in a PR has one ticket. Its key goes in the branch name (unless the handoff names the branch), the commit messages and the PR title.
- When the board asks for tickets on its own (`tb jira` says "every PR task gets a ticket"), a PR task waits for its ticket before it starts, and the key reaches your context once it's linked.
- A task with no PR (an investigation, a measurement) needs no ticket.

## Linking one

- You know the ticket (the owner, the goal, the backlog issue or the work names it): `tb task set T<n> --jira PROJ-123`, or `--jira PROJ-123` on `tb task new`.
- It needs a new one: `tb task set T<n> --jira new` (or `--jira new` on `tb task new`). The board searches Jira for an open ticket that already covers the work first, and only makes one when none does.
- It needs none: `tb task set T<n> --jira none`.
- A ticket that couldn't be made ("Couldn't make the ticket: …" on the task): fix what the reason says if it's yours to fix, then `tb task set T<n> --jira new` to try again, or link the right one.

Find or make the ticket yourself; never ask the owner which ticket. Settle the ticket and the branch right away, before the first commit, so every commit carries the key.

## The Jira desk

When the board's Jira desk is on, one Claude terminal in Midna's Background group handles the board's new tickets. If you are the desk (your first prompt says "You are the task board's Jira desk"):

1. Each job comes as a message starting `[task-board:J<n>]`. Do one at a time.
2. Search Jira for an open ticket that already covers the work: its summary, its description, the goal's epic. If one does, use it; never make a duplicate, and don't change it.
3. Only if none does, make the ticket with exactly the summary, description and fields the job gives. When the job lists products, pick the one whose description fits the work and give the ticket that product's fields.
4. Report right away: `tb jira J<n> ok key=PROJ-123 status="To Do"`, adding `found=yes` when it was already there and `product=<name>` when you picked one. If it can't be done: `tb jira J<n> fail "<why>"`.
5. Never ask the owner anything, never change code, never take board tasks, and keep the terminal open: the next job comes here.

`tb jira` lists the board's Jira setup and its latest jobs; `tb jira J<n>` shows one.
