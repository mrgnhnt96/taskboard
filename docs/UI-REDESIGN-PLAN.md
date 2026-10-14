# Taskboard UI/UX redesign plan

Written 2026-10-14 from an audit of `crates/taskboard-app` (every page, panel, the status bar and key bindings) and
the daemon's data model. The three design concepts built on this plan are on the "Taskboard Redesign Concepts" canvas.

## What's wrong today

1. **Tasks live in four places and none of them is complete.**
   - Board shows only the *current wave's* tiles per goal.
   - Tasks hides goal tasks by default.
   - A goal page lists its own tasks.
   - Sessions shows a task only while a terminal holds it.

   A queued task in wave 3 can't be found without knowing its goal. There's no search anywhere.
2. **There's no single goals overview.** Goal cards sit on Board (grouped by project, with wave tiles) and again on
   Goals (with a progress bar and a status). Neither answers *what is each goal doing right now, what's it waiting on,
   and what runs next*. Chains (`after_goals`), `hours_until` and wave holds are in the data but not on screen.
3. **The navigation doesn't explain itself.**
   - There are six tabs, and "Board" and "Tasks" sound like the same thing.
   - Board stays highlighted on a goal page.
   - ⌘1–⌘6 don't follow the tab order (⌘2 is Backlog, ⌘5 is Tasks).
4. **Everything competes for attention.**
   - The status bar carries up to 10 pills.
   - Home puts Needs you, six KPI tiles and every goal's wave tiles on one screen.
   - A single task can show in three tiles at once.
5. **The UI breaks its own read-only rule.** Backlog triage (move, priority, defer, drop, create goal), Deprioritize,
   the goal run options, task Details edits, attachment edit/remove and session rename all mutate. Edits belong to
   Claude and `tb`.
6. **Statuses use too many words.** About 20 pill labels (Starting, Waiting for a terminal, Checks running, Comments…)
   each get their own styling, so a glance doesn't tell you working from waiting from blocked.

## Principles

- **Answer three questions in this order:** what needs me, what's running, and where everything is.
- **Every object has one home and one address.**
  - Tasks open in one panel, goals on one page, issues in one panel.
  - ⌘K finds any of them by ref (`T14`, `G3`, `B7`), by words, or by terminal name.
- **Six statuses, one colour each, the same chip everywhere:**
  - **Needs you** (amber)
  - **Working** (blue)
  - **Queued** (grey; a sub-line says why it's waiting)
  - **In review** (teal, for PR stages)
  - **Done** (green)
  - **Failed** (red; also lost and couldn't start)

  The detail (Starting, Waiting for a terminal, Checks failed, 1 of 2 approved) goes in the row's second line, not
  in new chip colours.
- **Read-only.** The only buttons are signals:
  - Start, Pause and Resume
  - Continue the wave
  - Answer, I reviewed it and Mark created
  - Focus in Midna, Close terminal
  - Dismiss and Snooze

  Any change to tasks, goals or the backlog is an **Open in Claude** hand-off.
- **No helper copy.** Labels only where they name data. 12-hour times.
- **Health is quiet until it isn't.** Midna, hooks, work hours, usage, accounts and updates fold into one health
  item. Anything broken turns it amber and moves into Needs you.

## Information architecture

| Destination | What it answers | Replaces |
|---|---|---|
| **Needs you** (home) | Every item waiting on you, most urgent first, each with one signal button. | Board's Needs you list, alert dialog, banners for broken health |
| **Running** | Each live terminal, its task, how long it's been at it, its last turn, what's lined up next; idle and closed terminals below. | Sessions page, Tasks "Running" lane |
| **Goals** | Every goal on one screen: progress split by status, current wave, what it's waiting on, chains ("runs after G4"). Grouped Active · Waiting · Chained/queued · Paused · Finished. | Board goal cards, Goals page |
| **Tasks** | Every task, goal tasks included, in one searchable list. Group by Status, Goal or Project; filter by project. | Tasks page, Board tiles |
| **Backlog** | Open issues, grouped (similar, area, impact), read-only; select some → **Plan in Claude**. | Backlog triage page and the goal page's Backlog tab mutations |
| **Days** | Unchanged in substance: timeline, week, hours saved. | Days |

Detail surfaces:

- **Task panel** (right side): status, goal and wave breadcrumb, PR bar, the one signal that applies, then
  Overview · Context · Log.
- **Goal page**: header (progress, state, chain, epic), a wave timeline as the spine with tasks under each wave,
  and a side column for notes, attachments, devices and bits.
- **Issue panel**: what was reported, where, history; Plan in Claude.

Keys:

- ⌘K opens search.
- ⌘1–⌘6 follow the order of the navigation.
- J/K move through a list and ⏎ opens.
- Esc closes the panel.

## Concepts

- **A — Sidebar & Inbox.** A Mail/Linear-style three-pane window. The sidebar holds the destinations and pins every
  active goal with a progress ring. The middle is a list and the right is the detail. Light.
- **B — Mission Control.** One overview screen. A Needs-you strip sits on top, goals run as horizontal swimlanes across
  their waves, and live terminals are on the right. Everything is visible without navigating, and you drill in from
  there. Dark.
- **C — One List.** Minimal chrome and a single dense list of every task, with a Group by switch (Status / Goal /
  Project). Goal groups carry their own progress header, so grouping by goal *is* the goals overview. ⌘K is the main
  way to move. Light, dense, monochrome with status colour only.

Each concept is drawn at 1440×900 as three screens: home, the goals overview, and finding a task.

## Direction picked (2026-10-14)

Morgan picked **D**, which builds on A2: the sidebar with search, the project filter, and goals as a list. Each goal
row expands in place to show its tasks grouped by wave, and the full goal page is still its own screen.
**E** (list plus peek panel) was turned down because it made it look as if a goal lives in two places.
