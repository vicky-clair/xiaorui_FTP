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
use crate::known_hosts::KnownHostsStore;
use crate::models::{
    format_unix_mode, join_remote_path, normalize_remote_path, AgentStatus, AuthType, FileEntry,
    ShellOutput,
};
use crate::protocols::{ProgressCallback, RemoteFs};

/// SSH 客户端事件回调处理器（负责 TOFU 主机公钥指纹固定与变更拦截）
struct ClientHandler {
    host_port: String,
    known_hosts: Arc<KnownHostsStore>,
    mismatch_error: Arc<std::sync::Mutex<Option<String>>>,
}

impl ClientHandler {
    fn new(
        host_port: String,
        known_hosts: Arc<KnownHostsStore>,
        mismatch_error: Arc<std::sync::Mutex<Option<String>>>,
    ) -> Self {
        Self {
            host_port,
            known_hosts,
            mismatch_error,
        }
    }
}

#[async_trait]
impl client::Handler for ClientHandler {
    type Error = russh::Error;

    /// 校验远程服务器主机公钥（TOFU：首次信任并持久化，后续不一致时阻断连接）
    async fn check_server_key(
        &mut self,
        server_public_key: &PublicKey,
    ) -> Result<bool, Self::Error> {
        let fingerprint = server_public_key.fingerprint(HashAlg::Sha256).to_string();
        let algorithm = server_public_key.algorithm().to_string();
        match self
            .known_hosts
            .verify_or_trust(&self.host_port, &algorithm, &fingerprint)
        {
            Ok(_) => Ok(true),
            Err(mismatch) => {
                if let Ok(mut slot) = self.mismatch_error.lock() {
                    *slot = Some(mismatch.to_string());
                }
                Ok(false)
            }
        }
    }
}

/// SSH 跳板机（ProxyJump / Bastion）连接参数
#[derive(Debug, Clone)]
pub struct JumpHostConfig {
    pub host: String,
    pub port: u16,
    pub username: String,
    pub auth_type: AuthType,
    pub secret: Option<String>,
    pub private_key_path: Option<String>,
    pub passphrase: Option<String>,
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
        let target_mismatch = Arc::new(std::sync::Mutex::new(None));

        let (jump_handle, session) = if let Some(jh) = jump {
            let jump_host = jh.host.trim().to_string();
            let jump_host_port = format!("{}:{}", jump_host, jh.port);
            let jump_sock = resolve_socket_addr(jump_host.clone(), jh.port).await?;
            let jump_user = if jh.username.trim().is_empty() {
                target_user.clone()
            } else {
                jh.username.trim().to_string()
            };

            // 1. 连接并认证跳板机（带 20 秒超时保护，独立校验跳板机主机指纹）
            let jump_mismatch = Arc::new(std::sync::Mutex::new(None));
            let jump_handler =
                ClientHandler::new(jump_host_port, known_hosts.clone(), jump_mismatch.clone());
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
                if let Ok(Some(msg)) = jump_mismatch.lock().map(|g| g.clone()) {
                    return AppError::Auth(msg);
                }
                AppError::Connection(format!(
                    "无法连接 SSH 跳板机 {}:{}: {}",
                    jump_host, jh.port, e
                ))
            })?;

            let jump_sess = tokio::spawn(authenticate_handle(
                jump_sess,
                format!("跳板机 ({})", jump_host),
                jump_user,
                jh.auth_type,
                jh.secret,
                jh.private_key_path,
                jh.passphrase,
            ))
            .await
            .map_err(|e| AppError::Auth(format!("跳板机认证任务异常: {}", e)))??;

            // 2. 通过跳板机打开到目标主机的直连 TCP 隧道
            let tunnel = jump_sess
                .channel_open_direct_tcpip(
                    target_host.clone(),
                    port as u32,
                    String::from("127.0.0.1"),
                    0,
                )
                .await
                .map_err(|e| {
                    AppError::Connection(format!(
                        "跳板机 {} 无法建立到 {}:{} 的直连隧道: {}",
                        jump_host, target_host, port, e
                    ))
                })?;

            // 3. 在隧道流之上建立目标主机 SSH 加密连接并校验目标主机指纹
            let target_handler =
                ClientHandler::new(target_host_port, known_hosts, target_mismatch.clone());
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
                if let Ok(Some(msg)) = target_mismatch.lock().map(|g| g.clone()) {
                    return AppError::Auth(msg);
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
                ClientHandler::new(target_host_port, known_hosts, target_mismatch.clone());
            let direct_sess = tokio::time::timeout(
                Duration::from_secs(20),
                client::connect(config, target_sock, target_handler),
            )
            .await
            .map_err(|_| {
                AppError::Connection(format!("连接 SSH {}:{} 超时（20 秒）", target_host, port))
            })?
            .map_err(|e| {
                if let Ok(Some(msg)) = target_mismatch.lock().map(|g| g.clone()) {
                    return AppError::Auth(msg);
                }
                AppError::Connection(format!("无法连接 SSH {}:{}: {}", target_host, port, e))
            })?;
            (None, direct_sess)
        };

        // 认证目标主机
        let session = tokio::spawn(authenticate_handle(
            session,
            format!("目标主机 ({})", target_host),
            target_user,
            auth_type,
            password,
            private_key_path,
            passphrase,
        ))
        .await
        .map_err(|e| AppError::Auth(format!("SSH 认证任务异常: {}", e)))??;

        // 请求启动 SFTP 子系统
        let channel = session
            .channel_open_session()
            .await
            .map_err(|e| AppError::Protocol(format!("打开 SFTP 通道失败: {}", e)))?;
        channel
            .request_subsystem(true, String::from("sftp"))
            .await
            .map_err(|e| AppError::Protocol(format!("启动 SFTP 子系统失败: {}", e)))?;

        let sftp = SftpSession::new(channel.into_stream())
            .await
            .map_err(|e| AppError::Protocol(format!("初始化 SFTP 会话失败: {}", e)))?;

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
        let norm = normalize_remote_path(path);
        let dir = self
            .sftp
            .read_dir(&norm)
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
        let norm = normalize_remote_path(path);
        self.sftp
            .create_dir(&norm)
            .await
            .map_err(|e| AppError::Protocol(format!("创建目录 '{}' 失败: {}", norm, e)))?;
        Ok(())
    }

    async fn remove(&self, path: &str, is_dir: bool) -> AppResult<()> {
        let norm = normalize_remote_path(path);
        if is_dir {
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
        let src = normalize_remote_path(from);
        let dst = normalize_remote_path(to);
        self.sftp.rename(&src, &dst).await.map_err(|e| {
            AppError::Protocol(format!("重命名 '{}' -> '{}' 失败: {}", src, dst, e))
        })?;
        Ok(())
    }

    async fn chmod(&self, path: &str, mode: u32) -> AppResult<()> {
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
        let mut local_file = tokio::fs::File::create(local_path).await?;

        // 按 64 KB 分块流式下载并实时回调进度
        let mut buf = vec![0u8; 64 * 1024];
        let mut transferred: u64 = 0;

        loop {
            if cancel.load(Ordering::Relaxed) {
                drop(local_file);
                let _ = tokio::fs::remove_file(local_path).await;
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
        Ok(transferred)
    }

    async fn upload_file(
        &self,
        local_path: &Path,
        remote_path: &str,
        progress: ProgressCallback,
        cancel: Arc<AtomicBool>,
    ) -> AppResult<u64> {
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
}
