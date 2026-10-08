# Parity: the Python board's changes of 2026-10-08

The Python board (work machine, `morgan.harman-2026-10-08.zip`, `fd584e4` → `cd4959f` plus the uncommitted
`--here`) moved on after the port. Its `CHANGES-2026-10-08.md` lists 14 changes. This is each one against the Rust
board, the same way the other `parity/` files audit the web UI: what it does, where it lands natively, its status.

Status: ✅ ported · 🔧 ported with a native difference · ⏳ to do · ✋ needs a decision · — no counterpart here.

UI changes were first made to the frozen web copy (`crates/taskboard-app/parity/web/`: `alertStays`, `reviewStep`'s
re-review, `originRow`, `countedTasks`, the "From other goals" rows and "also for"), then new fixtures were added to
`parity/gen/{goal,task,chrome}.mjs` and the goldens regenerated (no existing case changed; 14 + 5 + 3 new), and the app
was made to match them.

The port is generalized (no names, hosts or repos built in), so the owner is `cfg.owner` wherever the Python says
Morgan, and the pieces that only exist on the work machine (the Review log, Fi, Slack reviewer nudges, Bitbucket
reviewer lists) have nothing to change here.

| # | Change | Python | Native | Status |
|---|---|---|---|---|
| 1 | Review log heals itself (restart `team.sebu.reviewlog`, then one alert) | `reviewlog.py` | the port has no Review log or Fi | — |
| 2 | Shared tasks: a home goal runs a task, other goals count it with `--also`; deleting the home goal moves it to the newest other goal | `shared.py`, `task_goals`; `pages.js` `countedTasks`, `goalTasks` "From other goals", `goalTaskMeta` "also for" | `shared.rs`, `task_goals`; `tb task new/set --also/--not-also`, `tb goal show`/`delete`; cards' `also`; goal detail `shared`; counts, peek, finished, goal filters, handoff, plan prompt, skill. App: `goal.rs` `counted`, `shared_row_view`, `task_meta` | ✅ `tests/oct8.rs` (3 tests); goal goldens `taskRows shared`, `header shared*`, `goalState shared*` |
| 3 | Re-review stage: addressed changes wait in `rereview` ("Awaiting re-review") until the reviewer looks again; a merge left open is retried at 1/5/15 min, then alerts once | `prflow.py` (`standing_changes`, `_merge_stalled`, `_merge_given_up`); `app.js` `reviewStep` | `prflow.rs`: GitHub's `CHANGES_REQUESTED`, answered → `rereview` (in review for Jira); the merge wake key includes the review decision; merge retries, then one alert (`merge_gave_up`). App: the Review step says "Awaiting re-review". The Slack re-ask and nudge have no counterpart | 🔧 `tests/oct8.rs` `addressed_changes_wait_in_rereview`, `tests/merge_retry.rs`; task golden `done PR: stage rereview` |
| 4 | Yes/no from a reviewer's Slack reply | `nudge.py` | no Slack nudges here | — |
| 5 | Other goals' tasks as branches joining the wave rail | `waves.js` (`waveRail`) | the goal page has no wave rail yet (the frozen web copy the app follows predates it) | ✋ |
| 6 | Live nudge replies through Fi | `nudge.py`, `/nudge/*` | no Fi or nudges here | — |
| 7 | Haiku 5.5 for the small jobs | `nudge.py`, `jira_api.py`, `screen.py` | `config.rs` `questions.model` (the screener; the port's only Haiku job) | ✅ |
| 8 | Reviewers' review bots | `prflow.py` (`bot_next_run`) | the board doesn't pick reviewers here | — |
| 9 | Never the same nudge wording twice | `nudge.py` | no nudges here | — |
| 10a | QA comments: Jira comments on board tickets become tasks or flags | `qa.py`, `POST /jira/comment` | needs a feed of Jira comments (Fi forwards them on the work machine) | ✋ |
| 10b | Task origin: every task stores `{from, by, url?}`; the task panel shows **From** | `api.py` `_origin`, `app.js` `originRow` | `tasks.origin`; `ops::origin` (only http(s) URLs kept); set by the board ("Added on the board"), `tb propose`/`goal new` ("Planned in G<n>"), `tb task new` ("Added by an agent"), Make it a task ("Backlog B<n>: title"), `--here`. App: `task_panel/view.rs` `origin_row` | ✅ `tests/oct8.rs` `every_task_says_where_it_came_from`; task goldens `context: from*`, `context: origin without from` |
| 10c | Good standing before asking reviewers | `reviewlog.holding` | no reviewer asking here | — |
| 11 | One reviewer per person | `prflow.py` `merge_people` | no reviewer rows here | — |
| 12 | Review alerts stay: no Dismiss, still Snooze; other alerts for the task don't replace it; never pushed out; clears once reviewed | `dispatch.py` `stays`; `app.js` `alertStays` | `dispatch.rs` `stays`; `POST alerts/:id/dismiss` → 409; "I reviewed it" prunes it. App: `app.rs` `alert_stays` (no Dismiss; Dismiss all skips it and hides when nothing else is left) | ✅ `tests/oct8.rs` `a_review_alert_stays_until_the_pr_is_reviewed`; chrome goldens `banner one review alert`, `alertsDialog with a review alert`, `alertsDialog only review alerts` |
| 13a | QA tab on the goal | `qa.py` `for_goal`, `pages.js` `goalQa` | follows 10a | ✋ |
| 13b | The no-wave group is called **Post** | `pages.js`, `waves.js` | follows 5 | ✋ |
| 14 | `tb task new … --here`: a standalone task for this terminal's work, claimed at once; the Stop hook blocks a turn that changed code with no task | `reports.py` `_new_task_here`, `_untracked_change`; `hook.py` `board_block` | `reports.rs` `new_task_here`, `untracked_change` (the turn's edits are read from the transcript before the transaction); `tb task new --here`; `hook.rs` blocks the Stop with `block` when nothing is being delivered; skill and the no-task line. No `--no-pr` (the port has no `ships_pr`) and no Jira desk check | 🔧 `tests/oct8.rs` `here_makes_*`, `a_turn_that_changed_code_*` |
