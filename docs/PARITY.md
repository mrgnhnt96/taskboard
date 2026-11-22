# Parity with the web board

Taskboard.app replaced a web UI (`crates/taskboard-app/parity/web/`, kept frozen as the reference).
This is the audit of the native app against it, one web behaviour per row, done in October 2026.

The area files are a record of that audit. The app has moved on since: pages were dropped or redesigned and many
of the view-model functions and tests they name were renamed or removed. Read them for what the web did and why
the app differs; for what checks parity today, see [How it's tested](#how-its-tested).
The Python board's own changes after the port (2026-10-08) are checked the same way in
[oct8-changes.md](parity/oct8-changes.md).

| Area | Web functions | Checklist | Same | Fixed in the audit | Native difference |
|---|---|---|---|---|---|
| Chrome: sidebar (goals rail + goal list), banner, alerts, status bar, hours menu, toasts, title, Esc, markdown | `goalsRailHtml`, `goalNavList`, `renderBanner`, `statusBarHtml`, `hoursMenuHtml`, `mdLite`, … | [chrome.md](parity/chrome.md) | 11 | 39 | 7 |
| Board | `renderBoard`, `sessionsHtml`, `columnsHtml`, `taskCard`, `issueCard`, drag to Working, rename | [board.md](parity/board.md) | 13 | 19 | 2 |
| Task panel | `taskPanel`, `overviewTab`, `contextTab`, `logTab`, `prBar`, `attachList`, `metaRows`, … | [task.md](parity/task.md) | 23 | 24 | 1 |
| Goal page | `goalMain`, `goalTasks`, `notesAside`, `goalBacklog`, `bulkBar`, `goalRunButtons`, … | [goal.md](parity/goal.md) | 10 | 34 | 2 |
| Issue panel (the web's Backlog page is replaced by the native planning page, `ui/backlog.rs`) | `issueAside`, `issuePanel`, `issueActions`, … | [backlog.md](parity/backlog.md) | 12 | 29 | 0 |
| Sessions page | removed (terminal links show the terminal in Midna); [sessions.md](parity/sessions.md) is the old audit | — | — | — | — |
| Forms and pickers | removed (the app has no forms); `localeCompare` still checked | [forms.md](parity/forms.md) | — | — | — |
| **Total** | | | **69** | **145** | **12** |

"Fixed in the audit" means the first native version differed from the web and now matches it: wording,
what shows when, ordering, which buttons are offered, request bodies, keys, saved state. Every native
difference is listed with its reason in the area's file; none drops a capability. They are layout
(one sidebar on every page instead of a rail on the board and a list on the goal page), things a native
window does differently (no URL routes, no keyboard focus on plain buttons, popovers that close on
outside click), native additions (menu-bar shortcuts, the login-item banner), and two
web bugs not carried over (the `#/goals` landing threw; a terminal from another project could be sent
with a new task).

## What the app changes

The owner rarely makes or changes board data in the app; Claude does that through `tb` when asked. The
2026-10-08 audit removed the app's forms (New task, New goal, Edit goal, Add an issue, Mark done… / Mark failed…,
Add a note, attachment Edit / Remove, the bulk "Move to a goal") from the app and from the frozen web copy, so
the goldens still match.

What the app does change, each one the owner asked for by name:

- one-click signals: start, resume, pause and stop a task or goal (drag to Working, Run, Pause, Close its
  terminal, Detach, Requeue, Queue it), "I reviewed it" on a PR waiting for you, dismissing alerts, a wave's
  "Stop after this wave for my review" (below), and buttons that start agent work on an object (an idea's Dig
  and Plan, Talk it over on a human task);
- answers: a task's open question (a choice, or typed), a human task's Done and its other answers with their
  optional note, and an idea's question;
- the backlog's triage and close buttons, move to a goal, and finishing a goal from its issue;
- New idea, and shelving, dropping or reopening an idea;
- Undo on the toast after an answer or a backlog change;
- Settings: work hours, the worktree pool and the Jira account.

Details editing has left the task panel; anything not listed goes through `tb`.

Brought back from the Python board (past the frozen web copy, so not in its goldens): a wave's "Stop after this
wave for my review" checkbox, the owner's own word (the API refuses `stop_after` unless the app sends it, with
`X-Task-Board-From: app` from the signed app on its socket, see API.md "The app's own requests"; agents hold a wave with `tb goal wave --hold` instead); the goal page's Devices aside
(who has each device, Focus) and Bits aside ("N of M created", Copy, Copy link, "Create in <tool>", "Local, not
in <tool>", and a one-click "Mark created" on a backend bit not made yet, the owner's signal like "I reviewed it";
agents record it with `tb bit made`; the button has since left the app); device and ⚑ bit chips on cards and wave rows; Devices and Bits rows in the
task's Context tab; a goal stopped at a wave shows "Waiting on you" once nothing in it is working, starting or
paused (rail: the stop line and a stop-square glyph; goal list: "Waiting on you" with the stop line as its
tooltip); a goal is Blocked only when some queued task is blocked (the rail also needs its blocked plus held tasks
to cover every queued one); and a done goal with unmade backend bits shows "Waiting on N bits" (unmade bits never
hold a task's start).

## How it's tested

- **Goldens from the web's own code.** `parity/gen/<area>.mjs` loads the frozen web UI in node, runs its
  functions on fixtures (every state each area showed) and writes `parity/golden/<area>.json`: the
  visible text, the actions offered, the request paths and bodies. The goldens hold 914 cases; the tests below
  check the groups of them that still have a native counterpart. Regenerate with
  `crates/taskboard-app/parity/regen.sh` (needs node; the output is deterministic: frozen clock, UTC).
- **Action tests.** A headless `MainWindow` (`parity::window`) on a recording backend (the real board
  in-process) presses a button and checks the exact request it sends, and what the window does next.
  `parity::Recording::fail` makes fetches fail to check the error states.

The tests that hold parity today, all in `crates/taskboard-app/src`:

| Area | Tests |
|---|---|
| Shared helpers | `parity.rs`: `fmt_matches_web` (times, durations, refs), `filters_persist_like_the_web` (the web's localStorage keys); `backend.rs`: `errors_read_like_the_web`; `ui/md.rs`: `md_lite_matches_web`; `ui/modals/tests.rs`: `names_sort_like_the_web` (`localeCompare`) |
| Chrome | `app.rs`: `banner_and_alerts_dialog_match_web`, `status_bar_matches_web`, `dismissing_alerts_posts_like_the_web`, `escape_follows_the_web_order`; `ui/hours.rs`: `pill_matches_web`, `menu_matches_web`, `save_body_matches_web`; `ui/goals.rs`: `counts_and_clear_match_web` |
| Board | `ui/board.rs`: `state_query_matches_web`, `keep_issue_only_on_the_board` |
| Task panel | `ui/task_panel/tests.rs`: `panel_text_matches_web`, `panel_controls_match_web`, `panel_tooltips_match_web`, `panel_fields_match_web`, `helpers_match_web` |
| Goal page | `ui/goal.rs`: `run_signals_match_web`, `run_toasts_match_web` |
| Issue panel | `ui/issue_panel.rs`: `issue_labels_match_web`, `issue_panel_matches_web`, `picker_items_rank_and_filter_like_web`, `panel_actions_send_what_the_web_sent` |

Run one with `cargo test -p taskboard-app <name>`.
