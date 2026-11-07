## Verdict

macOS desktop, pointer and keyboard. This is a native GPUI Settings window, reviewed from the 12 captures (1400x1000, 900x680, 720x460; light and dark; board up, stopped and stand-in) plus the code excerpts. Nothing was driven with real input, because there is no page to open. The blur audit could not be run, so this first impression comes from viewing the captures: each card is tidy, but the screen reads as a stack of forms rather than a status view. With nothing connected, three always-open token forms fill the default window, Slack is below the fold, and four blue buttons compete. Counts: 1 Catastrophe, 5 Major, 9 Minor, 1 Cosmetic. Top problem: Disconnect fires on one click, with no confirm or undo, 8px from Check, and it forgets a token the app says it never shows again. Close behind: at the 720x460 minimum, Sign in with GitHub is cut off and Connect is gone. When the board is stopped, every account falsely reads as not signed in.

## Findings

### R1 · 4 Catastrophe · Disconnect removes the token on one click, with no confirm or undo
- **Element:** Accounts › any signed-in card › Disconnect, all sizes (acct-normal-light-m.png, acct-warm-light-m.png; settings.rs signed_in_row)
- **Evidence:** Confirmed
- **Problem:** One click posts accounts/{id}/disconnect immediately. The button is 24px tall and sits 8px from Check, the safe action the owner reaches for most on this row. A slip forgets the token, and agents on a task lose their push and API access while they are running. Bitbucket, Azure and Slack then need a token re-pasted or a new one made, because the app never shows tokens again. No message says what happened; the card just turns back into a form.
- **Principle:** Destructive actions are separated from safe ones and can be undone or ask to confirm [NNG-H3] [NNG-H5]; severity anchor: destructive action with no confirm or undo next to a frequent one
- **Fix:** Make Disconnect two-step, inline in the row. The first click swaps Check and Disconnect for 'Disconnect GitHub? Agents on a task lose access.' with Cancel and a kit::btn_danger 'Disconnect'. Move Disconnect apart from Check with at least 16px, or put it behind a '…' menu. After it completes, show a note 'Disconnected {name}.' Also replace the hard-coded tooltip, which talks about gh and git on the Azure and Slack cards too, with per-service text.

### R2 · 3 Major · At the minimum window, Sign in with GitHub is clipped and Connect is gone
- **Element:** Accounts › GitHub, Bitbucket, Azure and Slack cards › controls at 720x460, both themes (accounts--light-s.png, accounts--dark-s.png, down-accounts-s.png)
- **Evidence:** Confirmed
- **Problem:** The card is about 436px wide with 404px inside its padding. The row needs a 150px label minimum, a 16px gap and flex_none controls of about 340px, and kit::card's overflow_hidden cuts the excess with no scrolling. Only 'Sign' of the primary button shows. The token field runs off the card, and Connect starts past the card's edge. Enter in the field still submits, and widening the window fixes it, which is why this is 3 and not 4, but nothing tells the owner either.
- **Principle:** Every control at every supported size; the layout reflows across the whole window range [HIG-MACOS] [NNG-H6]; severity anchor: feature missing at one size class
- **Fix:** Let rows wrap: add flex_wrap to row() and make controls() wrap too. Give the field flex_1 with min_w(140.) and max_w(260.) instead of w(260.). Below about 560px of content width, stack the controls under the label, full width, with the primary button first. Remove overflow_hidden from the card, or clip only the corners, so a future overflow can't hide a control.

### R3 · 3 Major · The page is a stack of open forms, so account status can't be read at a glance
- **Element:** Accounts › whole page and the Bitbucket, Azure and Slack cards at 900x680 and 720x460 (accounts--light-m.png, accounts--light-s.png, acct-normal-light-m.png)
- **Evidence:** Confirmed
- **Problem:** Each unconnected service permanently shows a full form with a four-line scope paragraph, even if the owner never uses it. Connected accounts get pushed down, Slack and the Keychain note fall below the fold at the default size, and only about 1.5 cards show at the minimum. A sidebar badge of 1 can point at a card that is off screen. Checks that fail: frequent in view and rare behind [SEC-8]; one emphasis, with four primary fills on one screen [SEC-5]; one fact, one place, with 'gh isn't changed' in the GitHub note, the tooltips and the footer [SEC-7]; and one shape per role, since GitHub says 'Not signed in' while the other three show no status line at all [SEC-6].
- **Principle:** The work dominates and everything earns its place [SCR-1] [SCR-5]; frequent in view, rare behind [SEC-8]; recognition rather than recall [NNG-H6]
- **Fix:** Give every card the same first row: a status ('Not connected', or the signed-in name) with its actions on the right. Collapse Bitbucket, Azure and Slack to that row plus a secondary 'Connect…' button that expands the form and focuses its first field. All four cards then fit at 900x680. Keep a primary fill only on an account that needs the owner, such as Sign in again. State the Keychain and gh facts once, in the footer, and cut them from the GitHub note. Put the scope text in the expanded form only.

### R4 · 3 Major · Buttons and sidebar items can't be reached from the keyboard
- **Element:** Accounts › Sign in with GitHub, Use my gh sign-in, Use a token, Connect, Check, Disconnect, Cancel, Copy, links, Agent commands, sidebar sections (kit.rs btn_base and link; settings.rs sidebar, page_header, cli_line)
- **Evidence:** Confirmed
- **Problem:** These are all plain divs with on_click: no focus handle, no tab stop, no key handler, no focus style. Only text fields take focus. In a token form Enter works from the field, but Sign in with GitHub, Check, Disconnect and switching section need the mouse. The field's focus state is also only a 1px border colour change. This is a keyboard-first owner on a keyboard platform.
- **Principle:** Keyboard reach and operation [WCAG 2.1.1] [MS-KBD]; focus visible [WCAG 2.4.7]; role and state exposed [WCAG 4.1.2]
- **Fix:** Fix it once in kit::btn_base and kit::link: give each button a FocusHandle with a tab stop, activate on Enter and Space, and draw a 2px t.accent ring on focus. Apply the same to Agent commands, the Copy control and the code chip. Make the sidebar one tab stop with Up and Down moving between sections. Check that Tab follows reading order within each card.

### R5 · 3 Major · With the board stopped, every account reads 'Not signed in' and the forms stay live
- **Element:** Accounts › whole page, board stopped (down-accounts-m.png, down-accounts-s.png; accounts_page early return, account() returning Null)
- **Evidence:** Confirmed
- **Problem:** accounts_page only stops early when there is no error. On error it renders Null accounts, so a signed-in owner sees 'Not signed in' and empty forms, and may start re-entering tokens that are fine. Card headings fall back to raw ids ('github', 'azure'). 'Use my gh sign-in' shows without knowing gh is installed. Every action looks enabled but can only fail. The banner uses the daemon name 'taskboardd', offers no next step, and the sidebar says 'Connecting…' while the banner says it can't reach the server.
- **Principle:** Status must be true: in the error state, commands never report what they didn't have [NNG-H1]; help users recover from errors [NNG-H9]; plain language [NNG-H2]; a simulated or error state that contradicts itself is a finding
- **Fix:** When !loaded and error is set, replace the cards with one panel: 'Can't reach the board, so sign-ins can't be shown. Your saved accounts aren't affected.', plus the way to start it and a Try again button that calls load(true). Set the sidebar to 'Not connected' in this state. If accounts were already loaded when the board drops, keep them but disable their actions. Take heading names from CARDS rather than the answer's label.

### R6 · 3 Major · Green push row stays on when the sign-in has been refused
- **Element:** Accounts › GitHub card › push row, reauth state, 900x680 (acct-warm-light-m.png; github_rows calling push_row, push_row uses t.up)
- **Evidence:** Confirmed
- **Problem:** push_row is added for every connected GitHub or Bitbucket account and always has a green dot. In acct-warm it reads 'Agents on a task git push to github.com with this account' directly under 'GitHub refused the saved sign-in' and an amber row. The owner's job here is to keep agents working, and the page says both that pushes work and that they won't. The refusal itself is small muted text joined to 'Taskboard's own token'.
- **Principle:** Visibility of system status [NNG-H1]; one fact, one place, no copies that can disagree [SEC-7]
- **Fix:** Pass the account's health into push_row. When reauth is true or error is set, use the t.warn dot and 'Agents can't push to github.com until you sign in again.' Keep the green dot and current sentence for the healthy state only. Move the refusal reason from the signed-in row's note into the amber row.

### R7 · 2 Minor · The Needs a new sign-in row can read as broken and offers a route that doesn't exist
- **Element:** Accounts › GitHub card › Needs a new sign-in row (acct-warm-light-m.png; reauth_row, github_rows)
- **Evidence:** Confirmed
- **Problem:** The sentence is built as 'Taskboard now needs {list}.', so an empty list gives 'Taskboard now needs . Sign in again'. That text is in acct-warm, which is stand-in data, so whether the real board sends an empty list is unknown, but the template has no guard. It also explains missing scopes when the shown cause is a refused sign-in. It says 'or use a token that has them', yet with gh present the row has only Sign in again and no way to open the token form. When the list isn't empty, the chips repeat it.
- **Principle:** Help users diagnose and recover from errors [NNG-H9]; users notice the correct action is available [NNG-CW]; one fact, one place [SEC-7]
- **Fix:** Branch on the cause. Empty list: 'GitHub refused the saved sign-in. Sign in again to keep agents working.' List present: 'Taskboard now needs repo, workflow.' with no chips. Add a secondary 'Use a token' button beside Sign in again, setting gh_form, or drop the phrase.

### R8 · 2 Minor · A connected account with a bad token can only be fixed by disconnecting it first
- **Element:** Accounts › signed-in cards, error or expiring token (github_rows, bitbucket_rows, azure_rows, slack_rows; needs_owner)
- **Evidence:** Confirmed
- **Problem:** The token form opens on a connected account only through a reauth button. If error is set without reauth, the card shows an amber message and only Check and Disconnect. slack_rows has no reauth branch at all, yet needs_owner still counts it, so the sidebar badge could point at a card with no amber row. Rotating a working token also means disconnecting first, which leaves agents without access in between. Whether the board sets error without reauth for an expired token wasn't visible.
- **Principle:** Error with a way to recover [NNG-H9]; user control [NNG-H3]
- **Fix:** Add a 'Replace token' item to every signed-in row (a secondary button or a '…' menu) that opens that service's form and keeps the current token until the new one passes its check. Show the same as the primary action in the error row. Give Slack the same reauth row as the others.

### R9 · 2 Minor · Check shows no result, and success notes never clear
- **Element:** Accounts › signed-in row › Check, and the card note row (signed_in_row, post, connect)
- **Evidence:** Confirmed
- **Problem:** Check's success callback is empty, so a healthy result looks like nothing happened: 'Checking…' just goes back to 'Check'. The owner can't tell a good result from a dropped click, and this is the main way to confirm that agents will keep working. In the other direction, notes such as 'Connected as…' stay until the next post for that account. In a window left open all day, they become stale claims.
- **Principle:** Visibility of system status; after the action, users see progress [NNG-H1] [NNG-CW]
- **Fix:** On a successful check, set a note 'Token works · checked 2:41 PM' (12-hour, with AM/PM). Clear success notes after about 8 seconds or when the owner leaves the section; keep error notes until resolved.

### R10 · 2 Minor · Actions are silently dropped while another account is busy
- **Element:** Accounts › Connect, Check, Disconnect, Sign in, Cancel across cards (post; busy compared per id)
- **Evidence:** Confirmed
- **Problem:** post() returns at once if any account is busy, but each button only disables for its own id. Pressing Connect on Slack while GitHub is checking looks enabled and does nothing, with no message. Cancel in the code panel clears opened_code before its post is dropped, so the panel stays and the next accounts answer copies the code to the clipboard again.
- **Principle:** Pressing a control responds visibly [NNG-H1]; error prevention [NNG-H5]
- **Fix:** Make busy a set of account ids so accounts run independently. If one at a time must stay, disable every account action while busy is set, and in Cancel do the local reset only after the post is accepted.

### R11 · 2 Minor · Connect's validation error is far from the field and reports one problem at a time
- **Element:** Accounts › Bitbucket card › Connect with empty fields, 900x680 (accounts-connect-empty-light-m.png; note row in accounts_page)
- **Evidence:** Confirmed
- **Problem:** 'Paste a token first.' appears at the very bottom of the card, under the link row, and the fields aren't marked. Focus doesn't move. In the capture the email is empty too, but only the token is mentioned. At short window heights the note can land below the fold with nothing scrolling to it (inferred from the layout, not captured). The check also needs a round trip to the board.
- **Principle:** Errors name the field, say how to fix it, next to the field [WCAG 3.3.1] [WCAG 3.3.3] [NNG-H9]; prevent errors [NNG-H5]
- **Fix:** Validate empty fields in connect() before posting. Show the message directly under the empty field, in t.down with a t.down field border. Cover every empty field at once, for example 'Enter your Atlassian email' and 'Paste an API token'. Focus the first empty field and scroll it into view. Keep the card-level note for errors from the service.

### R12 · 2 Minor · An opened token form can't be closed
- **Element:** Accounts › GitHub Token form and Use a new token forms (accounts-use-token-light-m.png; github_rows, field_keys)
- **Evidence:** Confirmed
- **Problem:** Use a token disappears once the form opens, and nothing replaces it. Esc only clears the field, so the form stays until a successful Connect. In the reauth path the amber row's explanation also disappears for Bitbucket and Azure while the form is open. A successful connect on any account closes all three forms, which hides text typed into another form. Esc also wipes a pasted token with no undo.
- **Principle:** User control and freedom [NNG-H3]; unsaved input survives in-app actions [NNG-H5]
- **Fix:** Add a Cancel button beside Connect that sets the form flag false and clears only that field. Make Esc do the same on an empty field, and only clear on a first press if the field has text. In connect(), reset only the flag for the id that connected. Keep the amber row visible above the form.

### R13 · 2 Minor · Agent commands repeats, truncates and hides its own toggle
- **Element:** Accounts › Agent commands toggle and command lines, 900x680 (accounts-agent-commands-light-m.png; cli_line, page_header)
- **Evidence:** Confirmed
- **Problem:** 'tb accounts' is repeated in every card, adding two boxes per card to an already long page. The useful line is cut off ('repos/OWNER/REPO/…') with no way to read the rest. The tooltip covers the toggle's own label and still says 'Show' while the commands are on. Copied state is matched on the text, so copying one 'tb accounts' flips all four to Copied. Commands also show on accounts that aren't connected, where they can't work.
- **Principle:** One fact, one place [SEC-7]; recognition rather than recall [NNG-H6]; state is shown [NNG-H1]
- **Fix:** Show 'tb accounts' once at the top of the page and keep only the agent_cli line in each card, and only when that account is connected. Use kit::clipped so a cut line shows in full on hover, or wrap it. Key the copied flash on the line id. Make the tooltip 'Hide the commands…' when on, and anchor it below the button so it doesn't cover the label.

### R14 · 2 Minor · Scope chips look like buttons and silently drop scopes past six
- **Element:** Accounts › signed-in row › scope chips, 900x680 (acct-normal-light-m.png, acct-warm-light-m.png; signed_in_row, kit::chip)
- **Evidence:** Confirmed
- **Problem:** The chips share the outlined rounded look of Check and Disconnect, sit right next to them and wrap to two rows, so read-only labels look clickable and take the row's main space. take(6) hides any further scopes with no '+n', which matters when the question is whether a token has a scope.
- **Principle:** One look per role; no label that looks like a button [SEC-4]; frequent in view, rare behind [SEC-8]
- **Fix:** Drop the chips from the signed-in row. Show scopes as plain muted text in the note line ('repo, read:org, workflow, gist…') with the whole list in a kit::clipped tooltip, or behind a 'Scopes' disclosure. Use the same treatment in off_row.

### R15 · 2 Minor · Placeholders and link-row URLs look under 4.5:1
- **Element:** Accounts › field placeholders and the URL beside each link, light theme (accounts--light-m.png; t.faint in link_row)
- **Evidence:** Likely
- **Problem:** Placeholders such as 'ATATT…' carry format hints, and the URLs show where a link goes, but both are pale grey on white, around 3:1 by eye. Theme values weren't in the excerpts, so this is not measured.
- **Principle:** Text contrast at least 4.5:1 [WCAG 1.4.3]
- **Fix:** Compute t.faint and the placeholder colour against t.card in both themes. If either is under 4.5:1, use t.muted for link_row's URL and the placeholder.

### R16 · 1 Cosmetic · Small inconsistencies in the cards
- **Element:** Accounts › GitHub Token row and button heights (accounts-use-token-light-m.png; kit.rs, settings.rs)
- **Evidence:** Confirmed
- **Problem:** The GitHub Token row has no divider above it because first is passed as true, unlike every other multi-row card. Buttons in one card are 24, 26 and 32px tall. The Azure placeholder 'Personal access token' only repeats the label above it, which breaks the no-filler rule.
- **Principle:** One look per role [SEC-4]; alignment [SCR-4]; no filler labels (brief rule)
- **Fix:** Pass first as false for the GitHub token row. Use one height for buttons in a row, 26px beside small buttons and 32px beside fields. Replace the placeholder with a format hint, or leave it empty.

## What works

- The GitHub code panel flow, as read in code, is well thought out: the code is shown and copied before the browser opens, it says what to press, and it says what happens next, including the 15-minute limit.
- Each card heading sits above its members with the service mark and a one-line purpose, which follows lesson L1 and makes the cards easy to scan.
- 'Use my gh sign-in' with a primary browser sign-in is a good set of routes for a developer, with the primary one clearly marked.
- Dark mode holds up in the captures: no lost borders or hard-coded light surfaces, and error and warning rows stay readable by eye.
- Token fields are secret and never copy out, and a failed connect keeps the typed text.

## Not checked

- Real input: this is a native GPUI app with no page or URL, so the driver wasn't used and no control was operated. Behaviour findings come from code and captures. inspect, the composition audit and the blurred screenshot were not run.
- Contrast was not measured: theme colours weren't in the excerpts, so R15 is an estimate. Icon, focus-ring, badge and chip contrast are also unmeasured.
- The GitHub code panel has no capture (code only): its look, its fit at 720x460, the expired-code state, and how the 'updates by itself' polling works. Nothing in the excerpts shows an expired-code message.
- Signed-in, reauth, error and Agent commands states at 720x460 and 1400x1000 and in dark mode: only light 900x680 captures exist, so chip squeezing and reauth row fit at the minimum are unseen. Bitbucket connected, Slack connected and the no-gh path have no capture either.
- Screen reader output and the macOS accessibility tree: GPUI divs seem unlikely to expose roles or names, but that wasn't tested. Status announcement (WCAG 4.1.3) is unchecked for the same reason.
- TextField was not in the excerpts: Tab between fields, paste, and any app-level key bindings or menu commands. No menu-bar coverage or shortcuts were seen.
- How often the page refreshes, what load() is called from, whether anything retries when the board is down, and whether the board sends reauth with an empty list or error without reauth for expired tokens.
- Agent_cli text, CARDS and the Slack reauth payload were not in the excerpts. Numbers and reason wording in acct-* captures are stand-in data.
- Capture labelling: down-accounts-s.png is listed as light in captures.txt but the image is dark, so no light capture of the stopped board at 720x460 was seen.
- Larger text, a very long account name or error text, motion, and real devices or displays. The usual checks for touch, orientation, 320-wide reflow and on-screen keyboard don't apply to this macOS window.
- One reviewer finds only some of the problems; a second reviewer or a usability test will find more [NNG-HE].
