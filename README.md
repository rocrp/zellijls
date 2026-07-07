# zellijls

Human-friendly zellij session listing. Shows running programs, AI agent status, and active tasks.

```
SESSION  STATUS         AGE  TASK
━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━
aris     claude 💤      15h  enhance-chat-handler-memory-context
rccc     claude 🏗️ +1sh  1h  Debug soul question understanding
ifiles   claude 🏗️       1h  Enhance zellij ls output
vas      idle           40m
hntui    exited          4h
```

## Features

- **Agent detection**: finds `claude`/`codex` processes, shows 🏗️ (working) or 💤 (waiting)
- **Working/waiting**: checks TCP connections via `netstat2` — active API call = working, no connection = waiting
- **Task extraction**: reads pane title set by Claude Code (spinner prefix stripped)
- **Attached sessions**: session name is underlined when a client is currently attached (read from zellij's `session-metadata.kdl`)
- **JSON output**: `zellijls --json` emits script-friendly session data without ANSI
- **Fast**: everything from disk state + the process tree — zero subprocess spawns, ~0.15s

## Install

Homebrew (macOS / Linux):

```sh
brew install rocrp/tap/zellijls
```

Install script (macOS / Linux):

```sh
curl -fsSL https://raw.githubusercontent.com/rocrp/zellijls/main/scripts/install.sh | bash
```

From source:

```sh
cargo install --path .
```

## Usage

```sh
zellijls              # table
zellijls pick         # choose and attach
zellijls watch        # auto-refreshing dashboard
zellijls --json       # machine-readable array
zellijls --help
zellijls --version
```

`--json` fields:

- session: `name`, `exited`, `current`, `attached`, `age_seconds`, `age`, `status`, `agent_state`, `task`, `panes`
- pane: `command`, `cwd`, `agent`, optional `state`
- states: `"working"`, `"waiting"`, or `null`

## How it works

1. `session_info` + runtime dirs → session list + age + exited state
2. `session-metadata.kdl` → connected clients + pane titles (ids are creation-ordered)
3. Process tree → each session's `zellij --server` children are the pane shells; one probe per shell (macOS `proc_pidinfo`, Linux `/proc/<pid>/stat`) yields tty (age from PTY slave mtime), foreground process group (the pane command + cwd, via `sysinfo`), and start time (creation order, for binding KDL titles to panes)
4. `netstat2` crate → check established remote TCP connections per agent PID (single syscall)
5. Render table

No zellij subprocess is ever spawned (see `docs/adr/0001`), which also sidesteps zellij 0.44.3 dropping `pane_command` under concurrent `list-panes` queries.
