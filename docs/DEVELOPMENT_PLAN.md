# xiaorui_FTP — 系统架构与开发文档

本文档面向参与 **xiaorui_FTP** 开发、维护与二次扩展的工程师，完整说明系统分层架构、核心协议与交互式终端实现、IPC 通信契约、数据持久化与安全模型以及全平台 x64 构建发布流程。

---

## 1. 产品定位与设计原则

1. **全平台原生 x64 构架**：基于 Rust + Tauri v2，面向 `x86_64-pc-windows-msvc`、`x86_64-apple-darwin`、`x86_64-unknown-linux-gnu` 原生编译打包，Windows x64 NSIS 安装包约 **3.7 MB**，运行时内存远低于 Electron 客户端。
2. **纯 Rust 现代密码学栈**：零外部 C/OpenSSL 依赖，传输层全面采用 `rustls`（TLS 1.2/1.3）与 `russh`（Curve25519 / Ed25519 / RSA-SHA2-256 / AES-GCM / ChaCha20-Poly1305），保证跨平台一致行为与可复现构建。
3. **现代桌面交互与零崩溃韧性**：
   - 双行防挤压文件面板工具栏（独立全宽路径面包屑、带容量进度条的自定义磁盘切换浮层、列头排序、语义图标、右键上下文菜单）；
   - 完整 Xshell 风格交互式 SSH 终端（底部停靠 / 工作区全屏 / 独立原生窗口三形态 + 多标签 + 快捷命令 + 与 SFTP 双向联动）；
   - 前端全局 `ErrorBoundary` + 后端全协议连接超时、原子落盘与防 panic 封装。

---

## 2. 总体分层架构

```text
┌────────────────────────────────────────────────────────────────────────────┐
│                   Frontend (React 19 + TypeScript + Vite)                  │
│  ErrorBoundary · TopBar (Ctrl+K) · SiteSidebar (在线态/备份/TOFU主机管理)   │
│  FilePane (双行工具栏/面包屑/磁盘浮层/右键联动) · TransferDrawer · StatusBar│
│  SiteModal (分段协议卡) · DialogModal · EditorModal · KnownHostsModal      │
│  TerminalDrawer (xterm.js 多标签/搜索/缩放/快捷命令) · StandaloneTerminalApp│
└──────────────────────────────┬─────────────────────────────────────────────┘
                               │ Tauri v2 Typed IPC Commands & Events
                               │ (transfer://update · pty://data · pty://exit)
┌──────────────────────────────▼─────────────────────────────────────────────┐
│                        Rust Core Backend (x86_64)                          │
│  ┌───────────────────┐  ┌────────────────────┐  ┌────────────────────────┐ │
│  │  AppState / IPC   │  │  TransferManager   │  │ Storage / KnownHosts   │ │
│  │ 会话/PTY/托盘/窗口 │  │ Semaphore 3 并发   │  │ sites/known_hosts/     │ │
│  │                   │  │ 流式节流 & 取消广播 │  │ settings + OS Keyring  │ │
│  └─────────┬─────────┘  └─────────┬──────────┘  └────────────────────────┘ │
│            │ 统一异步远程文件系统抽象 (`RemoteFs` Trait)                    │
│  ┌─────────▼─────────────────────────────────────────────────────────────┐ │
│  │ Protocol Adapters (`src-tauri/src/protocols/`)                        │ │
│  │  • sftp.rs   — russh + russh-sftp (Agent/ProxyJump/TOFU/PTY/Metrics) │ │
│  │  • ftp.rs    — suppaftp + rustls (显/隐式 FTPS/FTP + 递归删目录)      │ │
│  │  • s3.rs     — reqwest + AWS SigV4 (流式上传/分页列举/分页删除/去重)   │ │
│  │  • webdav.rs — reqwest + RFC 4918 XML (流式上传/中文路径/Bearer认证)   │ │
│  └───────────────────────────────────────────────────────────────────────┘ │
│  ┌───────────────────────────────────────────────────────────────────────┐ │
│  │ Local FS (`local_fs.rs`) — 跨平台本地目录与 Windows 盘符枚举           │ │
│  └───────────────────────────────────────────────────────────────────────┘ │
└────────────────────────────────────────────────────────────────────────────┘
```

---

## 3. 代码目录结构

```text
xiaorui_FTP/
├── .github/workflows/
│   └── build-x64.yml                # GitHub Actions 三平台 x64 门禁（tsc/fmt/clippy/test/build）
├── .gitattributes                   # 跨平台换行符与二进制资源属性声明
├── docs/
│   ├── DEVELOPMENT_PLAN.md          # 本架构与开发文档
│   └── COMPETITIVE_COMPARISON.md    # 与 FlashFXP/SmartFTP/CuteFTP/Wing FTP Server 对照报告
├── scripts/
│   ├── build-release.ps1            # Windows x64 一键验证 + NSIS/MSI Release 打包脚本
│   └── build-release.sh             # macOS/Linux x64 一键验证 + Release 打包脚本
├── index.html                       # 前端 HTML 入口
├── package.json                     # 前端依赖与脚本（含 @xterm/xterm, addon-fit, addon-search）
├── tsconfig.json / vite.config.ts   # TypeScript 严格配置与 Vite 分包（xterm / icons 独立 chunk）
├── README.md                        # 用户使用手册、快捷键速查与 FAQ
├── src/                             # React + TypeScript 前端源码
│   ├── main.tsx                     # 入口路由（自动区分主工作台与 ?mode=terminal 独立终端窗口）
│   ├── App.tsx                      # 主工作台状态协调、拖拽分栏、系统拖放、硬件监控与状态栏
│   ├── styles/app.css               # 现代桌面设计系统（Light / Dark、自定义滚动条与动效）
│   ├── types/index.ts               # 前后端共享数据类型与格式化工具
│   ├── api/tauri.ts                 # Tauri IPC 命令与事件类型安全封装
│   └── components/
│       ├── ErrorBoundary.tsx        # 全局 React 渲染错误边界与恢复页
│       ├── TopBar.tsx               # 命令栏式快速直连与全局视图开关
│       ├── SiteSidebar.tsx          # 左侧站点分组收藏夹（在线绿点/跳板标记/备份/主机入口）
│       ├── SiteModal.tsx            # 分段协议选择器 + 前置表单校验 + 跳板机与 ssh-agent 检测
│       ├── DialogModal.tsx          # 跨平台统一确认/输入模态框（零 window.prompt/confirm）
│       ├── KnownHostsModal.tsx      # SSH 已知主机公钥指纹（TOFU）管理弹窗
│       ├── FilePane.tsx             # 通用文件面板（双行工具栏/面包屑/磁盘浮层/排序/右键菜单）
│       ├── TransferDrawer.tsx       # 底部并发传输队列抽屉
│       ├── EditorModal.tsx          # 远程文本在线轻编辑器（Ctrl+S 回传）
│       ├── TerminalDrawer.tsx       # Xshell 风格多标签 xterm.js 交互终端工作台
│       └── StandaloneTerminalApp.tsx# 独立原生操作系统 SSH 终端窗口应用
└── src-tauri/                       # Rust 后端与 Tauri v2 配置
    ├── Cargo.toml                   # 依赖（含 tauri tray-icon、zeroize、tokio-util、pageant）
    ├── tauri.conf.json              # 窗口配置、严格 CSP 与全平台打包目标
    ├── build.rs                     # Tauri 构建脚本
    ├── capabilities/default.json    # Tauri v2 安全能力声明（覆盖 main 与 term-* 窗口）
    ├── icons/                       # 全平台应用图标集合
    └── src/
        ├── main.rs                  # 桌面可执行程序入口
        ├── lib.rs                   # Tauri Builder、系统托盘、关闭最小化钩子与命令注册
        ├── error.rs                 # 统一中文错误枚举 `AppError`
        ├── models.rs                # 领域模型、路径规范化、URI 解析、硬件指标与 PTY 事件
        ├── storage.rs               # `sites.json` 原子持久化与 OS Keyring 凭据读写
        ├── known_hosts.rs           # `known_hosts.json` 独立 TOFU 主机指纹仓库
        ├── local_fs.rs              # 跨平台本地文件系统封装
        ├── transfer.rs              # Semaphore 并发传输队列、取消事件与进度竞态防护
        ├── commands.rs              # 全部 Tauri IPC 命令、PTY 生命周期与独立窗口管理
        └── protocols/
            ├── mod.rs               # `RemoteFs` Trait、`PtyHandle` 与协议连接工厂
            ├── sftp.rs              # SFTP / 安全 Pageant / ProxyJump / TOFU / PTY / 硬件探测
            ├── ftp.rs               # FTP / FTPS (TLS 1.2/1.3) 与递归目录删除
            ├── s3.rs                # S3 SigV4 流式上传、分页列举与分页删除
            └── webdav.rs            # WebDAV RFC 4918 流式传输与中文路径解析
```

---

## 4. 核心模块实现细节

### 4.1 统一远程文件系统接口 (`RemoteFs`)
定义于 [src-tauri/src/protocols/mod.rs](../src-tauri/src/protocols/mod.rs)，统一抽象 4 大协议，所有网络连接均强制 **20 秒超时保护**：

- `list_dir(path)`：返回按「目录优先、名称不区分大小写升序」排序的 `Vec<FileEntry>`；SFTP 自动解析软链接目标是否为目录（保留 `is_symlink=true` 防递归死循环），并适配 Windows OpenSSH 盘符根路径（`/D:` → `/D:/`）；S3 自动分页并按路径去重。
- `mkdir(path)` / `remove(path, is_dir)` / `rename(from, to)`：四大协议均完整支持非空目录递归删除（含 FTP 迭代递归删除与 S3 分页删除）。
- `chmod(path, mode)`：SFTP 与 FTP/FTPS 支持，S3/WebDAV 优雅降级返回 `AppError::Unsupported`。
- `read_bytes(path, max_bytes)` / `write_bytes(path, data)`：支持 2 MB 以内在线文本查看与修改回传。
- `download_file` / `upload_file`：全协议采用 64 KB 流式分块读写，支持实时速率回调、中途取消与取消下载后自动清理残缺本地文件。
- `exec_shell(command, cwd)` / `server_metrics()` / `open_pty(cols, rows)`：SFTP/SSH 专属能力，支持单次命令执行、硬件规格与多磁盘采样以及全双工交互式登录 Shell。

### 4.2 安全 SSH `ProxyJump`、本地 `ssh-agent` 与交互式 PTY
位于 [src-tauri/src/protocols/sftp.rs](../src-tauri/src/protocols/sftp.rs) 与 [src-tauri/src/commands.rs](../src-tauri/src/commands.rs)：
- **安全本地 Agent 探测**：Windows 优先连接 `\\.\pipe\openssh-ssh-agent`；回退 Pageant 时绕过上游 `pageant 0.0.1` 的 `.unwrap()` panic 缺陷，采用自定义的 `connect_safe_pageant()`（先调用 `is_pageant_running()`，通信置于 `spawn_blocking` 且零 panic）；macOS/Linux 连接 `SSH_AUTH_SOCK`。
- **SSH 跳板机（ProxyJump）**：先连接认证跳板机，通过 `channel_open_direct_tcpip` 建立加密转发流，再以 `client::connect_stream` 握手目标主机（跳板机与目标机均独立执行 TOFU 公钥指纹校验）。
- **交互式 PTY 与独立窗口**：`open_pty` 仅短暂持有 SSH 会话锁申请新通道（不阻塞文件浏览与硬件探测），请求 `xterm-256color` 伪终端并调用 `request_shell(true)` 启动远端默认登录 Shell（`zsh` / `bash` / `fish` / PowerShell）。输出字节流经 Base64 封装通过 `pty://data` 推送至 `xterm.js`，避免多字节中文跨包乱码；断开会话时自动回收所有关联 PTY 通道。
- **远端硬件与多磁盘采集**：`server_metrics` 优先执行轻量 POSIX 探测脚本（`/etc/os-release`、`/proc/stat`、`/proc/meminfo`、`df -kP -T`），自动过滤 `/boot`、伪文件系统、容器挂载及同块设备重复 bind 挂载；若远端为 Windows OpenSSH 则自动回退 PowerShell CIM 采集（`C:\`、`D:\` 等物理磁盘）。

### 4.3 系统托盘与独立配置持久化（零数据丢失保障）
为确保版本迭代升级时**老用户数据绝不重置或丢失**，所有持久化文件彼此独立且保持稳定标识：
- **应用标识与凭据库服务名永久不变**：`identifier = "com.xiaorui.ftp"`、`KEYRING_SERVICE = "com.xiaorui.ftp"`；
- **`sites.json`**：站点元数据（不含明文密码），原子写入并向后兼容缺省新字段的旧版文件；
- **`known_hosts.json`**：SSH 主机 SHA-256 指纹（TOFU），独立文件原子写入；
- **`settings.json`**：轻量界面偏好（如 `minimizeToTray`，默认 `true`），由 `AppState` 内 `AtomicBool` 缓存供 `WindowEvent::CloseRequested` 无锁读取。
- **内存清零与 CSP**：`SiteSecrets` 实现 `Drop` 通过 `zeroize` 清零内存；`tauri.conf.json` 启用严格 CSP；`capabilities/default.json` 仅授权 `["main", "term-*"]` 窗口。

---

## 5. Tauri IPC 命令清单

| 分类 | 命令名称 | 说明 |
| :--- | :--- | :--- |
| **站点与凭据** | `list_sites`, `save_site`, `delete_site`, `export_sites_to_file`, `import_sites_from_file`, `check_ssh_agent_status`, `list_known_hosts`, `remove_known_host` | 站点 CRUD、JSON 备份恢复、本地 Agent 检测、TOFU 指纹管理 |
| **应用偏好** | `get_app_settings`, `set_minimize_to_tray`, `focus_main_window`, `save_text_file` | 关闭最小化托盘偏好、窗口聚焦与日志导出 |
| **会话管理** | `connect_saved_site`, `connect_adhoc`, `quick_connect_uri`, `disconnect_session`, `list_sessions` | 建立/断开多协议远程会话（断开时同步关闭关联 PTY） |
| **远程文件** | `remote_list_dir`, `remote_mkdir`, `remote_delete`, `remote_rename`, `remote_chmod`, `remote_read_text`, `remote_write_text` | 统一远程文件浏览、操作与在线轻编辑 |
| **本地文件** | `local_home_dir`, `local_list_roots`, `local_list_dir`, `local_mkdir`, `local_delete`, `local_rename` | 跨平台本地目录与 Windows 盘符枚举 |
| **传输队列** | `enqueue_upload`, `enqueue_upload_paths`, `enqueue_download`, `list_transfers`, `cancel_transfer`, `clear_finished_transfers` | 3 并发流式队列、防路径穿越递归下载（事件：`transfer://update`） |
| **终端与监测** | `ssh_exec_command`, `remote_server_metrics`, `ssh_pty_open`, `ssh_pty_write`, `ssh_pty_resize`, `ssh_pty_close`, `open_terminal_window` | 交互式默认 Shell PTY、独立终端窗口与硬件/多磁盘采集（事件：`pty://data`, `pty://exit`） |

---

## 6. 质量门禁、测试与发布流程

1. **静态门禁**：
   - 前端：`npx tsc --noEmit`（严格模式，零 `@ts-ignore` / `as any` / `console.log` / `window.prompt`）与 `npm run build`（`xterm`、`icons` 独立分包无告警）；
   - 后端：`cargo fmt -- --check` 与 `cargo clippy --all-targets -- -D warnings` 零警告。
2. **单元测试覆盖（17 项 Rust 单测）**：
   - 路径规范化与多协议/跳板机 URI 解析（`models.rs`）；
   - 本地递归上传扫描计划（`commands.rs`）；
   - 旧版 `sites.json` 兼容、导出脱敏与导入往返（`storage.rs`）；
   - SSH TOFU 首次信任、匹配校验、篡改阻断与移除（`known_hosts.rs`）；
   - 本地文件生命周期与排序（`local_fs.rs`）；
   - 传输队列取消与历史清理（`transfer.rs`）；
   - Windows/Unix 本地 SSH Agent 无崩溃检测 + Linux/Windows 多磁盘与硬件指标解析（`protocols/sftp.rs`）；
   - S3 分页 XML 与错误 XML 解析（`protocols/s3.rs`）；
   - WebDAV `PROPFIND` XML、中文编码基础路径与状态码校验（`protocols/webdav.rs`）。
3. **发布打包**：
   - Windows x64：`npm run release:win64`（调用 `scripts/build-release.ps1` 执行 Clippy + 单测 + 前端构建 + `npx tauri build` 生成 NSIS/MSI 与 SHA-256）；
   - macOS / Linux x64：`bash scripts/build-release.sh`；
   - GitHub Actions CI：[.github/workflows/build-x64.yml](../.github/workflows/build-x64.yml) 覆盖 `windows-latest`、`macos-13`、`ubuntu-22.04` 全量检查与打包。
