# xiaorui_FTP — 系统架构与开发文档

本文档面向参与 **xiaorui_FTP** 开发、维护与二次扩展的开发者，详细说明系统分层架构、核心协议实现、IPC 通信契约、安全模型以及全平台 x64 构建发布流程。

---

## 1. 产品定位与设计原则

1. **全平台原生 x64 构架**：基于 Rust + Tauri v2，针对 `x86_64-pc-windows-msvc`、`x86_64-apple-darwin`、`x86_64-unknown-linux-gnu` 原生编译打包，安装包小巧（Windows x64 NSIS 安装包约 2.7 MB）、内存占用低。
2. **纯 Rust 现代密码学栈**：零外部 C/OpenSSL 编译依赖，全面使用 `rustls`（TLS 1.2/1.3）与 `russh`（Curve25519 / Ed25519 / RSA-SHA2-256 / AES-GCM / ChaCha20-Poly1305），保证跨平台可复现构建。
3. **极简、直觉化交互**：
   - 顶部集成快速直连地址栏与核心视图开关；
   - 左侧可折叠站点分组收藏夹（支持即时搜索与 JSON 备份导入导出）；
   - 主区域支持双栏（本地 ↔ 远程）与单栏（纯远程）无缝切换，支持多会话标签页；
   - 底部可折叠并发传输队列与 SFTP 内置命令终端，不干扰主文件列表视野。

---

## 2. 总体分层架构

```text
┌──────────────────────────────────────────────────────────────────────┐
│               Frontend (React 19 + TypeScript + Vite)                │
│  TopBar 快速直连 · SiteSidebar 站点管理 · FilePane 通用双栏/单栏     │
│  SiteModal 动态多协议表单 · TransferDrawer · EditorModal · Terminal  │
└─────────────────────────────┬────────────────────────────────────────┘
                              │ Tauri v2 Typed IPC Commands & Events
┌─────────────────────────────▼────────────────────────────────────────┐
│                    Rust Core Backend (x86_64)                        │
│  ┌──────────────────┐  ┌───────────────────┐  ┌───────────────────┐ │
│  │  AppState / IPC  │  │  TransferManager  │  │ SiteStore/Keyring │ │
│  │  多会话生命周期   │  │ Semaphore 3 并发  │  │ OS 原生凭据库托管  │ │
│  └────────┬─────────┘  └─────────┬─────────┘  └───────────────────┘ │
│           │ 统一异步远程文件系统抽象 (`RemoteFs` Trait)               │
│  ┌────────▼───────────────────────────────────────────────────────┐ │
│  │ Protocol Adapters (`src-tauri/src/protocols/`)                 │ │
│  │  • sftp.rs   — russh + russh-sftp (Agent / ProxyJump / Shell)  │ │
│  │  • ftp.rs    — suppaftp + rustls (显式/隐式 FTPS & FTP)        │ │
│  │  • s3.rs     — reqwest + AWS SigV4 (S3 / OSS / COS / MinIO/R2) │ │
│  │  • webdav.rs — reqwest + RFC 4918 XML (Nextcloud / Alist / NAS)│ │
│  └────────────────────────────────────────────────────────────────┘ │
│  ┌────────────────────────────────────────────────────────────────┐ │
│  │ Local FS (`local_fs.rs`) — 跨平台本地目录与 Windows 盘符枚举    │ │
│  └────────────────────────────────────────────────────────────────┘ │
└──────────────────────────────────────────────────────────────────────┘
```

---

## 3. 代码目录结构

```text
xiaorui_FTP/
├── .github/workflows/
│   └── build-x64.yml            # GitHub Actions 全平台 x64 自动测试与打包流水线
├── docs/
│   └── DEVELOPMENT_PLAN.md      # 本架构与开发文档
├── scripts/
│   └── build-release.ps1        # Windows x64 一键测试 + Release 打包脚本
├── index.html                   # 前端 HTML 入口
├── package.json                 # 前端脚本与依赖配置
├── tsconfig.json / vite.config.ts
├── README.md                    # 用户使用手册与常见问题
├── src/                         # React + TypeScript 前端源码
│   ├── main.tsx                 # React 挂载入口
│   ├── App.tsx                  # 主工作台状态协调与系统拖放监听
│   ├── styles/app.css           # Fluent 极简设计系统（Light / Dark）
│   ├── types/index.ts           # 前后端共享数据类型与格式化工具
│   ├── api/tauri.ts             # Tauri IPC 命令与事件类型安全封装
│   └── components/
│       ├── TopBar.tsx           # 顶部快速直连栏与全局控制
│       ├── SiteSidebar.tsx      # 左侧站点收藏夹与备份导入导出
│       ├── SiteModal.tsx        # 动态多协议站点表单（含跳板机与 ssh-agent）
│       ├── FilePane.tsx         # 本地/远程通用文件面板（含拖拽互传）
│       ├── TransferDrawer.tsx   # 底部并发传输队列面板
│       ├── EditorModal.tsx      # 远程文本在线编辑弹窗
│       └── TerminalDrawer.tsx   # SFTP 内置 SSH 命令终端
└── src-tauri/                   # Rust 后端与 Tauri 配置
    ├── Cargo.toml               # 依赖与 Release LTO/strip 瘦身配置
    ├── tauri.conf.json          # 窗口、图标与 NSIS/MSI/DMG/AppImage 打包配置
    ├── build.rs                 # Tauri 构建脚本
    ├── capabilities/default.json# Tauri v2 窗口安全能力声明
    ├── icons/                   # 全平台应用图标集合
    └── src/
        ├── main.rs              # 桌面可执行程序入口
        ├── lib.rs               # Tauri Builder 初始化与命令注册
        ├── error.rs             # 统一中文错误类型 `AppError`
        ├── models.rs            # 数据结构、路径规范化与 URI 解析
        ├── storage.rs           # `sites.json` 持久化与系统 Keyring 凭据读写
        ├── local_fs.rs          # 跨平台本地文件系统封装
        ├── transfer.rs          # 基于 Semaphore 的并发传输队列与事件节流推送
        ├── commands.rs          # 全部 Tauri IPC 命令与递归目录扫描/下载实现
        └── protocols/
            ├── mod.rs           # `RemoteFs` Trait 定义与协议连接工厂
            ├── sftp.rs          # SFTP / SSH Agent / ProxyJump / Shell 实现
            ├── ftp.rs           # FTP / FTPS (TLS 1.2/1.3) 实现
            ├── s3.rs            # S3 SigV4 与 ListObjectsV2 解析实现
            └── webdav.rs        # WebDAV RFC 4918 实现
```

---

## 4. 核心模块实现细节

### 4.1 统一远程文件系统接口 (`RemoteFs`)
定义于 [src-tauri/src/protocols/mod.rs](../src-tauri/src/protocols/mod.rs)，所有远程协议均实现以下异步方法：

- `list_dir(path)`：返回按「目录优先、名称不区分大小写升序」排序的 `Vec<FileEntry>`。
- `mkdir(path)` / `remove(path, is_dir)` / `rename(from, to)`：统一目录与文件操作。
- `chmod(path, mode)`：SFTP 与 FTP/FTPS 支持，S3/WebDAV 默认返回 `AppError::Unsupported`。
- `read_bytes(path, max_bytes)` / `write_bytes(path, data)`：支持在线查看与回传保存文本文件（上限 2 MB）。
- `download_file(remote, local, progress, cancel)` / `upload_file(local, remote, progress, cancel)`：64 KB 流式分块读写，实时回调字节进度并响应 `AtomicBool` 取消标记。
- `exec_shell(command, cwd)`：仅 SFTP 会话实现，在目标主机当前远程目录下执行命令并返回 `stdout` / `stderr` / `exit_code`。

### 4.2 SSH `ProxyJump` 与本地 `ssh-agent`
位于 [src-tauri/src/protocols/sftp.rs](../src-tauri/src/protocols/sftp.rs)：
- **本地 SSH Agent**：Windows 优先连接 `\\.\pipe\openssh-ssh-agent` 命名管道，失败时自动回退至 Pageant；macOS/Linux 连接 `SSH_AUTH_SOCK`。采用泛型 monomorphization（`try_agent_auth<S: AgentStream>`）避免 trait object 在异步 `Signer` 中触发高阶生命周期（HRTB）限制。
- **SSH 跳板机（ProxyJump）**：先连接并认证跳板机获得 `jump_sess`，调用 `channel_open_direct_tcpip(target_host, target_port, "127.0.0.1", 0)` 获取直连转发流，再以 `russh::client::connect_stream` 在该流上建立端到端加密的目标 SSH 会话，并由 `SftpClient._jump_ssh` 持有跳板机句柄直至会话关闭。

### 4.3 递归文件夹传输与下载位置选择
位于 [src-tauri/src/commands.rs](../src-tauri/src/commands.rs) 与 [src/App.tsx](../src/App.tsx)：
- **上传（`enqueue_upload_paths`）**：接收来自系统原生拖放（`getCurrentWebview().onDragDropEvent`）、双栏互拖或文件/文件夹选择器的路径数组，在阻塞线程池中遍历所有本地目录树生成 `UploadPlan`，先顺序创建远程目录，再将所有文件推入 `TransferManager` 并发上传。
- **下载（`enqueue_download`）**：通过 `DownloadRequest` 结构体同时支持：
  1. 单文件「另存为」精确保存路径（`localFilePath`）；
  2. 远程文件夹递归遍历并下载至用户选定的目标目录（`localDir` + `isDir: true`）；
  3. 双栏拖拽直接下载到左侧本地面板当前目录。
- **自动刷新**：前端监听 `transfer://update` 事件，当任务变为 `completed` 时自动刷新对应侧的文件列表。

### 4.4 安全与凭据存储模型
- 站点元数据存储于操作系统标准应用数据目录的 `sites.json`，严禁保存明文密码或私钥口令。
- 所有敏感凭据（密码、私钥口令、S3 Secret Key、STS Token、跳板机密码/口令）统一通过 `keyring` 以服务名 `com.xiaorui.ftp` 写入系统凭据库，并在编辑站点时执行**增量合并**（留空字段不会覆盖已保存密钥）。
- 导出站点备份文件（`export_sites_to_file`）仅导出非敏感元数据；导入（`import_sites_from_file`）时自动将 `hasSavedSecret` 重置为 `false`。

---

## 5. Tauri IPC 命令清单

| 分类 | 命令名称 | 说明 |
| :--- | :--- | :--- |
| **站点与凭据** | `list_sites`, `save_site`, `delete_site`, `export_sites_to_file`, `import_sites_from_file`, `check_ssh_agent_status` | 站点 CRUD、JSON 文件备份/恢复、本地 SSH Agent 检测 |
| **会话管理** | `connect_saved_site`, `connect_adhoc`, `quick_connect_uri`, `disconnect_session`, `list_sessions` | 建立/断开多协议远程会话 |
| **远程文件** | `remote_list_dir`, `remote_mkdir`, `remote_delete`, `remote_rename`, `remote_chmod`, `remote_read_text`, `remote_write_text` | 统一远程文件管理与在线轻编辑 |
| **本地文件** | `local_home_dir`, `local_list_roots`, `local_list_dir`, `local_mkdir`, `local_delete`, `local_rename` | 跨平台本地浏览与文件操作 |
| **传输队列** | `enqueue_upload`, `enqueue_upload_paths`, `enqueue_download`, `list_transfers`, `cancel_transfer`, `clear_finished_transfers` | 并发上传/下载队列与进度控制（事件：`transfer://update`） |
| **远程终端** | `ssh_exec_command` | 在当前 SFTP 会话的远程目录中执行 Shell 命令 |

---

## 6. 质量保障与发布流程

1. **代码规范**：Rust 后端需通过 `cargo fmt --check` 与 `cargo clippy --all-targets -- -D warnings`；前端需通过 TypeScript 严格模式（`strict: true`, `noUnusedLocals: true`, `noUnusedParameters: true`）校验。
2. **自动化测试**：Rust 单元测试覆盖路径规范化、多协议及跳板机 URI 解析、S3 `ListObjectsV2` XML 解析、WebDAV `PROPFIND` XML 解析以及本地目录递归上传计划生成。
3. **发布打包**：
   - Windows x64 本地一键发布：`npm run release:win64`（依次执行单元测试、前端构建与 Release LTO 打包，生成 NSIS/MSI 安装包并输出 SHA-256 校验值）。
   - 多平台 CI 自动构建：[.github/workflows/build-x64.yml](../.github/workflows/build-x64.yml) 支持在 `windows-latest`、`macos-13`、`ubuntu-22.04` 三个 x64 矩阵环境自动运行测试与安装包构建。
