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

## Tell the owner

Give the owner the goal's link (`tb goal new` and `tb goal show` print it) and a line or two on anything that needs them. Don't paste the plan into the terminal: the goal page shows it.
