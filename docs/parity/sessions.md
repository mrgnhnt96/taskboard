# Parity: the Sessions page

Reference: `crates/taskboard-app/parity/web/sessions.js` (all of it) and the rename helpers in `app.js`
(`startRename`, `endRename`, `sessionCopies`, `renameFlash`, `nameView`) as the Sessions page uses them.
Native: `crates/taskboard-app/src/ui/sessions.rs`.

Tests: the differential golden `parity/gen/sessions.mjs` → `parity/golden/sessions.json` (233 cases, the web's
own functions on fixtures covering every state) is checked by `sessions_match_web`. The action tests drive a
headless window on the recording backend and assert the exact requests.

Status: ✅ same · 🔧 fixed in this pass · ↔ intentional native difference.

| Web function | Behaviour | Native | Status | Test |
|---|---|---|---|---|
| `SS_FILTERS`, `ssHeader` | Filter seg All / Needs you / Working / Idle / Idle over 1 hour, each with its count; "N Claude terminals in Midna" once loaded | `header_vm`, `header` | ✅ | `sessions_match_web` (ssHeader *) |
| `ssHeader` back link | "Back to T12" when the reader came from a task's terminal link | `header_vm`, `open_from_task`, `left` | 🔧 (was missing) | `back_to_the_task`, ssHeader back to task |
| `ssHeader` back link | "Task board" when there's no task to go back to | — | ↔ the sidebar is the way back to the board | ssHeader loaded (mapped explicitly in the test) |
| `forFilter`, `isStale` | Filters compare the raw `status \|\| 'idle'`; "Idle over 1 hour" = raw idle and idle > 1 h | `for_filter`, `is_stale` | 🔧 (an unknown status counted as idle) | forFilter * |
| `idleMs` | Since `last_activity`, else `seen_at`, 0 when neither parses; not clamped | `idle_ms` | 🔧 (was clamped at 0) | ssRow *, longAgo * |
| `longAgo` | under a minute / N min / N h M min (under 6 h) / N h / N days | `long_ago` | ✅ | longAgo * |
| `ssMatch` | Trimmed, lower-cased query in name, project, task title, task ref, branch | `matches` | ✅ | ssMatch * |
| `groupedSessions` | By project ("No project"); rows by `SESS_ORDER` then idle time; groups by best rank, then `localeCompare` | `grouped`, `locale_cmp` | 🔧 (names compared case-sensitively) | groupedSessions order, locale order |
| `afterSessionsLoad` | Nothing selected: first terminal of the first open group; from a collapsed group it stays collapsed (`hideSel`) | `auto_select` | 🔧 (`hideSel` missing) | `auto_select_skips_collapsed_groups` |
| `ssList` | "Loading…" until the list loads | `list_vm` | ✅ | ssList loading |
| `ssList` | List error text (`SS.listErr`) instead of Loading… | — | 🔧 `ListCtx.list_err` ← `data.errs.sessions` | `sessions_match_web` (list failed), `parity::failed_fetches_keep_the_page_and_say_why` |
| `ssList` | Empty: "No terminal matches “q”." (as typed, trimmed) / "No Claude terminals in Midna right now." / "None right now." | `list_vm` | 🔧 (query shown lower-cased) | ssList empty, empty filtered, search none |
| `ssList`, `groupHead` | Group head "project N terminals", " · N need(s) you" when collapsed; open if not collapsed, searching, or holding the selection (not a hidden one) | `list_vm`, `group_head` | 🔧 (`hideSel`) | ssList collapsed * |
| `ssList` | Closed group (All filter only): matching closed rows, count = total (or matches while searching), open when toggled or holding the selection, first 50 rows | `list_vm` | ✅ | ssList default, closed open, search closed, closed holds/hidden selection |
| `ssList` | "Select N idle" beside the search (not picking, N > 1, All filter), with its tooltip | `list_vm` | ✅ | ssList default, one stale, stale not all filter |
| `ssList` | No plain "Select" button (picking starts from "Select N idle" or a row's menu) | `list` | 🔧 (the app added a "Select" button) | ssList * acts |
| `ssList` bulk bar | "N selected" (also 0), "Add the N idle over 1 h", Done, "Close N terminals" — in that order | `list_vm`, `bulk_bar` | 🔧 (said "Pick idle terminals…" at 0; order differed) | ssList selecting * |
| `ssRow` | Name, "T3 · title" or "No task · branch", state (Closing… / Needs you / Working / Idle / Gone), when (idle: "for …" or "just now"; else "5m ago") | `row_vm`, `row` | ✅ | ssRow * |
| `ssRow` | Checkbox while picking for terminals that can be closed together, a spacer for others | `row_vm` (`Pick`) | ✅ | ssRow * selecting/picked |
| `ssRow` / `ss-open` | Click opens it (dropping any Close confirmation); while picking, a pickable row toggles instead | `row_click`, `open` | 🔧 (confirmation only dropped when switching) | `bulk_close_sends…`, `filter_and_open_drop…` |
| `ssRow` | Row names aren't renamable (no `data-rename-id`) | `row` | 🔧 (the app renamed on a row double-click, inline) | — |
| `ssClosedRow` | Name, "project · T1 · title" / "No task", "Closed", closed ago | `closed_row_vm` | ✅ | ssClosedRow * |
| `ssMenuItems`, `ssMenu` | Closed: "Reopen its conversation" if it can; else Open, Show in Midna, Rename…, Select/Deselect (pickable), Close terminal… / Force close… (danger) | `menu_items`, `row_menu` | ✅ | ssMenuItems * |
| contextmenu | No menu when it would be empty | `row` | 🔧 | — |
| `ss-menu` open/focus/rename/select/close/reopen | Open drops confirmation; Show in Midna → toast "Showing X in Midna"; Rename… opens and renames; Select starts picking; Close… opens and asks; Reopen → toast | `menu_action` | ✅ | `the_menu_shows_in_midna_with_a_toast`, `close_and_force_close_send_force`, `reopen_from_the_detail_and_the_menu` |
| menu: close on resize / scroll | — | — | ↔ native popover closes on click outside or Esc | — |
| `ssDetail` | "Pick a terminal to see what it’s done." / "Loading…" | `detail_vm` | ✅ | ssDetail nothing selected, loading |
| `ssDetail` | Detail error text (`SS.detailErr`) | — | 🔧 `DetailView::Error` ← `data.errs.session` | `sessions_match_web` (detail failed), `parity::failed_fetches_keep_the_page_and_say_why` |
| `ssDetail` pill + `statusLine` | Closing / Closed / Needs you / Working / Idle; "for 12 min" (working: status_at or prompt; needs: status_at or waiting; else idle time); closed: "2h ago" | `detail_vm`, `status_line` | ✅ | statusLine *, ssDetail * |
| `nameView` (detail) | Hover title "name · double-click to rename"; "Last rename failed: X · double-click…"; renaming "Renaming in Midna…"; text falls back to the id | `name_vm` | 🔧 (failed rename was drawn red; titles differed) | nameView * |
| `renameFlash` | After a rename: ✓ green 1.2 s when the name took, ✗ red with the asked name 1.8 s ("Rename failed: why" / "Midna kept the old name") | `observe_rename`, `tick_renames` | 🔧 (missing) | nameView flash *, `a_failed_rename_flashes_then_clears` |
| `startRename` / `endRename` | Double-click the live title (or menu Rename…); field starts from `renaming \|\| name`; ↩ or leaving the field saves, Esc drops; nothing sent if empty or same as `renaming \|\| name`; max 80 | `start_rename`, `end_rename`, `rename_field` | 🔧 (no save on blur; compared with name only; extra Save/Cancel buttons) | `rename_sends_only_a_new_name` |
| `sessionCopies` | The new name shows as renaming right away; on failure the error shows until the next answer | `send_rename`, `effective`, `retire_pending` | 🔧 (missing) | `rename_sends_only_a_new_name` |
| path | `project_path` with `/Users/<anyone>` → `~`, else project | `short_path` | 🔧 (only `$HOME` was shortened) | ssDetail idle no task last task dirty |
| meta | Branch as plain mono text | `detail` | 🔧 (had a ⎇ glyph) | ssDetail * |
| `diffView` | "N files +A −R" with "Not committed yet · N new"; "No changes"; dirty count when no diff; hidden when closed | `diff_vm` | ✅ | diffView *, ssDetail * |
| actions | Live: Show in Midna + Close terminal (close) or hold-to-Force close (force); none while closing or confirming | `detail_vm` | ✅ | ssDetail * acts |
| actions | No "New task here" | `detail` | 🔧 (the app added it; the New task form's "In an idle terminal" covers it) | ssDetail * acts |
| closed actions | Reopen when the closed row can reopen; note "can’t reopen" / "Reopening starts…" (also when it isn't in the closed list) | `detail_vm` | 🔧 (the note was missing without a closed row) | ssDetail gone * |
| `ss-focus` | POST `sessions/:id/focus` `{}`; "Sending…" while busy; then "Sent to Midna" / "Saved. It runs once Midna is back." under the buttons (errors there too, 15 s) | `focus`, `run`, `sent_note` | 🔧 (was a toast) | `show_in_midna_says_so_under_the_buttons`, sentNote * |
| `ss-reopen` | POST `sessions/:id/reopen` `{}`; note "Asked Midna to reopen it in a new terminal." | `reopen` | 🔧 (was a toast) | `reopen_from_the_detail_and_the_menu` |
| `closeCopy` + `ss-ask`/`ss-close`/`ss-cancel` | "Close X?" / "Force close X?" with the turn / task / conversation sentences; Close terminal / Force close and "Keep it open"; POST `sessions/:id/close` `{force}`; no toast (the closing box says it) | `close_copy`, `close` | 🔧 (sent a toast) | closeCopy *, `close_and_force_close_send_force` |
| closing box | "Asked Midna to close X. It moves to Closed once it’s gone." | `detail_vm` | ✅ | ssDetail closing |
| `holdBtn` | Hold 1.2 s → POST `{force: true}`; "Keep holding…" while held, "Sending…" when sent; letting go or leaving the button stops | `hold_start`, `hold_end`, `hold_fire` | 🔧 (leaving the button didn't stop; no busy label) | ssDetail holding / hold busy, `close_and_force_close_send_force` |
| hold by keyboard (Space/Enter) | — | — | ↔ app buttons don't take keyboard focus | — |
| `ssTaskBox` | Task row with `STATUS[stKey]` pill (Failed / Blocked / Planned / Working / Needs you / Done, anything else Queued) + goal row; or "Task No task" + last task; closed: last task only | `detail_vm`, `task_pill`, `links` | 🔧 (showed "Starting"; unknown status had no pill) | ssDetail task pill * |
| stats | Turns, Commits, Files edited, Last activity ("—") | `detail_vm` | ✅ | ssDetail * |
| prompt / reply boxes | "Last prompt · 3:05 PM" + text (500) / "Nothing yet."; "Waiting since …" (warn) or "Latest/Last reply · …" + markdown / "Last reply" "Nothing yet." | `detail_vm`, `boxes` | ✅ (times need the `fmt::hhmm` fix) | ssDetail * |
| timeline | "3:05 PM" (full time on hover), dot, text (260) + " · N files edited", kind label (unknown: none) | `detail_vm`, `timeline_kind` | ✅ | ssDetail working full |
| timeline `found` dot | `--seen` colour | `timeline_kind` | ↔ no `seen` token in the theme; uses warn | — |
| `ssBulkConfirm` | Modal "Close N idle terminals?", sentence, every pick "name · project · idle for …" in pick order, note, "Keep them open" / "Close N terminals"; backdrop click cancels | `bulk_vm`, `bulk_dialog` | 🔧 (was inline, capped at 8, unordered) | ssBulkConfirm * |
| `ss-bulk-go` | POST `sessions/close` `{ids}` in pick order; then picks cleared, picking stays on, toast "Asked Midna to close N terminals[; K got busy…]. Confirm on the Mac."; errors in the dialog | `bulk_close` | 🔧 (ids unordered; left picking mode; errors were toasts) | `bulk_close_sends_picks_in_order_and_stays_picking` |
| `ss-stale` | Picks every idle-over-an-hour terminal that can be closed | `pick_stale` | ✅ | — |
| `ss-done` | Leaves picking, clears picks | `done_picking` | ✅ | — |
| `loadSessionList` | Drops picks that are gone or no longer closable | `render` | ✅ | — |
| `ss-group` + `tb.sessCollapsed` | Collapse / open a project (as drawn); collapsing hides the selection from holding it open; saved under `tb.sessCollapsed` | `toggle_group`, `load_collapsed` | 🔧 (not saved; no `hideSel`) | `collapsed_groups_are_saved` |
| `ss-closed` | Toggle Closed; closing it hides the selection | `toggle_closed` | 🔧 | ssList closed * |
| `ss-filter` | Changes filter, drops confirmation | `set_filter` | ✅ | `filter_and_open_drop_a_close_confirmation` |
| Esc (keydown) | Menu first; then bulk dialog / confirmation; then leaves picking (not while typing) | `escape` (+ app.rs hook) | 🔧 (missing) | `escape_closes_confirmations_then_picking` |
| route `s=` / `f=` | Selection and filter live in the URL | `State.selected`, `State.filter` | ↔ kept in the window's state (no URL); the selection survives leaving the page | — |
