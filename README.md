# roost

Watch all your AI coding agents from one terminal panel.

`roost` is a passive, read-only observer: run it in any terminal and it shows every Claude Code and Codex session running on your machine — grouped by whether they need your attention — in real time.

```
┌─ roost ──────────────────────────────────────────────────────────────────────┐
│ 2 agents · 1 needs input                                           ● 14:32   │
├──────────────────────────────────────────────────────────────────────────────┤
│ NEEDS INPUT ──────────────────────────────────────────────────────────────── │
│ ▌⬡ roost-api       ▲ approval  run: git push --force                   3s   │
│                                                                              │
│ WORKING ──────────────────────────────────────────────────────────────────── │
│  ✻ payments         ⠸ working   Editing src/checkout.ts                 1s   │
└──────────────────────────────────────────────────────────────────────────────┘
  ↑/↓ j/k select   enter peek   o jump   q quit
```

roost does **not** start, proxy, or control agents. It works entirely through hook callbacks that each agent fires — installing them is a one-time `roost setup`.

---

## Quick start

```sh
# 1. Install hooks into Claude Code (and Codex if ~/.codex exists)
roost setup

# 2. Start the TUI panel (auto-starts the background daemon)
roost

# 3. Start a Claude Code session in another terminal — it appears immediately
```

---

## Commands

| Command | Description |
|---|---|
| `roost` | Open the TUI panel. Auto-starts the daemon if it is not running. |
| `roost setup` | Install hooks into `~/.claude/settings.json` (and `~/.codex/` if found). Idempotent. |
| `roost setup --uninstall` | Remove roost hooks. Preserves all other existing hooks/config. |
| `roost list` | Print current sessions as a text table (debug / scripting). |
| `roost daemon` | Run the background daemon in the foreground. Normally started automatically by `roost`. |
| `roost hook <family> <event>` | Called by agent hooks — sends one event to the daemon then exits immediately. |

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

- The daemon runs a Unix socket at `$XDG_RUNTIME_DIR/roost/roost.sock` (fallback: `$TMPDIR/roost-<uid>/roost.sock`).
- Protocol is NDJSON (newline-delimited JSON) over that socket.
- `roost hook` has a 200 ms write timeout and silently exits if the daemon is unreachable — it never blocks the agent.
- No async runtime (std threads + channels + `Arc<Mutex<_>>`).

---

## Agent support matrix

### Claude Code

Hooks registered in `~/.claude/settings.json`:

| Hook event | What roost uses it for |
|---|---|
| `SessionStart` | Session appears in the list |
| `UserPromptSubmit` | State → Working; captures the first prompt text |
| `PreToolUse` | Activity text: `Editing src/foo.rs` / `Running git push` |
| `PostToolUse` | Activity resets to `思考中…`; increments edit count |
| `Notification` | Determines Approval vs Question from the notification text |
| `Stop` | State → Done |
| `SessionEnd` | Session removed immediately |

**Full fidelity**: all 4 states (Approval, Question, Working, Idle/Done), per-tool activity text, automatic Question/Approval distinction.

### Codex

Hooks registered in `~/.codex/hooks.json`; feature flag set in `~/.codex/config.toml` (`[features] hooks = true`).

Low-noise subset (per-tool hooks are not registered to avoid log spam):

| Hook event | What roost uses it for |
|---|---|
| `SessionStart` | Session appears in the list |
| `UserPromptSubmit` | State → Working |
| `approval-requested` | State → Approval |
| `Stop` / `agent-turn-complete` | State → Done |

**Codex limitations (by design):**
- No per-tool activity detail — activity shows `思考中…` while working (Codex does not expose per-tool hooks in the low-noise subset).
- No Question state — Codex has no question-specific hook event.
- No `SessionEnd` hook — session removal relies on PID liveness probing (every ~2 s) and a 5-minute idle timeout.
- Codex hooks are behind an experimental feature flag and may change.

---

## Session states

Sessions are grouped in priority order: **NEEDS INPUT** (Approval, then Question) → **WORKING** → **IDLE**.

| State | Glyph | Colour | Trigger |
|---|---|---|---|
| Approval | `▲` | amber `#e3b341`, breathing pulse | Claude `Notification` with permission keywords; Codex `approval-requested` |
| Question | `?` | cyan `#58c4d6` | Claude `Notification` without permission keywords |
| Working | braille spinner | green `#6fd283` | `UserPromptSubmit`, `PreToolUse`, `PostToolUse` |
| Done | `✓` | muted green `#7f9e84` | `Stop` / `agent-turn-complete` |
| Idle | `○` | slate `#768390` | `SessionStart` (before first prompt) |

Sessions are removed (not shown as "disconnected") when `SessionEnd` fires, or when the agent process dies and liveness probing detects it.

---

## Session name resolution

The **name** column shows the git repository root basename. When the working directory is deeper than the repository root (monorepo), it shows `repo/subdir` (truncated to 16 characters). If the directory is not a git repository, the last path segment is used.

Examples: `/home/user/bigmono/packages/api` → `bigmono/api`; `/home/user/myapp` (git root) → `myapp`; `/home/user/scratch` (no git) → `scratch`.

---

## Responsive layout

The TUI adapts to terminal width automatically:

| Width | Layout |
|---|---|
| ≥ 100 | Full: selection bar, icon, name(16), state glyph, state label, activity (elastic), relative time |
| 80–99 | Drop state label text; keep glyph + colour |
| 60–79 | Name narrows to 12; activity elastic |
| 40–59 | Drop time + icon; minimal layout |
| < 40 | Minimal: glyph + truncated name only, or a "widen to ≥ 40" hint |

All widths use `unicode-width` so CJK characters (width 2) are counted correctly.

---

## Terminal jump (Phase 4, implemented)

Press **`o`** in the TUI to jump back to the agent's terminal window. This is best-effort — precision depends on the host:

| Host | Precision | Mechanism |
|---|---|---|
| **Codex.app** | conversation level | `codex://threads/<id>` deep link |
| Ghostty, iTerm2, Terminal.app | tab/pane level | AppleScript |
| WezTerm / Kaku | pane level | `wezterm cli activate-pane` |
| Zellij | session level | `zellij action` CLI |
| tmux | pane level | `tmux switch-client` + `select-pane` |
| Warp | approximate | Warp SQLite + keyboard simulation (fragile) |
| VS Code, Cursor, Windsurf, Trae, Zed | window level (not inner terminal tab) | `<cli> -r <workspace>` |
| JetBrains IDEs | window level | `<cli> <project>` |

Fallback ladder: precise focus → activate app → open workspace in app → open directory in Finder → unsupported message.

**Claude Code has no desktop app:** it always jumps to its host terminal/editor (rows 2–8 above).

---

## Needs real-machine verification

The following cannot be validated in automated tests:

1. **Claude Code hooks fire correctly** — requires a real Claude Code session; `roost setup` writes the config, but hook invocation depends on Claude Code's hook execution engine.
2. **Codex hooks fire correctly** — requires Codex with the experimental `hooks` feature; the event names and payload format may change.
3. **Terminal jump — AppleScript paths** (Ghostty, iTerm2, Terminal.app) — AppleScript execution requires Accessibility permissions on macOS.
4. **Terminal jump — Warp** — reads Warp's SQLite database; path and schema are undocumented and may change.
5. **TUI animations** (braille spinner, Approval breathing pulse, header heartbeat) — require a live terminal and are not testable headlessly.
6. **PID liveness / auto-removal** — works in unit tests but real-agent PID discovery (`ps` walk up the parent chain) should be verified with actual Claude Code and Codex processes.
7. **Codex.app deep link** (`codex://threads/<id>`) — requires Codex.app installed and registered as the `codex://` URL handler.

---

## Architecture notes

- Single static binary, no async runtime.
- `roost daemon` holds all state; killing the TUI does not lose session state.
- `roost hook` cold-starts in microseconds (same binary, sub-command dispatch, no heavy initialisation).
- PID liveness: daemon probes each session's PID every 2 s via `kill(pid, 0)` (not a full process scan). Three consecutive failures remove the session; 5-minute idle timeout with at least one failed probe is a secondary backstop.
- Hook JSON parsing: all fields are optional with `#[serde(default)]`; unknown fields are silently ignored — forward-compatible with Claude/Codex payload changes.
