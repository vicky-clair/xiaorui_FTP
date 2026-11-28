//! 现代 SFTP / SSH 协议适配器（基于纯 Rust `russh` + `russh-sftp`）
//!
//! 特性：
//! - 支持 Curve25519 / Ed25519 / RSA-SHA2-256 / ECDSA / AES-GCM / ChaCha20-Poly1305；
//! - 支持密码、私钥文件（OpenSSH / PEM 及可选口令）、本地 SSH Agent（Windows OpenSSH Agent / Pageant / Unix `SSH_AUTH_SOCK`）；
//! - 支持纯 Rust SSH `ProxyJump` 跳板机隧道（通过 `direct-tcpip` 转发，零外部 `ssh.exe` 依赖）；
//! - 支持 SFTP 文件操作、CHMOD 权限修改以及内置 SSH 远程命令执行。

use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use russh::client::{self, Config, Handle};
use russh::keys::agent::client::{AgentClient, AgentStream};
use russh::keys::key::PrivateKeyWithHashAlg;
use russh::keys::{load_secret_key, HashAlg, PublicKey};
use russh::ChannelMsg;
use russh_sftp::client::SftpSession;
use russh_sftp::protocol::{FileAttributes, OpenFlags};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::sync::Mutex;

use crate::error::{AppError, AppResult};
use crate::known_hosts::{HostKeyCheckError, KnownHostsStore};
use crate::models::{
    format_unix_mode, join_remote_path, normalize_remote_path, AgentStatus, AuthType, FileEntry,
    RemoteDisk, ServerMetrics, ShellOutput,
};
use crate::protocols::{ProgressCallback, PtyCommand, PtyHandle, RemoteFs};
use crate::security::{
    commit_staging_file, ensure_not_symlink_target, staging_part_path,
    validate_remote_path_no_control,
};

/// 将主机密钥校验失败转换为结构化 `AppError`，供前端展示首次确认弹窗或阻断性中间人告警
fn map_host_key_error(err: HostKeyCheckError) -> AppError {
    match err {
        HostKeyCheckError::Unknown(u) => {
            let payload = serde_json::to_string(&u).unwrap_or_else(|_| u.host_port.clone());
            AppError::HostKeyUnknown(payload)
        }
        HostKeyCheckError::Mismatch(m) => {
            let payload = serde_json::to_string(&m).unwrap_or_else(|_| m.to_string());
            AppError::HostKeyMismatch(payload)
        }
        HostKeyCheckError::StorageCorrupted(msg) => AppError::SecurityPolicy(msg),
    }
}

/// SSH 客户端事件回调处理器（严格校验主机公钥指纹：未知主机需显式确认，变更立即阻断）
struct ClientHandler {
    host_port: String,
    is_jump_host: bool,
    known_hosts: Arc<KnownHostsStore>,
    host_key_error: Arc<std::sync::Mutex<Option<HostKeyCheckError>>>,
}

impl ClientHandler {
    fn new(
        host_port: String,
        is_jump_host: bool,
        known_hosts: Arc<KnownHostsStore>,
        host_key_error: Arc<std::sync::Mutex<Option<HostKeyCheckError>>>,
    ) -> Self {
        Self {
            host_port,
            is_jump_host,
            known_hosts,
            host_key_error,
        }
    }
}

#[async_trait]
impl client::Handler for ClientHandler {
    type Error = russh::Error;

    /// 严格校验远程服务器主机公钥（绝不静默接受未知主机或变更密钥）
    async fn check_server_key(
        &mut self,
        server_public_key: &PublicKey,
    ) -> Result<bool, Self::Error> {
        let fingerprint = server_public_key.fingerprint(HashAlg::Sha256).to_string();
        let algorithm = server_public_key.algorithm().to_string();
        match self.known_hosts.verify_strict(
            &self.host_port,
            &algorithm,
            &fingerprint,
            self.is_jump_host,
        ) {
            Ok(()) => Ok(true),
            Err(check_err) => {
                if let Ok(mut slot) = self.host_key_error.lock() {
                    *slot = Some(check_err);
                }
                Ok(false)
            }
        }
    }
}

/// SSH 跳板机（ProxyJump / Bastion）连接参数（手写脱敏 Debug 并在 Drop 时清零敏感字段）
#[derive(Clone)]
pub struct JumpHostConfig {
    pub host: String,
    pub port: u16,
    pub username: String,
    pub auth_type: AuthType,
    pub secret: Option<String>,
    pub private_key_path: Option<String>,
    pub passphrase: Option<String>,
}

impl std::fmt::Debug for JumpHostConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("JumpHostConfig")
            .field("host", &self.host)
            .field("port", &self.port)
            .field("username", &self.username)
            .field("auth_type", &self.auth_type)
            .field("secret", &self.secret.as_ref().map(|_| "[REDACTED]"))
            .field(
                "passphrase",
                &self.passphrase.as_ref().map(|_| "[REDACTED]"),
            )
            .finish()
    }
}

impl Drop for JumpHostConfig {
    fn drop(&mut self) {
        use zeroize::Zeroize;
        if let Some(s) = self.secret.as_mut() {
            s.zeroize();
        }
        if let Some(p) = self.passphrase.as_mut() {
            p.zeroize();
        }
    }
}

/// 在 Windows 平台上安全连接 Pageant（PuTTY / KeePassXC / 1Password）
///
/// 绕过上游 `pageant 0.0.1` 中 `PageantStream::new()` 在未运行 Pageant 时调用 `.unwrap()` 导致进程崩溃的缺陷：
/// 1. 先通过 `pageant::is_pageant_running()` 探测窗口句柄，未运行时直接返回 `None`；
/// 2. 实际 Win32 通信放在 `spawn_blocking` 中执行，任何异常均优雅退出循环，绝不 panic。
#[cfg(windows)]
fn connect_safe_pageant() -> Option<AgentClient<tokio::io::DuplexStream>> {
    if !pageant::is_pageant_running() {
        return None;
    }

    let (client_stream, mut server_stream) = tokio::io::duplex(8192 * 16);
    let cookie = uuid::Uuid::new_v4().simple().to_string();

    tokio::spawn(async move {
        let mut len_buf = [0u8; 4];
        while server_stream.read_exact(&mut len_buf).await.is_ok() {
            let msg_len = u32::from_be_bytes(len_buf) as usize;
            if msg_len == 0 || msg_len > 8192 - 4 {
                break;
            }
            let mut msg = vec![0u8; 4 + msg_len];
            msg[..4].copy_from_slice(&len_buf);
            if server_stream.read_exact(&mut msg[4..]).await.is_err() {
                break;
            }

            let cookie_owned = cookie.clone();
            let resp = tokio::task::spawn_blocking(move || {
                pageant::query_pageant_direct(cookie_owned, &msg)
            })
            .await
            .ok()
            .and_then(|r| r.ok());

            let Some(response) = resp else {
                break;
            };
            if server_stream.write_all(&response).await.is_err() {
                break;
            }
        }
    });

    Some(AgentClient::connect(client_stream))
}

/// 检测本地平台 SSH Agent 服务可用性与已加载密钥数量
pub async fn check_ssh_agent_status() -> AgentStatus {
    #[cfg(windows)]
    {
        // 优先尝试 Windows 内置 OpenSSH Authentication Agent 命名管道
        if let Ok(mut pipe_agent) =
            AgentClient::connect_named_pipe(r"\\.\pipe\openssh-ssh-agent").await
        {
            return match pipe_agent.request_identities().await {
                Ok(keys) => AgentStatus {
                    available: true,
                    backend: "Windows OpenSSH Agent".to_string(),
                    key_count: keys.len(),
                    message: if keys.is_empty() {
                        "已连接 Windows OpenSSH Agent，但尚未加载任何密钥（请执行 ssh-add）"
                            .to_string()
                    } else {
                        format!(
                            "已连接 Windows OpenSSH Agent，检测到 {} 个可用密钥",
                            keys.len()
                        )
                    },
                },
                Err(e) => AgentStatus {
                    available: false,
                    backend: "Windows OpenSSH Agent".to_string(),
                    key_count: 0,
                    message: format!("读取 Windows OpenSSH Agent 密钥失败: {}", e),
                },
            };
        }

        // 若未启用 OpenSSH Agent，安全回退检测 Pageant（PuTTY / KeePassXC / 1Password）
        if let Some(mut pageant_agent) = connect_safe_pageant() {
            if let Ok(keys) = pageant_agent.request_identities().await {
                return AgentStatus {
                    available: true,
                    backend: "Pageant".to_string(),
                    key_count: keys.len(),
                    message: if keys.is_empty() {
                        "已连接 Pageant，但尚未加载任何密钥".to_string()
                    } else {
                        format!("已连接 Pageant，检测到 {} 个可用密钥", keys.len())
                    },
                };
            }
        }

        AgentStatus {
            available: false,
            backend: "none".to_string(),
            key_count: 0,
            message: "未检测到正在运行的本地 SSH Agent（请启动 Windows OpenSSH Authentication Agent 服务或 Pageant）".to_string(),
        }
    }

    #[cfg(unix)]
    {
        match AgentClient::connect_env().await {
            Ok(mut agent) => match agent.request_identities().await {
                Ok(keys) => AgentStatus {
                    available: true,
                    backend: "Unix ssh-agent".to_string(),
                    key_count: keys.len(),
                    message: if keys.is_empty() {
                        "已连接 ssh-agent，但尚未加载任何密钥（请执行 ssh-add）".to_string()
                    } else {
                        format!("已连接 ssh-agent，检测到 {} 个可用密钥", keys.len())
                    },
                },
                Err(e) => AgentStatus {
                    available: false,
                    backend: "Unix ssh-agent".to_string(),
                    key_count: 0,
                    message: format!("读取 ssh-agent 密钥列表失败: {}", e),
                },
            },
            Err(e) => AgentStatus {
                available: false,
                backend: "none".to_string(),
                key_count: 0,
                message: format!("无法连接本地 ssh-agent (SSH_AUTH_SOCK): {}", e),
            },
        }
    }
}

/// 逐一尝试本地 Agent 中托管的公钥进行 SSH 认证
async fn try_agent_auth<S>(
    session: &mut Handle<ClientHandler>,
    username: &str,
    mut agent: AgentClient<S>,
    backend: &str,
) -> AppResult<bool>
where
    S: AgentStream + Send + Unpin + 'static,
{
    let identities = agent
        .request_identities()
        .await
        .map_err(|e| AppError::Auth(format!("从 {} 获取密钥列表失败: {}", backend, e)))?;
    if identities.is_empty() {
        return Err(AppError::Auth(format!(
            "{} 中没有可用密钥，请先运行 `ssh-add` 添加私钥",
            backend
        )));
    }
    for key in identities {
        if let Ok(true) = session
            .authenticate_publickey_with(username.to_string(), key, &mut agent)
            .await
        {
            return Ok(true);
        }
    }
    Ok(false)
}

/// 按当前操作系统选择对应的本地 SSH Agent 完成认证
async fn authenticate_with_local_agent(
    session: &mut Handle<ClientHandler>,
    username: &str,
) -> AppResult<bool> {
    #[cfg(windows)]
    {
        if let Ok(pipe_agent) = AgentClient::connect_named_pipe(r"\\.\pipe\openssh-ssh-agent").await
        {
            return try_agent_auth(session, username, pipe_agent, "Windows OpenSSH Agent").await;
        }
        let pageant_agent = connect_safe_pageant().ok_or_else(|| {
            AppError::Auth(
                "未检测到正在运行的本地 SSH Agent（请启动 Windows OpenSSH Authentication Agent 服务或 Pageant）"
                    .to_string(),
            )
        })?;
        try_agent_auth(session, username, pageant_agent, "Pageant").await
    }

    #[cfg(unix)]
    {
        let agent = AgentClient::connect_env().await.map_err(|e| {
            AppError::Auth(format!(
                "无法连接本地 ssh-agent（请确认 SSH_AUTH_SOCK 已设置且 ssh-agent 正在运行）: {}",
                e
            ))
        })?;
        try_agent_auth(session, username, agent, "Unix ssh-agent").await
    }
}

/// 对已建立的 SSH 句柄执行指定方式的身份认证（同时供跳板机与目标主机复用）
async fn authenticate_handle(
    mut session: Handle<ClientHandler>,
    target_label: String,
    username: String,
    auth_type: AuthType,
    password: Option<String>,
    private_key_path: Option<String>,
    passphrase: Option<String>,
) -> AppResult<Handle<ClientHandler>> {
    let authenticated = match auth_type {
        AuthType::Agent => authenticate_with_local_agent(&mut session, &username).await?,
        AuthType::PrivateKey => {
            // 在同步块内加载私钥，避免临时路径引用跨 `.await` 影响 Future Send 推导
            let key_with_alg = {
                let key_path = private_key_path
                    .as_deref()
                    .filter(|s| !s.trim().is_empty())
                    .ok_or_else(|| {
                        AppError::InvalidInput(format!("请为 {} 选择 SSH 私钥文件", target_label))
                    })?;
                let key_pair =
                    load_secret_key(key_path, passphrase.as_deref().filter(|s| !s.is_empty()))
                        .map_err(|e| {
                            AppError::Auth(format!("加载 {} SSH 私钥失败: {}", target_label, e))
                        })?;
                // RSA 私钥自动启用现代 rsa-sha2-256，兼容 OpenSSH 8.8+/9.x
                let rsa_hash = if key_pair.algorithm().is_rsa() {
                    Some(HashAlg::Sha256)
                } else {
                    None
                };
                PrivateKeyWithHashAlg::new(Arc::new(key_pair), rsa_hash)
                    .map_err(|e| AppError::Auth(format!("准备私钥签名算法失败: {}", e)))?
            };
            session
                .authenticate_publickey(username.clone(), key_with_alg)
                .await
                .map_err(|e| AppError::Auth(format!("{} 公钥认证失败: {}", target_label, e)))?
        }
        _ => {
            let pwd = password.unwrap_or_default();
            session
                .authenticate_password(username.clone(), pwd)
                .await
                .map_err(|e| AppError::Auth(format!("{} 密码认证失败: {}", target_label, e)))?
        }
    };

    if !authenticated {
        return Err(AppError::Auth(format!(
            "{} 认证被服务器拒绝，请检查用户名、密码、私钥或 ssh-agent 密钥",
            target_label
        )));
    }
    Ok(session)
}

/// 在阻塞线程池中解析主机名与端口为 `SocketAddr`
async fn resolve_socket_addr(host: String, port: u16) -> AppResult<std::net::SocketAddr> {
    tokio::task::spawn_blocking(move || {
        use std::net::ToSocketAddrs;
        let mut addrs = (host.as_str(), port)
            .to_socket_addrs()
            .map_err(|e| AppError::Connection(format!("解析主机 {}:{} 失败: {}", host, port, e)))?;
        addrs.next().ok_or_else(|| {
            AppError::Connection(format!("无法解析主机 {}:{} 的网络地址", host, port))
        })
    })
    .await
    .map_err(|e| AppError::Connection(format!("DNS 解析任务异常: {}", e)))?
}

/// SFTP 客户端实例
pub struct SftpClient {
    /// 持有跳板机 SSH 会话句柄，确保整个目标会话生命周期内转发隧道不中断
    _jump_ssh: Option<Handle<ClientHandler>>,
    /// 目标主机 SSH 会话句柄（供内置命令终端按需打开 exec 通道）
    ssh: Arc<Mutex<Handle<ClientHandler>>>,
    /// 已启动的 SFTP 子系统会话
    sftp: Arc<SftpSession>,
}

impl SftpClient {
    /// 建立 SFTP 连接（支持直连或经由 SSH 跳板机隧道连接，均启用 TOFU 主机指纹校验）
    #[allow(clippy::too_many_arguments)]
    pub async fn connect(
        host: String,
        port: u16,
        username: String,
        auth_type: AuthType,
        password: Option<String>,
        private_key_path: Option<String>,
        passphrase: Option<String>,
        jump: Option<JumpHostConfig>,
        known_hosts: Arc<KnownHostsStore>,
    ) -> AppResult<Self> {
        let target_host = host.trim().to_string();
        let target_user = username.trim().to_string();
        if target_host.is_empty() {
            return Err(AppError::InvalidInput("SFTP 主机地址不能为空".into()));
        }
        if target_user.is_empty() {
            return Err(AppError::InvalidInput("SFTP 用户名不能为空".into()));
        }

        // 配置心跳保活，防止长连接被防火墙或 NAT 断开
        let config = Arc::new(Config {
            inactivity_timeout: Some(Duration::from_secs(300)),
            keepalive_interval: Some(Duration::from_secs(30)),
            keepalive_max: 3,
            ..Default::default()
        });

        let target_host_port = format!("{}:{}", target_host, port);
        let target_key_err: Arc<std::sync::Mutex<Option<HostKeyCheckError>>> =
            Arc::new(std::sync::Mutex::new(None));

        let (jump_handle, session) = if let Some(mut jh) = jump {
            let jump_host = jh.host.trim().to_string();
            let jump_host_port = format!("{}:{}", jump_host, jh.port);
            let jump_sock = resolve_socket_addr(jump_host.clone(), jh.port).await?;
            let jump_user = if jh.username.trim().is_empty() {
                target_user.clone()
            } else {
                jh.username.trim().to_string()
            };

            // 1. 连接并认证跳板机（连接与认证均带 20 秒超时保护，独立严格校验跳板机主机指纹）
            let jump_key_err: Arc<std::sync::Mutex<Option<HostKeyCheckError>>> =
                Arc::new(std::sync::Mutex::new(None));
            let jump_handler = ClientHandler::new(
                jump_host_port,
                true,
                known_hosts.clone(),
                jump_key_err.clone(),
            );
            let jump_sess = tokio::time::timeout(
                Duration::from_secs(20),
                client::connect(config.clone(), jump_sock, jump_handler),
            )
            .await
            .map_err(|_| {
                AppError::Connection(format!(
                    "连接 SSH 跳板机 {}:{} 超时（20 秒）",
                    jump_host, jh.port
                ))
            })?
            .map_err(|e| {
                if let Ok(Some(hk_err)) = jump_key_err.lock().map(|mut g| g.take()) {
                    return map_host_key_error(hk_err);
                }
                AppError::Connection(format!(
                    "无法连接 SSH 跳板机 {}:{}: {}",
                    jump_host, jh.port, e
                ))
            })?;

            let jump_secret = jh.secret.take();
            let jump_key_path = jh.private_key_path.take();
            let jump_passphrase = jh.passphrase.take();
            let jump_sess = tokio::time::timeout(
                Duration::from_secs(20),
                tokio::spawn(authenticate_handle(
                    jump_sess,
                    format!("跳板机 ({})", jump_host),
                    jump_user,
                    jh.auth_type,
                    jump_secret,
                    jump_key_path,
                    jump_passphrase,
                )),
            )
            .await
            .map_err(|_| {
                AppError::Auth(format!(
                    "SSH 跳板机 {}:{} 认证超时（20 秒）",
                    jump_host, jh.port
                ))
            })?
            .map_err(|e| AppError::Auth(format!("跳板机认证任务异常: {}", e)))??;

            // 2. 通过跳板机打开到目标主机的直连 TCP 隧道
            let tunnel = tokio::time::timeout(
                Duration::from_secs(20),
                jump_sess.channel_open_direct_tcpip(
                    target_host.clone(),
                    port as u32,
                    String::from("127.0.0.1"),
                    0,
                ),
            )
            .await
            .map_err(|_| {
                AppError::Connection(format!(
                    "跳板机 {} 打开到 {}:{} 的直连隧道超时（20 秒）",
                    jump_host, target_host, port
                ))
            })?
            .map_err(|e| {
                AppError::Connection(format!(
                    "跳板机 {} 无法建立到 {}:{} 的直连隧道: {}",
                    jump_host, target_host, port, e
                ))
            })?;

            // 3. 在隧道流之上建立目标主机 SSH 加密连接并严格校验目标主机指纹
            let target_handler =
                ClientHandler::new(target_host_port, false, known_hosts, target_key_err.clone());
            let target_sess = tokio::time::timeout(
                Duration::from_secs(20),
                client::connect_stream(config, tunnel.into_stream(), target_handler),
            )
            .await
            .map_err(|_| {
                AppError::Connection(format!(
                    "经由跳板机连接目标主机 {}:{} 超时（20 秒）",
                    target_host, port
                ))
            })?
            .map_err(|e| {
                if let Ok(Some(hk_err)) = target_key_err.lock().map(|mut g| g.take()) {
                    return map_host_key_error(hk_err);
                }
                AppError::Connection(format!(
                    "经由跳板机连接目标主机 {}:{} 失败: {}",
                    target_host, port, e
                ))
            })?;

            (Some(jump_sess), target_sess)
        } else {
            let target_sock = resolve_socket_addr(target_host.clone(), port).await?;
            let target_handler =
                ClientHandler::new(target_host_port, false, known_hosts, target_key_err.clone());
            let direct_sess = tokio::time::timeout(
                Duration::from_secs(20),
                client::connect(config, target_sock, target_handler),
            )
            .await
            .map_err(|_| {
                AppError::Connection(format!("连接 SSH {}:{} 超时（20 秒）", target_host, port))
            })?
            .map_err(|e| {
                if let Ok(Some(hk_err)) = target_key_err.lock().map(|mut g| g.take()) {
                    return map_host_key_error(hk_err);
                }
                AppError::Connection(format!("无法连接 SSH {}:{}: {}", target_host, port, e))
            })?;
            (None, direct_sess)
        };

        // 认证目标主机（独立 20 秒超时保护）
        let session = tokio::time::timeout(
            Duration::from_secs(20),
            tokio::spawn(authenticate_handle(
                session,
                format!("目标主机 ({})", target_host),
                target_user,
                auth_type,
                password,
                private_key_path,
                passphrase,
            )),
        )
        .await
        .map_err(|_| {
            AppError::Auth(format!(
                "SSH {}:{} 身份认证超时（20 秒）",
                target_host, port
            ))
        })?
        .map_err(|e| AppError::Auth(format!("SSH 认证任务异常: {}", e)))??;

        // 请求启动 SFTP 子系统（独立 20 秒超时保护）
        let sftp = tokio::time::timeout(Duration::from_secs(20), async {
            let channel = session
                .channel_open_session()
                .await
                .map_err(|e| AppError::Protocol(format!("打开 SFTP 通道失败: {}", e)))?;
            channel
                .request_subsystem(true, String::from("sftp"))
                .await
                .map_err(|e| AppError::Protocol(format!("启动 SFTP 子系统失败: {}", e)))?;

            SftpSession::new(channel.into_stream())
                .await
                .map_err(|e| AppError::Protocol(format!("初始化 SFTP 会话失败: {}", e)))
        })
        .await
        .map_err(|_| AppError::Protocol("初始化 SFTP 子系统超时（20 秒）".into()))??;

        Ok(Self {
            _jump_ssh: jump_handle,
            ssh: Arc::new(Mutex::new(session)),
            sftp: Arc::new(sftp),
        })
    }

    /// 递归删除远程目录及其所有子项
    fn remove_dir_recursive<'a>(
        &'a self,
        dir_path: &'a str,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = AppResult<()>> + Send + 'a>> {
        Box::pin(async move {
            let children = self.list_dir(dir_path).await?;
            for child in children {
                if child.is_dir && !child.is_symlink {
                    self.remove_dir_recursive(&child.path).await?;
                } else {
                    self.sftp.remove_file(&child.path).await.map_err(|e| {
                        AppError::Protocol(format!("删除 '{}' 失败: {}", child.path, e))
                    })?;
                }
            }
            self.sftp
                .remove_dir(dir_path)
                .await
                .map_err(|e| AppError::Protocol(format!("删除目录 '{}' 失败: {}", dir_path, e)))?;
            Ok(())
        })
    }
}

#[async_trait]
impl RemoteFs for SftpClient {
    async fn list_dir(&self, path: &str) -> AppResult<Vec<FileEntry>> {
        validate_remote_path_no_control(path)?;
        let norm = normalize_remote_path(path);
        // 兼容 Windows OpenSSH SFTP 盘符根目录（如 `/D:` -> `/D:/`）
        let is_win_drive_root = norm.len() == 3
            && norm.starts_with('/')
            && norm.as_bytes()[1].is_ascii_alphabetic()
            && norm.as_bytes()[2] == b':';
        let query_path = if is_win_drive_root {
            format!("{}/", norm)
        } else {
            norm.clone()
        };
        let dir = self
            .sftp
            .read_dir(&query_path)
            .await
            .map_err(|e| AppError::Protocol(format!("读取目录 '{}' 失败: {}", norm, e)))?;

        let mut entries = Vec::new();
        for entry in dir {
            let name = entry.file_name();
            if name == "." || name == ".." {
                continue;
            }
            let meta = entry.metadata();
            let file_type = entry.file_type();
            let is_symlink = file_type.is_symlink();
            let full_path = join_remote_path(&norm, &name);
            // 指向目录的软链接在浏览时按目录展示（仍保留 is_symlink=true 避免递归删除/下载陷入死循环）
            let is_dir = if is_symlink {
                self.sftp
                    .metadata(&full_path)
                    .await
                    .map(|m| m.is_dir())
                    .unwrap_or(false)
            } else {
                file_type.is_dir()
            };
            let mode = meta.permissions;
            let permissions = mode.map(format_unix_mode);
            let modified = meta.mtime.map(|m| m as i64);
            let size = if is_dir { 0 } else { meta.size.unwrap_or(0) };

            entries.push(FileEntry {
                path: full_path,
                name,
                is_dir,
                size,
                modified,
                permissions,
                mode: mode.map(|m| m & 0o777),
                is_symlink,
            });
        }

        entries.sort_by(|a, b| {
            b.is_dir
                .cmp(&a.is_dir)
                .then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase()))
        });
        Ok(entries)
    }

    async fn mkdir(&self, path: &str) -> AppResult<()> {
        validate_remote_path_no_control(path)?;
        let norm = normalize_remote_path(path);
        self.sftp
            .create_dir(&norm)
            .await
            .map_err(|e| AppError::Protocol(format!("创建目录 '{}' 失败: {}", norm, e)))?;
        Ok(())
    }

    async fn remove(&self, path: &str, is_dir: bool) -> AppResult<()> {
        validate_remote_path_no_control(path)?;
        let norm = normalize_remote_path(path);
        if norm == "/" {
            return Err(AppError::SecurityPolicy("禁止删除远程根目录 '/'".into()));
        }
        // 不盲信前端传入的 is_dir：先在服务端通过 lstat (symlink_metadata) 核实真实类型，
        // 若目标本身是符号链接则仅删除链接本身，绝不跟随递归删除链接指向的真实目录！
        let actual_is_dir = match self.sftp.symlink_metadata(&norm).await {
            Ok(meta) => {
                if meta.is_symlink() {
                    false
                } else {
                    meta.is_dir()
                }
            }
            Err(_) => is_dir,
        };
        if actual_is_dir {
            self.remove_dir_recursive(&norm).await?;
        } else {
            self.sftp
                .remove_file(&norm)
                .await
                .map_err(|e| AppError::Protocol(format!("删除文件 '{}' 失败: {}", norm, e)))?;
        }
        Ok(())
    }

    async fn rename(&self, from: &str, to: &str) -> AppResult<()> {
        validate_remote_path_no_control(from)?;
        validate_remote_path_no_control(to)?;
        let src = normalize_remote_path(from);
        let dst = normalize_remote_path(to);
        self.sftp.rename(&src, &dst).await.map_err(|e| {
            AppError::Protocol(format!("重命名 '{}' -> '{}' 失败: {}", src, dst, e))
        })?;
        Ok(())
    }

    async fn chmod(&self, path: &str, mode: u32) -> AppResult<()> {
        validate_remote_path_no_control(path)?;
        let norm = normalize_remote_path(path);
        let attrs = FileAttributes {
            permissions: Some(mode & 0o7777),
            ..Default::default()
        };
        self.sftp
            .set_metadata(&norm, attrs)
            .await
            .map_err(|e| AppError::Protocol(format!("修改权限 '{}' 失败: {}", norm, e)))?;
        Ok(())
    }

    async fn read_bytes(&self, path: &str, max_bytes: usize) -> AppResult<Vec<u8>> {
        validate_remote_path_no_control(path)?;
        let norm = normalize_remote_path(path);
        let meta = self
            .sftp
            .metadata(&norm)
            .await
            .map_err(|e| AppError::Protocol(format!("获取文件属性失败: {}", e)))?;
        if meta.size.unwrap_or(0) > max_bytes as u64 {
            return Err(AppError::InvalidInput(format!(
                "文件超过在线编辑限制 ({} KB)",
                max_bytes / 1024
            )));
        }
        let mut file = self
            .sftp
            .open(&norm)
            .await
            .map_err(|e| AppError::Protocol(format!("打开远程文件失败: {}", e)))?;
        let mut buf = Vec::new();
        file.read_to_end(&mut buf)
            .await
            .map_err(|e| AppError::Protocol(format!("读取远程文件失败: {}", e)))?;
        Ok(buf)
    }

    async fn write_bytes(&self, path: &str, data: &[u8]) -> AppResult<()> {
        validate_remote_path_no_control(path)?;
        let norm = normalize_remote_path(path);
        let mut file = self
            .sftp
            .open_with_flags(
                &norm,
                OpenFlags::CREATE | OpenFlags::TRUNCATE | OpenFlags::WRITE,
            )
            .await
            .map_err(|e| AppError::Protocol(format!("打开远程文件写入失败: {}", e)))?;
        file.write_all(data)
            .await
            .map_err(|e| AppError::Protocol(format!("写入远程文件失败: {}", e)))?;
        file.flush()
            .await
            .map_err(|e| AppError::Protocol(format!("刷新远程文件失败: {}", e)))?;
        Ok(())
    }

    async fn download_file(
        &self,
        remote_path: &str,
        local_path: &Path,
        progress: ProgressCallback,
        cancel: Arc<AtomicBool>,
    ) -> AppResult<u64> {
        validate_remote_path_no_control(remote_path)?;
        ensure_not_symlink_target(local_path)?;
        let norm = normalize_remote_path(remote_path);
        let total = self
            .sftp
            .metadata(&norm)
            .await
            .ok()
            .and_then(|m| m.size)
            .unwrap_or(0);

        if let Some(parent) = local_path.parent() {
            tokio::fs::create_dir_all(parent).await?;
        }

        let mut remote_file = self
            .sftp
            .open(&norm)
            .await
            .map_err(|e| AppError::Protocol(format!("无法打开远程文件 '{}': {}", norm, e)))?;

        // 先写入同目录临时 `.part` 文件，完全落盘后再原子提交，避免断网/取消留下半截文件
        let part_path = staging_part_path(local_path);
        let mut local_file = tokio::fs::File::create(&part_path).await?;

        // 按 64 KB 分块流式下载并实时回调进度
        let mut buf = vec![0u8; 64 * 1024];
        let mut transferred: u64 = 0;

        let transfer_res: AppResult<()> = async {
            loop {
                if cancel.load(Ordering::Relaxed) {
                    return Err(AppError::Cancelled);
                }
                let n = remote_file
                    .read(&mut buf)
                    .await
                    .map_err(|e| AppError::Protocol(format!("SFTP 读取流错误: {}", e)))?;
                if n == 0 {
                    break;
                }
                local_file.write_all(&buf[..n]).await?;
                transferred += n as u64;
                progress(transferred, total.max(transferred));
            }
            local_file.flush().await?;
            Ok(())
        }
        .await;

        drop(local_file);
        if let Err(err) = transfer_res {
            let _ = tokio::fs::remove_file(&part_path).await;
            return Err(err);
        }

        commit_staging_file(&part_path, local_path)?;
        Ok(transferred)
    }

    async fn upload_file(
        &self,
        local_path: &Path,
        remote_path: &str,
        progress: ProgressCallback,
        cancel: Arc<AtomicBool>,
    ) -> AppResult<u64> {
        validate_remote_path_no_control(remote_path)?;
        let norm = normalize_remote_path(remote_path);
        let meta = tokio::fs::metadata(local_path).await?;
        let total = meta.len();

        let mut local_file = tokio::fs::File::open(local_path).await?;
        let mut remote_file = self
            .sftp
            .open_with_flags(
                &norm,
                OpenFlags::CREATE | OpenFlags::TRUNCATE | OpenFlags::WRITE,
            )
            .await
            .map_err(|e| AppError::Protocol(format!("无法创建远程文件 '{}': {}", norm, e)))?;

        // 按 64 KB 分块流式上传并实时回调进度
        let mut buf = vec![0u8; 64 * 1024];
        let mut transferred: u64 = 0;

        loop {
            if cancel.load(Ordering::Relaxed) {
                return Err(AppError::Cancelled);
            }
            let n = local_file.read(&mut buf).await?;
            if n == 0 {
                break;
            }
            remote_file
                .write_all(&buf[..n])
                .await
                .map_err(|e| AppError::Protocol(format!("SFTP 写入流错误: {}", e)))?;
            transferred += n as u64;
            progress(transferred, total.max(transferred));
        }
        remote_file
            .flush()
            .await
            .map_err(|e| AppError::Protocol(format!("SFTP 刷新失败: {}", e)))?;
        Ok(transferred)
    }

    async fn exec_shell(&self, command: &str, cwd: Option<&str>) -> AppResult<ShellOutput> {
        let ssh = self.ssh.lock().await;
        let mut channel = ssh
            .channel_open_session()
            .await
            .map_err(|e| AppError::Protocol(format!("打开 SSH 执行通道失败: {}", e)))?;

        // 自动切换到前端当前浏览的远程目录后再执行命令
        let full_cmd = if let Some(dir) = cwd.filter(|d| !d.trim().is_empty()) {
            let escaped = dir.replace('\'', "'\\''");
            format!("cd '{}' && {}", escaped, command)
        } else {
            command.to_string()
        };

        channel
            .exec(true, full_cmd.as_bytes())
            .await
            .map_err(|e| AppError::Protocol(format!("执行远程命令失败: {}", e)))?;

        let mut stdout = Vec::new();
        let mut stderr = Vec::new();
        let mut exit_code: i32 = 0;

        while let Some(msg) = channel.wait().await {
            match msg {
                ChannelMsg::Data { data } => stdout.extend_from_slice(&data),
                ChannelMsg::ExtendedData { data, ext: 1 } => {
                    stderr.extend_from_slice(&data);
                }
                ChannelMsg::ExitStatus { exit_status } => {
                    exit_code = exit_status as i32;
                }
                _ => {}
            }
        }

        Ok(ShellOutput {
            stdout: String::from_utf8_lossy(&stdout).to_string(),
            stderr: String::from_utf8_lossy(&stderr).to_string(),
            exit_code,
        })
    }

    async fn server_metrics(&self) -> AppResult<ServerMetrics> {
        // 1. 优先尝试标准 POSIX/Linux/macOS 轻量采样脚本（单次 exec 完成，耗时约 250ms）
        let posix_probe = r#"sh -c '
OS=$(grep -m1 "^PRETTY_NAME=" /etc/os-release 2>/dev/null | cut -d= -f2- | tr -d "\"")
[ -z "$OS" ] && OS=$(uname -s 2>/dev/null || echo "Unix")
CORES=$(nproc 2>/dev/null || grep -c "^processor" /proc/cpuinfo 2>/dev/null || sysctl -n hw.ncpu 2>/dev/null || echo 0)
LOAD=$(awk "{print \$1}" /proc/loadavg 2>/dev/null || true)
CPU=""
if [ -r /proc/stat ]; then
  set -- $(head -n1 /proc/stat)
  t1=$(($2+$3+$4+$5+$6+$7+$8)); i1=$(($5+$6))
  sleep 0.2
  set -- $(head -n1 /proc/stat)
  t2=$(($2+$3+$4+$5+$6+$7+$8)); i2=$(($5+$6))
  dt=$((t2-t1)); di=$((i2-i1))
  [ "$dt" -gt 0 ] && CPU=$(awk "BEGIN{printf \"%.1f\", (1 - $di/$dt) * 100}")
fi
MEM_T=0; MEM_U=0
if [ -r /proc/meminfo ]; then
  MEM_T=$(awk "/^MemTotal:/{print \$2*1024}" /proc/meminfo)
  MEM_A=$(awk "/^MemAvailable:/{print \$2*1024}" /proc/meminfo)
  [ -z "$MEM_A" ] && MEM_A=$(awk "/^MemFree:/{print \$2*1024}" /proc/meminfo)
  MEM_U=$((MEM_T - MEM_A))
fi
echo "OS=$OS"
echo "CORES=$CORES"
echo "CPU=$CPU"
echo "LOAD=$LOAD"
echo "MEM=$MEM_U:$MEM_T"
echo "DISKS_BEGIN"
(df -kP -T 2>/dev/null || df -kP 2>/dev/null) | tail -n +2
echo "DISKS_END"
'"#;

        if let Ok(out) = self.exec_shell(posix_probe, None).await {
            if let Some(metrics) = parse_server_metrics(&out.stdout) {
                return Ok(metrics);
            }
        }

        // 2. 若远端为 Windows OpenSSH 服务器，自动回退 PowerShell CIM 采集
        let win_probe = r#"powershell -NoProfile -Command "$os=Get-CimInstance Win32_OperatingSystem;$cs=Get-CimInstance Win32_ComputerSystem;$cpu=(Get-CimInstance Win32_Processor|Measure-Object -Property LoadPercentage -Average).Average;Write-Output ('OS='+$os.Caption);Write-Output ('CORES='+$cs.NumberOfLogicalProcessors);Write-Output ('CPU='+$cpu);Write-Output ('MEM='+(($os.TotalVisibleMemorySize-$os.FreePhysicalMemory)*1024)+':'+($os.TotalVisibleMemorySize*1024));Write-Output 'DISKS_BEGIN';Get-CimInstance Win32_LogicalDisk -Filter 'DriveType=3'|ForEach-Object{$u=$_.Size-$_.FreeSpace;Write-Output ('WIN|'+$_.DeviceID+'|'+$_.Size+'|'+$u+'|'+$_.FreeSpace)};Write-Output 'DISKS_END'""#;

        let out = self.exec_shell(win_probe, None).await?;
        parse_server_metrics(&out.stdout)
            .ok_or_else(|| AppError::Protocol("无法解析远端服务器硬件与磁盘信息".into()))
    }

    async fn open_pty(&self, cols: u32, rows: u32) -> AppResult<PtyHandle> {
        // 仅在申请新通道期间短暂持有 SSH 句柄锁，随后释放锁，不阻塞 SFTP 浏览与硬件监测
        let mut channel = {
            let ssh = self.ssh.lock().await;
            ssh.channel_open_session()
                .await
                .map_err(|e| AppError::Protocol(format!("打开 SSH PTY 通道失败: {}", e)))?
        };

        let safe_cols = cols.clamp(20, 500);
        let safe_rows = rows.clamp(5, 200);

        // 申请标准 xterm-256color 伪终端，尝试透传 COLORTERM=truecolor 并启动远端默认登录 Shell（zsh/bash/fish/PowerShell）
        channel
            .request_pty(false, "xterm-256color", safe_cols, safe_rows, 0, 0, &[])
            .await
            .map_err(|e| AppError::Protocol(format!("申请远程 PTY 伪终端失败: {}", e)))?;
        // 使用 want_reply=false 尽力透传真彩色标识，即使服务端 sshd 未开启 AcceptEnv COLORTERM 也不影响登录
        let _ = channel.set_env(false, "COLORTERM", "truecolor").await;
        channel
            .request_shell(true)
            .await
            .map_err(|e| AppError::Protocol(format!("启动远程默认 Shell 失败: {}", e)))?;

        let (cmd_tx, mut cmd_rx) = tokio::sync::mpsc::channel::<PtyCommand>(256);
        let (output_tx, output_rx) = tokio::sync::mpsc::channel::<Vec<u8>>(256);

        tokio::spawn(async move {
            loop {
                tokio::select! {
                    cmd = cmd_rx.recv() => {
                        match cmd {
                            Some(PtyCommand::Input(bytes)) => {
                                if channel.data(bytes.as_slice()).await.is_err() {
                                    break;
                                }
                            }
                            Some(PtyCommand::Resize { cols, rows }) => {
                                let _ = channel
                                    .window_change(cols.clamp(20, 500), rows.clamp(5, 200), 0, 0)
                                    .await;
                            }
                            Some(PtyCommand::Close) | None => {
                                let _ = channel.eof().await;
                                let _ = channel.close().await;
                                break;
                            }
                        }
                    }
                    msg = channel.wait() => {
                        match msg {
                            Some(ChannelMsg::Data { data })
                            | Some(ChannelMsg::ExtendedData { data, .. }) => {
                                if output_tx.send(data.to_vec()).await.is_err() {
                                    break;
                                }
                            }
                            Some(ChannelMsg::Eof | ChannelMsg::Close) | None => {
                                break;
                            }
                            _ => {}
                        }
                    }
                }
            }
        });

        Ok(PtyHandle { cmd_tx, output_rx })
    }

    async fn deploy_public_key(&self, public_key_line: &str) -> AppResult<bool> {
        // 1. 严格校验公钥为单行合法 OpenSSH 公钥（杜绝任意内容或换行注入 authorized_keys）
        let (canonical_pub, _, _) = crate::ssh_keys::validate_openssh_public_key(public_key_line)?;
        let mut new_parts = canonical_pub.split_whitespace();
        let new_type = new_parts.next().unwrap_or("");
        let new_blob = new_parts.next().unwrap_or("");

        // 2. 通过 SFTP canonicalize(".") 获取当前登录用户的远端主目录
        let home_dir = self
            .sftp
            .canonicalize(".")
            .await
            .map_err(|e| AppError::Protocol(format!("解析远端用户主目录失败: {}", e)))?;
        let ssh_dir = join_remote_path(&home_dir, ".ssh");
        let auth_keys_path = join_remote_path(&ssh_dir, "authorized_keys");

        // 3. 确保 ~/.ssh 目录存在并设置 0700 权限
        let _ = self.sftp.create_dir(&ssh_dir).await;
        let _ = self
            .sftp
            .set_metadata(
                &ssh_dir,
                FileAttributes {
                    permissions: Some(0o700),
                    ..Default::default()
                },
            )
            .await;

        // 4. 读取现有 authorized_keys 并按公钥主体幂等去重
        let existing = match self.read_bytes(&auth_keys_path, 512 * 1024).await {
            Ok(bytes) => String::from_utf8_lossy(&bytes).to_string(),
            Err(_) => String::new(),
        };
        for line in existing.lines() {
            let mut parts = line.split_whitespace();
            if let (Some(t), Some(b)) = (parts.next(), parts.next()) {
                if t == new_type && b == new_blob {
                    return Ok(false);
                }
            }
        }

        // 5. 追加规范化公钥并设置 authorized_keys 权限为 0600
        let mut updated = existing;
        if !updated.is_empty() && !updated.ends_with('\n') {
            updated.push('\n');
        }
        updated.push_str(&canonical_pub);
        updated.push('\n');

        self.write_bytes(&auth_keys_path, updated.as_bytes())
            .await?;
        let _ = self
            .sftp
            .set_metadata(
                &auth_keys_path,
                FileAttributes {
                    permissions: Some(0o600),
                    ..Default::default()
                },
            )
            .await;

        Ok(true)
    }
}

/// 过滤 Linux/macOS 伪文件系统、内核引导分区与容器/系统临时挂载点，仅保留真实业务磁盘
fn is_ignored_fs(fs_type: Option<&str>, dev: &str, mount: &str) -> bool {
    if let Some(t) = fs_type {
        let lt = t.to_ascii_lowercase();
        if matches!(
            lt.as_str(),
            "tmpfs"
                | "devtmpfs"
                | "squashfs"
                | "overlay"
                | "proc"
                | "sysfs"
                | "cgroup"
                | "cgroup2"
                | "efivarfs"
                | "devfs"
                | "autofs"
                | "tracefs"
                | "debugfs"
                | "fuse.lxcfs"
                | "nsfs"
                | "ramfs"
                | "iso9660"
        ) {
            return true;
        }
    }
    if dev.starts_with("tmpfs")
        || dev.starts_with("devtmpfs")
        || dev.starts_with("overlay")
        || dev.starts_with("/dev/loop")
    {
        return true;
    }
    mount.starts_with("/snap/")
        || mount.starts_with("/run")
        || mount.starts_with("/dev")
        || mount.starts_with("/sys")
        || mount.starts_with("/proc")
        || mount == "/boot"
        || mount.starts_with("/boot/")
        || mount.starts_with("/var/lib/docker/")
        || mount.starts_with("/var/lib/containerd/")
        || mount.starts_with("/var/lib/kubelet/")
        || mount.starts_with("/etc/")
}

/// 解析 POSIX 或 Windows 远端硬件与多磁盘采集输出
pub fn parse_server_metrics(raw: &str) -> Option<ServerMetrics> {
    if !raw.contains("DISKS_BEGIN") {
        return None;
    }

    let mut os_name = String::new();
    let mut cpu_cores: u32 = 0;
    let mut cpu_usage_pct: Option<f32> = None;
    let mut load_avg: Option<String> = None;
    let mut mem_used_bytes: u64 = 0;
    let mut mem_total_bytes: u64 = 0;
    let mut disks: Vec<RemoteDisk> = Vec::new();
    let mut seen_mounts = std::collections::HashSet::new();
    // 同一物理块设备（如 /dev/sda1）在多处 bind 挂载时仅保留主挂载点，防止 1 块盘被重复计为多盘
    let mut seen_devs: std::collections::HashMap<String, usize> = std::collections::HashMap::new();
    let mut in_disks = false;

    for raw_line in raw.lines() {
        let line = raw_line.trim();
        if line.is_empty() {
            continue;
        }
        if line == "DISKS_BEGIN" {
            in_disks = true;
            continue;
        }
        if line == "DISKS_END" {
            in_disks = false;
            continue;
        }

        if !in_disks {
            if let Some(v) = line.strip_prefix("OS=") {
                os_name = v.trim().to_string();
            } else if let Some(v) = line.strip_prefix("CORES=") {
                cpu_cores = v.trim().parse().unwrap_or(0);
            } else if let Some(v) = line.strip_prefix("CPU=") {
                if let Ok(pct) = v.trim().parse::<f32>() {
                    if pct.is_finite() {
                        cpu_usage_pct = Some(pct.clamp(0.0, 100.0));
                    }
                }
            } else if let Some(v) = line.strip_prefix("LOAD=") {
                let s = v.trim();
                if !s.is_empty() {
                    load_avg = Some(s.to_string());
                }
            } else if let Some(v) = line.strip_prefix("MEM=") {
                if let Some((u, t)) = v.split_once(':') {
                    mem_used_bytes = u.trim().parse().unwrap_or(0);
                    mem_total_bytes = t.trim().parse().unwrap_or(0);
                }
            }
            continue;
        }

        // Windows 盘符格式：WIN|C:|total|used|free
        if let Some(rest) = line.strip_prefix("WIN|") {
            let parts: Vec<&str> = rest.split('|').collect();
            if parts.len() >= 4 {
                let drive = parts[0].trim().trim_end_matches('\\');
                let total: u64 = parts[1].trim().parse().unwrap_or(0);
                let used: u64 = parts[2].trim().parse().unwrap_or(0);
                let avail: u64 = parts[3].trim().parse().unwrap_or(0);
                if total > 0 && !drive.is_empty() {
                    let mount_path = format!("/{}/", drive);
                    if seen_mounts.insert(mount_path.clone()) {
                        let pct = ((used as f64 / total as f64) * 100.0).round() as u8;
                        disks.push(RemoteDisk {
                            label: format!("{}\\", drive),
                            mount_path,
                            total_bytes: total,
                            used_bytes: used,
                            avail_bytes: avail,
                            use_percent: pct.min(100),
                        });
                    }
                }
            }
            continue;
        }

        // POSIX `df -kP -T` (7+ 列: fs type 1024-blocks used avail cap% mount...)
        // 或 `df -kP` (6+ 列: fs 1024-blocks used avail cap% mount...)
        let cols: Vec<&str> = line.split_whitespace().collect();
        let parsed_posix = if cols.len() >= 7
            && cols[2].parse::<u64>().is_ok()
            && cols[3].parse::<u64>().is_ok()
        {
            Some((
                cols[0],
                Some(cols[1]),
                cols[2].parse::<u64>().unwrap_or(0).saturating_mul(1024),
                cols[3].parse::<u64>().unwrap_or(0).saturating_mul(1024),
                cols[4].parse::<u64>().unwrap_or(0).saturating_mul(1024),
                cols[5]
                    .trim_end_matches('%')
                    .parse::<u8>()
                    .unwrap_or(0)
                    .min(100),
                cols[6..].join(" "),
            ))
        } else if cols.len() >= 6
            && cols[1].parse::<u64>().is_ok()
            && cols[2].parse::<u64>().is_ok()
        {
            Some((
                cols[0],
                None,
                cols[1].parse::<u64>().unwrap_or(0).saturating_mul(1024),
                cols[2].parse::<u64>().unwrap_or(0).saturating_mul(1024),
                cols[3].parse::<u64>().unwrap_or(0).saturating_mul(1024),
                cols[4]
                    .trim_end_matches('%')
                    .parse::<u8>()
                    .unwrap_or(0)
                    .min(100),
                cols[5..].join(" "),
            ))
        } else {
            None
        };

        if let Some((dev, fs_type, total, used, avail, cap, mount)) = parsed_posix {
            if total == 0 || is_ignored_fs(fs_type, dev, &mount) {
                continue;
            }
            if dev.starts_with("/dev/") {
                if let Some(&idx) = seen_devs.get(dev) {
                    // 若新挂载点是 `/` 或路径更短，则替换为主挂载点，避免同一块盘重复列出
                    if mount == "/" || mount.len() < disks[idx].mount_path.len() {
                        disks[idx].label = mount.clone();
                        disks[idx].mount_path = mount;
                    }
                    continue;
                }
            }
            if seen_mounts.insert(mount.clone()) {
                let idx = disks.len();
                if dev.starts_with("/dev/") {
                    seen_devs.insert(dev.to_string(), idx);
                }
                disks.push(RemoteDisk {
                    label: mount.clone(),
                    mount_path: mount,
                    total_bytes: total,
                    used_bytes: used,
                    avail_bytes: avail,
                    use_percent: cap,
                });
            }
        }
    }

    // 根挂载点 `/` 或 `C:\` 优先排在最前面，其余按路径字典序排列
    disks.sort_by(|a, b| {
        let a_root = a.mount_path == "/" || a.mount_path.eq_ignore_ascii_case("/C:/");
        let b_root = b.mount_path == "/" || b.mount_path.eq_ignore_ascii_case("/C:/");
        b_root
            .cmp(&a_root)
            .then_with(|| a.mount_path.cmp(&b.mount_path))
    });

    if os_name.is_empty() && cpu_cores == 0 && mem_total_bytes == 0 && disks.is_empty() {
        return None;
    }

    Some(ServerMetrics {
        os_name: if os_name.is_empty() {
            "Remote Server".to_string()
        } else {
            os_name
        },
        cpu_cores,
        cpu_usage_pct,
        load_avg,
        mem_used_bytes,
        mem_total_bytes,
        disks,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_check_ssh_agent_status_never_panics() {
        // 即使本机未启动 OpenSSH Agent 或 Pageant，检测也必须安全返回而非触发 panic/abort
        let status = check_ssh_agent_status().await;
        assert!(!status.message.is_empty());
    }

    #[test]
    fn test_parse_server_metrics_linux_multi_disk() {
        let sample = r#"
OS=Ubuntu 22.04.4 LTS
CORES=8
CPU=23.5
LOAD=0.64
MEM=4294967296:17179869184
DISKS_BEGIN
tmpfs tmpfs 1638400 1200 1637200 1% /run
/dev/sda1 ext4 52428800 20971520 31457280 40% /
/dev/sda1 ext4 52428800 20971520 31457280 40% /var/bind-root
/dev/sda2 ext4 1048576 204800 843776 20% /boot
/dev/sdb1 xfs 1048576000 629145600 419430400 60% /data
/dev/loop0 squashfs 102400 102400 0 100% /snap/core20/123
DISKS_END
"#;
        let m = parse_server_metrics(sample).expect("should parse linux metrics");
        assert_eq!(m.os_name, "Ubuntu 22.04.4 LTS");
        assert_eq!(m.cpu_cores, 8);
        assert_eq!(m.cpu_usage_pct, Some(23.5));
        assert_eq!(m.load_avg.as_deref(), Some("0.64"));
        assert_eq!(m.mem_total_bytes, 17_179_869_184);
        assert_eq!(m.disks.len(), 2);
        assert_eq!(m.disks[0].mount_path, "/");
        assert_eq!(m.disks[1].mount_path, "/data");
        assert_eq!(m.disks[1].use_percent, 60);
    }

    #[test]
    fn test_parse_server_metrics_windows_multi_disk() {
        let sample = r#"
OS=Microsoft Windows Server 2022 Standard
CORES=16
CPU=12
MEM=8589934592:34359738368
DISKS_BEGIN
WIN|C:|214748364800|107374182400|107374182400
WIN|D:|1099511627776|329853488332|769658139444
DISKS_END
"#;
        let m = parse_server_metrics(sample).expect("should parse windows metrics");
        assert_eq!(m.cpu_cores, 16);
        assert_eq!(m.disks.len(), 2);
        assert_eq!(m.disks[0].label, "C:\\");
        assert_eq!(m.disks[0].mount_path, "/C:/");
        assert_eq!(m.disks[1].label, "D:\\");
        assert_eq!(m.disks[1].mount_path, "/D:/");
    }
}
