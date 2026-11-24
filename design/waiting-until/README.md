# Waiting until a set time

A mockup of how a task looks when it has parked itself until a set time (for example,
waiting for a nightly benchmark run). The canvas is in `canvas/`. `index.html` is the
build, and `screen.html` is built from `base.css`, `waiting.css`, `body.html` and
`setup.js`.

Rebuild:

```sh
cd design/waiting-until/canvas
{ printf '<template id="uh-screen"><style>\n'; cat base.css waiting.css; printf '</style>\n'; cat body.html; printf '</template>\n<script>\n'; cat setup.js; printf '</script>\n'; } > screen.html
node ~/.claude/plugins/cache/mrgnhnt96/ultrahand/0.11.0/shared/canvas/build.mjs \
  --presets ~/.claude/plugins/cache/mrgnhnt96/ultrahand/0.11.0/skills/desktop/references/presets.md \
  --config config.json --screen screen.html --head theme.html \
  --title "Waiting until a set time" --out index.html
```

## What it shows

| Place | Waiting | Didn't resume (overdue) |
|---|---|---|
| Status chip | **Waiting**: hollow blue pill (Working's blue, outlined, no fill). Ranked right after Working. | **Needs you** |
| Goal row line | `Resumes 6:30 PM · <plan>` with the time in bold | `Didn't resume at 6:30 PM · <plan>` in the Needs you colour |
| Goal status bar | Its own segment: Working's blue at about half strength | Needs you segment |
| Goal header chip and sidebar goal line | Unchanged (the goal is still Working) | Goal becomes Needs you; sidebar line `2 of 6 · Didn't resume` |
| Task panel top box | Quiet grey box, clock icon, `WAITING · RESUMES 6:30 PM`, then the plan in bold. No buttons, no Focus in Midna. | Needs you box, alert icon, `DIDN'T RESUME AT 6:30 PM`, the plan, and Focus in Midna |
| Timeline now card | Waiting chip, dashed blue dot, `after 52m of work · since 1:48 PM`, the latest update | Needs you chip, orange dot |
| Needs you page | Not there | A card in Agents waiting: the miss line in the Needs you colour, the plan under it in muted text |

Once the next turn starts, the box goes away, the task is Working again, and the timeline
keeps a past stop `Waited until 6:30 PM` (dashed dot) with the plan under it.

## Rules

- **Time:** 12-hour local time with AM/PM. Today's time is bare (`6:30 PM`); a time on
  another day adds the day: `tomorrow 6:30 AM`, then the weekday (`Thursday 6:30 AM`) within
  the week, then the date (`Dec 2 6:30 AM`).
- **Plan text** is what the task said it will do when it resumes, shown as written.
- **Overdue** starts once the resume time plus the grace period has passed with no turn.
  The mockup uses 15 minutes: resumes 6:30 PM, raised at 6:45 PM, shown at 22m.
- **No Asks you.** A waiting task never shows the Asks-you box or the orange Needs-you
  look until it is overdue.
- **A reply that only announces a wait** (it says it will wake at a time but didn't run the
  park command) has no look of its own. The board sends it back with the command; if it
  ends that way again, the board records it. It is never shown as Asks you, and the task
  stays as it was.
