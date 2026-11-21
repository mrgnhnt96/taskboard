# Fixes after the fix-check review (review-2)

Two review passes is the limit, so these were checked by hand, not by a third review.

| # | Finding | Fix | Checked |
| --- | --- | --- | --- |
| R1 | Offline: shortcuts, menus and New idea still changed the board | One guard at the top of every action; any toast that reports a change counts; Add idea is off and keeps the draft | ⌘↩ send and ⌘N → Add idea show the offline toast; the row stays |
| R2 | ⌘Z undid an action other than the one the toast named | The undo stack holds the toast's own action; each undo says what it undid and names the next | Wave start then Dismiss: ⌘Z restores the dismiss, then the wave start |
| R3 | No visible way to close Settings | Close button at the top of the Settings sidebar (⌘W) | Visible at 1536 and in the rail at 760 |
| R4 | Esc in a Settings field left Settings | Esc closes only when focus is not in a field | Esc in a field stays on Settings |
| R5 | Picking an option erased the typed answer | The draft stays; Send names what goes ("Send “Yes, always”", "Send your answer") | Typed, picked: draft kept, label follows |
| R6 | Idea answers carried over between questions | One draft per question | Question 2's field starts empty |
| R7 | Focus lost after a page or Settings section change | Focus goes to the new page's heading; the sidebar lists are one tab stop with arrow, Home and End keys | ⌘2 focuses the Goals heading; one tab stop per list |
| R8 | Goal names broke mid-word at 200% text | Names wrap between words; the count drops below | Screenshot at 200% |

Also fixed while checking: the Settings sidebar picked up the main sidebar's narrow-window style at 840–1200px.
