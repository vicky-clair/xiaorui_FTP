//! 核心数据模型与路径/URI 工具函数模块
//!
//! 定义站点配置、认证方式、会话信息、统一文件条目、传输任务结构，
//! 以及跨协议路径规范化和快速直连 URI 解析逻辑。

use serde::{Deserialize, Serialize};
use url::Url;

use crate::error::{AppError, AppResult};

/// 支持的远程访问协议枚举
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Protocol {
    /// SSH 文件传输协议（支持现代加密算法、跳板机与远程终端）
    Sftp,
    /// 显式加密 FTPS（先明文连接再通过 AUTH TLS 升级为 TLS 1.2/1.3）
    FtpsExplicit,
    /// 隐式加密 FTPS（默认 990 端口，建立连接即启用 TLS 1.2/1.3）
    FtpsImplicit,
    /// 标准明文 FTP（仅建议在受信任的内网环境中使用）
    Ftp,
    /// S3 兼容对象存储（AWS S3 / 阿里云 OSS / 腾讯云 COS / MinIO / Cloudflare R2 等）
    S3,
    /// WebDAV / HTTPS（Nextcloud / Alist / 群晖 NAS / 坚果云等）
    Webdav,
}

impl Protocol {
    /// 返回协议对应的默认网络端口
    pub fn default_port(self) -> u16 {
        match self {
            Protocol::Sftp => 22,
            Protocol::FtpsExplicit | Protocol::Ftp => 21,
            Protocol::FtpsImplicit => 990,
            Protocol::S3 | Protocol::Webdav => 443,
        }
    }

    /// 当前协议是否支持内置远程命令终端
    pub fn supports_shell(self) -> bool {
        matches!(self, Protocol::Sftp)
    }

    /// 当前协议是否支持修改 Unix 文件权限（CHMOD）
    pub fn supports_chmod(self) -> bool {
        matches!(
            self,
            Protocol::Sftp | Protocol::Ftp | Protocol::FtpsExplicit | Protocol::FtpsImplicit
        )
    }
}

/// 认证方式枚举
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AuthType {
    /// 用户名 + 密码（默认）
    #[default]
    Password,
    /// SSH 私钥文件认证（支持 Ed25519 / RSA-SHA2 / ECDSA 及可选口令）
    PrivateKey,
    /// 本地 SSH Agent 认证（Windows OpenSSH Agent / Pageant / Unix SSH_AUTH_SOCK）
    Agent,
    /// 对象存储 Access Key + Secret Key（支持可选 STS 临时令牌）
    AccessKey,
    /// Bearer Token / OAuth2 访问令牌（适用于 WebDAV）
    Token,
    /// 匿名免密登录
    Anonymous,
}

/// 持久化到磁盘的站点配置（严格不含任何明文密钥）
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SiteConfig {
    /// 站点唯一 ID（UUID v4）
    pub id: String,
    /// 站点显示名称
    pub name: String,
    /// 站点所属分组名称
    #[serde(default)]
    pub group: String,
    /// 使用的传输协议
    pub protocol: Protocol,
    /// 服务器主机名或 IP
    pub host: String,
    /// 服务器端口
    pub port: u16,
    /// 登录用户名（S3 模式下为 Access Key ID）
    #[serde(default)]
    pub username: String,
    /// 主机认证方式
    #[serde(default)]
    pub auth_type: AuthType,
    /// SSH 私钥本地文件路径
    #[serde(default)]
    pub private_key_path: Option<String>,
    /// 连接后默认进入的远程目录
    #[serde(default)]
    pub remote_path: String,
    /// S3 自定义 Endpoint 或 WebDAV 完整服务地址
    #[serde(default)]
    pub endpoint: Option<String>,
    /// S3 存储桶名称
    #[serde(default)]
    pub bucket: Option<String>,
    /// S3 区域标识（默认 us-east-1）
    #[serde(default)]
    pub region: Option<String>,
    /// S3 是否使用 Path-Style 路径风格（MinIO 等自建服务需开启）
    #[serde(default)]
    pub path_style: bool,
    /// 是否跳过自签名 TLS 证书校验（仅限内网测试环境）
    #[serde(default)]
    pub tls_skip_verify: bool,
    /// 可选 SSH 跳板机（ProxyJump）主机地址
    #[serde(default)]
    pub jump_host: Option<String>,
    /// 跳板机端口（默认 22）
    #[serde(default)]
    pub jump_port: Option<u16>,
    /// 跳板机登录用户名
    #[serde(default)]
    pub jump_username: Option<String>,
    /// 跳板机认证方式
    #[serde(default)]
    pub jump_auth_type: Option<AuthType>,
    /// 跳板机 SSH 私钥文件路径
    #[serde(default)]
    pub jump_private_key_path: Option<String>,
    /// 备注信息
    #[serde(default)]
    pub notes: String,
    /// 是否已在系统凭据库中托管密钥
    #[serde(default)]
    pub has_saved_secret: bool,
    /// 最后更新时间戳（秒）
    pub updated_at: i64,
}

/// 前端提交的新建/编辑站点或临时连接表单载荷（含瞬态敏感密钥，手写脱敏 Debug 并在 Drop 时清零内存）
#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SiteInput {
    pub id: Option<String>,
    pub name: String,
    #[serde(default)]
    pub group: String,
    pub protocol: Protocol,
    pub host: String,
    pub port: Option<u16>,
    #[serde(default)]
    pub username: String,
    #[serde(default)]
    pub auth_type: AuthType,
    /// 密码 / Secret Access Key / Bearer Token
    #[serde(default)]
    pub secret: Option<String>,
    /// SSH 私钥保护口令
    #[serde(default)]
    pub passphrase: Option<String>,
    /// S3 STS 临时安全令牌
    #[serde(default)]
    pub session_token: Option<String>,
    #[serde(default)]
    pub private_key_path: Option<String>,
    #[serde(default)]
    pub remote_path: String,
    #[serde(default)]
    pub endpoint: Option<String>,
    #[serde(default)]
    pub bucket: Option<String>,
    #[serde(default)]
    pub region: Option<String>,
    #[serde(default)]
    pub path_style: bool,
    #[serde(default)]
    pub tls_skip_verify: bool,
    #[serde(default)]
    pub jump_host: Option<String>,
    #[serde(default)]
    pub jump_port: Option<u16>,
    #[serde(default)]
    pub jump_username: Option<String>,
    #[serde(default)]
    pub jump_auth_type: Option<AuthType>,
    #[serde(default)]
    pub jump_private_key_path: Option<String>,
    /// 跳板机密码
    #[serde(default)]
    pub jump_secret: Option<String>,
    /// 跳板机私钥保护口令
    #[serde(default)]
    pub jump_passphrase: Option<String>,
    #[serde(default)]
    pub notes: String,
    /// 是否将密钥写入操作系统凭据库
    #[serde(default)]
    pub save_secret: bool,
}

impl std::fmt::Debug for SiteInput {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SiteInput")
            .field("id", &self.id)
            .field("name", &self.name)
            .field("protocol", &self.protocol)
            .field("host", &self.host)
            .field("port", &self.port)
            .field("username", &self.username)
            .field("auth_type", &self.auth_type)
            .field("secret", &self.secret.as_ref().map(|_| "[REDACTED]"))
            .field(
                "passphrase",
                &self.passphrase.as_ref().map(|_| "[REDACTED]"),
            )
            .field(
                "session_token",
                &self.session_token.as_ref().map(|_| "[REDACTED]"),
            )
            .field("jump_host", &self.jump_host)
            .field(
                "jump_secret",
                &self.jump_secret.as_ref().map(|_| "[REDACTED]"),
            )
            .field(
                "jump_passphrase",
                &self.jump_passphrase.as_ref().map(|_| "[REDACTED]"),
            )
            .field("save_secret", &self.save_secret)
            .finish()
    }
}

impl Drop for SiteInput {
    fn drop(&mut self) {
        use zeroize::Zeroize;
        if let Some(s) = self.secret.as_mut() {
            s.zeroize();
        }
        if let Some(s) = self.passphrase.as_mut() {
            s.zeroize();
        }
        if let Some(s) = self.session_token.as_mut() {
            s.zeroize();
        }
        if let Some(s) = self.jump_secret.as_mut() {
            s.zeroize();
        }
        if let Some(s) = self.jump_passphrase.as_mut() {
            s.zeroize();
        }
    }
}

/// 托管在操作系统凭据库中的站点敏感密钥集合（脱敏 Debug 且 Drop 时自动覆写清零内存）
#[derive(Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SiteSecrets {
    pub secret: Option<String>,
    pub passphrase: Option<String>,
    pub session_token: Option<String>,
    #[serde(default)]
    pub jump_secret: Option<String>,
    #[serde(default)]
    pub jump_passphrase: Option<String>,
}

impl std::fmt::Debug for SiteSecrets {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SiteSecrets")
            .field("secret", &self.secret.as_ref().map(|_| "[REDACTED]"))
            .field(
                "passphrase",
                &self.passphrase.as_ref().map(|_| "[REDACTED]"),
            )
            .field(
                "session_token",
                &self.session_token.as_ref().map(|_| "[REDACTED]"),
            )
            .field(
                "jump_secret",
                &self.jump_secret.as_ref().map(|_| "[REDACTED]"),
            )
            .field(
                "jump_passphrase",
                &self.jump_passphrase.as_ref().map(|_| "[REDACTED]"),
            )
            .finish()
    }
}

impl Drop for SiteSecrets {
    fn drop(&mut self) {
        use zeroize::Zeroize;
        if let Some(s) = self.secret.as_mut() {
            s.zeroize();
        }
        if let Some(s) = self.passphrase.as_mut() {
            s.zeroize();
        }
        if let Some(s) = self.session_token.as_mut() {
            s.zeroize();
        }
        if let Some(s) = self.jump_secret.as_mut() {
            s.zeroize();
        }
        if let Some(s) = self.jump_passphrase.as_mut() {
            s.zeroize();
        }
    }
}

/// 本地 SSH Agent 可用性检测结果
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentStatus {
    /// 是否成功连接到本地 Agent
    pub available: bool,
    /// 检测到的后端名称（Windows OpenSSH Agent / Pageant / Unix ssh-agent / none）
    pub backend: String,
    /// 当前已加载的密钥数量
    pub key_count: usize,
    /// 面向用户的中文状态说明
    pub message: String,
}

/// 活跃远程会话的摘要信息
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionInfo {
    pub session_id: String,
    pub site_id: Option<String>,
    pub name: String,
    pub protocol: Protocol,
    pub host: String,
    pub port: u16,
    pub current_path: String,
    pub supports_shell: bool,
    pub supports_chmod: bool,
    /// 是否处于明文传输状态（如 FTP 或 HTTP WebDAV/S3），供前端显示持续安全警示
    #[serde(default)]
    pub insecure_transport: bool,
    /// 是否跳过了 TLS 证书校验（自签名测试模式），供前端显示持续安全警示
    #[serde(default)]
    pub tls_skip_verify: bool,
}

/// 统一文件/目录条目（同时用于本地与所有远程协议）
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FileEntry {
    /// 文件或目录名称
    pub name: String,
    /// 完整路径
    pub path: String,
    /// 是否为目录
    pub is_dir: bool,
    /// 文件字节大小（目录固定为 0）
    pub size: u64,
    /// 最后修改时间戳（Unix 秒）
    pub modified: Option<i64>,
    /// 人类可读权限字符串（如 `rwxr-xr-x (755)`）
    pub permissions: Option<String>,
    /// Unix 八进制权限掩码（低 9 位）
    pub mode: Option<u32>,
    /// 是否为符号链接
    pub is_symlink: bool,
}

/// 传输方向
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TransferDirection {
    Upload,
    Download,
}

/// 传输任务状态
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TransferStatus {
    Queued,
    Running,
    Completed,
    Failed,
    Cancelled,
}

/// 单个文件传输任务模型
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TransferTask {
    pub id: String,
    pub session_id: String,
    pub direction: TransferDirection,
    pub local_path: String,
    pub remote_path: String,
    pub file_name: String,
    pub total_bytes: u64,
    pub transferred_bytes: u64,
    pub speed_bps: u64,
    pub status: TransferStatus,
    pub error: Option<String>,
    pub created_at: i64,
}

/// SSH 远程终端单次命令执行结果
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ShellOutput {
    pub stdout: String,
    pub stderr: String,
    pub exit_code: i32,
}

/// 交互式 SSH PTY 输出流事件载荷（使用 Base64 传输原始字节流，避免多字节 UTF-8 中文跨包截断乱码）
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PtyDataEvent {
    pub terminal_id: String,
    pub data_b64: String,
}

/// 交互式 SSH PTY 退出事件载荷
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PtyExitEvent {
    pub terminal_id: String,
}

/// 本地快捷根位置（主目录、桌面、下载或 Windows 盘符）
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LocalRoot {
    pub name: String,
    pub path: String,
}

fn default_true() -> bool {
    true
}

/// 应用轻量偏好设置（独立存储于 `settings.json`，绝不触碰 `sites.json` 与凭据库）
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AppSettings {
    /// 点击窗口关闭按钮时是否最小化到系统托盘（默认开启）
    #[serde(default = "default_true")]
    pub minimize_to_tray: bool,
}

impl Default for AppSettings {
    fn default() -> Self {
        Self {
            minimize_to_tray: true,
        }
    }
}

/// 远端服务器物理磁盘 / 挂载点信息（兼容 Linux 多挂载盘与 Windows 多盘符）
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RemoteDisk {
    /// 磁盘展示名（如 `/`、`/data`、`D:\`）
    pub label: String,
    /// 可直接跳转浏览的远程路径（如 `/`、`/data`、`/D:/`）
    pub mount_path: String,
    /// 总容量（字节）
    pub total_bytes: u64,
    /// 已用容量（字节）
    pub used_bytes: u64,
    /// 可用容量（字节）
    pub avail_bytes: u64,
    /// 使用率百分比（0–100）
    pub use_percent: u8,
}

/// 远端或本机单个网络接口的实时流量与地址信息
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NetInterface {
    /// 网卡接口名（如 `enp2s0`、`wlp1s0`、`Ethernet`、`WLAN`）
    pub name: String,
    /// IPv4 地址（未分配时为 `None`）
    pub ip: Option<String>,
    /// 实时下行速率（字节/秒）
    pub rx_bytes_per_sec: u64,
    /// 实时上行速率（字节/秒）
    pub tx_bytes_per_sec: u64,
}

/// 活跃 SSH 本地端口转发隧道摘要
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SshTunnelInfo {
    /// 隧道唯一 ID
    pub id: String,
    /// 所属 SSH 会话 ID
    pub session_id: String,
    /// 本地回环监听端口（强制绑定 `127.0.0.1`）
    pub local_port: u16,
    /// 远端目标主机（如 `127.0.0.1` 或内网数据库 IP）
    pub remote_host: String,
    /// 远端目标端口
    pub remote_port: u16,
    /// 当前活跃 TCP 转发连接数
    pub active_connections: u32,
    /// 创建时间戳（Unix 秒）
    pub created_at: i64,
}

/// 远端或本机服务器硬件规格与实时资源占用快照（区分 Linux / Windows 采集）
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ServerMetrics {
    /// 操作系统家族（`linux` / `windows` / `macos`）
    pub os_family: String,
    /// 操作系统名称（如 Debian GNU/Linux 13 / Windows 11 Pro）
    pub os_name: String,
    /// 主机名
    pub hostname: Option<String>,
    /// 系统运行时长（秒）
    pub uptime_secs: Option<u64>,
    /// CPU 逻辑核心数
    pub cpu_cores: u32,
    /// CPU 实时总占用率百分比（0.0–100.0）
    pub cpu_usage_pct: Option<f32>,
    /// CPU 用户态占比百分比（Linux `/proc/stat`）
    pub cpu_user_pct: Option<f32>,
    /// CPU 内核态占比百分比（Linux `/proc/stat`）
    pub cpu_sys_pct: Option<f32>,
    /// 系统负载均值（如 `0.42 0.38 0.31`）
    pub load_avg: Option<String>,
    /// 内存已用字节数
    pub mem_used_bytes: u64,
    /// 内存总字节数
    pub mem_total_bytes: u64,
    /// Swap / 页面文件已用字节数
    pub swap_used_bytes: u64,
    /// Swap / 页面文件总字节数
    pub swap_total_bytes: u64,
    /// GPU 名称（若检测到显卡）
    pub gpu_name: Option<String>,
    /// GPU 利用率百分比
    pub gpu_usage_pct: Option<f32>,
    /// GPU 显存已用（MB）
    pub gpu_mem_used_mb: Option<u64>,
    /// GPU 显存总量（MB）
    pub gpu_mem_total_mb: Option<u64>,
    /// 默认网关 IPv4 地址
    pub default_gateway: Option<String>,
    /// 汇总实时下行速率（字节/秒）
    pub net_rx_bps: u64,
    /// 汇总实时上行速率（字节/秒）
    pub net_tx_bps: u64,
    /// 各网络接口明细列表
    pub net_interfaces: Vec<NetInterface>,
    /// 全部已挂载物理磁盘 / 盘符列表
    pub disks: Vec<RemoteDisk>,
    /// 采集时间戳（Unix 秒）
    pub timestamp: i64,
}

/// 规范化 POSIX 风格远程路径：统一斜杠方向、消除 `.` / `..` 与重复斜杠，始终以 `/` 开头
pub fn normalize_remote_path(path: &str) -> String {
    let trimmed = path.trim().replace('\\', "/");
    if trimmed.is_empty() || trimmed == "/" {
        return "/".to_string();
    }
    let mut parts: Vec<&str> = Vec::new();
    for seg in trimmed.split('/') {
        match seg {
            "" | "." => continue,
            ".." => {
                parts.pop();
            }
            other => parts.push(other),
        }
    }
    if parts.is_empty() {
        "/".to_string()
    } else {
        format!("/{}", parts.join("/"))
    }
}

/// 安全拼接远程目录路径与子项名称
pub fn join_remote_path(base: &str, name: &str) -> String {
    let base_norm = normalize_remote_path(base);
    let clean_name = name.trim_matches('/');
    if base_norm == "/" {
        format!("/{}", clean_name)
    } else {
        format!("{}/{}", base_norm, clean_name)
    }
}

/// 将 Unix 权限整数格式化为易读字符串（例如 `rwxr-xr-x (755)`）
pub fn format_unix_mode(mode: u32) -> String {
    let perms = mode & 0o777;
    let mut out = String::with_capacity(9);
    for shift in [6, 3, 0] {
        let bits = (perms >> shift) & 0o7;
        out.push(if bits & 4 != 0 { 'r' } else { '-' });
        out.push(if bits & 2 != 0 { 'w' } else { '-' });
        out.push(if bits & 1 != 0 { 'x' } else { '-' });
    }
    format!("{} ({:03o})", out, perms)
}

/// 解析顶部地址栏的快速直连 URI，支持以下格式：
/// - `sftp://root:pass@192.168.1.10:22/var/www`
/// - `sftp://root@10.0.0.5/srv?jump=ops@bastion.example.com:22`（通过跳板机连接）
/// - `ftps://user:pass@ftp.example.com/public`（显式 FTPS）或 `ftpsi://...`（隐式 FTPS）
/// - `ftp://ftp.example.com`
/// - `webdav://user:pass@nas.local:5005/dav` 或 `webdavs://...`
/// - `s3://accessKey:secretKey@minio.local:9000/my-bucket/prefix`
pub fn parse_quick_connect_uri(raw: &str) -> AppResult<SiteInput> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return Err(AppError::InvalidInput("连接地址不能为空".into()));
    }

    // 未指定协议前缀时默认按 SFTP 处理
    let normalized = if trimmed.contains("://") {
        trimmed.to_string()
    } else {
        format!("sftp://{}", trimmed)
    };

    let url = Url::parse(&normalized)
        .map_err(|e| AppError::InvalidInput(format!("无法解析连接地址: {}", e)))?;

    let scheme = url.scheme().to_ascii_lowercase();
    let (protocol, endpoint_scheme) = match scheme.as_str() {
        "sftp" | "ssh" => (Protocol::Sftp, None),
        "ftps" | "ftpes" => (Protocol::FtpsExplicit, None),
        "ftpsi" => (Protocol::FtpsImplicit, None),
        "ftp" => (Protocol::Ftp, None),
        "webdav" | "http" => (Protocol::Webdav, Some("http")),
        "webdavs" | "davs" | "https" => (Protocol::Webdav, Some("https")),
        "s3" => (Protocol::S3, Some("https")),
        other => {
            return Err(AppError::InvalidInput(format!(
                "不支持的协议前缀 '{}'，支持 sftp/ftps/ftpsi/ftp/s3/webdav/webdavs",
                other
            )));
        }
    };

    let host = url
        .host_str()
        .ok_or_else(|| AppError::InvalidInput("地址中缺少主机名".into()))?
        .to_string();

    let port = url
        .port()
        .unwrap_or_else(|| match (protocol, endpoint_scheme) {
            (Protocol::Webdav, Some("http")) => 80,
            _ => protocol.default_port(),
        });

    let username = percent_encoding::percent_decode_str(url.username())
        .decode_utf8_lossy()
        .to_string();
    let secret = url.password().map(|p| {
        percent_encoding::percent_decode_str(p)
            .decode_utf8_lossy()
            .to_string()
    });
    let path = percent_encoding::percent_decode_str(url.path())
        .decode_utf8_lossy()
        .to_string();

    let (auth_type, endpoint, bucket, remote_path) = match protocol {
        Protocol::S3 => {
            let trimmed_path = path.trim_start_matches('/');
            let mut split = trimmed_path.splitn(2, '/');
            let bkt = split.next().unwrap_or("").to_string();
            let prefix = split.next().unwrap_or("");
            let ep = if port == 443 {
                format!("https://{}", host)
            } else {
                format!("https://{}:{}", host, port)
            };
            (
                AuthType::AccessKey,
                Some(ep),
                if bkt.is_empty() { None } else { Some(bkt) },
                normalize_remote_path(prefix),
            )
        }
        Protocol::Webdav => {
            let sch = endpoint_scheme.unwrap_or("https");
            let ep = if (sch == "https" && port == 443) || (sch == "http" && port == 80) {
                format!("{}://{}", sch, host)
            } else {
                format!("{}://{}:{}", sch, host, port)
            };
            (
                AuthType::Password,
                Some(ep),
                None,
                normalize_remote_path(&path),
            )
        }
        _ => {
            let at = if username.is_empty()
                && secret.is_none()
                && matches!(protocol, Protocol::Ftp | Protocol::FtpsExplicit)
            {
                AuthType::Anonymous
            } else if protocol == Protocol::Sftp && secret.is_none() {
                // SFTP 未提供密码时自动优先尝试本地 SSH Agent
                AuthType::Agent
            } else {
                AuthType::Password
            };
            (at, None, None, normalize_remote_path(&path))
        }
    };

    // 解析可选的 SSH 跳板机查询参数：?jump=user@bastion:22
    let mut jump_host = None;
    let mut jump_port = None;
    let mut jump_username = None;
    let mut jump_auth_type = None;
    for (k, v) in url.query_pairs() {
        if k.eq_ignore_ascii_case("jump") && !v.trim().is_empty() {
            let jump_str = if v.contains("://") {
                v.to_string()
            } else {
                format!("sftp://{}", v)
            };
            if let Ok(ju) = Url::parse(&jump_str) {
                if let Some(jh) = ju.host_str() {
                    jump_host = Some(jh.to_string());
                    jump_port = Some(ju.port().unwrap_or(22));
                    let juser = percent_encoding::percent_decode_str(ju.username())
                        .decode_utf8_lossy()
                        .to_string();
                    jump_username = Some(if juser.is_empty() {
                        username.clone()
                    } else {
                        juser
                    });
                    jump_auth_type = Some(AuthType::Agent);
                }
            }
        }
    }

    Ok(SiteInput {
        id: None,
        name: format!(
            "{}@{}",
            if username.is_empty() {
                "quick"
            } else {
                &username
            },
            host
        ),
        group: "快速连接".to_string(),
        protocol,
        host,
        port: Some(port),
        username,
        auth_type,
        secret,
        passphrase: None,
        session_token: None,
        private_key_path: None,
        remote_path,
        endpoint,
        bucket,
        region: Some("us-east-1".to_string()),
        path_style: protocol == Protocol::S3,
        tls_skip_verify: false,
        jump_host,
        jump_port,
        jump_username,
        jump_auth_type,
        jump_private_key_path: None,
        jump_secret: None,
        jump_passphrase: None,
        notes: String::new(),
        save_secret: false,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_normalize_remote_path() {
        assert_eq!(normalize_remote_path(""), "/");
        assert_eq!(normalize_remote_path("/a//b/../c/"), "/a/c");
        assert_eq!(normalize_remote_path("var\\www\\html"), "/var/www/html");
    }

    #[test]
    fn test_parse_quick_connect_sftp() {
        let site = parse_quick_connect_uri("sftp://admin:p%40ss@10.0.0.2:2222/var/log").unwrap();
        assert_eq!(site.protocol, Protocol::Sftp);
        assert_eq!(site.host, "10.0.0.2");
        assert_eq!(site.port, Some(2222));
        assert_eq!(site.username, "admin");
        assert_eq!(site.secret.as_deref(), Some("p@ss"));
        assert_eq!(site.remote_path, "/var/log");
    }

    #[test]
    fn test_parse_quick_connect_proxy_jump() {
        let site = parse_quick_connect_uri("sftp://root@10.0.0.5/srv?jump=ops@bastion.local:2200")
            .unwrap();
        assert_eq!(site.host, "10.0.0.5");
        assert_eq!(site.auth_type, AuthType::Agent);
        assert_eq!(site.jump_host.as_deref(), Some("bastion.local"));
        assert_eq!(site.jump_port, Some(2200));
        assert_eq!(site.jump_username.as_deref(), Some("ops"));
        assert_eq!(site.jump_auth_type, Some(AuthType::Agent));
    }

    #[test]
    fn test_parse_quick_connect_s3() {
        let site =
            parse_quick_connect_uri("s3://AKIA:SECRET@minio.local:9000/my-bucket/docs").unwrap();
        assert_eq!(site.protocol, Protocol::S3);
        assert_eq!(site.bucket.as_deref(), Some("my-bucket"));
        assert_eq!(site.remote_path, "/docs");
        assert_eq!(site.endpoint.as_deref(), Some("https://minio.local:9000"));
    }
}
