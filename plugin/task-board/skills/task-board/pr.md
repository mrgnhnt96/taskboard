# PRs

A project with no git remote has no PRs, and neither has a task set to `--pr no`: finish with a plain `tb done "<summary>"`.

## Opening the PR

0. The owner's steps for before the PR (in your handoff; `tb steps` lists them and how to get past each; see SKILL.md). The board refuses the PR until they pass.
1. Commit the work on the task's branch. Rebase onto the latest base branch (never merge it in), run the tests, and push. A rebased branch that was already pushed goes up with `git push --force-with-lease`, never plain `--force`.
2. Open the PR with the repo's normal tools (on GitHub, `gh pr create`). Write the description for developers who have never seen the task board: a short summary of what changes and why, the changes, and how it was tested. No task, goal or backlog refs (T45, G3, B1).
3. `tb done "<summary>"` with the PR's URL anywhere in the summary (or in your last message). The board picks up GitHub, GitLab and Bitbucket PR URLs and links the PR to the task.

Or let the board open it (GitHub): write the description to a file with `## Summary`, `## Changes` and `## Testing` (bullet lists for the last two, short paragraphs, no board refs), push the rebased branch, and run `tb done "<summary>" --pr-body <file>` (`--title "…"` to name it; the ticket key goes first by itself). The board checks the description, the steps and the branch (pushed, rebased on the remote base, no merge commits), opens the PR into the right base and finishes. It says what to fix when it can't; `tb pr body-check <file>` checks just the description.

**No PR after all**: when the work turns out to need none (another task already did it, nothing to change), `tb done "<summary>" --no-pr "<why>"`. Name the other task (T12) or ticket (ABC-7) in the why; the owner sees it as "PR canceled: …". `--no-evidence "<why>"` says why there's no screenshot or result to show.

**Stacked PRs**: a task made with `--stack-on T<n>` builds on T<n>'s unmerged PR. Cut its branch from T<n>'s (`origin/<its branch>`; a worktree the board made already starts there), rebase onto it, and open the PR into that branch, not main (`--pr-body` does this). The PR waits ("Waits on base") until T<n>'s merges; then the board points it at main and tells you to rebase.

## Accounts

The owner connects GitHub, Bitbucket and Slack in Taskboard ▸ Settings ▸ Accounts; `tb accounts` shows which are. On GitHub use `gh` (it's signed in there). On Bitbucket and Slack call their REST API through `tb api`, which adds the token:

- Comment: `tb api bitbucket repositories/<workspace>/<repo>/pullrequests/<n>/comments -d '{"content":{"raw":"…"}}'`
- Reviewers: `tb api bitbucket repositories/<workspace>/<repo>/pullrequests/<n> -X PUT -d '{"title":"…","reviewers":[{"account_id":"…"}]}'` (send the title too)
- Slack: `tb api slack chat.postMessage -d '{"channel":"#dev","text":"…"}'`

`git push` over HTTPS uses Taskboard's account for that host while you're on a task (`tb git-credential`, set up when the session starts); elsewhere git uses the Mac's own sign-in. `tb token <github|bitbucket|slack>` prints a token for a script; never echo it into a log, a commit or a message. An account that isn't connected: ask the owner with `tb question`, don't ask for a token.

## After the PR opens

Read this when the board brings you back about a done task's PR (`[task-board:T<n>] PR … needs you`): failed checks, review comments, or ready to merge. The task is already done: don't `tb take` it or run `tb done`. Every command takes the task, `T<n>`.

The board watches GitHub and Bitbucket PRs the same way, and every `tb pr` command works on both.

1. `tb pr status T<n>` reads the PR now: its stage, each check (a failed one with its failed steps and tests where the CI can be read, and `[base fails this too]` when the base branch's latest builds fail it), whether the base moved, the reviewers and their states, the open threads with their ids, and what still stands between it and a merge.
2. Do the work:
   - **A check failed**: work from its failed steps and tests, or its log (on GitHub, `gh pr checks` and `gh run view --log-failed`), never a guess. If the PR's change causes it, fix exactly that. If it fails the same way without the change, it isn't ours: don't fix someone else's failure; clear it with `tb pr not-ours T<n> --check "<check>" --title "<what fails>" --reason "<why it isn't this PR, 20-300 characters>" --proof <link>` (repeat `--proof` and `--check`; a link to the same failure on the base branch, or an issue about it). It clears only checks that failed on this push.
   - **Open threads or changes requested**: fix each one, or put one that can fairly wait in the backlog (`tb backlog add "<title>" --kind follow`). A comment on base code the PR didn't change goes to the backlog too: reply that it's tracked and resolve it. Never reverse an earlier decision (a goal note, a checkpoint decision, the owner's answer) for a comment; reply with the decision instead. Push, then answer each thread that asked for something: `tb pr reply T<n> <thread> "<what you did>" --resolve`. A comment that asks for nothing (praise, an FYI, a question already answered) gets `tb pr ack T<n> <thread>`: it's resolved silently, never replied to. When a reviewer asked for changes and every thread is answered, `tb pr addressed T<n>` asks them to look again (it refuses while a thread is open).
   - **Ready to merge**: merge only if the wake prompt says merging is allowed, with `tb pr merge T<n>`: it checks the PR once more (checks, approvals, threads, PR tasks, a stacked base) and merges it, deleting its branch. Don't merge with the host's own tools.
3. Push the fixes. If the base branch has moved, rebase onto it first and push with `--force-with-lease`. Don't push only to rebase. A step that runs on every push (`per_head`) has to pass on the new commit first: the push is held until it does (`tb steps`).
4. Finish every visit with `tb pr wait T<n>` (or `tb pr addressed` / `tb pr merge`, which finish it too): the board closes the terminal and keeps watching the PR. `tb pr merged T<n>` records a PR someone merged outside the board.

## The owner wants changes to a done task's PR

When the owner comes to you in a terminal with changes for a done task whose PR is still open, run `tb take T<n>` first. Make the changes, push them, then `tb done "<summary>"` again. A merged or closed PR can't be taken back: open a new task instead.
