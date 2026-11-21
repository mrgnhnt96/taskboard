# Taskboard Mac app: feature inventory

This is the complete list of what the owner can see and do in the Taskboard Mac app (`crates/taskboard-app`). It is the only input for a from-scratch redesign. Each feature is described by what it is for, the data it shows, the actions it offers, its states and edge cases, and how often it is used.

It says nothing about how the app looks today, on purpose. Layout, placement, colours, widget types and control labels are left for the design to decide. When a sentence is quoted, the quote is the information the app has to get across, not wording the design has to keep.

## How to read this

- **Entry shape.** Each feature has some of these parts: Purpose, Data shown, Actions, States and edge cases, Frequency, Source.
- **Frequency scale:**
  - **constantly:** in view or in use all the time.
  - **daily.**
  - **weekly.**
  - **rarely:** a few times a month or less.
  - **setup-only:** once per install, account or project.
- **Behaviour words name a behaviour, not a widget:**
  - "On hover" and "hover text" mean secondary detail the owner can ask for.
  - "Toast" means a short-lived message confirming a result, sometimes offering undo.
  - "Dialog" or "confirm" means the app stops and asks before it acts.
  - "Menu" or "picker" means choosing one of several options.
  - "Banner" means a notice that stays until its problem is gone or it is dismissed.
  - "Panel" means a task or backlog issue shown in depth without leaving the current page.
  - The redesign is free to deliver each of these however it likes.
- **Refs.** Refs such as T12, G3, B40, I7 and R2 are links wherever they appear. The owner also copies and pastes them into terminals. Every object needs its ref visible and copyable.
- **Endpoints.** Paths such as `POST tasks/{id}/start` are daemon API routes under `/tasks/api`. They are listed so the redesign keeps the same capabilities. `docs/API.md` documents all of them.
- **Source.** Paths are relative to `crates/taskboard-app/src/` unless noted.
- **Text.** All times are shown in 12-hour form with AM/PM. The app never shows 24-hour time.

## What the app is, in one paragraph

The board is a background service (the daemon) that plans and runs Claude Code agents on the owner's projects. Agents work in terminals inside Midna, a separate terminal app. The Mac app is a window onto the daemon and is mostly read-only. It changes the board only through a short list of owner actions:
- Start, stop, schedule, pause, resume and continue work.
- Answer questions, and answer or settle human tasks.
- Triage backlog issues.
- Write and move ideas.
- Undo.
- A few settings: accounts, work hours, the usage limit, history, QA, worktree pools.

Everything else is done by agents or with the `tb` command line. Section 14 lists what the daemon can do that the app only shows, or doesn't reach at all.

## Areas

1. App shell and navigation
2. Health, setup and connection
3. Needs you (the home)
4. One task and its pull request
5. Backlog
6. Goals
7. Ideas
8. Projects
9. Releases
10. Days, weekly review and recaps
11. Settings
12. Shared behaviours (toasts and undo, status words, accessibility, formatted text, saved view state)
13. Keyboard shortcuts
14. Daemon API cross-check (features reached only through odd paths, or not at all)
15. Sample data (the fake backend), and what it covers

## Glossary

The board's words, for a designer who has never seen the app. Refs are short ids the owner and agents use everywhere: they are typed, copied, searched and linked, so every object needs its ref visible and copyable.

### People and places

- **Owner.** The one person the board works for. Agents ask the owner questions, the owner starts work, answers, reviews and approves. The app speaks to the owner as "you". There is no multi-user model.
- **Agent.** A Claude Code session doing a task. It reports to the board through the `tb` command-line tool and through hooks that fire on its turns.
- **Terminal (session).** A terminal window in Midna, the AI-managed terminal app, usually running one agent. The board syncs every terminal from Midna, whether or not it holds a task. States: working (busy on a turn), needs (waiting at its prompt for a person), idle, gone (closed). A terminal can also be *lost*: it closed before its task was done.
- **Midna.** The separate terminal app the board drives: it opens, focuses, renames, types into and closes terminals for the board. When Midna is down the board can't start or reach agents.
- **Daemon.** The background service that owns all board data and runs the runner; it keeps working while the app window is closed. The app is only a view on it, and the app has to cope with the daemon being unreachable, restarting or a different version.
- **Project.** A code repository the board knows about. Tasks, goals, backlog items and releases belong to a project; a goal can hold tasks in several projects. Each project has its own rules (whether work ends in PRs, how many approvals, which checks must pass, whether it cuts releases, which projects it may wait on).

### Work

- **Task (T<n>).** One piece of work for one agent, normally ending in one pull request. Statuses: **planned** (on the board, not asked to run), **queued** (will start when everything lets it), **working** (an agent has it), **needs** (waiting on the owner: a question, a failure, a lost terminal, a review), **done**, **failed**. A task also carries: its project, goal and wave, priority (normal or high), a detail (what to do), its log, checkpoints and notes, its PR and the PR's stage, its terminal, its worktree slot, devices lent to it, the tasks it waits for, its locks, its Jira ticket, its feature flags (bits) and attachments.
- **Pickup.** How a queued task gets a terminal: *queue* (once the project's busy terminals settle), *new* (a fresh terminal right away), *attach* (in a chosen idle terminal) or *manual* (only when the owner presses start).
- **Handoff.** Everything a fresh terminal needs to pick a task up: the task, its goal, lessons that fit, answers, devices and where to work. It is the agent's first prompt; when a terminal is lost, the next one starts from it.
- **Checkpoint.** An agent's milestone report: what's done, what's next, decisions made. **Note:** a free-form progress line in the task log (or the goal's notes).
- **Question.** An agent asks the owner something and the task waits (status needs). Questions can be screened first: a headless Claude decides whether the owner's own written rules already answer it. Some questions are pickers with numbered options.
- **Human task (H<n>).** Work only the owner can do (sign in somewhere, buy something, change a setting behind their login). It takes no terminal and no PR; it reaches the owner when its turn in the goal comes, and is done when the owner says so. The owner can also say they can't do it, that they're waiting on someone until a date, that it isn't needed, or that dependent work shouldn't wait.
- **Check-in.** Every 20 minutes of a long turn, a small model reads the terminal and judges whether the agent is still getting somewhere. A stuck verdict marks the task "maybe stuck", with why and the last lines.
- **Waits-for.** A task that needs another task's work first waits for it (possibly across projects, when the projects allow it). **Stack:** a task whose PR builds on another task's unmerged PR; *head* stacks merge into the parent's branch, *tail* stacks go to main after the parent merges. **Same PR:** several tasks sharing one branch and PR, one after another.
- **Lock.** A named resource (an emulator, a local server) that only one task may hold at a time; others wait for it. **Alone:** a task that runs with nothing else in its goal (or on the whole board) running.
- **Lane / slot.** Where a task works: the project's *shared* checkout, a *slot* (a git worktree borrowed from the board's pool, so parallel tasks don't collide), *alone*, or *auto* (the board picks). Slots go back to the pool when the PR opens or the task ends.
- **Device.** An emulator, simulator or phone in the board's shared pool, asked for by tag, lent to a task while it runs, with optional start/stop/focus commands. A goal can reserve devices.
- **Bit.** A feature flag a goal's work hides behind. *Local* bits live only in code; *backend* bits must also be created in a flag tool, and a finished goal waits until the owner marks them made.

### Groups and order

- **Goal (G<n>).** An ordered group of tasks with an outcome. It has a name, a TLDR for the owner (two to four plain sentences), a *done-when* (the testable outcome agents work to), a project (or several), notes, attachments, a setup line every task runs first, a terminal limit (how many of its tasks run at once, default 2), and flags: paused, deprioritized, set aside, archived. A goal can follow other goals (it starts by itself once they finish), and a task can count toward a second goal ("also finishes"). A goal finishes when every task is done and every PR merged; it can be held open by a *miss* (an agent reported that the done-when isn't met), unmade backend bits or failed tasks. After it finishes there is a retrospective that may suggest lessons.
- **Wave.** A numbered step inside a goal. A wave's tasks run side by side (up to the terminal limit); the next wave starts once the one before has nothing left that can start. Tasks with no wave run last ("post"). A wave can have a name, an owner's **review stop** after it (the goal stops for the owner to look before continuing; only the owner sets one, only from the app; the app has no control for it yet, see section 14), or a **hold** (an agent holds it until something outside the board is ready).
- **Run.** Starting a goal: its planned tasks are queued and run wave by wave. Only the owner runs a goal or starts a task. A run can be scheduled for a time, or forced outside work hours ("now").
- **Plan review.** When an agent plans or re-plans a goal, the plan (its tasks, waves and the plan checker's warnings) can wait for the owner's review before it runs.

### Found work

- **Backlog item / issue (B<n>).** Something an agent found outside its task: a *bug*, a *gap*, a *follow*-up or a *clean*-up, tied to a project and often to a goal. States: open, then turned into a task, a Jira ticket, closed as fixed (by a task or PR), or dropped as won't do; a closed item can be reopened. Each has a history log. A periodic tidy closes repeats and false findings, saying why.
- **Idea (I<n>).** Something the owner wants to think about before it becomes work. The owner writes it; an agent can *dig* into it (research), *talk* it over, and *plan* it into a goal. Ideas can be shelved, marked won't do, closed and reopened, and can wait on the owner's answer.
- **Lesson (L<n>).** What finished work taught, in a sentence or two, for one project or the whole board; kinds: planning, code, process, tooling. States: suggested, kept (by the owner, or automatically when two goals confirm it), retired. Kept lessons reach later handoffs and planners.
- **Miss.** An agent's report that the goal's done-when isn't met. It goes to the owner, not the backlog, and the goal stays open until the owner settles it (for example "finish without it").

### Pull requests and shipping

- **PR and its stage.** A task's pull request on GitHub, GitLab or Bitbucket. The board watches GitHub PRs and gives each a stage: *checks* (CI running), *fix* (checks failed), *review* (waiting for reviewers, possibly the owner's own review first), *comments* (reviewers left comments to answer), *ask* (waiting to ask reviewers), *merge* (approved and green), *merged*, *declined*. Failed checks and new comments bring the agent's conversation back to fix them.
- **Release (R<n>).** A named cut (usually a version) of a project's done, merged tasks, for projects with releases on. Done work waits in **Unreleased** until the next cut; a cut can run the project's release script and can be pending until named tasks and goals finish.
- **Master break (M<n>).** The watched default branch went red. The board judges whether it's the board's fault (ours / not ours / unsure) and may make a fix task.
- **Jira.** Optional link to a Jira site: tickets for tasks and backlog items, epics for goals, statuses moved as work progresses. **QA comment (Q<n>):** a tester's Jira comment, turned into a task, a flag or ignored.

### Time and capacity

- **Work hours.** The days and times agents may start; outside them nothing new starts and idle task terminals close. The owner can extend today ("until 6am", "for 8h") and the Mac is kept awake during them.
- **Usage.** Claude's 5-hour and weekly usage. When it runs out, or today's share is used, nothing new starts until it resets; the owner can lift today's limit.
- **Terminal cap / pool.** How many task terminals run across the whole board (default 8); tasks over it wait for a free one.
- **Needs you.** Everything waiting on the owner, in one list: questions, failures, lost terminals, review stops, PRs to review or merge, human tasks, misses, plan reviews, alerts. Items can be answered, dismissed, snoozed or set aside.
- **Alert.** A board-level message the owner must see that isn't a task question (a watcher found main red, a task has been quiet too long). Urgent alerts repeat outside work hours and can't be dismissed.
- **Day / recap / weekly review.** The history views: what happened on a day (tasks, commits, PRs, questions, terminals running), weekly comparisons, how long work took and waited on the owner, and hours saved against agents' estimates.
- **Job (J<n>).** An internal instruction from the board to Midna (open a terminal, type a message, close, focus, rename). Mostly invisible; shown when something is stuck.

## 1. App shell and navigation

### App lifecycle and window

#### Launch and install
- **Purpose:** Get the board running without any setup steps for the owner.
- **Data shown:** None directly. Problems show up in the health summary (see Health summary and problem notices).
- **Actions:**
  - On launch, the installed app does three things without being asked:
    - It registers the daemon's login item (LaunchAgent).
    - It links the `tb` command into the PATH.
    - It starts the updater, which checks GitHub for newer releases now and then.
  - The fake backend runs the real board code in-process on sample data. It installs nothing.
- **States and edge cases:**
  - Dev builds, and `TASKBOARD_DEV=1`, skip the install and the login item.
  - If install fails, it surfaces as a health item (couldn't link `tb`, login item failed, and so on).
  - `TASKBOARD_NO_ACTIVATE` opens the window without taking focus.
- **Frequency:** setup-only. It happens once per install or update and is invisible otherwise.
- **Source:** main.rs:132-135, main.rs:64-73.

#### Main window
- **Purpose:** The single window that holds every page.
- **Data shown:**
  - The window title is "{page} — Task board", prefixed by "(n)" when n things need the owner.
  - On a goal or idea page, the title uses that goal's or idea's title.
- **Actions:**
  - Close window. Closing the last window quits the app; the board keeps running in the daemon.
  - Minimize, Zoom, Bring all to front.
  - Hide, Hide others, Show all.
  - Quit.
- **States and edge cases:**
  - Default size is about 1360×860. The minimum is 900×560.
  - The size can be overridden by environment variables.
  - Follows the system's light/dark mode; `TASKBOARD_THEME` overrides it.
- **Frequency:** constantly. It is the app.
- **Source:** main.rs:137-159, app.rs (page_title), menu_bar.rs (Window menu).

#### Polling and freshness
- **Purpose:** Keep what is shown current without the owner refreshing.
- **Data shown:** Indirect: everything on screen.
- **Actions:**
  - Manual refresh (⌘R, and in the View menu).
  - Polling runs on its own:
    - Every 3 s while the window is frontmost.
    - Every 15 s while another app is in front.
    - Every 30 s while the window is not visible.
    - At once when the window comes back.
- **States and edge cases:**
  - The daemon can answer "unchanged".
  - A late or stale answer is dropped.
  - Any change the owner posts invalidates a refresh already in flight, so old data never overwrites a fresh change.
  - Hooks state and login-item state are rechecked on every poll.
- **Frequency:** constantly. Background work.
- **Source:** app.rs (poll / refresh / generation logic).

#### Relaunch to update
- **Purpose:** Install a downloaded update without losing anything the owner was typing.
- **Data shown:**
  - After an update is downloaded, a health item says the new version is ready.
  - It lists any code reviews that a restart would restart. This list comes from the daemon (`GET /restart`).
  - After relaunching, the app says it was updated.
- **Actions:**
  - Restart to update.
  - See what's new.
  - Check for updates (installed bundle only).
- **States and edge cases:**
  - Typed drafts are saved before quitting and restored after the relaunch.
  - If the update fails, a health item offers to try again, copy the details, or dismiss it.
- **Frequency:** weekly. Updates arrive with releases.
- **Source:** main.rs:173-179, relaunch.rs, ui/health.rs, menu_bar.rs.

#### Export for debugging
- **Purpose:** Hand a redacted snapshot of the board to someone debugging it.
- **Data shown:** None in the app.
- **Actions:**
  - Writes a redacted zip to Downloads and reveals it in Finder.
  - It is the same zip as `tb export`.
- **States and edge cases:**
  - On failure, a system alert says the export failed and gives the error.
  - Not offered on the fake backend.
- **Frequency:** rarely. Only when reporting a problem.
- **Source:** main.rs:49-62, menu_bar.rs (Help menu).

#### Open log
- **Purpose:** See the app's own log.
- **Actions:** Opens the app's log file in Console.
- **Frequency:** rarely.
- **Source:** menu_bar.rs (Help menu).

#### Restart daemon
- **Purpose:** Recover a stuck board without using the terminal.
- **Actions:** Restarts the daemon. A toast confirms it, or an error toast explains why it failed.
- **States and edge cases:**
  - Installed bundle only.
  - Unavailable while a restart is already running.
- **Frequency:** rarely.
- **Source:** menu_bar.rs, app.rs.

#### Install or reinstall hooks
- **Purpose:** Make new Claude sessions report to the board.
- **Data shown:** Hooks state: missing, out of date, installing, or failed.
- **Actions:**
  - Install hooks, or reinstall them when they are out of date.
  - Reachable from the app menu and from the health summary.
  - Success toast: new sessions now report to the board.
- **States and edge cases:**
  - Offered only when hooks are missing or stale, this copy of the app can install them, and no install is running.
  - If this copy can't install them, the item says to install them from the installed app.
  - On failure: try again, copy details, or dismiss.
- **Frequency:** setup-only.
- **Source:** menu_bar.rs, app.rs, ui/health.rs, hooks.rs.

#### Developer and sample-data switches (environment only)
- **Purpose:** Run the app on sample data for demos, screenshots and tests.
- **Data shown:** A health item marks the board as a sample board.
- **Actions:** None in the UI. These are set by environment variables:
  - Starting page and panel: `TASKBOARD_PAGE`, `TASKBOARD_OPEN`.
  - A link to follow at startup: `TASKBOARD_LINK`.
  - Pre-checked backlog issues: `TASKBOARD_PICK`.
  - Sample-data flags: releases, stopped tasks, needs-you variety, goal stops, gone quiet, recaps, Jira set up, and a pre-planned goal.
- **Frequency:** setup-only. Dev and demo only.
- **Source:** main.rs:1-25.

### Navigation and history

#### Pages
- **Purpose:** The top-level places in the app.
- **Data shown:** The pages are:
  - Needs you (the home).
  - Projects, and one project's page.
  - Goals, and one goal's page.
  - Backlog.
  - Ideas, and one idea's page.
  - Days, with a weekly review page and a recap page under it.
  - Releases, and one project's releases.
- **Actions:**
  - Go to a page from the page list, the View menu, ⌘1–⌘7, the search palette or a link.
  - The current page is marked in the page list and in the View menu.
- **States and edge cases:**
  - Releases is reachable only while at least one project has releases turned on.
  - Opening Releases while they are off shows an explanation and the command that turns them on per project.
- **Frequency:** constantly.
- **Source:** app.rs (Page enum, go), ui/sidebar.rs, menu_bar.rs.

#### Back and forward history
- **Purpose:** Retrace steps the way a browser does.
- **Data shown:** None. Menu items are live only when there is somewhere to go.
- **Actions:**
  - Back: ⌘[ or ⌘←. Forward: ⌘] or ⌘→.
  - Both are also in the View menu.
- **States and edge cases:**
  - Each history entry is a page plus the task open in the panel on that page.
  - Closing the task panel counts as a step back.
  - A new visit clears forward history.
  - Releases is skipped while releases are off.
  - Inside a text field, ⌘← and ⌘→ move the cursor instead.
  - The last 8 goal pages are cached so going back is instant.
- **Frequency:** daily.
- **Source:** app.rs (history).

#### Return to the previous task
- **Purpose:** After following a link from one task to another, get back to the first.
- **Actions:** Reopens the task that was open before.
- **States and edge cases:** Offered only when the panel was showing a different task before.
- **Frequency:** daily.
- **Source:** app.rs.

#### Task side panel (shell behaviour only)
- **Purpose:** Show one task in depth without leaving the page. Content belongs to the task panel inventory.
- **Data shown:** The task, in four sections: overview, what, context, and log.
- **Actions:**
  - Open it from any task reference.
  - Close it with Esc.
  - Switch between its sections.
- **States and edge cases:**
  - With room to spare, the page stays fully usable while the task is open.
  - In a small window the task takes priority, and the page underneath still stays usable.
  - It stays open across page changes and remembers which section was showing.
- **Frequency:** constantly.
- **Source:** app.rs (Panel).

#### Issue (backlog item) side panel (shell behaviour only)
- **Purpose:** Show one backlog issue.
- **Actions:** Close it with Esc, or by clicking outside it when the window is narrow.
- **States and edge cases:** In a small window the task takes priority over the page.
- **Frequency:** daily.
- **Source:** app.rs.

#### Esc: close the top layer
- **Purpose:** One key to back out of whatever is on top.
- **Actions:** Esc closes the first of these that is open:
  1. A picker or menu.
  2. A dialog.
  3. A secondary detail opened inside the task.
  4. The side panel.
  5. The Backlog's checked issues.
  6. On Needs you with nothing focused, the project and goal filters.
- **States and edge cases:** While a text field has focus, Esc belongs to the field (it cancels the field's form).
- **Frequency:** constantly.
- **Source:** app.rs.

#### Filters (project, goal, done window)
- **Purpose:** Narrow pages to one project or goal, and choose how much finished work to show.
- **Data shown:**
  - The chosen project or goal.
  - The project list with a count for each project.
  - The done window: 24 hours, 7 days, or everything.
- **Actions:**
  - Choose a project from a searchable list, or choose all projects.
  - Choose a goal from a searchable list (by name, project or epic).
  - Choose the done window.
- **States and edge cases:**
  - Saved to preferences. Project and goal reset on every launch; the done window persists.
  - A search with no match says nothing matches what was typed.
  - Choosing the value already chosen does nothing.
  - Closing the list gives focus back to whatever had it before.
  - An issue the owner just changed stays visible even if the filter would hide it.
- **Frequency:** daily.
- **Source:** app.rs, ui/board.rs, ui/issue_panel.rs:940-1110.

#### Copy reference
- **Purpose:** Put the open thing's reference (T#, B# or G#) on the clipboard to paste into a session.
- **Actions:**
  - ⌘⇧C, or from the Edit menu.
  - A toast confirms which reference was copied.
- **States and edge cases:**
  - An open task or issue panel wins over the goal page under it.
  - On other pages there is nothing to copy, so it does nothing.
- **Frequency:** daily.
- **Source:** app.rs (open_ref, copy_ref).

### Menu bar

#### App menu
- **Purpose:** App-level commands.
- **Actions:**
  - About.
  - Check for updates (installed only).
  - Settings (⌘,).
  - Install hooks (only when needed).
  - Restart daemon (installed only, and not while busy).
  - Hide (⌘H), Hide others (⌘⌥H), Show all.
  - Quit (⌘Q).
- **Frequency:** rarely, apart from Settings and Quit.
- **Source:** menu_bar.rs.

#### File menu
- **Actions:**
  - New idea (⌘N).
  - Close window (⌘W).
- **States and edge cases:**
  - ⌘N while the new-idea dialog is already open keeps the draft and returns focus to its title.
  - A kept draft can be reopened later with ⌘N.
- **Frequency:** daily.
- **Source:** menu_bar.rs.

#### Edit menu
- **Actions:**
  - Undo (⌘Z): live only while a toast offers an undo.
  - Cut, Copy, Paste and Select all, for text fields and selected text.
  - Copy reference (⌘⇧C).
- **States and edge cases:** Inside a text field, ⌘Z does nothing, but the menu item still works.
- **Frequency:** daily.
- **Source:** menu_bar.rs.

#### View menu
- **Actions:**
  - Each page in page-list order (⌘1–⌘7), with the current one checked.
  - Search (⌘K).
  - Back and Forward.
  - Refresh (⌘R).
- **Frequency:** daily.
- **Source:** menu_bar.rs.

#### Task menu (acts on the task open in the panel)
- **Actions:**
  - Focus its terminal in Midna:
    - Needs a live session.
    - Toast when Midna shows it.
    - If Midna isn't running, it says Midna can't show the terminal.
  - Open its pull request (⌘↩): needs a PR.
  - Start it:
    - Only for a queued or planned task that is not in a goal, not already starting and has no start retry pending.
    - Toast when it is starting.
  - Stop it (close its terminal):
    - Needs a session.
    - If the terminal is busy, it first asks whether to force-close it.
    - Toast when the terminal is closing.
  - Mark it done: opens the finish confirmation. Not offered once the task is done or cancelled.
- **States and edge cases:**
  - Every item is unavailable when no task is open.
  - Items that change the board are unavailable while the board is down.
- **Frequency:** daily.
- **Source:** menu_bar.rs.

#### Idea menu (acts on the open idea)
- **Actions:**
  - Dig (⌘D).
  - Plan (⌘⇧P).
  - Talk it over (⌘T).
  - Close the idea, as either "later" (shelve) or "won't do".
  - Reopen.
- **States and edge cases:** Each item is live only when the idea's state allows it. Details belong to the ideas inventory.
- **Frequency:** weekly.
- **Source:** menu_bar.rs.

#### Window menu
- **Actions:**
  - Minimize (⌘M), Zoom, Bring all to front.
  - The main window entry, named after the current page.
- **Frequency:** rarely.
- **Source:** menu_bar.rs.

#### Help menu
- **Actions:**
  - Open the log.
  - Export for debugging (not on the sample board).
- **Frequency:** rarely.
- **Source:** menu_bar.rs.

#### Dialog suspends page keys
- **Purpose:** Keep keys from acting on the page behind an open dialog.
- **States and edge cases:**
  - While a dialog is open, page shortcuts, search and new idea do nothing.
  - The exception is ⌘N in the new-idea dialog (see File menu).
- **Source:** menu_bar.rs, app.rs.

### Page list and quick links (always available)

#### Page list with counts
- **Purpose:** Move between pages and see at a glance where attention is needed.
- **Data shown:**
  - Needs you: how many things wait on the owner. Flagged when above 0.
  - Projects: how many projects.
  - Goals: how many goals are active, plus how many are stopped.
  - Backlog: how many items are untriaged.
  - Ideas: no count.
  - Days: how many recaps are unseen. Flagged when above 0.
  - Releases: only present when some project has releases on.
- **Actions:**
  - Click to go to a page.
  - The current page is marked.
  - A goal page counts as Goals, an idea page as Ideas, and review and recap pages as Days.
- **Frequency:** constantly.
- **Source:** ui/sidebar.rs.

#### Search entry
- **Purpose:** A visible way into the search palette.
- **Actions:** Opens the palette. Shows the ⌘K hint.
- **Frequency:** daily.
- **Source:** ui/sidebar.rs.

#### Active goals list
- **Purpose:** Keep running goals in view from every page.
- **Data shown:** For each goal in progress:
  - Its reference and name.
  - Its stop kind, if any (see Goal stop states).
  - Its progress split by task status.
  - A progress line, one of:
    - "x of y" done.
    - Paused, plus "x of y".
    - Starts after another goal.
    - The count plus the stop's short label.
  - Time since its last update. Hovering shows the exact time.
- **Actions:**
  - Click to open the goal.
  - The open goal is marked.
- **States and edge cases:**
  - Order:
    - Stopped goals come first, worst stop first.
    - Then the rest, newest first.
  - A goal is flagged as stale when:
    - It is running and has had no update for over 30 minutes.
    - It is waiting and has had no update for over 120 minutes.
    - A goal waiting on the owner is never flagged as stale.
  - The list is empty when no goals are active.
- **Frequency:** constantly.
- **Source:** ui/sidebar.rs, ui/stop.rs.

#### Window drag area
- **Purpose:** Move the window. Any empty part of the window's top edge drags it.
- **Frequency:** rarely.
- **Source:** ui/sidebar.rs.

### Search palette

#### Global search (⌘K)
- **Purpose:** Find anything on the board by reference or words, and jump straight to it.
- **Data shown:** Results in groups:
  - Projects. Shown only once text is typed, and put first.
  - Tasks.
  - Goals.
  - Ideas.
  - Backlog issues.
  - Attachments.
  - Terminals.
- **Group sizes:**
  - Each group shows up to 5 results, with a way to show all N.
  - The total count comes from the server.
- **What each result shows:**
  - Its reference, title and status.
  - Where it lives:
    - A task: its goal, its wave within the goal when scoped, or its project.
    - A goal: "x of y" done, or its project. Its status is one of done, deprioritized, paused, needs you, working, up next, queued or in progress.
    - An idea: its state.
    - An issue: its state or kind, and its goal or project.
    - An attachment: its kind, and its task or goal.
    - A terminal: needs you, working, or how long it has been idle; and its task.
  - If the match was only in deeper text, it says so: matched in the description, or matched in details.
  - The words that matched are marked.
- **Empty query:** Shows open tasks, pinned goals (active, next and paused), and all terminals.
- **Actions:**
  - Type to search. The search runs about 120 ms after typing pauses.
  - Move the selection with up and down; it wraps. Hovering a result selects it.
  - Enter opens the selected result:
    - A project opens its page.
    - A task opens in the panel.
    - A goal opens its page.
    - An idea opens its page.
    - An issue opens in the panel.
    - An attachment opens.
    - A terminal is shown in Midna.
  - ⌘↩ opens a task's pull request.
  - Show all of one kind of result.
  - Esc or a click outside closes the palette.
- **States and edge cases:**
  - Typing a full reference puts that kind's group first.
  - Opened from a goal page, it searches only that goal's tasks, issues and attachments. To widen the search to everything:
    - Choose the explicit widen-the-search result.
    - Or remove the scope.
    - Or press Backspace in the empty field.
  - While the search is running, it says it is searching.
  - No results: it says nothing matches, naming the scope. When scoped, it offers to search everything.
  - When Midna is down, the terminals group is replaced by a line saying terminals can't be shown.
  - When the board is down, it says the board can't be reached and when it stopped answering, with a way to retry.
- **Frequency:** daily.
- **Source:** ui/search.rs.

### Deep links

#### taskboard:// links
- **Purpose:** Let notifications, other apps and the CLI open a precise place in the app.
- **Data shown:** None.
- **Actions:** Each link opens one place:
  - A task, which can also open its finish or cancel confirmation.
  - An issue.
  - A goal, which can open on the goal's backlog view.
  - The goals list.
  - An idea, or the ideas list.
  - A session.
  - The backlog or plan.
  - Days.
  - A review.
  - A recap.
  - Projects, or one project.
  - A release.
  - Anything else opens Needs you.
- **States and edge cases:**
  - Links that arrive before the window exists are queued, then followed.
  - The app comes to the front.
  - Arriving by link closes any open dialog. If that dialog was a new-idea draft with text, the draft is kept, and a toast offers to reopen it with ⌘N.
  - Web-style task query links also work.
- **Frequency:** daily. Notifications use them.
- **Source:** app.rs (arrive), main.rs:160-172.

## 2. Health, setup and connection

### Health summary and problem notices

#### Health summary
- **Purpose:** One place that says whether the machinery (daemon, Midna, hooks, updates, login item, PR feed, accounts) is healthy, and offers the fix.
- **Data shown:**
  - The worst current problem's title, plus "+N more".
  - When everything is clear, whether Midna is running, or else that the board is running.
  - Hovering lists every problem's title.
  - It also carries the streams indicator (see PR feed and streams).
- **Actions:** Opens the list of every item that needs a look. Each item has its own actions.
  - **Problems (worst):**
    - The board isn't answering: try again, copy details.
    - Midna isn't running: shows when it was last heard from.
    - Hooks install failed: try again, copy details, dismiss.
    - Update failed: try again, copy details, dismiss.
    - Couldn't link the `tb` command: copy details, dismiss.
    - Login item failed: try again, copy details.
  - **Needs action:**
    - PR builds stopped: who stopped them, when, and why.
    - PR feed stuck or silent.
    - Login item needs switching on: opens the system's Login Items settings.
    - Hooks aren't installed: install them.
    - Hooks are out of date: reinstall them. If this copy of the app can't install, the item says to install from the installed app.
    - Accounts:
      - One account needs signing in again, or its sign-in needs checking.
      - Or "N accounts need you".
      - Opens Settings.
  - **Information:**
    - Hooks installing.
    - A new version is ready, with what a restart would restart: restart to update, see what's new.
    - Sample board.
- **States and edge cases:**
  - While running, "try again" shows that it is working, and a toast reports the result.
  - Dismissing a failure forgets it. The item returns only if that step fails again, for example on the next install or update attempt.
- **Frequency:** daily for a glance. Acting on it is rare.
- **Source:** ui/health.rs.

#### Usage readout
- **Purpose:** Show how much of the Claude plan is used, so the owner knows whether agents can keep working.
- **Data shown:**
  - Two windows, 5-hour and 7-day. Each has percent used and when it resets.
  - A level for each window:
    - Ok.
    - Warm at 75% and above.
    - Hot at 90% and above.
    - Lifted when the daily limit is overridden.
  - Today's share left.
  - The time of the reading. After more than 1 hour it is flagged as an old reading.
- **Actions:** Opens Settings at the usage section.
- **Frequency:** daily.
- **Source:** ui/health.rs, app.rs (usage_view).

#### Work hours readout
- **Purpose:** Show when agents are allowed to work.
- **Data shown:** One of:
  - No work hours set.
  - Until a time today, or until a time tomorrow.
  - A start–end range.
  - Off until a given time.
  - Hovering shows the full hours line.
- **Actions:** Opens Settings at the hours section.
- **Frequency:** daily for a glance.
- **Source:** ui/health.rs.

#### Settings entry
- **Purpose:** Open Settings.
- **Actions:**
  - Opens Settings (⌘,). Hovering shows the shortcut.
  - Settings can also open directly on its system, usage or hours section from other entries.
- **Frequency:** weekly.
- **Source:** ui/health.rs, main.rs:98-118.

#### Midna connection
- **Purpose:** Show whether Midna, the terminal app agents run in, is reachable.
- **Data shown:**
  - Connected or not.
  - Hovering shows when it was last heard from.
  - When it is down, hovering also explains that starting, messaging and closing terminals wait until it is back.
- **Frequency:** daily.
- **Source:** app.rs (conn_pill).

#### Board-down banner
- **Purpose:** Say plainly that the daemon isn't answering, and what that means for what is on screen.
- **Data shown:**
  - That the board isn't answering.
  - One of three explanations:
    - What is shown is from a given time, and actions wait until the board is back.
    - What is shown is from before it stopped.
    - There is nothing to show until it answers.
  - The app keeps retrying every few seconds.
- **Actions:**
  - Try again.
  - Open Settings at the system section.
  - Copy details.
- **Frequency:** rarely.
- **Source:** ui/health.rs, app.rs (banner_view).

#### Login item and setup banners
- **Purpose:** Fix problems that stop the daemon from starting at login.
- **Data shown:** One of these situations:
  - The login item needs approval.
  - Another program holds the board's port. It names the program and its process id, says the board can't start until it quits, and that the board keeps trying.
  - Registration failed, with the error.
  - The `tb` link failed (for example, permission denied on the link's folder). Agents' shells then can't find `tb`.
  - Another board is in the way.
- **Actions:**
  - Open the system's Login Items settings.
  - Try again, which shows that it is working and reports the result in a toast.
  - Copy details.
  - Dismiss (for the link failure).
- **Frequency:** setup-only.
- **Source:** ui/health.rs.

#### Master-is-red banner
- **Purpose:** Tell the owner when a project's main branch is failing and the fix is theirs.
- **Data shown:**
  - One banner per open break that is the owner's.
  - Each shows the project, which checks fail, the branch, the commit title, and how long ago it broke.
- **Actions:** Open the fix task, if one exists.
- **States and edge cases:**
  - The matching urgent item on Needs you is hidden while this banner shows it, so the break is never shown twice.
  - Breaks whose cause the board is unsure about are not shown.
- **Frequency:** weekly.
- **Source:** ui/prwatch.rs.

#### PR feed and streams
- **Purpose:** Show whether the board is still hearing about PRs, builds, Jira and QA.
- **Data shown:**
  - PR feed problem:
    - Stuck or silent.
    - Why.
    - The last event.
    - What it is holding up (reviewer asks, nudges, reviewer swaps).
  - PR builds stopped:
    - Who stopped them.
    - When.
    - Why.
  - Streams indicator:
    - Shown only while any source is watched as a stream.
    - Shows the worst source's health.
    - Hovering shows one line per source (PRs, builds, Jira, QA) with:
      - Its mode: board-polled or stream.
      - Its health: healthy, stuck, silent or settling.
      - Its last event.
      - Whether polling is active or resting. Jira is never polled.
      - Why it is failing, if it is.
- **Actions:** None. It is read-only; modes change only through `tb stream mode`.
- **Frequency:** rarely.
- **Source:** ui/prwatch.rs.

#### Alert rows (shared alert behaviour)
- **Purpose:** Daemon alerts about tasks.
  - On the home page they appear as Needs you items.
  - The alerts dialog lists all of them.
- **Data shown:** Each alert's text and its age.
- **Actions:**
  - Open the alert's task, or its goal when there is no task. This first closes the alerts dialog.
  - Fix actions. Each is a named command the alert suggests:
    - It runs as Claude in a new Midna terminal, with a toast confirming.
    - If Midna is down, the command is copied instead.
  - Copy the alert's text.
  - Dismiss it, with no toast.
  - Dismiss every alert that can be dismissed.
- **States and edge cases:**
  - Urgent alerts and PR-review alerts can't be dismissed.
  - Urgent alerts are emphasised over the rest.
  - When several alerts exist, the non-urgent ones collapse into one line counting how many tasks need attention, with a way to show them all.
  - The alerts dialog closes once nothing is left.
- **Frequency:** daily.
- **Source:** app.rs:2101-2400.

### Board-down behaviour (shared)

#### Read-only while the board is down
- **Purpose:** Keep showing the last known data while the daemon is unreachable, and never pretend an action worked.
- **States and edge cases:**
  - Every action that changes the board becomes unavailable. This covers menu items, row actions and sends.
  - Actions that only open something still work.
  - Stats and the waiting line on Needs you are hidden.
  - The page's empty state says the board can't be loaded.
  - The board-down banner shows how old the data is.
  - Search says the board can't be reached, with a way to retry.
  - The app keeps retrying every few seconds.
- **Frequency:** rarely.
- **Source:** app.rs, ui/health.rs, ui/home/view.rs, ui/search.rs.

#### Midna down
- **Purpose:** Agents' terminals live in Midna. Without it, terminal actions can't happen right away.
- **States and edge cases:**
  - Starting, messaging and closing terminals wait until Midna is back.
  - A sent answer is saved and runs once Midna is back. The toast says so.
  - Focusing a terminal says Midna isn't running.
  - Alert fix actions copy the command instead of running it.
  - The search palette's terminals group is replaced by a note.
- **Frequency:** rarely.
- **Source:** app.rs, ui/home/view.rs, ui/search.rs.

## 3. Needs you (the home)

### Page data and ordering
- **Purpose:** The one list of everything waiting on the owner, so agents never sit idle for long.
- **Data shown:**
  - The daemon's home feed, filtered by project.
  - Today's day record, which feeds the stats.
- **Ordering:**
  - Items sort by severity (critical, warning, ok, quiet), then newest first.
- **States and edge cases:**
  - A master-is-red item is hidden when the master banner already shows it.
  - The list is virtualised.
  - The scroll position is kept across refreshes.
- **Frequency:** constantly. It is the home page.
- **Source:** ui/home.rs, ui/home/view.rs.

### Waiting summary line
- **Purpose:** One sentence saying how many agents are blocked on the owner.
- **Data shown:**
  - How many agents are waiting, or that none are.
  - The count includes agents whose items were put off for later.
- **States and edge cases:** Shown only once data has loaded and while the board is up.
- **Frequency:** constantly.
- **Source:** ui/home/groups.rs, ui/home/view.rs.

### Project switch
- **Purpose:** Narrow Needs you to one project.
- **Data shown:** Each project, with the number of items it has waiting.
- **Actions:**
  - Pick a project, or pick all projects.
  - Search within the project list.
- **States and edge cases:**
  - Only offered when there is more than one project.
  - The choice resets on launch.
  - Esc on the page with nothing focused clears it.
- **Frequency:** daily.
- **Source:** ui/home/view.rs, ui/issue_panel.rs:962.

### Talk it over (whole page)
- **Purpose:** Open a Claude conversation about everything that is waiting, scoped to the chosen project.
- **Actions:**
  - Opens a terminal in Midna.
  - If an earlier talk-it-over terminal still exists, it is shown again instead of opening a new one. A toast says which happened.
- **Frequency:** weekly.
- **Source:** ui/home/view.rs.

### Today's stats
- **Purpose:** A daily pulse of output and of how much the owner is blocking.
- **Data shown:**
  - Agent time today.
  - Tasks done today.
  - PRs opened today.
  - Items waiting on the owner. Flagged when above 0.
  - Time saved this week.
- **States and edge cases:**
  - Hidden while the board is down.
  - Numbers count up when they appear, unless reduced motion is on.
- **Frequency:** daily.
- **Source:** ui/home.rs (stats_vm), ui/home/view.rs.

### Empty, loading and down states
- **States and edge cases:** The page shows one of:
  - That the board can't be loaded, while the board is down.
  - That it is loading, before any data has arrived.
  - That nothing needs the owner, when the list is empty.
- **Frequency:** daily. An empty list is the goal state.
- **Source:** ui/home/view.rs.

### Groups
- **Purpose:** Sort waiting items by what kind of response they need.
- **Data shown:** Four groups, in this order, each with a count of its items:
  1. **Agents waiting:** work that has gone quiet or is waiting.
  2. **Decide:** everything else: questions, PRs to review or merge, urgent items, missed outcomes, wave stops and plain alerts.
  3. **Your to-dos:** tasks only the owner can do (human tasks, H#) and the weekly review.
  4. **Stopped:** stopped, terminal lost, couldn't start, failed, looks finished, and questions that already have an answer.
- **States and edge cases:** Only groups that have items appear.
- **Frequency:** constantly.
- **Source:** ui/home/groups.rs.

### Item kinds and their main action
- **Purpose:** Each kind of waiting item, and the one-step response it offers.
- **Data shown and main action, per kind:**
  - **Weekly review:** opens the review. If the review was already opened and fixes remain, opens the Backlog instead.
  - **Gone quiet:** opens the PR, or the task when there is no PR.
  - **Outcome missed** (a goal's done-when wasn't met): opens the issue. Also offers to make it a task or to close it as won't do (see To-do and miss actions).
  - **Holds a PR:** opens the issue. Its fix actions approve or reject. It can't be dismissed.
  - **Urgent:** opens the task.
  - **Looks finished:** opens the task. Also offers to mark it done.
  - **Stopped:** restarts it.
  - **Terminal lost:** restarts it. Also offers to mark it done.
  - **Couldn't start:** tries again.
  - **Failed:** opens the task.
  - **Checks failed, Review the PR, Ready to merge:** opens the PR.
  - **Human task:** marks it done, with a note (see Human to-do).
  - **Question / Needs you:** focuses the agent's terminal. If the terminal is gone, it opens a new terminal to ask again.
  - **A goal's closed terminals:** restarts them. The action restarts one, both, or all, depending on how many.
  - **Wave stop:**
    - Continue lets the next wave start, or lets the goal go on.
    - Hidden while the goal is paused.
- **What the posting actions do:**
  - Restart resumes the task fresh. For a goal, it restarts N tasks.
  - Mark done opens a new terminal that checks the work and marks the task done.
  - Try again starts the task.
  - Focus shows the terminal in Midna.
  - Ask again opens a new terminal.
  - Continue: a toast says that the wave can start, or that the goal goes on.
- **States and edge cases:**
  - Actions that change the board are disabled while the board is down.
  - Actions that only open something still work.
- **Frequency:** constantly.
- **Source:** ui/home.rs (ask_vm, Act), ui/home/groups.rs (finish_vm).

### Rows
- **Purpose:** One item, or one goal's bundle of items.
- **Data shown:**
  - The title, linking to the item's issue, else its task, else its goal.
  - The goal's name, unless the title already contains it.
  - A short tag for kinds that need one:
    - Gone quiet, failed, terminal lost, couldn't start.
    - Checks failed, review the PR, ready to merge.
    - Urgent, looks finished, holds a PR.
    - "1 missed" for a miss that is not inside a goal row.
  - What it holds, if anything: N tasks, or the whole goal.
  - Its age. Quiet rows show no age.
  - T#, G# and B# references inside any text are links.
- **Actions:**
  - Its main action, plus a second action for some kinds (see Item kinds).
  - A menu of more actions.
  - A menu for putting it off until later.
  - Expand or collapse the row's body, if it has one.
- **States and edge cases:**
  - Questions start expanded. The owner can flip any row.
  - Opening a to-do row puts focus in its note.
- **Frequency:** constantly.
- **Source:** ui/home/view.rs, ui/home/groups.rs.

### Goal bundle rows
- **Purpose:** Keep one goal's many waiting items from flooding the list.
- **Data shown:**
  - One row per goal per group. Used when the goal has 2 or more items in that group, or has closed terminals.
  - A count with its meaning: N missed, N stopped, N waiting, N to decide, or N to do.
  - The age of its oldest item.
  - Everything its items hold, combined.
  - It counts as quiet only if every item in it is quiet.
- **Actions:**
  - Each closed terminal is its own child row. Each child can be restarted, can be marked done, and can be put off on its own.
  - Make every miss in it into a task at once. A toast says how many tasks were made.
- **Frequency:** daily.
- **Source:** ui/home/groups.rs, ui/home/view.rs.

### Row bodies
- **Purpose:** Answer or act on an item without leaving Needs you.
- **Data shown and actions, per kind of item:**
  - **Alert:**
    - Its line, text and help.
    - Its fix actions, which run as Claude in Midna (see Alert rows).
  - **Question from an agent:**
    - The question.
    - For each question, its options and their descriptions.
    - Any answers the alert suggests.
    - Actions:
      - Send an answer. It is delivered now. A toast says it went to Midna, or that it was saved and will run once Midna is back.
      - Claude's own multiple-choice questions:
        - A single question with single choice answers in one press.
        - Otherwise choices toggle, and sending waits until every question has a choice.
      - Go to the agent's terminal, or open a new one if it is gone.
  - **Human to-do:** see Human to-do.
- **States and edge cases:**
  - The pressed choice shows that it is sending. Pressing again does nothing.
  - On failure, the choices come back.
- **Frequency:** daily.
- **Source:** ui/home/view.rs, ui/home.rs.

### Human to-do (owner tasks)
- **Purpose:** Let the owner close out, or push back on, work that only a human can do.
- **Data shown:**
  - The to-do's text.
  - A note field. Its purpose changes with the chosen response.
- **Actions:**
  - **Done with a note:**
    - A toast confirms that the task was marked done.
    - Undo reopens it.
  - **Can't do this, with the reason:** tells the agents. A toast confirms.
  - **Not needed, with the reason:** a toast confirms.
  - **Waiting on someone:**
    - Takes who the owner is waiting on, and requires a return date: tomorrow, next week (next Monday), 2 weeks, or 1 month.
    - A toast says it comes back on that day.
  - **Don't wait for me:**
    - The work goes on without the owner. A toast confirms.
    - Undo makes the agents wait again.
  - **Talk it over** in a Claude terminal.
- **States and edge cases:**
  - Sending a waiting response is impossible until a date is picked, and while the board is down.
  - Cancelling an alternative response goes back to the done note.
  - The done note's text is kept aside while another response is being written.
  - Enter submits. Esc cancels.
  - Done is unavailable while another response form is open.
- **Frequency:** daily.
- **Source:** ui/home/groups.rs, ui/home/view.rs.

### Miss actions (goal outcome missed)
- **Purpose:** Decide what to do about a goal outcome that was missed.
- **Actions:**
  - **Make it a task:**
    - It goes onto the board.
    - The toast offers to open the new task.
  - **Make every miss a task** (goal bundle): a toast says how many tasks were made.
  - **Won't do:**
    - Closes the miss.
    - Undo is offered when the server returns one.
  - **Create a ticket:**
    - Asks Jira to make a ticket.
    - Only offered when Jira is set up.
  - **Finish the goal without it:** undo is possible.
- **Frequency:** weekly.
- **Source:** ui/home/groups.rs (more_vm), ui/home/view.rs.

### More-actions menu
- **Purpose:** Less-common responses, kept out of the main path.
- **Actions, by item:**
  - **Human to-do:**
    - Can't do this.
    - Waiting on someone.
    - Don't wait for me.
    - Not needed.
    - Talk it over.
  - **Miss:**
    - Create a ticket (only with Jira).
    - Finish the goal without it.
  - **Stopped item that hasn't failed:**
    - Mark done, behind a confirmation when needed.
    - Cancel the task, behind a confirmation.
  - **Item whose main action is focus, ask again or open PR:** open the task.
  - **Alert:**
    - Talk it over, unless that is already the main action.
    - Dismiss, if it can be dismissed.
  - **Anything else:** talk it over about the task.
  - **Anything in a goal:**
    - Open the goal, unless that is the main action.
    - Pause the goal.
    - Set the goal aside.
  - **Goal bundle row:**
    - Pause the goal.
    - Set the goal aside.
- **What they do:**
  - Talk it over opens a Claude terminal about that task or alert.
  - Dismiss removes the row at once. A toast names what was dismissed, and undo restores it.
  - Pause goal and set aside change the goal's state. Set aside confirms with a toast.
- **Frequency:** weekly.
- **Source:** ui/home/groups.rs (more_vm), ui/home/view.rs.

### Put off until later (snooze)
- **Purpose:** Hide an item until a better time without losing it.
- **Actions:** Choose when it comes back:
  - In 1 hour.
  - In 3 hours.
  - Tomorrow.
  - Next week.
  - Until the owner is back.
  - After another goal finishes. This lists active, unpaused goals other than the row's own, and only appears if any exist.
- **What happens:**
  - Every item in a bundle row is put off together.
  - A toast says when it comes back: a time, a date, when the owner is back, or when the goal finishes.
- **Frequency:** daily.
- **Source:** ui/home/groups.rs, ui/home/view.rs.

### Later list
- **Purpose:** See what was put off, and when it returns.
- **Data shown:**
  - A summary: how many items, when the soonest comes back, and how many agents are waiting on them.
  - When expanded, each item shows its title and when it returns:
    - Back at a time or date.
    - Waiting on a person, and back on a date.
    - After a goal finishes.
    - When the owner is back.
- **Actions:**
  - Expand and collapse.
  - Open an item.
- **States and edge cases:** Filtered by project. Items with no project always show.
- **Frequency:** daily.
- **Source:** ui/home/groups.rs, ui/home/view.rs.

### Ideas waiting on the owner
- **Purpose:** Surface ideas that need an answer or are ready to plan.
- **Data shown:** For each idea:
  - Its reference and title.
  - The verdict on whether it fits.
  - Either how many questions it has, or that it is ready to plan.
  - Its age.
- **Actions:** Opens the idea.
- **States and edge cases:** Filtered by project.
- **Frequency:** weekly.
- **Source:** ui/home/groups.rs, ui/home/view.rs.

## 4. One task and its pull request

General rules across every task action (they apply to every "Actions" list below):
- Every action that changes the board is a request to the background daemon. Each control can have only one request in flight. While it runs, the control shows that it is sending and ignores repeat presses (pinned:).
- The result appears as a short note next to the group of controls it came from. A success note stays about 5 s and an error note about 15 s. A control with no group reports through a toast instead (`ui/task_panel.rs:305-370`).
- When the terminal app (Midna) is down, an action that would type into a terminal says the request was saved and will run once Midna is back. Otherwise it says the request was sent (`ui/task_panel.rs` `sent()`, `ui/task_panel/view.rs` `sent_note`).
- A control stays in its sending state until the board's next refresh shows the change, so its state never flickers back (`settle`).
- Free text the owner typed (answers, notes for agents, start times, choices picked) is kept as a draft. Drafts survive switching to another task and back, and an app restart for an update (`ui/task_panel.rs` drafts / restore_drafts).
- The app never offers a way to mark a task done or failed from its terminal controls. This is deliberate. Finishing comes only through the agent, the human-task flow, or the deep-link confirm described under Dialogs (`ui/task_panel/view.rs:1803` comment).

### Opening and moving around a task

#### Open a task
- **Purpose:** look at one unit of work in full without leaving the current page.
- **Data shown:** the task loads by itself. While its full record is still arriving, the panel shows what the clicked row already knew: title, status and PR progress, plus a loading notice (`ui/task_panel/view.rs:853` card_panel, app.rs `show_task`).
- **Actions / entry points:**
  - any task row or reference anywhere in the app
  - search results
  - notifications, and deep links of the form task/T12
  - review-page rows
  - "Open T…" lines in banners such as the red-default-branch banner
  - issue panels ("open the task it became / that fixed it")
  - blocker and finding links inside another task
  A web-style link with `?task=T6` opens the page and then the task (app.rs `open_link:737`).
- **States:**
  - Loading.
  - Fetch error: the error text is shown, together with whatever was already known.
  - Opening the task that is already open keeps its current view.
  - A different task always starts on the overview (timeline).
- **Frequency:** constantly. This is the main way to inspect work.
- **Source:** app.rs:615-660, 737-790; `ui/task_panel.rs:372`.

#### Stays open while browsing
- **Purpose:** keep a task in view while moving between pages, such as a goal or the board.
- **Behaviour:**
  - Page navigation does not close the task.
  - The page behind stays usable while a task is open. In a small window the task takes priority.
  - Clicking another row swaps which task is shown.
- **Frequency:** constantly.
- **Source:**.

#### Back trail inside a task, and global back/forward
- **Purpose:** follow a chain of tasks (for example, blocker of a blocker) and come back.
- **Data:** the back control names the task it returns to on hover.
- **Actions:**
  - Back returns to the previous task opened from inside this one.
  - Global history: Cmd+[ / Cmd+] or Cmd+Left / Cmd+Right. These walk pages and tasks, and keep the view the task was on.
  - Closing a task is itself a history step, so Back reopens it.
- **States:** no back control when the task was not opened from another task. Closing clears the in-task trail.
- **Frequency:** daily.
- **Source:** `ui/task_panel.rs:372-405`; app.rs:492-560, 1854-1857.

#### Close / Esc layering
- **Purpose:** dismiss things in a predictable order.
- **Behaviour:** Esc closes the topmost open thing first, in this order:
  1. an open picker
  2. an open menu
  3. a dialog
  4. an open reviewer detail inside the task
  5. the task (or issue) itself
  6. the backlog's checked items
  7. the board's goal/project filter
- **Frequency:** constantly.
- **Source:** app.rs:950-975; `ui/task_panel.rs` `escape`.

#### Copy the open task's reference
- **Action:** Cmd+Shift+C copies the open task's ref (for example "T12") and confirms with a toast.
- **Frequency:** daily (pasting refs into terminals and chat).
- **Source:** app.rs:2367.

#### Four views of a task
- **Purpose:** separate the story so far, the brief, what a new terminal would be given, and the raw log.
- **Views:**
  - the timeline (default)
  - the brief, meaning what the task asks for
  - the context / handoff
  - the full log, which shows its entry count
- All four views are always available, whatever the task's state.
- **Frequency:** the timeline constantly; the others daily.
- **Source:** `ui/task_panel/view.rs:865` panel.

### Task identity facts (always visible)

#### Title and where it belongs
- **Data:**
  - the task's title
  - its goal ref and goal name, which open the goal's page
  - the task ref
- **Frequency:** constantly.
- **Source:** `ui/task_panel/view.rs:681` top_row.

#### Status
- **Data:** one status, chosen in this order:
  - Planned, Queued, Blocked (queued but held by blockers), Working, Needs you, Done, or Failed (done and failed).
  - A starting state while a terminal is opening. Its hover text explains what it is waiting for.
  - When the work is done but its PR is not merged yet, the PR's stage replaces the status:
    - checks running
    - fixing
    - rebasing
    - asking the owner
    - in review
    - re-review
    - addressing comments
    - merging
    - merged
    - declined
    Each stage has its own tone. The hover text says the work is done but not merged.
  - When the PR work stopped, the status shows "needs you", and the hover text says whether its terminal asked about the PR or stopped before finishing.
- **Frequency:** constantly.
- **Source:** `ui/task_panel/view.rs:645` status_pill.

#### Kind of task
- **Data:**
  - Human task: only the owner can do it.
  - Design: ends with a design attached, and has no PR.
  - Research: ends with findings attached.
  - Ordinary code tasks show no kind.
- **Frequency:** daily.
- **Source:** `ui/task_panel/view.rs:697`.

#### Project, Jira, goal progress
- **Data:**
  - **Project name.** The project's folder path shows on hover. Opening it goes to the project's page.
  - **Jira key and Jira status.** These link to Jira when a URL is known.
  - **Goal progress.** This shows which wave the task is in and how many of the goal's tasks are done. The next task up shows on hover. Opening it goes to the goal.
- **Frequency:** daily.
- **Source:** `ui/task_panel/view.rs:697-850` head_chips.

#### Attachments
- **Purpose:** reach the designs, proposals, docs, evidence and results that belong to this task or its goal.
- **Data:**
  - A count of attachments.
  - A list that opens from the count. Each row shows the attachment's kind, which task or goal it is on, and its age.
  - Kinds: design, proposal, doc, evidence, results, other.
  - The file type is worked out from the file extension: video, image, audio, PDF, spreadsheet, archive, code or text. It picks the attachment's marker.
  - Each row's summary comes last.
- **Hover detail card:**
  - title and summary
  - kind
  - which task or goal it is on (opens it)
  - the link, which copies on click and confirms
  - file name, folder and type
  - who added it and when (opens the task that added it)
  - The card stays open while the pointer is on it.
- **Actions:**
  - open the attachment (a web URL in the browser, a local path in its default app)
  - copy its link
- **Not reachable:** the task's action handler still has attachment edit (title, URL, kind), save and remove actions (POST `attachments/{id}`, `attachments/{id}/remove`), but no control anywhere in the app reaches them. Today attachments can only be opened from the app (`ui/task_panel.rs:617-690`).
- **Frequency:** weekly (mostly design and research tasks).
- **Source:** `ui/task_panel/view.rs:697-850`; `ui/task_panel.rs:2600-2740`.

### What needs the owner right now (attention banner)

Only one attention block shows at a time. The first matching state in the list below wins. A board alert about this task is added to whichever block shows, or shows on its own if no block applies (`ui/task_panel/view.rs:1626`).

#### Lost or stopped terminal: recover the work
- **Purpose:** the agent's terminal vanished (closed by hand, crashed, or Midna lost it). The owner decides how to carry on.
- **Data:**
  - Which of three cases applies:
    - stopped by hand
    - terminal lost
    - terminal lost, but the work looks finished
  - When it closed.
  - The last update.
  - Anything that looks finished: what it is and when, with links (for example, a PR or commit that shipped since).
  - When the handoff was saved, with how many turns and checkpoints it holds.
- **Actions:**
  - Resume in a new terminal. The new terminal picks up from the handoff.
  - Check and finish. When the work looks finished, this becomes the main action instead of resuming.
  - Reopen the old conversation. Offered only when a previous Claude session id exists.
  - See what the new terminal would get. This switches to the context view.
  - Request: POST `tasks/{id}/resume` with mode fresh, finish or reopen.
- **Frequency:** weekly.
- **Source:** `ui/task_panel/view.rs:1626-1700`.

#### Could not start
- **Data and actions, by case:**
  - **Midna refused the start.**
    - Start it in a new terminal.
    - Or queue it inside Midna.
  - **A start retry is pending.**
    - Shows when it will try again.
    - Try again now.
  - **The conversation could not be compacted.**
    - Start fresh with the handoff.
    - Or reopen the old conversation.
- **Requests:** POST `/start` and POST `/resume`.
- **Frequency:** rarely.
- **Source:** `ui/task_panel/view.rs:1700-1720`.

#### The agent asks the owner something
- **Purpose:** unblock an agent waiting on a decision.
- **Data:**
  - when it asked
  - the question; if there is none, the latest line; if there is nothing, a note that it is waiting in Midna
  - for multiple-choice questions, each option and its description
  - for multi-question pickers, each question's own text
- **Actions:**
  - **Plain question with offered options:** one press sends that option as the answer (POST `/answer`).
  - **Picker with a single single-choice question:** one press answers.
  - **Picker with several questions or multi-select:** toggle choices locally. Sending becomes possible only once every question has at least one pick (POST `/pick` with the chosen indices).
  - **Typed answer:** offered for plain questions, not for pickers.
    - Sending is unavailable while the box is empty; an empty send gives an error asking for an answer first.
    - Cmd+Enter or Ctrl+Enter sends.
    - When work hours are closed, a second send option saves the answer and delivers it when work hours open (POST `/answer` with when = morning).
  - **Step controls**, when the question is about a brief step:
    - open the step's link
    - mark the step done
    - skip a failed step (POST `/step`)
  - **Terminal route:**
    - focus the live terminal in Midna
    - answer in Midna
    - when no terminal is live, answer in a new terminal (resume fresh)
- **States:**
  - A human task shows only the question and its options, and only when there is a real question.
  - The question shows once and is not repeated elsewhere on the task.
- **Frequency:** constantly. This is the core owner loop.
- **Source:** `ui/task_panel/view.rs:1720-1790`, `ask_options`; `ui/task_panel.rs:440-480`, `on_key:2160`.

#### May be stuck
- **Data:**
  - when it was flagged
  - why
  - the terminal's last lines, which can be expanded or collapsed (the state is remembered)
- **Action:** focus the terminal in Midna.
- **Frequency:** weekly.
- **Source:** `ui/task_panel/view.rs:1790`.

#### PR work stopped or asks about the PR
- **Data:** whether the PR work asked a question or simply stopped, and its message.
- **Actions:**
  - type an answer, or pick one of the answer options
  - focus the terminal
- **Frequency:** weekly.
- **Source:** `ui/task_panel/view.rs:1626+`.

#### Waiting on the owner's review
- **Data:** the PR number and title.
- **Actions:**
  - open the PR in the browser
  - mark it reviewed (POST `/pr/reviewed`)
- **Frequency:** daily, when owner review is on.
- **Source:**.

#### Nothing needed from you
- **Purpose:** reassure the owner while the PR is in a stage the agent handles: fixing, rebasing, addressing comments or merging.
- **Data:** the current stage.
- **Frequency:** daily.
- **Source:** `ui/task_panel/view.rs:1626+`.

### The timeline (the task's story)

The timeline runs newest first: the current state first, the start last. Each entry can be opened for more detail when there is more to show; entries with nothing extra cannot be opened (`ui/task_panel/view.rs:1168`).

#### Started
- **Data:**
  - which terminal it runs in
  - how many earlier terminals were closed
  - start time
- **Expanded detail:**
  - Every terminal it has used:
    - the live one with its state: running a background command (named), working, waiting, needs you, offline or idle
    - closed ones with when they closed
  - The branch.
  - Uncommitted changes: a count of files, "nothing uncommitted", or the time of the last commit.
- **Actions:** focus any terminal that is still open in Midna (POST `sessions/{id}/focus`).
- **Frequency:** daily.
- **Source:** `ui/task_panel/view.rs:1200-1285`.

#### Step results (verify steps, tests, checks the agent ran)
- **Data:**
  - step name, headline result and time
  - a flag when the code has moved on since the step ran (the result is stale)
  - a warning marker while findings are still open
- **Expanded detail:**
  - the full note
  - each finding: id, state, severity, title, file and line, note, and a link
  - what it ran on: lent devices and commit sha
- **Actions:** open finding links. A findings link elsewhere jumps here and expands the step results that hold findings.
- **Frequency:** daily.
- **Source:** `ui/task_panel/view.rs:1285-1323`.

#### Code review rounds (automated reviewer before PR)
- **Data, per round:**
  - round number
  - outcome: passed, skipped, N findings, the reviewer did not run, queued, or reviewing
  - progress while triaging or fixing (x of y)
  - When the round cap was hit: a notice that the task went on to its PR with named findings still to fix.
- **Expanded detail:**
  - any error
  - each finding with its triage outcome: to triage, to fix, fixed, sent to backlog, or dismissed. Each also shows severity, file, the linked issue, the reason, what fixed it, and a link.
  - who reviewed and what it cost in dollars
  - the commit
- **Frequency:** daily.
- **Source:** `ui/task_panel/view.rs:1323-1439`.

#### Outcome (done or failed)
- **Data:**
  - The agent's summary. If there is none, its latest line; if there is nothing, a note that no summary was given.
  - The reason there is no PR, and the reason there is no evidence, when either applies. Task refs inside these reasons open the task, and Jira keys link to Jira.
- **Actions:**
  - A failed task can be retried, which puts it back in the queue (POST `/requeue`).
  - When its terminal is still live, a secondary menu offers to close the terminal. See Terminal management.
  - A done task cannot be queued again.
- **Frequency:** daily.
- **Source:** `ui/task_panel/view.rs:1439-1472`.

#### Now (current state)
- **Data, by state:**
  - **Lost, or needs the owner.**
    - A warning that it is waiting on the owner.
    - When something was actually asked, how long the agent worked before asking.
  - **Working.**
    - How long it has run and when it last updated.
    - The latest line.
  - **A terminal opened, but no agent has checked in.**
    - What it is waiting for.
    - A warning when folder trust must be granted in the terminal.
    - Action: start over.
  - **Working outside its project.** Who is doing it, and that the board does not track that work.
  - **Waiting in a line.** The line it is in, and that it starts by itself once the named task finishes or comes next.
  - **Scheduled.**
    - When it starts. This applies whatever the work hours.
    - Actions: start now, or cancel the schedule (schedule "off").
- **Frequency:** constantly.
- **Source:** `ui/task_panel/view.rs:1472-1626`.

#### Start controls (queued or planned task with no goal, not starting, no retry pending)
- **Purpose:** start standalone work by hand.
- **Actions:**
  - **Start now** in a new terminal (POST `/start` with mode new).
  - **Start when the repo is free.** This queues it behind work in the same repo (mode queue).
  - **Start at a typed time,** in natural words like "5pm" or "tomorrow 9am" (POST `/start` with `at`).
    - Empty text gives an error asking when.
    - Success says when it will start.
- **Data:** a warning naming why it is not starting yet. The reason is said only once on the task.
- **Planned task inside a goal:**
  - The only action is to queue it now (POST `tasks/{id}` with status queued).
  - It leaves the plan, and the goal's rules decide when it starts.
- **Frequency:** daily.
- **Source:** `ui/task_panel/view.rs:1520-1600`; `ui/task_panel.rs:420-460`.

#### Blockers
- **Data, for each blocking task:**
  - its ref (opens it), status and title
  - for a blocker that is done but still holds this task, the reason it still holds (each done blocker states its own reason)
  - A blocker still at work shows no reason.
- **Frequency:** daily.
- **Source:** `ui/task_panel/view.rs:1841`.

#### Terminal management
- **Actions:**
  - **Focus the terminal in Midna.** Shown when working, live and not stuck (POST `tasks/{id}/focus`). When Midna is not running, a toast names the task and says it cannot show its terminal.
  - **Detach** (secondary menu). Takes the task off its terminal and queues it again (POST `/detach`).
  - **Close the terminal** (secondary menu). Stops the running command; it is forced unless the terminal is idle (POST `/close-terminal`). A confirm dialog appears first when the task failed or a command is still running (see Confirm closing a terminal).
- **States:**
  - The secondary menu appears only when there is a session id or a live terminal.
  - Only one secondary menu (terminal, attention-block or PR) is open at a time.
  - A click outside closes the menu.
- **Frequency:** daily.
- **Source:** `ui/task_panel/view.rs:1803`; `ui/task_panel.rs:540-580`; app.rs:995-1010.

#### What comes next (before a PR exists)
- **Data:** for a task that will ship a PR but has none yet, the planned path:
  1. author review, when configured
  2. pull request
  3. checks
  4. review
  5. merge
  How it will stack on another PR, if it does.
- **Frequency:** daily.
- **Source:** `ui/task_panel/view.rs:2603`.

### The pull request

#### PR identity
- **Data:**
  - PR number and title, with the Jira key removed from the title
  - repo
  - when the board last read it
- **Actions:** open the PR in the browser.
- **Frequency:** daily.
- **Source:** `ui/task_panel/view.rs:2466`.

#### Pipeline steps (where the PR stands)
- **Data, one status per step:**
  - **Author review.**
    - States: not reviewed by the named reviewer yet, the round's headline (links to its findings), moved since, open findings, passed, failed.
    - Before its first round it says so.
  - **Checks / CI.**
    - States: none, passed, needs the owner (the owner must fix), fixing stopped, the agent is fixing, failed, the agent is next, running, waiting, not this PR's fault, skipped, not needed, none yet.
    - Links to the build.
    - It shows "being fixed" only while a terminal is actually on it.
  - **Owner review.**
    - States: waiting on you, reviewed, skipped, off, after checks.
    - Owner review can be marked done before checks pass.
  - **Peer review.**
    - States: off, N new comments with the agent next (links to the first unread thread), approved, approvals so far out of needed, needs setup, awaiting re-review, changes asked with the agent next, waiting, not asked.
    - Not asked links nowhere.
    - With no approvals count, the host's approval finishes the step.
  - **Pre-merge steps.** Each one: passed, skipped with the reason, or the agent next. They run before the merge.
  - **Merge.**
    - States: waits on another task (links to that base's build or PR), waits on the base, waits on steps, waits on its stack, merging, merged, declined.
- **Frequency:** daily.
- **Source:** `ui/task_panel/view.rs:1934-2129`.

#### Failure cleared as not this PR's fault
- **Data:**
  - title
  - the failing check names
  - the reason
  - proof links
  - when this was checked
- **Frequency:** weekly.
- **Source:** `ui/task_panel/view.rs:2616`.

#### Stacked PRs
- **Data:**
  - whether this PR stacks on another, or another stacks on it, and that task's ref
  - the other PR's number and whether it has merged, or its branch name when there is no PR
  - the merge strategy, head merge or tail merge, with hover explanations
  - a parent PR that is waiting on its stack, and why
- **Frequency:** weekly.
- **Source:** `ui/task_panel/view.rs:2132`.

#### Reviewers
- **Data, per reviewer:**
  - name
  - state: changes asked, approved, re-review waiting, waiting, or commented
  - how long the PR has waited on them
  - how many times the reviewer was swapped
  - whether a re-review is queued, or when it was asked
  - when they were first asked
  - a link to the review
- **New comments:** a count of new comments, shown while changes were asked or a re-review is pending. It links to the first unread thread.
- **Frequency:** daily (in teams with peer review).
- **Source:** `ui/task_panel/view.rs:2172`.

#### Reviewer workload detail (read-only)
- **Purpose:** judge whether a reviewer is overloaded, or whether a swap is coming.
- **Data:**
  - The reviewer's name and login.
  - How many reviews they have open, their typical turnaround, and when they last reviewed.
  - Each open ask:
    - repo#number link and title
    - how long it has waited, in work time; flagged when within 30 min of a swap
    - the task and goal it belongs to (both open)
    - when it was asked, or when changes were addressed
    - notes: when it will swap, that it swaps now, re-review, or "this task"
  - An expandable list of the week's reviews: the answer, how long it took, and when.
- **Actions:**
  - Opening the reviewer's name loads this detail (GET `reviewers/asks?user=`).
  - Pressing the name again, or Esc, closes it.
- **States:** loading; failed to load (says so); otherwise read-only. Times are 12-hour, with the day added when it is not today.
- **Frequency:** weekly.
- **Source:** `ui/task_panel/view.rs:2223`; `ui/task_panel.rs` rev-pop.

#### The terminal doing PR work
- **Data:** which terminal is working on the PR. If none, either that it is waiting for its terminal to open, or the current stage.
- **Action:** focus that terminal.
- **Frequency:** daily.
- **Source:** `ui/task_panel/view.rs:2466+`.

#### Mark reviewed early
- **Purpose:** tell the board the owner has already reviewed, before the board asks.
- **Data:** shown while the PR is not yet reviewed.
- **Action:** mark it reviewed (POST `/pr/reviewed`). A fallback version of this action appears when the board did not send full pipeline data.
- **Frequency:** daily.
- **Source:** `ui/task_panel/view.rs:2466+`.

#### Merge anyway (owner override)
- **Purpose:** merge past checks and reviews when the owner judges it safe.
- **Action:**
  - A secondary PR menu offers to merge now. It always asks first (see Confirm merge without waiting).
  - Request: POST `tasks/{id}/pr/merge` with override set.
- **States:** hidden while the PR is merging, stacking, or rebasing (unless the board gave up), conflicted, or already merged or closed. Shown only on an open PR.
- **Frequency:** rarely.
- **Source:** `ui/task_panel.rs:523`.

#### Final PR state and the note for testers
- **Data:**
  - the final outcome: merged, closed without merging, or pending merge
  - the note for testers: the first three lines as a preview, with label-only lines joined to the value on the line below
  - while the note is pending, either that it is being posted and the ticket will move after, or that the PR merged without one and the ticket is waiting
- **Actions:**
  - **Ask for the note to be written** (POST `tasks/{id}/merged-note/ask`). The reply says either that the conversation is coming back to write it, or that it will come back once it can.
  - **Show the whole note** in a dialog (see The whole note for testers).
- **Frequency:** weekly (teams using Jira / QA flow).
- **Source:** `ui/task_panel/view.rs:1866`; `ui/task_panel.rs:688-700`.

### Human tasks (work only the owner can do)

#### To-do box
- **Purpose:** the owner performs a step that agents cannot, such as account setup or a manual check, and tells waiting agents what happened.
- **Data:**
  - why it is waiting, shown only when queued and not blocked, and said once
  - the steps, as formatted text
  - one quick-open action per web link found in the steps, named by host
  - a waiting-on list: each blocker's ref (opens it), title and PR link, and why a done blocker still holds
- **Actions:**
  - **Write a note for the agents.** The note is identified as what the waiting agents will read.
  - **Mark done** (POST `/did` with the note).
    - Unavailable while blockers remain; the hover text names them.
    - If the note is empty and a step in the brief asks for a note, the first press warns which step wants what, and a second press sends anyway.
    - Success shows a toast with undo (undo is POST `owner-reopen`). Cmd+Z also undoes the newest undoable toast.
  - **Talk it over.** Opens a Claude terminal in Midna to discuss the task (POST `/talk`).
- **States:** while sending, the control reads as sending until the box turns to done-by-you. No note is shown beside it.
- **Frequency:** weekly.
- **Source:** `ui/task_panel/view.rs:1073-1168`; `ui/task_panel.rs:560-604`; app.rs:860-890.

#### Done by you
- **Data:** that the owner did it, and the owner's note.
- **Action:** reopen it, which returns it to needing the owner (POST `/owner-reopen`).
- **Frequency:** rarely.
- **Source:** `ui/task_panel/view.rs:1073`.

### The brief (what the task asks for)
- **Data:** the task's written detail as formatted text:
  - paragraphs and lists
  - code blocks, each copyable
  - links, bold, inline code, and file paths shown as code
  - Empty state: nothing written yet.
- **Actions:**
  - copy a code block
  - open links
  - text is selectable, and Cmd+C copies a selection, including after typing in a field
- **Frequency:** daily.
- **Source:** `ui/task_panel/view.rs:1060`, `note_body:3108`; `ui/task_panel.rs:2135` leaf_pre.

### Context / handoff (what a new terminal would get)
- **Purpose:** see the task's working state, and exactly what a fresh agent would be handed when resuming.
- **Data:**
  - When the handoff was saved.
  - Branch. If unknown: not reported yet, or not started.
  - Worktree, last commit, and uncommitted files.
  - The Claude conversation id, and a note that it can be reopened in a new terminal.
  - Where the task came from: backlog issue, review log, or who added it (linked).
  - Devices: lent device labels, or what devices it needs.
  - Bits: each with local, created or not-created state. Display only.
  - Lists:
    - done so far
    - next
    - decisions
    - the owner's answers
  - Found but not fixed here: links that open the backlog issue, labelled by whether it is in the backlog or the goal's backlog.
  - Files touched: names, with the full path on hover. Empty state: none yet.
  - Details: key/value metadata.
  - The full handoff text. It scrolls when long.
- **Actions:** open the linked origin, open backlog issues, and open the conversation option.
- **States:**
  - The handoff is fetched when this view opens (GET `tasks/{id}/handoff`).
  - It is fetched again when the task moves on: a new context version, an update, a status change, or a new log entry. A stale handoff is marked out of date.
  - Failure shows a could-not-load message with the reason.
- **Frequency:** weekly (mostly before resuming a lost task).
- **Source:** `ui/task_panel/view.rs:2747`; `ui/task_panel.rs:766-825`.

### Full log
- **Purpose:** the complete, permanent record of everything that happened on the task.
- **Data:**
  - **Filters:** everything, status, turns, checkpoints, questions, found, Jira.
  - **Each entry:**
    - time, with the full date and time on hover
    - a marker for its kind
    - who; if none, the board itself
    - kind label and text
  - **Kinds:** status, note, turn, checkpoint, question, answer, jira, found, commit, midna, handoff, checkin, outside.
  - **Durability:** the log is saved as it happens, and survives a closed terminal or a restart.
- **States:**
  - Empty: nothing logged, or nothing of the chosen kind.
  - The newest 200 entries arrive with the task. Older pages load automatically while scrolling (GET `tasks/{id}/log?kind=&limit=200&before=`), and a loading line shows while they do.
  - A failed page fetch stops asking and keeps what is there.
  - Only visible rows are drawn.
- **Frequency:** weekly (deep investigation).
- **Source:** `ui/task_panel.rs:84-256`; `ui/task_panel/view.rs:2835`.

#### Remembered open/closed sections
- The open or closed state of findings sections, last-lines sections and the reviewed-this-week list is saved across sessions.
- **Source:**.

### Dialogs

General dialog behaviour (`ui/modals.rs:135-187`; `mo/tests.rs`):
- Clicking outside a dialog does nothing; it closes only by its own choices or Esc.
- Esc or the close control closes the dialog.
- Focus moves into the dialog, Tab stays inside it, and focus returns to where it was on close.
- Page keyboard shortcuts are paused while a dialog is open.

#### Confirm mark-done or cancel (from a terminal's link menu)
- **Purpose:** Midna's terminal link menu can ask to mark a task done, or cancel it. That menu skips Midna's approval, so nothing happens until the owner confirms here.
- **Trigger:** a deep link `taskboard://task/T6?do=done|cancel`. The task opens with the dialog over it. Any other value for "do" just opens the task.
- **Data:**
  - which task
  - what yes does:
    - Done: marks it done now, without waiting for the agent to hand it in.
    - Cancel: marks it failed as cancelled by the owner, without waiting for the agent.
- **Actions:**
  - confirm: POST `tasks/{id}/done`, or POST `tasks/{id}/fail` with reason "Cancelled by you"
  - close
- **States:**
  - Before sending, the task is looked up again. If it already finished or failed in the meantime, the dialog shows a note and sends nothing. The confirm action then disappears.
  - While busy, the confirm shows progress.
  - If the dialog is closed while busy, the result still arrives as a toast.
- **Frequency:** rarely.
- **Source:** `ui/modals.rs:418-617`; app.rs:745-760.

#### Confirm merge without waiting (owner override)
- **Data:**
  - the PR number
  - what the merge would skip:
    - CI still running, or CI failing
    - review with changes asked
    - approvals so far out of the number needed, or not approved yet
  - that a conflict with the base still stops it
  - that a merge cannot be undone from the app
  - when the merge is not possible: a note that the task has no PR, it is already merged, or it is not open
- **Actions:** merge now, or cancel.
- **Refused-merge outcome** (the dialog stays open showing it):
  - who refused: GitHub, Bitbucket, GitLab, or the board
  - When the branch is behind its base, or conflicts with it, the dialog names the base branch and says the agent can rebase. It then offers:
    - ask the agent to rebase (POST `tasks/{id}/pr/rebase` with the reason). On success the dialog closes with a toast; a refusal shows as a toast.
    - focus the task's terminal in Midna
    - close
  - Any other refusal is shown word for word.
- **Frequency:** rarely.
- **Source:** `ui/modals.rs:262-416`.

#### Confirm closing a terminal
- **Trigger:** closing the terminal of a failed task, or of one that is still running a command.
- **Data:**
  - what closing does: closes the terminal, or stops the running command
  - for a failed task, that the task stays failed
- **Actions:**
  - close the terminal (POST `/close-terminal` with force)
  - keep it open, which sends nothing
- **Frequency:** rarely.
- **Source:** `ui/modals.rs:543-551`.

#### The whole note for testers
- **Data:** the full note, formatted. It scrolls when taller than the window.
- **Action:** close.
- **Frequency:** weekly.
- **Source:** `ui/modals.rs:211-227`.

#### Confirm finishing a goal by settling its last miss (from an issue)
- **Trigger:** on an issue that is its goal's only remaining miss, the owner chooses to ticket it, won't-do it, or finish the goal without it.
- **Data:**
  - that this is the last thing holding the goal open
  - what happens to the issue: sent to Jira, closed as won't do, or stays in the backlog
  - that the goal's outcome stays unmet
  - when the goal closes terminals once done, that this will happen
- **Actions:** finish the goal (runs the chosen action), or keep it open.
- **Frequency:** rarely.
- **Source:** `ui/modals.rs:484-540`.

#### Other dialogs in the same dialog system (belong to other areas)
- **Alerts list.**
  - Read-only list of board alerts.
  - Dismiss all, offered when some alerts are dismissible.
  - Closes itself when none are left.
  - Opening an alert closes the dialog and opens its task or goal.
  - (`ui/modals.rs:189-209`; app.rs:2373-2395.)
- **New idea form** (`ui::ideas::form`).
- **Cmd+K search palette** (`ui::search`).

## 5. Backlog

The backlog holds issues that agents (and the owner) report while they work. Each issue has a kind: bug, gap, follow-up or clean-up. "Untriaged" means an open issue that is not in a goal, not deferred and not closed as won't do. This page exists to clear untriaged issues: you turn them into goals, put them into existing goals, defer them, or close them as won't do.

Global entry: a navigation shortcut (⌘4) and a navigation entry. That entry shows the untriaged count when it is above zero. Deep links: `taskboard://backlog` and `taskboard://plan`.

### Untriaged issue list

- **Purpose:** see every untriaged issue, grouped in a chosen way, and pick issues to act on.
- **Data source:** `GET /backlog/triage` with project, lens, kinds, q (filter words), limit and before. It loads 100 issues per page. The next older page loads as the list nears its end. The page polls every 3s, or every 1.5s while a plan is being made.
- **Data shown for the whole page:**
  - The total untriaged count. On hover: "N issues not in a goal".
  - A count per kind. These counts ignore the kind filter, so every kind shows its true count.
  - A per-project untriaged count, used by the project filter.
  - A "Claude is grouping new issues" indicator while the board is grouping new issues into similarity groups.
- **Data shown per group:**
  - The group's label.
  - The true total in the group, including issues not loaded yet.
  - An "about" line that summarises the group.
- **Data shown per issue:**
  - pick state
  - kind
  - ref (B<n>)
  - title
  - a "new" marker if the issue was created in the last 48 hours
  - project, shown only when viewing all projects
  - area (the part of the app), hidden under the Area lens because the group already says it
  - impact level 1-3, with hover text naming the level (high/medium/low)
  - priority P1/P2/P3, with P1 emphasised
  - Hover on the row: "open the issue: what was said and its history".
- **Actions:**
  - Open an issue in the issue panel by click or ↩. The issue panel belongs to another area.
  - Move the cursor with ↑/↓. ↑ stops at the first issue. While the issue panel is open, ↑/↓ open the previous or next issue in it.
  - Pick or unpick the cursor row with Space, or pick an issue directly.
  - Pick or unpick a whole group, including issues not loaded yet. The group's pick state is none / some / all.
  - Fold or unfold a group. While scrolling, the current group's heading stays in view.
  - Esc unpicks everything, but only when no panel or dialog is open.
- **States:**
  - First load shows "Loading…".
  - Load error: "Can't load the backlog" with try again.
  - Board down: "The board isn't running, so the backlog can't load" with try again.
  - Stale data, meaning the last refresh failed but older data is still shown, comes in two versions. If the refresh failed: "couldn't refresh; this is how it last loaded" with try again. If the board is down: "this is the backlog as it last loaded", with the rows dimmed.
  - Empty list has four versions:
    - "every open issue is in a goal, deferred or closed"
    - "no issues match the filter"
    - "no issues of this kind"
    - still loading because more pages exist
  - A trailing loading marker shows while older pages load.
  - Scroll position is kept across refreshes. It resets when the lens, filter, kind or project changes.
  - The page must work in a small window as well as a large one.
- **Frequency:** daily. Agents report issues all day, and triaging them into goals is the owner's main way to plan work.
- **Source:** ui/backlog.rs:1-45, 171-181, 463-548, 759-830, 901-920, 1220-1290; ui/sidebar.rs:110,144; app.rs:1844.

### Lens (grouping mode)

- **Purpose:** choose how the issues are grouped.
- **Options:**
  - **Similar:** Claude groups issues that touch the same code or the same problem. Issues not grouped yet sit in a "not grouped yet" group.
  - **Area:** by part of the app.
  - **Impact:** high / medium / low. Impact means fewer wasted agent turns and problems the owner sees sooner.
  - **Priority:** P1 first. Claude suggests a priority when an issue is reported.
- Each lens has a hover explanation of what it groups by.
- Switching lens unfolds every group.
- **Frequency:** weekly. Most owners settle on one lens.
- **Source:** ui/backlog.rs:47-85.

### Kind filter

- **Purpose:** show only one kind (bug / gap / follow-up / clean-up) or all kinds. Each option shows its count.
- **Behaviour:** this filter gives way to the bulk actions while anything is picked.
- **Frequency:** weekly.
- **Source:** ui/backlog.rs:41-42.

### Project filter

- **Purpose:** limit the list to one project.
- **Behaviour:**
  - It appears only when there is more than one project, or when a project is already chosen.
  - It lists each project with its untriaged count, plus a search field.
  - The same component is used on the Ideas page.
  - The Projects page can open the Backlog already filtered to a project.
- **Frequency:** weekly.
- **Source:** ui/backlog.rs:964; ui/projects.rs:531-534.

### Text filter

- **Purpose:** filter issues by words.
- **Keys:**
  - ⌘F focuses the filter.
  - ↩ moves focus to the list, with the cursor on the first row.
  - Esc clears the text, or leaves the field if it is already empty.
  - There is also a separate clear action.
- **Frequency:** weekly.
- **Source:** ui/backlog.rs:759-830.

### Bulk actions on picked issues

- **Purpose:** triage many issues at once.
- **Data:** how many issues are picked.
- **Actions:** each one posts `POST /backlog/bulk` with {ids, action}.
  - **Move into an existing goal** (key M opens the goal menu).
    - The menu lists live, unfinished, unarchived goals in the picked issues' project, sorted by name. Each goal shows its ref, name and status: Paused / Running / Not started / its state.
    - If the picks span several projects, the menu explains that a goal holds one project's work.
    - If the project has no open goals, the menu says so.
    - What happens: the issues become planned tasks in that goal. If the goal is running, they are queued instead. The toast reports either case, or a mix such as "N queued, M planned".
  - **Set priority** (key P opens the menu; 1 / 2 / 3 set P1 / P2 / P3 directly).
    - The options are P1 "do first", P2 "normal" and P3 "when there's time".
    - The menu marks the current priority when every picked issue already has it.
    - The selection is kept afterwards. There is no Undo.
  - **Defer** (key D): the issues leave the backlog and go to the Deferred list.
  - **Won't do** (key X): the issues are closed as won't do.
  - **Clear the selection.**
- **Feedback:** a toast names the issues, for example "B9, B20, B39 and 2 more". Move, defer and won't do offer Undo from the toast and remove the acted-on issues from the selection.
- **Edge case:** shortcut keys are ignored while a menu or dialog is open, so typing in the project search isn't taken as a shortcut.
- **Frequency:** daily, whenever the owner triages.
- **Source:** ui/backlog.rs:391-409, 582-651, 1360-1396.

### New goal from picked issues (plan into waves)

- **Purpose:** turn the picked issues into a new goal, with Claude planning the order of work as waves.
- **When shown:** once anything is picked, or just after a goal was made from picks.
- **Data shown:**
  - Issue count, and plan status: not planned, or N waves.
  - The plan's suggested goal name.
  - A warning when the picks span several projects (a goal holds one project).
  - A "Claude is planning the waves" state while planning.
  - The plan's note.
  - Per wave: number, why it is a separate wave, item count, and items. Each item shows ref, title, the issues it waits on ("after B…") and priority.
  - A section for issues not in the plan yet. It is titled one of two ways:
    - "Not planned yet", with an explanation: Claude splits the work into waves (what goes first, what runs side by side, what waits so terminals don't edit the same files).
    - "Added since the last plan".
- **Actions:**
  - **Plan:** `POST /backlog/plan` {ids}. Progress is polled with `GET /backlog/plan`, which returns state (planning or ready), ids, waves [{why, items [{ref, after}]}], name and note. Plan is disabled when the picks span projects; the hover says to pick issues from one project.
  - **Re-plan.**
  - **Add the newly picked issues to the last wave** without re-planning: `POST /backlog/plan/add`. The hover says they go into Wave N waiting on nothing.
  - **Remove an item** from a wave.
  - **Create the goal:** `POST /backlog/goal` {name (the plan's name, or "Backlog goal"), waves}.
    - Enabled only when the count is above 0, not planning, the plan exists and isn't stale, and the picks are one project. Otherwise its purpose reads as "plan before creating" or "re-plan before creating".
    - After creating: a toast says "Made G<n>. Start it when you're ready.", the picks clear, and the app goes to the new goal's page. A notice "G<n> · name created" links to the goal.
- **Edge cases:**
  - Unpicking an issue drops it from the plan without needing a re-plan.
  - Picking more issues after a plan marks the plan stale.
  - Picked issues that were triaged elsewhere drop out of the picks on refresh. Picks are re-fetched across projects with `GET /backlog/triage?ids=…&project=all&limit=500`.
  - The issue panel offers "select for goal", which adds that issue to these picks.
  - Some toasts carry an "open it" link to a task. These stay for 8s.
- **Frequency:** daily to weekly. This is the main way backlog issues become work.
- **Source:** ui/backlog.rs:343-389, 440-461, 653-706, 1398-1541.

### Deferred and won't-do issues (closed backlog)

- **Purpose:** review issues that were set aside or rejected, and bring any of them back.
- **How you get here:** a link from the backlog page; a back action returns to the Backlog.
- **Data source:** `GET /backlog/closed?state=defer|wont_do`, 100 per page, most recently closed first. Counts {defer, wont_do} cover every project. The page polls every 5s.
- **Data shown:**
  - Two lists, deferred and won't do, each with its count.
  - Per issue: kind, ref, title, project · area, and when it was closed ("Deferred today", "Deferred Nov 9", or "Won't do Nov 9, 2025" in another year).
- **Actions:**
  - Open the issue in the issue panel.
  - Reopen it: `POST /backlog/<ref>/reopen`. This shows a sending state, a toast "Opened B<n> again", and marks the backlog stale so it refetches.
  - Switch between the two lists. Scroll position is kept within each list.
- **States:**
  - "Loading…"
  - Error: "these didn't load", the message, and try again.
  - Empty: "nothing deferred" / "nothing closed as won't do".
- **Frequency:** rarely. This is an occasional clean-up or recovery of something set aside.
- **Source:** ui/backlog_closed.rs.

### Backlog issue detail

An issue is a reported problem or follow-up waiting in the backlog. It has two surfaces:
- the full issue panel: from the Board, goal pages, links, and review fixes
- a compact read-only version while browsing the Backlog page's list
(`ui/issue_panel.rs:1-10`)

#### Issue identity
- **Data:**
  - a backlog marker
  - kind: bug, test gap, follow-up, clean-up or issue
  - a blocks-PR or blocks-goal flag, with the reason it blocks
  - ref, title and project
  - a state line:
    - open, with when it was reported
    - made into a task (link to it)
    - Jira ticket key, or a note that a ticket is being created
    - closed as fixed, by a task or PR #, with a link to the fixing task
    - closed as won't do
    - deferred
- **Actions:**
  - open the task it became or that fixed it
  - close
  - Esc
- **States:** loading. The panel stays open while browsing, like a task.
- **Frequency:** daily.
- **Source:** `ui/issue_panel.rs:171-240`, `1403-1500`, `1524-1556`.

#### Triage actions (full panel only)
- **Purpose:** decide what happens to a reported issue.
- **Actions on an open issue not in a goal:**
  - **Select it for a goal.** Checks it on the Backlog page, closes the panel and goes there.
  - **Make it a task** (POST `backlog/{id}/promote` with where = board). On the Board, the new task opens.
  - **Create a Jira ticket** (POST `backlog/{id}/ticket`). Only when Jira is set up.
  - **Defer** (POST `backlog/bulk` with action = defer).
  - **Won't do** (POST `backlog/{id}/drop`).
- **An open issue already in a goal:** make it a task, create a ticket, or won't do.
- **A deferred or won't-do issue:** reopen (POST `backlog/{id}/reopen`).
- **An issue fixed or made into a task:** no actions.
- **Feedback:**
  - Defer, won't do and finish-without show a toast with undo when the board returns an undo path. Otherwise a short note appears in the panel.
  - The changed issue keeps its place in the backlog list, even if it no longer matches the filters.
- **Frequency:** daily.
- **Source:** `ui/issue_panel.rs:530-551`, `657-810`, `1190-1223`.

#### Goal miss (an issue holding its goal's outcome open)
- **Data:**
  - which goal it holds open
  - how many other misses remain, or that this is the last
  - the goal's done-when outcome
  - for each choice, what it does to the goal
- **Choices:**
  - **Make it a task.** The goal stays open until the task is done.
  - **Send to Jira.** The goal stops waiting for it, or the goal finishes with its outcome unmet.
  - **Won't do.** Same goal effect as sending to Jira.
  - **Finish without it** (POST `backlog/{id}/without-goal`). The issue stays in the backlog, and the goal stops waiting for it or finishes.
- **After making it a task:** the panel closes, and a toast says what was made and offers to open it.
  - A planned task waits until the goal starts.
  - A queued task runs, and the goal stays open until it is done.
- **States:** the last miss always asks first (see Confirm finishing a goal by settling its last miss).
- **Frequency:** weekly.
- **Source:** `ui/issue_panel.rs:484-530`, `723-765`.

#### How it was reported and what was happening
- **Data:**
  - One sentence on how it was reported:
    - the owner added it
    - it came from an answer
    - from the review log
    - the named terminal reported it while working on a named task
  - The reporter's own words, quoted.
  - Detail text, with line breaks kept.
  - A snapshot of what was happening, as labelled rows:
    - from, task, step, branch, worktree
    - last commit, uncommitted file count
    - file, last turn, output, folder
    - Empty state: no snapshot was saved.
- **Action:** open the reporting task's full log, at the time the issue was reported. This switches to the Board with that task's log view.
- **Frequency:** weekly.
- **Source:** `ui/issue_panel.rs:261-345`, `400-425`, `1251-1302`, `812-818`.

#### Issue history
- **Data:** each event's time (full time on hover), a marker for its kind (report, seen, ticket, task, move, lost, other), who did it, and the text.
- **States:**
  - Empty: nothing yet.
  - The oldest 200 events arrive with the issue. Later pages load automatically while scrolling (GET `backlog/{id}/history?limit=200&after=`).
  - A failed page fetch stops asking. Only visible rows are drawn.
- **Frequency:** weekly.
- **Source:** `ui/issue_panel.rs:41-170`, `1304-1340`.

#### Searchable project / goal chooser (shared with the Backlog page and page filters)
- **Purpose:** pick a goal or project from a long list quickly.
- **Data:**
  - Goals: name, with project and Jira epic. Only goals that are not archived; can be limited to one project.
  - Projects: name and folder path (home shown as ~).
  - Project filters: "all projects", then each project with its count.
  - The current value is marked.
  - Special entries come first when no query has been typed.
- **Actions:**
  - Type to filter. Matches rank as: starts with, then a word starts with, then contains. Goals also match on their project and epic.
  - Arrow keys move, Enter picks, Esc closes.
  - Picking the current value does nothing.
  - Focus returns to where it was.
- **States:** no match says so, quoting the query.
- **Frequency:** daily.
- **Source:** `ui/issue_panel.rs:819-1165`.

## 6. Goals

### Why a goal stopped (shared stop model)

#### Why a goal stopped
- **Purpose:** Say in one line why a running goal isn't moving, and give the action that unblocks it.
- **Data shown and actions:** At most one stop per goal, picked in this priority order:
  1. **A task failed, and the next wave waits.**
     - Open the failed task.
     - Continue anyway.
  2. **A wave is held.**
     - Let the held wave start.
     - The detail says when it was held.
  3. **The goal needs the owner's answer.**
     - One task waits on an answer, or N tasks need the owner.
     - Answer the task.
  4. **A wave is done and ready to look over.**
     - Continue to the next wave, or to the rest of the goal.
  5. **Outcomes were missed and hold the goal open.**
     - See the misses.
- **States and edge cases:** A paused goal shows no stop.
- **Frequency:** daily.
- **Source:** ui/stop.rs, ui/sidebar.rs.

### Goals list (all goals)

#### Goals overview
- **Purpose**: one place to see every non-archived goal, which ones need the owner, which are moving, and what's queued, paused, set aside or recently finished. Opened from the main nav (⌘3), the `#/goals` link, or a project crumb on a goal page (pre-filtered to that project).
- **Data shown**: goals grouped into Active, Next (not started, or chained after another goal), Paused, Set aside (deprioritized) and Finished. Active goals are split into a "stopped" section (goals with a stop, ordered failed → held → review, then others) and an "in progress" section (ordered by project then id).
- **Actions**: filter by project; expand/collapse all in-progress rows; open a goal; per-row actions below.
- **States and edge cases**: daemon down → a can't-load message; loading; no goals at all → empty message. Archived goals never appear.
- **Frequency**: daily — the owner's main way to check goal progress and clear stops.
- **Source**: `ui/goals_page.rs` (`goal_group`, sections, render); `ui/goals.rs` (`all_goals`, `goal_counts`, `nav_kind`).

#### Project filter
- **Purpose**: narrow the list to one project.
- **Data shown**: each project with a count of its unfinished goals. A goal spanning several projects (via its tasks) appears under each and shows them joined (e.g. "web + api").
- **Actions**: pick a project / all. The choice is in-page state.
- **States and edge cases**: only offered when there is more than one project. If the picked project has no goals left, it falls back to all.
- **Frequency**: weekly — useful with many projects.
- **Source**: `ui/goals_page.rs`.

#### In-progress goal row
- **Purpose**: one goal's state at a glance plus the next step to unblock it.
- **Data shown**: ref, project(s), name; how its tasks split across statuses; "x of N done · M in review"; "wave k of n"; a **now** line (counts working / needs you / failed, or in review / pending merge / stopped / queued / done; or the stop kind if stopped); a **next** line (e.g. wave n done and k tasks in the next wave, a task waiting on your answer, a task failed, PR stage, "stops for your review after this wave", or how many tasks are in the next wave).
- **Actions**:
  - open the goal page (ref);
  - pause the goal (nothing new starts; running tasks carry on) → `POST goals/{G} {"paused":true}` — shown when there's no stop;
  - when stopped: the stop's secondary then primary step (e.g. continue anyway / open the failing task; answer the waiting task; continue to the next wave) → continue posts `POST goals/{G}/waves/{n}/continue`, task steps open the task panel;
  - a review action on review stops that expands the row;
  - click the row to expand/collapse; expanding fetches `GET goals/{G}`.
- **States and edge cases**: results show inline (toast or inline note); busy actions are disabled with a busy label.
- **Frequency**: daily.
- **Source**: `ui/goals_page.rs` (row, now/next line builders, ~l.560-820); `ui/stop.rs`.

#### Expanded goal row (task list in place)
- **Purpose**: see a goal's tasks without leaving the list.
- **Data shown**: tasks grouped under each wave (wave meta: done at time/yesterday/weekday/date, "now · stops for your review after", after wave N, next), a no-wave group; goals without waves list tasks by status rank. Each task: status, ref, title, kind (design or research), a status line, age.
- **Actions**: open a task (in the task panel); for tasks needing an answer, an action to answer (opens the task); for tasks in review/pending merge with a PR link, an action to open the PR in the browser.
- **States and edge cases**: a loading placeholder until the goal detail arrives.
- **Frequency**: daily.
- **Source**: `ui/goals_page.rs`.

#### Shelves: Up next, Paused, Set aside, Finished this week
- **Purpose**: the goals that aren't moving right now, kept to one line each.
- **Data shown**:
  - Up next: task count and what it starts after (chained goal).
  - Paused: progress "x of N", plus a resume action whose wording says what resuming does (N tasks start / PRs move again).
  - Set aside: progress.
  - Finished this week (week starts Sunday): PRs merged (or "No PR"), failed count (accepted), when finished. A toggle reveals older finished goals ("N earlier" ↔ only this week).
  - When nothing is in progress, the shelves get roomier and show each goal's 2-line TLDR/outcome.
- **Actions**: open a goal; resume a paused goal (`POST goals/{G} {"paused":false}`, or run if it has no planned tasks — see goal page); toggle older finished goals.
- **Frequency**: weekly.
- **Source**: `ui/goals_page.rs` (strip).

### Goal page (one goal)

#### Opening and loading
- **Purpose**: the single home of a goal: its state, waves and tasks, how it runs, notes, files, devices, flags, backlog.
- **Entry points**: Goals list, the always-available "active goals" list, search (⌘K), project page, idea page (goal an idea became), weekly review, any G-ref link; route `#/goals/G3` and `#/goals/G3?view=backlog` (shows the whole backlog rather than the first few).
- **States**: while the detail loads, the header is filled from the goal's summary row and a loading/error line is shown. Run actions appear only once the detail has loaded. The page must work in a small window as well as a large one.
- **Frequency**: daily.
- **Source**: `ui/goal.rs` (`open`, render); `app.rs:764`; `ui/sidebar.rs:187`; `ui/search.rs:818`; `ui/idea.rs:750,949`; `ui/projects.rs:556`; `ui/review.rs:636`.

#### Header
- **Data shown**: where it belongs (Goals › project › ref, each step a way back); ref and project (hover shows the repo path); epic key and epic status (a link when the epic has a web URL); name; TLDR; the goal state; progress. Ref, name and TLDR are selectable text.
- **Goal state** (one, in priority order): waiting on you (a stopped PR, a task needing you, or stopped at a wave) · N PRs awaiting merge · outcome not met · waiting on N bits (feature flags) · done · deprioritized · paused · needs a restart (a terminal was lost, not closed by hand) · in progress · stopped (terminal closed by hand) · blocked (a queued task is blocked) · queued · no tasks yet · not started. Tasks shared from other goals count toward this goal.
- **Progress**: "x of N done" plus a breakdown by status (done, pending, working, needs you, review, failed, stopped, queued, planned) shown as proportions. Before the detail arrives it counts from the summary row.
- **Actions**:
  - project crumb / project → Goals list filtered to that project;
  - epic → opens in the browser;
  - when the outcome isn't met: a misses count that opens a menu of the open misses; each opens its backlog issue (issue panel).
- **Frequency**: daily.
- **Source**: `ui/goal.rs` (`goal_state`, `progress`, header).

#### Run controls (start, schedule, pause, resume)
- **Purpose**: the owner's levers for when a goal's work runs.
- **Actions** (only the ones that apply are offered):
  - **Start** the planned tasks / start N more tasks → `POST goals/{G}/run {}`. Explanatory hovers: queues the planned tasks and the goal's rules decide what runs; or re-queues tasks that couldn't start.
  - **Start now outside work hours** → `POST goals/{G}/run {"now":true}`; the goal runs until work hours open.
  - **Start at a time** (always in the start menu): a text field accepting free text like "5pm" or "tomorrow 9am"; Enter submits, Escape cancels and returns focus; empty input gives an inline hint; server errors show inline in the field → `POST goals/{G}/run {"at": text}`. "off" unschedules.
  - **Scheduled**: when a start time is set, the control shows when it starts (today/tomorrow/weekday + time) with a menu to start now, change the time, or cancel the schedule.
  - **After hours with planned tasks**: start at the next work-hours opening (with how many tasks), or start now, start at…, and pause (only while waiting for the hours).
  - **Waiting for work hours** (queued tasks wait): shows until when, with start now and pause.
  - **Resume**: when paused, or going but not started, or paused while reviewing. Hovers explain: let the board work on PRs again / let new tasks start again. A paused goal's start becomes "resume and start"; plain resume is then dropped from the header. If a paused goal has planned tasks, resume only unpauses; otherwise it runs (which also picks up parked tasks).
  - **Pause** → `POST goals/{G} {"paused":true}`. Shown while going or reviewing, not paused, not deprioritized and no start failure. Hover variants explain that PRs keep waiting for fixes, comments and merges.
  - **Chained goal**: instead of start, shows that it starts after another goal, which opens that goal.
  - **Open in Claude** (always): `POST goals/{G}/plan {"mode":"edit"}` and opens a Claude terminal (in Midna) on the goal's plan.
- **Results**: toasts, e.g. how many tasks queued and parked tasks resumed; that tasks are queued and start at a day/time; going again; started now until work hours open; "starts <when>" / "isn't scheduled any more". Toasts append up to two reasons tasks still wait (e.g. a task waiting for a human task) then "N more wait".
- **States and edge cases**: a stopped goal hides pause and the waiting control; busy controls disable with a busy label.
- **Frequency**: daily — every goal is started, paused or rescheduled.
- **Source**: `ui/goal.rs` (`run_buttons_view`, `header_buttons`, start-at field, run/pause/resume handlers).

#### Bands under the header (deprioritized, stop, paused)
- **Purpose**: tell the owner, in one place, why the goal isn't moving and the next step.
- **Data shown and actions**, in order:
  1. **Deprioritized**: off the board's goal list, nothing new starts, running work carries on; action to bring it back → `POST goals/{G} {"deprioritized":false}`.
  2. **Stop** (from the stop model): kind (failed / held / needs your answer / ready for review / outcome missed), title and detail, e.g. "T54 failed in wave 3 — wave 4 waits; try it again from its panel or continue past it"; "Wave 2 is held — starts only when someone lets it go (held at <time>)"; "T5 needs your answer"; "Wave 1 is done — look it over, then let wave 2 (or the rest of the goal) start"; "every task is done but N misses hold this goal open". Steps: open the failing/waiting task, continue anyway / let wave n start / continue to the next wave (`POST goals/{G}/waves/{n}/continue` → "Wave n can start" or "The goal goes on"), see the misses (opens the misses menu). A review stop also offers a review menu listing the stopped wave's tasks (with a PR marker); each opens its task.
  3. **Paused**: with a resume action whose wording says what resuming does.
- **States**: request results show inline under the bands.
- **Frequency**: daily.
- **Source**: `ui/goal.rs` (bands); `ui/stop.rs`.

#### Plan review findings
- **Purpose**: show what the latest automated review of the goal's plan against its outcome found, so gaps get fixed before work runs.
- **Data shown**: count of findings and when the review finished; each finding: its kind (outcome not proven, needs the owner, past an open decision, overlaps another goal, goes against a lesson; unknown kinds shown raw), a one-line summary, the refs it names (tasks, goals).
- **Actions**: click a finding to expand/collapse its markdown detail; with no detail, hover shows the full line. Read-only — rerun via `tb goal review G<n> --again`.
- **States and edge cases**: hidden entirely when the review is running, skipped, failed, or found nothing; long lists scroll within a capped height; details start folded.
- **Frequency**: weekly — after planning a new goal.
- **Source**: `ui/goal/review.rs:1-118`.

#### Waves and tasks
- **Purpose**: the goal's work in run order.
- **Data shown**: one group per wave, then a "no wave" group, then tasks shared from other goals. A goal without waves is one group with no heading. Each wave: number and name, and its phase — done at (time today / yesterday / weekday / date), done and ready for review, failed, held, now, after wave N and your review / after wave N / next; its overall status; a flag when the goal will stop for review after this wave (not yet released, not stopped/done).
- **Wave gate** (inside the wave): held → let it start; done and stopped for review → continue to the next; a task failed and the next wave waits → continue anyway. All → `POST goals/{G}/waves/{n}/continue`; result inline.
- **Task row**: status, ref, title, kind (design or research), a status line (emphasised when urgent), a PR cell (merged / open with stage / no PR and why / branch / shares another task's PR / ships one PR / no PR), age. Truncated text reveals in full on hover.
- **Actions**: click opens the task in the task panel (the open task is marked); a retry action when the task's start failed or a retry is scheduled → `POST tasks/{T}/start {"mode":"new"}`.
- **Frequency**: constantly while a goal runs.
- **Source**: `ui/goal/waves.rs`; virtualized via `ui/goal/lists.rs`.

#### How it runs
- **Purpose**: the goal's run rules at a glance.
- **Data shown**: terminals at most and in use (default 2); order (in order by wave / in order / side by side); whether terminals close when done; review stop after wave N (for review-stop waves not yet done).
- **Actions**: Jira tickets on/off (only when Jira is configured) → `POST goals/{G} {"jira_off":bool}`; result explains that turning Jira off means its tasks ask for no tickets, and turning it on applies to new tasks only.
- **Frequency**: rarely — set once per goal; Jira toggled occasionally.
- **Source**: `ui/goal.rs` (aside "How it runs").

#### Notes
- **Purpose**: decisions, findings and references agents (or the owner) recorded on the goal.
- **Data shown**: count ("N" or "x of N" when filtered); each note: kind, pinned flag, source (a task ref or "you"), text with clickable URLs and highlighted code/paths. Pinned first, then newest.
- **Actions**: show/hide (folded by default); filter by kind (all / decisions / findings / references, only when more than one kind exists); source task ref opens the task.
- **States and edge cases**: hidden when there are no notes. Paged 100 at a time (`GET goals/{G}/notes?limit=100&before=<id>`, fetching ahead within 20 rows of the end); a loading line while more come; on error it stops asking.
- **Frequency**: weekly.
- **Source**: `ui/goal.rs` (notes); `ui/goal/lists.rs` (NOTE_PAGE, NOTE_AHEAD).

#### Attached files
- **Purpose**: evidence, designs and docs attached to the goal and its tasks.
- **Data shown**: the goal's own files plus each task's, grouped by wave then task; count ("N" or "x of N").
- **Actions**:
  - wave picker (only when >1 wave has files): each wave with its phase (done/now/held/failed/your review) and file count; empty waves are inert. Defaults to the current wave, else all; remembered per goal.
  - kind filter (all / evidence incl. results / design / docs incl. proposals): only when some but not all files are of a kind; falls back to all if the shown files hold none.
  - open a file (URL → browser; local path → system open); open a task by its label.
  - hover an image/video for a preview card (title, filename · date, summary); other files show link/file and a short path.
- **States**: hidden when there are no files.
- **Frequency**: weekly — checking evidence at review stops.
- **Source**: `ui/goal/attached.rs`.

#### Devices
- **Purpose**: which test devices (simulators, emulators, phones) the goal's tasks use and who holds each.
- **Data shown**: what each task needs (and that it's the project's default); why the pool is empty, when it is; each device: label (name with kind/target), tags, pool info (for a purpose, reserved here / reserved for another goal), holder task (hover shows its title) or free/off.
- **Actions**: fold/unfold (folded by default); open the holder task; focus a device (raise its window) when possible → `POST devices/{name}/focus`, toast "Raising <name>". Adding/changing devices is CLI-only (`tb device`).
- **States**: hidden when the goal has no device info.
- **Frequency**: rarely — mobile/device goals only.
- **Source**: `ui/goal/pool.rs:24-168`.

#### Bits (feature flags)
- **Purpose**: flags the goal's work depends on and whether they exist in the flag tool.
- **Data shown**: "x of N created"; each flag: name, state (created / not created / local, not in <tool>), the tasks and goals using it, an optional note.
- **Actions**: open a link to create an unmade backend flag in the flag tool (browser). Changes otherwise via `tb bit`.
- **States**: hidden when there are none. Unmade flags make the goal "waiting on N bits".
- **Frequency**: rarely.
- **Source**: `ui/goal/pool.rs:73-200`.

#### Goal backlog
- **Purpose**: open issues tied to the goal.
- **Data shown**: open count; misses on the goal's "done when" first; each issue: ref, title, a misses-goal marker, a blocks-PR / blocks-goal marker with its reason (2 lines).
- **Actions**: open an issue (issue panel); show the rest beyond the first 5 (or open with `?view=backlog` to list all).
- **Frequency**: weekly.
- **Source**: `ui/goal.rs` (backlog aside).

## 7. Ideas

An idea is a rough proposal (I<n>) for one project. Claude can "dig" into an idea: it reads the code (and sometimes the web) and writes a report with a verdict on fit, size and risk, questions for the owner, and options. Once the idea fits and has no open questions, it can be planned into a goal. Ideas can also be talked over in a live Claude terminal, shelved, or dropped.

Global entry: ⌘5, and a navigation entry with no count. Deep links: `taskboard://ideas` and `taskboard://idea/I<n>`.

### Ideas list

- **Purpose:** see every idea and which ones are waiting on the owner.
- **Data source:** `GET /ideas?project=all&state=all&first=questions`, 200 per page, with more pages loaded as needed. Each project in the response carries {open, closed, total}. Without those, the counts are computed from the ideas held.
- **Data shown:**
  - Open and closed counts for the chosen project, or for all projects. "Closed" means shelved or dropped.
  - The open view has two sections, each sorted newest change first, with a count, and omitted when empty:
    - **Waiting on you:** ideas with open questions first, then ideas ready to plan.
    - **Other ideas:** open ideas first, then planned ones.
  - The closed view is a single list.
  - Per idea:
    - ref
    - title
    - project
    - one status: Planning, Digging, Reported, Planned, Shelved, Won't do or New. Planning overrides the others. For a reported idea, the newest report's verdict shows instead: fits / partly fits / doesn't fit.
    - a waiting note: "N question(s)", or "ready to plan" (reported, fits, no questions). There is no note while digging or planning, or for a closed idea.
    - when it last changed: "Today, 11:05 AM", "Oct 7, 9:05 AM", or "Dec 1, 2025, 9:05 AM" in another year. Times use the 12-hour clock.
- **Actions:**
  - Switch between open and closed.
  - Filter by project. The filter appears only when there is more than one project, and it counts open ideas. If the chosen project has no ideas, the page falls back to all projects.
  - Open an idea.
  - Make a new idea (see below).
- **States:**
  - "Loading…"
  - Error: "Ideas didn't load", the message, and try again.
  - Empty: "No ideas" / "No closed ideas" / loading while more pages exist.
- **Frequency:** daily. The owner checks which ideas need answers.
- **Source:** ui/ideas.rs; ui/sidebar.rs:145; app.rs:1845.

### New idea

- **Purpose:** write down an idea for a project. Notes added to an idea later come through the CLI (`tb idea`), not the app.
- **Entry:** ⌘N works from anywhere (it is a global key), or from the Ideas page.
- **Fields:**
  - Title, required. Error: "Say what the idea is."
  - Project, required. Error: "Choose the project it's for." It defaults to the project the Ideas page is filtered to. The options are every known project (from board state, goals and ideas), sorted.
  - Detail, optional, multi-line. ⇧↩ adds a new line.
- **Actions:**
  - Submit with ↩ in the title or ⌘↩ anywhere. This sends `POST /ideas` {title, detail, project}. While sending it shows "Adding…". Then a toast says "Kept I<n>" and the new idea opens.
  - Cancel or Esc keeps the draft, with the toast "Idea draft kept · Reopen ⌘N". An empty draft is simply dropped.
- **States:**
  - A field error shows on its field until that field changes.
  - A board refusal is shown with the actions.
  - The draft survives an app restart for an update.
- **Frequency:** weekly.
- **Source:** ui/ideas.rs (new idea dialog, restore_draft); app.rs:1849, 1884-1885.

### Idea page

- **Purpose:** read everything about one idea and move it forward: dig, answer questions, plan into a goal, talk it over, or close it.
- **Data source:** `GET /ideas/:id`.
- **Data shown:**
  - Where it belongs: Ideas › project › ref. The project step opens the Ideas list filtered to that project.
  - Ref and title, both selectable.
  - Project.
  - State.
  - Verdict.
  - Detail as markdown, cut off at 20,000 characters.
  - Notes, each with its text, source (for example T12) and time.
  - Reports (see below).
- **Primary-action emphasis:** one action is emphasised depending on state:
  - Dig, for a new idea.
  - Plan, when the idea fits and has no open questions.
  - Open goal, once planned.
  - Talk, while digging.
  - Nothing is emphasised when questions are open, when the idea is closed, or when the verdict is short of "fits".
- **Frequency:** daily while ideas are active.
- **Source:** ui/idea.rs.

#### Dig

- **Purpose:** have Claude read the project's code and write a report on the idea.
- **Action:** `POST /ideas/:id/dig`. Global key ⌘D, also in the app menu. Once a report exists, the action becomes "dig again".
- **States:**
  - Running: shows elapsed time ("digging… 4m"); hover says Claude is reading the code.
  - A running dig also shows the project it is reading, the time it will stop (it is limited to 15 minutes, e.g. "stops at 5:50 PM") and how long it has run.
  - Disabled while planning, once planned ("it became G5"), or when shelved or dropped ("reopen it first").
  - Hidden while digging, once planned, or while planning.
- **Stop a running dig:** `POST /ideas/:id/stop`. The hover warns that what it found so far is lost. Toast: "Dig stopped".
- **Note shown while digging:** "Plan and Close wait until the dig finishes."
- **Frequency:** weekly.

#### Plan

- **Purpose:** have Claude turn the idea into a planned goal. The goal is not started.
- **Action:** `POST /ideas/:id/plan`. Global key ⇧⌘P.
- **States and edge cases:**
  - If questions are still open, it first asks for confirmation ("Plan it now?") and lists the open questions, with plan anyway / cancel.
  - Disabled while digging: "wait for the dig to finish".
  - Shows a planning state with a progress indicator.
- **After planning:**
  - An action to open the goal G<n>.
  - **Undo plan:** `POST /ideas/:id/unplan`. It deletes the goal and reopens the idea, but not once any of the goal's tasks has started. Toast: "G deleted; I is open again".
- **Frequency:** weekly.

#### Talk it over

- **Purpose:** open a live Claude terminal in the project's checkout to discuss the idea. What gets settled comes back to the idea as notes.
- **Action:** `POST /ideas/:id/talk`. Global key ⌘T. It can also be aimed at one question (see Questions).
- **Availability:** any state except planned.
- **Error:** shown as a toast, e.g. when Midna (the terminal app) isn't running so no terminal can open.
- **Frequency:** weekly.

#### Close / reopen

- **Purpose:** set the idea aside.
- **Actions:**
  - Shelve for later: `POST /ideas/:id/shelve`.
  - Won't do: `POST /ideas/:id/drop`.
  - Both offer Undo from the toast, which posts `ideas/:id/reopen`.
  - Once closed, the closed state shows, with a reopen action (toast "I reopened").
- **Availability:** hidden once planned; disabled while digging or planning.
- **Frequency:** weekly to rarely.

#### All idea actions

Every idea action sends one request at a time and shows a sending state.

- **Source:** ui/idea.rs; menu_bar.rs:46-48 (⌘D, ⇧⌘P and ⌘T are menu-bar keys, so they work as menu commands).

#### Reports

- **Purpose:** what each dig found.
- **Data shown:**
  - Rounds, newest first ("Round N"). Only the newest starts expanded; each round can be expanded or collapsed.
  - A round's summary shows:
    - verdict
    - whether it's still running
    - whether it read the web
    - its focus: when collapsed, "focus — gist", or the error
    - a stale marker
    - its time
    - error text if it failed
- **Report blocks**, in this order: gist first, then questions, then the rest. Unknown or empty blocks are skipped.
  - **Gist:** a summary.
  - **Scorecard:** fit (fits / partly / doesn't fit), size (small / medium / large), risk (low / medium / high).
  - **Flow:** ordered steps, with new steps marked.
  - **Builds on:** code it builds on, each with what it is and a file name. The full path shows on hover.
  - **Point lists:** short lines with tags, where security and privacy tags are emphasised. Each line can be expanded to show detail.
  - **Questions:** see below.
  - **Compare:** options, each with pros and cons, and which option it is leaning toward.
  - **Related:** links to goals (goal page), tasks (task panel), issues (issue panel) and ideas (idea page).
  - Older reports map builds_on / risks / pitfalls / ways_to_extend / open_questions / related into these same blocks.
- **Frequency:** daily while digging ideas.

#### Questions ("to decide")

- **Purpose:** the dig's questions for the owner.
- **Answering:** pick one of the offered choices, or type an answer.
  - In the typed answer, ↩ sends. Esc leaves the box but keeps the text. An empty box sends nothing.
  - Answers go to `POST /ideas/:id/answer` {round, question, answer}.
- **Data shown:** an answered question shows its answer, with an action to change it.
- **Talk about one question:** opens a talk aimed at that question (talk with {question}).
- **States:**
  - Questions can't be answered once the idea is planned or while it is planning.
  - If the board refuses an answer, the choice or typed text is kept and an error toast shows.
- **Frequency:** daily while ideas are being shaped.

#### Idea page states

- "Loading…"
- Error: "This idea didn't load", the message, and try again.

## 8. Projects

A project is a codebase the board knows: its name and path. It can have releases, wait on other projects, and has its own backlog. This page is a per-project summary of open goals, loose tasks (tasks not in any goal) and finished goals. It is read-only.

Global entry: ⌘2, and a navigation entry that shows the project count. Deep links: `taskboard://projects`, `taskboard://projects/<name>` (picks that project) and `taskboard://tasks`. A task's project can also open this page on that project.

### Project list

- **Purpose:** see every project at a glance and pick one.
- **Data source:** the board state's `projects` (name, path, releases flag, latest_release, unreleased, waits_on, backlog_open, loose_open), plus the goal list. It is refreshed with the board's normal state polling.
- **Grouping and sort:**
  - Projects with open goals or loose tasks come first, by name.
  - The rest are in a "nothing open" group with its count. That group is folded by default and can be unfolded. Even when folded, the picked project stays visible.
- **Data shown per open project:**
  - Name.
  - A status line built from:
    - the number of goals and/or tasks
    - "needs you", if any goal or loose task needs the owner, or a goal stopped for the owner
    - "N working"
    - otherwise "queued" or "waits for <projects>"
  - Examples: "3 goals · needs you · 1 working", "1 goal · queued", "1 goal · 3 tasks · waits for lens", "2 tasks · 1 working".
  - A marker for "needs you" (most important) or "working".
- **Data shown per idle project:** name, and "N done" if it has finished goals.
- **Actions:** pick a project. Picking one collapses the "show all finished" expansion.
- **Edge cases:**
  - If the picked project disappears, the first project in the list is shown instead.
  - With no open goals, the live terminals on the project's tasks count toward needs you / working.
- **Frequency:** daily. This is the overview of where each codebase stands.
- **Source:** ui/projects.rs:1-235, 425-512.

### Project summary

- **Data shown:**
  - Name.
  - Short path.
  - Latest release name, as a link that opens it on the Releases page.
  - Release meta: its date and "N unreleased", e.g. "Oct 17 · 13 unreleased". Shown only when releases are on for the project.
  - "Waits for <projects>".
  - Backlog count, as a link that opens the Backlog filtered to this project.
- **Edge cases:**
  - With releases off, the release facts are hidden.
  - A project can have no path.
- **Frequency:** daily.
- **Source:** ui/projects.rs:237-269, 514-536.

### Open goals

- **Purpose:** the project's unfinished goals and how far along each is.
- **Order:** started first, then next, paused, then set aside; newest first within each.
- **Data shown per goal:**
  - ref
  - name
  - "with <other projects>" when its tasks span other projects
  - a progress breakdown by task status
  - "done of total", plus "· wave N" when it has more than one wave
  - one status, choosing the most pressing in this order: Paused, Set aside, Needs you (or stopped for owner), Working, Failed (stuck), In review, Pending, Stopped, Queued (or waiting on a blocker), Failed (only when nothing else is live), Planned
  - Paused and set-aside goals carry less weight than active ones.
- **Actions:**
  - Expand a goal in place to list its tasks. This loads `GET /goals/:id`.
  - Open the goal's own page from its ref.
- **Expanded goal:**
  - Tasks sorted by wave, then status, then id.
  - Then "shared" tasks that this goal counts, grouped under "From G<n> name", lowest goal first.
  - Each task shows ref, title, a line on what it's doing (otherwise "for <age>" while working, otherwise "wave N"), and its status.
  - Clicking a task's ref or title opens the task panel. The task currently open in the panel is marked.
- **States:** "Loading…" while a goal's tasks load.
- **Frequency:** daily.
- **Source:** ui/projects.rs:271-355, 542-695.

### Tasks outside any goal

- **Purpose:** the project's open loose tasks.
- **Data source:** `GET /tasks?goal=none&status=open`.
- **Data shown:** "Tasks · N", then the tasks, those needing you first and then newest. Each task row is the same as in an expanded goal.
- **Actions:** open the task panel.
- **Frequency:** daily.
- **Source:** ui/projects.rs:224-229, 752-758.

### Finished goals

- **Purpose:** history of the goals the project completed.
- **Data shown:** "Finished · N". Each goal shows ref, name, "N tasks" and the finish date (e.g. "Oct 18"). Newest first; the five newest are shown by default.
- **Actions:**
  - Show all / show fewer, when there are more than five.
  - Expand a goal in place to see its tasks.
  - Open the goal's page.
- **Frequency:** weekly.
- **Source:** ui/projects.rs:357-383, 697-720, 760-783.

### Projects page states

- No project available:
  - "Loading…" before the board answers.
  - "Can't load the board" when the board is down.
  - "No projects".
- A project with nothing open or finished: "No goals or tasks".
- The page must work in a small window as well as a large one.
- **Source:** ui/projects.rs:726-788.

## 9. Releases

Releases are optional and turned on per project with `tb project set <name> --releases on`. A release (R<n>, e.g. "v1.4") groups the finished tasks that shipped together. "Unreleased" holds finished tasks not yet in a release. A pending cut is a release that is set to be cut once certain tasks or goals finish. The board can drop a pending cut it can't make. A release can be marked failed (`tb release fail`), which sends its work back to Unreleased. The pages are read-only: cutting and editing releases happens in the CLI.

Global entry: ⌘7, and a navigation entry. The entry is shown only when at least one project has releases on. Deep links: `taskboard://release/R<n>` (Midna also turns R refs into links) and `taskboard://releases`.

### Releases off

- **Purpose:** explain why there is nothing here.
- **Data shown:** releases are off, with the CLI command that turns them on.
- **Behaviour:**
  - Back/forward history skips this page while releases are off.
  - If the open project's releases are turned off, the app falls back to the overview.
- **Frequency:** rarely.
- **Source:** app.rs:525-536, 1715-1721, 2028-2050.

### Releases overview

- **Purpose:** the release state of every project that has releases on.
- **Data source:** `GET /releases?project=<p>&limit=4` for each project.
- **Data shown:**
  - Page summary: total unreleased tasks across projects, and "N pending cuts" when there are any. Shown only once every project has loaded.
  - Per project:
    - Name, and its total number of releases. Choosing the total opens that project's page.
    - **Newest release:** name, cut time ("Today, 3:05 PM" / "Oct 7, 3:05 PM" / "Oct 7, 2025, 3:05 PM"), task count, a failed marker if it failed, and its headline (the first line of its notes). It opens that release on the project page.
    - **Unreleased:** shown when tasks are waiting or a cut is pending. It shows the task count and the next cut's status, one of:
      - "<name> · N still to finish"
      - "<name> · Ready to cut"
      - "<name> · Cutting"
      - "<name> dropped"
      It opens the project page.
    - **Up to three earlier releases:** name, headline, and cut time or a failed marker. Each opens on the project page.
- **States:**
  - Per project: loading, or "Can't load <p>'s releases" with try again. Older data stays visible.
  - With no projects yet: loading, or "Can't load the board" with try again.
- **Frequency:** weekly, around shipping.
- **Source:** ui/releases.rs:266-288, 822-956.

### Project releases page

- **Purpose:** one project's full release history, what is waiting, and the next cut.
- **Data source:**
  - `GET /releases?project=&limit=`, 50 at first, then 50 more as the list nears its end. The list is virtualised.
  - `GET /releases/unreleased?project=`.
  - `GET /releases/:id` for each release that is expanded.
- **Data shown:**
  - A back action to the overview, and the project name.
  - Counts: "N releases · N in the last 30 days", plus "· N failed" when there are any.
  - **Dropped cut:** "<name> was dropped: <why>", plus the exact CLI command to cut it by hand once it's ready.
  - **Next release** (when a cut is pending): name, progress status, then two parts:
    - **What it cuts after:** each task or goal with ref, title and standing.
      - met: "Done"
      - waiting task: "Running", "In review", "Checks", "Fixing", "Merging", "Needs you", "Blocked", "Planned" or "Queued"
      - waiting goal: "N of M done"
      - dead: "Can't finish", with the reason on hover
      Each row opens the task panel or the goal page.
    - **Already finished:** the unreleased work, grouped by goal.
  - **Unreleased** (when there's no pending cut but tasks are waiting): "N tasks · N goals", then the work grouped by goal.
  - **Past releases**, under month headings (e.g. "October 2026"), newest first. Each shows name, headline, cut time, and either its task count or a failed marker.
- **Expanded release:**
  - The newest release starts expanded; any release can be expanded or collapsed.
  - Shows: a failed line ("Failed <when> · its work went back to Unreleased"), the rest of the notes after the headline, and its work grouped by goal.
  - Goal groups: a goal heading (ref and name) that opens the goal. Tasks without a goal fall under "No goal"; a goal with no name is "Untitled goal".
  - Each task shows that it is done, its ref, title and a PR link (#num, or "PR"). The PR opens in the browser; the task opens in the task panel.
  - A failed release also lists what it contained ("was in it"), and for each task where it went since: "went out in v0.11.2", "in Unreleased", or nothing if it was reopened.
- **Actions:**
  - Expand or collapse a release.
  - Open a task, goal or PR.
  - Try again after an error.
  - A linked release (from a deep link or the Projects page) scrolls into view and expands, loading more pages until it appears. If the release's project wasn't known, the overview shows first, then the app moves to the right project page once the release's details arrive.
- **States:**
  - The page shows "Loading…", "Can't load the board", or "Can't load <p>'s releases" with try again. Older data is kept.
  - Per release: "Loading…", "Can't load <name>" with try again, or "No tasks".
  - "No releases yet" when there is nothing at all.
  - A trailing loading marker while more pages exist.
  - The current task or goal open in the panel is marked in every list.
- **Frequency:** weekly. Checked before and after a cut, and when a release fails.
- **Source:** ui/releases.rs:1-264, 290-820; app.rs:780-784, 1256-1261.

## 10. Days, weekly review and recaps

### Days

#### Day page
- **Purpose**: what every project's agents did on one day and across its week, so the owner can catch up after being away and look back at any earlier day. Data from `GET days?hide=<projects>&date=YYYY-MM-DD` (no date = today). Opened from the main nav (⌘6, `#/days`); the same nav entry also switches to the weekly review and recaps (another area) and carries an unread-recap count.
- **Data shown**: title (today, or the weekday and date); for today, the live clock.
- **Actions**:
  - previous day (disabled at the oldest day with data), next day and jump to today (both disabled on today);
  - pick a day by clicking it in the week chart or the week's day tiles (future days can't be picked);
  - switch between day, week (weekly review) and recaps views;
  - show/hide each project (hover says which); at least one project must stay visible; hidden projects persist across launches (pref `taskboard.days.hidden`) and are sent to the daemon. Each project keeps the same identity on every chart whatever is hidden, so it reads the same day to day.
- **States**: loading or error message until the answer arrives. Changing the day refetches and folds the "done" list back. The page is part of the app's regular refresh, so today's numbers stay live while it is open. The chosen day is not part of the page's link, so a link always opens today.
- **Frequency**: daily — morning catch-up.
- **Source**: `ui/days.rs:21-61, 771-794, 887-1005`; `app.rs:770,1467,1516`; `ui/review.rs:728`.

#### Key numbers for the day
- **Data shown**: agent time; estimated tasks (human-estimate time for the day's estimated tasks, with what agents took and the saving); tasks done; PRs opened; PRs merged (with lines added/removed when known); time waiting on you.
- **Frequency**: daily.
- **Source**: `ui/days.rs:359-373`.

#### Agent time for the week
- **Data shown**: per day of the week (starting on the configured first weekday), agent time stacked by project, with hours scale; each day's date and tasks-done count; hover: date, agent time, done, merged (with lines). The selected day is marked; future days are marked as not yet happened.
- **Actions**: click a past/current day to view it.
- **Frequency**: daily.
- **Source**: `ui/days.rs:375-413, 1011-1074`.

#### Day timeline
- **Purpose**: who worked on what, when, by project.
- **Data shown**: the day's work hours (daemon `from_hour`/`to_hour`, default 9–5) as an hour axis; one lane per project; one bar per task stretch, stacked into rows when they overlap (a task keeps its row). Bar states: working, done, waiting on you, failed, terminal lost, stopped, waiting while you were away — each must be told apart without relying on colour. Very short stretches carry no text. Marks along each lane: commit, question, PR, done, issue found. A "now" line on today. A key for states and marks.
  - **Running**: terminals working in each 10-minute slot and the day's peak; future slots marked "still to come" on hover.
  - **Usage**: plan usage % for the 5-hour and 7-day windows per slot (only when there are readings).
  - **PR host limits** (rare): per host, stretches it couldn't be used (rate-limited / hourly budget used up / easing back in), total time held, reason on hover.
  - **Slot pools** (rare): per project, when all worktree slots were full (times and total), which tasks waited for a slot and how long, which tasks held a slot while idle.
- **Actions**: click a bar to open its task; hover bars, marks, running slots, usage slots and host/slot stretches for detail (task, title, from–to or "now", state; mark time · ref · text).
- **States**: no lanes → "no agent work".
- **Frequency**: daily.
- **Source**: `ui/days.rs:415-528, 1079-1354`.

#### How long tasks took (week)
- **Data shown**: up to 8 of the week's tasks: ref, title, working time then waiting-on-you time (told apart), total, and a state (needs you / so far for today's live tasks, lost, failed) or the wait; a marker for the week's median task time; hover with project and both times.
- **Actions**: open the task (also reachable by Tab; the open task is marked).
- **States**: no tasks this week / that week.
- **Frequency**: weekly.
- **Source**: `ui/days.rs:530-559, 1367-1420`.

#### Done on the day
- **Data shown**: tasks finished on the day: project, ref, title, PR number with its current state (merged / open / declined), finish time; hover with project and PR.
- **Actions**: open the task; show all beyond 12 / show fewer.
- **States**: none finished.
- **Frequency**: daily.
- **Source**: `ui/days.rs:561-575, 1422-1461`.

#### Side cards
- **Notable** (only when something is notable): longest wait on you (when more than one wait), longest task, most terminals at once and when first, busiest hour by events. `ui/days.rs:577-598`.
- **Waiting on you (the day)**: median wait and the usual wait; each wait plotted 0–60m+ with task and minutes on hover; or "nothing waited on you". `ui/days.rs:632-649, 1565-1597`.
- **Agent time by project (the day)**: each project's share, time and %. Only when there is agent time. `ui/days.rs:666-675, 1614-1631`.
- **Hours saved (week)**: cumulative human-estimate minus agent time this week vs last (or shown week vs week before), total, difference, breakdown and ratio; or "no human estimates yet". `ui/days.rs:600-630, 1475-1563`.
- **Tasks done (week)**: total, vs last week, PRs and merged vs last week; per-day bars with last week's level and the selected day marked; hover per day. `ui/days.rs:651-664, 1599-1612`.
- **Planning, last 30 days** (per project, when any): goals and tasks count; added after planning %, failed or retried %, fix rounds per task, questions per task, time against estimate — each vs the half year before, marked better or worse (lower is better). `ui/days.rs:677-706, 1633-1665`.
- **Frequency**: weekly (planning: rarely).

### Weekly time review

Covered here because the brief named it. It is not a PR review.

#### One week's review
- **Purpose:** see where the agents' time went last week and act on the suggested fixes.
- **Data:**
  - **The week.**
    - Its date range.
    - Which earlier week it is compared with.
    - Key numbers:
      - Shipped: tasks, plus goals and releases.
      - Agent time.
      - Time waiting on the owner.
      - Timeouts.
      - Commands that failed and were rerun.
    - Each number shows its change against the earlier week: up or down, whether that change is good, and "no change" when it did not move. Agent time is neutral.
  - **Suggested fixes, as backlog issues.**
    - Each with kind, title, ref, and where it went: in a goal, done, or dropped.
    - A tally: open, in a goal, closed.
    - While filing: a note that it is filing.
    - If filing failed: the reason.
    - With no fixes, the plain suggestions as titles with detail.
  - **Shipped.**
    - Releases cut: name, headline, project.
    - Goals finished.
    - Tasks done, with PR links. Long lists scroll and draw only visible rows.
  - **Where the time went.** By category, with share and change.
  - **Waiting on you.** The reasons, and the longest waits.
  - **Slowest tasks.**
  - **Agent time by project.**
  - **Commands**, as three lists:
    - slow repeated commands, with median, p90 and run counts
    - timed-out commands
    - reruns, saying whether each passed in the end
- **Actions:**
  - Move to the earlier or later week, or pick any week from a list. The list shows each week's total and scrolls past 12.
  - Plan the open fixes in the Backlog. This checks them on the Backlog page alongside anything already checked.
  - File the fixes again after a failure (POST `reviews/{id}/fixes`).
  - Open the related fix issue, task, goal or release, or the PR in the browser.
  - Switch between the day, week and recap reports.
  - Opening an unseen review marks it seen (POST `reviews/{id}/seen`). This withdraws its notification. Its needs-you item goes too, unless filed fixes still wait in the backlog.
- **States:**
  - Loading, or an error with a retry.
  - **No reviews yet:**
    - when the first review comes (day, time, how far away), with the week's days marked up to it
    - or that weekly reviews are off
    - this week's agent time so far, or that there is none yet
- **Frequency:** weekly.
- **Source:** `ui/review.rs` throughout (view model `:228-345`, actions `:362-404`, render `:487-905`).

### Recaps

#### Recap page (weekly and monthly)
- Purpose: a written-up review of a week or month: what shipped, where time went, how planning went, and what was learned. The board writes the page; the app draws it.
- Data shown:
  - The period: a date range for a week, or a month and year. It is labelled as a weekly or monthly recap.
  - Then the page's components, in the order the board chose. Each component appears below.
  - If the page has no opening component, a summary block shows instead: the summary text and highlights.
- Actions:
  - Step to the earlier or later recap; disabled at either end.
  - Open the list of all recaps. Each entry has the period, whether it is a week or a month, and an unseen marker; the current recap is marked. The list scrolls after about 12 and is virtualized.
  - Switch between the day and week views of the Days area. The recap is one view of the Days page.
- States and edge cases:
  - No recaps yet (the first comes on Monday).
  - Loading.
  - Error, with the message.
  - Being written.
  - The page couldn't be written.
  - Opening an unseen recap marks it seen, once. This clears its Needs-you item, withdraws its notification, and drops it from the Days count.
  - Unknown component ids are skipped silently.
  - Components must fit both small and large windows.
  - Some disclosures are grouped so that only one in a group is open at a time.
- Frequency: weekly. A new recap arrives each Monday, and monthly ones arrive each month.
- Source: ui/recap.rs.

#### Recap shared conventions
- Comparisons with the previous period: up or down N against last week or month, or no change.
- Durations: minutes, hours and minutes, or whole hours past 100h. Large numbers use thousands separators.
- Projects: the 4 busiest projects get their own identity; the rest are grouped as Other.
- Command time categories: Tests, Builds, Lint, Git, CI and polling, Other.
- Clickable rows:
  - A task opens the task side panel.
  - A goal opens the goal's page.
  - A release opens that release on its project's page.
- Long lists: past 24 rows the list virtualizes and shows about 12 at a time.
- Goal summaries show the project and the estimated human time. A goal's tasks come from the recap's facts.
- Source: ui/recap/parts.rs.

#### Recap components (by board id)
Each component is optional and placed by the board. Several accept data overrides from the board (props) and otherwise read the recap's shared facts. Many have an expandable detail.

- open (opening): the summary sentence, revealed clause by clause, then the highlights as a list. Expandable Facts (only when there were tasks): tasks, PRs merged, releases, goals finished, active projects, agent time. Source: ui/recap/headline.rs:61.
- hero (headline number): tasks landed, with the change against the previous period. Expandable: merged, no PR, closed counts; tasks by project (top 4 plus Other) with a key. Source: headline.rs:105.
- heroR (release headline): the number of releases, with the change. Expandable: number of projects, days with a release, releases by project. Source: headline.rs:123.
- kpis (key numbers): tasks, PRs, releases, goals and agent time, each with the change against the previous period (hidden when both are zero). Selecting one opens its value by day, for each weekday, or each date in a month. Only one is open at a time. Source: headline.rs:158.
- fact (surprising fact): a big figure and a sentence. The figure is the percent change between two facts, or the value itself when the first is zero. Expandable: the two facts named in words (for example agent time last week, tasks this month) with their values. Source: headline.rs:252.
- quiet (quiet days): a sentence about quiet days. Expandable: every day with its task count. Source: headline.rs:269.
- first (milestones): first-ever releases. The first 3 show, then an expandable list of the rest; each has the project, version and time, and opens the release. Shows the count. Source: headline.rs:282.
- gcards (goals finished): finished goals, largest estimated human time first. Each has the name (opens the goal), project, estimate, finish day, and an expandable task list. Six show by default, then show all or fewer. Empty: 0 goals finished. Source: shipped.rs:29.
- story (goal story): one goal (named by the board, or the biggest): its name, project, estimate, finish day, then tasks finished per day. Expandable: tasks grouped by day. Source: shipped.rs:90.
- spot (biggest win): one goal: project, name (opens the goal), its short summary, and tiles for tasks, PRs, lines added, estimated human time (when known) and agent time.
  - Week: up to 5 of its tasks, then a link to the rest on the goal.
  - Month: up to 5 runner-up goals with their estimates.
  - Source: shipped.rs:123.
- timeline (releases): releases placed along the period, one lane per top-4 project plus Other. Each release shows project, version, time and task count, and opens the release. Day or date labels; release count. Empty: 0 releases. Source: shipped.rs:210.
- train (versions shipped): per project, the first and last version this period, with the release count and last time; opens the latest. Expandable: all releases by project, each opening its release. Source: shipped.rs:265.
- list (shipped): every task, grouped by goal and then tasks without a goal grouped by project, each group with its share.
  - Up to 30 tasks show everything. Above that: 5 goals and 4 projects, then show 10 more goals, then a count of goals and tasks left; extra projects fold into Other.
  - Each group expands to 8 tasks plus a "more" link (to the goal) or a count.
  - Source: shipped.rs:330.
- time (agent time): total agent time and command time by category, each with its share. Empty: 0m. Source: ui/recap/time.rs:60.
- vs (agent vs estimated human): how many times faster agents were than the estimated human time (for example 4.2x). Based on N of M tasks that had estimates.
  - A sentence on what the estimate means in working days, weeks or months.
  - Agent time and estimated human time, in 40-hour weeks.
  - Expandable per project: agent, estimate and ratio.
  - Empty: 0 estimated hours.
  - Source: time.rs:100.
- wait (waiting on you): total time tasks waited on the owner, across N tasks (noting when one task is more than half of it). Split by reason: human tasks, questions, attention, API errors. The top 4 tasks by wait open in the task panel, then up to 10 more. Empty: 0m. Source: time.rs:148.
- usage: the 7-day usage peak and average, and the peak per day (days without samples marked). Empty: no usage samples. Source: time.rs:183.
- days (day by day): tasks per day, with a marker on days with releases. Selecting a day shows its tasks, merged PRs, commits, agent time and that day's releases (each opens). Empty: 0 tasks. Source: ui/recap/trends.rs:21.
- ba (before and after): tasks, PRs, releases, goals and agent time, this period against the previous one. Empty when all are zero. Source: trends.rs:87.
- trend (PRs by week): PRs per week, from the first week with activity. Each week also has tasks and releases on hover. Empty: 0 PRs. Source: trends.rs:136.
- lines (lines changed): lines added and removed. Expandable: the six largest goals by lines, each opening the goal. Empty: 0. Source: trends.rs:169.
- board (projects): up to 8 projects by tasks, then Other. Selecting a project shows its PRs, releases, agent time and up to 6 goals with estimates (each opens the goal). Source: trends.rs:202.
- plan (plan quality): a table for this period, and the previous one when available.
  - Rows: goals, tasks, share added late, share failed or retried, fix rounds per task, questions per task, time against the estimate.
  - A lead sentence for the bigger of "added late" and "failed or retried" (N% of tasks, N of M).
  - Empty: 0 goals planned.
  - Source: trends.rs:253.
- lessons (lessons kept): each lesson has its id, text and how often it was confirmed. Expandable to its full text, kind and date kept. Empty: 0. Source: trends.rs:295.
- issues (found along the way): counts of bugs, gaps, follow-ups and clean-ups found, and the total. Empty: 0 issues found. Source: trends.rs:330.

## 11. Settings

### Settings window

#### Settings window shell
- Purpose: the single place where the owner configures the board and the machine setup.
- Data shown:
  - Nine sections, in this order: Accounts, History, Usage (daily limit), Work hours, QA, Questions, Reviewers, Worktrees, System. Each section has a one-sentence purpose line. The brief mentioned "Pool" and "Jira" sections, but there are none: pool settings live in Worktrees, and Jira lives in Accounts (the connection) and QA (the comment reader).
  - Attention counts on two sections:
    - Accounts counts the connected accounts that need the owner (an error, or a new sign-in is needed).
    - System shows 1 when an update is downloaded and ready.
  - Board connection state: running, not answering, or connecting. There is also a marker when the app runs on the sample (fake) board.
  - If reading the accounts fails, every section shows a notice that the board didn't answer, with the error.
- Actions:
  - Open Settings with ⌘, from anywhere. It opens on Accounts.
  - Deep links open straight to one section:
    - Usage, from the main-window usage block.
    - Work hours, from the main-window hours readout.
    - System, from the "update ready" signal in the main window or Midna's status line.
  - Choose a section; this reloads that section's data.
- States and edge cases:
  - Only one Settings window exists. Opening it again brings it forward, except in hidden test runs.
  - Default size is about 900x680, with a minimum of 720x460.
  - Data refreshes when the window becomes active, when a section is chosen, and on a timer: every 30s, or every 2s while a GitHub browser sign-in is waiting.
- Frequency: rarely. It is used for setup and occasional tuning.
- Source: settings.rs:65-121 (sections and descriptions), 897 (no-answer notice), 910-971 (counts and connection state); actions in app.rs (OpenSettings, OpenUsageSettings, OpenHoursSettings, OpenSystemSettings).

### Accounts

#### Service accounts (GitHub, Bitbucket, Azure DevOps, Slack, Jira)
- Purpose: give the board (and the agents, through the board) access to the services it acts on. A missing or broken account means some work waits.
- Data shown, per account:
  - Connection state, one of:
    - Not connected.
    - Connected: who it is signed in as and when the token was last checked and worked. For Azure this is the organization, not the user. For Jira it is the site and whether QA comments are on.
    - Refused: when the service refused the sign-in, plus the consequence. The raw error is available on hover or detail. Consequences:
      - GitHub and Bitbucket: agents can't push.
      - Azure: PR checks lose their failed pipeline steps.
      - Jira: tickets and QA comments wait.
      - Slack: no Slack messages.
  - Needs a new sign-in: connected but missing required scopes. Lists the missing scopes.
  - Partial scopes (information only): connected and working, but an optional scope is missing. It says what doesn't work, for example Bitbucket without read:user can't show the user's name, and lists the missing scopes. This does not count toward the attention count.
  - Error while not connected: an error line on an account that isn't connected.
  - A per-account result note: success (connected or signed in as whom) or the error from the last action.
  - A general note that tokens are stored in the login Keychain.
- Actions:
  - Connected accounts:
    - Check: asks the service whether the token still works. When the sign-in was refused, this becomes Sign in again: GitHub reruns the browser sign-in if `gh` is installed; the others reopen their token form.
    - Disconnect: an inline confirmation states what forgetting the account stops (for example, agents can't push). Cancel or confirm. It never touches `gh` or git credentials on the Mac.
    - Needs a new sign-in: GitHub offers a browser sign-in or a new token; Bitbucket and Azure offer a new token.
  - Not connected: Connect opens the account's form and focuses its first field. The same control cancels and closes the form.
  - Form fields per service:
    - GitHub, when the `gh` CLI is installed: two extra paths. Use gh's existing sign-in (imports its token), or browser sign-in (device flow, below). Without `gh`, a note suggests pasting a token or installing gh with brew.
    - GitHub token: a classic token with repo, read:org and workflow; or a fine-grained token with Contents, Pull requests and Workflows read and write.
    - Bitbucket: Atlassian email and API token (repository read/write, pull request read/write, read:user, pipeline write), with a link to create a token.
    - Azure DevOps: organization (a bare name or a dev.azure.com URL) and a PAT (Build Read, Test Management Read), with a how-to link.
    - Jira: if no Jira site is set in the board's config file (~/.config/taskboard/config.toml), the form explains how to set it and has no fields. Otherwise: the email used for that site and an API token, with a link to create one.
    - Slack: a user or bot token (xoxp- or xoxb-), with a link to the Slack apps page.
  - Keyboard: Enter submits the form; Esc clears the focused field.
  - Agent commands (Accounts only, can be shown or hidden): the copyable CLI lines agents use for each account (`tb accounts`, plus a per-service `tb api …` or `tb pr status`), each with copy.
- States and edge cases:
  - Loading, while asking the board.
  - On success the token field clears, the forms close, and a "connected as" note shows.
  - On refusal the typed token stays so it can be fixed, and the error shows under the account.
  - The board can be asked for a fresh check (`?fresh=1`), but the UI doesn't use this.
- Frequency: setup-only, plus rarely when a token expires or is refused (the health area points here).
- Source: settings.rs:137-171 (consequences, forget wording, status line), account forms and handlers throughout; endpoints GET accounts, POST accounts/<id>, /check, /disconnect.

#### GitHub browser sign-in (device flow)
- Purpose: sign in to GitHub without making a token by hand.
- Data shown:
  - While a code is being fetched: getting a code.
  - Then: the one-time code, a note that it's copied, and steps (paste it on GitHub's page, continue, authorize).
  - After the page is opened: waiting, it updates itself, and the code is valid for 15 minutes.
- Actions:
  - The code is copied to the clipboard automatically, once.
  - Clicking the code copies it again (a "copied again" confirmation shows for about 1.5s).
  - Open GitHub's device page (github.com/login/device), which also re-copies the code. After the first open it becomes open again.
  - Cancel ends the sign-in.
- States and edge cases:
  - Polls every 2s while waiting.
  - Success leaves a "signed in as @user" note.
  - Only offered when `gh` is installed.
- Frequency: setup-only.
- Source: settings.rs (device flow block); POST accounts/github/login, /github/import, /github/cancel.

### History

#### Retention of board history
- Purpose: control how long the board keeps detailed history and day summaries, and so how much disk it uses.
- Data shown:
  - Two retention settings. Their choices come from the board: four each in current builds.
    - Every event: commits, questions, checkpoints and status changes of finished tasks. These feed the day timeline marks and the task log.
    - Day summaries: one record per day per project. These feed the timeline bars and week totals.
    - Values are shown as durations: 0 = forever, 365 = 1 year, 730 = 2 years, multiples of 30 at 180 or more as months, otherwise days.
  - Space used: number of events and their size, number of day summaries and their size (KB or MB).
  - Cleanup: it runs nightly after 3 AM. Shows the last run time, or that it hasn't run yet.
- Actions:
  - Change a retention value. The board first previews the change:
    - If nothing would be removed, it saves at once.
    - Otherwise a confirmation states that the next cleanup (after 3 AM) removes N events and the summaries of N days, permanently. Cancel, or change and remove.
  - Clean up now. The result is either the number of events and day summaries removed, or that nothing was old enough.
- States and edge cases: busy while working; an error note on failure.
- Frequency: rarely.
- Source: settings.rs (History section); GET history, POST history (preview, then save), POST history/cleanup.

### Usage (daily limit)

#### Daily share of the weekly usage limit
- Purpose: stop the board starting new agent work once today's share of the weekly AI usage budget is used, so the week doesn't run out early. Agents already running carry on; the board starts nothing new until midnight.
- Data shown:
  - Base share, one of: Off (default), Even, 10%, 15%, 20%, 25%, plus any custom value set with `tb usage daily`, placed in order.
    - Even splits what's left of the week over the days until the weekly reset.
    - A percent gives each day that share of the week.
  - Only when on:
    - Unused share: carries to the next day (default) or is spread over the remaining days.
    - Today: a status sentence from the board. Before any reading it says there is no 7-day reading yet. It is shown as a warning when today is limited.
- Actions:
  - Change the base share; a change keeps what today has already used.
  - Change how the unused share carries.
  - Today's override: limit today or lift the limit for today.
- States and edge cases:
  - The board refuses bad values (for example a share outside 1 to 100) and shows the error.
  - No reading yet.
- Frequency: rarely to set. The today status and override are glanced at daily when usage runs hot.
- Source: settings.rs (Usage section); GET usage, POST usage/daily, POST usage/override.

### Work hours

#### Work hours schedule
- Purpose: only start agents during the owner's working hours, and let today end earlier or later than usual.
- Data shown:
  - On/off (only start agents during work hours), with the board's sentence about the current state.
  - Start and end times.
  - Days of the week, in week order. The week starts on the configured first weekday (default Sunday). Each day has its full name on hover.
  - Today until: a one-off end time for today only. Empty means the usual end. A time earlier than now means tomorrow. Shows whether it ends today or tomorrow, and at what time.
- Actions:
  - Turn on or off. Times and days are disabled while off.
  - Edit the start or end time, toggle days, and set or clear Today until.
  - Every change saves at once. Today until is only sent when it changed; clearing it turns it off.
- Time entry rules (12-hour; the owner never sees 24-hour time):
  - Digits only, with the colon inserted automatically. Each field has its own AM/PM switch.
  - Typing a, p, am or pm sets the half. Hours 0 and 13–23 are read as 24-hour input and converted.
  - While typing, a faint completion shows (for example 7 becomes 7:00).
  - In Today until, a bare hour means its next occurrence (at 10 PM, typing 1 means 1 AM).
  - Commits on blur or Enter. Esc reverts. Invalid input reverts.
  - Today until shows no AM/PM switch while it's empty and unfocused; when unset, it defaults to PM.
  - The board's values never overwrite a field while it is being typed in.
- States and edge cases:
  - If the board refuses a save, the saved hours are restored and "not saved" shows with the reason (for example, start and end are the same: turn work hours off instead). Try again resends.
- Frequency: rarely for the schedule. Today until may be used daily (stay late, stop early).
- Source: ui/hours.rs (page, time field, save/retry); POST hours.

#### Keep-awake (read-only)
- Purpose: show whether the Mac is kept awake for agents (the Midna keep-awake feature).
- Data shown: whether keep-awake is holding, a status sentence, and the schedule (only when enabled).
- Actions: none in the app; it changes with `tb keep-awake`.
- Frequency: rarely.
- Source: ui/hours.rs (keep-awake block, on the Work hours page).

### QA

#### Read QA comments from Jira
- Purpose: turn QA feedback left as Jira comments on the board's tickets into work, or into questions for the owner.
- Data shown:
  - On/off; off by default.
  - When on: comments read, how many are waiting on the owner, and when Jira was last checked (or not yet).
- Behaviour:
  - Every few minutes the board reads new comments on its tickets.
  - A clear direction becomes a high-priority follow-up task.
  - A question waits on the goal's QA view and in alerts.
  - Comments from before it was switched on are never read.
- Actions: turn on or off.
- States and edge cases: disabled unless Jira is set up in the board's config file; a warning explains this.
- Frequency: setup-only. The counts are glanced at rarely.
- Source: settings.rs (QA section); GET qa, POST qa.

### Questions

#### Send back buried questions
- Purpose: make sure a question an agent asks in the middle of a reply reaches the owner as a proper Question.
- Behaviour: when a task's reply asks something above its last paragraph, the board sends it back once, so the agent re-asks it with `tb question` or a final Questions block. It then reaches the owner as a Question card.
- Data shown: on/off; on by default.
- Actions: turn on or off.
- Frequency: setup-only.
- Source: settings.rs (Questions section); GET and POST buried-questions.

### Reviewers

#### Reviewer roster (read-only)
- Purpose: show who the board asks for code reviews per project, and how they behave.
- Data shown, per project, a list of reviewers with:
  - Name.
  - User, emails and aliases.
  - Pinned (always asked).
  - For bots, how often they review (every N hours).
  - An automation factor, when it isn't 1.
  - Median time to review.
  - Open asks.
  - When last asked.
  - Removed, with the reason. Removed reviewers are marked as removed and never asked.
- Actions: none in the app. Agents and `tb reviewers` change the roster.
- States and edge cases: loading; no reviewers.
- Frequency: rarely.
- Source: settings.rs (Reviewers section); GET reviewers?project=all.

### Worktrees

#### Worktree pool settings (per project)
- Purpose: control how many isolated checkouts (slots) each project keeps for agents, and how idle ones are cleaned up.
- Data shown and settings:
  - Minimum and maximum slots.
    - Minimum choices run from 0 to the smaller of max and 12.
    - Maximum choices run from the larger of min and 1, up to the larger of the current max and 12.
    - Setting min above max is refused.
  - Clear build output after: 1 day, 3 days, 7 days, or never.
  - Remove an idle slot after: 1 day, 7 days, 30 days, or never.
  - A custom value set with `tb project set` is kept among the choices.
  - The pool also has a low-disk threshold (`low_disk_gb`), which the UI does not show.
- Actions:
  - Change any value; it saves at once.
  - Invalid combinations are offered but disabled, with the reason:
    - Clearing build output can't come later than removing the slot.
    - Removing can't come before clearing.
  - The count menu closes on an outside click.
- Frequency: rarely.
- Source: settings.rs (Worktrees section); GET slots (loaded only while this section shows), POST projects/<name>.

#### Worktree slots (per project)
- Purpose: see disk use and who holds each checkout.
- Data shown:
  - Each slot: its name, size on disk, when last used (a time today, or a date and time), and either the task holding it (with the task title on hover) or free.
  - Tasks waiting for a slot: how many, and which.
  - Headings name the project when there are several projects.
- Actions: none.
- States and edge cases: loading; no slots.
- Frequency: rarely. Useful when disk is low or tasks are stuck waiting.
- Source: settings.rs (Worktrees section).

### System

#### App updates
- Purpose: keep the Mac app current, and apply a downloaded update at a convenient moment.
- Data shown:
  - Update state, one of:
    - Off, with the reason: the sample board doesn't update; a dev build not run from Taskboard.app; Taskboard Dev updates with a script; or not a signed release build.
    - Not checked yet.
    - Checking.
    - Up to date, with when it was checked.
    - Downloading a version (it is checked before it is offered).
    - Ready: the version, plus a note on what a restart does. Agents keep running in Midna, the board is offline for about 10 seconds, and N code reviews in progress start again; the count comes from the board.
    - Applying (the app reopens in a moment).
    - Failed: what failed (couldn't check for updates, or couldn't download, check or install a version), the error, and when.
  - Channel: updated to a version today at a time; or that betas are included (when the running version is a beta); or public releases only.
  - Release notes link: the ready version's page, or the releases page.
  - App version.
- Actions:
  - Check now (in the Not checked, Up to date and Failed states).
  - Restart to update (Ready).
  - Try again (Failed).
  - Open release notes.
- Behaviour: the first check is about 20s after launch, then every 6 hours, plus on demand.
- States and edge cases: a failed restart-to-update leaves an error note here.
- Frequency: rarely, about once per release.
- Source: settings.rs (System section); updater.rs:36-56 (states, failure titles), 242-256 (off reasons), 319-362 (schedule, check now, restart note).

#### Board (daemon) status
- Purpose: show whether the background board is running the same version as the app, and restart it.
- Data shown: the board's version, its uptime and process id, or that it's not connected or not running.
- Actions:
  - Restart the board. Only offered when the app runs from an installed bundle. It restarts the launch agent, waits about 2s, then reloads the info.
  - On failure: couldn't restart the board, with the error.
- States and edge cases: when the board's version differs from the app's, a warning says it doesn't match, and restarting is promoted to the main action.
- Frequency: rarely, mainly right after an update.
- Source: settings.rs (System section); install.rs:172 (installed bundle), 258 (restart via launchctl kickstart); GET info, GET restart.

## 12. Shared behaviours

### Toasts and undo

#### Confirmation toasts
- **Purpose:** Confirm that an action happened.
- **Data shown:** A short sentence naming what changed.
- **Actions:**
  - None needed. Toasts go away after about 3 seconds.
  - A new toast replaces the previous one.
- **States and edge cases:** Some toasts carry an extra action:
  - Open the task just made from a backlog item.
  - Reopen a kept new-idea draft (⌘N).
- **Frequency:** constantly.
- **Source:** app.rs (toast).

#### Undo toasts
- **Purpose:** Make destructive or surprising changes reversible.
- **Data shown:** What was done.
- **Actions:**
  - Undo, also with ⌘Z or from the Edit menu.
  - Close the toast.
- **States and edge cases:**
  - They last about 6 seconds.
  - Only the newest toast that can be undone takes ⌘Z.
  - Actions that offer undo include:
    - Dismissing a Needs you item.
    - Marking a human to-do done.
    - Telling agents not to wait.
    - Closing a backlog item as won't do, when the server returns an undo.
    - Finishing a goal without a miss, when the server returns an undo.
- **Frequency:** daily.
- **Source:** app.rs, ui/home/groups.rs, ui/home/view.rs.

#### Error toasts
- **Purpose:** Say plainly why something failed.
- **Data shown:** The error, reworded into plain language:
  - When the board can't be reached, it says to check that the board is running and try again.
  - Raw technical text becomes a short "try again in a moment".
  - A server-side failure says the board hit a problem.
  - Anything else is shown as is, ending with a full stop.
- **Actions:** Dismiss.
- **States and edge cases:**
  - They stay until dismissed.
  - At most 3 show at once.
  - A repeat of an error already showing is not added again.
  - Each one is announced to VoiceOver.
- **Frequency:** rarely.
- **Source:** ui/status.rs (plain_error, announce), app.rs.

#### Confirm before risky actions
- **Purpose:** Ask before an action that can't easily be taken back.
- **Data shown:** What will happen.
- **Actions:** Confirm or back out. Esc backs out.
- **States and edge cases:** Used for:
  - Force-closing a busy terminal.
  - Mark done, when it is not straightforward.
  - Cancel task.
- **Frequency:** weekly.
- **Source:** menu_bar.rs, ui/home/view.rs, ui/kit.rs (modal shell).

#### "Sending" state on pressed actions
- **Purpose:** Show that a press registered, and stop double sends.
- **States and edge cases:**
  - A pressed action shows that it is sending for at least 600 ms.
  - A second press while sending does nothing.
  - On failure, the choices come back.
- **Frequency:** constantly.
- **Source:** app.rs (settle), ui/home/view.rs.

### Status vocabulary

#### Task statuses
- **Purpose:** One shared set of words for where a task is.
- **Data shown:**
  - **Statuses, in rank order:** needs you, failed, working, in review, pending merge, stopped, queued, planned, done.
  - **Sub-states:**
    - Lost, stopped, couldn't start, couldn't compact.
    - Question, needs you.
    - Compacting, maybe stuck.
    - Working, starting, queued.
  - **PR sub-states:**
    - Needs setup.
    - Checks running, checks failed.
    - Conflicts, comments.
    - Couldn't merge.
    - Ready to merge.
    - Pending merge after another task.
    - In review, k of n approved. Each reviewer shows approved, changes requested, or pending.
- **The line under a task, by state:**
  - Terminal lost, with detail.
  - The question.
  - Maybe stuck, with detail.
  - Starting, with detail.
  - Why it is queued.
  - When done, one of: the PR was merged, the PR was declined, or the commit it landed in.
- **The age shown:**
  - While running, how long it has run.
  - While asking, how long since it asked.
  - When finished, the time it finished.
- **Frequency:** constantly.
- **Source:** ui/status.rs, ui/home.rs.

### Accessibility and interaction

#### Keyboard focus
- **Purpose:** Run the whole app from the keyboard.
- **Actions:**
  - Tab and Shift-Tab move between controls and show where focus is.
  - Return or Space presses the focused control.
- **States and edge cases:**
  - Inside a dialog, focus stays in the dialog.
  - While a text field is focused, Esc and plain keys belong to the field.
  - Unavailable controls are skipped.
  - In a multi-line field:
    - Shift-Return adds a new line.
    - ⌘↩ submits.
  - In a one-line field, Return submits.
- **Frequency:** constantly.
- **Source:** ui/focus.rs, ui/kit.rs (Input).

#### Text selection anywhere
- **Purpose:** Copy any text on a page, not just text in fields.
- **Actions:**
  - Drag to select, across several pieces of text.
  - Copy with ⌘C.
  - A plain click on a link inside selectable text still opens it.
- **Frequency:** daily.
- **Source:** ui/select.rs.

#### Truncated text reveals on hover
- **Purpose:** Never lose information to truncation.
- **States and edge cases:** Text that is cut short shows all of it on hover, and only when it was actually cut.
- **Frequency:** daily.
- **Source:** ui/kit.rs (clipped).

#### Hover cards
- **Purpose:** Secondary detail on demand.
- **Data shown:** Examples include:
  - Exact times behind ages.
  - Midna's last contact.
  - The full list of health problems.
  - Stream details.
  - The full hours line.
  - Settings shortcut hints.
- **Frequency:** daily.
- **Source:** ui/kit.rs (tip, card_tip), various.

#### No context menus
- Nothing in the app offers a right-click menu today. Every action is reachable from the object itself, its more-actions list or the menu bar.
- **Source:** whole crate (no secondary-click handlers).

#### Motion
- **Purpose:** Make changes noticeable without being jarring.
- **States and edge cases:**
  - Entrances fade in.
  - Lists stagger.
  - Progress grows into place.
  - Numbers count up.
  - Sections expand.
  - All of it is turned off when the Mac's reduce-motion setting is on.
- **Frequency:** constantly.
- **Source:** ui/motion.rs, main.rs:87-88.

#### VoiceOver announcements
- **Purpose:** Screen-reader users hear errors.
- **States and edge cases:** Each error toast is spoken.
- **Source:** ui/status.rs (announce).

#### Paged long lists
- **Purpose:** Handle very large lists smoothly.
- **States and edge cases:**
  - Long lists render only what is visible.
  - The Ideas page loads one page at a time and fetches older pages as the owner nears the end.
  - A refresh fetches the first page again, plus any loaded rows that may have changed.
- **Frequency:** constantly.
- **Source:** ui/paged.rs.

### Formatted text

- **What it renders:**
  - paragraphs, with line breaks kept
  - headings
  - bullet and numbered lists
  - fenced code
  - inline code, bold, italic, markdown links and bare http(s) links
  - Text is cut at a maximum length, preferably at a late line break, and ends with an ellipsis.
  - Text is selectable, and links are clickable.
- **Used by:** the idea detail and the goal review findings (`ui/idea.rs:1374`, `ui/goal/review.rs:115`).
- **Not used by:** the task panel. Its brief, notes and tester note use the panel's own renderer, described under The brief (`ui/task_panel/view.rs` `note_body`).
- **Frequency:** n/a (shared capability).

### Local preferences and developer-only behaviour

#### Local UI preferences
- Purpose: remember view state between launches on this Mac only.
- Data stored:
  - Main filter.
  - The page list's size, whether it is hidden, and the recent list.
  - Linked state.
  - Folded sections.
  - Unsent drafts, per key.
  - Collapsed sessions.
- Where: app-state.json in the Taskboard data folder (or $TASKBOARD_DATA). Written atomically. Not written for sample or test boards.
- Actions: none directly; each owning area writes its own key. These keys belong to the page list, filters, drafts and sessions areas.
- Frequency: constantly, in the background.
- Source: prefs.rs.

#### Hidden screenshot runs (developer-only)
- Purpose: let agents capture app screens without disturbing the owner.
- Behaviour:
  - In debug or snapshot builds with SLATE=1, windows open off screen, never take focus, and the app never activates.
  - A page variable (TASKBOARD_PAGE=settings) captures the Settings window.
  - TASKBOARD_NO_ACTIVATE also prevents activation.
- Not an owner feature; it is listed only so it isn't mistaken for one.
- Source: hidden.rs.

## 13. Keyboard shortcuts

| Keys | Action | Where |
|---|---|---|
| ⌘1 | Needs you | anywhere (not in a dialog) |
| ⌘2 | Projects | anywhere |
| ⌘3 | Goals | anywhere |
| ⌘4 | Backlog | anywhere |
| ⌘5 | Ideas | anywhere |
| ⌘6 | Days | anywhere |
| ⌘7 | Releases (when on) | anywhere |
| ⌘K | Search palette | anywhere (not in a dialog) |
| ⌘N | New idea; in its open dialog, keeps the draft and refocuses the title; after a link closed it, reopens the kept draft | anywhere |
| ⌘R | Refresh now | anywhere |
| ⌘[ or ⌘← | Back | outside text fields (⌘← moves the cursor in a field) |
| ⌘] or ⌘→ | Forward | outside text fields |
| ⌘Z | Undo the newest undoable toast | outside text fields (the menu item works anywhere) |
| ⌘⇧C | Copy the open task's, issue's or goal's reference | anywhere |
| ⌘C | Copy selected text | anywhere |
| ⌘↩ | Open the open task's PR; in a multi-line field, submit; in search, open the selected task's PR | context |
| ⌘D | Dig into the open idea | idea open |
| ⌘⇧P | Plan the open idea | idea open |
| ⌘T | Talk over the open idea | idea open |
| ⌘, | Settings | anywhere |
| ⌘W | Close the window (quits the app) | anywhere |
| ⌘M | Minimize | anywhere |
| ⌘H / ⌘⌥H | Hide / hide others | anywhere |
| ⌘Q | Quit | anywhere |
| Esc | Close the top layer (picker, dialog, secondary detail in a panel, panel, backlog selection, Needs you filters); cancel a field's form | anywhere |
| Tab / ⇧Tab | Move focus (stays inside a dialog) | anywhere |
| Return / Space | Press the focused control | focused control |
| Return | Submit a one-line field or to-do note | text field |
| ⇧Return | New line | multi-line field |
| ↑ / ↓ | Move the selection (wraps) | search palette, pickers |
| Return | Open the selected result | search palette, pickers |
| Backspace in an empty field | Widen a goal-scoped search to everything | search palette |

## 14. Daemon API cross-check

What the daemon (`crates/taskboardd`) can do, checked against every request the app makes. It lists what an owner can reach only from the `tb` command line or through odd paths, so the redesign can decide what to bring into the app. The route map at the end is the evidence.

### Owner-facing capabilities not in the app (CLI or agent only)

#### Creating things
- Create a task, a goal, a backlog issue, a goal note, a lesson, a release. The app can only make an idea (New idea), promote an issue into a task, and make a goal from planned backlog issues. Everything else is `tb add`, `tb goal`, `tb found`, `tb note --goal`.
- Edit task/goal/issue fields beyond a few toggles: title, detail, priority, project, waits-for, device needs, locks, lane, outcome, "after" chains, goal order, delete.

#### Goals and waves
- Wave review stops: set or name a wave's stop (`goals/:id/waves/:n`). The daemon accepts this only from the app (as the owner's own click), yet no app control sets one today; the app can only continue past a stop. The redesign needs a way to add, name and remove a review stop after a wave. Hold a wave. Accept a goal's outcome. Bring a set-aside goal back. Reorder goals. Ask for an outcome/plan review on demand.

#### Releases
- Cut a release, a pending cut waiting on tasks/goals, cancel the pending cut, move work between releases, mark one failed, edit notes. The app's Releases pages are read-only.

#### Lessons and planning quality
- Lessons: list, search, keep or retire suggestions, confirm, edit, run a goal retrospective. Purpose: what the board learned; only kept lessons reach later handoffs and planners. Owner is the only one who keeps/retires. App shows only a "lessons kept" count in recaps.
- Planning rules in force (`planning`), planning lessons, and planning quality per project (tasks added late, failed/retried, review rounds, questions, agent vs estimated developer time).

#### Master breaks
- Watch on each project's default branch: when CI goes red a break M<n> opens, the board works out whether it's the owner's, closes when green. Owner gives verdicts (mine / not ours…), checks now. App shows them only read-only inside PR watch.

#### PR and CI controls
- PR builds stop/resume board-wide (CI time scarce: cancels builds on owner's PRs while on). Lift host rate-limit holds. Refresh PR feed. Switch stream mode. Per-project PR rules: approvals needed, expected/required checks, when to ask the owner, agents may merge, owner review required.
- Reviewer roster per project: add, sync from host, remove/restore, pin, auto-assign level, bot schedule, aliases, merge duplicates. App only reads the roster and a task's ask ledger.

#### QA comments
- Jira comments from testers, classified by Claude into a follow-up task, an answer, or nothing; owner can act on or ignore each. App has only the on/off setting; the comments themselves are unreachable.

#### Jira desk
- Background terminal that finds/creates tickets; its jobs (J<n>) state and failures. No app view.

#### Start grants
- Standing permission for terminals opened by a named Midna trigger to start the tasks their conversation made. Granted via a notification (Grant / Not now) or `tb start-grant add`; list/remove is CLI only.

#### Keep-awake
- Midna keeps the Mac awake on the work-hours schedule; on/off, mode, battery floor, linger, off today. App shows it read-only.

#### Status bar
- Midna status bar line for the selected terminal (its task, goal, line, today's usage share); options goal/title. CLI only.

#### Context limits
- When agents compact, when a cold conversation must compact before resuming, when it's too big/old and starts fresh from the handoff, max compactions, generated-file globs. CLI only.

#### Locks and slots
- Named locks / run-alone tasks currently held and who waits. Discard a slot (save its changes to a backup and free it). Slot pool listing is in Settings but discard is not.

#### Devices and bits
- Device pool management (add/tag/remove/focus command; who borrows what; waiting list). Goal device needs. Feature flags (bits): local vs backend, made in the flag tool or not; a finished goal waits on its unmade backend bits.

#### Terminals
- Closed terminals list and reopen; rename; close one/many; close done tasks' terminals; board-wide terminal cap.

#### Misc
- Usage: app has daily share and override; the raw usage detail is shown via state.
- CI token (Azure), areas backfill, tidy the backlog (dupes/stale), mark an issue critical, move it, note it, `start-misses` (why starts were refused), `summary`, `lanes`, checkpoint request to a running agent, attach, edit or remove files on tasks and goals (the app only opens them).

### Reachable only through odd paths

Deep links (`taskboard://…`, followed at startup with `TASKBOARD_LINK`; web-board forms `#/?task=T6`, `tasks#/goals/G5` also read)
- `task/T12` opens the task; `task/T12?do=done` and `?do=cancel` open the Finish confirm (Mark done / Cancel). This is how Midna hover cards and notification-style links finish a task.
- `issue/B7`, `goal(s)/G3` (`?view=backlog` opens the goal's backlog view), `goals`, `idea(s)/I3`, `ideas`, `session/<id>` (focus the terminal in Midna), `backlog` or `plan`, `days`, `review(s)/<id>`, `recap(s)/<id>`, `tasks` (Projects), `projects/<name>`, `release(s)/R2`, `releases`.

#### Midna (the terminal app) integration
- Link patterns turn T/H/G/B/R/I refs in agent terminals into clickable taskboard:// links.
- Link menus: task ref (Copy, Go to its terminal), goal ref (Copy, Open its backlog), issue ref (Copy); a PR link gets "Open T<n>".
- Hover previews (`tb preview`): status, key fields, actions Open its PR, Mark done, Cancel task….
- Tab statuses on terminals: In review, Done, Question, Alert, Urgent alert, Task failed, Waiting (Midna's needs-you).
- Status bar script: selected terminal's task, goal, latest line, today's usage share; click opens Days.

#### macOS menu bar (app menu, not a status item)
- Focus in Midna, Open PR, Start task, Stop task (force close confirm), Done task (Finish confirm), idea Dig/Plan/Talk/Shelve/Drop/Reopen, Check for updates, Install hooks, Restart daemon, Open log, Export for debugging (zip), Undo (backlog toast undo), Settings / Usage settings / Hours settings.

#### Environment variables (dev/test only)
- Backend: TASKBOARD_BACKEND (fake), TASKBOARD_URL, TASKBOARD_DATA. Opening state: TASKBOARD_PAGE, TASKBOARD_OPEN, TASKBOARD_LINK, TASKBOARD_TAB, TASKBOARD_PROJECT, TASKBOARD_SEARCH, TASKBOARD_PICK, TASKBOARD_GOALS_OPEN, TASKBOARD_GOAL_VIEW, TASKBOARD_GOAL_NOTES, TASKBOARD_GOAL_ATT_WAVE, TASKBOARD_GOAL_REVIEW_OPEN, TASKBOARD_PROJECTS_OPEN, TASKBOARD_RELEASES_OPEN, TASKBOARD_RELEASES_URL. Window: TASKBOARD_W/H, TASKBOARD_SETTINGS_W/H, TASKBOARD_NO_ACTIVATE, TASKBOARD_THEME, TASKBOARD_TL. Misc: TASKBOARD_DEV, TASKBOARD_NO_TB_LINK, SLATE. Fake data: TASKBOARD_FAKE_* (Part 2).

### Notifications the daemon sends

(via Midna; categories "Task board" and "Task board asks", the latter stays until answered)
1. Alert: title = the alert text; body "Task board" or "Task board · urgent"; click opens the alert's terminal/task/goal; up to 4 snooze actions from config (e.g. Snooze 15 min, Snooze 1 hour). Only for alerts that wait on the owner or are urgent; held outside work hours unless urgent; repeats every N minutes.
2. Digest (when configured): "T12 needs you" or "N things need you", lists up to 5, opens Needs you, snooze actions; withdrawn when nothing is left.
3. Start ask: "Start T12?" naming the asking terminal and its prompt, with an outside-hours note; actions Start / Not now.
4. Start-grant ask: "Grant trigger 'X' a standing start?" actions Grant / Not now; failure notice "Trigger 'X' wasn't granted: …".
5. Weekly review ready: headline, opens the review; also a quiet Needs-you item.
6. Recap ready: "Weekly recap: …" / "Monthly recap: …", "N tasks finished", opens the recap.
7. Plain notices: a QA comment became a task; out of usage, N terminals closed; today's daily usage share used up.
Alert sources include: agent questions, out-of-scope asks, PR stages/held/claims/declined, merged-note waits, human tasks, waits-for cycles/diverged order, stacked PR problems, stuck/dirty slots, setup gaps, Jira failures, start jobs that didn't start, delivery failures, code review turned back, review triage, same-PR critical asks, quiet agents, goal misses, pending release cut, master breaks (urgent), feed problems, PR builds stopped, PR pace, reviewer roster asks, and manual `tb alert raise`.

### Route map

Source: crates/taskboardd/src/api.rs (route table, `route()`), its sub-routers (devices, bits, lessons, ideas, releases, reviewers, breaks), docs/API.md, and every `get(`/`post(` path in crates/taskboard-app/src (tests excluded). All API paths live under `/tasks/api/`. POSTs need `X-Task-Board: 1`; the app also sends `X-Task-Board-From: app` (plus the app token or the app socket), which the board treats as the owner's own click (needed for Start and wave review stops). `GET /tasks/files/<id>` serves attachment files.

"App" column: the app file that calls it, or "no". "Owner" column: whether it is something the owner (not an agent or relay) would use.

#### Board state, info, health
| Route | Purpose | App | Owner |
|---|---|---|---|
| GET state | Whole board snapshot (tasks, goals, alerts, usage, accounts, Midna, stream, master, qa…) polled by every view | app.rs, settings.rs | yes |
| GET info | Daemon version, data folder, config | settings.rs | yes |
| GET restart | Restart history / why the daemon restarted | settings.rs, ui/health.rs | yes |
| GET summary | Short text summary of the board | no | yes |
| GET whoami / steps / handoff / jobs | Agent-side: which task this terminal is, its steps, its handoff, start jobs | no | no |
| POST api-call | Agent proxy for host API calls (rate-limited) | no | no |
| GET hooks/payload | Hook payload for Claude hooks | no | no |
| GET start-misses | Recent `tb start` refusals (why a start was refused) | no | yes |
| GET search | Search tasks/goals/issues/ideas/sessions (`palette=1` for the command palette) | ui/search.rs | yes |
| GET home / POST home/talk | Needs-you page data; open a Claude terminal seeded with the Needs-you context | app.rs; ui/home/view.rs | yes |
| GET lanes | Tasks page: loose tasks per lane (queued/running/needs/done today) and area counts | no (app uses GET tasks) | yes |
| GET days | Day timeline: what each task did per day, hidden rows | app.rs | yes |
| GET/POST history, POST history/cleanup | Retention of detail/summary days; preview/apply; clean up now | settings.rs | yes |

#### Accounts, tokens
| Route | Purpose | App | Owner |
|---|---|---|---|
| GET accounts | Connected services (GitHub, Slack, Jira…) with health | settings.rs | yes |
| POST accounts/github/{login,cancel,import} | GitHub device sign-in, cancel, import from gh | settings.rs | yes |
| POST accounts/:id, accounts/:id/{check,disconnect} | Save token, test it, disconnect | settings.rs | yes |
| GET/POST ci-token, POST ci-token/clear | Azure DevOps CI token for build reads | no | yes |

#### Projects, slots, locks
| Route | Purpose | App | Owner |
|---|---|---|---|
| GET projects | Projects with their rules | no (from state) | yes |
| POST projects/:name | Project rules: PR flow, approvals, expected/required checks, ask stage, swap, review, releases on/off, worktree pool (min/max slots, idle trim/remove days) | settings.rs (pool fields only) | yes |
| GET/POST projects/agents-merge | Whether agents may merge their own PRs, per project | no | yes |
| GET/POST projects/owner-review | Whether the owner must review before merge, per project | no | yes |
| GET slots, slots/at, slots/sharing | Worktree slot pool: who holds what, which slot a path is, slots shared | settings.rs (slots) | yes |
| POST slots/discard | Save a slot's changes to a backup ref and free it | no | yes |
| GET locks | Named locks and run-alone tasks currently held/waiting | no | yes |
| POST areas/backfill | Assign existing cards to a project's areas | no | yes |

#### Terminals (sessions)
| Route | Purpose | App | Owner |
|---|---|---|---|
| GET sessions (`project=all`) | Live Midna terminals | app.rs (palette) | yes |
| GET sessions/closed | Recently closed terminals | no | yes |
| GET sessions/:id | One terminal | no | yes |
| POST sessions/:id/focus | Raise a terminal in Midna | app.rs, task_panel.rs | yes |
| POST sessions/:id/{close,rename,reopen}, sessions/close | Close, rename, reopen one; close many | no | yes |
| GET/POST terminals | Board-wide cap on open terminals | no | yes |
| POST done/close-terminals | Close terminals of done tasks | no | yes |

#### Tasks
| Route | Purpose | App | Owner |
|---|---|---|---|
| GET tasks | Task list (loose tasks for Projects page) | app.rs | yes |
| POST tasks | Create a task | no | yes |
| POST tasks/start | Agent start by prompt word | no | no |
| GET tasks/:id | One task with everything | app.rs, modals.rs | yes |
| GET tasks/:id/{handoff,log} | Handoff text; event log | task_panel.rs | yes |
| GET tasks/:id/{pr,slot,spans} | PR detail; slot; time spans | no (pr in task) | partly |
| POST tasks/:id | Edit fields (status queued, title, detail, priority…) | task_panel.rs (requeue to queued) | yes |
| POST tasks/:id/start | Start now, or at a time (`at`), new or resume | task_panel.rs, home.rs, goal/waves.rs, menu_bar.rs | yes |
| POST tasks/:id/resume | Start again fresh or finish (lost tasks) | task_panel.rs, home.rs | yes |
| POST tasks/:id/requeue | Back to the queue | task_panel.rs | yes |
| POST tasks/:id/answer, /pick | Answer an agent's question; pick one of its options | task_panel.rs, home.rs | yes |
| POST tasks/:id/talk | Open a terminal to talk it over | task_panel.rs, home/view.rs | yes |
| POST tasks/:id/did, owner-answer, owner-reopen, owner-wait | Human (owner to-do) tasks: mark done, answer, reopen, "waiting on someone" | home/groups.rs, task_panel.rs, goal/lists.rs | yes |
| POST tasks/:id/focus, /detach, /close-terminal | Raise terminal; detach it; stop (force) | task_panel.rs, modals.rs, menu_bar.rs | yes |
| POST tasks/:id/done, /fail | Mark done / cancel ("Cancelled by you") | modals.rs (Finish confirm) | yes |
| POST tasks/:id/pr/merge, /pr/rebase | Merge (with override) / rebase from the Finish modal | modals.rs | yes |
| POST tasks/:id/pr/reviewed | Owner reviewed the PR | task_panel.rs | yes |
| POST tasks/:id/merged-note/ask | Ask the agent to write the testers' note after merge | task_panel.rs | yes |
| POST tasks/:id/step | Tick a step | task_panel.rs | partly |
| POST tasks/:id/queue | Queue with pickup options | no | yes |
| POST tasks/:id/meta, /jira, /delete, /attachments | Edit metadata (device needs, lane, locks…), link Jira, delete, attach | no | yes |
| POST tasks/:id/checkpoint-request | Ask a running agent for a checkpoint | no | yes |
| GET/POST tasks/:id/code-review/triage | Triage of code review findings | no | partly |
| POST tasks/:id/pr/* (wait, skip-checks, merged, reply, ack, body, body-check, thread, addressed, close, checks-running, not-ours, needs-owner, reviewers) | Agent PR loop commands | no | no |
| POST tasks/:id/slot/{claim,release} | Agent slot handling | no | no |
| POST humans/done | Mark a human task done by ref (CLI) | no | yes |

#### Goals and waves
| Route | Purpose | App | Owner |
|---|---|---|---|
| GET goals / GET goals/:id | Goal list; one goal with waves, notes, plan review, devices, bits | app.rs | yes |
| POST goals | Create a goal | no | yes |
| POST goals/:id | Edit (paused, jira_off, deprioritized, name, outcome, after…) | ui/goal.rs (paused, jira_off, deprioritized) | yes |
| POST goals/:id/run | Run now / at a time | ui/goal.rs | yes |
| POST goals/:id/plan | Plan (or edit plan) in a Claude terminal | ui/goal.rs | yes |
| POST goals/:id/restart | Restart a stopped goal | ui/home.rs | yes |
| POST goals/:id/set-aside / bring-back | Put a goal aside; bring it back | home/view.rs (set-aside only) | yes |
| POST goals/:id/{accept,order,delete,attachments} | Accept outcome; reorder; delete; attach | no | yes |
| GET goals/:id/notes | Goal notes | goal/lists.rs | yes |
| POST goals/:id/notes, notes/:nid | Add / edit a goal note | no | yes |
| GET/POST goals/:id/review | Plan review (findings) / outcome review | no (reads plan_review from goal) | yes |
| POST goals/:id/waves/:n | Name a wave, set its review stop (owner-only, app origin) | no (app only displays stops) | yes |
| POST goals/:id/waves/:n/{hold,continue} | Hold a wave; continue past a stop | continue: ui/goal.rs, goal/waves.rs, home.rs; hold: no | yes |
| GET/POST goals/:id/devices, :name/remove | Goal's device needs/reservations | no | yes |

#### Backlog (issues)
| Route | Purpose | App | Owner |
|---|---|---|---|
| GET/POST backlog | List; create an issue | GET via state/goal; POST no | yes |
| POST backlog/bulk | Act on many issues | backlog.rs, issue_panel.rs, home/view.rs | yes |
| GET backlog/triage | Suggested grouping/triage | backlog.rs | yes |
| GET/POST backlog/plan, backlog/plan/add, backlog/goal | Plan a new goal from issues; add to plan; make the goal | backlog.rs | yes |
| GET/POST backlog/tidy | Tidy suggestions (merge dupes, close stale) | no | yes |
| GET backlog/closed | Closed issues | backlog_closed.rs | yes |
| GET backlog/:id, /history | One issue; its history | app.rs, issue_panel.rs | yes |
| POST backlog/:id/{promote,ticket,drop,without-goal,reopen} | Make a task; file a Jira ticket; Defer/Won't do; finish goal without it; reopen | issue_panel.rs, home/view.rs, goal/pool.rs, backlog_closed.rs | yes |
| POST backlog/:id, /critical, /move, /note | Edit; mark critical; move to another goal/project; add note | no | yes |
| POST backlog/fixed | Agent: mark an issue fixed | no | no |
| POST backlog/undo/:n | Undo the last backlog act (toast) | issue_panel.rs | yes |
| POST report | Agent reports (found issues, questions) | no | no |

#### Reviews, recaps, quality, planning, lessons
| Route | Purpose | App | Owner |
|---|---|---|---|
| GET reviews, reviews/now, reviews/:id | Weekly time reviews | app.rs | yes |
| POST reviews | Make a review now | no | yes |
| POST reviews/:id/fixes, /seen | File fixes the review suggests; mark seen | ui/review.rs | yes |
| GET/POST recaps, GET recaps/:id, POST recaps/:id/seen | Weekly/monthly recaps; make one; mark seen | app.rs, ui/recap.rs (POST recaps no) | yes |
| GET quality | Planning quality per project over time | no | yes |
| GET planning, GET/POST planning/lessons | Planning rules in force; planning lessons suggested/accepted | no | yes |
| lessons: GET lessons, lessons/search, lessons/:r; POST lessons, lessons/:r, /keep, /retire, /suggest, /confirm; GET/POST lessons/retro/:g | Lessons: list, search, add, edit, keep/retire suggestions, confirm, goal retrospective | no (recap only shows "lessons kept" count) | yes |

#### Ideas
| Route | Purpose | App | Owner |
|---|---|---|---|
| GET/POST ideas | List (paged); new idea | ui/ideas.rs, app.rs | yes |
| GET ideas/:r | One idea with rounds and notes | app.rs | yes |
| POST ideas/:r/{answer,reopen,talk,shelve,drop,dig,plan,unplan,stop} | Idea lifecycle | ui/idea.rs, menu_bar.rs | yes |
| POST ideas/:r, ideas/:r/note | Edit; add a note | no | yes |

#### Releases
| Route | Purpose | App | Owner |
|---|---|---|---|
| GET releases (`project`, `limit`), releases/:r, releases/unreleased | Releases per project, one release, work not yet released | app.rs, ui/releases.rs | yes |
| POST releases | Cut a release (or a pending cut that waits on tasks/goals) | no | yes |
| POST releases/:r | Edit notes/name | no | yes |
| POST releases/move, releases/pending/cancel, releases/:r/fail | Move work between releases; cancel the pending cut; mark failed | no | yes |

#### Reviewers
| Route | Purpose | App | Owner |
|---|---|---|---|
| GET reviewers (`project=all`), reviewers/asks, reviewers/:id/asks | Roster per project; ledger of review asks | settings.rs, task_panel.rs | yes |
| POST reviewers, reviewers/sync, reviewers/:id/{remove,back,pin,auto,bot,alias,merge} | Add, sync from host, remove/restore, pin, auto-assign level, bot schedule, aliases, merge people | no | yes |

#### Alerts
| Route | Purpose | App | Owner |
|---|---|---|---|
| POST alerts | Raise an alert (tb alert raise) | no | partly |
| POST alerts/:id/dismiss, /restore | Dismiss; undo dismiss | ui/home.rs, home/view.rs, app.rs | yes |
| POST alerts/:id/snooze | Snooze for mins / until tomorrow / next week / back / after a goal | home/view.rs | yes |
| POST alerts/:id/talk | Open a terminal about the alert | home/view.rs | yes |
| POST alerts/:id/act | Run the alert's suggested fix in a new terminal | app.rs | yes |
| POST alerts/:id/clear | Clear (agent/CLI) | no | partly |
| GET/POST buried-questions | Setting: catch questions agents bury mid-reply | settings.rs | yes |

#### Settings-type switches
| Route | Purpose | App | Owner |
|---|---|---|---|
| GET/POST hours | Work hours (start/end/days, "today until") | settings.rs | yes |
| GET/POST keep-awake | Midna keeps the Mac awake: on/off, mode, battery floor, linger, off today | no (read-only from state in hours view) | yes |
| GET usage; POST usage/daily, usage/override | Claude usage; daily share base/carry; lift today's limit | settings.rs | yes |
| GET/POST limits | Context limits (compact window, cold idle, warm tokens, max compactions, generated globs, review gaps) | no | yes |
| GET/POST start-grants, POST start-grants/:id/remove | Standing start grants for Midna triggers | no | yes |
| GET/POST status-bar | Midna status bar options (goal, title) | no | yes |
| GET/POST qa | QA comments on/off and settings | settings.rs | yes |
| GET qa-comments, qa-comments/:id; POST qa-comments/:id | Jira QA comments read and what they became; act / ignore | no | yes |
| GET jira; GET/POST jira/jobs/:id; POST jira/comment | Jira desk state and jobs; comment intake | no | partly |

#### PR feed, builds, master
| Route | Purpose | App | Owner |
|---|---|---|---|
| GET prs/feed; POST prs/event, prs/heartbeat, prs/refresh | PR feed health; events from relays; refresh now | no (feed health via state) | refresh: yes |
| GET prs/limits; POST prs/limits/lift | Host API rate limits; lift a self-imposed hold | no | yes |
| GET stream; POST stream/events, stream/mode | Event stream health; relay events; switch mode | no (health via state) | mode: yes |
| GET/POST pr-builds; POST pr-builds/event | Stop/resume PR builds board-wide when CI time is scarce | no | yes |
| POST master-builds | Relay of default-branch builds | no | no |
| GET master, master/:id; POST master/:id, master/check | Master breaks (red default branch): list, detail, verdict (mine/not ours…), check now | no (shown read-only via state in PR watch) | yes |
| GET master-breaks, :id; POST :id/verdict | Older alias of the above | no | yes |

#### Devices and bits
| Route | Purpose | App | Owner |
|---|---|---|---|
| GET/POST devices, devices/:name; POST devices/:name/remove | Device pool: list, add, change tags/focus command, remove; waiting list | no (pool shown inside goal via goal data) | yes |
| POST devices/:name/focus | Raise a device window | ui/goal/pool.rs | yes |
| GET/POST bits, bits/:name; POST bits/:name/{made,remove} | Feature flags behind a goal: list, add, edit, mark created in the flag tool, remove | no (DESIGN/module docs mention the app's "Mark created", but no app code calls it) | yes |

## 15. Sample data (the fake backend)

`TASKBOARD_BACKEND=fake` runs the real board code in-process on a throwaway seeded folder (no runner, no Midna). Owner is "Alex"; projects "webapp" and "api".

Always present (crates/taskboardd/src/seed.rs + backend.rs seed_sessions/seed_attachments)
- Goal G1 "Sign-in with passkeys" (outcome: users can sign in with a passkey on web): T1 done with summary, T2 working with latest line and a terminal, T3 needs you with a question ("Should removing the last passkey require a password re-check?"), a planned docs task; one loose queued task "Upgrade the HTTP client".
- One open backlog gap reported by T1's terminal with snapshot.
- Goal "Faster sign-in" planned in 3 waves: wave 1 "Measure" with a review stop already released, a running wave, a waiting wave "Check" with a stop, a task with no wave, a task shared with G1.
- Device pool (one simulator lent to T2, a task needing an ios device) and G1's feature flags (some made, some not).
- Today's timeline for T1–T3 and two weeks of finished history across both projects (with questions, PR links, estimated developer minutes) for Days.
- Ideas: "Offline mode" (reported, with dig rounds, risks, open questions, notes), "Dark theme" (reported), "Search across projects" (digging), "Keyboard shortcuts sheet" (new), "Passkey sign-in" (planned into G1), "Rate limits per key" (api, shelved).
- Two weekly reviews: last week's unseen with filed fixes (one already in a goal), plus an older one; time by category, per project, top tasks, owner wait, change vs prior week.
- Three terminals: T2 working, T3 needs, an idle "api shell".
- Attachments on "Faster sign-in": a design and a doc on the goal; screenshots (real images), a trace, a video and a doc on its tasks.

#### Switches
| Variable | Adds |
|---|---|
| TASKBOARD_FAKE_RELEASES | Releases on for both projects; nine webapp releases over two months (one failed: "Notarizing timed out."), multi-line notes, unreleased done work, a running task, a pending cut v0.14.0 waiting on a task and a goal; api with one release |
| TASKBOARD_FAKE_PLAN=B2,B3 | Backlog's new goal already planned from those issues |
| TASKBOARD_FAKE_PLAN_REVIEW | Finished plan review of G1 with four findings (outcome unproven, owner prerequisite, planned past an open question, overlap with another goal) |
| TASKBOARD_FAKE_STOPPED | T2 and T3 lost their terminals (closed by hand) for the Stopped state |
| TASKBOARD_FAKE_NEEDS | Two goals with 3 and 2 "misses its done-when" issues and alerts; a plain alert ("Release … was dropped"); owner to-dos (human tasks), one blocking a task; two to-dos snoozed until next week |
| TASKBOARD_FAKE_STOPS | Goals stopped at a failed task, a held wave and a review stop; a paused goal; a goal chained after another; a finished goal whose outcome missed |
| TASKBOARD_FAKE_RECAPS | This week's and this month's recaps, unseen; the weekly one carries every recap component |
| TASKBOARD_FAKE_PANEL | Task need states: lost task that looks finished (evidence of tag and release), PR waiting on owner review, PR rebasing, PR stopped with failing check and agent question, merged PR missing its testers' note, a working task with a PR and a human task waiting on it |
| TASKBOARD_FAKE_SLOTS | webapp worktree pool: slot-1 held by T2, two free; pool rules 1–4 slots, trim 3 days, remove 7 days |
| TASKBOARD_FAKE_JIRA | Jira site acme.atlassian.net / WEB; QA comments on; GitHub, Slack and Jira accounts connected |
| TASKBOARD_FAKE_QUIET | Alert "T5: no update for 40 minutes" (work gone quiet) |
| TASKBOARD_FAKE_SETUP=blocked\|failed | The install/setup status row shows blocked or failed (otherwise "Dev") |

#### States with NO sample data (designer should not assume they look tested)
- Empty board / first run: the fake is always seeded; no switch shows a brand-new board, no projects, or no accounts connected alongside other data.
- Daemon unreachable, Midna down, account errors or expired tokens, GitHub sign-in in progress, usage limit hit / override active, outside work hours.
- Start asks and start grants, master breaks, QA comments, Jira desk jobs, PR builds stopped, feed unhealthy, host rate limit hold.
- Lessons and retrospectives, planning quality.
- Code review rounds and triage, stacked PRs, same-PR chains, reviewer roster and review asks.
- Locks and run-alone waits, device waiting list, goal device reservations, backend bit blocking a finished goal.
- Closed sessions, closed backlog issues, backlog tidy suggestions, areas.
- Failed/declined task starts, scheduled starts/runs (at a time), set-aside goals, deprioritized goals.
- Large volumes (hundreds of tasks/ideas/days/releases for paging and virtualization) and very long logs or handoffs.
- Urgent alerts and alert digests; human tasks marked "waiting on someone".
