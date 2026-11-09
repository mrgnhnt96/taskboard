## Verdict

macOS desktop app (GPUI), pointer and keyboard, lower Days page. Evidence is the code slice, the full days.rs and theme.rs, and 5 captures. Nothing was driven with real input, because this is a native window and the browser can't open it. Contrast is computed from theme hex values. Sizes seen: about 1363 CSS px wide (days-tall.png, days-900.png, days-dark.png) and 900 wide (days-s.png). First impression: the left column reads as organised, with the week, the timeline, then task lists. The right rail is a stack of same-looking cards that switch between day and week without saying so, and at 900 wide it falls under everything in a half-width column with the other half empty. Counts: 0 catastrophe, 4 major, 12 minor, 1 cosmetic. Top problem: the lower cards don't reliably say which period they describe. "This week" is hard-coded even when an earlier week is shown, and "Agent time by project" is one day's figure sitting under a week chart with almost the same name, so "webapp 100%" reads as false on a page that exists to show how the work went.

## Findings

### R1 · 3 Major · "This week" stays on the cards when an earlier week is shown
- **Element:** Days › rail › Hours saved this week and Tasks done this week (titles and legends), on any day outside the current week, all sizes
- **Evidence:** Confirmed
- **Problem:** The daemon builds the week from the selected date (week_start(date, ...)), but saved_card and done_card hard-code "this week" and "last week". Go back to October and the card still says "Hours saved this week" over October's numbers. Going back to earlier days is a stated purpose of the page, so the owner can misread an old week as the current one. The left cards already use v.week_label correctly.
- **Principle:** Visibility of system status and truthful readouts [NNG-H1]; consistency [NNG-H4]
- **Fix:** Use the same pattern as the left column: h2(format!("Hours saved · {}", v.week_label)) and h2(format!("Tasks done · {}", v.week_label)). Keep "this week" only when the shown week contains today. Change the legends and the vs_last line to "shown week" / "week before" in the same case.

### R2 · 3 Major · The rail mixes day and week scopes and repeats the same facts
- **Element:** Days › rail (Notable, Hours saved, Waiting on you, Tasks done, Agent time by project, Planning): days-tall.png, days-900.png, days-s.png
- **Evidence:** Confirmed
- **Problem:** Six same-looking cards alternate scope: Notable (day), Hours saved (week), Waiting on you (day, unlabelled), Tasks done (week), Agent time by project (day, unlabelled), Planning (30 days). "Agent time by project" reads webapp 100% directly below a page whose "Agent time · Nov 1 – Nov 7" chart shows api and webapp, because it comes from day["by_project"]. Facts repeat: 36m is the KPI, the Notable lead, the Waiting on you total and its median; 5h 36m is both KPI and split total; each day's done count is on the week tiles and again in the Tasks done chart. Reading the rail means working out each card's period one at a time.
- **Principle:** One fact, one place [SEC-7]; group by function [SEC-1]; every element earns its place [SCR-5] [NNG-H8]
- **Fix:** Reorder the rail by scope: Notable, Waiting on you, Agent time by project (the selected day), then Hours saved, Tasks done, Planning (longer spans). Put the day in the two unlabelled titles with the finished_card logic: "Waiting on you today" / "Waiting on you on Friday, November 6", and the same for "Agent time by project". Remove the 24px total from Waiting on you, since the KPI already holds it, and lead with the strip and "median 36m · usual 18m". Drop the "longest wait" Notable line when there is only one wait.

### R3 · 3 Major · Waiting time in the task bars is hard to see and has no number
- **Element:** Days › How long tasks took › bar wait segment: T14 in days-tall.png, T23 in days-900.png; light and dark
- **Evidence:** Confirmed
- **Problem:** The wait segment butts straight against the work segment with no gap, and the two are separated only by hue: warn against api blue is 1.17:1 light and 1.72:1 dark; warn against webapp amber is 2.39:1 light and 1.45:1 dark. Amber and orange are also close hues, although the palette comment says oranges are left out. The row prints only the total, so the wait in minutes exists only in a hover tooltip. This is the card's second meaning and the part the owner can act on.
- **Principle:** Non-text contrast [WCAG 1.4.11]; use of colour [WCAG 1.4.1]
- **Fix:** Add a 2px card-colour gap between the segments: left(relative(work)) plus ml(px(2.)). Give the wait segment a shape difference, such as a 2px top and bottom border or diagonal stripes. Print the wait in the row: "2h 48m" with a muted "32m wait" in the state column when no state is set. Replace the webapp amber slot with a hue further from warn, then re-run the palette's colour-blind check.

### R4 · 3 Major · Rows and chart details can't be reached from the keyboard
- **Element:** Days › How long tasks took rows, Done today rows, Show all link, Waiting on you squares, Tasks done bars; all sizes
- **Evidence:** Confirmed
- **Problem:** days.rs has no focus handle, tab stop or key handling. Task rows and Done rows open the right panel on click only, the Show all link is a clickable div, and every hover tooltip (work and wait split, PR state, each wait, each day's counts) is pointer-only. Without a mouse the owner can't open a task from this page or read those numbers. The rows also have no focus style. Search can still find a task, so it isn't a dead end, but this page's list can't be used.
- **Principle:** Keyboard [WCAG 2.1.1]; focus visible [WCAG 2.4.7]; arrows move within groups [MS-KBD]
- **Fix:** Give each list one focus handle via track_focus, as backlog.rs does: Up and Down move a selected row, Enter runs m.open_task, and the focused row gets a 2px accent outline. Make Show all a tab stop operated with Enter or Space. Show a focused row's tooltip content while it has focus, or print it in the row (see R3).

### R5 · 2 Minor · Below about 1040 wide the rail drops under everything at half width
- **Element:** Days › lower layout at window widths under about 1040: days-s.png, 900 wide
- **Evidence:** Confirmed
- **Problem:** The left column (min 420) plus the rail (fixed 300) plus gap needs 736 px, so at 900 wide the flex-wrap sends the rail below Done today. It keeps w(px(300.)), leaving about 297 px beside it empty, and six cards stack in one thin column. The page roughly doubles in length, and Notable, the day's summary, ends up last.
- **Principle:** The order holds as it narrows [SCR-6]; desktop window range [HIG-MACOS]
- **Fix:** When the rail wraps (viewport under 1040), make it .w_full().flex().flex_row().flex_wrap() and give each card .flex_1().min_w(px(280.)), so the cards fill two columns of about 290 px. Pair the cards by scope from R2: Notable with Waiting on you, Hours saved with Tasks done.

### R6 · 2 Minor · The lower cards have no maximum width
- **Element:** Days › How long tasks took and Done today on a large display, code path; widest capture is about 1363 wide
- **Evidence:** Confirmed
- **Problem:** The page column is w_full() and left is flex_1 with no max width, so on a wide display bars keep stretching and each Done today time sits further from its title. It is already about 650 px away in days-900.png, with only the hover fill to connect the two. The rail stays 300 while the left grows.
- **Principle:** Nothing stretches past a readable line length at the maximum window size [HIG-MACOS] [tablet:AND-T2]
- **Fix:** Add .max_w(px(1600.)) to the page's inner column. In Done rows, move the time next to the title's end or add a divider line per row if the column stays wider than about 900.

### R7 · 2 Minor · Failed and lost tasks look like any other row
- **Element:** Days › How long tasks took › state column, failed or lost task (code; not in the sample captures)
- **Evidence:** Confirmed
- **Problem:** state_c gives warn_fg to "needs you" and muted grey to everything else, so "failed" and "lost" are the same quiet 12px text as "so far", on a bar in normal project colour. Those are the rows the owner most needs in "how the work went". The timeline already uses t.down and down_soft for them.
- **Principle:** Consistency [NNG-H4]; visibility of system status [NNG-H1]
- **Fix:** Set state_c to t.down for "failed" and "lost" (6.57:1 light, 5.80:1 dark on card) with FontWeight::SEMIBOLD, matching bar_colors.

### R8 · 2 Minor · The task list is silently cut to 8 and its median line carries no information
- **Element:** Days › How long tasks took: days-tall.png, days-900.png
- **Evidence:** Confirmed
- **Problem:** View::of uses .take(8), so the card shows 8 tasks in a week with 15 done, with no count and no way to see the rest; the title suggests it covers the week's tasks. The median is computed over the whole week, so all 8 long tasks pass it and the same tick crosses every bar, forming a heavy vertical rule that says nothing about any row. In dark mode the tick is also 2.52:1 on webapp and 2.99:1 on api.
- **Principle:** Visibility of system status [NNG-H1]; every element earns its place [SCR-5]
- **Fix:** Title it "Longest tasks · Nov 1 – Nov 7" with a muted "8 of 23", and add the same Show all link Done today has. Drop the per-row tick and keep "median 1h 36m" as text in the legend, or draw it once as a single line behind the bars at t.border_2.

### R9 · 2 Minor · A row's project is shown by colour alone
- **Element:** Days › How long tasks took bars, Done today swatches; light and dark
- **Evidence:** Confirmed
- **Problem:** Neither list prints the project name, which sits only in the tooltip. Past four projects project_color falls back to one grey, so an owner with many projects can't tell them apart, and that is this owner's normal use. In light mode the webapp swatch is 2.17:1 on white, so the small swatch is faint too.
- **Principle:** Use of colour [WCAG 1.4.1]; non-text contrast [WCAG 1.4.11]
- **Fix:** Add a muted 12px project name in Done rows between the title and the time, and in task rows after the title once more than one project is shown. Darken the light webapp slot from #eda100 to at least 3:1 on white, around #b87a00, then re-check colour-blind separation.

### R10 · 2 Minor · Notable names tasks by bare ref and they can't be opened
- **Element:** Days › Notable › "longest wait on you · T3", "longest task · T2": days-tall.png, days-s.png
- **Evidence:** Confirmed
- **Problem:** Elsewhere on the page a task ref opens the right panel. In Notable it is plain text with no title, so the owner has to remember what T3 is or hunt for it further down. "3 at once" and "busiest hour · 4 events" are also vague about what ran and what counts as an event.
- **Principle:** Recognition rather than recall [NNG-H6]; tasks open in the right panel (owner's one-home rule)
- **Fix:** Carry the ref in notes as Option<String>, and make the note a row with the same hover and on_click open_task as task rows. Show the title with truncate: "longest wait on you · T3 Settings passkey". Reword to "3 agents at once" and "4 commits, PRs and questions".

### R11 · 2 Minor · The Waiting on you strip is unlabelled and hides waits
- **Element:** Days › Waiting on you › strip: days-tall.png, days-s.png
- **Evidence:** Confirmed
- **Problem:** Nothing says the tall tick is "usual" or that a square is one wait, and the median in the subtitle isn't plotted. Each square is 10 px with a 2 px card-colour border, so about 6 px is visible, and that is the only hover target for which task waited (under the 24 px floor). Every wait of 60m or more is clamped to x=1.0, so the worst waits stack on one spot and only the top tooltip can be reached. Squares aren't clickable. With no waits the card still shows 0m and an empty scale.
- **Principle:** Target size [WCAG 2.5.8]; recognition rather than recall [NNG-H6]
- **Fix:** Label the tick with a small "usual 18m" above it, and remove usual from the subtitle. Replace the squares with a list of up to 3 waits ("36m · T3 Settings passkey") opening the task, with the strip above as a summary; the rows are then the 24 px-plus targets. With no waits, show only "No waits" in muted text and drop the strip.

### R12 · 2 Minor · Tasks done card: quiet selected day, weak last-week line, muddled subtitle
- **Element:** Days › Tasks done this week: days-tall.png, days-900.png
- **Evidence:** Confirmed
- **Problem:** The selected day is only a band at 1.17:1 against the card in dark and 1.16:1 in light, and its label stays plain, while Hours saved bolds "Fr". Where the last-week line crosses a bar it is 2.14:1 in dark. Bars use the accent blue, close to the api project blue beside them (#3563e9 and #2a78d6 light), so they can read as api's work. The subtitle "last week 16 · 6 PRs, last week 8" repeats "last week" and adds PRs to a tasks card; with merged counts it grows long enough that it will likely wrap in the 272 px of card width.
- **Principle:** Non-text contrast [WCAG 1.4.11]; use of colour [WCAG 1.4.1]; one look per role [SEC-4]
- **Fix:** Bold the selected label in t.text, as in Hours saved. Give the last-week line a 1px card-colour halo. Use a neutral bar colour (t.text_2 at reduced strength) so blue stays api. Split the subtitle into two muted lines: "16 last week" and "6 PRs (8) · 4 merged (4)".

### R13 · 2 Minor · Hours saved can't be read per day or squared with the agent chart
- **Element:** Days › Hours saved this week: days-tall.png, days-s.png
- **Evidence:** Likely
- **Problem:** The chart has no hover values, unlike the Usage row and Tasks done, so a day's figure can only be estimated between 0, 40h and 80h. The breakdown says "24h agent" while the week chart above adds up to roughly 28h (read from the bars), because this uses est_agent_min and the label doesn't say so. The legend draws last week as a solid line, but the chart draws it dashed. "−16h on last week" doesn't say it compares the same point in the week.
- **Principle:** Consistency [NNG-H4]; visibility of system status [NNG-H1]
- **Fix:** Overlay seven flex_1 hover targets, as usage_row does, with a tip such as "Thu · 52h · week before 58h". Relabel to "79h human estimate − 24h agent on estimated tasks". Draw the legend's last-week swatch dashed. Change the delta to "16h behind last week at this point".

### R14 · 2 Minor · Planning shows better or worse by colour only, and "was" has no period
- **Element:** Days › Planning, last 30 days (code; card not present in any capture, which may be the sample's)
- **Evidence:** Confirmed
- **Problem:** Green or red on the value is the only better-or-worse signal, and lower being better isn't stated anywhere, so "1.2 was 0.9" can't be judged without colour. "was" doesn't say it means the half year before. "Fix rounds a task" and "Questions a task" read as fragments.
- **Principle:** Use of colour [WCAG 1.4.1]; match with the real world [NNG-H2]
- **Fix:** Add ↓ or ↑ before the value with better or worse in the tooltip. Add a muted line under the title: "Lower is better · was: the 6 months before". Reword to "Fix rounds per task" and "Questions per task", and check the longest label still fits the 300 px rail.

### R15 · 2 Minor · Show all renders the whole Done list without virtualizing
- **Element:** Days › Done today › Show all N
- **Evidence:** Confirmed
- **Problem:** Expanding renders every finished row as plain .children(rows) inside the page scroller. A busy day with many agents is a long list, and this breaks the owner's rule that long lists are virtualized. The page also gets longer in one step.
- **Principle:** Owner rule: long lists virtualized; flexibility and efficiency [NNG-H7]
- **Fix:** Reveal in steps of 50 ("Show 50 more", then "Show fewer") so the page keeps one scroller, or move the expanded list to a gpui list sized to the page scroller. Avoid a fixed-height inner scroller, which nests same-direction scrolling [HIG-SCROLL].

### R16 · 2 Minor · Task titles truncate at any window width and start on ragged edges
- **Element:** Days › How long tasks took › name column: days-900.png, days-s.png
- **Evidence:** Confirmed
- **Problem:** The name column is fixed at 230 px, so "Add the WebAuthn registration en…" and "Retry webhook deliveries with bac…" are cut at every size while the bars take all extra width; the full title is only in a hover tooltip. Refs have no fixed width, so titles start at different x positions (T19 against T2), unlike Done today's aligned 52 px ref column.
- **Principle:** Aligned to a grid [SCR-4]; recognition rather than recall [NNG-H6]
- **Fix:** Give the ref a fixed width of 40 px, matching across both cards. Make the name column .w(relative(0.4)).min_w(px(230.)).max_w(px(420.)) so titles get space before bars do.

### R17 · 1 Cosmetic · Too many type sizes and uneven gaps
- **Element:** Days › lower cards, all sizes
- **Evidence:** Confirmed
- **Problem:** The cards use seven sizes (10, 11, 12, 12.5, 13, 13.5, 24), several just half a point apart. The left stack gaps 16 and the rail gaps 12, so card edges don't line up across the columns; inner gaps switch between 8 and 10.
- **Principle:** A small type scale [SCR-3]; aligned to a grid [SCR-4]
- **Fix:** Use 11 for axes, 12 for secondary text, 13 for body and titles (semibold), 24 for totals. Use gap 16 in both columns and gap 10 in all cards.

## What works

- All measured text passes: muted is 7.12:1 dark and 5.43:1 light on card, and pill and failure colours are 5.5:1 or better.
- Task and Done rows follow the one-home rule: they open the right panel, with a clear hover fill.
- Times are 12-hour with AM/PM throughout, and Done today lists newest first with a capped default length.
- Left-column titles carry the week range ("· Nov 1 – Nov 7"), which is the pattern the rail should copy. Done today's title also follows the selected date.

## Not checked

- Real input: this is a native GPUI window, so no control was used. Hover, click, wheel, keyboard and tooltips were read from code only, and Show all, scroll position after expanding and right-panel opening weren't exercised.
- Composition audit and the blurred squint test: audit.mjs needs a page. Composition was judged from captures.
- Capture limits: captures.txt lists all five as dark at their sizes, but days-tall.png is the light theme and days-s.png is 1302x1568, not 900x560. So the 560 minimum height wasn't seen, and the 900-wide capture cuts off before Agent time by project and Planning. days-dark.png looks identical to days-900.png, so it adds no state.
- days-1700.png is blank past the page end, as the shifted build's artefact; not treated as a finding. The dark strip at the top of the shifted shots is treated the same way.
- Light theme below the first screen: the lower cards were only seen in dark, and light numbers come from theme.rs.
- Sample limits: only webapp ran today, no PR pills appear in Done today, Planning is absent, and there is one wait. PR pill appearance, multi-project splits, a busy day and more than four projects are from code only. The Hours saved 28h total is read off chart bars, so approximate.
- Data states: loading, error and offline for these cards, an earlier day or week on screen, and hidden projects. R1 comes from code, not a capture.
- The right panel open: it may narrow the content and trigger the rail wrap at wider windows than the 1040 estimate, which itself assumes a sidebar of about 247 px measured from days-s.png.
- Weekly review suggestions, releases and the next-cut check named in the primary task: they aren't in the supplied lower cards or code slice.
- VoiceOver and any accessibility tree: no roles or labels were found in the app source, but screen reader output wasn't tested. Also not checked: macOS text-size or zoom behaviour, motion, and real displays.
- One reviewer finds only some problems; a second reviewer or a usability test will find more [NNG-HE].
