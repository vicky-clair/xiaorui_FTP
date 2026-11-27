# xiaorui_FTP

> 一款基于 **Rust + Tauri v2 + React 19 + TypeScript** 打造的现代、极简、跨平台 **x64** 文件传输与 SSH 终端工作台。

**xiaorui_FTP** 专注「界面简单化、操作易用化、加密现代化、终端一体化」：无繁杂菜单栏、无外部 OpenSSL 运行库依赖，原生 64 位（`x86_64`）构架，安装包仅约 **3.7 MB**。开箱支持 **SFTP（含跳板机、本地 ssh-agent 与 TOFU 主机指纹校验）、FTPS/FTP、S3 兼容对象存储、WebDAV** 等主流远程访问方式，并内置 **Xshell 风格多标签/独立窗口 SSH 交互终端**、**远端服务器硬件监控与多磁盘切换**、**关闭最小化到系统托盘**及**文件夹拖拽递归传输**。

---

## 核心特性

### 1. 现代极简桌面界面
- **双栏 / 单栏自由切换 + 可拖拽比例调节**：支持本地 ↔ 远程双栏联动或纯远程单栏模式，中间分割线支持拖动调宽（双击还原），支持多会话标签页与浅色/深色双主题自适应。
- **高效文件浏览**：顶部命令栏式快速直连（`Ctrl+K`）、独立全宽可点击路径面包屑、点击列头按名称/大小/时间升降序排序、语义化文件类型图标、原生级右键上下文菜单与目录即时过滤。
- **关闭最小化到系统托盘**：默认点击关闭按钮隐藏至系统托盘（不中断后台传输与会话），左键单击托盘图标恢复窗口，右键提供「显示主窗口 / 退出」，底部状态栏可一键切换「关闭→托盘 / 关闭→退出」。

### 2. 全协议现代加密栈（纯 Rust `rustls` + `russh`，零 OpenSSL 依赖）
- **SFTP over SSH**：支持 Curve25519、Ed25519、RSA-SHA2-256、ECDSA、AES-GCM、ChaCha20-Poly1305；兼容 OpenSSH 8.8+/9.x 服务端与 Windows OpenSSH；自动识别软链接目录；全量内置 20 秒连接超时保护。
- **SSH 跳板机（ProxyJump）与本地 Agent**：纯 Rust `direct-tcpip` 隧道直连内网主机；安全适配 Windows OpenSSH Authentication Agent、Pageant 及 macOS/Linux `SSH_AUTH_SOCK`。
- **SSH 主机公钥 TOFU 校验**：首次连接自动记录主机（含跳板机）SHA-256 指纹至 `known_hosts.json`，指纹变更立即拦截告警防范中间人攻击，支持在「主机」弹窗中按条管理。
- **FTPS / FTP**：显式 FTPS（`AUTH TLS`）与隐式 FTPS（990 端口），原生 TLS 1.2 / TLS 1.3、二进制传输、EPSV/PASV 自动回退、支持非空目录递归删除。
- **S3 兼容对象存储**：内置 AWS Signature V4，提供 **AWS S3 / 阿里云 OSS / 腾讯云 COS / MinIO / Cloudflare R2** 一键预设，支持 64 KB 流式上传、千级以上对象自动分页列举与递归删除、STS 临时凭证与 Path-Style。
- **WebDAV / HTTPS**：兼容 **Nextcloud / Alist / 群晖 NAS / 坚果云**，支持 Basic 认证、Bearer Token（OAuth2）、匿名访问、中文路径编解码与流式传输。

### 3. Xshell 风格交互式 SSH 终端（直接调用远端默认 Shell）
- **原生默认登录 Shell**：基于真实 `xterm-256color` PTY + `xterm.js`，直接启动远端服务器默认 Shell（如 `zsh` / Oh-My-Zsh / `bash` / `fish` / PowerShell），完整支持彩色提示符、`Tab` 补全、`Ctrl+C`、`Ctrl+R` 以及 `vim` / `htop` / `tmux` / `tail -f` 等交互程序。
- **三种显示形态**：底部停靠（可拖拽调高）、工作区全屏放大（不断开运行中的 Shell）、**弹出独立操作系统窗口**（可拖至副屏、自由缩放最大化）。
- **Xshell 实用效率套件**：同会话多标签 Shell 并行、`Ctrl+F` 终端日志搜索（大小写/正则）、字号缩放（`A-`/`A+` 或 `Ctrl+=`/`-`/`0`）、5 套配色主题、选中即复制 & 右键粘贴、会话日志导出为 `.log`、底部可自定义快捷命令栏。
- **本地 ↔ SSH 双向灵活联动**：
  - 远程文件面板右键菜单支持 **「在此目录打开 SSH 终端」** 与 **「将路径插入终端光标」**；
  - 终端内置 **「同步目录」**（一键 `cd` 到当前 SFTP 浏览目录）与 **「上传文件」**（免装 `lrzsz`，直走高速 SFTP 队列）；
  - **独立终端窗口**同样支持直接拖入本地文件/文件夹递归上传。

### 4. 远端硬件监控与多磁盘一键切换
- **底部状态栏实时监测**（SFTP/SSH 会话）：每 15 秒轻量采样远端服务器的 **CPU 核数与占用率、内存已用/总量、磁盘占用率**（高负载自动琥珀/红色预警），兼容 Linux / macOS 与 Windows OpenSSH 服务端。
- **远端多磁盘自由切换**：自动剔除 `/boot`、容器虚拟挂载与重复 bind 挂载，仅展示真实物理磁盘/数据挂载点（Linux 如 `/`、`/data`、`/mnt/...`，Windows 如 `C:\`、`D:\`）。在远程面板工具栏点击磁盘按钮展开带容量进度条的浮层卡片即可一键切换，也可点击底部状态栏磁盘胶囊循环切换。

---

## 支持协议与认证对照表

| 协议 | 默认端口 | 传输加密 | 支持认证方式 | 特色能力 |
| :--- | :---: | :--- | :--- | :--- |
| **SFTP (SSH)** | `22` | SSHv2 (Curve25519 / Ed25519 / AES-GCM / ChaCha20) | 密码、私钥文件（OpenSSH/PEM）、本地 `ssh-agent` / Pageant | ProxyJump 跳板机、TOFU 指纹校验、CHMOD、Xshell 风格交互终端、硬件监控与多磁盘切换 |
| **FTPS (显式 TLS)** | `21` | TLS 1.2 / TLS 1.3 (`AUTH TLS`) | 用户名/密码、匿名登录 | 二进制流传输、EPSV/PASV 回退、非空目录递归删除、CHMOD |
| **FTPS (隐式 TLS)** | `990` | TLS 1.2 / TLS 1.3 | 用户名/密码、匿名登录 | 兼容传统 990 端口加密服务 |
| **FTP (明文)** | `21` | 无（明文） | 用户名/密码、匿名登录 | 仅建议在可信内网环境中使用 |
| **S3 兼容存储** | `443` | HTTPS (TLS 1.2 / 1.3) | Access Key + Secret Key（支持可选 STS Token） | AWS / OSS / COS / MinIO / R2 预设、64 KB 流式上传、全量分页列举与删除 |
| **WebDAV** | `443` / `80` | HTTPS / HTTP | Basic（用户名+密码）、Bearer Token、匿名 | 兼容 Alist / Nextcloud / 群晖 NAS / 坚果云，流式传输 |

---

## 快速使用指南

### 1. 顶部地址栏一键直连（`Ctrl+K`）

按 `Ctrl+K` 聚焦顶部地址栏，输入或粘贴 URI 后按 **回车** 即可直连：

```text
# SFTP（密码登录）
sftp://root:your_password@192.168.1.10:22/var/www

# SFTP（未写密码时自动尝试本地 ssh-agent / Pageant）
sftp://ubuntu@10.0.0.8/home/ubuntu

# SFTP 经由 SSH 跳板机（ProxyJump）直连内网机器
sftp://root@10.0.0.5/srv/app?jump=ops@bastion.example.com:22

# 显式 FTPS / 隐式 FTPS / 明文 FTP
ftps://user:pass@ftp.example.com/public
ftpsi://user:pass@ftp.example.com:990/
ftp://ftp.example.com/pub

# S3 兼容对象存储（Path-Style）
s3://ACCESS_KEY:SECRET_KEY@minio.local:9000/my-bucket/docs

# WebDAV（HTTPS 使用 webdavs://，HTTP 使用 webdav://）
webdavs://user:pass@nas.example.com/dav
webdav://user:pass@192.168.1.20:5244/dav
```

> **提示**：若用户名或密码中包含 `@`、`:`、`/`、`#` 等特殊字符，URI 直连中需做百分号编码（如 `@` 写为 `%40`），或直接按 `Ctrl+N` / 点击 **「新建站点」** 填写表单（无需手动编码）。

### 2. 新建、管理与迁移站点

1. 点击右上角 **「新建站点」**（`Ctrl+N`），在顶部 5 协议分段按钮中选择协议，表单自动呈现所需字段。
2. **SFTP 跳板机**：勾选 **「通过 SSH 跳板机连接」**，跳板机与目标机支持各自独立配置 `ssh-agent` / 私钥 / 密码。
3. **S3 预设**：点击 **阿里云 OSS / 腾讯云 COS / AWS S3 / MinIO / Cloudflare R2** 预设按钮一键填充 Endpoint 与 Region。
4. 勾选 **「将密钥安全托管到操作系统凭据库」** 后，凭据由系统原生凭据库加密保管，下次点击左侧站点即可一键无感重连。
5. 侧边栏底部的 **「导入」/「备份」** 可迁移 `.json` 站点清单（**严禁包含明文密码**）；**「主机」** 用于管理已信任的 SSH 主机指纹。

### 3. 文件上传、下载、编辑与多磁盘访问

- **上传文件/文件夹**：
  - 直接从 Windows 资源管理器或 macOS Finder 将文件/文件夹拖入主窗口或独立终端窗口，松手即递归建目录并并发上传；
  - 双栏模式下可将左侧「本地」文件/文件夹直接拖到右侧「远程」面板；
  - 或点击远程面板顶部 **「上传」**（文件）/ 文件夹上传图标（递归上传文件夹）。
- **下载文件/文件夹**：
  - **选择保存位置（默认）**：选中远程项后点击顶部 **「下载」**、行末下载图标或右键菜单，单文件弹出 **「另存为」** 对话框，文件夹弹出 **「选择保存目录」** 递归下载；
  - **双栏免弹窗直传**：直接将右侧远程项拖拽到左侧本地面板。
- **远端多磁盘切换**：
  - SFTP 会话自动探测全部真实磁盘/挂载点，点击远程面板顶部的**磁盘按钮**（如 `/` `2盘`）即可在下拉卡片中直观查看容量并一键跳转，或点击底部状态栏的磁盘胶囊循环切换；
  - 也可点击路径面包屑空白处手输挂载路径（Linux 如 `/data`、`/mnt/disk2`，Windows OpenSSH 如 `/D:/`、`/E:/`）。
- **在线编辑远程文本**：双击 ≤ 512 KB 的远程文本文件或右键「在线查看 / 编辑」（上限 2 MB），按 `Ctrl+S` 保存回传。

### 4. 常用快捷键速查表

| 场景 | 快捷键 | 功能说明 |
| :--- | :--- | :--- |
| **全局工作台** | `Ctrl+K` / `Cmd+K` | 聚焦顶部快速直连地址栏 |
| **全局工作台** | `Ctrl+N` / `Cmd+N` | 打开「新建站点」弹窗 |
| **全局工作台** | `F5` | 刷新当前远程（或本地）文件列表 |
| **文件列表** | `F2` / `Delete` | 重命名 / 删除当前选中项 |
| **文件列表** | `右键` | 打开原生上下文菜单（含终端联动、复制路径、CHMOD） |
| **在线编辑器** | `Ctrl+S` / `Esc` | 保存回传远程服务器 / 关闭编辑器 |
| **SSH 终端** | `Ctrl+F` | 打开终端输出搜索栏（支持大小写/正则） |
| **SSH 终端** | `Ctrl+=` / `Ctrl+-` / `Ctrl+0` | 放大字号 / 缩小字号 / 重置为 13px |
| **SSH 终端** | `Ctrl+Shift+C` / `Ctrl+Shift+V` | 复制选中文本 / 粘贴剪贴板到终端（开启选中即复制后划选即复制、右键即粘贴） |

---

## 注意事项与常见问题（FAQ）

1. **升级覆盖安装会丢失我的站点和密码吗？**
   - **不会**。站点清单（`sites.json`）、SSH 已知主机指纹（`known_hosts.json`）与托盘偏好（`settings.json`）保存在操作系统标准应用数据目录（`com.xiaorui.ftp`），敏感凭据保存在操作系统凭据库（服务名 `com.xiaorui.ftp`），与程序安装目录完全分离，覆盖安装新版后数据 100% 保留。
2. **Windows 首次安装出现 SmartScreen「已保护你的电脑」提示**
   - 开源自构建安装包未购买商业代码签名证书，Windows Defender SmartScreen 会弹出提示。确认来源可信后点击 **「更多信息」→「仍要运行」** 即可。
3. **关闭软件后怎么彻底退出？**
   - 默认开启「关闭时最小化到系统托盘」，点击窗口右上角 `×` 会隐藏到任务栏右下角托盘继续保持后台连接与传输。若需彻底退出，可**右键托盘图标选择「退出 xiaorui_FTP」**，或点击底部状态栏右下角的 `关闭→托盘` 切换为 `关闭→退出`。
4. **Windows 上如何启用本地 `ssh-agent`？**
   - **方式 A（推荐，Windows 内置 OpenSSH Agent）**：以管理员身份在 PowerShell 执行：
     ```powershell
     Set-Service -Name ssh-agent -StartupType Automatic
     Start-Service ssh-agent
     ssh-add $env:USERPROFILE\.ssh\id_ed25519
     ```
   - **方式 B（Pageant）**：启动 PuTTY Pageant、KeePassXC 或 1Password 的 Pageant 兼容模式并加载私钥，软件会自动安全检测并回退使用（未启动任何 Agent 时会安全提示，绝不崩溃）。
5. **服务器重装后连接 SSH 提示主机指纹不匹配**
   - 本软件默认启用严格 TOFU 主机指纹校验以防范中间人攻击。确认服务器确实重装或更换了主机密钥后，点击左侧站点栏底部的 **「主机」** 按钮，移除对应 `host:port` 的旧指纹记录即可重新信任连接。
6. **部分 SFTP 账号底部不显示硬件监控或无法打开终端**
   - 若远端管理员配置了仅允许 `internal-sftp` 的受限账号（禁用了 SSH Shell/exec 通道），硬件采样会自动静默跳过，不影响正常文件上传下载。
7. **S3 连接报 `SignatureDoesNotMatch` 或 `403 Forbidden`**
   - MinIO、Ceph RGW、Cloudflare R2 请勾选 **「使用 Path-Style 访问」**；
   - 阿里云 OSS / 腾讯云 COS 请取消勾选 Path-Style，并确认 **Region** 与 Endpoint 地域严格一致（如 `oss-cn-hangzhou`）。
8. **WebDAV 常见地址参考**
   - **Alist**：`http://<IP>:5244/dav`（或 `https://域名/dav`）
   - **Nextcloud**：`https://你的域名/remote.php/dav/files/<用户名>`（推荐使用应用专用密码）
   - **群晖 NAS**：`https://<NAS地址>:5006`（自签名证书需勾选「忽略自签名 TLS 证书校验」）
9. **网络安全建议**
   - 公网传输请优先使用 **SFTP**、**FTPS（显式 TLS）**、**HTTPS S3** 或 **HTTPS WebDAV**，避免在公网使用明文 FTP。

---

## 本地开发与全平台 x64 构建

### 环境要求
- **Node.js** 20+（推荐 22/24 LTS）
- **Rust** 1.80+（已安装 `x86_64` 目标工具链、`rustfmt`、`clippy`）
- **Windows 10/11 x64**：Visual Studio C++ Build Tools + WebView2 Runtime（Win11 内置）
- **Linux x64（Ubuntu/Debian）**：`libwebkit2gtk-4.1-dev libappindicator3-dev librsvg2-dev patchelf libdbus-1-dev`

### 常用命令

```bash
# 1. 安装前端依赖
npm install

# 2. 启动桌面端热重载开发模式
npm run tauri:dev

# 3. 执行前端类型检查与生产构建
npm run build

# 4. 运行 Rust 格式检查、Clippy 零警告门禁与全部单元测试
cd src-tauri
cargo fmt -- --check
cargo clippy --target x86_64-pc-windows-msvc --all-targets -- -D warnings
cargo test --target x86_64-pc-windows-msvc

# 5. Windows x64 一键测试 + 构建 Release 安装包（NSIS + MSI，自动输出体积与 SHA-256）
npm run release:win64

# 6. macOS / Linux x64 一键测试 + 构建 Release 安装包
bash scripts/build-release.sh
```

更多架构设计与模块接口细节请参阅 [docs/DEVELOPMENT_PLAN.md](docs/DEVELOPMENT_PLAN.md)；与主流商业 FTP 软件（FlashFXP / SmartFTP / CuteFTP / Wing FTP Server）的架构与功能对照分析请参阅 [docs/COMPETITIVE_COMPARISON.md](docs/COMPETITIVE_COMPARISON.md)。
