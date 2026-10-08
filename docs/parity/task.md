# Parity: the task side panel

Web reference: `crates/taskboard-app/parity/web/app.js` (`taskPanel` and friends) and `pages.js` (`noteBody`,
the attachment actions). Native: `crates/taskboard-app/src/ui/task_panel.rs` (drawing, `act()` = the web's
`ACTIONS`, `run()` = the web's `run()`), `task_panel/view.rs` (the panel as a node tree: one function per web
function), `task_panel/tests.rs`.

**How it's checked.** `parity/gen/task.mjs` renders the web's own `taskPanel()` for 99 states (loading, fetch
errors, every needs/lost/start-failed/queued/planned/working/done/failed variant, every PR phase, Jira on/off,
attachments with menu/edit, the Context and Log tabs, trails, busy buttons, inline notes, folds, drafts) and
records, per state: the visible text (closed `<details>` show only their summary), every control (`data-act` +
`data-arg`, disabled marked), every tooltip (`title`) and every field's contents. `panel_text_matches_web`,
`panel_controls_match_web`, `panel_tooltips_match_web` and `panel_fields_match_web` require the native tree to
produce exactly the same four lists for every state. Action tests press the control the panel offers (looked up
in the tree, so a missing button fails) on a headless window and assert the exact request.

Legend: ✅ same · 🔧 fixed in this pass · ↔ intentional native difference (reason given)

| Web function | Behaviour | Native | Status | Test |
|---|---|---|---|---|
| `renderPanel` | Task panel when `?task=`; issue panel otherwise | `app.rs` render → `task_panel::render` | ✅ | `renders_every_tab` |
| `taskPanel` (loading) | Back button (if trail), ref pill, close; "Loading…" or the fetch error in the error style | `view::panel` | 🔧 (error shown, was always "Loading…") | golden `loading`, `fetch failed` |
| `taskPanel` | Top row: back, status pill, "Needs you" alert pill (tooltip = alert text), "High priority", ref, close | `view::panel` | 🔧 (alert pill and back button were missing) | golden `needs: alert…`, `trail back button` |
| `taskPanel` | Fetch error over a loaded task (`S.taskErr` note under the top row) | `view::panel` (`Ctx.err` ← `m.data.errs.task`) | 🔧 | golden `fetch failed after loading` |
| `taskPanel` | Title; subline: project pill, `whenLine`, `tookLine`, capitalised `runningFor` (working/needs only) | `view::panel` | ✅ | golden (all) |
| `taskPanel` | Alert box "Needs you" + text | `view::panel` | ✅ | golden `needs: alert…` |
| `taskPanel` | Tabs Overview / Context / Log (`tab` act; any other tab → overview) | `view::panel`, `act("tab")` | ✅ | golden acts, `tabs_and_log_filter` |
| `statusPill` / `prStatus` / `awaitingMerge` / `prStopped` | Awaiting-merge pill: stage label in `PR_STAGE_CLS` tone, "Awaiting merge", or "Needs you" when stopped; tooltips; "Starting"; `STATUS[stKey]` | `view::status_pill` | ✅ | golden `done PR: *`, `queued starting`, `queued blocked` |
| `hoursOpenAt` | "Thu 6am" / "6pm" while hours closed with a next open; "" otherwise | `view::hours_open_at` | ✅ | `helpers_match_web` |
| `answerBtns` | "Send answer"; "Send at …" (tooltip) when hours closed | `view::answer_btns` | ✅ | golden `needs: question, hours closed/open later today` |
| `savedLine` | "Saved at 2:40 PM · 3 turns · 2 checkpoints" | `view::saved_line` | 🔧 (time format was "2:40 pm") | golden `lost: can reopen`, `context: full` |
| `overviewTab` lost box | "Terminal lost", latest or default, saved line, Resume (fresh) / Reopen (disabled + tooltip without a conversation), note, "See what the new terminal gets" (→ Context) | `view::overview_tab` | ✅ | golden `lost: *`, `lost_task_resumes` |
| `overviewTab` start failed | "Couldn’t start in Midna", latest or default, New Midna terminal / Queue in Midna, note | `view::overview_tab` | ✅ | golden `start failed*` |
| `overviewTab` needs box | "{who} is asking/needs you", question/latest/default **as plain text**, textarea (draft), answer buttons, Focus (no question + live terminal), note | `view::overview_tab` | 🔧 (question was rendered as markdown; "⌘↩ to send" hint removed: the web had none) | golden `needs: *`, `answer_sends_text_and_clears_the_draft` |
| `overviewTab` starts by hand | Start / "Start when the repo’s free", note, "Not starting yet: …" | `view::overview_tab` | ✅ | golden `queued by hand*`, `start_buttons` |
| `overviewTab` planned | "Queue it now" (tooltip), note | `view::overview_tab` | ✅ | golden `planned*`, `planned_task_queues` |
| `overviewTab` stopped PR | "{terminal} asks you about / stopped on the PR", message, textarea, answer buttons + Focus, note | `view::overview_tab` | ✅ | golden `done PR: stage fix stopped`, `…comments asked`, `done PR stopped, draft` |
| `overviewTab` done box | Foldable "Summary from {who}" (**closed by default**, `tb.fold.summary`), summary/latest/default as plain text, Try again / Queue again, Close its terminal (live), note | `view::overview_tab` | 🔧 (summary wasn't foldable and was markdown) | golden `done: *`, `done_task_requeues` |
| `manageBox` | Focus (working + live), Detach (session id or live, tooltip), Close its terminal (force unless idle, tooltip); note. **No heading**. No Mark done / Mark failed: Claude finishes tasks (`tb done`, `tb fail`) | `view::manage_box` | 🔧 (removed a "Manage" heading the web never had) | golden `working: *`, `manage_box_actions` |
| `linkedOpen` / `foldOpen` | Details open unless `tb.linked=closed`; summary/what/terms closed unless `tb.fold.*=open`; toggling saves | `Ctx::linked_open/fold_open`, `Draw::c_fold` → `prefs` | 🔧 (not persisted; "What to do" was open by default) | golden `working: details closed`, `…earlier terminals open`, `folds_are_saved` |
| `blockedRow` | "Blocked by": ref (opens it from the panel) + status pill, title | `view::blocked_row` | ✅ | golden `queued blocked` |
| `goalRow` | Hidden on that goal's page; name link, "Task n of m" (integer, ≤ total), next / last, backlog link; "Not in a goal." | `view::goal_row` | 🔧 (`position` 0 was hidden; web shows it) | golden `goal *`, `planned, goal page of its goal` |
| `termItem` / `waitsOnYou` | Dot, name link (tooltip "Open this terminal") or plain name, Focus icon (live), state pill (Idle/Working/Needs you/Gone/Closed), why (Task/PR/free text or the stopped-PR reason), time | `view::term_item` | 🔧 (JS `===` on missing vs `null` ids now matches) | golden `working: *`, `done PR: *` |
| `termHref` | Terminal name / PR bar → Sessions page on that terminal with "Back to T12" | `task_panel::go(Go::Session)` → `sessions::open_from_task` | ✅ | `links_leave_the_panel` |
| `terminalRow` | Synthesised rows (live session / `who`), rank sort, first shown when > 2 with "N earlier terminals" fold, "Terminal(s)", "No terminal (yet)." + note | `view::terminal_row` | ✅ | golden `working: *`, `needs: no question, no terminal` |
| `jiraRow` | Hidden without Jira unless a key; key link (tooltip) or key, status pill; "Ticket asked for"; "No ticket." | `view::jira_row` | ✅ | golden `jira: *` |
| `checksStep` / `reviewStep` / `mergeStep` / `stepHtml` | Names, states, subs for every combination | `view::pr_steps` | ✅ | golden `done PR: *` |
| `prBar` | Fix/comments/merge on an open PR: label or "Needs you · label", "Terminal ›" link, tooltips | `view::pr_row` | ✅ | golden `done PR: stage *` |
| — | Native: "I reviewed it" while a green PR waits for its owner (`stage.awaiting_you`): `POST tasks/:id/pr/reviewed`, note "Marked reviewed" | `view::pr_row`, `act("pr-reviewed")` | ↔ new | `reviewed_button_shows_only_while_the_pr_waits_for_you`, `reviewed_button_tells_the_board` |
| `prRow` | "#n title" (Jira key trimmed off the title), repo, link tooltip; "No PR yet." | `view::pr_row` | ✅ | golden `done PR: no url…`, `…no title` |
| `attSrc` / `attachList` / `attachRow` | "Attached N"; name link (tooltip title, or title + path), mono name without link; "Kind · src · age"; task source chip opens the task (shown even for this task, as the web did); goal source chip; empty text; note | `view::attach_items`, `view::attach_row` | 🔧 ("Attached (N)" → "Attached N"; own-task source was hidden) | golden `attachments*` |
| `attMenuHtml` + `att-*` | ⋯ menu: Open (if a link), Copy link / Copy path ("Copied" toast), Edit, Remove ("Removed") | `view::attach_items`, `act("att-*")` | 🔧 (menu was flat buttons; no Edit) | golden `attachments: menu open*`, `attachments_edit_and_remove` |
| `attEditHtml` + `att-esave` | Kind segment, title, link/path fields, Save (sends only the fields changed) / Cancel | `view::att_edit`, `act("att-esave")` | 🔧 (ported; was missing) | golden `attachments: editing*`, `attachments_edit_and_remove` |
| `attHref` | Path attachments open via `/tasks/files/:id` | `task_panel::open_target` (`open` on the path) | ↔ the app opens the file directly instead of through the daemon's file route | — |
| `handoffText` / `loadHandoff` | Handoff string / `{text}` / fetched on the Context tab when missing ("Loading…", or "Couldn’t load the handoff: …") | `view::handoff_text`, `task_panel::load_handoff` | 🔧 (fallback fetch was missing) | golden `context: handoff *` (fetch itself not exercised: the sample board always returns a handoff) |
| `metaRows` / `saveMeta` / `meta` input & change / `meta-del` | Editable name/value rows (only when there are fields), saved on Enter or leaving the field with rows trimmed and empty rows dropped; remove a row ("Removed") | `view::meta_rows`, `task_panel::save_meta`, `on_input`, `on_key`, `act("meta-del")` | 🔧 (rows weren't editable; an "+ Add a field" form the web didn't have was removed) | golden `context: meta*`, `details_fields_edit_in_place` |
| `contextTab` | Saved line; where (Branch / Worktree / Last commit / Not committed / Conversation); Done so far / Next / Decisions / Your answers with coloured glyphs; Found (link → board + issue panel, "(in the [goal’s ]backlog)"); Files touched (basename, tooltip path) / "None yet."; Details; "What a new terminal gets" | `view::context_tab` | 🔧 (removed a Copy button on the handoff the web didn't have) | golden `context: *`, `links_leave_the_panel` |
| `logTab` | Filter chips (`S.logFilter`, kept across tasks), items: time (tooltip full time), dot, who / "Task board", kind label, text; empty texts; help line | `view::log_tab` | 🔧 (filter reset per task; times were "2:59 pm"/"Oct 7, 2:59 pm") | golden `log: *`, `tabs_and_log_filter` |
| `noteBody` / `inlineText` | "What to do": paragraphs with line breaks, `- `/`* `/`• ` lists (indented lines continue an item), indented code, links / `code` / path-like words | `view::note_body`, `view::inline_text` | 🔧 (was rendered with the markdown subset `mdLite`) | golden `working: what to do open` |
| `openTask(r, tab, fromPanel)` / `taskTrail` / `task-back` | Opening a task from inside the panel remembers this one; back pops; opening the same task or from elsewhere clears it | `open_from_panel`, `act("task-back")`, `TRAIL` | 🔧 (was missing) | golden `trail back button`, `trail_and_back` |
| `close-panel` | Closes the panel and clears the trail; Esc does the same | `act("close-panel")`; Esc in `app.rs` | ✅ | `trail_and_back` |
| `run` / `btn` busy | One request per button; "Sending…" + disabled meanwhile; group note cleared, then the ok sentence or the board's error inline (5 s / 15 s), toast when no group | `task_panel::run`, `Ctx::btn`, `set_note` | 🔧 (was toasts, no busy state) | golden `needs: question, sending`, `queued by hand, busy start`, `a_busy_button_sends_once` |
| `sentNote` | "Sent to Midna" / "Saved. It runs once Midna is back." | `view::sent_note` | ✅ | `helpers_match_web` |
| `answer` | Empty → "Write an answer first." (error note, no request); `{text, when: now/morning}`; clears the draft; morning note "Saved. It sends at …" | `act("answer")` | 🔧 (wording was "Answer sent"/…) | `answer_sends_text_and_clears_the_draft`, `answer_later_waits_for_work_hours` |
| ⌘↩ in an answer | Presses Send answer | `task_panel::on_key` | ✅ | — (key path; the action itself is tested) |
| `start` / `queue-planned` / `requeue` / `detach` / `close-term` / `focus` / `term-focus` / `resume` | Paths and bodies as the web (`{mode}`, `{status: queued}`, `{}`, `{force}`), ok sentences ("Queued", "Back in the queue", "Detached. It’s back in the queue.", `sentNote`) | `act(...)` | 🔧 (ok sentences and toasts differed) | `start_buttons`, `planned_task_queues`, `done_task_requeues`, `manage_box_actions`, `lost_task_resumes` |
| `S.drafts` | Answer / finish-form / attachment-edit drafts survive switching tasks | `UI.drafts` | 🔧 | `answer_sends_text_and_clears_the_draft` |
| `goalRow` backlog link | `#/goals/G1?view=backlog` opens the goal on its Backlog tab | `go(Go::GoalBacklog)` → goal page | 🔧 `goal::open(m, r, true, cx)` opens its Backlog tab | `parity::goal_backlog_link_opens_the_backlog_tab` |

Not ported: nothing else in this area. `S.taskErr` for non-404 failures and the 404 case now come from
`m.data.errs.task` (app.rs).
