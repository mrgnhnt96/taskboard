# Moving over from the old board

The Python board ran as its own LaunchAgent on `127.0.0.1:8792`, kept everything in one SQLite file
(`tasks.db`), and its Claude plugin put its own `tb` on agents' PATH. Taskboard wants the same port,
so the old board has to stop first. Its data comes over with `taskboardd import`, keeping every
T/G/B number, so refs in notes, commits and PRs stay valid.

Taskboard.app checks for what's in the way each time it starts (another program on the board's port,
another `tb` first on the PATH, another task-board plugin in Claude Code) and says so in its banner.
`taskboardd check` prints the same from a terminal.

## 1. Stop the old board

Find its LaunchAgent and stop it:

```sh
grep -l 8792 ~/Library/LaunchAgents/*.plist      # the old board's plist
launchctl bootout gui/$(id -u) ~/Library/LaunchAgents/<its label>.plist
lsof -nP -iTCP:8792 -sTCP:LISTEN                  # nothing should be listening now
```

Move the plist out of `~/Library/LaunchAgents` so it doesn't start again at login.

## 2. Import its data

Quit Taskboard.app (and stop its daemon if it already ran: `launchctl bootout gui/$(id -u)/<label>`,
or `taskboard-app --uninstall`), then import into the board's data folder while it's still empty:

```sh
/Applications/Taskboard.app/Contents/MacOS/taskboardd import <path to the old tasks.db>
# the old board's master breaks were on one project: name it unless [master.projects] has just one
/Applications/Taskboard.app/Contents/MacOS/taskboardd import --master-project <name> <path to the old tasks.db>
```

The old file is only read (it's copied first, with its `-wal` and `-shm`). The import:

- carries over tasks with their log (handoffs, checkpoints, notes) and conversation ids, goals with
  their notes and waves, the backlog and its history, PR links and stages, attachments, terminals
  and their history, QA comments, and the owner's settings (work hours, PR flows, …);
- carries each goal's setup (`task_setup`) and each task's PR plan (`ships_pr`, `no_pr`, the
  stacked-on task `pr_after`);
- rebuilds each PR's host and link from its link, its repo, or the project's remote
  (`[pr_body] remote`, GitHub or Bitbucket), so open PRs are watched again (an old bare repo name
  becomes the link's `owner/name`); a PR it can't place is listed;
- maps the device pool (`devices`, `device_loans`, a task's `device_need`, `goal_devices`) and the
  bits (`bits`, a task's or goal's `bits`, link tables) into the board's own tables, and marks the
  old Jira desk terminal (`sessions.jira_desk`) as the desk. An old need's `device:<id>` becomes
  that one device and `tag:x` tag `x`; a task's `none` stays its own "needs none"; a loan a
  finished task still held comes back. Old `goal_devices` rows with a purpose or a reserved flag
  are a goal's own pool: they become the goal's own devices (`tb goal devices`), the purpose made a
  tag word (`Payments on Android` → `payments-on-android`), rather than needs. A blocked device
  (`devices.blocked`) comes over switched off, its note saying what it's kept for; a removed one
  (`devices.removed_at`) doesn't come over, and is listed;
- maps the reviewer roster (`reviewers`: one row per person and project, `source` = `import`).
  Rows that are one person fold into one: the most active row stays, with its own removed mark,
  and the commits, asks and swaps add up. The host display name (`bb_name`) becomes an alias;
  removed, pinned, the automation level (`automated`) and the bot schedule (`bot_every`, in
  minutes) are kept; the old asks, swaps and last ask (`asks`, `swaps`, `last_asked`) count
  toward each reviewer's next turn;
- maps every review ask (`review_asks`): who was asked by their `email` (matched against the
  reviewers' emails and aliases), its state, answer and stand-in (`replaces`), and whether its
  fill-in was asked (`filled_at`). An ask still open on a PR that has merged or closed (`pr_state`)
  comes over closed; one on a done task whose PR is still open stays open;
- takes each reviewer's last bot run from `reviewers.bot_ran_at` (or an old runs table);
- maps the master breaks (`master_breaks`), keeping each `M<n>` number and its verdict (the old
  yours / not yours / unsure). The old board watched one project, so its breaks name none: they
  go on `--master-project <name>`, else the only project in `[master.projects]`, else they're
  listed and skipped. The build (`url`, `build_id`, `pipeline`), `title`, `error` and `fix` go
  into the evidence; `proof` links are the break's proof; a `fix` naming a task is the fix task
  (a sha, the fixed head); `base` is the branch (a sha, the last green head). A break still open
  on a project `[master.projects]` doesn't watch comes over closed, and is listed;
- keeps any column of those tables the board has no place for (like an ask's `tries` and `busy`)
  in `settings` as `import.<table>.unmapped`, with each row's id, and lists it;
- keeps every T, G and B number, and new ones carry on after the highest;
- leaves out the old board's jobs (its pending work would run again), its alerts, and its other
  running state (`bridge_*`, the terminal manager's project lists `midna_projects` and
  `saggar_projects`, which the bridge fills again, `dispatch_seen:*`, `usage_guard_handled:*`, `review_round:*`, health readings and live session ids, …);
- keeps any other old table this board has no table for whole in `settings` as `import.<table>`,
  and lists it;
- counts the rows it actually wrote, and lists every old row it skipped (a clash, an unknown
  device, a device need the board can't take) with why;
- refuses a board that already has tasks, goals or backlog issues (`--data <empty folder>` imports
  somewhere else).

It reads the old file's own tables and columns, so an older or newer old schema still comes over:
columns match by name (or a known older name, like `conversation_id`), and tables by name (or
`backlog` for `issues`).

## 3. Swap the plugin and `tb`

```sh
claude plugin list                                # an old task-board plugin?
claude plugin uninstall <its name>
which -a tb                                       # the first should be ~/.local/bin/tb (Taskboard's)
```

Then open Taskboard.app: it links `~/.local/bin/tb` and its status bar offers to install the hooks
(the `task-board@taskboard` plugin).

## 4. Start

Opening Taskboard.app registers its LaunchAgent, which starts `taskboardd serve` on the port. Run
`taskboardd check` to confirm nothing is in the way, and the board shows the imported work.
