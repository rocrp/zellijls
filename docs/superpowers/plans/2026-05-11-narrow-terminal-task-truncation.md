# Narrow-terminal task truncation Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make `zellijls`'s default table render fit narrow terminals by shrinking (or dropping) the `TASK` column based on terminal width, while preserving existing behavior on wide terminals and non-TTY output.

**Architecture:** All changes are local to `src/main.rs`. Add one pure helper (`truncate_to_width`) that truncates a string to a display-width budget with a `…` suffix when shortened, fixing a latent byte-boundary panic in the existing code. In `print_table`, query terminal width via `crossterm::terminal::size()`, compute the remaining budget for the task column, and either truncate the task or omit the column entirely if no useful budget remains. Adjust header and divider accordingly.

**Tech Stack:** Rust 2024 edition. Already-present crates: `crossterm` (terminal size), `unicode-width` (display width).

**Spec:** `docs/superpowers/specs/2026-05-11-narrow-terminal-task-truncation-design.md`

---

## File Structure

Only one file is touched:

- `src/main.rs` — add `truncate_to_width` helper + its tests; modify `print_table` body.

No new files, no module changes, no new dependencies.

---

### Task 1: Add display-width-aware truncation helper

**Files:**
- Modify: `src/main.rs` (add helper near `display_width` around line 102; add `#[cfg(test)] mod tests` at end of file — none currently exists in this file)

- [ ] **Step 1: Write the failing tests**

Append to the bottom of `src/main.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn truncate_to_width_no_truncation_when_fits() {
        assert_eq!(truncate_to_width("hello", 10), "hello");
        assert_eq!(truncate_to_width("hello", 5), "hello");
    }

    #[test]
    fn truncate_to_width_ascii_truncates_with_ellipsis() {
        assert_eq!(truncate_to_width("Analyze hermes-agent", 10), "Analyze h…");
        assert_eq!(display_width(&truncate_to_width("Analyze hermes-agent", 10)), 10);
    }

    #[test]
    fn truncate_to_width_zero_or_one_returns_empty() {
        assert_eq!(truncate_to_width("anything", 0), "");
        assert_eq!(truncate_to_width("anything", 1), "");
    }

    #[test]
    fn truncate_to_width_multibyte_no_panic() {
        // Byte index 49 used to land inside a multi-byte sequence and panic.
        // Should be safe now: truncate by display cells, not bytes.
        let s = "查询".repeat(40); // each '查'/'询' is 2 display cells, 3 bytes
        let out = truncate_to_width(&s, 10);
        assert!(display_width(&out) <= 10);
        assert!(out.ends_with('…'));
    }

    #[test]
    fn truncate_to_width_emoji_safe() {
        // Emoji are typically 2 cells; truncation must not split surrogate halves
        // and must not exceed the budget.
        let out = truncate_to_width("🚧🚧🚧🚧🚧", 5);
        assert!(display_width(&out) <= 5);
        assert!(out.ends_with('…'));
    }
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test --quiet truncate_to_width`
Expected: compilation error — `cannot find function 'truncate_to_width'`.

- [ ] **Step 3: Implement `truncate_to_width`**

Insert this function in `src/main.rs` immediately after `display_width` (after the closing `}` of `display_width` around line 118):

```rust
/// Truncate `s` so its display width does not exceed `max_width`. When
/// truncation occurs, the last cell is replaced with `…`. Returns `""` if
/// `max_width < 2` (no room for even one char plus the ellipsis suffix that
/// would otherwise be needed; a bare 1-cell char is also dropped for
/// consistency since callers only invoke this when budget is meaningful).
pub(crate) fn truncate_to_width(s: &str, max_width: usize) -> String {
    if max_width < 2 {
        return String::new();
    }
    if display_width(s) <= max_width {
        return s.to_string();
    }
    // Reserve 1 cell for '…'.
    let budget = max_width - 1;
    let mut out = String::new();
    let mut width = 0usize;
    let mut prev_char_width = 0usize;
    for c in s.chars() {
        if c == '\u{FE0F}' {
            // Match display_width semantics: VS16 promotes the previous
            // text-presentation char to 2 cells.
            if prev_char_width < 2 {
                let extra = 2 - prev_char_width;
                if width + extra > budget {
                    break;
                }
                width += extra;
            }
            out.push(c);
            prev_char_width = 0;
            continue;
        }
        let cw = UnicodeWidthChar::width(c).unwrap_or(0);
        if width + cw > budget {
            break;
        }
        out.push(c);
        width += cw;
        prev_char_width = cw;
    }
    out.push('\u{2026}');
    out
}
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test --quiet truncate_to_width`
Expected: all 5 tests pass, no compile warnings about `truncate_to_width`.

- [ ] **Step 5: Run the full test + lint suite**

Run: `cargo test --quiet && cargo clippy --quiet -- -D warnings`
Expected: all tests pass, no clippy warnings.

- [ ] **Step 6: Commit**

```bash
git add src/main.rs
git commit -m "feat(display): add display-width-aware truncate helper"
```

---

### Task 2: Apply terminal-width-aware task column in `print_table`

**Files:**
- Modify: `src/main.rs::print_table` (current body lines ~533-629)

- [ ] **Step 1: Read the current `print_table` for orientation**

Open `src/main.rs` and locate `fn print_table` (around line 533). The relevant pieces:
- Column widths computed at the top: `max_name`, `max_cmd`, `max_age`.
- Header `println!` (line 555) and divider `println!` (line 559).
- The per-row task block (lines 610-625) with the existing 50-byte truncation.
- The final row `println!` (line 627).

- [ ] **Step 2: Determine task column budget at the top of `print_table`**

Insert this block in `print_table` immediately after `max_age` is computed (after line 553, before the header `println!`):

```rust
    // Task column budget: shrink (or drop) the TASK column so the whole row
    // fits the terminal. On non-TTY (e.g., piped output), keep the original
    // 50-cell cap and never drop the column.
    let term_width = crossterm::terminal::size().ok().map(|(w, _)| w as usize);
    let fixed_width = max_name + max_cmd + max_age + 6; // three "  " separators
    let task_cap: Option<usize> = match term_width {
        Some(tw) => Some(tw.saturating_sub(fixed_width).min(50)),
        None => Some(50),
    };
    let show_task_column = task_cap.map(|c| c >= 4).unwrap_or(true);
    let task_cap = task_cap.unwrap_or(50);
```

- [ ] **Step 3: Update the header to omit `TASK` when the column is dropped**

Replace the existing header `println!` (currently):

```rust
    println!(
        "{DIM}{:<max_name$}  {:<max_cmd$}  {:<max_age$}  TASK{RESET}",
        "SESSION", "STATUS", "AGE"
    );
```

with:

```rust
    if show_task_column {
        println!(
            "{DIM}{:<max_name$}  {:<max_cmd$}  {:<max_age$}  TASK{RESET}",
            "SESSION", "STATUS", "AGE"
        );
    } else {
        println!(
            "{DIM}{:<max_name$}  {:<max_cmd$}  {:<max_age$}{RESET}",
            "SESSION", "STATUS", "AGE"
        );
    }
```

- [ ] **Step 4: Update the divider length to match the visible columns**

Replace the existing divider `println!`:

```rust
    println!(
        "{DIM}{}{RESET}",
        "\u{2501}".repeat(max_name + max_cmd + max_age + 10)
    );
```

with:

```rust
    let divider_len = if show_task_column {
        // 6 separator cells + "TASK" header (4) = 10 trailing cells
        max_name + max_cmd + max_age + 10
    } else {
        max_name + max_cmd + max_age + 4 // 2 separators between three columns
    };
    println!("{DIM}{}{RESET}", "\u{2501}".repeat(divider_len));
```

- [ ] **Step 5: Replace the per-row task block with width-aware truncation**

Replace the existing task block (lines ~610-625):

```rust
        let task_display = if s.task.is_empty() {
            String::new()
        } else {
            let task = if s.task.len() > 50 {
                format!("{}\u{2026}", &s.task[..49])
            } else {
                s.task.clone()
            };
            if matches!(tier, AgeTier::Old | AgeTier::Exited) {
                paint(&task, &[BRIGHT_BLACK])
            } else if s.agent_state == Some(AgentState::Waiting) || matches!(tier, AgeTier::Stale) {
                paint(&task, &[DIM])
            } else {
                task
            }
        };
```

with:

```rust
        let task_display = if !show_task_column || s.task.is_empty() {
            String::new()
        } else {
            let task = truncate_to_width(&s.task, task_cap);
            if matches!(tier, AgeTier::Old | AgeTier::Exited) {
                paint(&task, &[BRIGHT_BLACK])
            } else if s.agent_state == Some(AgentState::Waiting) || matches!(tier, AgeTier::Stale) {
                paint(&task, &[DIM])
            } else {
                task
            }
        };
```

- [ ] **Step 6: Update the per-row `println!` to drop the trailing task cell when the column is gone**

Replace the existing row print (line ~627):

```rust
        println!("{name_display}{name_pad}  {cmd_display}{cmd_pad}  {age_display}  {task_display}");
```

with:

```rust
        if show_task_column {
            println!("{name_display}{name_pad}  {cmd_display}{cmd_pad}  {age_display}  {task_display}");
        } else {
            println!("{name_display}{name_pad}  {cmd_display}{cmd_pad}  {age_display}");
        }
```

- [ ] **Step 7: Build and lint**

Run: `cargo build --quiet && cargo clippy --quiet -- -D warnings && cargo test --quiet`
Expected: clean build, no clippy warnings, all tests still pass.

- [ ] **Step 8: Manual narrow-terminal verification**

Build and run at a few widths. From an interactive terminal:

```bash
cargo build --release
# Wide check (should look identical to before)
stty cols 200 && ./target/release/zellijls
# Medium narrow
stty cols 80 && ./target/release/zellijls
# Narrow
stty cols 60 && ./target/release/zellijls
# Very narrow (TASK column should disappear)
stty cols 40 && ./target/release/zellijls
# Restore (use a sensible default for your terminal)
stty cols 200
```

Expected at each width:
- 200: identical to the pre-change render.
- 80 / 60: long tasks end with `…`; row never wraps.
- 40: header shows only `SESSION  STATUS  AGE`; rows have no trailing task; divider matches.

Also verify piped output is unchanged:

```bash
./target/release/zellijls | cat
```

Expected: full 50-cell task cap, no column dropped.

- [ ] **Step 9: Commit**

```bash
git add src/main.rs
git commit -m "fix(ui): fit table to terminal width by shrinking TASK column"
```

---

## Self-review summary

- **Spec coverage:** Task 1 covers the helper (multi-byte fix, display-width budget). Task 2 covers the budget formula, the `< 4` drop threshold, the non-TTY fallback, the divider adjustment, and the header omission.
- **Placeholders:** None — every step has the actual code or command.
- **Type consistency:** `truncate_to_width(&str, usize) -> String` is defined once in Task 1 and called once in Task 2 Step 5 with matching types. `task_cap` is a plain `usize`; `show_task_column` is `bool`.
