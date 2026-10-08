# Parity: the goal page

The web board's goal page (`pages.js`: `goalMain` and everything it calls, minus the goal list `goalNavList`,
which the chrome audit covers) against `crates/taskboard-app/src/ui/goal.rs`.

Tests: `parity/gen/goal.mjs` runs the web's own functions on fixtures and writes 198 cases to
`parity/golden/goal.json`; `ui::goal::tests` checks the app against them:
- `views_match_web` covers every view (167 cases).
- `times_match_web` covers times of day.
- `toasts_match_web` covers the action messages.
- `run_and_settings_send_what_the_web_sent` and `notes_attachments_and_issues_send_what_the_web_sent` drive each action
  in a headless window on the recording backend. They compare the request with what the web's own handler sent
  (the gen stubs `run`/`api` and calls `ACTIONS[...]`), and check where the result shows.
- `kept_rows_stay_where_they_were` and `shift_click_picks_a_range` cover the list bookkeeping.

Status: ✅ same · 🔧 fixed in this pass · ↔ intentional native difference.

| Web function | Behaviour | Native | Status | Test |
|---|---|---|---|---|
| `resetPageState` | Kept rows, order, show-dropped and selection reset when the goal changes | `State::reset_for` | ✅ | `views_match_web` (backlog) |
| `resetPageState` | …also reset when you leave the goal page and come back to the same goal | none | 🔧 app.rs `go()` calls `goal_page.reset_for("")` when leaving | `parity::another_goal_starts_empty` |
| `renderGoalPage` | "Loading…" until the goal arrives | `render` | ✅ | — |
| `renderGoalPage` | The load error (`P.goalErr`) shown in place of the goal | `render` (`m.data.errs.goal`) | 🔧 was "Loading…" forever | — |
| `goalState` | Done / Waiting on you (stopped PR) / N PRs awaiting merge / Deprioritized / Paused / Needs a restart / Waiting on you / In progress / Blocked / Queued / Not started / No tasks yet | `goal_state` | 🔧 a PR with no `state` counts as open (`prOpen`) | `views_match_web` goalState × 20 |
| `goalCounts` | done / active / queued, from the tasks | `counts` | ✅ | header cases |
| `goalMain` header | Pills: ⚑ Goal, state (colored like `.st-*`), ref; Edit opens the goal form | `header`, `header_view` | 🔧 state chip colors follow `.st-*` (Deprioritized and No tasks were muted gray, Queued was neutral) ↔ no Edit button: goals are edited through AI sessions | header × 40 |
| `goalMain` header | Name, **TLDR** line only | `header_view` | 🔧 removed the Outcome line the web never showed | header |
| `goalMain` header | Meta: project chip · "Jira epic KEY · status" (link to `epic_url`) or "No Jira epic" with Jira on · "d of n done · a active · PRs · k open in the backlog" | `header_view` | ✅ ↔ hovering the project chip shows the goal's `repo_path` | header (jira on/off) |
| `goalMain` | Progress bar: done / active / queued | `header` | ✅ | — |
| `goalMain` | Tabs "Tasks n" / "Backlog k" (count marked when k > 0); switching closes the open issue | `render` | 🔧 switching tabs now closes the issue panel (`goal-view` clears `issue`) | header (tabs) |
| `goalMain` | Tool: "Add a task" (task form, this goal, planned) / "Add an issue" (issue form, this goal) | `render` | ✅ | header (tool), backlogTool |
| `goalMain` / `issueAside(false)` | Right column with Tasks: Attached + Goal notes. With Backlog: the picked issue (no move picker), "Loading…" / its error, nothing when none is picked | `render`, `issue_aside` (via `issue_panel::aside_view` / `render_aside`) | 🔧 was the issue drawer | — |
| `goalRunButtons` | Start / Start N more tasks / Resume, with the web's titles; Pause; Plan in Claude; Deprioritize (not when deprioritized or all done) | `run_buttons_view` | ✅ | runButtons × 40 (hours open / off / closed today / tomorrow / weekday / unknown) |
| `startLater` / `opensAt` | Outside hours: split "Start tomorrow[: N more tasks]" + ▾ menu (Start tomorrow · Queued now, starts when work hours open at 6am / Start now · Runs outside work hours until they open); day = today / tomorrow / weekday name / later | `run_buttons_view`, `start_menu_view`, `opens_at` | 🔧 `opens_at` used the machine's clock and zone directly; now `fmt::now`/`fmt::local` | startMenu × 4, opensAt × 4 |
| `goal-run` / `goal-run-now` | `POST goals/:g/run` `{}` / `{now: true}`; toasts "Started: N tasks queued" / "Going again" / "N tasks queued. They start tomorrow at 6am." / "Started now: N tasks queued. The goal runs until work hours open." | `run_goal`, `run_toast`, `run_now_toast` | ✅ | action run*, `toasts_match_web` |
| `btn` busy | A running request disables its button and shows its busy label (Starting… / Queuing… / Opening… / Deprioritizing… / Sending…); a second press is ignored | `run` (busy keys `act:arg:id`), `act_btn` | 🔧 new | — |
| `goal-plan-edit` | `POST goals/:g/plan {mode: edit}`; toast | `plan_goal` | ✅ | action plan |
| `goal-pause` | Pause: `POST goals/:g {paused: true}`, toast. Resume from the gate: `{paused: false}`, note "Going again" in the gate | `set_paused` | 🔧 the gate's result is an inline note, not a toast | action pause, resume (gate) |
| `goal-deprio-ask` / `gdeprioHtml` | Dialog "Deprioritize this goal?": "**Name** goes off the board’s goal list and nothing new starts. N tasks already running carry on. N queued tasks won’t start." + "You can bring it back from the Deprioritized list under Goals."; Cancel / Deprioritize | `deprio_dialog_view`, `dialog` | 🔧 was an inline box with different words; now a dialog with the web's text | deprioDialog × 5 |
| modal keys | Backdrop clicks do nothing (`modal-bg`); Esc closes only read-only dialogs (the note dialog), not Deprioritize | `dialog` | 🔧 the backdrop used to close dialogs | — |
| `goal-deprio-yes` / `goal-deprio` off | `POST goals/:g {deprioritized: true}` (toast, closes dialog) / `{deprioritized: false}` (gate note "Back on the board") | `set_deprioritized` | 🔧 gate note | action deprioritize, bring back |
| `gateBanner` | Deprioritized / Paused box with Bring it back / Resume the goal; shown above the task list only; else just the gate's note | `gate_view`, `gate` | 🔧 was also shown in the Backlog view | gate × 4 |
| `goalTasks` rows | Number, `STATUS` chip, Jira mark (tooltip "KEY · status"), PR mark "#n" (tooltip "PR #n · stage · checks …"), title, meta | `task_row_view`, `task_rows` | 🔧 removed the ref the web didn't show; the stage label moved into the tooltip; added the Jira mark; the open task's row is highlighted | taskRows × 6 |
| `goalTaskMeta` | who · finished/stopped ago · key; Terminal lost · who; who · running · asked/updated ago · key; High · starting / waiting / after task N / starts when a terminal is free; planned: after task N | `task_meta` | 🔧 the web's invisible trailing space ("finished " with no time) is trimmed; `run_in_order` read as JS truthiness | taskRows rich (both orders) |
| `prMark` | Colored by stage phase | `task_rows` | ↔ glyphs: color only, no stage icon | — |
| "How this goal runs" | "At most [n terminals ▾] working on this goal at once" (1, 2, 3, 4, 5, 6, 8; `Number(max) || 2`); run in order / auto close checkboxes (on unless 0/false); saved at once, note "Saved" | `how_runs_view`, `how_runs`, `set_run_option` | 🔧 a dropdown with the web's options (was a stepper); note instead of toast; one save at a time (shared busy key) | taskRows (max_*), action set * |
| `notesAside` | Fold "Goal notes" (remembered as `tb.fold.gnotes`); help text; Add a note; groups Findings / Decisions / References (unknown kinds are findings); pinned first, then by time (as strings); "Nothing yet / Notes from tasks and from you collect here." | `notes`, `notes_view` | 🔧 fold + prefs key; empty state wording; sort key for a missing `at` | notes × 3, noteForm |
| `noteItem` | Plain text (not markdown), whitespace collapsed; "Pinned" mark; byline inline after short notes; long notes (> 220 UTF-16 units or > 4 lines) clamped to 3 lines with "Show all" | `notes` | 🔧 was markdown with inline expand/collapse | notes mixed |
| `noteFrom` | "you, 2:05 PM" / source / time | `note_from` | ✅ (after the chrome audit's `fmt::hhmm` fix) | `times_match_web` |
| `noteDialogHtml` / `noteBody` / `inlineText` | Dialog title "[Pinned] Kind · from"; body: paragraphs (line breaks), bullets (- * •, indented lines continue a bullet), indented code, links, `code` and path-like tokens | `note_dialog_title`, `note_blocks`, `inline_segments`, `note_body` | 🔧 new (a port of the web's `INLINE` regex) | noteBody × 8, noteDialog × 4 |
| `gnote-save` | Empty: "Write the note first." (error note). Else `POST goals/:g/notes {kind, text (trimmed), source: you}`, note "Note added", form closes; the draft and kind survive Cancel and goal switches | `save_note` | 🔧 inline note; drafts kept per goal | action note* |
| `attachAside` / `attachList` | "Attached"; each: title (link when http(s) or a `~`/`/` path, else mono text), meta "Kind · T4/G2 · ago" (other → Link, own goal hidden), tooltip title / title + path | `attachments_view`, `attachments` | 🔧 kind "Other" → "Link"; dropped `added_by` (not shown on web); source is a link | attachments × 2 |
| `attMenuHtml` | ••• menu: Open (when linkable), Copy link / Copy path, Edit, Remove | `att_menu` | 🔧 was Copy + Remove buttons | attMenu × 4 |
| `attEditHtml` / `att-esave` | Kind seg, title, link or path; Save sends only the touched fields to `POST attachments/:id`; Cancel | `start_att_edit`, `save_attachment` | 🔧 new | attEdit, action edit attachment × 3 |
| `att-remove` / `att-copy` | `POST attachments/:id/remove`, note "Removed" in the aside; "Copied" toast | `remove_attachment`, `att_menu` | 🔧 note, not toast | action remove attachment |
| `goalBacklog` | Help text; rows: checkbox (open only), kind chip, title, `issueFrom(b, long)`; open rows offer Make it a task / Create ticket (Jira) / Won’t do, others show `goalIssueState` ("Now task 2 · working", "Jira PROJ-7", …); the picked issue is highlighted | `backlog_view`, `backlog` | 🔧 "Make a task" → "Make it a task"; added Create ticket; dropped the ref; won't-do rows hidden unless kept or shown | backlog × 9 |
| `goalBacklog` | "N closed as won’t do · Show/Hide" or "Nothing in the backlog yet. …" | `backlog_view` | 🔧 Show/Hide toggle; `closed_count` falls back to counting | backlog (closed lines) |
| `keepBacklogRow` / `withKept` / `refreshKept` | An issue you change stays in its place, refreshed from `GET backlog/:id` | `keep_issue`, `refresh_kept`, `backlog_rows` | 🔧 new | `kept_rows_stay_where_they_were` |
| `promote` / `ticket` / `drop` (goal rows) | `POST backlog/:b/promote {where: goal}` (no message), `/ticket` ("Asked Jira for a ticket"), `/drop` ("Closed as won’t do"): notes on the row | `issue_action` | 🔧 the promote toast was removed; notes on the row | action promote / ticket / drop |
| `bulkBar` | "Select all" / "N selected"; Make tasks, Create tickets (Jira), Won’t do, Move to a goal (picker), Clear; disabled while any bulk request runs; errors in the bar | `backlog_view`, `backlog` | 🔧 busy state; error note | backlog (bars) |
| `bl-pick` / `bl-pick-all` | Click toggles; shift-click picks a range from the last one; select all / none; the selection keeps the order picked | `pick` | 🔧 shift ranges and pick order are new | `shift_click_picks_a_range` |
| `bulkChange` | `POST backlog/bulk {ids, action, where: goal (tasks), goal_id (move)}`; toast `BULK_DONE` (count, else the number of ids); selection cleared | `bulk`, `move_selected`, `bulk_toast` | 🔧 count fallback | action bulk * |
| goal picker (`pickerItems`, kind goal) | "Choose a goal"; search "Search goals by name, project or epic"; "Not in a goal" first, then every goal (the current one included) by name, sub "project · epic"; ranked search; ↑↓ ↩, Esc; "No goal matches “q”." | `goal_picker_items`, `picker` | 🔧 was a plain menu that left out the current goal | goalPicker × 6 |
| Collation | `localeCompare` | `locale_cmp` | 🔧 uses the node-verified `modals::locale_cmp` | `forms_match_web` (localeCompare), goal goldens |

Not ported: the ↔ rows above.
