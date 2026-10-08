# Parity: the Backlog page and backlog issues

Web reference: `crates/taskboard-app/parity/web/` (pages.js `renderBacklogPage`, `backlogRows`, `blStateText`,
`withKept`, `keepBacklogRow`, `refreshKept`, `loadBacklog`, `issueAside`, `blFiltersChanged`, `openNewIssue`; app.js
`issueState`, `foundTask`, `issueFrom`, `snapRows`, `issueHow`, `issueDetail`, `issueActions`, `issuePanel`,
`currentIssueRef`, `keepIssue`, the `promote` / `ticket` / `drop` / `pick-issue` /
`add-issue` / `bl-kind` handlers, `run`, `note`/`setNote`, `btn` busy, the picker).
Native: `crates/taskboard-app/src/ui/backlog.rs`, `src/ui/issue_panel.rs`.
Goldens: `parity/gen/backlog.mjs` → `parity/golden/backlog.json` (99 cases; visible text with `.sr` labels removed and
whitespace collapsed as the browser shows it, plus the `data-act`s offered).

Status: ✅ same · 🔧 fixed in this pass · ↔ intentional native difference.

| Web function | Behaviour | Native | Status | Test |
|---|---|---|---|---|
| `kindLabel` | Bug / Test gap / Follow-up / Clean-up; other kinds capitalised; empty → Issue | issue_panel.rs `kind_label` | ✅ | `issue_labels_match_web` |
| `issueState` | short + text per state (task with/without T-ref, ticket with key / "being created", won't do), none while open | `issue_state` | ✅ | `issue_labels_match_web` |
| `foundTask` | found_by_task as object or bare id → T-ref | `found_task` | ✅ | `issue_labels_match_web` (via issueFrom/issueHow) |
| `issueFrom` | Added by you / From your answer / Found by T4 · name (long) / name; then ` · ` time | `issue_from` | ✅ | `issue_labels_match_web` |
| `hhmm` (used by issueFrom, panel, history) | today "3:05 PM", else "Oct 6" (en-US) | `fmt::hhmm` | 🔧 needs the fmt.rs change below (was "3:05 pm" / "6 Oct") | `issue_labels_match_web`, `page_matches_web` |
| `fullTime` (history tooltip) | "10/7/2026, 3:05:00 PM" | `fmt::full_time` | 🔧 needs the fmt.rs change below (was "Oct 7, 3:05 pm") | — (tooltip) |
| `issueHow` | `how`, else You added it. / It came from one of your answers. / The X terminal reported it while working on T4. | `issue_how` | ✅ | `issue_labels_match_web` |
| `snapRows` | object (ordered by SNAP_ORDER, labels, Nothing / N files, objects as ref · title or JSON), array of pairs (non-pairs dropped), JSON string, plain string → Snapshot, empty values dropped | `snap_rows` | 🔧 non-pair array entries were dropped differently; first-element non-strings | `issue_labels_match_web` |
| `blStateText` | Planned task in its goal / Queued task, no goal / Won't do / state text | backlog.rs `bl_state_text` | ✅ | `state_text_matches_web` |
| `renderBacklogPage` header | "‹ Task board" back link, Backlog, "N open" pill, Goals link, Add an issue | backlog.rs `render`, `PageView::header_text` | 🔧 back link and Goals link were missing | `page_matches_web` |
| `P.openAll` | "N open" = default-filter total; with other filters a second `GET /backlog` with the defaults; falls back to `counts.open_issues`; hidden when neither | `State::open_all`, `sync`, `page_view` | 🔧 used the board's `counts.open_issues` (wrong filters) | `page_matches_web`, `row_actions_and_filters_send_what_the_web_sent` |
| Goals link (`#/goals`) | goal page: the board's goal filter, else first active goal by project, else first goal | `goals_landing` | 🔧 | — (navigation) |
| Add an issue (`openNewIssue`) | removed: issues are added through Claude (`tb backlog add`) | — | ↔ | — |
| Filters | Project picker (All projects), Goal picker (All goals / Not in a goal, goals of the chosen project), Type seg, State select (Jira ticket only with Jira or when chosen), Sort select (Newest / Oldest / Type) | `render`, `page_view` | 🔧 project/goal were plain menus, now the searchable picker | `page_matches_web` (labels, state options) |
| `loadBacklog` query | goal sent as id number (or all/none) | `State::query` (goal kept as a ref like the web) | ✅ | `query_sends_goal_ids` |
| `blFiltersChanged` | kept rows and order cleared, picked issue cleared, refetch; the old list stays until the answer | `filters_changed` | 🔧 list was blanked to "Loading…" and selection kept | `row_actions_and_filters_send_what_the_web_sent` |
| Filters saved | not saved (module state only) | `State` (not persisted) | ✅ | — |
| "Showing N of T issue(s)" | rows shown (kept included) of the answer's total, plural on total | `page_view` | 🔧 used max(total, rows) | `page_matches_web` ("kept rows" case) |
| Empty list | "No issues match these filters." (always) | `page_view`, `render` | 🔧 had a different default-filter sentence | `page_matches_web` |
| Loading / error | "Loading…" or the error | `ListView::Loading` | 🔧 `data.errs.backlog` (app.rs keeps failed fetches like the web) | `page_matches_web` (list failed), `parity::failed_fetches_keep_the_page_and_say_why` |
| Row | kind chip, title; goal name or "Not in a goal", project (mono), issueFrom(long); open → Make it a task (into goal / board) · Create ticket (Jira) · Won't do; else state chip; inline note | `RowView`, `row_el` | 🔧 had an extra "N ago", a tooltip, kept rows faded with an "Updated" pill and no actions | `page_matches_web` |
| Row selection | clicking a row shows it beside the list (`?issue=`); selected row highlighted | `row_el` → `State::selected` | 🔧 opened a drawer | `page_matches_web` |
| Bulk selection | none on the Backlog page (only the Goal page has a bulk bar) | removed | 🔧 the native page had checkboxes and a bulk bar | `row_actions_and_filters_send_what_the_web_sent` (no bulk posts) |
| `currentIssueRef` / `issueAside` | the picked issue, else the first row; kind, state or "Open · reported at", ref, title, "Open task T7", the detail; no actions box; Loading… / error | `render_aside`, `aside_view`, `sync` | 🔧 (was the drawer) | `page_matches_web` (aside cases) |
| `keepBacklogRow` / `withKept` / `refreshKept` | changed rows stay at their old index until the filters change; refreshed from the answer or `GET /backlog/:id` | `keep_row`, `with_kept`, `sync` | 🔧 kept copies were stale | `kept_rows_go_back_in_place_and_refresh`, `page_matches_web` |
| `issuePanel` | only on the Board: Backlog · kind · ref · close; title; project, state (· Open it); open → "Not part of any task yet. Make it a task to queue it[, or send it to Jira]." + Make it a task (board) · Create ticket · Won't do + note; detail; See every backlog issue | `render`, `panel_view` | 🔧 had goal-plan / queue variants, a different sentence, and a Reopen button the web never had | `issue_panel_matches_web` |
| `issueDetail` move | Goal picker (Not in a goal + goals, searchable), note under it | `render_detail`, `open_picker` | 🔧 was a plain menu | `issue_panel_matches_web` |
| `issueDetail` how | sentence, quote, detail as plain text with line breaks (not markdown) | `render_detail` | 🔧 detail was rendered as markdown | `issue_panel_matches_web` |
| `issueDetail` what was happening | rows (regular font) or "No snapshot was saved with this issue."; "Open T4's log at …" → the Board with that task's Log | `render_detail`, `open_log` | 🔧 values were mono; the link stayed on the current page | `issue_panel_matches_web` |
| `issueDetail` history | oldest first, time (full time on hover), dot colour by kind, "who · text"; "Nothing yet." | `render_detail` | ✅ | `issue_panel_matches_web` |
| `promote` | `POST /backlog/:id/promote {where}`; on the Board opens the new task (else "Made into a task"); elsewhere no message | `promote` | 🔧 toasted "Made into T5" off the Board | `promoting_on_the_board_opens_the_new_task`, `row_actions_and_filters_send_what_the_web_sent` |
| `ticket` / `drop` | `POST …/ticket {}` → "Asked Jira for a ticket"; `POST …/drop {}` → "Closed as won't do" | `ticket`, `drop_issue` | 🔧 messages were toasts, now inline notes | `panel_actions_send_what_the_web_sent` |
| `keepIssue` | on the Board: keep in the Backlog column (`?keep=`); on the Backlog page: keep the row | `keep_issue` → `board::keep_issue`, `keep_row` | ✅ | `panel_actions_send_what_the_web_sent` |
| `run` / `note` / `setNote` | one call per button ("Sending…", disabled), group note cleared first, ok note 5 s / error 15 s inline, then refresh | `run`, `set_note`, `Ctx::act` | 🔧 was toasts and a single busy flag | `issue_panel_matches_web` (busy cases) |
| Picker (`openPicker`, `pickerItems`, `pickerKey`) | specials first, goals sorted by name with "project · epic", projects with ~ paths, "Current" marker, query ranks starts-with / word / contains (and searches the sub line for goals), ↑↓ ↩ esc | `picker_items`, `picker_label`, `render_picker` | 🔧 new | `picker_items_rank_and_filter_like_web` |
| Esc | closes the Board's issue panel; nothing on the Backlog page | app.rs `CloseOverlay` (panel only on the Board) | ✅ | — |
| Page title | "(N) Backlog" | app.rs | 🔧 app.rs `page_title` says "(N) Backlog" | `app::tests` (title golden) |
| Leaving / coming back (`resetPageState`) | kept rows, order and `?issue` reset when the route changes | `enter` (called by "See every backlog issue") | 🔧 app.rs `go()` calls `backlog::enter(m, None)` on entering the page | `kept_rows_go_back_in_place_and_refresh` |
| Goal page aside | the Goal page's backlog shows the issue in an aside too (only the Board uses the panel) | `render_aside` + `aside_view` are public | 🔧 the Goal page uses `render_aside`; the drawer draws only off the Backlog and Goal pages | goal tests |
