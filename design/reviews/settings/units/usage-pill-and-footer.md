## Verdict

macOS desktop (pointer and keyboard), sidebar footer at 1360x860 and 900x560, reviewed from 7 captures plus code excerpts. The app itself wasn't run, so nothing was clicked, hovered or tabbed; the browser was used only to compute contrast. First impression: the main window is calm, and the footer reads as a loose stack of unrelated pills rather than one organised health area. In the normal state it holds together, but as soon as the daily limit is on, the usage pill runs off the sidebar and is cut at "Toda". Counts: 1 Catastrophe, 3 Major, 5 Minor, 1 Cosmetic. Top problem (R1): the pill is too wide for the fixed 248px sidebar once Today is added, so the daily reading and the Lifted state never show at any window size, and the visible part can stay green while the day's allowance is the thing running out. R1's 4 is a Major raised one step under the severity guide, because it shows on every page at every size; a hover tooltip still carries the daily line. Next in line: a click goes to Days rather than to the Settings window where limits are set, and the window labels and the stale state fail contrast.

## Findings

### R1 · 4 Catastrophe · The Today segment is cut off at the sidebar edge, so the daily limit and Lifted never show
- **Element:** Main window › sidebar footer › usage pill › Today segment, whenever the daily limit is on, at 1360x860 and 900x560, light and dark (pill-daily-light-m.png, pill-warm-light-m.png, pill-hot-light-m.png, pill-hot-dark-s.png)
- **Evidence:** Confirmed
- **Problem:** The pill shows '5h … | 7d … | Toda' and stops at the sidebar's edge: the Today bar and value are gone, and the pill loses its rounded end. The two-window pill is already about 211px of the 224px available, and pill_shell is flex_none and whitespace_nowrap, so it can neither wrap nor shrink. Because the sidebar is a fixed 248px, no window size fixes this. The value that says how much of today is left, or 'Lifted', is visible only by hovering. In pill-daily-light-m.png both visible windows are green, so the pill reads as fine whatever Today's level is.
- **Principle:** Nothing is cut off or lost, and every state of a control is visible at every size [WCAG 1.4.10] [NNG-H1]; recognition rather than recall [NNG-H6]; narrowing keeps the order [SCR-6]
- **Fix:** Change usage_pill from one inline row to a full-width block (w_full, so it fills the 224px) with one flex_1 min_w_0 column per window. Each column has a 15px line with the label on the left and the value on the right, and a 4px full-width bar under it. Cut the segment padding from 7px to 5px. Use short values: 'Today 88%' or 'Today Lifted', with whole percents only. That comes to about 204-214px wide and only about 5px taller than today, which matters because 900x560 has only about 142px left between the nav and the footer. Avoid one row per window: it would add about 36px, and that space comes out of the Active Goals list. Check the width with 100% in all three columns.

### R2 · 3 Major · Clicking the pill goes to Days, not to where limits are set
- **Element:** Main window › sidebar footer › usage pill (usage-pill-go), all states
- **Evidence:** Confirmed
- **Problem:** The click runs m.go(Page::Days) (sidebar.rs line 241). The primary task is to check and then adjust usage limits, and the Settings window is the only place to set them. So an owner who sees 96% and clicks lands on a page that, under that rule, can't change the limit, and must then find Settings separately. It also breaks the footer's own pattern: the accounts pill opens Settings and the update pill opens Settings › System. Nothing announces the destination either. The tooltip lists only readings, and the pill has no hover style, unlike the Settings link beside it, so it looks like a passive meter.
- **Principle:** Will users associate the action with the result they want? [NNG-CW]; consistency and standards [NNG-H4]; one home per object (brief rule); hover and pressed states designed [NNG-H1]
- **Fix:** Dispatch a new OpenUsageSettings action, modelled on OpenSystemSettings, that opens the Settings window on the pane holding the usage limits. Add a last tooltip line, 'Click to change limits', and a hover style (background t.border, or a t.border_2 border in the pill's existing transparent 1px border). If Days holds usage history worth keeping, link to it from that Settings pane.

### R3 · 3 Major · The window labels fail text contrast in every state
- **Element:** Main window › sidebar footer › usage pill › '5h', '7d', 'Today' labels, and the 'Lifted' value, light and dark (pill-normal-light-m.png, pill-hot-dark-s.png)
- **Evidence:** Confirmed
- **Problem:** Labels are 11.5px in t.faint on t.seg: 2.57:1 in light and 3.36:1 in dark, against the 4.5:1 required. These labels are the only thing that says which limit each number belongs to, and the owner reads them all day. 'Lifted' in t.accent is 4.31:1 in light, also under. The empty bar track is nearly invisible too (1.25:1 light, 1.44:1 dark) and the green fill is 2.96:1 on it in light, though the number beside it carries the same information.
- **Principle:** Contrast (minimum) [WCAG 1.4.3]; non-text contrast [WCAG 1.4.11]
- **Fix:** Set the labels to t.muted (4.58:1 light, 6.30:1 dark). Set the Lifted value to t.accent_fg (6.64:1 light, 7.90:1 dark), keeping t.accent for the bar.

### R4 · 3 Major · The stale state is too faint to read and never says it is stale
- **Element:** Main window › sidebar footer › usage pill, stale state (pill-stale-light-m.png)
- **Evidence:** Confirmed
- **Problem:** A reading over an hour old only gets .opacity(0.6). Computed over the sidebar, values fall to 2.99:1 in light and labels to 1.69:1 (4.17:1 and 2.05:1 in dark). The pill then looks disabled rather than out of date, and it still shows '34%' as a plain number. 'Last read …' exists only in the tooltip. While many agents run, usage can climb a long way in an hour, so an owner glancing at a faded 34% can trust a number that is no longer true, and the fade also weakens warm and hot colours just when a stale reading is riskiest.
- **Principle:** Visibility of system status [NNG-H1]; contrast [WCAG 1.4.3]; colour isn't the only signal [WCAG 1.4.1]
- **Fix:** Remove .opacity(0.6). When stale, draw values in t.muted, draw bars in t.faint, and add a line inside the pill under the columns: 'Last read 3h ago' at 11.5px in t.warn_fg (6.18:1 light, 8.56:1 on dark seg). Use the pill's transparent 1px border in t.warn_line. Stale is rare, so the extra line costs little height.

### R5 · 2 Minor · Warm and hot barely differ and never rise above the footer
- **Element:** Main window › sidebar footer › usage pill, warm and hot states (pill-warm-light-m.png, pill-hot-light-m.png)
- **Evidence:** Confirmed
- **Problem:** The whole escalation is a recoloured 5px bar and a 12.5px number on the same grey pill. Warm and hot are close in hue: the bars are 1.27:1 apart in light and 1.35:1 in dark, and the values 1.11:1 in light, so only the semibold weight and the digits separate them. In pill-hot-light-m.png the filled '2 accounts need you' pill below is louder than 96% used. For an app left open all day, the state most likely to stop agents is easy to miss from the corner of the eye.
- **Principle:** Visibility of system status [NNG-H1]; one emphasis, with the strongest signal on what matters most [SEC-5] [NNG-VH]
- **Fix:** Tint the whole pill by its worst level: hot uses background t.down_soft with border t.down_line, and warm uses t.warn_soft with t.warn_line. Contrast holds: t.down on down_soft is 5.60:1 light and 5.46:1 dark; t.warn_fg on warn_soft is 6.37:1 and 8.50:1; t.muted labels are at least 4.62:1 on either. Put the soonest reset in the hot state, for example 'Resets 3 PM' on the stale-style second line.

### R6 · 2 Minor · The footer mixes looks and has no fixed places
- **Element:** Main window › sidebar footer, all states (pill-normal-light-m.png, pill-warm-light-m.png, pill-hot-light-m.png, down-main-light-m.png)
- **Evidence:** Confirmed
- **Problem:** Footer items flow in one flex_wrap, so position depends on what is showing. Settings sits at the far left in the normal state (x≈18 in the capture), moves to x≈222 beside the GitHub pill and x≈219 beside '2 accounts need you', so a fixed command drifts. Midna sits alone on a row with empty space beside it in these captures. Looks don't match role: Midna (status, no click), accounts (click) and update (click) share one filled pill shape, and only some have a hover style. Hooks is plain text with a dot, the Sample pill is 20px tall against 25-27px, and dots are 7px or 8px.
- **Principle:** One look per role [SEC-4]; grouped by function, with one job per section [SEC-1] [SEC-2]; alignment [SCR-4]; consistency [NNG-H4]
- **Fix:** Use three fixed rows. Row 1: Midna, work hours, hooks, then Settings pinned right with ml_auto. Row 2: the usage pill at full width (R1). Row 3, only when something needs attention: accounts, update, PR feed, Sample board. That is one row shorter in the normal state, which helps at 900x560. Give every clickable pill the same hover style, keep passive status without one, and use one dot size (7px) and one pill height.

### R7 · 2 Minor · The pill and its tooltip can't be reached from the keyboard
- **Element:** Main window › sidebar footer › usage pill and tooltip
- **Evidence:** Confirmed
- **Problem:** In the excerpts the pill is a div with on_click and a hover tooltip: no focus handle, tab stop or key handler. So the click can't be triggered from the keyboard, and reset times, the daily line and 'Last read' can't be reached without a pointer. The same goes for the accounts and update pills. This is rated 2 rather than 3 because the single owner has a pointer; the excerpts don't show whether a key binding exists elsewhere.
- **Principle:** Keyboard [WCAG 2.1.1]; content on hover or focus [WCAG 1.4.13]; name, role, value [WCAG 4.1.2]
- **Fix:** Give each clickable footer item a focus handle and tab stop, activate on Enter and Space, and draw a 2px t.accent focus ring. Show the same tooltip on focus and dismiss it with Esc. The readings then also need to exist in the Settings pane R2 opens, so they aren't locked in a tooltip.

### R8 · 2 Minor · Today counts down while the other windows count up
- **Element:** Main window › sidebar footer › usage pill › Today segment (code; hidden today by R1)
- **Evidence:** Confirmed
- **Problem:** usage_today draws the bar as the share used but prints what's left, so a nearly full bar reads '12% left' next to '96%', which means used. Higher means worse in two segments and better in the third, and a glance at the row gives the wrong picture. js_num also prints the raw number, so a fractional 'left' would widen the segment. This will surface as soon as R1 is fixed.
- **Principle:** Consistency and standards [NNG-H4]; match with the real world [NNG-H2]
- **Fix:** Print used share rounded to a whole percent, 'Today 88%', matching the bar and the other windows. Keep the 'left' wording in the tooltip line ('12% of today left').

### R9 · 2 Minor · Tooltip reset times break the AM/PM rule
- **Element:** Main window › sidebar footer › usage pill › tooltip, 'resets …'
- **Evidence:** Confirmed
- **Problem:** usage_view builds reset times with day_clock, which uses clock12 and gives '3pm' or 'Thu 6am'. The brief's rule is 12-hour times with AM/PM, and hhmm elsewhere in the same file prints '3:05 PM', so two styles coexist. Reset time is what the owner plans around when usage is hot.
- **Principle:** 12-hour times with AM/PM (brief rule); consistency [NNG-H4]
- **Fix:** Change clock12 to return '3 PM' and '9:30 AM' (uppercase, with a space). That also updates day_clock ('Thu 6 AM') and day_time. Update the parity goldens if they pin the old text.

### R10 · 1 Cosmetic · Settings shifts 6px when the board is down
- **Element:** Main window › sidebar footer › Settings (down-main-light-m.png against pill-normal-light-m.png)
- **Evidence:** Likely
- **Problem:** Settings starts at x≈25 in the capture with the board stopped and x≈18 otherwise, about 6 CSS px. That matches the footer's 6px gap, which suggests an empty hours pill still takes a slot before it. Separately, with the board down the pill vanishes and no last reading stays; the banner explains why, so this is acceptable. Note that usage_view also returns None, with no message, when the board is up but has no reading.
- **Principle:** Alignment [SCR-4]; visibility of system status [NNG-H1]
- **Fix:** Have hours::pill return an Option and skip it when empty; R6's fixed rows also remove the shift. For a running board with no reading, show a muted 'No usage reading' pill with the reason in its tooltip.

## What works

- The normal state is compact and quiet, and each window pairs a label, a bar and a number, so level never depends on colour alone.
- Warm and hot recolour both the bar and the number, and hot adds weight, in both themes.
- Value colours pass contrast in every live state: 8.07:1 normal, 6.18:1 warm and 5.55:1 hot in light; 8.88:1, 8.56:1 and 5.14:1 in dark.
- The whole pill is one click target about 27px tall by code, above the 24px floor.
- The tooltip spells out '5-hour limit: 34% used, resets …', so the short labels have a full form on hover.

## Not checked

- Real input: the native app wasn't available, so no click, hover, tooltip display, focus or tab order was exercised. Click targets and keyboard findings come from the code excerpts.
- Lifted state: the brief mentions usage-lifted, but captures.txt lists no such capture; it was reviewed from code only, and Lifted would be clipped by R1 anyway.
- Stale in dark, warm in dark, and every state except hot at 900x560: no captures. Stale dark contrast is computed from theme tokens. The sidebar's fixed width means R1 should hold at any size.
- What the Days page shows, and the Settings window's panes: not in the excerpts, so R2's claim rests on the brief's rule that Settings is the only place to set limits, and the pane name for OpenUsageSettings is unknown.
- The work-hours pill: the code puts it in the footer, but it isn't visible in any capture, and hours::pill isn't in the excerpts. A filled footer (work hours, hooks, PR feed, update, Sample board together) wasn't seen, so wrapping height at 900x560 is unconfirmed.
- Active Goals sharing the sidebar with the footer: the captures have no goals, so the roughly 142px free at 900x560 is measured from pill-hot-dark-s.png without a populated list.
- Stand-in data: numbers and reasons in pill-* captures are made up, and the daily 'line' tooltip text and how a reached daily limit affects agents aren't in the code given.
- Screen reader output, including whether the UI framework exposes any accessibility tree; motion; 200% text; the macOS Increase Contrast setting; real devices.
- Composition audit and inspect: they need a web page, so composition was judged from the captures, sizes by eye and from code, and contrast was computed from theme tokens.
- Skipped as not applicable: forms, drag, scrolling, destructive actions, long-list virtualization.
- One reviewer finds only some of the problems; a second reviewer or a usability test will find more [NNG-HE].
