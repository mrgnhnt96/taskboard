# Human tasks

Read this when the task needs something only the owner can do: make an account, sign an agreement, buy something, get a key from a site you can't reach.

- **Asking for it**: `tb task new "<what to do>" --human --detail "<numbered steps, each link written out, and what to send back>"`, then `tb wait-for H<n> --why "…"` and end your turn. The steps hold everything they need and nothing for you; one H task per thing they do.
- **Marking it done**: when the owner tells you they did it or to close it ("H4's done", "mark them done", "close H4"), `tb needs done H4` marks it done for them (several at once: `tb needs done H3 H4`); never on your own judgment.
- **Not a human task**: anything an agent can run (a release cut, a deploy, a `tb` command) is never a human task, even if the plan says not to do it yourself: make it an agent task (`--no-pr`) and `tb wait-for T<n>` on that.
- **Refused**: the board screens every new human task against the owner's rules and refuses one an agent could do (a setting behind a sign-in Claude in Chrome already has, a lookup through an MCP connector, a fact in the code) or the rules already settle: do what the refusal says instead.
