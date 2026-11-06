# Lessons

Read this before you add or confirm a lesson, or when the owner asks about them.

A goal note is for this goal's tasks; a lesson is for later work: a trap in the code, a step that keeps failing, a plan that had to be redone. One or two sentences, saying what to do.

- `tb lesson add "<what to do differently>" --kind planning|code|process|tooling` suggests one (`--global` for every project).
- It's suggested until the owner keeps it (`tb lesson keep L<n>`, only on their word). Kept ones reach later handoffs in the project (the few that fit the task best, by its words and the code it's likely to touch), and planning ones reach planners.
- Later work that shows the same thing again: `tb lesson confirm L<n> --evidence T<n>`.
- `tb lessons` lists them; `tb lesson search "<words>"` finds the ones that fit.

Lessons also look after themselves: after each goal's retrospective, a suggested lesson confirmed by two goals is kept, one that five goals in its scope went by without confirming is retired, and a project lesson confirmed in two projects is marked "make global?" for the owner.

`tb planning --suggest` shows the edit that writes the project's kept planning lessons into its planning file; `tb planning --accept` writes it, only on the owner's word. `tb quality` shows how well goals were planned, by month.
