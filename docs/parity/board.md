# Parity: Board page

Reference: `crates/taskboard-app/parity/web/app.js` (`renderBoard` and what it calls). Native: `crates/taskboard-app/src/ui/board.rs`.
Goldens: `parity/gen/board.mjs` → `parity/golden/board.json` (25 cases: 11 board scenarios, 3 state queries, 3 goal-picks,
2 done-windows, 2 keepIssue, 4 nameView). Tests are in `board.rs` (`ui::board::tests::*`).

Status: ✅ same · 🔧 fixed in this pass · ↔ intentional native difference

| Web function | Behaviour | Native | Status | Test |
|---|---|---|---|---|
| `renderBoard` | Board = goals rail + sessions strip + goal bar + columns; no filter bar of its own | `render` | 🔧 removed the native-only top bar (title, Project/Goal pickers, "Clear filters", Done segmented control): the web board had none. Filtering comes from the goals rail (sidebar) and the goal bar | `board_matches_web` |
| `renderBoard` | Goals rail on the board's left, resizable (`tb.rail.w`) | sidebar | ↔ the native sidebar holds the rail on every page (sidebar audit) | — |
| `sessionsHtml` | Heading "Sessions" (links to the Sessions page, tooltip "See every session") | `sessions_strip` | 🔧 was "Terminals" | `board_matches_web` |
| `sessionsHtml` | Loading… / "Can’t load the sessions." before the first state | `strip_vm` | ✅ | `board_matches_web` (loading, cannot load) |
| `sessionsHtml` | Only the filtered project's terminals; order needs → working → idle → gone (unknown = idle), stable | `strip_vm` | ✅ | `board_matches_web` (project webapp) |
| `sessionsHtml` | At most 6 cards; "Showing 6 of N" | `strip_vm` | ✅ | `board_matches_web` (many sessions) |
| `sessionsHtml` | Empty: "No Midna terminals[ in P] yet. They show up here once Midna lists them." | `strip_vm` | ✅ | `board_matches_web` (empty board) |
| `sessCard` | Dot, name, state label (Idle/Working/Needs you/Gone; unknown → Idle) | `session_card` | ✅ | `board_matches_web` |
| `sessCard` | Project line with the folder as tooltip | `session_card` | ✅ | `board_matches_web` |
| `sessCard` | Sub line: task title, clickable (opens the task, tooltip "Open T2") when it has a task; plain title otherwise | `session_card` | 🔧 native showed "T2 title" and idle time ("Idle 5m"): the web shows only the title and no idle time | `board_matches_web` |
| `sessCard` | Card click opens the terminal on the Sessions page (tooltip "See what X has done"); needs = warn border (not a warn background); gone = faded | `open_session`, `session_card` | 🔧 needs background removed, gone fading added, tooltip added | `open_session_goes_to_the_sessions_page` |
| `nameView` | Renaming: new name greyed, "Renaming in Midna…"; last failure: "Last rename failed: …"; no name → id; tooltip ends " · double-click to rename" | `name_view` | 🔧 native showed no tooltip and an empty name | `name_view_matches_web`, `board_matches_web` |
| `renameFlash` | After a rename settles: green flash 1.2 s if it took, red 1.8 s "Rename failed: …" (or "Midna kept the old name") | `update_flashes` | 🔧 added | — (timer-driven; logic in `name_view`) |
| `startRename` / dblclick | Double-click a strip name to edit it in place (single click waits 260 ms, then opens the terminal) | `start_rename`, name `on_click` | 🔧 added (was missing on the strip) | — |
| `endRename` | ↩ / focus-out save, esc cancels; nothing sent for empty/unchanged; shown as renaming at once; error → renaming cleared, `rename_error` set | `end_rename` | 🔧 added | `rename_from_the_strip` |
| `goalBarHtml` | When a goal is shown: ⚑ name, "d of n done · k need(s) you · w working · i in the backlog · Deprioritized/Paused", "Open goal", ✕ (Show every goal) | `goal_bar_vm`, `goal_bar` | ✅ (goal now looked up by ref) | `board_matches_web` (goal G1/G2/G3) |
| `goal-pick` (✕) | Picking `all` also resets the project to all; picking a goal sets its project; picking the shown goal again shows all; clears the kept issues; saved | `goal_pick` | 🔧 native ✕ only cleared the goal | `goal_pick_and_done_window_match_web` |
| `inFilter`, `filterTasks`, `filterIssues` | Columns re-filter on the page by project and goal (goal matched by ref) | `columns_vm` | 🔧 native didn't filter client-side | `board_matches_web` (project/goal scenarios) |
| `columnsHtml` | Loading… / "Can’t load the board." in every column before the first state; no counts | `columns_vm` | ✅ | `board_matches_web` |
| `columnsHtml` (backlog) | Count = `backlog_open` (else shown open); "See all"; "N more on the Backlog page" before "Nothing here" | `columns_vm` | ✅ | `board_matches_web` |
| `columnsHtml` (done) | Count = shown; empty "Nothing in the last 24 hours/last 7 days/whole history" when some are hidden; "N older task(s) hidden · show more" → 24h→7d, else all | `columns_vm` | ✅ | `board_matches_web` (done hidden) |
| `colShell` | Square, name, count pill, extra at the end; column note under the head | `columns` | ✅ | `board_matches_web` |
| `doneMenuButton` | "Show" + ✓ on the current window; "Close their terminals" disabled with no done tasks | `done_menu_vm`, `done_menu` | 🔧 removed native-only tooltip on the close item | `board_matches_web` |
| `done-window` | Sets the window, saves the filters, keeps the kept issues | `set_done_window` | 🔧 now persisted (`taskboard.filter`) | `goal_pick_and_done_window_match_web`, `done_window_saves_the_filters` |
| `close-done` | POST `/done/close-terminals {}`; the Done column's note says `sentNote()` ("Sent to Midna" / "Saved. It runs once Midna is back."), errors in red; 5 s / 15 s | `close_done`, `sent_note` | 🔧 native used a toast with a count | `close_done_posts_and_notes`, `sent_note_matches_web_wording` |
| `taskCard` | ⚑ G · T ref, ago (tooltip `whenLine`), title, High chip, chips: project, Jira key, "PR #n · checks", Failed, Terminal lost; who | `task_card_vm`, `task_card` | 🔧 removed native-only extras: question/lost/start-failed box, latest line, waiting/blocked/starting line, summary, running time, Start button, PR stage label; High is warn-colored | `board_matches_web` |
| `taskCard` | Selected (open task) outlined in accent; needs = warn border | `task_card` | ✅ | `board_matches_web` (selected) |
| `checksOf` | pass Passed, fail Failed, pending Running, none No checks, else Unknown, with their colors | `checks_of` | 🔧 color classes now the web's (`b-*`); merged PR no longer recolored | `board_matches_web` |
| `startsByHand` + drag handlers | Queued/planned, not in a goal, not starting: draggable (tooltip "Drag to Working to start it"); Working highlights while dragging and on hover; drop → POST `/tasks/T/start {mode:"new"}`, toast "Starting T" | `starts_by_hand`, `DragTask`, `columns` (`drag_over`/`on_drop`), `start_new` | 🔧 real drag and drop replaces the native Start button (the web had no button) | `board_matches_web` (drag flags), `dropping_on_working_starts_a_new_terminal` |
| `issueCard` | Kind chip (Bug/Test gap/Follow-up/Clean-up, else capitalised, empty → Issue), state chip (Now a task / Jira key or "Ticket asked for" / Won’t do), ago (tooltip `issueFrom`), title, goal line "G3" (tooltip goal name) or "Not in a goal" | `issue_card_vm`, `issue_card` | 🔧 goal line showed "G3 name"; issueFrom missed "Added by you" when nothing names the reporter; colors now the web's | `board_matches_web` |
| `issueFrom` | Times via `hhmm`: "2:20 PM" today, "Oct 6" otherwise | `fmt::hhmm` | 🔧 needs the shared `fmt.rs` change (see report) | `board_matches_web` |
| `keepIssue` / `?keep` | Issues changed on the board stay in the column (board page only); cleared by goal/project filter changes | `keep_issue`, `goal_pick` | ✅ (`keep_issue` for the issue panel to call) | `keep_issue_only_on_the_board` |
| `loadState` | `/state?project&goal=<number>&done&keep` (goal ref → id) | `state_query` | 🔧 written; `app.rs` must call it (see report) | `state_query_matches_web` |
| `filter-project` / `filter-goal` CHANGES | Handlers exist but no control renders them | — | ↔ dead in the web; not ported | — |
