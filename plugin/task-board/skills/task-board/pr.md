# PRs

A project with no git remote has no PRs, and neither has a task set to `--pr no`: finish with a plain `tb done "<summary>"`.

## Opening the PR

0. The owner's steps for before the PR (in your handoff; `tb steps` lists them and how to get past each; see SKILL.md). The board refuses the PR until they pass.
1. Commit the work on the task's branch. Rebase onto the latest base branch (never merge it in), run the tests, and push. A rebased branch that was already pushed goes up with `git push --force-with-lease`, never plain `--force`.
2. Open the PR with the repo's normal tools (on GitHub, `gh pr create`). Write the description for developers who have never seen the task board: a short summary of what changes and why, the changes, and how it was tested. No task, goal or backlog refs (T45, G3, B1).
3. `tb done "<summary>" --pr <url>` (or the PR's URL anywhere in the summary). The board picks up GitHub, GitLab and Bitbucket PR URLs and links the PR to the task. A plain `tb done` on a task that ends in a PR, with none linked, is refused: give `--pr`, `--pr-body`, or `--no-pr "<why>"`.

Or let the board open it (GitHub): write the description to a file with `## Summary`, `## Changes` and `## Testing` (bullet lists for the last two, short paragraphs, no board refs), push the rebased branch, and run `tb done "<summary>" --pr-body <file>` (`--title "…"` to name it; the ticket key goes first by itself). The board checks the description, the steps and the branch (pushed, rebased on the remote base, no merge commits), opens the PR into the right base and finishes. It says what to fix when it can't; `tb pr body-check <file>` checks just the description.

**No PR after all**: when the work turns out to need none (another task already did it, nothing to change), `tb done "<summary>" --no-pr "<why>"`. Name the other task (T12) or ticket (ABC-7) in the why; the owner sees it as "PR canceled: …". Keep the why to one short line (100 characters at most); a blank one is refused. `--no-evidence "<why>"` says why there's no screenshot or result to show. A task with a design attached finishes with evidence (`tb attach <screenshot or link> --kind evidence`) or `--no-evidence "<why>"`; evidence links are added to the PR's description once it's open.

**Stacked PRs**: a task made with `--stack-on T<n>` builds on T<n>'s unmerged PR. Cut its branch from T<n>'s (`origin/<its branch>`; a worktree the board made already starts there), rebase onto it, and open the PR into that branch, not main (`--pr-body` does this). The PR waits ("Waits on base") until T<n>'s merges; then the board points it at main and tells you to rebase.

## Accounts

The owner connects GitHub, Bitbucket and Slack in Taskboard ▸ Settings ▸ Accounts; `tb accounts` shows which are. On GitHub use `gh` (it's signed in there). On Bitbucket and Slack call their REST API through `tb api`, which adds the token:

- Comment: `tb api bitbucket repositories/<workspace>/<repo>/pullrequests/<n>/comments -d '{"content":{"raw":"…"}}'`
- Slack: `tb api slack chat.postMessage -d '{"channel":"#dev","text":"…"}'`

`git push` over HTTPS uses Taskboard's account for that host while you're on a task (`tb git-credential`, set up when the session starts); elsewhere git uses the Mac's own sign-in. `tb token <github|bitbucket|slack>` prints a token for a script; never echo it into a log, a commit or a message. An account that isn't connected: ask the owner with `tb question`, don't ask for a token.

## Reviewers

Set a PR's reviewers with `tb pr reviewers T<n>` (GitHub and Bitbucket alike), never with the host's own tools: the board records every ask and never asks the PR's author or anyone removed from the project's roster.

- `tb pr reviewers T<n>` alone: the board picks (a main contributor of the changed files, then whoever's turn it is) and asks them. `--dry-run` shows who it would pick; `--count N` asks N more.
- `--ask <name>` (repeat for more): a reviewer's name, alias, email or host id.
- `--replace <name>` (`--with <name>`, else the board's pick): take one off and ask another in their place. `--drop <name>`: take one off.
- It refuses while the PR feed is down, stale or just back (`tb feed` says why), and, with the project's ask stage on, before the owner has reviewed the PR. Wait; don't ask on the host instead.

The roster is the project's (`tb reviewers`, run in the project's folder or with `--project`): `list`, `sync` (commit authors join it), `add "<name>" --user <host id> --email <commit email> --alias <other name>`, `alias`, `merge <keep> <other>` (two rows that are one person), `remove <name> --reason "…"` (never ask them; only on the owner's word), `back`, `pin`/`unpin` (first in line for the main-contributor pick), `bot <name> --every <hours> --mark "<text its comments carry>"` (not asked until the board has seen a run), `auto <name> low|normal|high`.

## After the PR opens

Read this when the board brings you back about a done task's PR (`[task-board:T<n>] PR … needs you`): failed checks, review comments, ready to merge, or asking for reviews. The task is already done: don't `tb take` it or run `tb done`. Every command takes the task, `T<n>`.

The board watches GitHub and Bitbucket PRs the same way, and every `tb pr` command works on both.

1. `tb pr status T<n>` reads the PR now: its stage, each check (a failed one with its failed steps and tests where the CI can be read, each marked `[base fails this too]` when the base branch's latest builds fail that same step or test; `[base fails this check too]` when only the check could be compared), whether the base moved (with the commands to rebase onto it), the reviewers and their states, the open threads with their ids and replies, and what still stands between it and a merge.
2. Do the work:
   - **A check failed**: work from its failed steps and tests, or its log (on GitHub, `gh pr checks` and `gh run view --log-failed`), never a guess. If the PR's change causes it, fix exactly that. If it fails the same way without the change, it isn't ours: don't fix someone else's failure; clear it with `tb pr not-ours T<n> --check "<check>" --title "<what fails>" --reason "<why it isn't this PR, 20-300 characters>" --proof <link>` (repeat `--proof` and `--check`; a link to the same failure on the base branch, or an issue about it). It clears only checks that failed on this push.
   - **Open threads or changes requested**: fix each one, or put one that can fairly wait in the backlog (`tb backlog add "<title>" --kind follow`). A comment on base code the PR didn't change goes to the backlog too: reply that it's tracked and resolve it. Never reverse an earlier decision (a goal note, a checkpoint decision, the owner's answer) for a comment; reply with the decision instead. Push, then answer each thread that asked for something: `tb pr reply T<n> <thread> "<what you did>" --resolve`. A comment that asks for nothing (praise, an FYI, a question already answered) gets `tb pr ack T<n> <thread>`: it's resolved silently, never replied to. When a reviewer asked for changes and every thread is answered, `tb pr addressed T<n>` asks them to look again (it refuses while a thread is open).
   - **Asking for reviews** (the owner has reviewed it): run `tb pr reviewers T<n>`. The board picks and asks the reviewers; `--ask <name>` asks someone in particular. It finishes the visit.
   - **Ready to merge**: merge only if the wake prompt says merging is allowed, with `tb pr merge T<n>`: it checks the PR once more (checks, approvals, threads, PR tasks, a stacked base) and merges it, deleting its branch. Don't merge with the host's own tools.
3. Push the fixes. If the base branch has moved, rebase onto it first and push with `--force-with-lease`. Don't push only to rebase. A step that runs on every push (`per_head`) has to pass on the new commit first: the push is held until it does (`tb steps`).
4. Finish every visit with `tb pr wait T<n>` (or `tb pr addressed` / `tb pr merge`, which finish it too): the board closes the terminal and keeps watching the PR. `tb pr merged T<n>` records a PR someone merged outside the board.

## The owner wants changes to a done task's PR

When the owner comes to you in a terminal with changes for a done task whose PR is still open, run `tb take T<n>` first. Make the changes, push them, then `tb done "<summary>"` again. A merged or closed PR can't be taken back: open a new task instead.

## A PR feed

When the owner asks you to set up something that hears about PR changes (a Slack listener, a webhook relay), have it run `tb feed event <PR link>` for each PR change (`--kind build --state started|running|passed|failed --head <sha> --provider <ci>` for a build) and `tb feed heartbeat` every minute or so; the board reads that PR again at once. If the listener keeps its own hours, have its heartbeat say so: `--active` inside them, `--idle --idle-until <time>` outside (it may stop heartbeating until then), and `--connected-at <time>` after each connect. `tb feed` shows whether the board trusts the feed.

`tb pr-builds stop --reason "<why>"` cancels every build of the owner's PRs (the board's and ones opened by hand) and pushes board-wide (their checks count as passed) and `tb pr-builds resume` lets them run: only on the owner's word, with `--who "<owner>"`. `tb pr-builds` says whether they're stopped and lists the recent cancels.
