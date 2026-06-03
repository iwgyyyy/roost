# roost

**English** · [中文](README.zh-CN.md)

Watch all your AI coding agents from one terminal panel.

`roost` is a passive, read-only observer: run it in any terminal and it shows every AI coding agent session running on your machine — **Claude Code**, **Codex**, **DeepSeek (CodeWhale)**, and **Cursor** — grouped by whether they need your attention, in real time. Add a remote dev box with `roost add user@host` and its agents show up in the same panel, over SSH.

```
  roost                                            4 agents · live ●

  NEEDS INPUT  1
  ┌ ✻ Claude ───────────────────────── ? Question ·  8s ┐
  ┊ ▸ choose: Postgres or SQLite?                       ┊
  ┊ ~/work/payments-api              @local · via Cursor ┊
  └─────────────────────────────────────────────────────┘

  WORKING  2
  ┏ ✻ Claude ──────────────────────── ⠹ Working ·  3s ━┓  ← selected: heavy accent border
  ┃ ▸ Editing src/components/Editor.tsx                 ┃
  ┃ ~/dev/myapp                    @local · via Ghostty  ┃
  ┗━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━┛
  ┌ ⬡ Codex ───────────────────────── ⠹ Working · 12s ┐
  ┊ ▸ Editing app/page.tsx                             ┊
  ┊ ~/work/api               @desk-mini · via VS Code  ┊  ← remote: device in cold blue
  └────────────────────────────────────────────────────┘

  OFFLINE  1
  ┌┄ ≈ CodeWhale ┄┄┄┄┄┄┄┄┄┄┄┄┄┄┄┄┄┄┄ ⊘ offline ·  2m ┄┐  ← disconnected: dashed + dimmed
  ┊ ⊘ Training run · epoch 4/10                         ┊
  ┊ ~/ml/pipeline                 @gpu-rig · via Cursor  ┊
  └┄┄┄┄┄┄┄┄┄┄┄┄┄┄┄┄┄┄┄┄┄┄┄┄┄┄┄┄┄┄┄┄┄┄┄┄┄┄┄┄┄┄┄┄┄┄┄┄┄┄┄┄┘

  ~/dev/myapp
  ↑/↓ select   enter peek   o jump   s stats   c settings   q quit
```

roost does **not** start, proxy, or control agents. It works entirely through the hook callbacks each agent fires — installing them is a one-time `roost setup`.

---

## Features

- **One panel for every agent** — all running Claude Code, Codex, DeepSeek (CodeWhale), and Cursor sessions on the machine, live.
- **Attention-first grouping** — sessions sorted into `NEEDS INPUT` → `WORKING` → `IDLE` → `OFFLINE`, so the ones waiting on you float to the top; dropped remote sessions sink to the bottom.
- **Rich per-session state** — Approval, Question, Working, Done, Idle — each with its own glyph and colour.
- **Clarify question cards** — when an agent asks an interactive question (Claude `AskUserQuestion`, Codex `request_user_input`), roost shows the actual question text and a `(1/N)` indicator for multi-question cards, instead of a generic "needs permission".
- **Live activity text** — what each agent is doing right now (`Editing src/foo.rs`, `Running tests`, `thinking…`).
- **Cumulative busy timer** — total active time per session; freezes when the turn ends and resumes on the next prompt.
- **Peek panel** — press `enter` for a detail view: path, status, current action, and a recent-event timeline with frozen per-step durations.
- **Jump to the agent** — press `o` to focus the agent's terminal window or editor (best-effort, host-dependent).
- **History & stats** — press `s` for daily work time, a per-project breakdown, and how often agents waited on you. Stored locally in `~/.roost/history.db` (bundled SQLite — nothing to install).
- **Desktop notifications (macOS & Linux)** — the daemon fires a notification + sound when an agent needs you (clarify / approve) or finishes (done), so you get pulled back even when the panel isn't focused — or isn't open. Per-stage banner/sound toggles live in `~/.roost/settings.json` or an in-app settings page (`c`).
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

# 3. Start any supported agent (Claude Code, Codex, CodeWhale, Cursor) — it appears immediately
```

On first run, if hooks aren't installed yet, `roost` offers to run `roost setup`
for you — so you can also just run `roost` and accept the prompt.

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
| `roost add <user@host>` | Add a remote dev box: installs roost + hooks over SSH and forwards its events back to this machine. See [Remote fleet](#remote-fleet-ssh). |
| `roost remove <user@host>` | Remove a remote: tear down the tunnel, unregister it, uninstall its hooks. `--purge` also removes the remote binary. |
| `roost remotes` | List added remotes and their tunnel status. |

### Keys

`↑`/`↓` or `j`/`k` select · `enter` peek · `o` jump to agent · `s` stats · `c` settings · `q` / `esc` quit

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

## Supported agents

`roost setup` installs hooks for each agent it finds on your machine; agents it doesn't find are skipped.

| Agent | What it is | Config |
|---|---|---|
| **Claude Code** | Anthropic's `claude` CLI / IDE sessions | `~/.claude/settings.json` |
| **Codex** | OpenAI's `codex` CLI | `~/.codex/` |
| **DeepSeek (CodeWhale)** | the `codewhale` / `deepseek` terminal agent | `~/.codewhale/` (or `~/.deepseek/`) |
| **Cursor** | Cursor's built-in Agent | `~/.cursor/hooks.json` |

Each shows up with its own glyph and colour. New agents are added over time — see the issues for what's planned.

---

## Session states

Grouped in priority order: **NEEDS INPUT** (Approval, then Question) → **WORKING** → **IDLE** → **OFFLINE** (bottom).

| State | Glyph | Colour | Trigger |
|---|---|---|---|
| Approval | `▲` | amber, breathing pulse | Claude `Notification` with permission keywords; Codex `PermissionRequest` |
| Question | `?` | cyan | Clarify card (`AskUserQuestion` / `request_user_input`); Claude `Notification` without permission keywords |
| Working | braille spinner | green | `UserPromptSubmit`, `PreToolUse`, `PostToolUse` |
| Done | `✓` | muted green | `Stop` |
| Idle | `○` | slate | `SessionStart` before the first prompt, or after an idle notification |
| Offline | `⊘` | dim | Remote session whose SSH tunnel has dropped; card shown with dashed border and dimmed colours |

A local session is removed (not shown as "disconnected") when `SessionEnd` fires or when the agent process dies and liveness probing detects it. Remote sessions whose tunnel drops are frozen in the OFFLINE group instead of vanishing, and refresh automatically on reconnect.

---

## Notifications (macOS & Linux)

When a session crosses **into** a state that wants your attention, the background daemon fires a desktop notification with a sound — so you get pulled back even when the panel isn't focused, or isn't open. Notifications are fired by the daemon (not the TUI), once per transition into the stage.

| Stage | Trigger state | Default sound |
|---|---|---|
| `clarify` | Question — a clarify card / waiting for your input | Submarine |
| `approve` | Approval — permission for a risky action | Bottle |
| `done` | Done — the agent finished a turn | Ping |

The banner **title** is `roost`; the **body** shows `<agent> · <project> — <what it's waiting on>` (or a completion note for `done`). Each agent family is labelled (Claude Code / Codex / DeepSeek / Cursor) so you can tell who needs you.

**Configurable per stage** — every stage has independent `banner` (popup) and `sound` switches, stored in `~/.roost/settings.json`:

```json
{
  "accent": "#d97757",
  "notify": {
    "clarify":        { "banner": true,  "sound": true  },
    "approve":        { "banner": true,  "sound": true  },
    "done":           { "banner": true,  "sound": true  },
    "remote_offline": false
  }
}
```

- **`accent`** — theme colour (hex) used for the selected-card border and the `roost` brand text in the header. Defaults to `"#d97757"`.
- **`notify.remote_offline`** — fire a notification when a remote SSH tunnel drops (the session moves to the OFFLINE group). Off by default; opt in here or on the settings page (`c`).

Edit the file directly, or press **`c`** in the panel to open a scrollable settings page and toggle each switch — changes are saved immediately and take effect on the next notification. A missing file or field falls back to all-on (fail-open); `remote_offline` is the exception and defaults to `false`.

**Platforms.** macOS uses `osascript` for banners and `afplay` for sounds — both built in, nothing to install. Linux uses `notify-send` (from `libnotify-bin` / `libnotify`); the per-stage sound rides along as a freedesktop `sound-name` hint (`message-new-instant` / `dialog-warning` / `complete`), falling back to `canberra-gtk-play` / `paplay` when only the sound is enabled. If a tool is missing — or there's no desktop session (headless / SSH) — notifications degrade to a silent no-op, never an error. `roost setup` points you at the package to install when `notify-send` is absent. Other platforms are always a no-op.

---

## Remote fleet (SSH)

Run agents on remote dev boxes? `roost add user@devbox1` brings that machine's
agents into your local panel — and fires their notifications on *your* machine.

This machine is the **hub**: it runs the only daemon, the TUI, and the
notifications. Each remote is a passive forwarder that runs **no daemon**. `roost add`
installs roost and its hooks on the remote over SSH (auto-installing the binary if
it's missing), then the local daemon supervises a persistent SSH reverse-forward
tunnel (`ssh -N -R`) that carries the remote's hook events back to the hub. Remotes
are recorded in `~/.roost/remotes.json` on the hub and re-established on daemon start.

| Command | What it does |
|---|---|
| `roost add user@host [--origin name]` | Install + register a remote, bring up its tunnel |
| `roost remove user@host [--purge]` | Tear down the tunnel, unregister, uninstall remote hooks |
| `roost remotes` | List remotes and tunnel status (up / reconnecting / down) |

Remote sessions show their origin host in the panel. If a tunnel drops, those
sessions **freeze** (greyed, dashed border) instead of vanishing, and the daemon
reconnects automatically; they refresh on the agent's next event.

**Preconditions.** Before `roost add`, make sure:

- **Passwordless SSH** — `ssh <host>` connects non-interactively with a key. roost adds no auth of its own and runs every command in `BatchMode`, so a password prompt makes it fail. Run `ssh-copy-id <host>` first if needed.
- **Standard port (22)** — `roost add user@host` takes no `-p`. For a non-standard port, add a `~/.ssh/config` alias (with `Port`, `User`, `IdentityFile`) and run `roost add <alias>`.
- **Network + `curl`** on the remote, for the one-time binary install.
- **Supported OS** — the remote must be Linux (x86_64 / arm64) or macOS.

Only **newly started** agent sessions report after `roost add` — already-running
agents load their hooks at launch, so they won't show up until their next session.

A **remote-offline** notification is available but **off by default** — toggle it on
the settings page (`c`) or in `~/.roost/settings.json`.

**Jump.** `o` can't focus a remote terminal from here; for a remote session it tells
you which host to SSH into. **Security.** Anyone who can write the forwarded socket
on a remote can inject events into your local daemon — fine for personal dev boxes,
worth noting for shared machines.

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
| cmux | surface | `surface.focus` JSON-RPC over its Unix socket |
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
