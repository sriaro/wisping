# Wisp

[English](README.md) | 简体中文

<details>
<summary><b>English (click to expand)</b></summary>

A portable, single-exe Windows cleaner with an English and Chinese interface.

- Cleans junk files, finds stale registry entries, manages startup items and installed programs, maps disk usage, and offers a few Windows tweaks.
- Nothing is deleted until you confirm, registry fixes are backed up, and every optional recording feature is off by default.
- Zero idle cost: no background service, and no CPU use while you are not using it.
- Download: get the zip from [Releases](https://github.com/sriaro/wisp/releases), unzip it and run `Wisp.exe`. No installer.

Full English text: [README.md](README.md).

</details>

一款绿色便携的 Windows 清理工具，只有一个 exe，界面支持中文和英文。用 Rust 和 Slint（femtovg 渲染器）编写，液态玻璃风格。

- 清理垃圾文件，查找失效的注册表项，管理启动项和已安装的程序，分析磁盘占用，还提供一些常用的 Windows 小调整。
- 安全第一：没有你的确认，什么都不会删；修复注册表之前会先备份；所有可选的记录功能（日志、历史记录、记住设置、还原点、磁盘空间不足提醒）默认都是关闭的。
- 空闲时零开销：没有后台服务，不用的时候也不占 CPU。
- 自动适配处理器（`src/engine/cpu.rs`）：扫描线程数跟随逻辑处理器数量，哈希计算使用高性能核心，需要等待的扫描以略低于正常的优先级运行并使用高 QoS，后台哈希使用 EcoQoS。线程不会绑定到固定核心，核心调度交给 Windows 以及 AMD、Intel 的驱动。

## 下载

到 [Releases](https://github.com/sriaro/wisp/releases) 页面下载 `Wisp-x.y.z-win-x64.zip`，解压到任意位置，双击运行 `Wisp.exe` 即可，不需要安装。

exe 没有代码签名，第一次运行时 Windows SmartScreen 可能会弹出提示，这是正常的。你可以用发布页上的 SHA-256 校验下载的文件。

## 构建

需要 Windows 10/11 和 Rust 1.99 或更高版本。

```
cargo build --release
```

生成的 `target/release/Wisp.exe` 是一个静态链接的单文件程序。

如果要发布 exe，请改用 `tools/build-release.ps1` 构建。它会把项目、Cargo 和 Rust 的目录映射成中性名称，这样 exe 里就不会带有本机路径和用户名。

## 测试

```
target\release\Wisp.exe --selftest result.txt
```

`result.txt` 的最后一行必须是 `ALL PASS`。运行前请先关闭正在运行的 Wisp，因为其中一项检查需要占用单实例锁。

`Wisp.exe --render <dir>` 会为每个页面生成预览截图。`PC_PREVIEW_ROOT` 用来指定磁盘页要扫描的文件夹，`PC_PREVIEW_ONLY` 只保留名称中包含指定文字的场景（例如 `light-disk`），`PC_PREVIEW_TICK` 设置每个场景等待的毫秒数。

## 目录结构

| 路径 | 内容 |
| --- | --- |
| `src/engine/` | 清理、注册表、启动项、程序、备份、历史、设置，不包含界面代码 |
| `src/pages/` | 为 Slint 数据模型提供数据的页面逻辑 |
| `src/app.rs`、`src/winui.rs` | 应用初始化、扫描/清理/修复流程、窗口处理、Win32 封装 |
| `src/views.rs` | 把引擎数据构建成 Slint 列表模型，并原地更新 |
| `src/motion.rs` | 页面切换、主题渐变、语言切换的淡入淡出动画 |
| `src/i18n.rs`、`src/i18n_zh.rs` | 英文和中文文本表（每个键两边都必须有） |
| `src/selftest.rs`、`src/preview.rs` | `--selftest` 自检和 `--render <dir>` 预览截图 |
| `ui/` | Slint 界面：`theme.slint`、`widgets.slint`、`motion.slint`、`main.slint`、`pages/` |
| `ui/icons/` | 由 `tools/gen_icons.py` 生成的 SVG 图标 |
| `ui/art/` | 由 `tools/make-assets` 生成的圆环和阴影图片 |
| `assets/` | 应用图标和清单文件 |
| `third_party/femtovg/`、`third_party/i-slint-renderer-femtovg/` | 修改过的 femtovg 画布和 Slint femtovg 渲染器（见 `Cargo.toml` 中的 `[patch.crates-io]` 和 `third_party/README.md`） |
| `tools/` | 资源生成脚本和 `build-release.ps1` |

## 重新生成资源

```
python tools/gen_icons.py
cd tools/make-assets && cargo run --release -- ../../assets ../../ui/art
```

## 数据存放位置

Wisp 默认把备份、设置、历史记录和日志存放在 `Wisp.exe` 旁边的 `data` 文件夹里，你可以在“备份”页面把它移到别处。如果 exe 放在临时文件夹中（比如直接在压缩包里运行），数据会改存到 `%LOCALAPPDATA%\Wisp`，以免被临时文件清理掉。`data` 文件夹不属于源码，不会提交到仓库。

## 许可证

Wisp 是自由软件，采用 GNU 通用公共许可证第 3 版（仅限 GPL-3.0），详见 `LICENSE`。版权所有 (C) 2026 sriaro。根据该许可证第 7(b) 条，复制和修改后的版本必须保留作者署名“sriaro”，具体措辞见 `NOTICE`。Wisp 基于 Slint 开发，采用的是 Slint 的 GPL-3.0 授权方式。

Wisp 不提供任何担保（见许可证第 15 和 16 条）。它只会在你确认之后才删除文件、修改注册表，但重要的数据请务必提前备份。

编译进 exe 的所有依赖库及其许可证都列在 `THIRD-PARTY.md` 中。

第三方部分：

- `third_party/femtovg` 是 femtovg 0.27.0（MIT 或 Apache-2.0），带有本地修改，许可证文件保留在该文件夹中。
- `third_party/i-slint-renderer-femtovg` 是 Slint 的 femtovg 渲染器 1.18.1（GPL-3.0-only，或 Slint 免版税许可，或商业许可），带有本地修改，许可证文件在 `third_party/i-slint-renderer-femtovg/LICENSES` 中。
- `ui/icons` 里的图标来自 Lucide（ISC 许可证）。下面是它的原文声明，保留英文：

  Copyright (c) for portions of Lucide are held by Cole Bemis 2013-2022 as part of Feather (MIT licence). All other copyright (c) for Lucide are held by Lucide Contributors 2022. Permission to use, copy, modify, and/or distribute this software for any purpose with or without fee is hereby granted, provided that the above copyright notice and this permission notice appear in all copies.
