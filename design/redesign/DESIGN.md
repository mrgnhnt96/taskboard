# Taskboard redesign: design rationale

This is a from-scratch design of the Mac app, built only from `FEATURES.md`. It uses no
earlier screens, themes or design records. The design is the ultrahand canvas in
`canvas/`: build it with `canvas/build.sh`, then open `canvas/index.html`. It covers 58
screens, each with five states (default, loading, empty, board down, first run).

There is no blueprint stage. Morgan reviews the full design in its place.

## What the app is for

The app answers one question: **what needs me right now, and what is the board doing
without me?** Most visits are short. You see a question or a PR waiting, deal with it
and leave. Longer visits are for planning (backlog, ideas, goals) or looking back
(days, releases).

The app stays mostly read-only. It keeps every action the inventory lists for the
current app, and adds none:

- start, stop and ack, and the start signals (Dig, Plan);
- New idea; shelve, drop, reopen and answer an idea;
- answers on human tasks and typed answers to questions, plus Talk it over;
- the existing backlog and goal commands (move, priority, defer, won't do, plan into
  waves, pause, resume, set aside, finish), merge and the miss actions;
- Undo and Reopen on the toasts;
- the setup retries;
- work hours, the worktree pool and the Jira account card in Settings.

Everything else shows the `tb` command that changes it.

## Information architecture

One window with three parts:

- **Sidebar.** Navigation and live status. It holds:
  - a scope picker (all projects or one project);
  - eight pages;
  - the active goals as quick links;
  - a status block for health, usage, work hours and Settings.
- **Main column.** The page. It is one column, capped at 880px (Days and a goal's waves
  get 1180–1280px).
- **Right panel.** One task or one backlog issue, opened from any row. It is the single
  home for a task (memory: one home per object). Goals expand in place and have their own
  page. There are no goal side panels.

| Page | Job | Leads with |
| --- | --- | --- |
| Needs you (home, ⌘1) | Everything waiting on the owner, most urgent first | Groups: Agents waiting, Decide, Your to-dos, Stopped, Ideas waiting; then Later |
| Goals (⌘2) | Every goal by where it stands | Active goals expand in place to waves; Next, Paused, Set aside, Finished |
| Goal page | One goal | Tabs: Waves, Notes, Attachments, Backlog, How it runs |
| Backlog (⌘3) | Untriaged issues | Lens (similar, by project, by kind), filters, bulk pick, plan into waves |
| Ideas (⌘4) | Ideas and their questions | List; the idea page has a stepped question flow and a scorecard |
| Days (⌘5) | Looking back | Day / Week in review / Recaps, chosen with a segmented control |
| Releases (⌘6) | Release state per project | Overview cards, then a project page: next cut, unreleased, past releases by month |
| Projects (⌘7) | Where each codebase stands | Picker plus the picked project's summary, open goals, loose tasks and finished goals |
| Settings (⌘,) | Its own window | Nine sections, as in the inventory |

## Theme: "Ledger"

The theme is a warm paper neutral with ink-dark type. Colour is used only for status, so
the screen reads like a ledger where only the exceptions are marked.

- **Surfaces:** `--surface`, `--surface-side`, `--surface-raised`, `--surface-sunken`,
  `--surface-hover`, `--surface-select`. Light and dark values come from `light-dark()`.
- **Status hues, one meaning each:**
  - `--needs` (amber): waiting on you.
  - `--error`: failed.
  - `--working` (blue).
  - `--review` (violet).
  - `--done` (green).
  - `--quiet`.
- **Status always has a glyph as well as its hue.** The glyph shapes differ (ring, half
  ring, check, cross, dashed ring), so the hue is never the only signal.
- **Primary:** one ink button per section, and only for the main action.
- **Type:** 12 / 13 / 15 / 22 px, in rem, so 200% text grows the words and not the controls.
- **Spacing:** 4 / 8 / 12 / 16 / 24 / 32 / 48.
- **Radii:** 4 / 8 / 12.

**Proposed theme additions** (for the theme skill, after review):

- `--pbar` segment colours, which reuse the status hues;
- a `table.fixed` variant.

## Key decisions

1. **Needs you is the home, grouped by what it asks of the owner, not by project.**
   - The owner's job is to unblock, so the groups run from most to least urgent: Agents
     waiting (a stalled agent), Decide (questions, reviews, misses), Your to-dos, Stopped
     (failed or paused work to restart) and Ideas waiting.
   - Each row has one main action in place. Everything else is in the row's ⋯ menu and
     its context menu.
   - *Principles: one primary per section; frequent members in view.*
2. **One right panel for a task or an issue, with tabs** (Story, Brief, Log, Handoff).
   - The attention banner sits at the top of Story and holds the one thing to do now.
   - The PR, checks and the timeline follow below it.
   - The panel docks at 1100px and wider, and overlays below that.
3. **Goals expand in place to their waves, and also have a page.**
   - There are no goal peek panels, so the right panel is only ever a task or an issue.
4. **The scope picker replaces per-page project filters.**
   - One picker at the top of the sidebar scopes every page.
   - Backlog keeps a project lens for grouping.
5. **Health lives in the sidebar's status block plus a popover.**
   - Board, Midna, hooks, accounts and updates share one place.
   - Problems surface as one line with a count. Nothing shows when all is well, which
     follows the no-filler-labels rule.
6. **Settings is its own window**, as on any Mac app: ⌘, opens it, and the close button at
   the top of its sidebar, ⌘W or Esc closes it (Esc in a field stays in the field).
   It has its own sidebar, with ⌘1–⌘9 for the inventory's nine sections in the
   inventory's order. The canvas draws it at the frame's size.
7. **Context menus are added.**
   - The inventory notes the current app has none.
   - Each one repeats commands that already exist in the row's ⋯ menu or the panel.
     *macOS HIG: context menus hold only commands found elsewhere.*
8. **Undo instead of confirmations.**
   - Undo covers start, stop, snooze, ack, sent answers and close. It is on the toast
     (shown for 10 seconds, held while the pointer or focus is on it) and on ⌘Z, which
     steps back through several actions. Each undo says what it undid and names the next
     one, so ⌘Z never reverses something you can't see.
   - After a row leaves the list, focus moves to the next row; Undo puts it back on the
     restored row.
   - Dialogs are only for "merge without waiting", "finish a goal without an outcome" and
     "confirm done from a link".
9. **Lists are virtualised.**
   - Backlog, the log, releases and Days render the visible rows plus a buffer.
   - They page from the daemon; the canvas shows the trailing loading marker.
10. **Time is shown in 12-hour form with AM/PM everywhere.**

## Size classes

| Window width | Layout |
| --- | --- |
| ≥ 1400 | The goal page and Days use two columns (`.two`). |
| ≥ 1200 | Full sidebar; panel 420px, docked. |
| < 1200 | Sidebar becomes a 56px icon rail; panel 380px. |
| < 1100 | Panel overlays the page (shadowed), so the list keeps its width. |
| < 960 | Ages and timeline lanes drop secondary columns. |
| < 840 | Compact: one column; secondary row actions go behind ⋯; the Projects picker wraps above the detail. |

- **Window size:** minimum 720 × 480, maximum 2560 × 1440.
- **Checked at:**
  - 1536 × 864, 1366 × 768, 960 × 1080 and 720 × 480;
  - dark mode and 200% text;
  - the composition audit at the largest and smallest sizes.

## Where every feature lives

Section numbers follow `FEATURES.md`.

1. **App shell and navigation**
   - Window, lifecycle and the menu bar: the macOS menu bar (File, Edit, View, Go, Task,
     Window, Help) holds every command with its shortcut.
   - Back and forward: toolbar arrows, ⌘[ and ⌘].
   - Page list and quick links: the sidebar, including active goals.
   - Search palette: ⌘K, which finds tasks, goals, issues, ideas and commands.
   - Deep links: these open the matching page or panel. "Confirm done from a link" is a
     dialog.
2. **Health, setup and connection**
   - Health summary: the sidebar status block and the Health popover.
   - First run: a setup page with numbered steps and "Try again".
   - Board down: every page keeps its last data, with an offline line. Actions that
     change the board are shown as unavailable and, if chosen by click, menu or shortcut,
     say the board is offline; a new idea is kept as a draft. Opening, copying and
     "Restart the board" still work.
3. **Needs you**
   - Waiting summary, today's stats, groups, item kinds and their main actions.
   - Goal bundles: rows that unfold.
   - Human to-dos: Done plus a note, Can't do, Waiting, Don't wait, Not needed.
   - Miss actions: File again, Add to last wave, Finish without an outcome.
   - The ⋯ menu.
   - Put off until later: a Later menu on each row; the snoozed items sit under the
     "Later" group.
   - Ideas waiting: a group that links to the idea page.
   - Talk it over (⌘T): opens a Midna conversation.
4. **One task and its pull request**: the right panel.
   - Identity facts: the panel header.
   - Attention banner: answers, start, ack and the tester note.
   - Timeline: the Story tab.
   - PR, checks and merge: a PR block, plus the Merge-without-waiting and Merge-refused
     dialogs.
   - Human tasks: the to-do panel.
   - Other tabs: Brief, Handoff, Log.
   - Whole tester note: a dialog.
5. **Backlog**
   - The list with lens, kind filter and text filter. The project filter is the scope
     picker.
   - Bulk actions: a sticky bar.
   - Plan into waves: the Plan page.
   - Closed issues: a tab.
   - Issue detail: the right panel.
6. **Goals**
   - The stop model: one status line plus the attention banner.
   - The goals list.
   - The goal page, with tabs for Waves, Notes, Attachments, Backlog and How it runs.
7. **Ideas**: the list, New idea (a dialog) and the idea page (questions, answers,
   scorecard, close and reopen).
8. **Projects**: the project list (open first; "Nothing open" folded), the summary, open
   goals with progress (expand in place), loose tasks, and finished goals (five, then
   Show all).
9. **Releases**
   - Releases off: the empty state. The navigation entry is hidden while every project has
     releases off.
   - The overview: per-project cards with a per-project error and Try again.
   - The project page:
     - the next release (cuts after, already finished);
     - unreleased work;
     - a dropped cut, with its CLI command;
     - past releases by month, the newest expanded;
     - a failed release, with "was in it".
10. **Days, weekly review and recaps**: the Days page with Day, Week in review and Recaps.
11. **Settings**: Accounts (GitHub device flow), History, Usage, Work hours, QA,
    Questions, Reviewers, Worktrees, System.
12. **Shared behaviours**
    - Toasts with Undo; the status vocabulary (glyph plus words).
    - Accessibility: full keyboard use, visible focus, 24px targets, and no hover-only
      content.
    - Formatted text: quoted message content is set as blockquotes.
13. **Keyboard shortcuts**: shown in menus and tooltips. ⌘1–⌘7 go to the pages, ⌘K
    searches, ⌘N starts a new idea, ⌘, opens Settings, ⌥⌘I toggles the panel, ⌃⌘S
    toggles the sidebar, ⇧⌘C copies the open task's ref, ⌘Z undoes, and ⇧F10 opens the
    context menu. The page numbers follow the new sidebar order, so they differ from the
    inventory's (Projects moves from ⌘2 to ⌘7, Releases from ⌘7 to ⌘6).
14. **Daemon API cross-check**: features that are only in the CLI stay there. See below.

### Merged

- **Alerts → Needs you.** Alerts become items in the home list, so there is one list of
  things waiting on the owner.
- **Per-page project filters → the sidebar scope picker.**
- **Midna status → the Health popover and Settings › System.**
- **"Nothing needed" banners → shown by absence.** An empty group is simply not drawn
  (no filler labels).
- **Bits → shown as "Feature flags"** on the task panel.

### Dropped or left to the CLI, on purpose

These follow the read-only rule. Each place shows the `tb` command instead:

- release cut and project add;
- History, Usage, QA, Questions and Reviewers settings;
- goal run settings and a wave review stop;
- connecting accounts other than GitHub.

The canvas's earlier "Try again" on a failed release was removed, because release retries
are CLI-only.

## Open questions for Morgan

1. Should Settings › Usage and History become editable, as Work hours is? They are
   read-only now, following the app-is-read-only list.
2. Context menus are new. Keep them, or leave every command on the ⋯ menu only?
3. Keep the scope picker as the one project filter, rather than a filter on each page?
4. The backlog and goal commands (move, priority, defer, won't do, pause, set aside,
   merge anyway) are kept because the current app has them. Should any of them become
   CLI-only to keep the app closer to read-only?
