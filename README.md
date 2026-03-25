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
- **Fast**: process inspection via `sysinfo` + `netstat2` (no `lsof`/`ps` subprocess calls)

## Install

```sh
cargo install --path .
```

## How it works

1. `zellij ls --no-formatting` → session list + age + exited state
2. `zellij -s <name> action list-panes --all --json` → pane commands, CWDs, titles (sequential — zellij drops fields under concurrent queries)
3. `sysinfo` crate → find agent PIDs + CWDs (single in-process scan)
4. `netstat2` crate → check established remote TCP connections per PID (single syscall)
5. Match agent PIDs to sessions by CWD, render table
