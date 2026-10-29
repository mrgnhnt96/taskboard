# PRs

A project with no git remote has no PRs, and neither has a task set to `--pr no`: finish with a plain `tb submit "<summary>"`.

## Opening the PR

The board opens the PR; opening it yourself from a task's terminal (`gh pr create`, a POST to `…/pulls` or `…/pullrequests`, a create-PR MCP tool) is refused.

0. The owner's steps for before the PR (in your handoff; `tb steps` lists them and how to get past each; see SKILL.md). The board refuses the PR until they pass.
1. Commit the work on the task's branch. Rebase onto the latest base branch (never merge it in), run the tests that cover your change (not the whole suite), and push. A rebased branch that was already pushed goes up with `git push --force-with-lease`, never plain `--force`.
2. Write the description to a file with `## Summary`, `## Changes` and `## Testing` (bullet lists for the last two, short paragraphs), for developers who have never seen the task board: what changes and why, the changes, and how it was tested. No task, goal or backlog refs (T45, G3, B1).
3. `tb submit "<summary>" --pr-body <file>` (`--title "…"` to name it; the ticket key goes first by itself). The board checks the description, the steps and the branch (pushed, rebased on the remote base, no merge commits), opens the PR into the right base and finishes. It says what to fix when it can't; `tb pr body-check <file>` checks just the description. In a shared checkout you can't switch (another task's edits are there), push the work to its own branch off the remote base (`git push origin <sha>:refs/heads/<branch>`, or from a branch rebuilt on it) and add `--branch <branch>`: the PR opens from that pushed branch and the checkout stays where it is. The board adds a `## Context` section itself (the ticket, a stacked base, evidence, designs, and the task's bits with how to turn them on), so don't repeat those.

A PR that was already open before the task (someone else's, or one the task was made for) is linked with `tb submit "<summary>" --pr <url>` (or its URL anywhere in the summary); the board picks up GitHub, GitLab and Bitbucket PR URLs. On GitLab, where the board can't open merge requests, open it with `glab mr create` and link it the same way. A plain `tb submit` on a task that ends in a PR, with none linked, is refused: give `--pr-body`, `--pr`, or `--no-pr "<why>"`.

**A ticket for every PR**: while the board has Jira, `tb submit` with a PR is refused for a task with no ticket. Link one with `tb task set T<n> --jira ABC-123` (or `--jira new` to have the board find or make one), put the key in the PR title, then run `tb submit` again.

**No PR after all**: when the work turns out to need none (another task already did it, nothing to change), `tb submit "<summary>" --no-pr "<why>"`. Name the other task (T12) or ticket (ABC-7) in the why; the owner sees it as "No PR: …". Keep the why to one short line (100 characters at most); a blank one is refused. `--no-evidence "<why>"` says why there's no screenshot or result to show. A task with a design attached finishes with evidence (`tb attach <screenshot or link> --kind evidence`) or `--no-evidence "<why>"`; evidence links are added to the PR's description once it's open.

**Stacked PRs**: a task made with `--stack-on T<n>` builds on T<n>'s unmerged PR. Cut its branch from T<n>'s (`origin/<its branch>`; a worktree the board made already starts there), rebase onto it, and open the PR into that branch, not main (`--pr-body` does this). The PR waits ("Waits on base") until T<n>'s merges; then the board points it at main and tells you to rebase. A task made with `--stack-merge head` (`tb status T<n>` ends its stack line in "head merge") is never moved to main: its PR goes through review as usual and `tb pr merge` merges it into T<n>'s branch. T<n>'s PR holds ("Waits on stack") until its head-merge children are merged into it, then takes the whole stack to main in one merge; the board counts a head-merged PR as merged once that happens. A PR into a branch other than main often runs no checks, so run the tests your change affects before you open it. A stacked PR the host has going anywhere but T<n>'s branch gets an alert: point it at that branch.

**Pushes**: every push to a PR into main runs its full checks. Push a PR into main only once its tests pass locally, and push once per review round (commit each fix, then push them together). Rebase a branch with an open PR only when it conflicts or right before it merges: a rebase is a push.

## Accounts

The owner connects GitHub, Bitbucket and Slack in Taskboard ▸ Settings ▸ Accounts; `tb accounts` shows which are. Every call on an account counts against the host's hourly rate limit, shared with the board's own calls and the owner's other tools, so go through the board.

**Your PR: use `tb pr`.** It reads from the board's copy of the PR, which the board keeps up to date, so it usually costs no call at all:

- What's going on (stage, checks, review, open threads with their ids): `tb pr status`
- One thread in full: `tb pr thread <id>`
- Answer a thread: `tb pr reply <id> "…"`; resolve one that asks for nothing: `tb pr ack <id>`
- Reviewers and merging: `tb pr reviewers`, `tb pr merge` (`tb pr close` only on the owner's word)
- Waiting for a review or checks: `tb pr wait` (the board brings you back), never a loop of reads

**Anything else on Bitbucket or Slack: `tb api`.** It adds the token and goes through the board, which paces it, counts it and keeps its reads: asking for the same thing again soon gets the board's answer (a PR's reads until the board sees the PR change, a commit's files and diff for an hour, the rest for a minute; `--fresh` asks the host again). It says so when `tb pr` covers the call.

- A new top-level PR comment: `tb api bitbucket repositories/<workspace>/<repo>/pullrequests/<n>/comments -d '{"content":{"raw":"…"}}'`
- A file at a commit: `tb api bitbucket repositories/<workspace>/<repo>/src/<commit>/<path>`
- Slack: `tb api slack chat.postMessage -d '{"channel":"#dev","text":"…"}'`

On GitHub, `gh` is signed in; prefer `tb pr` there too.

`git push` over HTTPS uses Taskboard's account for that host while you're on a task (`tb git-credential`, set up when the session starts); elsewhere git uses the Mac's own sign-in. `tb token <github|bitbucket|slack>` prints a token for a script outside the board; don't use it for calls `tb api` can make (they skip the board's rate limits), and never echo it into a log, a commit or a message. An account that isn't connected: ask the owner with `tb question`, don't ask for a token.

## Reviewers

Set a PR's reviewers with `tb pr reviewers T<n>` (GitHub and Bitbucket alike), never with the host's own tools: the board records every ask and never asks the PR's author or anyone removed from the project's roster.

- `tb pr reviewers T<n>` alone: the board picks (a main contributor of the changed files, then whoever's turn it is) and asks them. `--dry-run` shows who it would pick; `--count N` asks N more.
- `--ask <name>` (repeat for more): a reviewer's name, alias, email or host id.
- `--replace <name>` (`--with <name>`, else the board's pick): take one off and ask another in their place. `--drop <name>`: take one off.
- It refuses while the PR feed is down, stale or just back (`tb feed` says why), and, with the project's ask stage and owner review both on, before the owner has reviewed the PR (unless someone is already on its reviewers, or it's approved or being re-reviewed). Wait; don't ask on the host instead.

The roster is the project's (`tb reviewers`, run in the project's folder or with `--project`): `list`, `sync` (commit authors join it), `add "<name>" --user <host id> --email <commit email> --alias <other name>`, `alias`, `merge <keep> <other>` (two rows that are one person), `remove <name> --reason "…"` (never ask them; only on the owner's word), `back`, `pin`/`unpin` (first in line for the main-contributor pick), `bot <name> --every <hours> --mark "<text its comments carry>"` (not asked until the board has seen a run), `auto <name> low|normal|high`.

## After the PR opens

Read this when the board brings you back about a done task's PR (`[task-board:T<n>] PR … needs you`): failed checks, review comments, ready to merge, or asking for reviews. The task is already done: don't `tb take` it or run `tb submit`. Every command takes the task, `T<n>`.

The board watches GitHub and Bitbucket PRs the same way, and every `tb pr` command works on both.

1. `tb pr status T<n>` reads the PR now: its stage, each check (a failed one with its failed steps and tests where the CI can be read, each marked `[base fails this too]` when the base branch's latest builds fail that same step or test; `[base fails this check too]` when only the check could be compared), whether the base moved (with the commands to rebase onto it), the reviewers and their states, the open threads with their ids and replies (clipped; `tb pr thread T<n> <thread>` prints one's whole text and replies, read with the board's account), and what still stands between it and a merge.
2. Do the work:
   - **A check failed**: work from its failed steps and tests, or its log (on GitHub, `gh pr checks` and `gh run view --log-failed`), never a guess. If the PR's change causes it, fix exactly that. If it fails the same way without the change, it isn't ours: don't fix someone else's failure; clear it with `tb pr not-ours T<n> --check "<check>" --title "<what fails>" --reason "<why it isn't this PR, 20-300 characters>" --proof <link>` (repeat `--proof` and `--check`; a link to the same failure on the base branch, or an issue about it). It clears only checks that failed on this push. If only the owner can fix it (a missing secret or variable, a permissions error), don't work around it: run `tb pr needs-owner T<n> --reason "<what the owner has to do, 20-300 characters>"` (`--check` to name the checks); it tells them and finishes the visit.
     - Write the title and reason for a person, not a log: the owner reads them on the task page and finds the detail in the proof links. Keep them brief, in plain words and names, with no test names, file paths, line numbers or build ids. The title says what the owner learns ("Already broken on main, fixed by Pat's #163"), not "failed, but not because of this PR"; the reason says who or what broke it and why it isn't this PR, in two sentences at most.
     - No speculative fixes: read the log before you change anything, and don't push a fix you guessed. Revert one you made before reading it. Each pushed guess runs the checks again and stays in the PR's history.
   - **Open threads or changes requested**: fix each one, or put one that can fairly wait in the backlog (`tb backlog add "<title>" --kind follow`). A comment on base code the PR didn't change goes to the backlog too: reply that it's tracked and resolve it. Never reverse an earlier decision (a goal note, a checkpoint decision, the owner's answer) for a comment; reply with the decision instead. Push, then answer each thread that asked for something: `tb pr reply T<n> <thread> "<what you did>" --resolve`. A comment that asks for nothing (praise, an FYI, a question already answered) gets `tb pr ack T<n> <thread>`: it's resolved silently, never replied to. When a reviewer asked for changes and every thread is answered, `tb pr addressed T<n>` asks them to look again (it refuses while a thread is open).
   - **The description is out of date** (the change grew, or a reviewer asks for it): write the whole new description to a file, as for `--pr-body`, and run `tb pr body T<n> <file>` (`-` reads it from stdin). It checks it as `tb pr body-check` does and replaces the PR's description on the host; the board's own `## Code review` and `## Context` sections stay as they are, so leave them out. Don't edit the description with the host's own tools.
   - **Asking for reviews** (the owner has reviewed it, or owner review is off): run `tb pr reviewers T<n>`. The board picks and asks the reviewers; `--ask <name>` asks someone in particular. It finishes the visit.
   - **Ready to merge**: merge only if the wake prompt says merging is allowed, or the owner tells you in this terminal to merge it (their "approved" there counts as the approval), with `tb pr merge T<n>`: it checks the PR once more (checks, approvals, threads, PR tasks, a stacked base) and merges it, deleting its branch. Don't merge with the host's own tools.
   - **The owner's review**: with owner review on, reviewers aren't asked (`tb pr reviewers` is refused) until the owner has looked at the PR. When the owner tells you in this terminal they've reviewed it ("mark PR #N reviewed", by its number, its task or "the PR"), run `tb pr reviewed T<n>`; once checks pass it goes on to asking reviewers. Never mark it on your own judgment.
   - **Closing without a merge**: only when the owner tells you in this terminal to close (or decline) this PR, by its number, its task or "the PR": `tb pr close T<n> [--reason "…"]` closes it on GitHub (declines it on Bitbucket), posts the reason as a comment and keeps its branch. Never close one on your own judgment, and don't close it with the host's own tools.
3. Push the fixes. If the base branch has moved, rebase onto it first and push with `--force-with-lease`. Don't push only to rebase. A step that runs on every push (`per_head`) has to pass on the new commit first: the push is held until it does (`tb steps`).
4. Finish every visit with `tb pr wait T<n>` (or `tb pr addressed` / `tb pr merge`, which finish it too): the board closes the terminal and keeps watching the PR. `tb pr merged T<n>` records a PR someone merged outside the board; one closed outside it the board reads on its own. When the owner asks you to check the PRs now, `tb pr check` reads every PR on the board from its host at once, instead of waiting for the next poll.

## Rate limits

Every call to GitHub and Bitbucket goes through the board, which keeps each account under its hourly budget and spaces out its writes, so a `tb pr …` command can take a few seconds longer. When a command says the host has the account rate-limited, or that the board is easing back in, don't sleep and retry or schedule your own retries: replies and resolves are held and posted for you, and the board brings you back when the PR needs you. Finish with `tb pr wait T<n>`. `tb pr lift` ends the board's own hold; run it only on the owner's word.

## The owner wants changes to a done task's PR

When the owner comes to you in a terminal with changes for a done task whose PR is still open, run `tb take T<n>` first. Make the changes, push them, then `tb submit "<summary>"` again. With owner review on, until a peer has been asked to review, taking it back drops the owner's "reviewed" mark: the PR waits for the owner to look at the new changes before anyone else is asked. With peers already on it, it goes to re-review as it is. A merged or closed PR can't be taken back: open a new task instead.

## A PR feed

When the owner asks you to set up something that hears about PR changes (a Slack listener, a webhook relay), have it run `tb feed event <PR link>` for each PR change (`--kind build --state started|running|passed|failed --head <sha> --provider <ci>` for a build) and `tb feed heartbeat` every minute or so; the board reads that PR again at once. If the listener keeps its own hours, have its heartbeat say so: `--active` inside them, `--idle --idle-until <time>` outside (it may stop heartbeating until then), and `--connected-at <time>` after each connect. `tb feed` shows whether the board trusts the feed.

`tb pr-builds stop --reason "<why>"` cancels every build of the owner's PRs (the board's and ones opened by hand) and pushes board-wide (their checks count as passed) and `tb pr-builds resume` lets them run: only on the owner's word, with `--who "<owner>"`. `tb pr-builds` says whether they're stopped and lists the recent cancels.
