# Parity with the web board

Taskboard.app replaced a web UI (`crates/taskboard-app/parity/web/`, kept frozen as the reference).
This is the audit of the native app against it, one web behaviour per row, with the test that holds it.

| Area | Web functions | Checklist | Same | Fixed in the audit | Native difference |
|---|---|---|---|---|---|
| Chrome: sidebar (goals rail + goal list), banner, alerts, status bar, hours menu, toasts, title, Esc, markdown | `goalsRailHtml`, `goalNavList`, `renderBanner`, `statusBarHtml`, `hoursMenuHtml`, `mdLite`, … | [chrome.md](parity/chrome.md) | 11 | 39 | 7 |
| Board | `renderBoard`, `sessionsHtml`, `columnsHtml`, `taskCard`, `issueCard`, drag to Working, rename | [board.md](parity/board.md) | 13 | 19 | 2 |
| Task panel | `taskPanel`, `overviewTab`, `contextTab`, `logTab`, `prBar`, `attachList`, `metaRows`, … | [task.md](parity/task.md) | 23 | 24 | 1 |
| Goal page | `goalMain`, `goalTasks`, `notesAside`, `goalBacklog`, `bulkBar`, `goalRunButtons`, … | [goal.md](parity/goal.md) | 10 | 34 | 2 |
| Issue panel (the web's Backlog page is replaced by the native planning page, `ui/backlog.rs`) | `issueAside`, `issuePanel`, `issueActions`, … | [backlog.md](parity/backlog.md) | 12 | 29 | 0 |
| Sessions page | all of `sessions.js` + the rename helpers | [sessions.md](parity/sessions.md) | 23 | 32 | 5 |
| Forms and pickers | removed (the app has no forms); `localeCompare` still checked | [forms.md](parity/forms.md) | — | — | — |
| **Total** | | | **111** | **200** | **24** |

"Fixed in the audit" means the first native version differed from the web and now matches it: wording,
what shows when, ordering, which buttons are offered, request bodies, keys, saved state. Every native
difference is listed with its reason in the area's file; none drops a capability. They are layout
(one sidebar on every page instead of a rail on the board and a list on the goal page), things a native
window does differently (no URL routes, no keyboard focus on plain buttons, popovers that close on
outside click), native additions (menu-bar shortcuts, the login-item banner), and two
web bugs not carried over (the `#/goals` landing threw; a terminal from another project could be sent
with a new task).

## The app doesn't edit anything

The owner never makes or changes board data in the app; Claude does that through `tb` when asked.
The app keeps one-click signals only: start, resume, pause and stop a task or goal (drag to Working,
Run, Pause, Close its terminal, Detach, Requeue, Queue it), answer a task's question, "I reviewed it"
on a PR waiting for you, triage buttons (Make it a task, Create ticket, Won't do), dismiss alerts,
and Settings / work hours. Removed on 2026-10-08, from the app and from the frozen web copy so the
goldens still match:

- the New task, New goal, Edit goal and Add an issue forms (and their ⌘N / ⌘⇧N / ⌘⇧B shortcuts,
  File menu items, sidebar buttons, and the goal page's "Add a task" / "Add an issue");
- Mark done… / Mark failed… in the task panel (`tb done`, `tb fail`);
- the goal page's Add a note (`tb note --goal`) and its attachments' Edit / Remove (`tb attach`, `tb unattach`);
- the issue's Add a note and goal picker, and the goal page's bulk "Move to a goal" (`tb backlog move`).

The task panel keeps its metadata and attachment editing and terminal rename, which the owner chose to keep.

## How it's tested

- **Goldens from the web's own code.** `parity/gen/<area>.mjs` loads the frozen web UI in node, runs its
  functions on fixtures (every state each area shows) and writes `parity/golden/<area>.json`: the
  visible text, the actions offered, the request paths and bodies. 1,034 cases. The app's tests feed the
  same inputs to its view-model functions and must produce the same answers. Regenerate with
  `crates/taskboard-app/parity/regen.sh` (needs node; the output is deterministic: frozen clock, UTC).
- **Action tests.** A headless `MainWindow` (`parity::window`) on a recording backend (the real board
  in-process) presses each button the app offers and checks the exact request it sends, and what the
  window does next. `parity::Recording::fail` makes fetches fail to check the error states.
- `cargo test --workspace` runs all of it (182 tests: 115 app, 7 CLI, the rest daemon and end-to-end).

Shared pieces checked the same way: `fmt` (times, durations, refs: `fmt_matches_web`), `localeCompare`
(`modals::locale_cmp`), `mdLite` (exact HTML for 32 inputs), the saved UI state (`prefs`, the web's
localStorage keys), fetch errors (`taskErr`/`issueErr`/`goalErr`/`backlogErr`/`SS.listErr`/`SS.detailErr`)
and the "isn't answering" banner (`api()`'s sentences).
