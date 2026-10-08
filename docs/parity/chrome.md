# Parity: chrome (sidebar, banner, status bar, work hours, toasts, title, keys, markdown)

Reference: the frozen web UI in `crates/taskboard-app/parity/web/` (`app.js`, `pages.js`).
Goldens: `parity/gen/chrome.mjs` → `parity/golden/chrome.json` (221 cases, produced by the web's own
functions with the clock frozen at 2026-10-07 15:00 UTC).

Status: ✅ same · 🔧 fixed in this pass · ↔ intentional native difference (reason given).

## Goals rail → sidebar (board, backlog and sessions pages)

| Web function | Behaviour | Native | Status | Test |
|---|---|---|---|---|
| `goalCounts` | n / done / active / queued / prs from the summary, or from `tasks` on a goal detail (`awaitingMerge`: done, not failed, PR open); finished = all done and no open PR | `sidebar.rs: goal_counts` | 🔧 (had its own "finished" rule, no task-list branch) | `status_counts_ring_match_web` |
| `goalDone` | finished per `goalCounts` | `sidebar::finished` | 🔧 (also used `finished_at`) | same |
| `agentsHeldWhy` / `agentsHeld` | "After hours" when on and closed; "Out of usage" when the 5-hour window is at 100 % | `sidebar::held_why` | 🔧 (missing) | same |
| `goalStatus` | Awaiting merge / N PRs awaiting merge, "N task(s) need(s) you", Running, Starting, Paused, Blocked, "Queued until agents can start", Queued, or nothing | `sidebar::status_of` | 🔧 (wording was "2 need you", "N working · …"; no Starting/Blocked/held) | same (16 goals × 3 states) |
| `goalRing` | ring: done arc (green), done+active arc (accent), track; tick when finished, else the status glyph | `sidebar::ring` + `ring_el` (canvas) | 🔧 (was a flat progress bar) | `goalRing` cases |
| `GOAL_GLYPH` | play (working/starting), ! (needs), pause, moon (held), lock (blocked), clock (queued) | `ring_el` | 🔧 | snapshot |
| `goalRowHtml` | ring, "G3" + name, done/n, status as hidden label (tooltip natively), selected row highlighted with an "Open the goal" button, finished/deprioritized greyed | `sidebar::row`, `rail_row` | 🔧 | `rail_matches_web` |
| `goalsRailHtml` brand | "Task board" + New task | `sidebar::render` brand | 🔧 (button was below the nav) | `rail_matches_web` (brand) |
| `goalsRailHtml` head | "Goals ›" (opens the goal list) + count of active goals + New goal | `rail_list` head | 🔧 | `rail_matches_web` (head) |
| `goalsRailHtml` groups | projects = live terminals' projects ∪ active goals' projects ∪ the filter's project, sorted (`localeCompare`); each group folds (`tb.rail.shut`), stays open while it holds the picked goal, shows a needs dot when folded, highlights the filtered project, counts its goals | `sidebar::rail_view` | 🔧 (was a flat list) ↔ a project with no active goals isn't listed unless the board is filtered to it (asked for: empty projects were noise) | `rail_matches_web` (9 rails) |
| `goalsRailHtml` filter | active = not finished and not deprioritized (deprioritized goals hidden) | `rail_view` | 🔧 (showed them) | same |
| `goalsRailHtml` none | "No goals yet" / "Loading…" when there's no project at all | `rail_view.none` | 🔧 | same |
| `recentSince` / `recentlyDone` | finished in the last day, or since Friday's start on weekends and Mondays (local time) | `sidebar::recent_since`, `recently_done` | 🔧 (showed every finished goal) | `recent_window_matches_web` |
| `recentHtml` | "Recently completed" group, newest first, folded unless `tb.rail.recent` = open or it holds the picked goal | `rail_view.recent`, `rail_list` | 🔧 | `rail_matches_web` |
| `goal-pick` | toggle the board's goal filter; picking sets the project to the goal's; un-picking clears the project; clears kept backlog cards; saves the filter | `MainWindow::rail_pick_goal` → `board::goal_pick` | 🔧 (navigated to the goal page instead) ↔ also switches to the Board page (the rail is visible on every page natively) | `goal_pick_matches_web`, `rail_clicks_filter_the_board` |
| `rail-project` | a project name filters the board to it (again: all); with a goal picked, switches to the whole project | `sidebar::pick_project`, `MainWindow::rail_pick_project` | 🔧 (missing) | `rail_actions_match_web` |
| `rail-fold` / `setRailShut` | fold a project, saved as a JSON array string in `tb.rail.shut` | `sidebar::toggle_shut` | 🔧 | same |
| `rail-recent` | toggle `tb.rail.recent` open/closed | `sidebar::toggle_recent` | 🔧 | same |
| `railClear` | Esc on the board with a filter: goal and project back to all, kept cards cleared, saved | `sidebar::rail_clear`, `MainWindow::rail_clear` | 🔧 | `rail_actions_match_web`, `escape_follows_the_web_order` |
| `railWidth` / `setRailWidth` / `bindRailGrip` | width from `tb.rail.w` clamped 220–600, default 300; drag the grip; double-click resets | `sidebar::rail_width`, `save_rail_width`, grip in `render`, drag in `app.rs` | 🔧 (fixed 248 px) | `rail_actions_match_web` (railWidth) |
| `bindRailGrip` arrows | ←/→ on the focused grip moves 16 px | — | ↔ not ported: the grip isn't keyboard-focusable natively | — |
| `goalPeekHtml` | hover card: "G1 name", [held reason ·] status (held shows as "Queued") · d of n done [· N planned], then Needs you / Running / Failed / Starting / Blocked groups with count and tasks (ref, title, why) | `sidebar::peek_view`, `render_peek` | 🔧 (missing) | `peek_matches_web` |
| `peekShow` / `peekHideSoon` / hover listeners | shows after 300 ms (at once when a card is already up), hides 150 ms after leaving unless over the card; a task opens it and hides the card | `on_goal_hover`, `hide_peek_soon`, `render_peek` | 🔧 | snapshot (manual) |
| peek on focus / scroll / mousedown | keyboard focus shows it; scrolling or clicking elsewhere hides it | — | ↔ rows aren't keyboard-focusable; the card hides on leave | — |
| `.bg-item.on` / `.bg-proj.on` styles | goal tint + line for the picked goal; accent soft for the filtered project | `rail_row`, `group_head` | 🔧 | snapshot |

## Goal list (goal page) → sidebar on the goal page

| Web function | Behaviour | Native | Status | Test |
|---|---|---|---|---|
| `goalNavList` | Active / Deprioritized / Finished tabs with counts; grouped by project ("No project"), sorted; finished newest first; empty texts "No active goals" / "No deprioritized goals" / "No finished goals yet" | `sidebar::nav_view`, `nav_list` | 🔧 (missing: deprioritized and finished goals had no list) | `goal_list_matches_web` |
| `P.gnav` | the view follows the current goal when the goal changes, else keeps the chosen tab | `sidebar::nav_view_for`, `State.gnav` | 🔧 | same |
| `goalNavStatus` | Awaiting merge / Finished / Needs you / N need you / Paused / Running / N running / Blocked / Queued / No tasks yet / Not running / Not started, with kind colours and a dot unless idle | `sidebar::nav_status` | 🔧 | `status_counts_ring_match_web` |
| `goalItem` | ring, ref + name, status, "· d of n done", current goal highlighted | `nav_list` | 🔧 | `goal_list_matches_web` |
| `gnav-head` | "Goals" + New goal | `nav_list` | 🔧 | snapshot |
| `gnav` back link | "← Task board" | Board in the sidebar's page list | ↔ the page list is always there | — |
| `renderGoalPage` with no id (`#/goals`) | open the board's goal, else the first active goal by project, else the first | `sidebar::landing`, `MainWindow::open_goals` | 🔧 — note: in the web this branch threw (`nav` used before its `const`), so the link never worked there | `goal_list_matches_web` (landing), `goals_link_opens_the_first_active_goal` |

## Banner and alerts

| Web function | Behaviour | Native | Status | Test |
|---|---|---|---|---|
| `renderBanner` (down) | "<error> Trying again every few seconds." | `app::banner_view`, `banner` | ✅ | `banner_and_alerts_dialog_match_web` |
| `renderBanner` (alerts) | one line per alert with its age, "Open T4"/"Open G2" (task wins) and Dismiss; with more than one: "N tasks need your attention." + the last alert's age + Show all | `banner_view`, `alert_row` | ✅ (age now inline, as the web's `<small>`) | same |
| `alert-open` | closes the alerts dialog, opens the task, else the goal | `app::open_alert` | ✅ | — (navigation covered by `notification_links_open_the_right_place`) |
| `alert-dismiss` | `POST /alerts/:id/dismiss {}`, no toast | `app::dismiss_alert` | ✅ | `dismissing_alerts_posts_like_the_web` |
| `alertsDialogHtml` | title "N tasks need your attention", rows as above, foot "Dismiss all" | `app::alerts_dialog_view` (data); drawn by `modals.rs` | ✅ data; see "Outside this area" | `banner_and_alerts_dialog_match_web` |
| `alerts-dismiss-all` | dismiss every alert; the dialog closes once none are left (`renderBanner`) | `app::dismiss_all_alerts`; auto-close in `Got::apply` | 🔧 (auto-close) | `dismissing_alerts_posts_like_the_web` |
| login-item banner | — | `banner` (install status) | ↔ native only: the app registers the daemon | — |

## Status bar

| Web function | Behaviour | Native | Status | Test |
|---|---|---|---|---|
| `statusBarHtml` | connection, hours, usage pills in that order; each only when its data is there | `app::status_bar` | ✅ | — |
| `connPill` | "Connected to Midna" (green pill) / "Midna isn’t running" (warn pill); tooltip "Last heard from …" / "Not heard from yet" (+ the wait sentence when down) | `app::conn_pill` | 🔧 (was plain text with a dot) | `status_bar_matches_web` |
| `hoursPill` | "No work hours", "Work hours until 4pm today", "Work hours 9am–5pm", "Agents off until [Thu ]6am" (warn pill); tooltip = `line`; ▾ | `hours::pill_view`, `pill` | 🔧 (pill style; trailing space when `next_open` is missing) | `pill_matches_web` |
| `usagePill` / `usageLevel` / `USAGE_NAMES` / `resetWhen` | per window: label, bar (0–100), "N%"; levels ok / warm ≥ 75 / hot ≥ 90; tooltip "5-hour limit: 35% used, resets 6pm" / "7-day …", + "Last read 2h ago" when over an hour old (greyed); hidden with no windows | `app::usage_view`, `usage_pill` | 🔧 (warm was ≥ 70; "Weekly" instead of "7-day"; value was clamped; an unreadable `seen_at` counted as stale) | `status_bar_matches_web` |

## Work hours menu

| Web function | Behaviour | Native | Status | Test |
|---|---|---|---|---|
| `openHoursMenu` | copies on/start/end/days/today from `work_hours` | `hours::State::from_hours` | ✅ | `save_body_matches_web` (openHoursMenu) |
| `hoursMenuHtml` | "Only start agents during work hours" check; From / to selects; S M T W T F S days (tooltips Sun…); "Today until" select with "Usual end"; all but the check disabled when off; error note | `hours::render_menu` | 🔧 (time steppers replaced by the same select lists) | `menu_matches_web` |
| `hourOptions` | every half hour labelled 12am…11:30pm, plus the current value when off the grid | `hours::hour_options`, `today_options` | 🔧 | `menu_matches_web` |
| `hours-day` | toggle a day (appended) and save | `State::toggle_day` | ✅ | `changing_a_day_posts_hours` |
| `saveHours` | `POST /hours {on, start, end, days}` + `today_until` only when changed (`""` → `"off"`); success replaces `work_hours`; failure shows in the menu | `State::body`, `hours::save` | ✅ | `save_body_matches_web`, `changing_a_day_posts_hours` |
| close on Esc / outside click | | `MainWindow::escape` (menu first), menu dismiss layer | ✅ | `escape_follows_the_web_order` |

## Toasts, title, keyboard

| Web function | Behaviour | Native | Status | Test |
|---|---|---|---|---|
| `toast` | one at a time (a new one replaces it); 3 s, errors 6 s | `MainWindow::toast` | 🔧 (stacked up to 3, 4 s) | `one_toast_at_a_time` |
| `renderAll` title | "Task board" / "Goals" / "Backlog" / "Sessions", "(n) " in front while tasks need you | `app::page_title`, `MainWindow::title` | 🔧 (always "Task board") | `title_matches_web` |
| keydown Esc | in order: picker (forms), hours menu, rename field, a read-only dialog (alerts; forms keep their typing), the task panel / an issue panel on the board, then `railClear` on the board when nothing is typed in | `MainWindow::escape` | 🔧 (closed any dialog, never cleared filters) | `escape_follows_the_web_order` |
| keydown Enter/Space on `role=button`, `data-enter`, ⌘↩ answer | | — / owning pages | ↔ native buttons are clicked; ⌘↩ lives in the answer box (task panel) | — |
| ⌘N, ⌘⇧N, ⌘⇧B, ⌘1–3, ⌘R | — | `app::bind_keys` | ↔ native additions (menu bar) | — |
| `visibilitychange` refresh / 3 s poll | | `start_polling` | ✅ | — |

## mdLite

| Web function | Behaviour | Native | Status | Test |
|---|---|---|---|---|
| `mdLite` | `\r\n` → `\n`, trim, cut at `max` (UTF-16) at a newline past 60 % + " …"; paragraphs joined with line breaks; `#`–`######` headings; `-*+` and `1.`/`1)` lists with indented continuation; fenced code (unclosed runs to the end); inline: escape, `code`, **bold**, *italic* (not inside words), [text](https://…), bare http(s) links minus trailing punctuation — each a global regex pass over the previous result | `md.rs: prepare`, `parse`, `inline` (the same passes, same markup) | 🔧 (the old port differed: headings needed exactly `# `, inline order, `***`, links inside code, cut rule) | `md_lite_matches_web` (28 inputs, exact HTML) |

## Outside this area (requested of other files)

- `modals.rs` (alerts dialog): use `app::alerts_dialog_view` for the rows and `app::dismiss_all_alerts` for "Dismiss all", and don't close the dialog immediately (it closes itself when the alerts are gone, as the web did).
- `backend.rs`: the web's error texts were "Can’t reach the task board server." (no connection) and "The board answered N." (non-JSON error); `Daemon::answer` says "The board isn't answering at …: …". The banner shows whatever the backend says.
- Esc now leaves forms open (web `READ_ONLY_MODALS` = `gnote`, `alerts`): forms must close with their own Cancel / esc handling; the goal note dialog (`gnote`, goal page) should close on Esc itself.
