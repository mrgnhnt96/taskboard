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

1. `tb pr status T<n>` shows the PR's stage, its checks and its open reviews.
2. Do the work:
   - **A check failed**: read its log (on GitHub, `gh pr checks` and `gh run view --log-failed`). Work from the error, never a guess. If the PR's change causes it, fix exactly that. If it fails the same way on the base branch without the change, it isn't ours: say so with `tb note`, and don't fix someone else's failure.
   - **Comments or changes requested**: fix each one, or put one that can fairly wait in the backlog (`tb backlog add "<title>" --kind follow`). Reply to each thread with what you did.
   - **Ready to merge**: merge only if the wake prompt says merging is allowed. Then merge with the repo's tools and run `tb pr merged T<n>`.
3. Push the fixes. If the base branch has moved, rebase onto it first and push with `--force-with-lease`. Don't push only to rebase. A step that runs on every push (`per_head`) has to pass on the new commit first: the push is held until it does (`tb steps`).
4. Finish every visit with `tb pr wait T<n>` (or `tb pr merged T<n>` after a merge): the board closes the terminal and keeps watching the PR.

## The owner wants changes to a done task's PR

When the owner comes to you in a terminal with changes for a done task whose PR is still open, run `tb take T<n>` first. Make the changes, push them, then `tb done "<summary>"` again. A merged or closed PR can't be taken back: open a new task instead.
