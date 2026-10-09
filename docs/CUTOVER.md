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
```

The old file is only read (it's copied first, with its `-wal` and `-shm`). The import:

- carries over tasks with their log (handoffs, checkpoints, notes) and conversation ids, goals with
  their notes and waves, the backlog and its history, PR links and stages, attachments, terminals
  and their history, QA comments, and the owner's settings (work hours, PR flows, …);
- keeps every T, G and B number, and new ones carry on after the highest;
- leaves out the old board's jobs (its pending work would run again) and its alerts;
- keeps old tables this board has no table for (reviewers and review asks, bits, devices, master
  breaks, …) whole in `settings` as `import.<table>`, and lists them;
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
