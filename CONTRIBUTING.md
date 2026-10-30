# 贡献指南

感谢你对 DesKOS / tAbLeOS 的关注！本文档说明如何构建、测试和提交改动。
小而聚焦的 patch 永远欢迎。

## 环境准备

只需要 macOS 或 Linux + 以下工具链：

| 依赖 | 版本要求 | 用途 |
|---|---|---|
| `rustup` + nightly Rust | ≥ 2024-01 | 内核与所有 ring-3 程序（target: `x86_64-unknown-none`） |
| `zig` | ≥ 0.11 | 引导扇区汇编（bootsect.S、stage2.S） |
| `python3` | ≥ 3.9 | 构建辅助脚本（mke1fss、mkimage、mkrit 等） |
| `VirtualBox` | ≥ 7.0 | 运行构建结果（headless 或 GUI） |

首次构建时 `build.sh` 会自动下载 `x86_64-unknown-none` 的 Rust 工具链；
如果本机没有 zig，会尝试从同机 e1LibreOS 的 downloads 目录复用。
**不需要 root 权限、不需要系统级编译器**。

```sh
# 确认 Rust target
rustup target add x86_64-unknown-none

# 一键构建（两个品牌 × 两个变体 = 4 个镜像）
./build.sh

# 也可以只构建 DesKOS workstation
BRAND=desk EDITION=workstation ./build.sh
```

## 目录地图

| 路径 | 内容 | 语言 / 约定 |
|---|---|---|
| `kernel/` | Rust 内核（无 std，无 heap at boot） | Rust nightly，`#![no_std]` + `#![no_main]` |
| `kernel/src/isr.S` | ISR 汇编入口（唯一的汇编文件） | NASM / GAS 语法，zig-cc 汇编 |
| `boot/bootsect.S`、`boot/stage2.S` | MBR + 二阶段引导 | zig-cc 汇编，x86 实模式代码 |
| `userspace/` | 主要 ring-3 程序（ush、hello、libe1） | Rust nightly，`no_std` + libe1 |
| `apps/` | 额外 ring-3 应用（e1wm、e1dm、e1apfs…） | Rust nightly，同 userspace |
| `tools/` | 构建辅助工具（mk*.py 系列） | Python 3，只用标准库 |
| `build.sh` | 一键构建脚本 | POSIX sh（busybox ash 兼容，不用 bashisms） |
| `run-vbox.sh` | VBox 启动脚本 | POSIX sh |

## 编码约定

### Rust（kernel、userspace、apps）

- **内核代码**严禁 `std`、严禁启动阶段使用 heap（`pmm.rs`、`heap.rs` 提供
  panic 前的分配器）。`console.rs` 等 MMIO 访问必须加 `volatile`。
- **ring-3 代码**用 `#![no_std]` + `libe1`；`libe1` 只暴露 syscall 封装层，
  不实现 libc。
- 错误处理：内核用 `panic!`（会触发 `IsrFrame` dump 到串口），用户态用
  `Result` + `unwrap`。
- 注释语言：中文或英文均可。内核现有注释大部分是中文，保持一致。
- 提交前跑：`cargo fmt --all`、`cargo check --all-targets`（kernel 里
  `cargo check` 要加 `--target x86_64-unknown-none`）。

### 汇编（bootsect.S、stage2.S、isr.S）

- `isr.S` 是 NASM/GAS 语法，zig-cc 汇编；每个 ISR 必须正确保存/恢复寄存器
  并 `iretq`。
- `bootsect.S` / `stage2.S` 是 16 位实模式 / 32 位保护模式代码，**不能**
  引入任何 C 运行时假设。
- 改 ISR 时务必保证 `CONSOLE.lock_irq()` → `unlock_irq()` 成对、
  `cli` / `sti` 不会泄漏到 syscall 返回之后。

### Python（tools/）

- 只用标准库，能在构建主机和 Live 镜像内跑。
- 目标 Python 3.9+。

### Shell（build.sh、run-vbox.sh）

- `#!/bin/sh`，禁止 bashisms（数组、`[[ ]]`、`${var^^}` 等）。
- `set -eu` 在文件头。

## 添加新的 ring-3 程序

1. 在 `userspace/` 下新建目录，仿照 `userspace/hello/`：
   - `Cargo.toml`（`libe1` 依赖、`x86_64-unknown-none` target）
   - `.cargo/config.toml`（linker script 指向 `linker.ld`）
   - `linker.ld`（linkscript）
   - `src/main.rs`
2. 在 `build.sh` 的 `[4/6] userspace & apps` 部分加一条 `cargo build`。
3. 在 `tools/mkimage.py` 里把产物加进 `rootfs/usr/bin/`。
4. 确保 ELF 只有一个 LOAD segment、虚拟地址在 `0x7fffffxx`（libe1 假设
   启动在高地址栈）。

## 添加内核模块

1. 在 `kernel/src/` 新建 `mymodule.rs`。
2. 在 `main.rs` 里 `pub mod mymodule;` 并调用 `init_*()`。
3. 不要在新模块里引入 `std` 或使用全局可变状态（优先用 `Mutex` +
   `CriticalSection` 或 `CONSOLE.lock_irq()` 做并发保护）。

## 测试你的改动

提 PR 前请跑完：

1. **构建**：`BRAND=desk EDITION=workstation ./build.sh` +
   `BRAND=desk EDITION=server ./build.sh`。
2. **启动**：`./run-vbox.sh` 或手动转 VDI 挂载 VBox。
3. **基础功能**：USH 输入 hello → 两个 hello 都能执行 → ush 不再卡死
   （说明 CR3 / scheduler 路径没被破坏）。
4. **framebuffer 改动**：workstation 版启动后 VBox GUI 窗口内能看到启动
   日志、ush prompt、hello 输出（VBox `screenshotpng` 可做像素验证）。
5. **scheduler 改动**：跑 `ps` 确认 idle / ush 状态；多跑几个 hello 后
   再 `ps` 确认不会出现僵尸进程。
6. **e1FSS 改动**：`ls` / `cat` 后重启再跑，确认文件仍然在。

串口输出（headless 模式下 `nc localhost 4561`）是最可信的调试源。
如果改了 console 相关代码，务必看 `console.rs` 里 `CONSOLE.lock_irq()`
的位置——framebuffer 写入必须在 IRQ 关闭期间完成，且不能嵌套 lock。

## Commit 规范

项目用 Conventional Commits（英文或中文皆可）：

```
feat: 添加 ring3 gui 程序
fix: fb_write_pixel 在 user pml4 下 #PF
refactor: sched.rs snapshot 逻辑简化
docs: README 增加架构图
chore: 更新 zig 到 0.13.0
```

原因写在 body 里——尤其是和 ISR、CR3、framebuffer 映射相关的改动。

## Pull Request 流程

1. Fork 仓库，从 `main` 拉分支。
2. 小而聚焦的 PR，说明你测了哪些东西。
3. **不要** commit 构建产物：`build/`、`target/`、`*.img`、`*.vdi` 都在
   `.gitignore` 里。
4. **不要** commit token / cookie / 本地 keychain dump。
5. 维护者可能要求修改——rebase 不要 merge main 到你的分支。

## 提 Issue 时请附

- 宿主机 OS 和版本
- 运行的确切命令
- 构建品牌 / 变体（`BRAND=desk EDITION=workstation` 还是别的）
- 完整的串口或 VBox 日志
- 如果是引导或 framebuffer bug，附上 `.img` 或 VDI 方便复现

## 许可证

本仓库所有自研代码采用 MIT 许可证。提交即代表你同意以相同许可证
发布你的贡献。
