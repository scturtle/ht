# 本地 Core 运行时

本仓库使用带本地补丁的 **herdr-core v0.1.22**。版本和补丁顺序以 [`source.json`](source.json) 为准，其中记录了 Core 与其 Ghostty 子模块的提交、下载地址和 SHA-256。这里的 Ghostty 与 `ht` 使用的 `libghostty-vt` 是两套源码。

## 构建与验证

在仓库根目录运行：

```sh
just build-core   # 下载、校验、打补丁并构建
just test-core    # 构建脚本契约测试和 Core 的 Rust 测试
just smoke        # 用私有 Core 和 PTY 验证真实 TUI
```

依赖见[仓库构建说明](../../README.md#build)。Core 固定使用 `.local/toolchain/zig/0.16.0/zig`，不读取外部 `ZIG`；构建和测试均使用本机架构的 `*-unknown-linux-musl` 目标。

[`build.py`](build.py) 校验归档、以零模糊度应用补丁，并在构建或测试前后校验准备好的源码。它不会覆盖手改的准备目录，也不会在构建失败时替换可执行文件。

| `.local/hc/` 下的路径 | 用途 |
| --- | --- |
| `downloads/` | 按 SHA-256 缓存归档，升级时保留旧归档 |
| `work/prepared/<bundle-id>/` | 已打补丁的源码 |
| `work/artifacts/<bundle-id>/` | 每个源码包独立的 Cargo/Zig 产物，避免混用 Ghostty 静态库 |
| `bin/herdr-core` | 构建成功后原子替换的可执行文件 |

**构建不会重启或迁移正在运行的 Core，也不会修改 systemd/linger。** 重新连接旧 socket 仍会使用旧进程。验证新二进制时用[隔离启动器](../core-tui/README.md#隔离试用)和一个未使用的目录；手动启动时同时指定 `--core-home` 和 `--core-session`，并取消继承的 `HC_SOCKET`。`hc --version` 不区分同版本的不同构建，排查时请核对 `source.json` 中的提交和运行中 ELF 的 Build ID。

## 本地补丁

| 补丁 | 保留的行为 | 真实 TUI 验证 |
| --- | --- | --- |
| [0001](patches/0001-ordered-interactive-attach.patch) | `ordered_attach` / `terminal.attach_ordered` 保证首屏和替换重画的顺序为 **RESIZE → 重画 → 实时 DATA**。观察者不改尺寸；恢复的被动客户端不因重连或自动回包抢占尺寸。旧 `terminal.attach` 不变。 | `just smoke multiclient` |
| [0002](patches/0002-keyboard-stack-replay.patch) | `keyboard_stacks_v1` 完整重放主屏和备用屏的 Kitty 键盘标志栈，包括溢出槽及后续弹栈。 | `just smoke keyboard` |
| [0003](patches/0003-existing-connect.patch) | `existing_connect` / `hc connect --existing` 只连接已运行的 Core，避免重连复活已停止的会话；普通 `hc connect` 保留自动启动。 | `just smoke reconnect` |
| [0006](patches/0006-single-winner-image-answers.patch) | 每次图像查询只接收第一份匹配的 `AUTOMATIC_INPUT` 回包。关联的 DA1 放行后，迟到的能力探测回包被丢弃；正常上传 ACK 不受此期限影响，传统 DATA 保持透传。 | `just smoke image-answers` |
| [0007](patches/0007-pty-pixel-geometry.patch) | 将尺寸主控客户端的像素尺寸传给 Linux PTY，供 `kitten icat` 等程序读取。同网格切换主控也会更新像素；客户端的 RESIZE 帧仍是四字节。**重新构建 Core 或重连不会升级已运行的旧 holder。** | `just smoke icat`（依赖 kitten，可用 `CORE_TUI_KITTEN` 指定） |

只有固定的上游版本提供等价行为、宣告客户端依赖的能力（如适用），且相关 Core 和 TUI 测试在移除补丁后仍通过，才移除对应补丁。`0004`（重画完成标记）和 `0005`（单元格读取与语义键）已退役，不参与构建。

## 协议边界

- 尺寸采用 `latest`：一个 PTY 只有一套网格，由最后操作该 Pane 的客户端决定；其他客户端跟随，空间不足时裁剪而不重新换行。
- 有序重画不提供完成标记或宿主绘制确认；重连不会重放投递结果不明的输入。
- 保留进程不代表能恢复图像。Core 不保存完整的 Kitty 图像场景，替换客户端可能需要程序重新发送图像。
- 此客户端不支持远程 Core 传输或旧 Herdr 的 CLI API 桥。
