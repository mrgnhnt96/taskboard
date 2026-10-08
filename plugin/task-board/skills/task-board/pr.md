# PRs

A project with no git remote has no PRs: finish with a plain `tb done "<summary>"`.

## Opening the PR

0. The owner's steps for before the PR (in your handoff; `tb steps` lists them and how to get past each; see SKILL.md). The board refuses the PR until they pass.
1. Commit the work on the task's branch. Rebase onto the latest base branch (never merge it in), run the tests, and push. A rebased branch that was already pushed goes up with `git push --force-with-lease`, never plain `--force`.
2. Open the PR with the repo's normal tools (on GitHub, `gh pr create`). Write the description for developers who have never seen the task board: a short summary of what changes and why, the changes, and how it was tested. No task, goal or backlog refs (T45, G3, B1).
3. `tb done "<summary>"` with the PR's URL anywhere in the summary (or in your last message). The board picks up GitHub, GitLab and Bitbucket PR URLs and links the PR to the task.

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
3. Push the fixes. If the base branch has moved, rebase onto it first and push with `--force-with-lease`. Don't push only to rebase.
4. Finish every visit with `tb pr wait T<n>` (or `tb pr merged T<n>` after a merge): the board closes the terminal and keeps watching the PR.

## The owner wants changes to a done task's PR

When the owner comes to you in a terminal with changes for a done task whose PR is still open, run `tb take T<n>` first. Make the changes, push them, then `tb done "<summary>"` again. A merged or closed PR can't be taken back: open a new task instead.
