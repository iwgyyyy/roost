# roost

**English** · [中文](README.zh-CN.md)

Watch all your AI coding agents from one terminal panel.

`roost` is a passive, read-only observer: run it in any terminal and it shows every **Claude Code** and **Codex** session running on your machine — grouped by whether they need your attention — in real time.

```
┌─ roost ──────────────────────────────────────────────────────────── 2 agents · live ● ─┐

  NEEDS INPUT  1
 ▌⬡ Codex  payments-api   ? Which migration strategy? (1/2)              Codex      8s

  WORKING  1
  ✻ Claude  roost          ⠸ working    Editing src/session.rs   Zed              1m

  ↑/↓ j/k select · enter peek · o jump · q quit
```

roost does **not** start, proxy, or control agents. It works entirely through the hook callbacks each agent fires — installing them is a one-time `roost setup`.

---

## Features

- **One panel for every agent** — all running Claude Code and Codex sessions on the machine, live.
- **Attention-first grouping** — sessions sorted into `NEEDS INPUT` → `WORKING` → `IDLE`, so the ones waiting on you float to the top.
- **Rich per-session state** — Approval, Question, Working, Done, Idle — each with its own glyph and colour.
- **Clarify question cards** — when an agent asks an interactive question (Claude `AskUserQuestion`, Codex `request_user_input`), roost shows the actual question text and a `(1/N)` indicator for multi-question cards, instead of a generic "needs permission".
- **Live activity text** — what each agent is doing right now (`Editing src/foo.rs`, `Running tests`, `thinking…`).
- **Cumulative busy timer** — total active time per session; freezes when the turn ends and resumes on the next prompt.
- **Peek panel** — press `enter` for a detail view: path, status, current action, and a recent-event timeline with frozen per-step durations.
- **Jump to the agent** — press `o` to focus the agent's terminal window or editor (best-effort, host-dependent).
- **Passive & safe** — read-only, hook-driven, never blocks the agent, and `setup` merges into existing config without clobbering your other hooks.
- **Responsive layout** — adapts columns to terminal width, CJK-aware.
- **Single static binary** — no async runtime; the background daemon keeps state even when the panel is closed.

---

## Install

**curl** — macOS (arm64/x86_64) & Linux (x86_64):

```sh
curl -fsSL https://github.com/iwgyyyy/roost/releases/latest/download/roost-installer.sh | sh
```

**From source:**

```sh
cargo install --git https://github.com/iwgyyyy/roost
```

To **update** an install done via the script, run `roost update` (or just re-run
the curl command). Source installs update with `cargo install … --force`.

---

## Quick start

```sh
# 1. Install hooks into Claude Code (and Codex if ~/.codex exists)
roost setup

# 2. Start the TUI panel (auto-starts the background daemon)
roost

# 3. Start a Claude Code or Codex session in another terminal — it appears immediately
```

---

## Commands

| Command | Description |
|---|---|
| `roost` | Open the TUI panel. Auto-starts the daemon if it is not running. |
| `roost setup` | Install hooks into `~/.claude/settings.json` (and `~/.codex/` if found). Idempotent. |
| `roost setup --uninstall` | Remove roost hooks. Preserves all other existing hooks/config. |
| `roost list` | Print current sessions as a text table (debug / scripting). |
| `roost update` | Update to the latest release in place (script installs only). |
| `roost daemon` | Run the background daemon in the foreground. Normally started automatically by `roost`. |
| `roost hook <family> <event>` | Called by agent hooks — sends one event to the daemon then exits immediately. |

### Keys

`↑`/`↓` or `j`/`k` select · `enter` peek · `o` jump to agent · `q` / `esc` quit

---

## How it works

```
agent fires hook
  → roost hook <family> <event>   [reads stdin JSON + env, connects socket, sends one line, exits]
      ↓
  roost daemon                    [persistent, Unix socket, in-memory session table, PID liveness]
      ↓
  roost / roost list              [connect daemon, read session views, render]
```

- The daemon listens on a Unix socket at `$XDG_RUNTIME_DIR/roost/roost.sock` (fallback: `$TMPDIR/roost-<uid>/roost.sock`).
- Protocol is NDJSON (newline-delimited JSON) over that socket.
- `roost hook` has a short write timeout and silently exits if the daemon is unreachable — it **never blocks the agent**.
- No async runtime: std threads + channels + `Arc<Mutex<_>>`.
- The daemon outlives the panel — quitting `roost` (the TUI) does not lose session state.

---

## Agent support

### Claude Code

Hooks registered in `~/.claude/settings.json`:

| Hook event | What roost uses it for |
|---|---|
| `SessionStart` | Session appears in the list (Idle) |
| `UserPromptSubmit` | State → Working; captures the first prompt |
| `PreToolUse` | Activity text (`Editing …` / `Running …`); detects the `AskUserQuestion` clarify card → Question |
| `PostToolUse` | Activity resets to `thinking…`; counts edits |
| `Notification` | Approval vs Question vs idle, from the notification text |
| `Stop` | State → Done |
| `SessionEnd` | Session removed immediately |

### Codex

Hooks registered in `~/.codex/hooks.json`; feature flag set in `~/.codex/config.toml` (`[features] hooks = true`).

| Hook event | What roost uses it for |
|---|---|
| `SessionStart` | Session appears in the list (Idle) |
| `UserPromptSubmit` | State → Working |
| `PreToolUse` | Detects the `request_user_input` clarify card → Question (matcher-scoped to that tool only, so ordinary tool calls fire no extra hook) |
| `PostToolUse` | Activity text (e.g. `Bash done`) |
| `PermissionRequest` | State → Approval |
| `Stop` | State → Done |

**Codex notes:**

- No `SessionEnd` hook — session removal relies on PID liveness probing (every ~2 s) plus an idle timeout.
- Codex hooks are behind an experimental feature flag and may change.
- A clarify card pauses the turn without firing a `Stop`, so the Question state persists until the user answers.

---

## Session states

Grouped in priority order: **NEEDS INPUT** (Approval, then Question) → **WORKING** → **IDLE**.

| State | Glyph | Colour | Trigger |
|---|---|---|---|
| Approval | `▲` | amber, breathing pulse | Claude `Notification` with permission keywords; Codex `PermissionRequest` |
| Question | `?` | cyan | Clarify card (`AskUserQuestion` / `request_user_input`); Claude `Notification` without permission keywords |
| Working | braille spinner | green | `UserPromptSubmit`, `PreToolUse`, `PostToolUse` |
| Done | `✓` | muted green | `Stop` |
| Idle | `○` | slate | `SessionStart` before the first prompt, or after an idle notification |

A session is removed (not shown as "disconnected") when `SessionEnd` fires or when the agent process dies and liveness probing detects it.

---

## Clarify question cards

Both agents model an interactive question as a tool call (`AskUserQuestion` for Claude, `request_user_input` for Codex). Its `PreToolUse` fires the moment the card opens, so roost shows it as **NEEDS INPUT / Question** with the first question's text. Cards carrying more than one question append a `(1/N)` count.

Because the card-internal navigation between questions fires no hook, roost shows the card as a single "needs input" signal — to answer it, press `o` to jump to the agent's own UI.

---

## Terminal jump

Press **`o`** to jump back to the agent's terminal window. Best-effort — precision depends on the host:

| Host | Precision | Mechanism |
|---|---|---|
| Codex.app | conversation | `codex://threads/<id>` deep link |
| Ghostty, iTerm2, Terminal.app | tab/pane | AppleScript |
| WezTerm | pane | `wezterm cli activate-pane` |
| Zellij | session | `zellij action` CLI |
| tmux | pane | `tmux switch-client` + `select-pane` |
| Warp | approximate | Warp SQLite + keyboard simulation (fragile) |
| VS Code, Cursor, Windsurf, Trae, Zed | window | `<cli> -r <workspace>` |
| JetBrains IDEs | window | `<cli> <project>` |

Fallback ladder: precise focus → activate app → open workspace in app → open directory → unsupported message. Launched helpers run fully detached so their output never corrupts the panel.

---

## Responsive layout

The TUI adapts to terminal width automatically; all widths use `unicode-width` so CJK characters (width 2) are counted correctly.

| Width | Layout |
|---|---|
| ≥ 100 | Full: selection bar, icon, family, name, glyph, state label, activity (elastic), time |
| 60–99 | Narrower name; family/label columns drop progressively |
| 40–59 | Minimal columns |
| < 40 | Glyph + truncated name, or a "widen to ≥ 40" hint |

---

## Session name resolution

The **name** column shows the git repository root basename. In a monorepo (working dir deeper than the repo root) it shows `repo/subdir` (truncated). If the directory is not a git repository, the last path segment is used.

Examples: `/home/user/bigmono/packages/api` → `bigmono/api`; `/home/user/myapp` (git root) → `myapp`; `/home/user/scratch` (no git) → `scratch`.

---

## Architecture notes

- Single static binary, no async runtime; sub-command dispatch keeps `roost hook` cold-start in microseconds.
- The daemon holds all state in memory; killing the TUI does not lose it.
- PID liveness: the daemon probes each session's PID every ~2 s via `kill(pid, 0)` (not a full process scan). Repeated failures, or an idle timeout with a failed probe, remove the session.
- Hook JSON parsing: all fields are optional (`#[serde(default)]`); unknown fields are ignored — forward-compatible with agent payload changes.
- `roost setup` merges into existing config and only ever touches its own entries; uninstall removes only roost's hooks and leaves everything else intact.

---

## Building

```sh
cargo build --release      # target/release/roost
cargo install --path .     # install into ~/.cargo/bin
cargo test                 # unit + integration tests
```
