# task-board Claude Code plugin

The `task-board` skill that teaches agents the `tb` command, and the `/start` command. The hooks that
report each Claude Code session's activity to the local task board aren't part of the plugin:
Taskboard.app writes them into `~/.claude/settings.json` (see below). They do nothing outside a
Midna terminal.

```
plugin/
  .claude-plugin/marketplace.json     marketplace "taskboard"
  task-board/
    .claude-plugin/plugin.json
    bin/tb                            shim that runs the Rust tb binary
    bin/wait-hint                     stops sleep-poll loops; steers long builds to the background
    commands/                         /start
    skills/task-board/                SKILL.md, questions.md, planning.md, pr.md
```

## Install

Install Taskboard.app first (see the repo README); it links `~/.local/bin/tb`. Without the app, from the repo root:

```sh
./install.sh          # without the app: copies taskboardd and tb into ~/.local/bin
```

With Taskboard.app, click **Install hooks** in its status bar instead: it installs this plugin from the copy
inside the app and adds the hooks to Claude Code's `settings.json`. Each hook entry runs the app's own `tb`
at its fixed path, guarded so a missing app is skipped:

```sh
[ -x '/Applications/Taskboard.app/Contents/MacOS/tb' ] && exec '/Applications/Taskboard.app/Contents/MacOS/tb' hook Stop || true
```

so an update reaches them without a reinstall; the status bar only asks again when the entries the new
version writes differ. The app leaves the rest of `settings.json` alone, keeps the original as
`settings.json.before-taskboard` the first time it changes it, and never rewrites a file that doesn't parse.
`taskboard-app --uninstall` takes its entries out again. The events, matchers and timeouts are in
`crates/taskboard-app/src/hooks.rs`.

Without the app, add the marketplace and install the plugin (for the skill and `/start`):

```sh
claude plugin marketplace add <path-of-this-repo>/plugin
claude plugin install task-board@taskboard
```

Restart Claude Code (or run `/reload-plugins`) so the hooks and skill load.

## The version

The `version` in `task-board/.claude-plugin/plugin.json` is a hash of the plugin's tracked files (all but
`plugin.json`), as `1.0.0-h<12 hex>`, not the app's version: it changes only when the skill, the `/start` command or
the `bin/` scripts do. `scripts/release.sh` writes it through `scripts/plugin-version.sh`, and the release workflow's
`--check` fails when it's out of step; run `./scripts/plugin-version.sh` after a plugin change to write it yourself.

Claude Code serves the plugin from the app bundle, so an update brings a changed skill or command with it, and the
app doesn't ask for a reinstall over the version. A Claude Code that copies plugins into its cache
(`~/.claude/plugins/cache/taskboard/task-board/<version>`) keys that copy by the version, so the hash gives a new copy
exactly when the contents change.

## How `bin/tb` finds the binary

The shim resolves its own real path (following symlinks) and runs the first of these that exists
and is executable:

1. `$TASKBOARD_TB`
2. `../../../target/release/tb`, then `../../../target/debug/tb`, relative to
   the shim: the repo's own build when the plugin is loaded in place from the repo
3. `~/.cargo/bin/tb` (`cargo install --path crates/taskboard-cli` puts it there)
4. `~/.local/bin/tb` (Taskboard.app links it there on first launch; `./install.sh` copies it there)
5. `/Applications/Taskboard.app/Contents/MacOS/tb`, then the same under `~/Applications`

Agents type `tb` whether or not `~/.local/bin` is on their PATH, because the plugin's `bin/` is on
the Bash tool's PATH. If another program called `tb` sits in `~/.local/bin`, the app leaves it alone;
set `TASKBOARD_TB` to the app's copy instead. A plugin installed from
a marketplace may be copied into Claude Code's plugin cache, where step 2 can't find the repo
build: install `tb` or set `TASKBOARD_TB`.

If no binary is found, `tb hook …` and `tb statusline` exit 0 silently, so a missing binary never
breaks Claude Code. Any other command prints a one-line error and exits 1.

## Status line (usage limits)

`tb statusline` reads the status-line JSON Claude Code sends, saves it (with the current rate
limits) for the board, and prints a short status line (model · folder · 5-hour usage). With it,
the board can stop starting new agents when the 5-hour usage runs out. Point Claude Code's
`statusLine` at it in `~/.claude/settings.json`:

```json
{
  "statusLine": {
    "type": "command",
    "command": "~/.local/bin/tb statusline"
  }
}
```

To keep a status line of your own, run `tb statusline --pass | your-command`: with
`--pass` it prints its input unchanged instead.
