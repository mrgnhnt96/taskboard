# Parity: forms, pickers, drafts and the modal stack

Reference: `crates/taskboard-app/parity/web/` (`pages.js` forms, `app.js` pickers / drafts / modals).
Native: `crates/taskboard-app/src/ui/modals.rs`. Tests: `src/ui/modals/tests.rs`.

`forms_match_web` checks 114 cases in `parity/golden/forms.json`, all produced by the web code itself
(`parity/gen/forms.mjs`; submit bodies are captured at `fetch`, so they're the exact wire JSON). The
other tests drive the real forms in a headless window on a recording backend.

Status: ✅ same · 🔧 fixed in this pass · ↔ intentional native difference.

| Web function | Behaviour | Native | Status | Test |
|---|---|---|---|---|
| `openTaskForm` | project = goal's project, else given project, else filter project, else first of `projectNames` | `open_task_vals` | 🔧 (`projectNames` now includes goal projects, sorted with `localeCompare`) | `forms_match_web` (openTask ×10) |
| `openTaskForm` | goal = given goal (kept even if unknown), else the board's goal filter | `open_task_vals` | ✅ | `forms_match_web` |
| `openTaskForm` | `auto_close` from the goal (`!== false && !== 0`), else on; planned as given | `open_task_vals` | ✅ | `forms_match_web`, `add_task_to_a_goal_plans_it` |
| `openTaskForm` + `restoreDraft` | a saved task draft (with words) is restored unless a goal was given; draft note shows | `open_task_vals`, `form_shell` | 🔧 (drafts weren't ported) | `forms_match_web`, `a_task_draft_comes_back_and_can_be_discarded` |
| — | "New task here" on Sessions presets "In an idle terminal" + that terminal (after any draft) | `open_task_vals` | ↔ native addition (no web caller) | — |
| `openGoalForm(g)` | edit: fields from the goal, `epicMode: keep`, `max_terminals` `Number()||2`, run_in_order / auto_close `!== false && !== 0` | `open_goal_vals` | 🔧 (0 values now read as off, like the web) | `forms_match_web` (openGoal ×7) |
| `openGoalForm(null)` | new: default project, epic none, in order, 2 terminals, auto close; restores a goal draft | `open_goal_vals` | 🔧 (drafts) | `forms_match_web` |
| `openNewIssue` | goal = given, else the Backlog page's goal filter (not all/none); project = goal's, else Backlog project filter, else board filter, else first project | `open_issue_vals` | ✅ | `forms_match_web` (openIssue ×7) |
| `taskFormHtml` | labels, help texts, planned checkbox only with a goal, planned help replaces Start | `task_view` → `render_task` | ✅ | `forms_match_web` (taskView ×12) |
| `taskFormHtml` | Start choices; "In an idle terminal" disabled (tooltip "No idle terminal in this project") without idle terminals, and shown as "When the repo’s free" | `task_view` | ✅ | `forms_match_web` |
| `taskFormHtml` | terminal choice: "Pick a terminal" then idle terminals by name (no branch) | `task_view` | 🔧 (the native list showed the branch) | `forms_match_web` |
| `taskFormHtml` | Jira ticket (only with Jira): Create one / Link existing / None; key field "PROJ-123" for link | `task_view` | ✅ | `forms_match_web` |
| `taskFormHtml` | submit "Add task" / "Adding…"; error in the footer | `task_view` | ✅ | `forms_match_web` |
| `goalFormHtml` | title New goal / Edit goal; TLDR and Done-when help; project picker | `goal_view` → `render_goal` | ✅ | `forms_match_web` (goalView ×9) |
| `goalFormHtml` | Jira epic: edit = key field "PROJ-123, or leave empty" + help; new = Create an epic / Link existing / None, key field for link, help per choice | `goal_view` | ✅ | `forms_match_web` |
| `goalFormHtml` | "At most N terminals" (select 1–5), two checkboxes | `render_goal` stepper | ↔ stepper instead of a select (same 1–5 range); a goal already at 8 shows "8 terminals" where the web's select showed "1 terminal" but still sent 8 | `forms_match_web` |
| `goalFormHtml` | submit "Add goal" / "Save goal" / "Saving…" | `goal_view` | ✅ | `forms_match_web` |
| `goalFormHtml` | `prefill` paragraph | — | ↔ not reachable: nothing on the web passed `o.prefill` | — |
| `newIssueHtml` | Title, Type (Bug / Test gap / Follow-up / Clean-up), Goal, Project, What you saw, More detail (optional) + help; "Add issue" / "Adding…" | `issue_view` → `render_issue` | ✅ | `forms_match_web` (issueView ×3) |
| `seg` / `m-set` | a segmented choice sets the field and clears the form's error | `seg_choices`, pickup rows | 🔧 (errors weren't cleared) | `a_segment_click_clears_the_error` |
| `setModalField` | checkboxes toggle without clearing the error | `check` handlers | ✅ | — |
| `submitTask` | validation order and messages; ticket key `^[A-Z][A-Z0-9_]*-\d+$` upper-cased | `task_request` | ✅ | `forms_match_web` (submitTask ×19) |
| `submitTask` | body: trimmed title/detail, priority, goal_id (number), auto_close, pickup (planned → queue + `status: planned`; attach → session_id), `jira` only with Jira | `task_request` | ✅ | `forms_match_web`, `new_task_posts_the_web_body_and_opens_the_task` |
| `submitTask` | a terminal picked before the project changed was still sent | `task_request` | ↔ fixed web bug: it must be one of the project's idle terminals ("Pick the terminal to hand it to.") | `a_terminal_from_another_project_is_not_sent` |
| `submitTask` | success: clear draft, close, toast "Added T7" / "Task added", open the task; failure: error in the form | `submit_task` | 🔧 (draft clearing) | `new_task_posts_the_web_body_and_opens_the_task`, `adding_a_task_clears_its_draft`, `a_task_without_a_title_says_so_and_sends_nothing` |
| `submitGoal` | validation; body; edit → `POST goals/G1` (+ `epic_key` or null with Jira); new → `POST goals` + `epic` | `goal_request` | ✅ | `forms_match_web` (submitGoal ×14), `edit_goal_posts_to_the_goal` |
| `submitGoal` | success: edit → close, "Goal saved"; new → clear draft, close, "Added G4", go to the goal | `submit_goal` | 🔧 (draft clearing) | `new_goal_posts_and_goes_to_its_page` |
| `submitNewIssue` | validation; body with said/detail only when not blank | `issue_request` | ✅ | `forms_match_web` (submitIssue ×6) |
| `submitNewIssue` | success: close, "Added B3"; open it only when not on the board | `submit_issue` | 🔧 (it always opened) | `an_issue_added_on_the_board_stays_on_the_board`, `an_issue_added_from_a_goal_opens` |
| `pickerButton` | label: special's label, goal name, else "Choose a goal" / "Choose a project" (archived goals count as unknown) | `picker_label` | 🔧 (unknown goal said "Not in a goal"; empty project said "Pick a project") | `forms_match_web` (pickerButton ×6) |
| `pickerBase` (project) | `projectNames` + the current value if unknown; path line with `/Users/x` → `~` | `picker_items` | 🔧 (goal projects were missing) | `forms_match_web` (picker ×13) |
| `pickerBase` (goal) | every unarchived goal (all projects in forms), sorted by name; line "project · epic" | `picker_items` | 🔧 (was project-first with the ref) | `forms_match_web` |
| `pickerItems` | search: projects by name only, goals by name or line; rank starts-with, word-start, contains; specials hidden while searching | `picker_items` | 🔧 (no ranking; specials stayed; projects matched paths) | `forms_match_web` |
| `renderPickerList` | "Current" on the chosen row; empty text `No project matches “q”.` | `picker_list`, `picker_empty` | 🔧 | `forms_match_web` |
| `openPicker` | search placeholder per kind; the current row is highlighted | `open_picker`, `picker_start` | 🔧 | `forms_match_web`, `picker_keys_wrap_and_escape_closes_only_the_picker` |
| `pickerKey` | ↑/↓ wrap, ↩ picks the highlighted row, esc closes the picker, ⇥ does nothing; typing moves the highlight to the top | `picker_key`, `picker_list` | 🔧 (↩ took the first row, no ↑/↓) | `picker_keys_wrap_and_escape_closes_only_the_picker`, `picking_a_goal_changes_only_the_goal` |
| `pickItem` / `applyPick` | picking sets only that field (no project/goal/terminal/auto-close cascade); the same value does nothing | `pick_item` | 🔧 (native cascaded) | `picking_a_goal_changes_only_the_goal` |
| `openPicker` UI | a separate search dialog | inline list under the button | ↔ layout only; same items, keys and footer hints | — |
| `saveDraft` | on every change to a new task/goal not being sent: keep `DRAFT_FIELDS` when it has words, else remove; edits and issues never | `draft_write`, `save_draft` (prefs `tb.draft.task` / `tb.draft.goal`) | 🔧 | `forms_match_web` (saveDraft ×6) |
| `saveDraft` | `max_terminals` was saved as the select's text ("3") | `goal_draft` | ↔ saved as a number (read back either way) | `forms_match_web` (normalized) |
| `draft-discard` | forget the draft, close, open a fresh form of the same kind | `discard_draft` | 🔧 | `a_task_draft_comes_back_and_can_be_discarded` |
| `renderModal` / `closeModal` | Close / Cancel close the form (no `back` modal is ever set by a caller) | `close` | ✅ | — |
| `modal-bg` | clicking the backdrop does nothing | `render` (scrim) | 🔧 (it closed the form) | — (no handler) |
| keydown Escape | esc closes the picker, then only read-only dialogs (alerts); forms stay open | `on_escape` (scrim `CloseOverlay`) | 🔧 (esc closed forms) | `escape_keys_reach_the_form_not_the_window` |
| form submit | ↩ in a one-line field submits; not while sending | `on_form_key`, `submit_*` | ✅ | — |
| — | ⌘↩ submits from any field; ⇥/⇧⇥ cycle the text fields | `on_form_key` | ↔ native additions (the web's ⇥ also visited buttons) | — |
| `alertsDialogHtml` | title "N tasks need your attention", each alert with Open / Dismiss, "Dismiss all" | `alerts` (+ `app::alert_row`) | ✅ | — |
| `alerts-dismiss-all` | dismisses every alert; the dialog closes by itself when none are left (`renderBanner`) | `alerts`, `render` | 🔧 (it closed at once, and never closed by itself) | `the_alerts_dialog_closes_itself_when_none_are_left` |
| `localeCompare` | ordering of projects and goals | `locale_cmp` | 🔧 (was byte order) | `forms_match_web` (localeSort ×2) |
