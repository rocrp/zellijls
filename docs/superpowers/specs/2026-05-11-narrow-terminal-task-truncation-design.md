# Narrow-terminal task truncation

## Problem

`print_table` in `src/main.rs` lays out four fixed-width columns (`SESSION`, `STATUS`, `AGE`, `TASK`) with a hard 50-byte cap on the task string. On wide terminals the table reads cleanly. On narrow terminals (≲80 cols), rows wrap mid-cell and the table becomes unreadable. The current truncation is also byte-indexed (`&s.task[..49]`), which panics if a 50-byte boundary lands inside a multi-byte UTF-8 character.

## Goal

The table fits the terminal width by shrinking the task column. Wide terminals behave exactly as today.

## Approach

Detect terminal width via `crossterm::terminal::size()` (already a dependency). Allocate the task column whatever space remains after the other columns and their separators.

```
task_budget = term_width − (max_name + max_cmd + max_age + 6)
              ^^^^^^^^^^                                  ^
              cols only                                   2-space separators ×3
```

Apply these rules to the task column only:

1. `cap = min(50, task_budget)` when terminal width is known; otherwise `cap = 50` (preserves current behavior on piped output / non-TTY).
2. Truncate by **display width** using `unicode_width::UnicodeWidthChar` (already a dependency), not byte length. Replace the trimmed tail with `…` only when truncation actually occurred. This also fixes the latent multi-byte panic.
3. If `cap < 4` (no room for even one char plus the ellipsis), drop the task column entirely for this render: omit the `TASK` header, do not print the trailing task cell on any row, and shorten the divider's `━` run to match.

The other columns (`SESSION`, `STATUS`, `AGE`) keep their content-derived widths. If those three alone exceed the terminal width, rows wrap naturally — out of scope.

## Affected code

- `src/main.rs::print_table` — body change only; no signature change, no new helpers exported.
- New private helper `truncate_display_width(s: &str, max_width: usize) -> String` (or inline; whichever is cleaner). Lives in `main.rs` alongside the existing `display_width`.

No new dependencies. No changes to `Session`, `cmd_summary`, or any module besides `main.rs`.

## Behavior matrix

| Terminal width | Result |
|---|---|
| Wide enough that `task_budget ≥ 50` | Identical to today |
| `4 ≤ task_budget < 50` | Task truncated to `task_budget` display cells with `…` suffix; other columns unchanged |
| `task_budget < 4` | TASK column omitted entirely (header, divider extent, all rows) |
| Width detection fails (pipe, no TTY) | Identical to today (50-cell cap, no column-drop) |

## Non-goals

- Reflowing or hiding `SESSION` / `STATUS` / `AGE` when extremely narrow
- Multi-line layouts
- Persistent user preference for narrow-mode behavior
- Changing the picker (`pick.rs`) — only the default table render

## Testing

Manual verification by resizing the terminal (or piping through `stty cols N; zellijls`) at widths 200, 100, 80, 60, 40. Sanity check: piped output (`zellijls | cat`) renders identically to the pre-change build.
