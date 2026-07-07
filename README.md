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
- **Fast**: process inspection via `sysinfo` + `netstat2` (no `lsof`/`ps` subprocess calls)

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
2. `zellij -s <name> action list-panes --all --json` → pane commands, CWDs, titles (2-wide batches; corrupt concurrent responses are retried sequentially)
3. `sysinfo` crate → find agent PIDs + CWDs (single in-process scan)
4. `netstat2` crate → check established remote TCP connections per PID (single syscall)
5. Match agent PIDs to sessions by CWD, render table
