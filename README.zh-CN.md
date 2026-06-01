# roost

[English](README.md) · **中文**

在一个终端面板里盯住你所有的 AI 编码 agent。

`roost` 是一个被动、只读的观察器：在任意终端里运行它，就能实时看到机器上所有正在运行的 **Claude Code** 和 **Codex** 会话，并按「是否需要你介入」分组排列。

```
┌─ roost ──────────────────────────────────────────────────────────── 2 agents · live ● ─┐

  NEEDS INPUT  1
 ▌⬡ Codex  payments-api   ? 用哪种迁移策略？ (1/2)                       Codex      8s

  WORKING  1
  ✻ Claude  roost          ⠸ working    Editing src/session.rs   Zed              1m

  ↑/↓ j/k select · enter peek · o jump · q quit
```

roost **不会**启动、代理或控制任何 agent。它完全依靠各 agent 自己触发的 hook 回调工作——只需一次 `roost setup` 安装这些 hook。

---

## 功能

- **一个面板看全部 agent**——机器上所有运行中的 Claude Code 和 Codex 会话，实时更新。
- **以「需要关注」优先分组**——会话按 `NEEDS INPUT` → `WORKING` → `IDLE` 排序，等你处理的浮在最上面。
- **细粒度会话状态**——Approval（待授权）、Question（待回答）、Working（工作中）、Done（已完成）、Idle（空闲），各有独立字形与颜色。
- **clarify 询问卡片**——当 agent 弹出交互式问题（Claude 的 `AskUserQuestion`、Codex 的 `request_user_input`）时，roost 直接显示**实际的问题文本**，多问题卡片还带 `(1/N)` 计数，而不是笼统的「needs permission」。
- **实时活动文本**——每个 agent 当前在干什么（`Editing src/foo.rs`、`Running tests`、`thinking…`）。
- **累计忙碌计时**——每个会话的累计活跃时长；turn 结束时冻结，下次开始时接着计。
- **详情面板（peek）**——按 `enter` 查看详情：路径、状态、当前动作，以及一条带「冻结耗时」的最近事件时间线。
- **一键跳转**——按 `o` 聚焦到该 agent 所在的终端窗口或编辑器（尽力而为，取决于宿主）。
- **被动且安全**——只读、hook 驱动、绝不阻塞 agent；`setup` 是合并写入现有配置，不会覆盖你其它的 hook。
- **自适应布局**——按终端宽度调整列，正确处理 CJK 宽字符。
- **单一静态二进制**——无 async 运行时；后台 daemon 在面板关闭后仍保留状态。

---

## 安装

```sh
# curl（macOS arm64/x86_64、Linux x86_64）
curl -fsSL https://github.com/iwgyyyy/roost/releases/latest/download/roost-installer.sh | sh

# 从源码
cargo install --git https://github.com/iwgyyyy/roost
```

---

## 快速开始

```sh
# 1. 安装 hook 到 Claude Code（若存在 ~/.codex 则一并装到 Codex）
roost setup

# 2. 启动 TUI 面板（自动拉起后台 daemon）
roost

# 3. 在另一个终端启动 Claude Code 或 Codex 会话——它会立刻出现
```

---

## 命令

| 命令 | 说明 |
|---|---|
| `roost` | 打开 TUI 面板。daemon 未运行时自动拉起。 |
| `roost setup` | 安装 hook 到 `~/.claude/settings.json`（找到 `~/.codex/` 则一并安装）。幂等。 |
| `roost setup --uninstall` | 移除 roost 的 hook。保留你其它所有 hook / 配置。 |
| `roost list` | 以文本表格打印当前会话（调试 / 脚本用）。 |
| `roost daemon` | 在前台运行后台 daemon。通常由 `roost` 自动启动。 |
| `roost hook <family> <event>` | 由 agent 的 hook 调用——向 daemon 发送一个事件后立即退出。 |

### 按键

`↑`/`↓` 或 `j`/`k` 选择 · `enter` 详情 · `o` 跳转到 agent · `q` / `esc` 退出

---

## 工作原理

```
agent 触发 hook
  → roost hook <family> <event>   [读 stdin JSON + 环境变量，连 socket，发一行，退出]
      ↓
  roost daemon                    [常驻，Unix socket，内存会话表，PID 存活探测]
      ↓
  roost / roost list              [连 daemon，读会话视图，渲染]
```

- daemon 在 Unix socket `$XDG_RUNTIME_DIR/roost/roost.sock` 上监听（回退：`$TMPDIR/roost-<uid>/roost.sock`）。
- 协议是 NDJSON（按行分隔的 JSON）。
- `roost hook` 有很短的写超时，daemon 不可达时静默退出——**绝不阻塞 agent**。
- 无 async 运行时：std 线程 + channel + `Arc<Mutex<_>>`。
- daemon 的生命周期长于面板——退出 `roost`（TUI）不会丢失会话状态。

---

## Agent 支持

### Claude Code

注册在 `~/.claude/settings.json` 的 hook：

| Hook 事件 | roost 用途 |
|---|---|
| `SessionStart` | 会话出现在列表中（Idle） |
| `UserPromptSubmit` | 状态 → Working；记录首条 prompt |
| `PreToolUse` | 活动文本（`Editing …` / `Running …`）；识别 `AskUserQuestion` clarify 卡片 → Question |
| `PostToolUse` | 活动重置为 `thinking…`；统计编辑次数 |
| `Notification` | 根据通知文本判定 Approval / Question / 空闲 |
| `Stop` | 状态 → Done |
| `SessionEnd` | 立即移除会话 |

### Codex

注册在 `~/.codex/hooks.json` 的 hook；在 `~/.codex/config.toml` 设置特性开关（`[features] hooks = true`）。

| Hook 事件 | roost 用途 |
|---|---|
| `SessionStart` | 会话出现在列表中（Idle） |
| `UserPromptSubmit` | 状态 → Working |
| `PreToolUse` | 识别 `request_user_input` clarify 卡片 → Question（matcher 只匹配该工具，普通工具调用不会多触发 hook） |
| `PostToolUse` | 活动文本（如 `Bash done`） |
| `PermissionRequest` | 状态 → Approval |
| `Stop` | 状态 → Done |

**Codex 说明：**

- 没有 `SessionEnd` hook——会话移除依赖 PID 存活探测（约每 2 秒）加空闲超时。
- Codex 的 hook 处于实验性特性开关之后，可能变化。
- clarify 卡片会让 turn 暂停但**不发 `Stop`**，所以 Question 状态会一直保持到用户作答。

---

## 会话状态

按优先级分组：**NEEDS INPUT**（先 Approval 后 Question）→ **WORKING** → **IDLE**。

| 状态 | 字形 | 颜色 | 触发 |
|---|---|---|---|
| Approval | `▲` | 琥珀色，呼吸脉动 | Claude `Notification` 含权限关键词；Codex `PermissionRequest` |
| Question | `?` | 青色 | clarify 卡片（`AskUserQuestion` / `request_user_input`）；Claude `Notification` 不含权限关键词 |
| Working | 盲文 spinner | 绿色 | `UserPromptSubmit`、`PreToolUse`、`PostToolUse` |
| Done | `✓` | 暗绿色 | `Stop` |
| Idle | `○` | 石板灰 | 首条 prompt 之前的 `SessionStart`，或收到空闲通知后 |

当 `SessionEnd` 触发、或 agent 进程退出被存活探测发现时，会话会被**移除**（而不是显示成「已断开」）。

---

## clarify 询问卡片

两家 agent 都把交互式提问建模成一次工具调用（Claude 用 `AskUserQuestion`，Codex 用 `request_user_input`）。卡片弹出的那一刻就会触发它的 `PreToolUse`，所以 roost 会把它显示成 **NEEDS INPUT / Question**，并带上第一个问题的文本；含多个问题的卡片追加 `(1/N)` 计数。

由于卡片内部在多个问题之间切换不会触发任何 hook，roost 把整张卡片当作一个「需要输入」的信号——要作答请按 `o` 跳到 agent 自己的界面。

---

## 终端跳转

按 **`o`** 跳回 agent 所在的终端窗口。尽力而为，精度取决于宿主：

| 宿主 | 精度 | 机制 |
|---|---|---|
| Codex.app | 会话级 | `codex://threads/<id>` deep link |
| Ghostty、iTerm2、Terminal.app | tab/pane 级 | AppleScript |
| WezTerm | pane 级 | `wezterm cli activate-pane` |
| Zellij | session 级 | `zellij action` CLI |
| tmux | pane 级 | `tmux switch-client` + `select-pane` |
| Warp | 近似 | Warp SQLite + 键盘模拟（脆弱） |
| VS Code、Cursor、Windsurf、Trae、Zed | 窗口级 | `<cli> -r <workspace>` |
| JetBrains 系 | 窗口级 | `<cli> <project>` |

回退阶梯：精确聚焦 → 激活应用 → 在应用里打开工作区 → 打开目录 → 提示不支持。被拉起的辅助命令完全脱离当前终端运行，其输出不会污染面板。

---

## 自适应布局

TUI 按终端宽度自动调整；所有宽度计算使用 `unicode-width`，正确处理 CJK（宽度 2）字符。

| 宽度 | 布局 |
|---|---|
| ≥ 100 | 完整：选择条、图标、家族、名称、字形、状态标签、活动文本（弹性）、时间 |
| 60–99 | 名称变窄；家族/标签列逐步省略 |
| 40–59 | 最简列 |
| < 40 | 字形 + 截断名称，或「请加宽到 ≥ 40」提示 |

---

## 会话名称解析

**name** 列显示 git 仓库根目录的 basename。在 monorepo 里（工作目录比仓库根更深）显示 `repo/subdir`（截断）。若目录不是 git 仓库，则用路径最后一段。

示例：`/home/user/bigmono/packages/api` → `bigmono/api`；`/home/user/myapp`（git 根）→ `myapp`；`/home/user/scratch`（非 git）→ `scratch`。

---

## 架构说明

- 单一静态二进制，无 async 运行时；子命令分发让 `roost hook` 的冷启动在微秒级。
- daemon 在内存里保存全部状态；杀掉 TUI 不会丢失。
- PID 存活探测：daemon 约每 2 秒用 `kill(pid, 0)` 探测各会话的 PID（不是全进程扫描）。连续失败、或空闲超时且探测失败，会移除会话。
- Hook JSON 解析：所有字段可选（`#[serde(default)]`）；未知字段忽略——对 agent 的 payload 变化向前兼容。
- `roost setup` 是合并写入现有配置，且只动它自己的条目；卸载只移除 roost 的 hook，其余原样保留。

---

## 构建

```sh
cargo build --release      # target/release/roost
cargo install --path .     # 安装到 ~/.cargo/bin
cargo test                 # 单元 + 集成测试
```
