# Core-backed Herdr TUI

这里只保留**真实现有 Herdr TUI 的默认 Core 后端**，没有独立的替代 TUI。
旧 Herdr runtime 已物理删除；默认 `ht` 使用 Core，`--core` 仅为兼容别名，没有旧后端 fallback。
公开 generation-1 wire codec 和冻结夹具保持不变。
这是个人 fork 的进行中迁移，**不是功能完整或稳定版承诺**。

## 目录与职责

- `src/client/core/`：真实 TUI 的 Core 适配，默认入口在 `src/main.rs`。
- `src/client/shell/`、`src/ui/`：原版交互与绘制。
- `scripts/core-tui/`：本启动器及独立试用配置。
- `scripts/core-runtime/`：固定源码的 Core 构建工具和补丁。
- `tests/core-tui/`：真实 TUI 的隔离烟测和测试用 ANSI 屏幕解码器。

目录整理没有改变 `.local/core-tui/existing/` 下的运行时或客户端身份，
不会重启已有 Core，也不迁移其中的程序。

## 现有 Herdr TUI（试用入口）

```sh
bash scripts/core-tui/run.sh
```

使用真正的 `ClientShellState`、侧栏、Tab 栏、键位、鼠标路由、通知和 Ghostty，
不另造近似 UI。脚本构建固定源码的增强 Core 和当前 Herdr release binary，
然后运行默认入口。也可用 `CORE_TUI_HC` 指定兼容的增强 Core。
**现在要求 `ordered_attach` / `terminal.attach_ordered` 与 `keyboard_stacks_v1`。未打补丁的 Core，以及包含已退役 paint 屏障补丁的旧试用 Core 也能连接（这些能力只是不再要求），但缺少前两者的 Core 会被明确拒绝。**
重建二进制不会升级已经运行的 server；不会自动重启、替换或迁移旧程序。如需马上试用新门禁，可显式选择
新的隔离 runtime（这会创建新程序，不是迁移原程序）：

```sh
CORE_UI_HOME="$PWD/.local/core-tui/ordered-trial" \
CORE_UI_CONFIG="$PWD/.local/core-tui/personal-config.toml" \
  bash scripts/core-tui/run.sh
```

后文的“不必停止 Core”均以 server 已满足能力要求为前提。

构建输入、补丁范围、测试和移除条件见 [Core runtime](../core-runtime/README.md)。
脚本清除继承的 `HC_*` / `HERDR_*` 身份和 socket 设置，运行时独立放在
`.local/core-tui/existing/`，不会连接旧 Herdr server。TUI 仍读取正常 Herdr 界面配置；
可用 `CORE_UI_CONFIG=/path/to/config.toml` 显式选择另一份配置。
**已有 `[keys]` 会覆盖默认键位**，例如前缀可能是 `Ctrl+a` 而不是 `Ctrl+b`。
嵌套在 Herdr/tmux 中运行时，相同前缀还可能被外层截获；可以使用独立终端，
或显式使用随附的试用配置（不会修改全局配置）：

```sh
CORE_UI_CONFIG="$PWD/scripts/core-tui/try-config.toml" \
  bash scripts/core-tui/run.sh
```

试用配置同时保留 **Ctrl+b** 和 **F12** 两个前缀，以及常规的 `前缀 v`、`前缀 -`、
`前缀 Tab`、`前缀 q`。另有 **F2** 左右分屏、**F3** 上下分屏、**F6** 下一 Pane、
**F10** detach。嵌套外层截获 Ctrl+b 时，可改用 F12（例如 F12 然后 `?` 查看帮助）。

本工作目录另有不提交的个人配置副本 `.local/core-tui/personal-config.toml`，
保留当前全局配置的 fish、底部 Tab、隐藏侧栏、主题和 Ctrl+a 键位；只增加 F12 备用前缀、
旧终端 `|` / `_` 别名及允许嵌套。原全局配置不变：

```sh
CORE_UI_CONFIG="$PWD/.local/core-tui/personal-config.toml" \
  bash scripts/core-tui/run.sh
```

此副本保留 `prompt_new_tab_name = false`；新 Tab 不弹命名框，可用 Ctrl+a Shift+T 重命名。
`terminal.default_shell` 会作为 Core 创建程序时的 executable（不是 shell command 字符串）传递；
未配置时由 Core 选择默认 shell。它不会替换已经运行的程序。

入口曾有独立的 stdin 缺陷：用 Python heredoc 再 exec TUI，导致界面显示但
键盘输入丢失，换键位不能修复。现已把启动逻辑移到 `run.py`，保留调用方
的终端 stdin。满足能力要求的 Core 只需重进界面，不必停止 Core 或删除会话。
`just core-launcher-test` 检查启动脚本保留调用方 stdin，以及 `--stop` 只作用于
自己那个私有 Core。原先通过真实 PTY 驱动同一启动脚本的完整 UI 烟测按低频率随机
超时（等待条件靠屏幕内容，负载高时不再收敛），已移除；分屏、鼠标、Tab、detach
重连等行为仍由 `core-multiclient-smoke`、`core-keyboard-smoke`、`core-reconnect-smoke`
覆盖。
鼠标测试会先检查真实输出中的
上报模式仍启用，不能在终端本来不会上报时强行注入鼠标来冒充通过。
另已修复把程序鼠标需求误当成整个 TUI 捕获开关的问题；普通 Shell 不请求鼠标，
也不应关闭 TUI 的鼠标操作。
此前直接启动二进制的四项 smoke 没有覆盖这一入口缺陷。

## 单连接与事件驱动调度

每个 ht 只有一个 Core 连接和一条带关联身份的命令队列，不再包含单客户端多机器目录、
SSH endpoint 激活/交接。**这不是单客户端限制**：手机、桌面等多个 ht 仍可同时连接
同一 Core，保留独立焦点、选择、滚动、Zoom，以及 `latest` 权威网格策略。
不确定投递不重试；generation / projection boot / request id 的请求围栏仍然有效。

适配器直接发布已有类型化 snapshot，不再在进程内序列化 JSON 再解析。
Core worker 由命令、帧/断线、异步写完成唤醒，无事时 park；有写入时保留实际超时 deadline，
有剩余 graphics dirty 时继续发布。UI 只在 dirty/repaint/实际定时器变化时合成，不因每个事件重画。
relay 重连会创建新的投影 epoch，使重建后从低值开始的 revision 不被旧缓存拒绝，
同时取消旧 scope 请求并全量重绘；不会因此重启 Core 或程序。

`just core-scheduling-smoke` 在自建私有 Core/PTY 中检查空闲 worker 上下文切换、
空闲输出字节、20 次按键到完整同步输出帧的延迟，以及高 revision 后仅丢失 relay 的恢复。
它只终止经过 socket/命令行验证的测试 relay，不以分屏、resize 或 focus 手动恢复程序。
可以比较预先保存的旧 release binary：

```sh
CORE_UI_BASELINE=/absolute/path/to/old-ht just core-scheduling-smoke
```

这些是**本机 PTY 调度/输出测量，不是 WAN RTT，也不是物理终端绘制时间或整机 CPU 指标**。
测试只运行本地原始终端 fixture，不调用模型；Core、程序及临时文件均属于本次测试。

## Pane 冻结现场诊断

遇到冻结时，先不要分屏、调整尺寸或切换 pane：点击侧栏 `menu` →
`dump pane diagnostics`；也可在目标 pane 的右键菜单选择 `Dump pane diagnostics`。
通知会给出 `/var/tmp/ht-pane-dump-<client-pid>-<timestamp>/` 路径。
目录权限为 `0700`，**文件包含屏幕文字、程序信息，分享前务必检查敏感内容**；不会自动上传。

- `client.json`：UI 焦点/模式、snapshot/surface revision、pane cells、待处理请求、输入/滚动状态、客户端线程状态及 wait channel。
- `adapter.json`：目标 pane 的 channel、尺寸、credit、解析器模式（含 synchronized output）、viewport 和适配器状态。
- `core.json` / `core-screen.txt`：独立 existing-only relay 只读查询的 Core 状态、程序事实及当前屏幕。
- `report.json`：版本、时间、来源 socket、耗时和缺失/超时项。三层数据独立采样，不是原子快照。

抓取不向 pane 发送输入、focus、resize、attach，不消费渲染 dirty 状态，也不操作程序生命周期。
后台采集预算约 4 秒；Core 或适配器无响应也保留已拿到的部分，不阻塞 UI。
`just core-diagnostics-smoke` 在私有会话验证两个菜单入口、缺失 socket 不启动 Core、
暂停测试 Core 时仍能保存部分信息，且抓取不恢复 Core、不改变原程序 PID/网格/attach channel。
更新客户端只需 detach 后重新运行 `ht`，**不需要重启 Core 或 Pi**。

## 支持范围

目前支持：

- 多 Workspace、多 Tab、每 Tab 的 BSP 分屏；现有创建、命名、移动、关闭、切换 UI/API。
- Core 的 `herdr-session-v1` CAS 文档是共享组织/布局的唯一权威；外部修改和 missed events 触发重读。
  显式关闭先持久化 retirement tombstone 再终止程序；不确定的发布结果不会导致误杀。
  Core 确认正常退出或终止后，以纯 CAS 移除对应 Pane 和空 Tab/Workspace，不再发 kill；
  lost、restart、server stop、CLOSE 或未知记录本身不作为程序退出证据。
  最后一个程序结束后退出 TUI，恢复外层终端，但不停止 Core。
  用户下次显式启动 TUI 时复用正常 Workspace 创建动作；不是断线恢复，不采用旧的孤立程序。
- Tab 命名复用原版 `Workspace::tab_display_name` 拆出的纯函数：无自定义名时按当前顺序显示
  `1、2、3…`，关闭/移动后重排，不带 `Tab` 前缀；稳定 ID/counter 不作显示编号。
  命名框未改默认建议时保持自动命名；自定义名固定。共享文档新增可选命名意图，旧默认标签
  只在显示时兼容，不改存储文本。请把各个试用 TUI 都脱离后以同一新版重进，Core 不必停。
- Pane 重命名复用原版对话框、快捷键和去空白/清空规则；个人配置可用 Ctrl+a Shift+P。
  名称只存 Core 的 `herdr.display_name` 标签，跨客户端同步，脱离重进保留，清空恢复默认；
  不修改 Core 程序身份、PID、BSP 文档、尺寸领导权或 OSC 7501 状态。
  元数据通过 retained surface 更新，不重画终端；Core 缺少可选的 `terminal.label` 时只禁用此动作。
- 关闭后迟到的按键释放/重复、鼠标释放/拖动等延续事件对已结束目标为空操作，绝不改投其它 Pane；
  真正的新输入/粘贴失败仍提示，只有粘贴错误显示 `Paste rejected`。
- Zoom 复用原版状态转移和全屏几何：单 Pane 无操作、On/Off 幂等、缩放中导航切换目标；
  本地或远端插入/删除 Pane 后恢复 BSP。缩放只保存到客户端 preferences，不改共享文档。
  隐藏 Pane 不竞争尺寸；重新显示时即使期望网格未变，也重新声明一次，避免沿用其它客户端的网格。
- Pane Swap 复用原版 `TileLayout::swap_panes` 和方向查找，保持程序身份、焦点、缩放和 BSP 形状；
  支持方向交换和显式 ID 对。原版无邻居/同 Pane/跨 Tab/未找到的无操作结果保留。
  CAS 重算是纯布局操作，不重放输入或程序操作；无变化时不写相同文档、不推进版本。
- focus、active Workspace/Tab、scroll、selection、Zoom 属于各客户端；focus/几何变化不写共享文档。
  preferences 位于 `clients/<identity>/preferences.json`，旧 `layout.json` 只在首次空文档时迁移。
- 多客户端可连接同一 Core 程序。Unix 自动按 TTY 派生身份，也可显式指定
  `HERDR_CORE_CLIENT_ID`（直接启动二进制）或 `CORE_UI_CLIENT_ID`（使用 launcher），
  值为 1–96 个 ASCII 字母、数字、`-`、`_`。launcher 仍丢弃外层继承的 `HERDR_*` 身份；
  只有显式 `CORE_UI_CLIENT_ID` 才选用指定身份。不要让两个同时运行的客户端使用同一身份。
  锁只阻止同一身份重复连接，不再锁住整个 Core home；没有稳定身份时不持久化 preferences。
- attach 使用 `terminal.attach_ordered`：每次首帧与替换重绘之前都有权威 RESIZE→DATA→live DATA，
  没有额外的 paint 完成标记。重放与实时字节不作区分，输入不做完成前闸门（已知且接受的让步）。
  parser 跟随 Core 实际网格，客户端期望尺寸独立保存，不用权威尺寸反向制造 resize。
  后台 Tab 的 attach 被动参与，后台窗口变化不抢尺寸领导权。
- 几何策略为 `latest`：获得焦点或重新显示 pane 时客户端主动补发 RESIZE 夺回该 pane 的网格；
  其他客户端收到 RESIZE 后跟随重绘，因此不会错格，但布局更小的一侧显示为裁剪而非重新折行。
  一个 PTY 只有一个尺寸，两端不可能各自满屏吻合。
- Core 唯一拥有 PTY、进程和 OSC 7501 状态；没有屏幕猜测或 Agent 专属脚本。未报告状态就是 Unknown。
- 输入经 Ghostty 语义编码；自动回复走 automatic-input，不冒充人的输入、不抢领导权。
  Kitty 主/备用屏幕完整 flags 栈可恢复；真实 TUI 已验证 press/repeat/release、替换客户端后的逐层 pop。
- 工作区语义与原版对齐：无自定义名时工作区名来自"第一个 tab 的根 pane cwd"
  （`derive_label_from_cwd`，HOME 显示 `~`），随 `cd` 更新；`WorkspaceInfo.number`
  与侧栏报的是**位置**（`index + 1`），不是不复用的计数器；关闭当前工作区后按
  原版 `close_workspaces` 规则落到"占据该位置的那个，越界夹到末尾"。
  共享文档里空 `label` 表示"无自定义名"，校验通过 `valid_name` 放行。
- 断线重连由适配层自己完成：`Session::resume` 用 `hc connect --existing`（补丁
  0003）重连，重新 bootstrap、恢复客户端本地展示偏好、清掉旧通道、重置保留帧
  （但保留已投递给宿主的图像缓存），再以权威 RESIZE 重新 attach 每个 pane。
  排队输入**丢弃而非重放**（Core 没有启动纪元，重放可能作用在过期几何/模式上）。
  重启会在 1 秒内重建 socket，因此 socket 连续缺失 5s 判定为"用户主动 stop"，
  TUI 以原来的 `lost connection to Core` 退出。断连期间不向 TUI 发
  `ServerDisconnected`，所以 UI 无需改动。
- Kitty graphics 由客户端解析并绘制：pane 的 Ghostty 终端按配置启用图像存储，
  复用原服务端的 `collect_scene`/`collect_visible_placements`（从旧 server 恢复，
  只把"pane 来源"换成当前布局），再交给原有 `ClientState` 编码到宿主终端。
  宿主必须报告 cell 像素（或回答 `CSI 16 t`），否则无法测量放置，图像被正确跳过。
  已知边界：attach/重连后的**首帧**重放不含图像字节（Core 的 paint 只有字符网格），
  程序需重新发送；这与 Ghostty 自身"快照不保留 image/placement"的契约一致。
- copy-mode 复用现有 UI：选择/复制/滚动、行首尾、大小单词和段落、正反向/循环搜索；
  有内容版本校验与有界结果，也验证了另一客户端决定尺寸时的搜索/复制。
- 增强 Core 另提供 `connect --existing`，已验证不启动、不复活被明确停止的 server。
  后续重连必须使用此模式；目前 TUI 尚未接入自动重连状态机。
- 输入/输出有界，管道写入有 5 秒上限，RPC 总预算 15 秒（包含写入）。超时或断流会隔离旧连接，
  不重试不确定已送达的输入/动作，也不接受迟到回复；这还不是自动重连。
  CREDIT 用有界的异步标量队列，避免每个输出 chunk 都等待写线程往返。
- 消费 DATA 后才回补 credit；hidden output 更新 parser，不额外重绘后台 Pane。
  metadata-only snapshot 配套匹配 revision 的 retained surface，避免卡住现有 overlay。
- 默认键位：`Ctrl+b v` 左右分屏、`Ctrl+b -` 上下分屏、`Ctrl+b Tab` 切换 Pane、`Ctrl+b q` detach。

### 必需能力与当前缺口

以下是用户明确要求的迁移验收项，不能以 Unsupported 或“以后支持”作为完成结果：

| 能力 | 当前阶段 |
| --- | --- |
| 多 Workspace、多 Tab | 主要实现和真实 TUI 生命周期/并发 CAS/跨客户端 membership 与本地 focus 测试通过。 |
| live handoff | 已验证客户端替换后的同 PID、键盘模式栈与输入；有界断线重连、旧 epoch fencing、holder-preserving Core server 重启/替换尚未实现和验收。 |
| Kitty graphics | 主机显示/缓存组件保留，但适配器尚未发布图片 scene；Core 首屏/重连/重启图片重放仍缺失。 |
| Kitty keyboard protocol | 编码、主/备用屏幕完整栈、脏 viewer、溢出、release/repeat、替换客户端后 pop 已验证；完整 text/AltGr/dead-key 与 server restart 联合验收待做。 |
| copy-mode | 单元测试和真实 TUI 搜索/移动/选择/复制/退出通过；多客户端不同权威尺寸下的搜索复制也通过，最终 handoff 联合验收未完成。 |
| 多客户端尺寸协调 | 新的显式 ordered attach 已实现；两个真实默认 TUI 验证输入、窗口 resize、detach 的尺寸仲裁，后台 Tab 不竞争、无 resize feedback；尚未验证更大客户端数和断线恢复。 |

远程 Core transport、完整自动化 CLI bridge 尚未实现；旧 socket/integration 命令明确禁用。
上游自更新也已禁用，防止装回退休 runtime。
此前曾观察到窄 Pane resize 后 Bash 提示符重复重绘及 Core `read` 缺少部分最近输出；
本轮几何仲裁通过不等于已证明任意 resize 无损。客户端只能恢复 Core 实际保留的内容。
Core 使用独立 runtime dir，不自动配置 systemd/linger；不承诺注销存活，请勿放重要任务。

直接使用构建好的二进制时：

```sh
just core-source-build
HERDR_CORE_BIN="$PWD/.local/core-runtime/target/release/herdr-core" \
  target/release/ht --core-home /absolute/path/to/a-private-core-home --core-session private-demo
```

`HERDR_CORE_BIN` 默认是 PATH 上的 `herdr-core`。Core home 只能由 `--core-home` 指定，
**不读任何环境变量**，避免被外层进程悄悄改掉。

默认客户端连接 `$XDG_RUNTIME_DIR/herdr-core/default.sock`，也就是 `hc` 默认那个 Core，
因此 `ht` 里开的终端通过 `hc ls` 直接可见。`--core-home` 只指定客户端状态目录；
`--core-session <name>` 才选择并隔离私有 Core，会话 socket 仍遵循 hc 的命名规则。

试用脚本根据 home 生成固定的 `trial-…` session，并将 XDG runtime/state/config 目录
都放在该 home 下。启动和 `--stop` 共用这组标识，socket 位于
`<home>/run/herdr-core/trial-….sock`，不会使用旧的 `<home>/core.sock` 或外层的 `HC_SOCKET`。

停止试用脚本启动的 Core **及其中全部程序**：

```sh
bash scripts/core-tui/run.sh --stop
```

### 验证入口与当前证据

```sh
just core-source-test
just core-copy-smoke
just core-organization-smoke
just core-multiclient-smoke
just core-keyboard-smoke
just core-launcher-test
just core-paint-smoke
just bench-render-scale
```

真实 TUI 烟测用 `/var/tmp` 私有 HOME/config/runtime/PTY，finally 只停止自己的 Core。
禁用桌面 clipboard 环境，仅捕获 OSC 52；Bash 无 profile/rc，键盘测试使用独立 raw-PTY peer。
`tests/core-tui/Cargo.toml` 只构建测试用 `screen-oracle`，依赖仅 `vt100` 与 `serde_json`，不含 TUI、Core transport
或程序运行逻辑。它解码真实 TUI 的 PTY 输出并重放 resize 字节边界；不能用 retained ANSI
原始子串冒充屏幕。测试解码器不是另一条用户入口。

最近验证：**Herdr 1,898 passed / 8 skipped**；固定源码增强 Core 源码本轮未改，复验全套为
**449 passed / 3 ignored**。`just lint` 及七项真实 TUI 烟测通过。它们都明确返回 `full_migration_acceptance: false`。

默认 TUI 已消费新完成边界；Core 两个旧 attach 的协议仍未改，新 TUI 不静默降级到旧方法。
输出边界／本地队列令牌不等于 Core input/boot epoch 或 host 实际绘制确认，也不证明 graphics 恢复或完整 handoff。详见
[`scripts/core-runtime/README.md`](../core-runtime/README.md)。
另在私有 HOME/PTY 下用真实 fish 验证个人副本：底部 Tab、Ctrl+a、`|` / `_` 分屏、
无新 Tab 命名提示、Tab/Pane 重命名、清空 Pane 名称、Ctrl+a z 缩放、Ctrl+a Shift+H/L
缩放中交换 Pane、exit 和 Ctrl+a d 均通过；未驱动用户会话。
这不是 `just check` 全绿：部分退休 runtime 维护契约仍待清理，Bun 缺失、CoW 不可用、Windows 未验证。

本轮固定 120×40 采样：未命名、未 zoom 的 1／15 Pane render 为 **548.71／541.13 µs**，
1→15 delta **−7.58 µs**；增加 14 个 populated 后台 Tab 的单 Pane delta **−3.88 µs**。
相较上一阶段的 −3.81／+0.41 µs 没有可见线性放大，负值属于本机噪声，不是优化证明。
未测实际多客户端 fanout。新队列令牌只在 attach／paint／生命周期边界更新，DATA／render 不更新 registry。

下表保留上一阶段的基线（µs/frame，非 CI 时间阈值）：

| 布局 Pane | populated 后台 Tab | 所有 Pane 均有名称 | Zoom | render | retained projection |
| ---: | ---: | :---: | :---: | ---: | ---: |
| 1 | 0 | 否 | 否 | 548.33 | 164.82 |
| 15 | 0 | 否 | 否 | 544.52 | 169.60 |
| 1 | 14 | 否 | 否 | 548.74 | 166.14 |
| 15 | 14 | 否 | 否 | 541.71 | 166.82 |
| 1 | 0 | 是 | 否 | 552.39 | 170.01 |
| 15 | 0 | 是 | 否 | 542.43 | 167.25 |
| 1 | 14 | 是 | 否 | 546.35 | 166.36 |
| 15 | 14 | 是 | 否 | 541.92 | 167.63 |
| 1 | 0 | 否 | 请求后仍为否（单 Pane） | 553.34 | 170.04 |
| 15 | 0 | 否 | 是，仅绘制 1 Pane | 331.44 | 165.90 |
| 1 | 14 | 否 | 请求后仍为否（单 Pane） | 547.89 | 165.93 |
| 15 | 14 | 否 | 是，仅绘制 1 Pane | 325.50 | 165.70 |

未缩放、未命名时 1→15 可见 Pane 增量 −3.81 µs（−0.7%），增加 14 个后台 Tab +0.41 µs（+0.1%）；
全部命名时分别为 −9.96 µs（−1.8%）及 −6.04 µs（−1.1%）。
Zoom 的 15 Pane 行只绘制 1 个 Pane，不可当成 15 可见 Pane 的加速结果。
小幅差异和负增量受采样噪声影响，不能当成优化证明；未测多客户端 frame fanout scaling。
针对新增写入超时的管道专项采样还比较了 1/15 个 channel 的 CREDIT 开销：
原同步写入 5.00/28.35 µs，一律等待写线程曾增至 8.35/74.18 µs，
因此改为有界异步 CREDIT，采样为 1.38/16.18 µs；这些仅为本机支持性证据。

`existing-ui-smoke.py` 是初始单客户端 milestone 的历史烟测，仍含旧 Unsupported/互斥断言。
新专项烟测复用它的 PTY driver，但不执行其过时主流程，也不把它当成最终 migration gate。
键盘烟测的清理曾误用不存在的顶层 `stop`，已改为检查返回码的 `server stop`，
等待私有 socket 删除后才清理目录、输出成功；失败会保留现场。
PTY driver 的异常清理会持续排空输出并有界等待，必要时只强制结束自己创建的 viewer；
避免 renderer 堵在满 PTY 时令 finally 永久等待、跳过私有 Core 的清理。
根据用户明确指令，已停止本机 default hc 及确认属于烟测的遗留进程；Linger 未改。
当时的停机检查确认无 hc 服务/进程遗留；用户后续试用会重新启动试用 Core。
自动测试仅启动并清理自己的私有 Core，不停止用户正在试用的 Core。
没有提交、push、发布、系统安装或真实会话迁移。

