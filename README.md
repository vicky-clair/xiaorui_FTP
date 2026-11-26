# xiaorui_FTP

> 一款基于 **Rust + Tauri v2 + React + TypeScript** 打造的现代、极简、跨平台 **x64** 文件传输与远程访问客户端。

**xiaorui_FTP** 专注「界面简单化、操作易用化、加密现代化」：无繁杂菜单栏、无沉重运行库依赖，原生 64 位（`x86_64`）构架，开箱支持 **SFTP（含跳板机与本地 ssh-agent）、FTPS/FTP、S3 兼容对象存储、WebDAV** 等主流远程访问方式，并内置文件拖拽递归传输、远程文本轻编辑与 SSH 命令行终端。

---

## 核心特性

- **极简现代界面**：单栏（纯远程）/ 双栏（本地 ↔ 远程）一键切换，支持多会话标签页、目录过滤、浅色/深色主题自适应与高分屏（Per-Monitor V2 DPI）清晰显示。
- **全协议现代加密栈**（纯 Rust `rustls` + `russh`，零外部 OpenSSL 依赖）：
  - **SFTP over SSH**：支持 Curve25519、Ed25519、RSA-SHA2-256、ECDSA、AES-GCM、ChaCha20-Poly1305；兼容 OpenSSH 8.8+/9.x 服务端。
  - **SSH 跳板机（ProxyJump）与本地 Agent**：纯 Rust `direct-tcpip` 隧道直连内网主机；自动适配 Windows OpenSSH Authentication Agent、Pageant 以及 macOS/Linux `SSH_AUTH_SOCK`。
  - **FTPS / FTP**：显式 FTPS（`AUTH TLS`）与隐式 FTPS（990 端口），原生支持 TLS 1.2 / TLS 1.3、二进制模式与 EPSV/PASV 自动回退。
  - **S3 兼容对象存储**：内置 AWS Signature V4，提供 **AWS S3 / 阿里云 OSS / 腾讯云 COS / MinIO / Cloudflare R2** 一键预设，支持 STS 临时凭证与 Path-Style。
  - **WebDAV / HTTPS**：兼容 **Nextcloud / Alist / 群晖 NAS / 坚果云**，支持 Basic 认证、Bearer Token（OAuth2）与匿名访问。
- **高效文件传输与日常运维**：
  - **拖拽递归上传 / 下载**：直接从 Windows 资源管理器或 macOS Finder 拖入文件/整个文件夹即可递归上传；双栏支持本地与远程面板互相拖拽传输。
  - **灵活选择保存位置**：点击远程文件「下载」弹出系统**另存为**对话框自由选择路径或重命名；点击远程文件夹「下载」弹出**选择目录**对话框递归保存；传输完成后自动刷新目录。
  - **远程文本在线轻编辑**：双击或点击编辑图标即可直接查看/修改 2 MB 以内的配置文件（如 `nginx.conf`、`.env`、`docker-compose.yml`），按 `Ctrl+S` 自动保存回传。
  - **内置 SSH 终端**：连接 SFTP 站点后可一键展开终端，自动定位到当前远程浏览目录，支持上下方向键历史命令与 `clear` 清屏。
  - **系统原生密钥安全托管**：站点密码、私钥口令、S3 Secret Key 与跳板机密钥均交由操作系统凭据库加密保管（Windows 凭据管理器 / macOS Keychain / Linux Secret Service），配置文件本身不落盘明文密钥。

---

## 支持协议与认证对照表

| 协议 | 默认端口 | 传输加密 | 支持认证方式 | 特色能力 |
| :--- | :---: | :--- | :--- | :--- |
| **SFTP (SSH)** | `22` | SSHv2 (Curve25519 / Ed25519 / AES-GCM / ChaCha20) | 密码、私钥文件（OpenSSH/PEM）、本地 `ssh-agent` / Pageant | ProxyJump 跳板机、CHMOD、内置 SSH 终端 |
| **FTPS (显式 TLS)** | `21` | TLS 1.2 / TLS 1.3 (`AUTH TLS`) | 用户名/密码、匿名登录 | 二进制流传输、EPSV/PASV 自动回退、CHMOD |
| **FTPS (隐式 TLS)** | `990` | TLS 1.2 / TLS 1.3 | 用户名/密码、匿名登录 | 兼容传统 990 端口加密服务 |
| **FTP (明文)** | `21` | 无（明文） | 用户名/密码、匿名登录 | 仅建议在可信局域网内使用 |
| **S3 兼容存储** | `443` | HTTPS (TLS 1.2 / 1.3) | Access Key + Secret Key（支持可选 STS Token） | AWS / OSS / COS / MinIO / R2 一键预设、Path-Style |
| **WebDAV** | `443` / `80` | HTTPS / HTTP | Basic（用户名+密码）、Bearer Token、匿名 | 兼容 Alist / Nextcloud / 群晖 NAS / 坚果云 |

---

## 快速使用指南

### 1. 顶部地址栏一键直连

在顶部输入框输入或粘贴连接 URI，按 **回车** 或点击 **「直连」** 即可快速连接：

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

> **提示**：若用户名或密码中包含 `@`、`:`、`/`、`#` 等特殊字符，在 URI 直连中需进行 URL 百分号编码（如 `@` 写为 `%40`），或直接点击右上角 **「新建站点」** 填写表单（无需手动编码）。

### 2. 新建与管理站点

1. 点击右上角 **「新建站点」**，选择协议类型后界面会自动精简为当前协议所需的字段。
2. **SFTP 跳板机（ProxyJump）**：在 SFTP 表单中勾选 **「通过 SSH 跳板机连接」**，填写跳板机地址、端口及认证方式（支持跳板机和目标机分别独立选择 `ssh-agent` / 私钥 / 密码）。
3. **S3 兼容存储**：点击表单顶部的 **阿里云 OSS / 腾讯云 COS / AWS S3 / MinIO / Cloudflare R2** 预设按钮可一键填入标准 Endpoint 与 Region，再填写 Bucket 与 AccessKey/SecretKey 即可。
4. 勾选底部的 **「将密钥安全托管到操作系统凭据库」** 后，下次点击左侧站点即可一键无感重连。
5. 左侧站点栏底部的 **「导出备份」/「导入」** 支持读写 `.json` 备份文件，方便在多台电脑间迁移站点列表。

### 3. 文件上传、下载与在线编辑

- **上传文件/文件夹**：
  - **方式一（推荐）**：直接从 Windows 资源管理器或 macOS Finder 将任意文件或多个文件夹拖入应用窗口，看到蓝色虚线提示框后松手即可自动递归建目录并并发上传。
  - **方式二**：在双栏模式下，直接把左侧「本地」面板的文件或文件夹拖到右侧「远程」面板，或悬浮点击上传图标。
  - **方式三**：点击远程面板右上角的 **「上传」**（多选文件）或文件夹上传图标（多选文件夹递归上传）。
- **下载文件/文件夹**：
  - **自由选择保存位置（默认）**：选中远程文件/文件夹后点击远程顶部 **「下载」** 按钮，或悬浮点击行末下载图标——单文件会弹出 **「另存为」** 窗口（可换目录并改名），文件夹会弹出 **「选择保存目录」** 窗口递归下载。
  - **双栏免弹窗直传**：直接将右侧「远程」的文件/文件夹拖拽到左侧「本地」面板，立即下载到当前本地目录。
- **在线编辑远程文本**：双击远程小文本文件（≤ 512 KB）或点击行末代码图标即可打开编辑器，修改后按 `Ctrl+S`（macOS 为 `Cmd+S`）直接保存回传。
- **内置 SSH 终端**：连接 SFTP 站点后，点击顶部右侧终端图标即可在底部打开命令行，自动在当前远程目录下执行命令。

---

## 使用注意事项与常见问题（FAQ）

1. **Windows 首次安装出现 SmartScreen「已保护你的电脑」提示**
   - 开源自构建安装包未购买商业代码签名证书，Windows Defender SmartScreen 会出现未知发布者提醒。确认安装包来自本人构建或可信发布页后，点击 **「更多信息」→「仍要运行」** 即可正常安装。
2. **Windows 上如何使用本地 `ssh-agent` 认证？**
   - **方法 A（推荐，Windows 10/11 内置 OpenSSH Agent）**：以管理员身份打开 PowerShell 运行：
     ```powershell
     Set-Service -Name ssh-agent -StartupType Automatic
     Start-Service ssh-agent
     ssh-add $env:USERPROFILE\.ssh\id_ed25519
     ```
   - **方法 B（Pageant）**：启动 PuTTY Pageant、KeePassXC 或 1Password 的 Pageant 兼容模式并加载私钥，软件会自动检测并回退使用 Pageant。
3. **S3 连接报 `SignatureDoesNotMatch` 或 `403 Forbidden`**
   - 自建 MinIO、Ceph RGW 或 Cloudflare R2 请勾选 **「使用 Path-Style 访问」**；
   - 阿里云 OSS / 腾讯云 COS 请取消勾选 Path-Style，并确认 **Region** 与 Endpoint 所在地域一致（例如阿里云杭州为 `oss-cn-hangzhou`，Endpoint 为 `https://oss-cn-hangzhou.aliyuncs.com`）。
4. **WebDAV 常见服务地址示例**
   - **Alist**：`http://<IP>:5244/dav`（或反代后的 `https://域名/dav`）
   - **Nextcloud**：`https://你的域名/remote.php/dav/files/<用户名>`（建议使用 Nextcloud 安全设置生成的「应用密码」）
   - **群晖 NAS**：`https://<NAS地址>:5006`（若使用自签名证书，请勾选「忽略自签名 TLS 证书校验」）
5. **站点备份与密钥安全说明**
   - 导出的 `xiaorui_FTP-sites-backup.json` **严格不包含任何密码、私钥口令或 Secret Key**，可放心备份；在新设备导入后，首次连接时编辑站点补充密钥即可重新写入新设备的系统凭据库。
6. **关于 SSH 主机密钥（TOFU）与安全网络建议**
   - 首次连接某台 SSH 服务器（含跳板机）时，程序会自动将其公钥的 **SHA-256 指纹** 持久化保存到应用数据目录的 `known_hosts.json`（与站点配置独立存放）；后续每次连接均严格比对指纹，若检测到指纹变化将**立即阻断连接并告警**以防范中间人攻击。
   - 若某台服务器合法重装系统或更换了主机密钥，可点击左侧站点栏底部的 **「主机」** 按钮打开「SSH 已知主机指纹管理」，移除旧指纹后再重新连接。
   - **明文 FTP** 协议以明文传输账号密码与文件内容，公网环境请务必优先使用 **SFTP** 或 **FTPS (显式 TLS)**。
7. **远程文本在线编辑大小限制**
   - 为避免大文件卡顿，在线编辑限制最大读取 **2 MB**（双击快捷预览为 512 KB）；更大的文件请使用下载功能在本地专业编辑器中查看。

---

## 本地开发与全平台 x64 构建

### 环境要求
- **Node.js** 20+（推荐 22 LTS）
- **Rust** 1.80+（已安装对应平台 `x86_64` 目标工具链）
- **Windows 10/11 x64**：Visual Studio C++ Build Tools + WebView2 Runtime（Win11 已内置）
- **Linux x64（Ubuntu/Debian）**：需安装 `libwebkit2gtk-4.1-dev libappindicator3-dev librsvg2-dev patchelf libdbus-1-dev`

### 常用命令

```bash
# 1. 安装前端依赖
npm install

# 2. 启动桌面端热重载开发模式
npm run tauri:dev

# 3. 运行 Rust 静态检查（Clippy）与单元测试
cd src-tauri
cargo clippy --target x86_64-pc-windows-msvc --all-targets -- -D warnings
cargo test --target x86_64-pc-windows-msvc

# 4. Windows x64 一键测试并构建 Release 安装包（NSIS + MSI，输出体积与 SHA-256）
npm run release:win64

# 5. 按平台单独构建 x64 安装包
npm run tauri:build:win64    # Windows x64 (NSIS / MSI)
npm run tauri:build:mac64    # macOS x64 (DMG)
npm run tauri:build:linux64  # Linux x64 (AppImage / DEB)
```

更多架构设计与模块接口细节请参阅 [docs/DEVELOPMENT_PLAN.md](docs/DEVELOPMENT_PLAN.md)。
