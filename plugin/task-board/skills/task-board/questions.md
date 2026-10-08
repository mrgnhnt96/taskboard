# Before you ask the owner

Ask the owner only about what the work should do. Make routine calls yourself, note them with `tb note`, and carry on:

- **Branch name**: follow the repo's convention; if it has none, `<type>/<short-slug>` with a conventional-commit type (`feat`, `fix`, `chore`, `test`…). Never ask what to call a branch.
- **Anything a goal note, the plan or the repo already answers**: follow it. Don't ask the owner to confirm it.
- **Waiting on another task** (its branch, commit or result, in this goal or another): that isn't a question for the owner, who can't make it finish any sooner. Run `tb wait-for T<n> --why "<what you need>"` and end your turn. If that work is ready, it prints the steps to bring it in and you carry on. If not, the task goes back to the queue and the board starts the other task first; once it's done, the board carries this conversation on with the steps. Don't build the other task's work yourself, and don't stop to ask.
- **Plans that disagree** (two goals naming something differently, an API one expects and the other shapes another way): pick the option that keeps both goals working, note it on both goals (`tb note --goal --kind decision`), and carry on.

Still the owner's call? `tb question "<the question>"`, then stop. The board may check it against the owner's rules, the task and the goal first. If it prints an answer, the question never reached the owner: follow the answer and carry on. If the answer really doesn't fit, ask again and say why.
