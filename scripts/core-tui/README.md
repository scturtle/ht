# Herdr TUI：启动与使用

`ht` 是 Linux 上的 Herdr 终端客户端，沿用原有 UI，只使用 `herdr-core` 运行程序。
`--core` 仍可用，但它只是默认入口的别名。构建依赖见[仓库 README](../../README.md#build)，
Core 源码版本、补丁及协议要求见 [Core runtime](../core-runtime/README.md)。

## 连接现有 Core

两份二进制都在 `PATH` 时，直接运行 `ht`。它会连接默认 Core，未运行时启动它。
App 与 `ht` 使用同一个 Linux 用户和 Core endpoint（通常是
`$XDG_RUNTIME_DIR/herdr-core/default.sock`），即可共享 `herdr-app` 布局；启动顺序不限。
空布局可直接用 `prefix+c` 创建第一个 Tab。

- `HERDR_CORE_BIN` 指定 Core 可执行文件，默认从 `PATH` 查找 `herdr-core`。
- `HC_SOCKET=/path/to/core.sock` 只连接该处**已运行**的 Core，不自动启动。
- `--core-session <name>` 选择私有会话，不可与 `HC_SOCKET` 同用；
  `--core-home <dir>` 指定客户端状态目录，单独使用不会隔离 Core。

建议使用 `just build-core` 构建仓库固定的 Core。`ht` 要求 `terminal.attach_ordered`、
`keyboard_stacks_v1` 等能力，不会降级到旧 attach。构建新二进制不会替换、重启或迁移
正在运行的 Core；遇到能力不匹配，可先用下文的隔离启动器验证。

## 隔离试用

在仓库根目录运行：

```sh
bash scripts/core-tui/run.sh
```

启动器构建并运行真实 TUI，保留调用方的终端 stdin，清除继承的 `HC_*` 和 `HERDR_*`
设置。默认使用 `.local/core-tui/existing/`，根据目录派生固定的 `trial-…` 会话名，
socket 位于 `<home>/run/herdr-core/`。换用另一个 `CORE_UI_HOME` 就能试用新会话，
不动现有 Core 或程序。

| 变量 | 用途 |
| --- | --- |
| `CORE_UI_HOME` | 隔离目录。启动和 `--stop` 必须使用同一目录。 |
| `CORE_UI_CONFIG` | 指定 TUI 配置文件；未指定时，使用 `<home>/config/herdr/config.toml`，文件不存在则用默认值。 |
| `CORE_TUI_HC` | 使用指定的兼容 Core，跳过 Core 构建；仍构建 `ht`。 |
| `CORE_UI_CLIENT_ID` | 显式指定客户端身份；不要让同时运行的客户端共用同一身份。 |

如果外层 Herdr 或 tmux 截获了前缀键，可以使用随附的[试用配置](try-config.toml)：

```sh
CORE_UI_CONFIG="$PWD/scripts/core-tui/try-config.toml" bash scripts/core-tui/run.sh
```

试用配置用 **Ctrl+b** 或 **F12** 作前缀，以 `prefix+q` 或 **F10** detach。
普通 detach 只退出客户端，程序继续运行。要停止试用 Core **及其中的程序**，才运行：

```sh
bash scripts/core-tui/run.sh --stop
```

私有会话没有布局文档时会创建一个 Shell；最后一个 Pane 结束后客户端退出，但 Core 仍运行。
隔离运行不配置 systemd/linger，不保证注销后会话存活。

## 导航与配置

下表使用默认前缀 **Ctrl+a**。`prefix+…` 表示先按前缀，再按后续键；已有 `[keys]` 可覆盖默认值。

| 操作 | 默认键位 |
| --- | --- |
| 帮助 | `prefix+?` |
| 工作区选择器 / 导航器 | `prefix+w` / `prefix+g` |
| 新建 / 重命名工作区 | `prefix+Shift+n` / `prefix+Shift+w` |
| 新建 / 重命名 Tab | `prefix+c` / `prefix+Shift+t` |
| 按栏上顺序切换 Pane / 按编号切换 Tab | `prefix+p`、`prefix+n` / `prefix+1..9` |
| 重命名 / 关闭当前 Pane | `prefix+Shift+p` / `prefix+x` |
| 键盘复制模式 / 使用编辑器查看历史 | `prefix+[` / `prefix+e` |
| 重载客户端配置 / detach | `prefix+Shift+r` / `prefix+d` |

没有常驻侧栏。一次显示一个 Pane，同 Tab 的 Pane 在栏上以 `1.1`、`1.2` 等编号列出；
点击可切换，拖动可重排 Tab。工作区选择器支持方向键和右键菜单；导航器支持方向键或
`j`/`k`，按 Enter 确认。`ht` 不绘制分屏或 Zoom，也不改动 App 的分屏几何。
关闭 Pane 会结束对应程序；关闭工作区会结束只属于该工作区的程序。App 标记为不可关闭的
Pane 受保护。

直接启动时通常读取 `~/.config/herdr/config.toml`，可用 `HERDR_CONFIG_PATH` 指定文件。
`ht --default-config` 查看支持项，`ht config check` 检查配置。热重载不修改 Core 配置。
`terminal.default_shell` 和 `[advanced] scrollback_limit_lines` 在客户端启动时读取，修改后需
detach 再连。前者只影响新建程序；后者默认保留 `1000` 个物理行，`0` 禁用客户端历史。
Ghostty 按整页回收，实际保留行数可能略多；Core 已丢弃的历史无法恢复。

UI 默认使用宿主终端的默认色和 ANSI 色，`[theme.custom]` 只覆盖显式设置的 UI 颜色项。
不支持内置主题或亮暗切换；退役的配置键会提示并忽略，不改写文件。
宿主默认前景、背景与 256 色调色板只在启动和手动重载时通过 OSC 查询。
退出时先关闭输入报告并离开备用屏幕，再等待终端确认、读取在途回包（最多两秒），
最后把 TTY 交还外层 shell。退出不重新查询或改写宿主颜色。

## 多客户端与运行边界

- Core 持有 PTY、程序和共享布局；客户端各自保留焦点、滚动和选择。布局修改使用
  `herdr-app` 文档的局部 CAS 更新，不覆盖无关字段。App 的插件项保留在文档中，
  但不显示在 TUI。
- 每个客户端使用一条 `hc connect` relay。断线时用 `hc connect --existing` 恢复，
  不启动或复活已停止的 Core，也不重放投递结果不明的输入。换客户端不需要重启 Core。
- PTY 大小按 `latest` 仲裁：最后操作该 Pane 的客户端决定网格，其他客户端跟随；
  后台 Pane 不抢尺寸。Ordered attach 在每次首帧和替换重绘前发送权威尺寸。
- 默认按 TTY 派生客户端身份，焦点偏好保存在 `clients/<identity>/app-preferences.json`。
  直接启动可设 `HERDR_CORE_CLIENT_ID`（1–96 个 ASCII 字母、数字、`-`、`_`）。
- Kitty 图像显示需要宿主可用的单元格像素尺寸。Core 的首屏及重连重放不包含图像数据，
  已显示过的图像可能需要程序重新发送。
- Agent 状态仅来自程序上报的 OSC 7501，不从屏幕猜测。没有旧 Herdr 服务端、远程 Core
  传输、集成安装器或 CLI API 桥；管理 Core 程序请用 `hc`。

## 日志与验证

客户端日志为状态目录下的 `herdr-client.log`：隔离启动时在 `CORE_UI_HOME` 下；直接启动
默认在 `$XDG_STATE_HOME/herdr/core-client-v1/`（无 XDG 设置时用 `~/.local/state`）。
退出时会先停止 stdin 读取、关闭终端报告模式，再恢复 tty；不等待宿主颜色回复。
仍在传输中的回包可能在交还 tty 后到达。

```sh
just core-launcher-test         # stdin 和隔离会话选择
just smoke                      # 真实 TUI，通过私有 Core/PTY 串行验证
just smoke terminal-restore    # 不等宿主回包，验证 TTY 交还
just smoke keyboard graphics   # 只运行指定烟测
just bench-render-scale        # 固定几何下比较 1 与 15+ Pane
just bench-render-stages       # 测量渲染各阶段
```

烟测清单、前置条件及测量边界见 [TUI 测试文档](../../tests/core-tui/README.md)。
