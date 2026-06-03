# roost

[English](README.md) · **中文**

在一个终端面板里盯住你所有的 AI 编码 agent。

`roost` 是一个被动、只读的观察器：在任意终端里运行它，就能实时看到机器上所有正在运行的 AI 编码 agent 会话——**Claude Code**、**Codex**、**DeepSeek（CodeWhale）**、**Cursor**——并按「是否需要你介入」分组排列。用 `roost add user@host` 加一台远程 dev box,它的 agent 也会经 SSH 出现在同一块面板里。

```
  roost                                            4 agents · live ●

  NEEDS INPUT  1
  ┌ ✻ Claude ───────────────────────── ? Question ·  8s ┐
  ┊ ▸ choose: Postgres or SQLite?                       ┊
  ┊ ~/work/payments-api              @local · via Cursor ┊
  └─────────────────────────────────────────────────────┘

  WORKING  2
  ┏ ✻ Claude ──────────────────────── ⠹ Working ·  3s ━┓  ← 选中:heavy 主题色边框
  ┃ ▸ Editing src/components/Editor.tsx                 ┃
  ┃ ~/dev/myapp                    @local · via Ghostty  ┃
  ┗━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━┛
  ┌ ⬡ Codex ───────────────────────── ⠹ Working · 12s ┐
  ┊ ▸ Editing app/page.tsx                             ┊
  ┊ ~/work/api               @desk-mini · via VS Code  ┊  ← 远程:设备名染冷蓝
  └────────────────────────────────────────────────────┘

  OFFLINE  1
  ┌┄ ≈ CodeWhale ┄┄┄┄┄┄┄┄┄┄┄┄┄┄┄┄┄┄┄ ⊘ offline ·  2m ┄┐  ← 掉线:虚线 + 置灰
  ┊ ⊘ Training run · epoch 4/10                         ┊
  ┊ ~/ml/pipeline                 @gpu-rig · via Cursor  ┊
  └┄┄┄┄┄┄┄┄┄┄┄┄┄┄┄┄┄┄┄┄┄┄┄┄┄┄┄┄┄┄┄┄┄┄┄┄┄┄┄┄┄┄┄┄┄┄┄┄┄┄┄┄┘

  ~/dev/myapp
  ↑/↓ select   enter peek   o jump   s stats   c settings   q quit
```

roost **不会**启动、代理或控制任何 agent。它完全依靠各 agent 自己触发的 hook 回调工作——只需一次 `roost setup` 安装这些 hook。

---

## 功能

- **一个面板看全部 agent**——机器上所有运行中的 Claude Code、Codex、DeepSeek（CodeWhale）、Cursor 会话，实时更新。
- **以「需要关注」优先分组**——会话按 `NEEDS INPUT` → `WORKING` → `IDLE` → `OFFLINE` 排序，等你处理的浮在最上面；掉线的远端会话沉到底部。
- **细粒度会话状态**——Approval（待授权）、Question（待回答）、Working（工作中）、Done（已完成）、Idle（空闲），各有独立字形与颜色。
- **clarify 询问卡片**——当 agent 弹出交互式问题（Claude 的 `AskUserQuestion`、Codex 的 `request_user_input`）时，roost 直接显示**实际的问题文本**，多问题卡片还带 `(1/N)` 计数，而不是笼统的「needs permission」。
- **实时活动文本**——每个 agent 当前在干什么（`Editing src/foo.rs`、`Running tests`、`thinking…`）。
- **累计忙碌计时**——每个会话的累计活跃时长；turn 结束时冻结，下次开始时接着计。
- **详情面板（peek）**——按 `enter` 查看详情：路径、状态、当前动作，以及一条带「冻结耗时」的最近事件时间线。
- **一键跳转**——按 `o` 聚焦到该 agent 所在的终端窗口或编辑器（尽力而为，取决于宿主）。
- **历史与统计**——按 `s` 查看每日工作时长、按项目的耗时分布,以及 agent 等你介入的频次。数据存在本地 `~/.roost/history.db`（内置 SQLite,无需安装任何东西）。
- **桌面通知（macOS & Linux）**——agent 需要你（clarify / approve）或完成（done）时，后台 daemon 弹出系统通知 + 声音，即使面板没聚焦、甚至没打开也能把你叫回来。每个阶段的弹窗/声音开关存在 `~/.roost/settings.json`，也可在面板内设置页（`c`）调整。
- **被动且安全**——只读、hook 驱动、绝不阻塞 agent；`setup` 是合并写入现有配置，不会覆盖你其它的 hook。
- **自适应布局**——按终端宽度调整列，正确处理 CJK 宽字符。
- **单一静态二进制**——无 async 运行时；后台 daemon 在面板关闭后仍保留状态。

---

## 安装

**curl** —— macOS（arm64/x86_64）和 Linux（x86_64）：

```sh
curl -fsSL https://github.com/iwgyyyy/roost/releases/latest/download/roost-installer.sh | sh
```

**从源码：**

```sh
cargo install --git https://github.com/iwgyyyy/roost
```

脚本安装的，**更新**直接 `roost update`（或重跑上面的 curl 命令）。源码安装的用 `cargo install … --force` 更新。

---

## 快速开始

```sh
# 1. 安装 hook 到 Claude Code（若存在 ~/.codex 则一并装到 Codex）
roost setup

# 2. 启动 TUI 面板（自动拉起后台 daemon）
roost

# 3. 启动任意支持的 agent（Claude Code、Codex、CodeWhale、Cursor）——它会立刻出现
```

首次运行时，如果还没装 hook，`roost` 会询问是否帮你跑 `roost setup`——所以也可以直接 `roost` 然后确认提示。

---

## 命令

| 命令 | 说明 |
|---|---|
| `roost` | 打开 TUI 面板。daemon 未运行时自动拉起。 |
| `roost setup` | 安装 hook 到 `~/.claude/settings.json`（找到 `~/.codex/` 则一并安装）。幂等。 |
| `roost setup --uninstall` | 移除 roost 的 hook。保留你其它所有 hook / 配置。 |
| `roost list` | 以文本表格打印当前会话（调试 / 脚本用）。 |
| `roost update` | 就地更新到最新版本（仅适用于脚本安装）。 |
| `roost daemon` | 在前台运行后台 daemon。通常由 `roost` 自动启动。 |
| `roost hook <family> <event>` | 由 agent 的 hook 调用——向 daemon 发送一个事件后立即退出。 |
| `roost add <user@host>` | 接入一台远程 dev box:经 SSH 装好 roost + hook,并把它的事件转发回本机。见[远程 fleet](#远程-fleetssh)。 |
| `roost remove <user@host>` | 移除一台远程:停隧道、注销、卸载其 hook。`--purge` 连远端二进制一起删。 |
| `roost remotes` | 列出已接入的远程及其隧道状态。 |

### 按键

`↑`/`↓` 或 `j`/`k` 选择 · `enter` 详情 · `o` 跳转到 agent · `s` 统计 · `c` 设置 · `q` / `esc` 退出

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

## 支持的 agent

`roost setup` 会为机器上检测到的每个 agent 安装 hook；没装的会自动跳过。

| Agent | 是什么 | 配置位置 |
|---|---|---|
| **Claude Code** | Anthropic 的 `claude` CLI / 编辑器内会话 | `~/.claude/settings.json` |
| **Codex** | OpenAI 的 `codex` CLI | `~/.codex/` |
| **DeepSeek（CodeWhale）** | `codewhale` / `deepseek` 终端 agent | `~/.codewhale/`（或 `~/.deepseek/`） |
| **Cursor** | Cursor 自带 Agent | `~/.cursor/hooks.json` |

每个 agent 在面板里有自己的字形和颜色。后续会陆续接入更多 agent——计划中的可看 issues。

---

## 会话状态

按优先级分组：**NEEDS INPUT**（先 Approval 后 Question）→ **WORKING** → **IDLE** → **OFFLINE**（垫底）。

| 状态 | 字形 | 颜色 | 触发 |
|---|---|---|---|
| Approval | `▲` | 琥珀色，呼吸脉动 | Claude `Notification` 含权限关键词；Codex `PermissionRequest` |
| Question | `?` | 青色 | clarify 卡片（`AskUserQuestion` / `request_user_input`）；Claude `Notification` 不含权限关键词 |
| Working | 盲文 spinner | 绿色 | `UserPromptSubmit`、`PreToolUse`、`PostToolUse` |
| Done | `✓` | 暗绿色 | `Stop` |
| Idle | `○` | 石板灰 | 首条 prompt 之前的 `SessionStart`，或收到空闲通知后 |
| Offline | `⊘` | 置灰 | 远端 SSH 隧道掉线；卡片以虚线边框 + 整体置灰显示 |

本地会话在 `SessionEnd` 触发、或 agent 进程退出被存活探测发现时，会被**移除**（而不是显示成「已断开」）。远端会话隧道掉线时进入 OFFLINE 组冻结显示，重连后自动刷新。

---

## 通知（macOS & Linux）

当某个会话**进入**需要你关注的状态时，后台 daemon 会弹出系统通知并发声——即使面板没聚焦、甚至没打开，也能把你叫回来。通知由 daemon（不是 TUI）触发，每次进入对应阶段只响一次。

| 阶段 | 触发状态 | 默认声音 |
|---|---|---|
| `clarify` | Question——clarify 卡片 / 等待你输入 | Submarine |
| `approve` | Approval——批准危险操作 | Bottle |
| `done` | Done——agent 完成一个 turn | Ping |

通知**标题**固定为 `roost`；**内容**显示 `<agent> · <项目> — <在等什么>`（done 则显示完成提示）。每个 agent 家族都有标注（Claude Code / Codex / DeepSeek / Cursor），一眼看清是谁在等你。

**每个阶段可独立配置**——三个阶段各有独立的 `banner`（弹窗）和 `sound`（声音）开关，存在 `~/.roost/settings.json`：

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

- **`accent`** — 主题色（hex），用于选中卡片边框和 header 中 `roost` 品牌文字的颜色。默认 `"#d97757"`。
- **`notify.remote_offline`** — 远端 SSH 隧道掉线时是否弹通知（会话进入 OFFLINE 组）。默认关闭，在此处或设置页（`c`）开启。

可以直接编辑该文件，或在面板里按 **`c`** 打开可滚动的设置页逐个开关——切换即保存，下次通知生效。文件或字段缺失时回退为全部开启（fail-open）；`remote_offline` 例外，缺省为 `false`。

**平台支持。** macOS：弹窗用 `osascript`，声音用 `afplay`，都是系统自带、无需安装。Linux：用 `notify-send`（来自 `libnotify-bin` / `libnotify`），各阶段的声音通过 freedesktop 的 `sound-name` hint 随通知一起播放（`message-new-instant` / `dialog-warning` / `complete`）；当只开声音不开弹窗时，回退到 `canberra-gtk-play` / `paplay`。工具缺失、或没有桌面会话（headless / SSH）时，通知静默降级为 no-op，**绝不报错**；`notify-send` 不存在时 `roost setup` 会提示你装哪个包。其它平台一律 no-op。

---

## 远程 fleet（SSH）

agent 跑在远程 dev box 上?`roost add user@devbox1` 把那台机器的 agent 接进你本机面板——而且它们的通知弹在 **你** 这台机器上。

本机是 **hub**:唯一的 daemon、TUI、通知都在这。每台远程是被动转发端,**不跑 daemon**。`roost add` 经 SSH 在远程装好 roost 及其 hook(二进制缺失则自动安装),随后本机 daemon 看守一条常驻的 SSH 反向转发隧道(`ssh -N -R`),把远程的 hook 事件透传回 hub。已接入的远程记录在 hub 的 `~/.roost/remotes.json`,daemon 启动时自动重新拉起。

| 命令 | 作用 |
|---|---|
| `roost add user@host [--origin name]` | 安装 + 注册一台远程,拉起隧道 |
| `roost remove user@host [--purge]` | 停隧道、注销、卸载远端 hook |
| `roost remotes` | 列出远程及隧道状态(up / reconnecting / down) |

远程会话在面板里标出来源主机。隧道掉线时,这些会话会**冻结**(置灰、虚线边框)而不是消失,daemon 自动重连;它们在该 agent 的下一个事件时刷新。

**前提。** 免密、非交互的 `ssh <host>` 必须可用(roost 不引入任何自有认证);远程需要联网做一次性二进制安装。**远端掉线通知**可用但**默认关闭**——在设置页(`c`)或 `~/.roost/settings.json` 里开。

**跳转。** `o` 无法从本机聚焦远程终端;对远程会话它只会告诉你该 SSH 进哪台机器。**安全。** 能在远程写那个转发 socket 的人就能往你本机 daemon 注入事件——个人 dev box 没问题,共享机器需留意。

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
| cmux | surface 级 | 经其 Unix socket 发 `surface.focus` JSON-RPC |
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
