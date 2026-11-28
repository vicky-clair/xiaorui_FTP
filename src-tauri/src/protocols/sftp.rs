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
    NetInterface, RemoteDisk, ServerMetrics, ShellOutput, TempSensor,
};
use crate::protocols::{ProgressCallback, PtyCommand, PtyHandle, RemoteFs, TunnelRuntimeHandle};
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
        // 1. 优先尝试标准 POSIX/Linux/macOS 全维度轻量采样脚本（单次 exec 完成，耗时约 350ms）
        let posix_probe = r#"sh -c '
OS=$(grep -m1 "^PRETTY_NAME=" /etc/os-release 2>/dev/null | cut -d= -f2- | tr -d "\"")
[ -z "$OS" ] && OS=$(uname -s 2>/dev/null || echo "Unix")
UNAME=$(uname -s 2>/dev/null || echo "Linux")
OS_FAM="linux"
[ "$UNAME" = "Darwin" ] && OS_FAM="macos"
HOST=$(hostname 2>/dev/null || uname -n 2>/dev/null || true)
UPTIME=$(awk "{print int(\$1)}" /proc/uptime 2>/dev/null || true)
CORES=$(nproc 2>/dev/null || grep -c "^processor" /proc/cpuinfo 2>/dev/null || sysctl -n hw.ncpu 2>/dev/null || echo 0)
LOAD=$(awk "{print \$1, \$2, \$3}" /proc/loadavg 2>/dev/null || true)
NET1=$(awk "NR>2{gsub(/:/,\"\",\$1); print \$1,\$2,\$10}" /proc/net/dev 2>/dev/null)
CPU=""; CPU_U=""; CPU_S=""
if [ -r /proc/stat ]; then
  set -- $(head -n1 /proc/stat)
  u1=$(($2+$3)); s1=$(($4+$7+$8)); i1=$(($5+$6)); t1=$(($2+$3+$4+$5+$6+$7+$8))
  sleep 0.35
  set -- $(head -n1 /proc/stat)
  u2=$(($2+$3)); s2=$(($4+$7+$8)); i2=$(($5+$6)); t2=$(($2+$3+$4+$5+$6+$7+$8))
  dt=$((t2-t1)); di=$((i2-i1)); du=$((u2-u1)); ds=$((s2-s1))
  if [ "$dt" -gt 0 ]; then
    CPU=$(awk "BEGIN{printf \"%.1f\", (1 - $di/$dt) * 100}")
    CPU_U=$(awk "BEGIN{printf \"%.1f\", ($du/$dt) * 100}")
    CPU_S=$(awk "BEGIN{printf \"%.1f\", ($ds/$dt) * 100}")
  fi
fi
NET2=$(awk "NR>2{gsub(/:/,\"\",\$1); print \$1,\$2,\$10}" /proc/net/dev 2>/dev/null)
MEM_T=0; MEM_U=0; SWAP_T=0; SWAP_U=0
if [ -r /proc/meminfo ]; then
  MEM_T=$(awk "/^MemTotal:/{print \$2*1024}" /proc/meminfo)
  MEM_A=$(awk "/^MemAvailable:/{print \$2*1024}" /proc/meminfo)
  [ -z "$MEM_A" ] && MEM_A=$(awk "/^MemFree:/{print \$2*1024}" /proc/meminfo)
  MEM_U=$((MEM_T - MEM_A))
  SWAP_T=$(awk "/^SwapTotal:/{print \$2*1024}" /proc/meminfo)
  SWAP_F=$(awk "/^SwapFree:/{print \$2*1024}" /proc/meminfo)
  [ -n "$SWAP_T" ] && [ -n "$SWAP_F" ] && SWAP_U=$((SWAP_T - SWAP_F))
fi
GW=$(ip route show default 2>/dev/null | awk "/default/{print \$3; exit}")
KERNEL="$(uname -r 2>/dev/null | cut -d- -f1) · $(uname -m 2>/dev/null)"
PROCS=$(ls -d /proc/[0-9]* 2>/dev/null | wc -l | tr -d " ")
TCP_EST=$(awk "\$4==\"01\"{c++} END{print c+0}" /proc/net/tcp /proc/net/tcp6 2>/dev/null || echo 0)
GPU=""
if command -v nvidia-smi >/dev/null 2>&1; then
  GPU=$(nvidia-smi --query-gpu=name,utilization.gpu,memory.used,memory.total,temperature.gpu --format=csv,noheader,nounits 2>/dev/null | head -n1)
fi
echo "OS_FAMILY=$OS_FAM"
echo "OS=$OS"
echo "KERNEL=$KERNEL"
echo "HOST=$HOST"
echo "UPTIME=$UPTIME"
echo "CORES=$CORES"
echo "CPU=$CPU"
echo "CPU_USER=$CPU_U"
echo "CPU_SYS=$CPU_S"
echo "LOAD=$LOAD"
echo "MEM=$MEM_U:$MEM_T"
echo "SWAP=$SWAP_U:$SWAP_T"
echo "GW=$GW"
echo "PROCS=$PROCS"
echo "TCP_EST=$TCP_EST"
[ -n "$GPU" ] && echo "GPU=$GPU"
echo "TEMPS_BEGIN"
FOUND_HWMON=0
for hw in /sys/class/hwmon/hwmon*; do
  [ -d "$hw" ] || continue
  hname=$(cat "$hw/name" 2>/dev/null || true)
  for tin in "$hw"/temp*_input; do
    [ -r "$tin" ] || continue
    raw=$(cat "$tin" 2>/dev/null || true)
    [ -n "$raw" ] && [ "$raw" -gt 0 ] 2>/dev/null || continue
    lbl_f="${tin%_input}_label"
    lbl=$(cat "$lbl_f" 2>/dev/null || true)
    [ -z "$lbl" ] && lbl="$hname"
    [ -z "$lbl" ] && lbl="sensor"
    deg=$(awk "BEGIN{printf \"%.1f\", $raw/1000}")
    echo "${hname}:${lbl}|${deg}"
    FOUND_HWMON=1
  done
done
if [ "$FOUND_HWMON" -eq 0 ]; then
  for tz in /sys/class/thermal/thermal_zone*; do
    [ -r "$tz/temp" ] || continue
    raw=$(cat "$tz/temp" 2>/dev/null || true)
    [ -n "$raw" ] && [ "$raw" -gt 0 ] 2>/dev/null || continue
    tname=$(cat "$tz/type" 2>/dev/null || basename "$tz")
    deg=$(awk "BEGIN{printf \"%.1f\", $raw/1000}")
    echo "${tname}|${deg}"
  done
fi
echo "TEMPS_END"
echo "NET_BEGIN"
if [ -n "$NET2" ]; then
  IPS=$(ip -4 -o addr show 2>/dev/null | awk "{split(\$4,a,\"/\"); print \$2, a[1]}")
  printf "%s\n" "$NET2" | while read -r iface r2 t2; do
    [ -z "$iface" ] && continue
    [ "$iface" = "lo" ] && continue
    r1=$(printf "%s\n" "$NET1" | awk -v k="$iface" "\$1==k{print \$2; exit}")
    t1=$(printf "%s\n" "$NET1" | awk -v k="$iface" "\$1==k{print \$3; exit}")
    ip4=$(printf "%s\n" "$IPS" | awk -v k="$iface" "\$1==k{print \$2; exit}")
    [ -z "$r1" ] && r1=$r2
    [ -z "$t1" ] && t1=$t2
    rx=$(awk "BEGIN{d=$r2-$r1; if(d<0)d=0; printf \"%.0f\", d/0.35}")
    tx=$(awk "BEGIN{d=$t2-$t1; if(d<0)d=0; printf \"%.0f\", d/0.35}")
    echo "$iface|$ip4|$rx|$tx"
  done
fi
echo "NET_END"
echo "DISKS_BEGIN"
(df -kP -T 2>/dev/null || df -kP 2>/dev/null) | tail -n +2
echo "DISKS_END"
'"#;

        if let Ok(out) = self.exec_shell(posix_probe, None).await {
            if let Some(metrics) = parse_server_metrics(&out.stdout) {
                return Ok(metrics);
            }
        }

        // 2. 若远端为 Windows OpenSSH 服务器，自动回退 PowerShell CIM + 网卡速率采集
        let win_probe = build_windows_metrics_command();
        let out = self.exec_shell(&win_probe, None).await?;
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

    async fn start_local_forward(
        &self,
        local_port: u16,
        remote_host: String,
        remote_port: u16,
    ) -> AppResult<TunnelRuntimeHandle> {
        let target_host = remote_host.trim().to_string();
        if target_host.is_empty()
            || target_host.len() > 253
            || target_host
                .chars()
                .any(|c| c.is_whitespace() || c.is_control())
        {
            return Err(AppError::InvalidInput(
                "远端转发目标主机地址非法（不能为空或包含空白/控制字符）".into(),
            ));
        }
        if remote_port == 0 {
            return Err(AppError::InvalidInput("远端转发端口必须为 1–65535".into()));
        }

        // 严格限定仅绑定本地回环地址 127.0.0.1，防范将内网服务意外暴露到局域网
        let listener = tokio::net::TcpListener::bind(("127.0.0.1", local_port))
            .await
            .map_err(|e| {
                AppError::Connection(format!(
                    "绑定本地回环端口 127.0.0.1:{} 失败: {}",
                    local_port, e
                ))
            })?;
        let bound_port = listener
            .local_addr()
            .map_err(|e| AppError::Connection(format!("获取本地监听地址失败: {}", e)))?
            .port();

        let ssh = self.ssh.clone();
        let active_connections = Arc::new(std::sync::atomic::AtomicU32::new(0));
        let active_ref = active_connections.clone();
        let (shutdown_tx, mut shutdown_rx) = tokio::sync::oneshot::channel::<()>();

        tokio::spawn(async move {
            loop {
                tokio::select! {
                    _ = &mut shutdown_rx => {
                        break;
                    }
                    accepted = listener.accept() => {
                        let Ok((mut tcp_stream, peer_addr)) = accepted else {
                            break;
                        };
                        let ssh_conn = ssh.clone();
                        let rhost = target_host.clone();
                        let counter = active_ref.clone();
                        tokio::spawn(async move {
                            let channel = {
                                let guard = ssh_conn.lock().await;
                                guard
                                    .channel_open_direct_tcpip(
                                        rhost,
                                        remote_port as u32,
                                        peer_addr.ip().to_string(),
                                        peer_addr.port() as u32,
                                    )
                                    .await
                            };
                            let Ok(ch) = channel else {
                                return;
                            };
                            counter.fetch_add(1, Ordering::Relaxed);
                            let mut ssh_stream = ch.into_stream();
                            let _ = tokio::io::copy_bidirectional(&mut tcp_stream, &mut ssh_stream).await;
                            counter.fetch_sub(1, Ordering::Relaxed);
                        });
                    }
                }
            }
        });

        Ok(TunnelRuntimeHandle {
            local_port: bound_port,
            active_connections,
            shutdown_tx,
        })
    }
}

/// 构造 Windows 平台硬件/温度/进程/TCP/多盘/网卡实时采集 PowerShell 命令（同时服务远端 Windows SSH 与本机 Windows 监控）
pub fn build_windows_metrics_script() -> &'static str {
    r#"$os=Get-CimInstance Win32_OperatingSystem;$cs=Get-CimInstance Win32_ComputerSystem;$cpu=(Get-CimInstance Win32_Processor|Measure-Object -Property LoadPercentage -Average).Average;$up=[int64]([DateTime]::Now-$os.LastBootUpTime).TotalSeconds;$gw=(Get-NetRoute -DestinationPrefix '0.0.0.0/0' -ErrorAction SilentlyContinue|Sort-Object RouteMetric|Select-Object -First 1 -ExpandProperty NextHop);$gpu=Get-CimInstance Win32_VideoController -ErrorAction SilentlyContinue|Select-Object -First 1;$procs=(Get-Process -ErrorAction SilentlyContinue).Count;$tcpEst=(Get-NetTCPConnection -State Established -ErrorAction SilentlyContinue).Count;$s1=@{};Get-NetAdapterStatistics -ErrorAction SilentlyContinue|ForEach-Object{$s1[$_.Name]=@($_.ReceivedBytes,$_.SentBytes)};Start-Sleep -Milliseconds 350;$s2=Get-NetAdapterStatistics -ErrorAction SilentlyContinue;$ips=@{};Get-NetIPAddress -AddressFamily IPv4 -ErrorAction SilentlyContinue|Where-Object{$_.IPAddress -notlike '127.*' -and $_.IPAddress -notlike '169.254.*'}|ForEach-Object{if(-not $ips.ContainsKey($_.InterfaceAlias)){$ips[$_.InterfaceAlias]=$_.IPAddress}};Write-Output 'OS_FAMILY=windows';Write-Output ('OS='+$os.Caption);Write-Output ('KERNEL='+$os.Version+' · '+$os.OSArchitecture);Write-Output ('HOST='+$cs.Name);Write-Output ('UPTIME='+$up);Write-Output ('CORES='+$cs.NumberOfLogicalProcessors);Write-Output ('CPU='+$cpu);Write-Output ('PROCS='+$procs);Write-Output ('TCP_EST='+$tcpEst);Write-Output ('MEM='+(($os.TotalVisibleMemorySize-$os.FreePhysicalMemory)*1024)+':'+($os.TotalVisibleMemorySize*1024));$swT=($os.TotalVirtualMemorySize-$os.TotalVisibleMemorySize)*1024;if($swT -lt 0){$swT=0};$swF=($os.FreeVirtualMemory-$os.FreePhysicalMemory)*1024;if($swF -lt 0){$swF=0};$swU=$swT-$swF;if($swU -lt 0){$swU=0};Write-Output ('SWAP='+$swU+':'+$swT);if($gw){Write-Output ('GW='+$gw)};$nv=Get-Command nvidia-smi -ErrorAction SilentlyContinue;if($nv){$nvOut=& nvidia-smi --query-gpu=name,utilization.gpu,memory.used,memory.total,temperature.gpu --format=csv,noheader,nounits 2>$null|Select-Object -First 1;if($nvOut){Write-Output ('GPU='+$nvOut)}}elseif($gpu){$vram=[int64]([math]::Round(($gpu.AdapterRAM)/1MB));Write-Output ('GPU='+$gpu.Name+',,0,'+$vram)};Write-Output 'TEMPS_BEGIN';Get-CimInstance -Namespace root/wmi -ClassName MSAcpi_ThermalZoneTemperature -ErrorAction SilentlyContinue|ForEach-Object{$c=[math]::Round(($_.CurrentTemperature-2732)/10.0,1);if($c -gt 0 -and $c -lt 130){Write-Output ('ACPI|'+$c)}};Write-Output 'TEMPS_END';Write-Output 'NET_BEGIN';Get-NetAdapter -ErrorAction SilentlyContinue|Where-Object{$_.Status -eq 'Up' -or $ips.ContainsKey($_.Name)}|ForEach-Object{$n=$_.Name;$st=$s2|Where-Object{$_.Name -eq $n}|Select-Object -First 1;$rx=0;$tx=0;if($st -and $s1.ContainsKey($n)){$rx=[int64]([math]::Max(0,($st.ReceivedBytes-$s1[$n][0])/0.35));$tx=[int64]([math]::Max(0,($st.SentBytes-$s1[$n][1])/0.35))};$ip=$ips[$n];Write-Output ($n+'|'+$ip+'|'+$rx+'|'+$tx)};Write-Output 'NET_END';Write-Output 'DISKS_BEGIN';Get-CimInstance Win32_LogicalDisk -Filter 'DriveType=3'|ForEach-Object{$u=$_.Size-$_.FreeSpace;Write-Output ('WIN|'+$_.DeviceID+'|'+$_.Size+'|'+$u+'|'+$_.FreeSpace)};Write-Output 'DISKS_END'"#
}

fn build_windows_metrics_command() -> String {
    format!(
        "powershell -NoProfile -Command \"{}\"",
        build_windows_metrics_script().replace('"', "\\\"")
    )
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

/// 将 Linux `hwmon` / `thermal_zone` 原始传感器名称归一化为易读短标签并赋予展示优先级
fn normalize_temp_sensor_label(raw: &str) -> (u8, String) {
    let lower = raw.to_ascii_lowercase();
    if lower.contains("package id")
        || lower.contains("tctl")
        || lower.contains("tdie")
        || lower.contains("x86_pkg_temp")
        || lower.contains("cpu-thermal")
        || lower.contains("coretemp")
        || lower.contains("k10temp")
        || lower.contains("zenpower")
    {
        return (0, "CPU".to_string());
    }
    if lower.contains("gpu") || lower.contains("amdgpu") || lower.contains("nouveau") {
        return (1, "GPU".to_string());
    }
    if lower.contains("nvme") || lower.contains("drivetemp") || lower.contains("sata") {
        return (2, "NVMe".to_string());
    }
    if lower.contains("iwlwifi") || lower.contains("wifi") || lower.contains("phy") {
        return (4, "WiFi".to_string());
    }
    if lower.contains("acpitz") || lower.contains("acpi") || lower.contains("pch") {
        return (3, "主板".to_string());
    }
    let clean = raw
        .split(':')
        .next_back()
        .unwrap_or(raw)
        .trim()
        .chars()
        .take(16)
        .collect::<String>();
    (
        5,
        if clean.is_empty() {
            "传感器".into()
        } else {
            clean
        },
    )
}

/// 解析 POSIX（Linux/macOS）或 Windows 远端/本机硬件、温度传感器、网卡流量与多磁盘采集输出
pub fn parse_server_metrics(raw: &str) -> Option<ServerMetrics> {
    if !raw.contains("DISKS_BEGIN") {
        return None;
    }

    let mut os_family = String::new();
    let mut os_name = String::new();
    let mut kernel_version: Option<String> = None;
    let mut hostname: Option<String> = None;
    let mut uptime_secs: Option<u64> = None;
    let mut cpu_cores: u32 = 0;
    let mut cpu_usage_pct: Option<f32> = None;
    let mut cpu_user_pct: Option<f32> = None;
    let mut cpu_sys_pct: Option<f32> = None;
    let mut load_avg: Option<String> = None;
    let mut mem_used_bytes: u64 = 0;
    let mut mem_total_bytes: u64 = 0;
    let mut swap_used_bytes: u64 = 0;
    let mut swap_total_bytes: u64 = 0;
    let mut gpu_name: Option<String> = None;
    let mut gpu_usage_pct: Option<f32> = None;
    let mut gpu_mem_used_mb: Option<u64> = None;
    let mut gpu_mem_total_mb: Option<u64> = None;
    let mut gpu_temp_c: Option<f32> = None;
    let mut raw_temps: Vec<(u8, String, f32)> = Vec::new();
    let mut process_count: Option<u32> = None;
    let mut tcp_established: Option<u32> = None;
    let mut default_gateway: Option<String> = None;
    let mut net_interfaces: Vec<NetInterface> = Vec::new();
    let mut disks: Vec<RemoteDisk> = Vec::new();
    let mut seen_mounts = std::collections::HashSet::new();
    // 同一物理块设备（如 /dev/sda1）在多处 bind 挂载时仅保留主挂载点，防止 1 块盘被重复计为多盘
    let mut seen_devs: std::collections::HashMap<String, usize> = std::collections::HashMap::new();
    let mut section = 0u8; // 0 = header, 1 = disks, 2 = net, 3 = temps

    for raw_line in raw.lines() {
        let line = raw_line.trim();
        if line.is_empty() {
            continue;
        }
        if line == "DISKS_BEGIN" {
            section = 1;
            continue;
        }
        if line == "DISKS_END" || line == "NET_END" || line == "TEMPS_END" {
            section = 0;
            continue;
        }
        if line == "NET_BEGIN" {
            section = 2;
            continue;
        }
        if line == "TEMPS_BEGIN" {
            section = 3;
            continue;
        }

        if section == 0 {
            if let Some(v) = line.strip_prefix("OS_FAMILY=") {
                os_family = v.trim().to_ascii_lowercase();
            } else if let Some(v) = line.strip_prefix("OS=") {
                os_name = v.trim().to_string();
            } else if let Some(v) = line.strip_prefix("KERNEL=") {
                let s = v.trim();
                if !s.is_empty() && s != "·" {
                    kernel_version = Some(s.to_string());
                }
            } else if let Some(v) = line.strip_prefix("HOST=") {
                let s = v.trim();
                if !s.is_empty() {
                    hostname = Some(s.to_string());
                }
            } else if let Some(v) = line.strip_prefix("UPTIME=") {
                if let Ok(sec) = v.trim().parse::<u64>() {
                    uptime_secs = Some(sec);
                }
            } else if let Some(v) = line.strip_prefix("CORES=") {
                cpu_cores = v.trim().parse().unwrap_or(0);
            } else if let Some(v) = line.strip_prefix("PROCS=") {
                if let Ok(n) = v.trim().parse::<u32>() {
                    process_count = Some(n);
                }
            } else if let Some(v) = line.strip_prefix("TCP_EST=") {
                if let Ok(n) = v.trim().parse::<u32>() {
                    tcp_established = Some(n);
                }
            } else if let Some(v) = line.strip_prefix("CPU=") {
                if let Ok(pct) = v.trim().parse::<f32>() {
                    if pct.is_finite() {
                        cpu_usage_pct = Some(pct.clamp(0.0, 100.0));
                    }
                }
            } else if let Some(v) = line.strip_prefix("CPU_USER=") {
                if let Ok(pct) = v.trim().parse::<f32>() {
                    if pct.is_finite() {
                        cpu_user_pct = Some(pct.clamp(0.0, 100.0));
                    }
                }
            } else if let Some(v) = line.strip_prefix("CPU_SYS=") {
                if let Ok(pct) = v.trim().parse::<f32>() {
                    if pct.is_finite() {
                        cpu_sys_pct = Some(pct.clamp(0.0, 100.0));
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
            } else if let Some(v) = line.strip_prefix("SWAP=") {
                if let Some((u, t)) = v.split_once(':') {
                    swap_used_bytes = u.trim().parse().unwrap_or(0);
                    swap_total_bytes = t.trim().parse().unwrap_or(0);
                }
            } else if let Some(v) = line.strip_prefix("GW=") {
                let s = v.trim();
                if !s.is_empty() {
                    default_gateway = Some(s.to_string());
                }
            } else if let Some(v) = line.strip_prefix("GPU=") {
                let parts: Vec<&str> = v.split(',').map(|s| s.trim()).collect();
                if let Some(name) = parts.first().filter(|s| !s.is_empty()) {
                    gpu_name = Some((*name).to_string());
                }
                if let Some(u) = parts.get(1).and_then(|s| s.parse::<f32>().ok()) {
                    if u.is_finite() {
                        gpu_usage_pct = Some(u.clamp(0.0, 100.0));
                    }
                }
                if let Some(used) = parts.get(2).and_then(|s| s.parse::<u64>().ok()) {
                    gpu_mem_used_mb = Some(used);
                }
                if let Some(total) = parts.get(3).and_then(|s| s.parse::<u64>().ok()) {
                    if total > 0 {
                        gpu_mem_total_mb = Some(total);
                    }
                }
                if let Some(t) = parts.get(4).and_then(|s| s.parse::<f32>().ok()) {
                    if t.is_finite() && t > 0.0 && t < 130.0 {
                        gpu_temp_c = Some(t);
                    }
                }
            }
            continue;
        }

        if section == 3 {
            // 温度行格式：sensor_name|48.5
            if let Some((name, val)) = line.split_once('|') {
                if let Ok(deg) = val.trim().parse::<f32>() {
                    if deg.is_finite() && deg > 0.0 && deg < 130.0 {
                        let (prio, label) = normalize_temp_sensor_label(name.trim());
                        raw_temps.push((prio, label, deg));
                    }
                }
            }
            continue;
        }

        if section == 2 {
            // 网卡行格式：name|ip|rx_bps|tx_bps
            let parts: Vec<&str> = line.split('|').collect();
            if parts.len() >= 4 {
                let name = parts[0].trim();
                if !name.is_empty() {
                    let ip = Some(parts[1].trim())
                        .filter(|s| !s.is_empty())
                        .map(|s| s.to_string());
                    let rx_bytes_per_sec = parts[2].trim().parse::<u64>().unwrap_or(0);
                    let tx_bytes_per_sec = parts[3].trim().parse::<u64>().unwrap_or(0);
                    net_interfaces.push(NetInterface {
                        name: name.to_string(),
                        ip,
                        rx_bytes_per_sec,
                        tx_bytes_per_sec,
                    });
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

        // POSIX `df -kP -T` (7+ 列) 或 `df -kP` (6+ 列)
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

    // 自动补全 os_family（兼容旧版采样输出与 Windows/macOS 识别）
    if os_family.is_empty() {
        let lower = os_name.to_ascii_lowercase();
        if lower.contains("windows") || disks.iter().any(|d| d.label.ends_with(":\\")) {
            os_family = "windows".to_string();
        } else if lower.contains("darwin") || lower.contains("macos") {
            os_family = "macos".to_string();
        } else {
            os_family = "linux".to_string();
        }
    }

    let net_rx_bps: u64 = net_interfaces
        .iter()
        .map(|n| n.rx_bytes_per_sec)
        .fold(0u64, u64::saturating_add);
    let net_tx_bps: u64 = net_interfaces
        .iter()
        .map(|n| n.tx_bytes_per_sec)
        .fold(0u64, u64::saturating_add);

    if let Some(gt) = gpu_temp_c {
        raw_temps.push((1, "GPU".to_string(), gt));
    }
    // 同类传感器按最高温度去重聚合（例如多核 CPU 保留最高核心温度），再按 CPU -> GPU -> NVMe -> 主板 排序
    raw_temps.sort_by(|a, b| a.0.cmp(&b.0).then_with(|| a.1.cmp(&b.1)));
    let mut temperatures: Vec<TempSensor> = Vec::new();
    for (_, label, deg) in raw_temps {
        if let Some(existing) = temperatures.iter_mut().find(|t| t.label == label) {
            if deg > existing.temp_c {
                existing.temp_c = deg;
            }
        } else {
            temperatures.push(TempSensor { label, temp_c: deg });
        }
    }

    Some(ServerMetrics {
        os_family,
        os_name: if os_name.is_empty() {
            "Remote Server".to_string()
        } else {
            os_name
        },
        kernel_version,
        hostname,
        uptime_secs,
        cpu_cores,
        cpu_usage_pct,
        cpu_user_pct,
        cpu_sys_pct,
        load_avg,
        mem_used_bytes,
        mem_total_bytes,
        swap_used_bytes,
        swap_total_bytes,
        gpu_name,
        gpu_usage_pct,
        gpu_mem_used_mb,
        gpu_mem_total_mb,
        gpu_temp_c,
        temperatures,
        process_count,
        tcp_established,
        default_gateway,
        net_rx_bps,
        net_tx_bps,
        net_interfaces,
        disks,
        timestamp: chrono::Utc::now().timestamp(),
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
OS_FAMILY=linux
OS=Ubuntu 22.04.4 LTS
KERNEL=6.12.22 · x86_64
HOST=debian-srv
UPTIME=1425600
CORES=8
CPU=23.5
CPU_USER=10.2
CPU_SYS=11.9
LOAD=0.64 0.51 0.42
MEM=4294967296:17179869184
SWAP=536870912:2147483648
GW=192.168.9.1
PROCS=184
TCP_EST=26
GPU=NVIDIA RTX 4060, 18, 1024, 8192, 52
TEMPS_BEGIN
coretemp:Package id 0|48.5
coretemp:Core 0|45.0
nvme:Composite|41.0
acpitz:acpitz|44.0
TEMPS_END
NET_BEGIN
enp2s0|192.168.9.165|18432|7168
wlp1s0||0|0
NET_END
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
        assert_eq!(m.os_family, "linux");
        assert_eq!(m.os_name, "Ubuntu 22.04.4 LTS");
        assert_eq!(m.hostname.as_deref(), Some("debian-srv"));
        assert_eq!(m.uptime_secs, Some(1_425_600));
        assert_eq!(m.cpu_cores, 8);
        assert_eq!(m.cpu_usage_pct, Some(23.5));
        assert_eq!(m.cpu_user_pct, Some(10.2));
        assert_eq!(m.cpu_sys_pct, Some(11.9));
        assert_eq!(m.load_avg.as_deref(), Some("0.64 0.51 0.42"));
        assert_eq!(m.mem_total_bytes, 17_179_869_184);
        assert_eq!(m.swap_total_bytes, 2_147_483_648);
        assert_eq!(m.default_gateway.as_deref(), Some("192.168.9.1"));
        assert_eq!(m.gpu_name.as_deref(), Some("NVIDIA RTX 4060"));
        assert_eq!(m.gpu_mem_total_mb, Some(8192));
        assert_eq!(m.gpu_temp_c, Some(52.0));
        assert_eq!(m.kernel_version.as_deref(), Some("6.12.22 · x86_64"));
        assert_eq!(m.process_count, Some(184));
        assert_eq!(m.tcp_established, Some(26));
        assert_eq!(m.temperatures.len(), 4);
        assert_eq!(m.temperatures[0].label, "CPU");
        assert_eq!(m.temperatures[0].temp_c, 48.5);
        assert_eq!(m.temperatures[1].label, "GPU");
        assert_eq!(m.temperatures[2].label, "NVMe");
        assert_eq!(m.net_interfaces.len(), 2);
        assert_eq!(m.net_rx_bps, 18432);
        assert_eq!(m.net_tx_bps, 7168);
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
