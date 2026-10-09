# Before you ask the owner

Ask the owner only about what the work should do. Make routine calls yourself, note them with `tb note`, and carry on:

- **Branch name**: the goal's setup, then the handoff's branch name; otherwise the repo's convention; if it has none, `<type>/<short-slug>` with a conventional-commit type (`feat`, `fix`, `chore`, `test`…), with the Jira key in it when the task has one. Never ask what to call a branch.
- **Which Jira ticket** (when the board has Jira): link the one the work names (`tb task set T<n> --jira KEY`) or ask for a new one (`--jira new`); the board searches for an existing one first. Never ask the owner which ticket, and settle the ticket and branch right away (`jira.md`).
- **A review finding on code this change didn't touch**: not yours to fix. Put it in the backlog and dismiss it; don't ask the owner, and don't undo an earlier decision for it.
- **Anything a goal note, the plan or the repo already answers**: follow it. Don't ask the owner to confirm it.
- **Waiting on another task** (its branch, commit or result, in this goal or another): that isn't a question for the owner, who can't make it finish any sooner. Run `tb wait-for T<n> --why "<what you need>"` and end your turn. If that work is ready, it prints the steps to bring it in and you carry on. If not, the task goes back to the queue and the board starts the other task first; once it's done, the board carries this conversation on with the steps. Don't build the other task's work yourself, and don't stop to ask.
  - **Needs it merged, not just done** (you can only build on it once it's on the base branch): `tb wait-for T<n> --merged --why "…"`. The task waits until the other task is done and its PR has merged (work that ships no PR counts as merged), then carries on here. A plain `wait-for` sees done work as ready and doesn't park, so without `--merged` you'd end your turn with nothing to wait on. If its PR is declined, the card says so and the task keeps waiting; ask the owner only then.
- **Plans that disagree** (two goals naming something differently, an API one expects and the other shapes another way): pick the option that keeps both goals working, note it on both goals (`tb note --goal --kind decision`), and carry on.

Still the owner's call? `tb question "<the question>"`, then stop. The board may check it against the owner's rules, the task and the goal first. If it prints an answer, the question never reached the owner: follow the answer and carry on. If the answer really doesn't fit, ask again and say why.
