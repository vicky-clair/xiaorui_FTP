//! 远程存储协议抽象层与连接工厂
//!
//! 所有具体协议适配器（SFTP、FTP/FTPS、S3 兼容存储、WebDAV）均实现统一的
//! `RemoteFs` 异步接口，使上层会话管理、文件浏览与并发传输队列完全协议无关。

pub mod ftp;
pub mod s3;
pub mod sftp;
pub mod webdav;

use std::path::Path;
use std::sync::atomic::AtomicBool;
use std::sync::Arc;

use async_trait::async_trait;

use crate::error::{AppError, AppResult};
use crate::known_hosts::KnownHostsStore;
use crate::models::{
    AuthType, FileEntry, Protocol, ServerMetrics, ShellOutput, SiteConfig, SiteSecrets,
};

/// 传输进度回调函数类型：参数依次为 `(已传输字节数, 总字节数)`
pub type ProgressCallback = Arc<dyn Fn(u64, u64) + Send + Sync>;

/// 向交互式远端 PTY 发送的控制指令
pub enum PtyCommand {
    /// 写入原始用户输入字节（含方向键、Tab、Ctrl+C、中文 UTF-8 等）
    Input(Vec<u8>),
    /// 同步终端行列尺寸（触发远端 SIGWINCH）
    Resize { cols: u32, rows: u32 },
    /// 主动关闭 PTY 通道
    Close,
}

/// 已打开的交互式远端 PTY 双向异步流句柄
pub struct PtyHandle {
    pub cmd_tx: tokio::sync::mpsc::Sender<PtyCommand>,
    pub output_rx: tokio::sync::mpsc::Receiver<Vec<u8>>,
}

/// 已启动的 SSH 本地端口转发隧道运行时句柄
pub struct TunnelRuntimeHandle {
    /// 实际绑定的本地回环端口（`127.0.0.1:<local_port>`）
    pub local_port: u16,
    /// 当前活跃并发转发连接计数
    pub active_connections: Arc<std::sync::atomic::AtomicU32>,
    /// 发送关闭信号以立即停止监听与转发
    pub shutdown_tx: tokio::sync::oneshot::Sender<()>,
}

/// 统一异步远程文件系统抽象接口
#[async_trait]
pub trait RemoteFs: Send + Sync {
    /// 列举指定远程目录下的文件与子目录
    async fn list_dir(&self, path: &str) -> AppResult<Vec<FileEntry>>;

    /// 创建远程目录
    async fn mkdir(&self, path: &str) -> AppResult<()>;

    /// 删除远程文件或目录
    async fn remove(&self, path: &str, is_dir: bool) -> AppResult<()>;

    /// 重命名或移动远程文件/目录
    async fn rename(&self, from: &str, to: &str) -> AppResult<()>;

    /// 修改远程文件 Unix 权限（不支持的协议默认返回 Unsupported 错误）
    async fn chmod(&self, _path: &str, _mode: u32) -> AppResult<()> {
        Err(AppError::Unsupported(
            "当前协议不支持修改 Unix 权限 (CHMOD)".into(),
        ))
    }

    /// 读取远程文件字节内容（受 `max_bytes` 大小保护，用于在线文本编辑）
    async fn read_bytes(&self, path: &str, max_bytes: usize) -> AppResult<Vec<u8>>;

    /// 将字节内容写入指定远程文件（覆盖写入）
    async fn write_bytes(&self, path: &str, data: &[u8]) -> AppResult<()>;

    /// 流式下载远程文件到本地，支持进度上报与取消标记
    async fn download_file(
        &self,
        remote_path: &str,
        local_path: &Path,
        progress: ProgressCallback,
        cancel: Arc<AtomicBool>,
    ) -> AppResult<u64>;

    /// 流式上传本地文件到远程，支持进度上报与取消标记
    async fn upload_file(
        &self,
        local_path: &Path,
        remote_path: &str,
        progress: ProgressCallback,
        cancel: Arc<AtomicBool>,
    ) -> AppResult<u64>;

    /// 执行远程 Shell 命令（仅 SFTP/SSH 会话支持）
    async fn exec_shell(&self, _command: &str, _cwd: Option<&str>) -> AppResult<ShellOutput> {
        Err(AppError::Unsupported(
            "仅 SFTP/SSH 会话支持远程命令终端".into(),
        ))
    }

    /// 采集远端服务器硬件规格、资源占用与多磁盘挂载列表（仅 SFTP/SSH 会话支持）
    async fn server_metrics(&self) -> AppResult<ServerMetrics> {
        Err(AppError::Unsupported(
            "仅 SFTP/SSH 会话支持采集远端硬件规格与磁盘信息".into(),
        ))
    }

    /// 打开交互式远端 PTY 伪终端并启动服务器默认登录 Shell（如 zsh / bash / fish / PowerShell）
    async fn open_pty(&self, _cols: u32, _rows: u32) -> AppResult<PtyHandle> {
        Err(AppError::Unsupported(
            "仅 SFTP/SSH 会话支持交互式伪终端 (PTY)".into(),
        ))
    }

    /// 通过纯 SFTP 通道将合法 OpenSSH 公钥部署到远端 `~/.ssh/authorized_keys`（幂等去重，零 Shell 注入风险）。
    /// 返回 `Ok(true)` 表示已新增写入，`Ok(false)` 表示该公钥已存在无需重复部署。
    async fn deploy_public_key(&self, _public_key_line: &str) -> AppResult<bool> {
        Err(AppError::Unsupported(
            "仅 SFTP/SSH 会话支持一键部署 SSH 公钥".into(),
        ))
    }

    /// 通过纯 SFTP 读取远端用户的真实 Shell 历史记录（Zsh / Bash / Fish / PowerShell），已脱敏去重
    async fn read_shell_history(&self, _limit: usize) -> AppResult<Vec<String>> {
        Err(AppError::Unsupported(
            "仅 SFTP/SSH 会话支持读取远端 Shell 历史记录".into(),
        ))
    }

    /// 在本机 `127.0.0.1:<local_port>` 启动 SSH 本地端口转发隧道（`-L local_port:remote_host:remote_port`）
    async fn start_local_forward(
        &self,
        _local_port: u16,
        _remote_host: String,
        _remote_port: u16,
    ) -> AppResult<TunnelRuntimeHandle> {
        Err(AppError::Unsupported(
            "仅 SFTP/SSH 会话支持端口转发隧道".into(),
        ))
    }
}

/// 根据站点配置与密钥分发并建立对应的远程协议连接
pub async fn connect_protocol(
    site: SiteConfig,
    secrets: SiteSecrets,
    known_hosts: Arc<KnownHostsStore>,
) -> AppResult<Arc<dyn RemoteFs>> {
    match site.protocol {
        Protocol::Sftp => {
            // 若配置了跳板机主机，则构造 ProxyJump 配置
            let jump = site
                .jump_host
                .as_deref()
                .filter(|h| !h.trim().is_empty())
                .map(|jh| sftp::JumpHostConfig {
                    host: jh.trim().to_string(),
                    port: site.jump_port.unwrap_or(22),
                    username: site
                        .jump_username
                        .clone()
                        .unwrap_or_else(|| site.username.clone()),
                    auth_type: site.jump_auth_type.unwrap_or(AuthType::Agent),
                    secret: secrets.jump_secret.clone(),
                    private_key_path: site.jump_private_key_path.clone(),
                    passphrase: secrets.jump_passphrase.clone(),
                });

            let client = tokio::spawn(sftp::SftpClient::connect(
                site.host.clone(),
                site.port,
                site.username.clone(),
                site.auth_type,
                secrets.secret.clone(),
                site.private_key_path.clone(),
                secrets.passphrase.clone(),
                jump,
                known_hosts,
            ))
            .await
            .map_err(|e| AppError::Connection(format!("SFTP 连接任务异常: {}", e)))??;
            Ok(Arc::new(client))
        }
        Protocol::Ftp | Protocol::FtpsExplicit | Protocol::FtpsImplicit => {
            let client = ftp::FtpClient::connect(
                &site.host,
                site.port,
                &site.username,
                secrets.secret.as_deref(),
                site.protocol,
                site.tls_skip_verify,
            )
            .await?;
            Ok(Arc::new(client))
        }
        Protocol::S3 => {
            let endpoint = site
                .endpoint
                .as_deref()
                .filter(|s| !s.trim().is_empty())
                .unwrap_or(&site.host)
                .to_string();
            let bucket = site
                .bucket
                .as_deref()
                .ok_or_else(|| AppError::InvalidInput("请填写 S3 Bucket 名称".into()))?
                .to_string();
            let region = site.region.as_deref().unwrap_or("us-east-1").to_string();
            let secret_key = secrets.secret.as_deref().unwrap_or("").to_string();
            let client = s3::S3Client::connect(
                &endpoint,
                &bucket,
                &region,
                &site.username,
                &secret_key,
                secrets.session_token.clone(),
                site.path_style,
                site.tls_skip_verify,
            )
            .await?;
            Ok(Arc::new(client))
        }
        Protocol::Webdav => {
            let endpoint = site
                .endpoint
                .as_deref()
                .filter(|s| !s.trim().is_empty())
                .map(|s| s.to_string())
                .unwrap_or_else(|| {
                    if site.port == 80 {
                        format!("http://{}", site.host)
                    } else if site.port == 443 {
                        format!("https://{}", site.host)
                    } else {
                        format!("https://{}:{}", site.host, site.port)
                    }
                });
            let client = webdav::WebDavClient::connect(
                &endpoint,
                &site.username,
                secrets.secret.as_deref(),
                site.auth_type,
                site.tls_skip_verify,
            )
            .await?;
            Ok(Arc::new(client))
        }
    }
}
