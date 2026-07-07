# zellijls

Fast zellij session lister: renders a table of sessions, their panes, and AI-agent activity without asking zellij itself — everything comes from disk state and the process tree.

## Language

**Session**:
One zellij session, live or exited. Live means a socket dir exists in the runtime dir; exited sessions exist only as session_info cache dirs.
_Avoid_: workspace, tab

**Server**:
The `zellij --server <runtime>/<session>` process owning one session. The root of that session's process subtree.

**Pane Shell**:
A direct child of the Server — one per pane, each on its own PTY. Usually a shell, but `zellij run -- cmd` panes spawn the command directly.
_Avoid_: pane process (ambiguous with Pane Command)

**Pane Command**:
The foreground process group leader on a pane's tty (the Pane Shell's `tpgid`). When the shell is idle, the Pane Command is the shell itself. This matches what zellij's `list-panes` reports as `pane_command`.
_Avoid_: spawn command, child command

**Agent Pane**:
A pane whose Pane Command is an AI agent (`claude`, `codex`, `codex-*`).

**Agent State**:
Working or Waiting. Working = spinner-prefixed title, high CPU, or established non-loopback TCP with some CPU; otherwise Waiting.

**Spinner Title**:
A pane title prefixed with a braille spinner glyph or `✳`, set by claude via OSC. The working-signal and the source of Task text.

**Task**:
The human-readable description of what an agent is doing, extracted from a Spinner Title after stripping the prefix and filtering default-title noise (session name, cwd basename, "Claude Code").

**Metadata KDL**:
`session-metadata.kdl` in a session's session_info dir. Rewritten every zellij tick; source of pane titles, pane ids, plugin flags, and connected client count.

**Creation-Order Binding**:
The rule associating Metadata KDL panes with Pane Shells: terminal pane ids and Pane Shell start times are both creation-ordered, so sorting each and zipping binds title↔process. Falls back to session-level association when the counts disagree.

**Session Age**:
Time since last user interaction, measured from PTY slave mtime (the kernel touches it on pane I/O). Metadata KDL mtime is not an age signal for live sessions — it refreshes every tick.
