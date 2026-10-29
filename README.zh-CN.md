<p align="center">
  <picture>
    <source media="(prefers-color-scheme: dark)" srcset="docs/header-dark.png">
    <img alt="Wisping" src="docs/header-light.png" width="571">
  </picture>
</p>

<h3 align="center">稳妥的 Windows 系统维护工具。</h3>

<p align="center">
  <a href="#功能">功能</a> ·
  <a href="#下载">下载</a> ·
  <a href="#构建">构建</a> ·
  <a href="#文件和设置">文件和设置</a> ·
  <a href="#许可证">许可证</a> ·
  <a href="README.md">English</a>
</p>

Wisping 是一款适用于 Windows 10 和 11 的便携式系统维护工具。Wisping 可以清理无用文件，找出已卸载程序留下的残留，并管理启动程序、服务和计划任务。此外还提供磁盘占用分析和程序卸载功能。各项功能的详细说明见下文。

## 功能

- **清理**：临时文件、缓存、日志、崩溃转储、更新下载文件、失效的快捷方式、浏览器缓存、资源管理器和 Office 的历史记录、回收站，以及桌面应用、游戏启动器、开发工具和 AI 工具的缓存。每一项都会说明删除的内容和风险程度。会导致退出登录或清除历史记录的项目默认不勾选；最近 24 小时内的文件默认保留，可以关闭此保护。
- **定时清理**：`Wisping.exe --clean` 不打开窗口直接清理：开启“记住设置”后清理上次在窗口中选择的项目，否则清理默认项目。报告、日志和历史记录与在窗口中清理时相同，可以交给 Windows 任务计划程序定时运行。
- **注册表和残留**：查找指向已不存在目标的共享 DLL、应用路径、卸载项、MUI 缓存、COM 类、文件扩展名、“打开方式”列表和托盘图标，以及已删除程序留下的文件夹。注册表修复会先写入 .reg 备份，文件夹会移到回收站。
- **启动项**：登录时启动的应用、计划任务、右键菜单项和服务，每项都有开关，并说明关闭后是否安全。Windows 自带项目受保护，默认隐藏。
- **程序**：列出已安装的程序及其大小和安装日期。卸载时运行程序自带的卸载程序。
- **磁盘占用**：以图形显示个人文件、应用数据或整个磁盘的空间分布，并列出最大的文件、重复的文件以及每个程序占用的空间。只做查看，不做任何改动。
- **工具**：提供关闭游戏栏、诊断数据、错误报告和设备制造商应用下载的开关，以及重置通知列表和托盘图标的功能。修改前都会备份相关注册表项。
- **备份**：每次修改注册表前都会先保存备份，可以在“备份”页恢复。清理前创建还原点、日志和清理历史可以按需开启，默认关闭。
- **外观**：深色或浅色主题，英文或中文界面，未手动选择时跟随 Windows 设置。

确认之前不会删除任何内容。部分系统项目需要管理员权限，侧边栏有一个按钮，可以以管理员身份重新启动 Wisping。

## 下载

到 [Releases 页面](https://github.com/sriaro/wisping/releases)下载 `Wisping-<version>-windows-x64.zip`，解压到任意位置，运行 `Wisping.exe`。无需安装。程序没有代码签名，首次运行时 SmartScreen 可能会发出警告。

## 构建

需要 Windows 10 或 11、Rust 1.99 或更高版本，以及 MSVC 生成工具。

```
cargo build --release --locked
```

生成的程序位于 `target/release/Wisping.exe`。`tools\build-release.ps1` 构建用于发布的同一程序，其中不包含本机路径。`Wisping.exe --selftest result.txt` 运行内置自检（请先关闭其他正在运行的 Wisping），结果的最后一行应为 `ALL PASS`。`ui/` 中的图标和插图以及 `docs/` 中的标题图由 `tools/` 中的脚本生成。

## 文件和设置

- 备份、设置、清理报告、日志和历史记录保存在 `Wisping.exe` 旁边的 `data` 文件夹中。如果该文件夹无法使用，例如 exe 位于临时文件夹、Windows、Program Files 或 OneDrive 目录下，或位于只读位置，Wisping 会改用 `%LOCALAPPDATA%\Wisping`。在“备份”页选择的文件夹记录在 exe 旁边的 `wisping-location.txt` 中；如果 exe 所在文件夹是只读的，则记录在 `%LOCALAPPDATA%\Wisping\location.txt` 中。
- 以管理员身份运行时，Wisping 会在 `HKLM\SOFTWARE\Wisping` 下保存少量记录：已关闭服务的原始启动类型、在“工具”页设置的策略的原始值，以及以管理员权限创建的注册表备份的完整性记录。这类备份只有在未被改动的情况下才能以管理员权限恢复。
- 开启还原点后，每次清理或修复前都会请求 Windows 系统还原创建一个还原点；Windows 每天只允许创建一个。

Wisping 没有后台进程，不开启日志功能时不记录任何日志，也不进行任何网络连接。

## 许可证

Copyright (C) 2026 sriaro.

Wisping 是自由软件，依据 GNU 通用公共许可证第 3 版（仅限该版本，GPL-3.0-only）发布。详见 [LICENSE](LICENSE)。

Wisping 另附一项依据 GPL 第 7(b) 条的附加条款：复制、修改或分发 Wisping 的人，必须在源代码、文档和法律声明中保留版权声明和作者署名 "sriaro"，并在修改版本中标明已修改。详见 [NOTICE](NOTICE)。

Wisping 使用 Slint 构建。Slint 提供 GPL-3.0-only、免版税许可证和商业许可证三种授权方式，Wisping 按 GPL-3.0-only 使用，因此整个程序以 GPL 发布。femtovg 画布（MIT 或 Apache-2.0）和 Slint 的 femtovg 渲染器由 `third_party/` 中的修改版构建（见 [third_party/README.md](third_party/README.md)），并保留了各自的许可证文件。`ui/icons` 中的图标取自 Lucide（ISC）。其余依赖均采用 MIT、Apache-2.0、BSD、Zlib、ISC 或 CC0 等宽松许可证。所有链接进程序的软件包的声明见 [THIRD-PARTY-NOTICES.txt](THIRD-PARTY-NOTICES.txt)。

本仓库包含该程序的完整源代码。
