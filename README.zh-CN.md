# DesKOS / tAbLeOS

**简体中文** | [English](README.md)

一个从零开始的 x86_64 小型实验操作系统。MBR 引导扇区 → stage2 → Rust 内核 → ring3 用户态。

- **DesKOS**（`BRAND=desk`）—— 开源（MIT），本仓库发布的公开版本
- **tAbLeOS**（`BRAND=table`）—— 闭源，专有变体

两个品牌共用同一套源码、同一种磁盘镜像布局；split 完全是许可证 / 发行层面的划分。

## 特性一览

- **无 Linux、无 POSIX、无 libc** —— 内核 100% Rust（`no_std`，启动阶段无 heap），仅在
  ISR 入口层和两段引导扇区（MBR + stage2）使用少量 x86_64 汇编。ring-3 程序链接
  一个极简 `libe1`，通过内联 `syscall` 指令访问内核。
- **基于 PIT 的协作式调度器** —— `sched.rs` 利用 PIT IRQ 0 驱动多任务；`ps` /
  `memmap` 可查看进程状态和内存布局。
- **e1FSS 文件系统** —— 简单的单分区平面文件系统；`ush` shell 下 `ls` / `cat`
  可用，用户态程序通过 syscall 打开/读/写文件。元数据放在固定头部、数据块
  只追加，重启安全。
- **VGA 文本 + framebuffer 字形控制台** —— 三路输出：UART、VGA 文本模式、
  VBE framebuffer（8×8 字形 2×2 放大到 16×16 字符单元）。console 写操作
  用 `cli` 关中断保证 IRQ 安全。
- **Server / Workstation 双分支**
  - `server`：UART 纯串口头less，最小体积（适合 VBox tcpserver 或 CI）
  - `workstation`：启用 framebuffer 字形控制台，GUI 窗口内可见输出
- **OpenRC 风格的 init** —— `openrc.rs` 解析 `/etc/init.d/` 脚本树，按序启动服务
  并做进程监督。
- **100 个一次性激活密钥** —— 公测试用，每个 release 随 `activation-keys.txt` 发布。

## 快速开始

### 下载预构建镜像

到 [GitHub Releases](../../releases) 下载：

| 文件 | 固件 | 说明 |
|---|---|---|
| `deskos-workstation-0.1-amd64.img` | BIOS | Framebuffer + VGA 控制台（8 MB） |
| `deskos-server-0.1-amd64.img`      | BIOS | UART 头less（8 MB） |

### VirtualBox 运行

```sh
# 一键脚本（构建 + 转换 + headless 启动，UART 监听 localhost:4561）
./run-vbox.sh

# 或手动：VirtualBox → 新建 → Other/Unknown 64-bit → 把 .img 转成 VDI 挂到 IDE 0
VBoxManage convertfromraw deskos-workstation-0.1-amd64.img deskos.vdi --format VDI
```

启动后直接进入 ring-3 的 `ush` shell，试试：

```
hello          # 运行 ring-3 "hello from ring3!" 程序
ps             # 列出进程
memmap         # 查看内存映射
help           # 内建命令
```

### 真机 / U 盘

```sh
sudo dd if=deskos-workstation-0.1-amd64.img of=/dev/sdX bs=4M conv=fsync
```

## 从源码构建

需要 macOS 或 Linux + `python3`、`cargo`（Rust nightly，目标 `x86_64-unknown-none`）、
`zig`（仅用于引导扇区汇编，首次自动下载）。

```sh
# DesKOS（开源品牌）
BRAND=desk EDITION=workstation ./build.sh     # GUI 工作站版
BRAND=desk EDITION=server      ./build.sh     # 头less 服务器版

# 两个品牌 × 两个变体（共 4 个镜像）
./build.sh
```

产物在 `build/`：
- `bootsect.bin`、`stage2.bin` —— MBR + 二阶段引导
- `kernel.bin` —— Rust 内核
- `deskos-{server,workstation}-0.1-amd64.img` —— 可引导磁盘镜像

## 架构一览

```
┌──────────────────────────────────────────────────┐
│ ring3  ush / hello / e1wm / e1dm / e1apfs / exaltop  │
│        （ELF，链接 libe1；内联 syscall 指令）         │
├──────────────────────────────────────────────────┤
│ syscall → syscall.rs → {vmm, sched, e1fss, ...}    │
├──────────────────────────────────────────────────┤
│ kernel  main.rs · vmm.rs · sched.rs · isr.S        │
│         idt.rs · gdt.rs · pmm.rs · heap.rs         │
│         console.rs · keyboard.rs · mouse.rs        │
│         pit.rs · pic.rs · ata.rs                    │
│         elf.rs · brand.rs · openrc.rs · e1fss.rs    │
├──────────────────────────────────────────────────┤
│ stage2  （zig-cc 汇编 → 二进制，把 kernel.bin 搬到  │
│         高规范地址，设置 PML4，跳到 Rust main）       │
├──────────────────────────────────────────────────┤
│ MBR bootsect （512 字节，int 13 加载 stage2）       │
└──────────────────────────────────────────────────┘
```

## 源码结构

```
boot/           bootsect.S + stage2.S（MBR + 二阶段引导）
kernel/         Rust 内核 — vmm.rs, sched.rs, syscall.rs, console.rs 等
userspace/      ring-3 程序：ush、hello、libe1
apps/           额外的 ring-3 应用（e1wm、e1dm、e1apfs、e1vfsx、exaltop）
tools/          mke1fss.py、mkrit.py、mkkeys.py、mkfont_rs.py、mkimage.py
build.sh        一键构建：引导扇区 → 内核 → 用户态 → 磁盘镜像
run-vbox.sh    创建并启动 VirtualBox 虚拟机（headless 或 GUI）
sign_github.py  M9 发布工具 —— 创建 GitHub 仓库 + release + 上传产物
LICENSE         双品牌许可（tAbLeOS 闭源 / DesKOS MIT）
```

## 许可证

双品牌 —— 详见 [LICENSE](LICENSE)：

- **tAbLeOS**（`BRAND=table`）：闭源，保留所有权利
- **DesKOS**（`BRAND=desk`）：MIT License，本仓库发布的版本

本仓库的构建结果是 **DesKOS** 变体。
