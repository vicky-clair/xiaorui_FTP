//! Tauri IPC 命令层与全局应用状态管理
//!
//! 将后端站点管理、会话连接、远程/本地文件操作、并发传输队列以及 SSH 终端命令
//! 统一暴露给前端界面调用。

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use base64::Engine;
use chrono::Utc;
use percent_encoding::{utf8_percent_encode, NON_ALPHANUMERIC};
use serde::Deserialize;
use tauri::{AppHandle, Emitter, Manager, State, WebviewUrl, WebviewWindow, WebviewWindowBuilder};
use tokio::sync::RwLock;
use uuid::Uuid;

use crate::audit::{AuditCategory, AuditEntry, AuditLogger, AuditSeverity};
use crate::error::{AppError, AppResult};
use crate::known_hosts::{KnownHostEntry, KnownHostsStore};
use crate::local_fs;
use crate::local_pty::{self, LocalShellOption, LocalTerminalDefaults};
use crate::models::parse_quick_connect_uri;
use crate::models::{
    join_remote_path, normalize_remote_path, AgentStatus, AppSettings, AuthType, FileEntry,
    LocalRoot, Protocol, PtyDataEvent, PtyExitEvent, ServerMetrics, SessionInfo, SiteConfig,
    SiteInput, SiteSecrets, SshTunnelInfo, TransferDirection, TransferTask,
};
use crate::protocols::{connect_protocol, sftp, PtyCommand, RemoteFs};
use crate::security::{
    ensure_not_symlink_target, resolve_safe_download_path, validate_scoped_file_path,
    write_private_file,
};
use crate::ssh_config::{self, SshConfigHost};
use crate::ssh_keys::{self, SshKeyEntry};
use crate::storage::{load_secrets_strict, SiteStore};
use crate::transfer::{TransferManager, TRANSFER_EVENT};

/// 向前端推送交互式 SSH PTY 原始输出与退出状态的事件名
pub const PTY_DATA_EVENT: &str = "pty://data";
pub const PTY_EXIT_EVENT: &str = "pty://exit";

/// 单个 PTY 单次推送事件最大字节片（16 KB）与每秒输出限流阈值（1 MB/s），防范恶意服务端刷屏耗尽内存
const PTY_MAX_CHUNK_BYTES: usize = 16 * 1024;
const PTY_MAX_BYTES_PER_SEC: usize = 1024 * 1024;
/// 本地日志导出文件最大体积（10 MB）
const MAX_TEXT_EXPORT_BYTES: usize = 10 * 1024 * 1024;

/// 活跃交互式 SSH PTY 实例（绑定所属会话与创建窗口，防范跨窗口越权操控）
pub struct ActivePty {
    pub session_id: String,
    pub owner_window: String,
    pub cmd_tx: tokio::sync::mpsc::Sender<PtyCommand>,
}

/// 活跃 SSH 本地端口转发隧道运行时状态
pub struct ActiveTunnel {
    pub info: SshTunnelInfo,
    pub active_connections: Arc<std::sync::atomic::AtomicU32>,
    pub shutdown_tx: Option<tokio::sync::oneshot::Sender<()>>,
}

/// 活跃远程会话实例（包含元数据与协议适配器句柄）
pub struct ActiveSession {
    pub info: SessionInfo,
    pub fs: Arc<dyn RemoteFs>,
}

/// 全局 Tauri 托管应用状态
pub struct AppState {
    /// 站点持久化仓库（`sites.json`）
    pub store: SiteStore,
    /// SSH 已知主机公钥指纹仓库（`known_hosts.json`，独立于站点配置）
    pub known_hosts: Arc<KnownHostsStore>,
    /// 本地脱敏安全审计日志管理器（`audit.log.jsonl`）
    pub audit: AuditLogger,
    /// 轻量偏好配置文件路径（`settings.json`）
    settings_file: PathBuf,
    /// 关闭主窗口时是否最小化到系统托盘（原子无锁读取，供窗口事件钩子安全使用）
    minimize_to_tray: AtomicBool,
    /// 当前所有活跃的远程会话映射表（session_id -> ActiveSession）
    pub sessions: RwLock<HashMap<String, ActiveSession>>,
    /// 当前所有活跃的交互式 SSH PTY 伪终端映射表（terminal_id -> ActivePty）
    pub ptys: Arc<RwLock<HashMap<String, ActivePty>>>,
    /// 独立终端窗口与远程会话的绑定表（window_label -> session_id）
    pub terminal_windows: RwLock<HashMap<String, String>>,
    /// 当前活跃的 SSH 本地端口转发隧道映射表（tunnel_id -> ActiveTunnel）
    pub tunnels: RwLock<HashMap<String, ActiveTunnel>>,
    /// 后台并发传输队列管理器
    pub transfers: TransferManager,
}

impl AppState {
    /// 初始化全局状态（默认最大 3 并发传输）
    pub fn new(app_data_dir: PathBuf) -> AppResult<Self> {
        let settings_file = app_data_dir.join("settings.json");
        let loaded_settings = std::fs::read_to_string(&settings_file)
            .ok()
            .and_then(|raw| serde_json::from_str::<AppSettings>(&raw).ok())
            .unwrap_or_default();
        Ok(Self {
            store: SiteStore::new(app_data_dir.clone())?,
            known_hosts: Arc::new(KnownHostsStore::new(app_data_dir.clone())?),
            audit: AuditLogger::new(app_data_dir)?,
            settings_file,
            minimize_to_tray: AtomicBool::new(loaded_settings.minimize_to_tray),
            sessions: RwLock::new(HashMap::new()),
            ptys: Arc::new(RwLock::new(HashMap::new())),
            terminal_windows: RwLock::new(HashMap::new()),
            tunnels: RwLock::new(HashMap::new()),
            transfers: TransferManager::new(3),
        })
    }

    /// 校验调用方是否为主窗口（禁止独立终端窗口 `term-*` 越权调用站点/本地删除/系统配置等高危 IPC）
    fn ensure_main_window(&self, window: &WebviewWindow, action: &str) -> AppResult<()> {
        if window.label() != "main" {
            self.audit.record(
                AuditSeverity::High,
                AuditCategory::SecurityPolicy,
                "ipc_window_denied",
                window.label(),
                &format!("拦截来自非主窗口的敏感操作: {}", action),
            );
            return Err(AppError::PermissionDenied(format!(
                "安全策略拒绝：操作 '{}' 仅允许在主窗口执行",
                action
            )));
        }
        Ok(())
    }

    /// 校验窗口对指定 `session_id` 的访问授权（主窗口可访问全部会话；`term-*` 仅限访问其绑定会话）
    async fn ensure_window_session_access(
        &self,
        window: &WebviewWindow,
        session_id: &str,
        action: &str,
    ) -> AppResult<()> {
        let label = window.label();
        if label == "main" {
            return Ok(());
        }
        let allowed = self
            .terminal_windows
            .read()
            .await
            .get(label)
            .is_some_and(|bound| bound == session_id);
        if !allowed {
            self.audit.record(
                AuditSeverity::High,
                AuditCategory::SecurityPolicy,
                "ipc_session_denied",
                label,
                &format!("拦截跨会话越权调用 '{}' (session={})", action, session_id),
            );
            return Err(AppError::PermissionDenied(
                "安全策略拒绝：当前独立终端窗口无权访问其他远程会话".into(),
            ));
        }
        Ok(())
    }

    /// 在独立终端窗口关闭时回收其会话绑定记录，并兜底关闭该窗口名下全部活跃 PTY（含远端通道与本地子进程）
    pub async fn unregister_terminal_window(&self, label: &str) {
        self.terminal_windows.write().await.remove(label);
        let to_close: Vec<tokio::sync::mpsc::Sender<PtyCommand>> = {
            let mut ptys = self.ptys.write().await;
            let ids: Vec<String> = ptys
                .iter()
                .filter(|(_, p)| p.owner_window == label)
                .map(|(id, _)| id.clone())
                .collect();
            ids.into_iter()
                .filter_map(|id| ptys.remove(&id).map(|p| p.cmd_tx))
                .collect()
        };
        for tx in to_close {
            let _ = tx.send(PtyCommand::Close).await;
        }
    }

    /// 供窗口关闭事件钩子无锁查询是否启用“关闭时最小化到托盘”
    pub fn is_minimize_to_tray(&self) -> bool {
        self.minimize_to_tray.load(Ordering::Relaxed)
    }

    /// 读取当前偏好设置快照
    pub fn current_settings(&self) -> AppSettings {
        AppSettings {
            minimize_to_tray: self.is_minimize_to_tray(),
        }
    }

    /// 持久化偏好设置至独立文件 `settings.json`（Unix 下收紧权限至 `0o600`）
    pub fn set_minimize_to_tray(&self, enabled: bool) -> AppResult<AppSettings> {
        self.minimize_to_tray.store(enabled, Ordering::Relaxed);
        let settings = self.current_settings();
        let json = serde_json::to_string_pretty(&settings)
            .map_err(|e| AppError::Storage(format!("序列化偏好设置失败: {}", e)))?;
        write_private_file(&self.settings_file, &json)?;
        Ok(settings)
    }

    /// 按会话 ID 查找对应的远程文件系统实现
    async fn get_fs(&self, session_id: &str) -> AppResult<Arc<dyn RemoteFs>> {
        let guard = self.sessions.read().await;
        guard
            .get(session_id)
            .map(|s| s.fs.clone())
            .ok_or_else(|| AppError::SessionNotFound(session_id.to_string()))
    }
}

/// 下载任务入队参数（使用结构体传参以保持接口整洁）
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DownloadRequest {
    pub session_id: String,
    pub remote_path: String,
    /// 目标本地保存目录（目录递归下载或拖拽到左侧本地栏时使用）
    pub local_dir: Option<String>,
    /// 目标本地完整文件路径（单文件通过「另存为」选择精确路径时使用）
    pub local_file_path: Option<String>,
    /// 是否为远程文件夹
    #[serde(default)]
    pub is_dir: bool,
    /// 可选文件大小预估值
    pub size_hint: Option<u64>,
}

// ============================================================================
// 站点管理与本地备份导入导出
// ============================================================================

/// 获取所有已保存站点列表
#[tauri::command]
pub async fn list_sites(state: State<'_, AppState>) -> AppResult<Vec<SiteConfig>> {
    state.store.list()
}

/// 新增或更新站点配置（密钥自动托管至操作系统凭据库，仅主窗口可调用）
#[tauri::command]
pub async fn save_site(
    window: WebviewWindow,
    state: State<'_, AppState>,
    input: SiteInput,
) -> AppResult<SiteConfig> {
    state.ensure_main_window(&window, "save_site")?;
    let save_secret = input.save_secret;
    let saved = state.store.save(input)?;
    state.audit.record(
        AuditSeverity::Info,
        AuditCategory::Credential,
        "site_saved",
        &format!("{}:{}", saved.host, saved.port),
        &format!(
            "保存站点「{}」(协议={:?}, 凭据托管={})",
            saved.name, saved.protocol, save_secret
        ),
    );
    Ok(saved)
}

/// 删除站点配置并清理对应的系统凭据（仅主窗口可调用）
#[tauri::command]
pub async fn delete_site(
    window: WebviewWindow,
    state: State<'_, AppState>,
    site_id: String,
) -> AppResult<()> {
    state.ensure_main_window(&window, "delete_site")?;
    let target = state
        .store
        .get(&site_id)
        .map(|s| format!("{} ({}:{})", s.name, s.host, s.port))
        .unwrap_or_else(|_| site_id.clone());
    state.store.delete(&site_id)?;
    state.audit.record(
        AuditSeverity::Warn,
        AuditCategory::Credential,
        "site_deleted",
        &target,
        "已删除站点配置并清理系统凭据库中的关联密钥",
    );
    Ok(())
}

/// 将站点列表导出为本地 JSON 备份文件（不包含敏感密钥，仅主窗口且限 `.json` 文件）
#[tauri::command]
pub async fn export_sites_to_file(
    window: WebviewWindow,
    state: State<'_, AppState>,
    file_path: String,
) -> AppResult<()> {
    state.ensure_main_window(&window, "export_sites_to_file")?;
    validate_scoped_file_path(Path::new(&file_path), &["json"])?;
    let json = state.store.export_json()?;
    tokio::fs::write(&file_path, json).await?;
    state.audit.record(
        AuditSeverity::Info,
        AuditCategory::Credential,
        "sites_exported",
        &file_path,
        "已导出脱敏站点配置备份（不含密钥）",
    );
    Ok(())
}

/// 从本地 JSON 备份文件导入站点配置，返回成功导入的站点数量（仅主窗口且限 `.json` 文件）
#[tauri::command]
pub async fn import_sites_from_file(
    window: WebviewWindow,
    state: State<'_, AppState>,
    file_path: String,
) -> AppResult<usize> {
    state.ensure_main_window(&window, "import_sites_from_file")?;
    validate_scoped_file_path(Path::new(&file_path), &["json"])?;
    let json = tokio::fs::read_to_string(&file_path).await?;
    let count = state.store.import_json(&json)?;
    state.audit.record(
        AuditSeverity::Info,
        AuditCategory::Credential,
        "sites_imported",
        &file_path,
        &format!("已导入 {} 个站点元数据（无密钥）", count),
    );
    Ok(count)
}

/// 检测本地 SSH Agent（Windows OpenSSH Agent / Pageant / Unix ssh-agent）运行状态与密钥数量
#[tauri::command]
pub async fn check_ssh_agent_status() -> AgentStatus {
    sftp::check_ssh_agent_status().await
}

/// 列出所有已信任的 SSH 主机公钥指纹记录
#[tauri::command]
pub async fn list_known_hosts(state: State<'_, AppState>) -> AppResult<Vec<KnownHostEntry>> {
    state.known_hosts.list()
}

/// 移除指定 `host:port` 的已知 SSH 主机公钥指纹（仅主窗口可调用）
#[tauri::command]
pub async fn remove_known_host(
    window: WebviewWindow,
    state: State<'_, AppState>,
    host_port: String,
) -> AppResult<bool> {
    state.ensure_main_window(&window, "remove_known_host")?;
    let removed = state.known_hosts.remove(&host_port)?;
    if removed {
        state.audit.record(
            AuditSeverity::Warn,
            AuditCategory::HostKey,
            "host_key_removed",
            &host_port,
            "用户主动移除了已固定的 SSH 主机公钥指纹记录",
        );
    }
    Ok(removed)
}

/// 经用户核对主机算法与 SHA-256 指纹并明确确认后，信任并保存该 SSH 主机指纹（仅主窗口可调用）
#[tauri::command]
pub async fn trust_known_host(
    window: WebviewWindow,
    state: State<'_, AppState>,
    host_port: String,
    algorithm: String,
    fingerprint: String,
) -> AppResult<KnownHostEntry> {
    state.ensure_main_window(&window, "trust_known_host")?;
    let entry = state
        .known_hosts
        .trust_host(&host_port, &algorithm, &fingerprint)?;
    state.audit.record(
        AuditSeverity::Info,
        AuditCategory::HostKey,
        "host_key_trusted",
        &entry.host_port,
        &format!(
            "用户确认信任 SSH 主机公钥 ({} / {})",
            entry.algorithm, entry.fingerprint
        ),
    );
    Ok(entry)
}

/// 查询最近的脱敏安全审计日志（仅主窗口可调用）
#[tauri::command]
pub async fn list_audit_logs(
    window: WebviewWindow,
    state: State<'_, AppState>,
    limit: Option<usize>,
) -> AppResult<Vec<AuditEntry>> {
    state.ensure_main_window(&window, "list_audit_logs")?;
    state.audit.recent(limit.unwrap_or(200))
}

/// 将安全审计日志导出为本地 JSON 文件（仅主窗口且限 `.json` 文件）
#[tauri::command]
pub async fn export_audit_logs(
    window: WebviewWindow,
    state: State<'_, AppState>,
    file_path: String,
) -> AppResult<()> {
    state.ensure_main_window(&window, "export_audit_logs")?;
    validate_scoped_file_path(Path::new(&file_path), &["json"])?;
    let json = state.audit.export_json(1000)?;
    tokio::fs::write(&file_path, json).await?;
    Ok(())
}

// ============================================================================
// SSH 密钥管理、纯 SFTP 一键部署公钥与 ~/.ssh/config 导入
// ============================================================================

/// 列出本地 `~/.ssh` 目录下的 SSH 密钥元信息（仅主窗口可调用，严禁暴露私钥内容）
#[tauri::command]
pub async fn list_ssh_keys(
    window: WebviewWindow,
    state: State<'_, AppState>,
) -> AppResult<Vec<SshKeyEntry>> {
    state.ensure_main_window(&window, "list_ssh_keys")?;
    tokio::task::spawn_blocking(ssh_keys::list_local_ssh_keys)
        .await
        .map_err(|e| AppError::Storage(format!("扫描本地 SSH 密钥异常: {}", e)))?
}

/// 在本地 `~/.ssh` 目录生成现代 Ed25519 密钥对（支持可选 Passphrase 加密，仅主窗口可调用）
#[tauri::command]
pub async fn generate_ssh_key(
    window: WebviewWindow,
    state: State<'_, AppState>,
    name: String,
    comment: String,
    passphrase: Option<String>,
) -> AppResult<SshKeyEntry> {
    use zeroize::Zeroize;
    state.ensure_main_window(&window, "generate_ssh_key")?;
    let has_pass = passphrase.as_deref().is_some_and(|p| !p.is_empty());
    let mut pass_owned = passphrase;
    let entry = tokio::task::spawn_blocking(move || {
        let res = ssh_keys::generate_local_ed25519_key(&name, &comment, pass_owned.as_deref());
        if let Some(p) = pass_owned.as_mut() {
            p.zeroize();
        }
        res
    })
    .await
    .map_err(|e| AppError::Protocol(format!("生成 SSH 密钥任务异常: {}", e)))??;

    state.audit.record(
        AuditSeverity::Info,
        AuditCategory::Credential,
        "ssh_key_generated",
        &entry.name,
        &format!(
            "已生成 Ed25519 密钥对 ({} / {}, 口令保护={})",
            entry.algorithm, entry.fingerprint, has_pass
        ),
    );
    Ok(entry)
}

/// 通过纯 SFTP 通道将指定 SSH 公钥一键部署至当前已连接的远端主机 `~/.ssh/authorized_keys`
#[tauri::command]
pub async fn deploy_ssh_public_key(
    window: WebviewWindow,
    state: State<'_, AppState>,
    session_id: String,
    public_key_line: String,
) -> AppResult<bool> {
    state.ensure_main_window(&window, "deploy_ssh_public_key")?;
    let (_, alg, fp) = ssh_keys::validate_openssh_public_key(&public_key_line)?;
    let fs = state.get_fs(&session_id).await?;
    let target = {
        let guard = state.sessions.read().await;
        guard
            .get(&session_id)
            .map(|s| format!("{}:{}", s.info.host, s.info.port))
            .unwrap_or_else(|| session_id.clone())
    };
    let added = fs.deploy_public_key(&public_key_line).await?;
    state.audit.record(
        AuditSeverity::Warn,
        AuditCategory::Credential,
        "ssh_key_deployed",
        &target,
        &format!(
            "{} SSH 公钥至远端 authorized_keys ({} / {})",
            if added {
                "已部署"
            } else {
                "已存在跳过"
            },
            alg,
            fp
        ),
    );
    Ok(added)
}

/// 扫描并解析本机 `~/.ssh/config` 中的 SSH 主机配置列表（仅主窗口可调用）
#[tauri::command]
pub async fn discover_ssh_config_hosts(
    window: WebviewWindow,
    state: State<'_, AppState>,
) -> AppResult<Vec<SshConfigHost>> {
    state.ensure_main_window(&window, "discover_ssh_config_hosts")?;
    tokio::task::spawn_blocking(ssh_config::discover_local_ssh_config)
        .await
        .map_err(|e| AppError::Storage(format!("解析 ~/.ssh/config 异常: {}", e)))?
}

/// 将用户勾选的 `~/.ssh/config` 主机批量导入站点列表（自动去重，绝不覆盖或破坏既有站点）
#[tauri::command]
pub async fn import_ssh_config_hosts(
    window: WebviewWindow,
    state: State<'_, AppState>,
    hosts: Vec<SshConfigHost>,
) -> AppResult<usize> {
    state.ensure_main_window(&window, "import_ssh_config_hosts")?;
    let existing = state.store.list()?;
    let mut imported = 0usize;

    for h in hosts {
        let already = existing.iter().any(|s| {
            s.protocol == Protocol::Sftp
                && s.host.eq_ignore_ascii_case(&h.host)
                && s.port == h.port
                && s.username == h.username
        });
        if already {
            continue;
        }
        let auth_type = if h.private_key_path.as_deref().is_some_and(|p| !p.is_empty()) {
            AuthType::PrivateKey
        } else {
            AuthType::Agent
        };
        let input = SiteInput {
            id: None,
            name: h.alias.clone(),
            group: "SSH Config".to_string(),
            protocol: Protocol::Sftp,
            host: h.host,
            port: Some(h.port),
            username: h.username,
            auth_type,
            secret: None,
            passphrase: None,
            session_token: None,
            private_key_path: h.private_key_path,
            remote_path: "/".to_string(),
            endpoint: None,
            bucket: None,
            region: None,
            path_style: false,
            tls_skip_verify: false,
            jump_host: h.jump_host.clone(),
            jump_port: h.jump_port,
            jump_username: h.jump_username,
            jump_auth_type: h.jump_host.as_ref().map(|_| AuthType::Agent),
            jump_private_key_path: None,
            jump_secret: None,
            jump_passphrase: None,
            notes: "导入自 ~/.ssh/config".to_string(),
            save_secret: false,
        };
        state.store.save(input)?;
        imported += 1;
    }

    state.audit.record(
        AuditSeverity::Info,
        AuditCategory::Credential,
        "ssh_config_imported",
        "~/.ssh/config",
        &format!("从 OpenSSH 配置导入 {} 个新 SFTP 站点", imported),
    );
    Ok(imported)
}

// ============================================================================
// 会话连接与生命周期管理
// ============================================================================

/// 连接已保存站点（严格读取系统凭据库中的密钥，仅主窗口可调用）
#[tauri::command]
pub async fn connect_saved_site(
    window: WebviewWindow,
    state: State<'_, AppState>,
    site_id: String,
    override_secret: Option<String>,
    override_passphrase: Option<String>,
    override_jump_secret: Option<String>,
    override_jump_passphrase: Option<String>,
) -> AppResult<SessionInfo> {
    state.ensure_main_window(&window, "connect_saved_site")?;
    let site = state.store.get(&site_id)?;
    let has_override = override_secret.as_deref().is_some_and(|v| !v.is_empty())
        || override_passphrase
            .as_deref()
            .is_some_and(|v| !v.is_empty());
    let mut secrets = match load_secrets_strict(&site_id, site.has_saved_secret && !has_override) {
        Ok(s) => s,
        Err(e) => {
            state.audit.record(
                AuditSeverity::High,
                AuditCategory::Credential,
                "credential_load_failed",
                &format!("{}:{}", site.host, site.port),
                &e.to_string(),
            );
            return Err(e);
        }
    };
    if let Some(s) = override_secret.filter(|v| !v.is_empty()) {
        secrets.secret = Some(s);
    }
    if let Some(p) = override_passphrase.filter(|v| !v.is_empty()) {
        secrets.passphrase = Some(p);
    }
    if let Some(js) = override_jump_secret.filter(|v| !v.is_empty()) {
        secrets.jump_secret = Some(js);
    }
    if let Some(jp) = override_jump_passphrase.filter(|v| !v.is_empty()) {
        secrets.jump_passphrase = Some(jp);
    }
    create_session(&state, site, secrets).await
}

/// 使用临时表单参数建立一次性连接（不落盘保存，仅主窗口可调用）
#[tauri::command]
pub async fn connect_adhoc(
    window: WebviewWindow,
    state: State<'_, AppState>,
    mut input: SiteInput,
) -> AppResult<SessionInfo> {
    state.ensure_main_window(&window, "connect_adhoc")?;
    let secrets = SiteSecrets {
        secret: input.secret.take(),
        passphrase: input.passphrase.take(),
        session_token: input.session_token.take(),
        jump_secret: input.jump_secret.take(),
        jump_passphrase: input.jump_passphrase.take(),
    };
    let jump_host = input.jump_host.take().filter(|s| !s.trim().is_empty());
    let site = SiteConfig {
        id: input.id.take().unwrap_or_default(),
        name: if input.name.trim().is_empty() {
            format!("{}@{}", input.username, input.host)
        } else {
            input.name.trim().to_string()
        },
        group: std::mem::take(&mut input.group),
        protocol: input.protocol,
        host: input.host.trim().to_string(),
        port: input.port.unwrap_or_else(|| input.protocol.default_port()),
        username: input.username.trim().to_string(),
        auth_type: input.auth_type,
        private_key_path: input
            .private_key_path
            .take()
            .filter(|s| !s.trim().is_empty()),
        remote_path: normalize_remote_path(&input.remote_path),
        endpoint: input.endpoint.take().filter(|s| !s.trim().is_empty()),
        bucket: input.bucket.take().filter(|s| !s.trim().is_empty()),
        region: input.region.take().filter(|s| !s.trim().is_empty()),
        path_style: input.path_style,
        tls_skip_verify: input.tls_skip_verify,
        jump_port: jump_host.as_ref().map(|_| input.jump_port.unwrap_or(22)),
        jump_username: jump_host
            .as_ref()
            .and_then(|_| input.jump_username.take().filter(|s| !s.trim().is_empty())),
        jump_auth_type: jump_host.as_ref().and(input.jump_auth_type),
        jump_private_key_path: jump_host.as_ref().and_then(|_| {
            input
                .jump_private_key_path
                .take()
                .filter(|s| !s.trim().is_empty())
        }),
        jump_host,
        notes: std::mem::take(&mut input.notes),
        has_saved_secret: false,
        updated_at: Utc::now().timestamp(),
    };
    create_session(&state, site, secrets).await
}

/// 解析地址栏快速直连 URI 并立即建立远程会话（仅主窗口可调用）
#[tauri::command]
pub async fn quick_connect_uri(
    window: WebviewWindow,
    state: State<'_, AppState>,
    uri: String,
) -> AppResult<SessionInfo> {
    state.ensure_main_window(&window, "quick_connect_uri")?;
    let parsed = parse_quick_connect_uri(&uri)?;
    connect_adhoc(window, state, parsed).await
}

/// 建立远程连接并注册到全局活跃会话池（同步写入脱敏安全审计日志）
async fn create_session(
    state: &State<'_, AppState>,
    site: SiteConfig,
    secrets: SiteSecrets,
) -> AppResult<SessionInfo> {
    let target_label = format!("{}:{}", site.host, site.port);
    let site_for_connect = site.clone();
    let secrets_for_connect = secrets.clone();
    let known_hosts = state.known_hosts.clone();
    // 在独立异步任务中完成网络握手，避免连接过程阻塞状态锁
    let connect_res = tokio::spawn(async move {
        connect_protocol(site_for_connect, secrets_for_connect, known_hosts).await
    })
    .await
    .map_err(|e| AppError::Connection(format!("连接任务异常: {}", e)))?;

    let fs = match connect_res {
        Ok(fs) => fs,
        Err(err) => {
            match &err {
                AppError::HostKeyUnknown(_) => state.audit.record(
                    AuditSeverity::Info,
                    AuditCategory::HostKey,
                    "host_key_confirm_required",
                    &target_label,
                    "首次连接未知 SSH 主机，已暂停连接等待用户核对公钥指纹",
                ),
                AppError::HostKeyMismatch(_) => state.audit.record(
                    AuditSeverity::Critical,
                    AuditCategory::HostKey,
                    "host_key_mismatch",
                    &target_label,
                    "检测到 SSH 主机公钥指纹变化，已立即阻断连接以防范中间人攻击",
                ),
                AppError::SecurityPolicy(msg) => state.audit.record(
                    AuditSeverity::High,
                    AuditCategory::SecurityPolicy,
                    "connect_policy_blocked",
                    &target_label,
                    msg,
                ),
                AppError::Auth(msg) => state.audit.record(
                    AuditSeverity::Warn,
                    AuditCategory::Session,
                    "auth_failed",
                    &target_label,
                    msg,
                ),
                other => state.audit.record(
                    AuditSeverity::Warn,
                    AuditCategory::Session,
                    "connect_failed",
                    &target_label,
                    &other.to_string(),
                ),
            }
            return Err(err);
        }
    };

    let insecure_transport = matches!(site.protocol, Protocol::Ftp)
        || (matches!(site.protocol, Protocol::Webdav | Protocol::S3)
            && site
                .endpoint
                .as_deref()
                .is_some_and(|e| e.to_ascii_lowercase().starts_with("http://")));

    let session_id = Uuid::new_v4().to_string();
    let info = SessionInfo {
        session_id: session_id.clone(),
        site_id: if site.id.is_empty() {
            None
        } else {
            Some(site.id.clone())
        },
        name: site.name.clone(),
        protocol: site.protocol,
        host: site.host.clone(),
        port: site.port,
        current_path: normalize_remote_path(&site.remote_path),
        supports_shell: site.protocol.supports_shell(),
        supports_chmod: site.protocol.supports_chmod(),
        insecure_transport,
        tls_skip_verify: site.tls_skip_verify,
    };

    state.sessions.write().await.insert(
        session_id,
        ActiveSession {
            info: info.clone(),
            fs,
        },
    );

    let severity = if insecure_transport || site.tls_skip_verify {
        AuditSeverity::Warn
    } else {
        AuditSeverity::Info
    };
    state.audit.record(
        severity,
        AuditCategory::Session,
        "session_connected",
        &target_label,
        &format!(
            "已建立 {:?} 会话「{}」(明文={}, 跳过TLS校验={})",
            site.protocol, site.name, insecure_transport, site.tls_skip_verify
        ),
    );
    Ok(info)
}

/// 主动断开指定远程会话并彻底释放底层连接、关联传输任务、PTY 通道与独立终端窗口
#[tauri::command]
pub async fn disconnect_session(
    app: AppHandle,
    window: WebviewWindow,
    state: State<'_, AppState>,
    session_id: String,
) -> AppResult<()> {
    state.ensure_main_window(&window, "disconnect_session")?;

    // 1. 取消该会话名下所有尚未完成的排队/运行中传输任务并同步通知前端
    let cancelled_tasks = state.transfers.cancel_by_session(&session_id).await;
    for task in cancelled_tasks {
        let _ = app.emit(TRANSFER_EVENT, task);
    }

    // 2. 关闭该会话名下所有活跃 PTY 通道
    let to_close: Vec<tokio::sync::mpsc::Sender<PtyCommand>> = {
        let mut ptys = state.ptys.write().await;
        let ids: Vec<String> = ptys
            .iter()
            .filter(|(_, p)| p.session_id == session_id)
            .map(|(id, _)| id.clone())
            .collect();
        ids.into_iter()
            .filter_map(|id| ptys.remove(&id).map(|p| p.cmd_tx))
            .collect()
    };
    for tx in to_close {
        let _ = tx.send(PtyCommand::Close).await;
    }

    // 3. 关闭该会话名下所有活跃 SSH 端口转发隧道
    let tunnels_to_stop: Vec<tokio::sync::oneshot::Sender<()>> = {
        let mut map = state.tunnels.write().await;
        let ids: Vec<String> = map
            .iter()
            .filter(|(_, t)| t.info.session_id == session_id)
            .map(|(id, _)| id.clone())
            .collect();
        ids.into_iter()
            .filter_map(|id| map.remove(&id).and_then(|mut t| t.shutdown_tx.take()))
            .collect()
    };
    for tx in tunnels_to_stop {
        let _ = tx.send(());
    }

    // 4. 关闭所有绑定到该会话的独立终端窗口（term-*）
    let bound_windows: Vec<String> = {
        let mut map = state.terminal_windows.write().await;
        let labels: Vec<String> = map
            .iter()
            .filter(|(_, sid)| *sid == &session_id)
            .map(|(lbl, _)| lbl.clone())
            .collect();
        for lbl in &labels {
            map.remove(lbl);
        }
        labels
    };
    for lbl in bound_windows {
        if let Some(win) = app.get_webview_window(&lbl) {
            let _ = win.close();
        }
    }

    // 4. 移除会话并记录审计日志
    let removed = state.sessions.write().await.remove(&session_id);
    if let Some(sess) = removed {
        state.audit.record(
            AuditSeverity::Info,
            AuditCategory::Session,
            "session_disconnected",
            &format!("{}:{}", sess.info.host, sess.info.port),
            &format!(
                "已断开 {:?} 会话「{}」并清理全部关联资源",
                sess.info.protocol, sess.info.name
            ),
        );
    }
    Ok(())
}

/// 列出当前所有已连接的远程会话
#[tauri::command]
pub async fn list_sessions(state: State<'_, AppState>) -> AppResult<Vec<SessionInfo>> {
    let guard = state.sessions.read().await;
    Ok(guard.values().map(|s| s.info.clone()).collect())
}

// ============================================================================
// 远程文件系统操作
// ============================================================================

/// 列举指定远程目录下的文件与子目录
#[tauri::command]
pub async fn remote_list_dir(
    state: State<'_, AppState>,
    session_id: String,
    path: String,
) -> AppResult<Vec<FileEntry>> {
    let fs = state.get_fs(&session_id).await?;
    let norm = normalize_remote_path(&path);
    let entries = fs.list_dir(&norm).await?;
    if let Some(sess) = state.sessions.write().await.get_mut(&session_id) {
        sess.info.current_path = norm;
    }
    Ok(entries)
}

/// 在远程服务器新建文件夹
#[tauri::command]
pub async fn remote_mkdir(
    state: State<'_, AppState>,
    session_id: String,
    path: String,
) -> AppResult<()> {
    let fs = state.get_fs(&session_id).await?;
    fs.mkdir(&path).await
}

/// 删除远程文件或目录（仅主窗口可调用，并写入安全审计日志）
#[tauri::command]
pub async fn remote_delete(
    window: WebviewWindow,
    state: State<'_, AppState>,
    session_id: String,
    path: String,
    is_dir: bool,
) -> AppResult<()> {
    state.ensure_main_window(&window, "remote_delete")?;
    let fs = state.get_fs(&session_id).await?;
    fs.remove(&path, is_dir).await?;
    state.audit.record(
        AuditSeverity::Warn,
        AuditCategory::FileOp,
        "remote_delete",
        &path,
        &format!(
            "删除远程{} (session={})",
            if is_dir { "目录" } else { "文件" },
            session_id
        ),
    );
    Ok(())
}

/// 重命名或移动远程文件/目录（仅主窗口可调用）
#[tauri::command]
pub async fn remote_rename(
    window: WebviewWindow,
    state: State<'_, AppState>,
    session_id: String,
    from: String,
    to: String,
) -> AppResult<()> {
    state.ensure_main_window(&window, "remote_rename")?;
    let fs = state.get_fs(&session_id).await?;
    fs.rename(&from, &to).await?;
    state.audit.record(
        AuditSeverity::Info,
        AuditCategory::FileOp,
        "remote_rename",
        &from,
        &format!("重命名为 {}", to),
    );
    Ok(())
}

/// 修改远程文件/目录 Unix 权限（CHMOD，仅主窗口可调用并记录审计日志）
#[tauri::command]
pub async fn remote_chmod(
    window: WebviewWindow,
    state: State<'_, AppState>,
    session_id: String,
    path: String,
    mode: u32,
) -> AppResult<()> {
    state.ensure_main_window(&window, "remote_chmod")?;
    let fs = state.get_fs(&session_id).await?;
    fs.chmod(&path, mode).await?;
    state.audit.record(
        AuditSeverity::Warn,
        AuditCategory::FileOp,
        "remote_chmod",
        &path,
        &format!("修改远程权限为 {:03o}", mode & 0o7777),
    );
    Ok(())
}

/// 读取远程文本文件内容用于在线快速预览与编辑（限制 2 MB 以防内存溢出）
#[tauri::command]
pub async fn remote_read_text(
    window: WebviewWindow,
    state: State<'_, AppState>,
    session_id: String,
    path: String,
) -> AppResult<String> {
    state.ensure_main_window(&window, "remote_read_text")?;
    let fs = state.get_fs(&session_id).await?;
    let bytes = fs.read_bytes(&path, 2 * 1024 * 1024).await?;
    Ok(String::from_utf8_lossy(&bytes).to_string())
}

/// 将在线编辑后的文本内容直接回传保存至远程文件（仅主窗口可调用）
#[tauri::command]
pub async fn remote_write_text(
    window: WebviewWindow,
    state: State<'_, AppState>,
    session_id: String,
    path: String,
    content: String,
) -> AppResult<()> {
    state.ensure_main_window(&window, "remote_write_text")?;
    let fs = state.get_fs(&session_id).await?;
    fs.write_bytes(&path, content.as_bytes()).await?;
    state.audit.record(
        AuditSeverity::Info,
        AuditCategory::FileOp,
        "remote_write_text",
        &path,
        &format!("在线编辑并覆盖保存远程文件 ({} 字节)", content.len()),
    );
    Ok(())
}

// ============================================================================
// 本地文件系统操作（仅主窗口可调用，且受后端路径安全策略约束）
// ============================================================================

/// 获取本地用户主目录
#[tauri::command]
pub fn local_home_dir() -> String {
    local_fs::home_dir()
}

/// 列出本地常用快捷位置与磁盘根路径
#[tauri::command]
pub fn local_list_roots() -> Vec<LocalRoot> {
    local_fs::list_roots()
}

/// 列举本地目录内容（仅主窗口可调用）
#[tauri::command]
pub async fn local_list_dir(
    window: WebviewWindow,
    state: State<'_, AppState>,
    path: String,
) -> AppResult<Vec<FileEntry>> {
    state.ensure_main_window(&window, "local_list_dir")?;
    local_fs::list_dir(&path).await
}

/// 在本地创建文件夹（仅主窗口可调用）
#[tauri::command]
pub async fn local_mkdir(
    window: WebviewWindow,
    state: State<'_, AppState>,
    path: String,
) -> AppResult<()> {
    state.ensure_main_window(&window, "local_mkdir")?;
    local_fs::mkdir(&path).await
}

/// 删除本地文件或文件夹（仅主窗口可调用，并记录审计日志与拦截危险路径）
#[tauri::command]
pub async fn local_delete(
    window: WebviewWindow,
    state: State<'_, AppState>,
    path: String,
    is_dir: bool,
) -> AppResult<()> {
    state.ensure_main_window(&window, "local_delete")?;
    if let Err(err) = local_fs::delete(&path, is_dir).await {
        if matches!(err, AppError::SecurityPolicy(_)) {
            state.audit.record(
                AuditSeverity::High,
                AuditCategory::SecurityPolicy,
                "local_delete_blocked",
                &path,
                &err.to_string(),
            );
        }
        return Err(err);
    }
    state.audit.record(
        AuditSeverity::Warn,
        AuditCategory::FileOp,
        "local_delete",
        &path,
        &format!("删除本地{}", if is_dir { "目录" } else { "文件" }),
    );
    Ok(())
}

/// 重命名本地文件或文件夹（仅主窗口可调用）
#[tauri::command]
pub async fn local_rename(
    window: WebviewWindow,
    state: State<'_, AppState>,
    from: String,
    to: String,
) -> AppResult<()> {
    state.ensure_main_window(&window, "local_rename")?;
    local_fs::rename(&from, &to).await
}

// ============================================================================
// 传输队列（支持单文件与文件夹递归上传/下载）
// ============================================================================

/// 递归扫描本地路径后生成的远程建目录计划与文件上传清单
struct UploadPlan {
    remote_dirs: Vec<String>,
    files: Vec<(PathBuf, String, u64)>,
}

/// 扫描一批本地文件/文件夹，生成对应的远程目标目录与文件映射计划
fn collect_local_upload_plan(local_paths: &[String], remote_base: &str) -> AppResult<UploadPlan> {
    let mut plan = UploadPlan {
        remote_dirs: Vec::new(),
        files: Vec::new(),
    };
    let remote_root = normalize_remote_path(remote_base);

    for raw in local_paths {
        let path = PathBuf::from(raw);
        let meta = std::fs::metadata(&path).map_err(|e| {
            AppError::Io(std::io::Error::new(
                e.kind(),
                format!("无法访问本地路径 '{}': {}", raw, e),
            ))
        })?;
        let name = path
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .ok_or_else(|| AppError::InvalidInput(format!("无法解析文件名: {}", raw)))?;

        if meta.is_file() {
            plan.files
                .push((path, join_remote_path(&remote_root, &name), meta.len()));
        } else if meta.is_dir() {
            let target_dir = join_remote_path(&remote_root, &name);
            walk_local_dir(&path, &target_dir, &mut plan)?;
        }
    }

    Ok(plan)
}

/// 使用显式栈递归遍历本地子目录，收集需要创建的远程目录与全部文件
fn walk_local_dir(local_dir: &Path, remote_dir: &str, plan: &mut UploadPlan) -> AppResult<()> {
    plan.remote_dirs.push(remote_dir.to_string());
    let mut stack = vec![(local_dir.to_path_buf(), remote_dir.to_string())];

    while let Some((cur_local, cur_remote)) = stack.pop() {
        let entries = match std::fs::read_dir(&cur_local) {
            Ok(rd) => rd,
            Err(_) => continue,
        };
        for item in entries.flatten() {
            let p = item.path();
            let Ok(meta) = item.metadata() else {
                continue;
            };
            let name = item.file_name().to_string_lossy().to_string();
            let next_remote = join_remote_path(&cur_remote, &name);
            if meta.is_dir() {
                plan.remote_dirs.push(next_remote.clone());
                stack.push((p, next_remote));
            } else if meta.is_file() {
                plan.files.push((p, next_remote, meta.len()));
            }
        }
    }
    Ok(())
}

/// 批量上传本地文件或文件夹（支持拖拽与多选，按窗口归属校验会话权限）
#[tauri::command]
pub async fn enqueue_upload_paths(
    app: AppHandle,
    window: WebviewWindow,
    state: State<'_, AppState>,
    session_id: String,
    local_paths: Vec<String>,
    remote_dir: String,
) -> AppResult<Vec<TransferTask>> {
    state
        .ensure_window_session_access(&window, &session_id, "enqueue_upload_paths")
        .await?;
    if local_paths.is_empty() {
        return Ok(Vec::new());
    }
    let fs = state.get_fs(&session_id).await?;
    let plan =
        tokio::task::spawn_blocking(move || collect_local_upload_plan(&local_paths, &remote_dir))
            .await
            .map_err(|e| AppError::Protocol(format!("扫描本地目录失败: {}", e)))??;

    // 先按层级创建所有需要的远程目录（若目录已存在则忽略错误）
    for rdir in &plan.remote_dirs {
        let _ = fs.mkdir(rdir).await;
    }

    let mut queued = Vec::with_capacity(plan.files.len());
    for (lp, rp, size) in plan.files {
        let task = state
            .transfers
            .enqueue(
                app.clone(),
                fs.clone(),
                session_id.clone(),
                TransferDirection::Upload,
                lp.to_string_lossy().to_string(),
                rp,
                size,
            )
            .await?;
        queued.push(task);
    }

    Ok(queued)
}

/// 上传单个本地文件或文件夹（底层复用批量递归上传逻辑）
#[tauri::command]
pub async fn enqueue_upload(
    app: AppHandle,
    window: WebviewWindow,
    state: State<'_, AppState>,
    session_id: String,
    local_path: String,
    remote_dir: String,
) -> AppResult<Vec<TransferTask>> {
    enqueue_upload_paths(app, window, state, session_id, vec![local_path], remote_dir).await
}

/// 下载远程单个文件（支持「另存为」精确路径）或递归下载整个远程文件夹（仅主窗口可调用）
#[tauri::command]
pub async fn enqueue_download(
    app: AppHandle,
    window: WebviewWindow,
    state: State<'_, AppState>,
    request: DownloadRequest,
) -> AppResult<Vec<TransferTask>> {
    state.ensure_main_window(&window, "enqueue_download")?;
    let fs = state.get_fs(&request.session_id).await?;
    let norm = normalize_remote_path(&request.remote_path);
    let file_name = norm
        .rsplit('/')
        .next()
        .filter(|s| !s.is_empty())
        .ok_or_else(|| AppError::InvalidInput("无法解析远程名称".into()))?;

    // 单文件下载模式（严格校验文件名、防盘符覆盖与防本地符号链接覆写）
    if !request.is_dir {
        let local_path =
            if let Some(exact) = request.local_file_path.filter(|s| !s.trim().is_empty()) {
                ensure_not_symlink_target(Path::new(&exact))?;
                exact
            } else {
                let dir = request
                    .local_dir
                    .filter(|s| !s.trim().is_empty())
                    .ok_or_else(|| AppError::InvalidInput("请指定本地下载保存路径".into()))?;
                resolve_safe_download_path(Path::new(&dir), file_name)?
                    .to_string_lossy()
                    .to_string()
            };
        let task = state
            .transfers
            .enqueue(
                app,
                fs,
                request.session_id,
                TransferDirection::Download,
                local_path,
                norm,
                request.size_hint.unwrap_or(0),
            )
            .await?;
        return Ok(vec![task]);
    }

    // 远程文件夹递归下载模式（根目录及全部嵌套子项均强制经过安全路径解析，杜绝路径穿越与保留设备名）
    let target_dir = request
        .local_dir
        .filter(|s| !s.trim().is_empty())
        .ok_or_else(|| AppError::InvalidInput("请指定本地下载保存目录".into()))?;
    let root_local = resolve_safe_download_path(Path::new(&target_dir), file_name)?;
    tokio::fs::create_dir_all(&root_local).await?;

    let mut stack = vec![(norm, root_local)];
    let mut queued = Vec::new();

    while let Some((cur_remote, cur_local)) = stack.pop() {
        let entries = fs.list_dir(&cur_remote).await?;
        for entry in entries {
            let Ok(next_local) = resolve_safe_download_path(&cur_local, &entry.name) else {
                // 跳过恶意服务端返回的非法/穿越/保留设备名/符号链接目标条目
                continue;
            };
            if entry.is_dir && !entry.is_symlink {
                tokio::fs::create_dir_all(&next_local).await?;
                stack.push((entry.path, next_local));
            } else if !entry.is_dir && !entry.is_symlink {
                let task = state
                    .transfers
                    .enqueue(
                        app.clone(),
                        fs.clone(),
                        request.session_id.clone(),
                        TransferDirection::Download,
                        next_local.to_string_lossy().to_string(),
                        entry.path,
                        entry.size,
                    )
                    .await?;
                queued.push(task);
            }
        }
    }

    Ok(queued)
}

/// 查询当前全部传输任务
#[tauri::command]
pub async fn list_transfers(state: State<'_, AppState>) -> AppResult<Vec<TransferTask>> {
    Ok(state.transfers.list().await)
}

/// 取消指定传输任务并立即通知前端刷新队列状态
#[tauri::command]
pub async fn cancel_transfer(
    app: AppHandle,
    state: State<'_, AppState>,
    task_id: String,
) -> AppResult<()> {
    if let Some(task) = state.transfers.cancel(&task_id).await? {
        let _ = app.emit(TRANSFER_EVENT, task);
    }
    Ok(())
}

/// 清理已完成、失败或已取消的历史传输记录
#[tauri::command]
pub async fn clear_finished_transfers(state: State<'_, AppState>) -> AppResult<()> {
    state.transfers.clear_finished().await;
    Ok(())
}

// ============================================================================
// 交互式 SSH PTY 终端与远端服务器硬件监测
// ============================================================================

/// 采集当前 SFTP/SSH 会话远端服务器的硬件规格、资源占用与多磁盘挂载点
#[tauri::command]
pub async fn remote_server_metrics(
    state: State<'_, AppState>,
    session_id: String,
) -> AppResult<ServerMetrics> {
    let fs = state.get_fs(&session_id).await?;
    fs.server_metrics().await
}

/// 采集当前本机操作系统的实时硬件规格、网卡流量与多磁盘信息（Windows 静默无黑框执行）
#[tauri::command]
pub async fn local_server_metrics() -> AppResult<ServerMetrics> {
    tokio::task::spawn_blocking(|| -> AppResult<ServerMetrics> {
        #[cfg(windows)]
        let output = {
            use std::os::windows::process::CommandExt;
            const CREATE_NO_WINDOW: u32 = 0x0800_0000;
            std::process::Command::new("powershell.exe")
                .args([
                    "-NoProfile",
                    "-Command",
                    sftp::build_windows_metrics_script(),
                ])
                .creation_flags(CREATE_NO_WINDOW)
                .output()
                .map_err(|e| AppError::Protocol(format!("采集本机 Windows 监控指标失败: {}", e)))?
        };

        #[cfg(not(windows))]
        let output = {
            let script = r#"
OS=$(grep -m1 "^PRETTY_NAME=" /etc/os-release 2>/dev/null | cut -d= -f2- | tr -d "\"")
[ -z "$OS" ] && OS=$(uname -s 2>/dev/null || echo "Unix")
OS_FAM="linux"
[ "$(uname -s 2>/dev/null)" = "Darwin" ] && OS_FAM="macos"
HOST=$(hostname 2>/dev/null || true)
UPTIME=$(awk '{print int($1)}' /proc/uptime 2>/dev/null || true)
CORES=$(nproc 2>/dev/null || sysctl -n hw.ncpu 2>/dev/null || echo 0)
LOAD=$(awk '{print $1, $2, $3}' /proc/loadavg 2>/dev/null || true)
MEM_T=0; MEM_U=0
if [ -r /proc/meminfo ]; then
  MEM_T=$(awk '/^MemTotal:/{print $2*1024}' /proc/meminfo)
  MEM_A=$(awk '/^MemAvailable:/{print $2*1024}' /proc/meminfo)
  MEM_U=$((MEM_T - MEM_A))
fi
echo "OS_FAMILY=$OS_FAM"
echo "OS=$OS"
echo "HOST=$HOST"
echo "UPTIME=$UPTIME"
echo "CORES=$CORES"
echo "LOAD=$LOAD"
echo "MEM=$MEM_U:$MEM_T"
echo "DISKS_BEGIN"
(df -kP -T 2>/dev/null || df -kP 2>/dev/null) | tail -n +2
echo "DISKS_END"
"#;
            std::process::Command::new("sh")
                .args(["-c", script])
                .output()
                .map_err(|e| AppError::Protocol(format!("采集本机系统监控指标失败: {}", e)))?
        };

        let stdout = String::from_utf8_lossy(&output.stdout);
        sftp::parse_server_metrics(&stdout)
            .ok_or_else(|| AppError::Protocol("无法解析本机硬件与磁盘监控数据".into()))
    })
    .await
    .map_err(|e| AppError::Protocol(format!("本机监控采集任务异常: {}", e)))?
}

/// 为指定 SSH 会话创建本地回环端口转发隧道（`127.0.0.1:local_port -> remote_host:remote_port`）
#[tauri::command]
pub async fn ssh_tunnel_start(
    window: WebviewWindow,
    state: State<'_, AppState>,
    session_id: String,
    local_port: u16,
    remote_host: String,
    remote_port: u16,
) -> AppResult<SshTunnelInfo> {
    state
        .ensure_window_session_access(&window, &session_id, "ssh_tunnel_start")
        .await?;
    let fs = state.get_fs(&session_id).await?;
    let runtime = fs
        .start_local_forward(local_port, remote_host.clone(), remote_port)
        .await?;

    let id = Uuid::new_v4().to_string();
    let info = SshTunnelInfo {
        id: id.clone(),
        session_id: session_id.clone(),
        local_port: runtime.local_port,
        remote_host: remote_host.trim().to_string(),
        remote_port,
        active_connections: 0,
        created_at: Utc::now().timestamp(),
    };
    state.tunnels.write().await.insert(
        id,
        ActiveTunnel {
            info: info.clone(),
            active_connections: runtime.active_connections,
            shutdown_tx: Some(runtime.shutdown_tx),
        },
    );
    state.audit.record(
        AuditSeverity::Info,
        AuditCategory::Session,
        "ssh_tunnel_started",
        &format!(
            "127.0.0.1:{} -> {}:{}",
            info.local_port, info.remote_host, info.remote_port
        ),
        &format!("已建立 SSH 本地端口转发隧道 (session={})", session_id),
    );
    Ok(info)
}

/// 查询指定 SSH 会话当前全部活跃的端口转发隧道
#[tauri::command]
pub async fn ssh_tunnel_list(
    window: WebviewWindow,
    state: State<'_, AppState>,
    session_id: String,
) -> AppResult<Vec<SshTunnelInfo>> {
    state
        .ensure_window_session_access(&window, &session_id, "ssh_tunnel_list")
        .await?;
    let guard = state.tunnels.read().await;
    let mut list: Vec<SshTunnelInfo> = guard
        .values()
        .filter(|t| t.info.session_id == session_id)
        .map(|t| {
            let mut item = t.info.clone();
            item.active_connections = t.active_connections.load(Ordering::Relaxed);
            item
        })
        .collect();
    list.sort_by_key(|t| t.created_at);
    Ok(list)
}

/// 停止并移除指定的 SSH 本地端口转发隧道
#[tauri::command]
pub async fn ssh_tunnel_stop(
    window: WebviewWindow,
    state: State<'_, AppState>,
    tunnel_id: String,
) -> AppResult<()> {
    let session_id = {
        let guard = state.tunnels.read().await;
        guard
            .get(&tunnel_id)
            .map(|t| t.info.session_id.clone())
            .ok_or_else(|| AppError::SessionNotFound("端口转发隧道不存在或已停止".into()))?
    };
    state
        .ensure_window_session_access(&window, &session_id, "ssh_tunnel_stop")
        .await?;

    let removed = state.tunnels.write().await.remove(&tunnel_id);
    if let Some(mut tunnel) = removed {
        if let Some(tx) = tunnel.shutdown_tx.take() {
            let _ = tx.send(());
        }
        state.audit.record(
            AuditSeverity::Info,
            AuditCategory::Session,
            "ssh_tunnel_stopped",
            &format!(
                "127.0.0.1:{} -> {}:{}",
                tunnel.info.local_port, tunnel.info.remote_host, tunnel.info.remote_port
            ),
            "已停止 SSH 本地端口转发隧道",
        );
    }
    Ok(())
}

/// 枚举当前操作系统可用的白名单本地 Shell 预设
#[tauri::command]
pub fn list_local_shells() -> Vec<LocalShellOption> {
    local_pty::list_local_shells()
}

/// 读取当前操作系统的默认本地终端配置（Windows 下自动同步 Windows Terminal 默认配置）
#[tauri::command]
pub fn get_local_terminal_defaults() -> LocalTerminalDefaults {
    local_pty::get_local_terminal_defaults()
}

/// 读取本机真实 Shell 历史记录（Windows PSReadLine / Git Bash / Unix Zsh & Bash）
#[tauri::command]
pub async fn local_shell_history(limit: Option<usize>) -> AppResult<Vec<String>> {
    let max_items = limit.unwrap_or(400);
    tokio::task::spawn_blocking(move || local_pty::read_local_shell_history(max_items))
        .await
        .map_err(|e| AppError::Protocol(format!("读取本机 Shell 历史记录失败: {}", e)))
}

/// 读取远端 SSH 主机的真实 Shell 历史记录（纯 SFTP 读取 `~/.zsh_history` 等并自动脱敏）
#[tauri::command]
pub async fn remote_shell_history(
    window: WebviewWindow,
    state: State<'_, AppState>,
    session_id: String,
    limit: Option<usize>,
) -> AppResult<Vec<String>> {
    state
        .ensure_window_session_access(&window, &session_id, "remote_shell_history")
        .await?;
    let fs = state.get_fs(&session_id).await?;
    fs.read_shell_history(limit.unwrap_or(400)).await
}

/// 打开本地原生交互终端（PowerShell / Git Bash / CMD / Unix Shell），严格走白名单与窗口归属隔离
#[tauri::command]
pub async fn local_pty_open(
    app: AppHandle,
    window: WebviewWindow,
    state: State<'_, AppState>,
    profile: String,
    cwd: Option<String>,
    cols: u32,
    rows: u32,
) -> AppResult<String> {
    let owner_window = window.label().to_string();
    if owner_window != "main" && !owner_window.starts_with("term-") {
        return Err(AppError::PermissionDenied(
            "安全策略拒绝：未知窗口禁止打开本地终端".into(),
        ));
    }

    let profile_for_spawn = profile.clone();
    let cwd_for_spawn = cwd.clone();
    let mut handle = tokio::task::spawn_blocking(move || {
        local_pty::open_local_pty(&profile_for_spawn, cwd_for_spawn.as_deref(), cols, rows)
    })
    .await
    .map_err(|e| AppError::Protocol(format!("启动本地终端任务异常: {}", e)))??;

    let terminal_id = Uuid::new_v4().to_string();
    state.ptys.write().await.insert(
        terminal_id.clone(),
        ActivePty {
            session_id: "__local__".to_string(),
            owner_window: owner_window.clone(),
            cmd_tx: handle.cmd_tx,
        },
    );

    state.audit.record(
        AuditSeverity::Info,
        AuditCategory::Session,
        "local_pty_opened",
        &profile,
        &format!("在窗口 {} 中打开本地终端 ({})", owner_window, profile),
    );

    let ptys_ref = state.ptys.clone();
    let tid = terminal_id.clone();
    tokio::spawn(async move {
        let mut window_start = std::time::Instant::now();
        let mut bytes_in_window: usize = 0;

        while let Some(chunk) = handle.output_rx.recv().await {
            for slice in chunk.chunks(PTY_MAX_CHUNK_BYTES) {
                if window_start.elapsed().as_secs() >= 1 {
                    window_start = std::time::Instant::now();
                    bytes_in_window = 0;
                }
                bytes_in_window = bytes_in_window.saturating_add(slice.len());
                if bytes_in_window > PTY_MAX_BYTES_PER_SEC {
                    tokio::time::sleep(std::time::Duration::from_millis(20)).await;
                }
                let data_b64 = base64::engine::general_purpose::STANDARD.encode(slice);
                let _ = app.emit_to(
                    &owner_window,
                    PTY_DATA_EVENT,
                    PtyDataEvent {
                        terminal_id: tid.clone(),
                        data_b64,
                    },
                );
            }
        }
        ptys_ref.write().await.remove(&tid);
        let _ = app.emit_to(
            &owner_window,
            PTY_EXIT_EVENT,
            PtyExitEvent { terminal_id: tid },
        );
    });

    Ok(terminal_id)
}

/// 打开交互式 SSH PTY 伪终端（校验窗口与会话归属，并启用输出速率/分片限流防刷屏 DoS）
#[tauri::command]
pub async fn ssh_pty_open(
    app: AppHandle,
    window: WebviewWindow,
    state: State<'_, AppState>,
    session_id: String,
    cols: u32,
    rows: u32,
) -> AppResult<String> {
    state
        .ensure_window_session_access(&window, &session_id, "ssh_pty_open")
        .await?;
    let fs = state.get_fs(&session_id).await?;
    let mut handle = fs.open_pty(cols, rows).await?;

    let owner_window = window.label().to_string();
    let terminal_id = Uuid::new_v4().to_string();
    state.ptys.write().await.insert(
        terminal_id.clone(),
        ActivePty {
            session_id,
            owner_window: owner_window.clone(),
            cmd_tx: handle.cmd_tx,
        },
    );

    let ptys_ref = state.ptys.clone();
    let tid = terminal_id.clone();
    tokio::spawn(async move {
        let mut window_start = std::time::Instant::now();
        let mut bytes_in_window: usize = 0;

        while let Some(chunk) = handle.output_rx.recv().await {
            // 按 16 KB 切片发送，并对每秒超 1 MB 的远端异常刷屏输出进行轻量限流，防止前端内存耗尽
            for slice in chunk.chunks(PTY_MAX_CHUNK_BYTES) {
                if window_start.elapsed().as_secs() >= 1 {
                    window_start = std::time::Instant::now();
                    bytes_in_window = 0;
                }
                bytes_in_window = bytes_in_window.saturating_add(slice.len());
                if bytes_in_window > PTY_MAX_BYTES_PER_SEC {
                    tokio::time::sleep(std::time::Duration::from_millis(20)).await;
                }
                let data_b64 = base64::engine::general_purpose::STANDARD.encode(slice);
                let _ = app.emit_to(
                    &owner_window,
                    PTY_DATA_EVENT,
                    PtyDataEvent {
                        terminal_id: tid.clone(),
                        data_b64,
                    },
                );
            }
        }
        ptys_ref.write().await.remove(&tid);
        let _ = app.emit_to(
            &owner_window,
            PTY_EXIT_EVENT,
            PtyExitEvent { terminal_id: tid },
        );
    });

    Ok(terminal_id)
}

/// 向活跃的交互式 SSH PTY 写入用户输入数据（严格校验窗口归属，防范跨窗口劫持输入）
#[tauri::command]
pub async fn ssh_pty_write(
    window: WebviewWindow,
    state: State<'_, AppState>,
    terminal_id: String,
    data: String,
) -> AppResult<()> {
    let tx = {
        let guard = state.ptys.read().await;
        let active = guard
            .get(&terminal_id)
            .ok_or_else(|| AppError::SessionNotFound("终端会话已关闭".into()))?;
        if active.owner_window != window.label() {
            return Err(AppError::PermissionDenied(
                "安全策略拒绝：禁止跨窗口向他人 PTY 终端写入数据".into(),
            ));
        }
        active.cmd_tx.clone()
    };
    tx.send(PtyCommand::Input(data.into_bytes()))
        .await
        .map_err(|_| AppError::Protocol("终端输入通道已断开".into()))
}

/// 同步调整交互式 SSH PTY 窗口行列尺寸（严格校验窗口归属）
#[tauri::command]
pub async fn ssh_pty_resize(
    window: WebviewWindow,
    state: State<'_, AppState>,
    terminal_id: String,
    cols: u32,
    rows: u32,
) -> AppResult<()> {
    let tx = {
        let guard = state.ptys.read().await;
        match guard.get(&terminal_id) {
            Some(active) if active.owner_window == window.label() => Some(active.cmd_tx.clone()),
            Some(_) => {
                return Err(AppError::PermissionDenied(
                    "安全策略拒绝：禁止跨窗口修改他人 PTY 终端尺寸".into(),
                ));
            }
            None => None,
        }
    };
    if let Some(tx) = tx {
        let _ = tx.send(PtyCommand::Resize { cols, rows }).await;
    }
    Ok(())
}

/// 主动关闭指定交互式 SSH PTY 伪终端（仅所属窗口或主窗口可关闭）
#[tauri::command]
pub async fn ssh_pty_close(
    window: WebviewWindow,
    state: State<'_, AppState>,
    terminal_id: String,
) -> AppResult<()> {
    let active = {
        let mut guard = state.ptys.write().await;
        match guard.get(&terminal_id) {
            Some(p) if p.owner_window == window.label() || window.label() == "main" => {
                guard.remove(&terminal_id)
            }
            Some(_) => {
                return Err(AppError::PermissionDenied(
                    "安全策略拒绝：无权关闭其他窗口的 PTY 终端".into(),
                ));
            }
            None => None,
        }
    };
    if let Some(active) = active {
        let _ = active.cmd_tx.send(PtyCommand::Close).await;
    }
    Ok(())
}

/// 为当前 SSH 会话打开独立的原生操作系统终端窗口（仅主窗口可创建，并严格绑定会话归属）
#[tauri::command]
pub async fn open_terminal_window(
    app: AppHandle,
    window: WebviewWindow,
    state: State<'_, AppState>,
    session_id: String,
    cwd: Option<String>,
) -> AppResult<String> {
    state.ensure_main_window(&window, "open_terminal_window")?;
    let info = {
        let guard = state.sessions.read().await;
        guard
            .get(&session_id)
            .map(|s| s.info.clone())
            .ok_or_else(|| AppError::SessionNotFound(session_id.clone()))?
    };

    if !info.supports_shell {
        return Err(AppError::Unsupported(
            "仅 SFTP/SSH 会话支持打开独立终端窗口".into(),
        ));
    }

    let label = format!("term-{}", &Uuid::new_v4().simple().to_string()[..10]);
    let enc_sid = utf8_percent_encode(&session_id, NON_ALPHANUMERIC).to_string();
    let enc_name = utf8_percent_encode(&info.name, NON_ALPHANUMERIC).to_string();
    let enc_host = utf8_percent_encode(&info.host, NON_ALPHANUMERIC).to_string();
    let cur_cwd = cwd.unwrap_or_else(|| info.current_path.clone());
    let enc_cwd = utf8_percent_encode(&cur_cwd, NON_ALPHANUMERIC).to_string();

    let url = format!(
        "index.html?mode=terminal&sessionId={}&name={}&host={}&cwd={}",
        enc_sid, enc_name, enc_host, enc_cwd
    );

    // 先登记窗口与会话绑定关系，避免新窗口启动瞬间调用 ssh_pty_open 时发生竞态
    state
        .terminal_windows
        .write()
        .await
        .insert(label.clone(), session_id.clone());

    let title = format!("{} ({}) — xiaorui_FTP 终端", info.name, info.host);
    if let Err(e) = WebviewWindowBuilder::new(&app, &label, WebviewUrl::App(url.into()))
        .title(&title)
        .inner_size(980.0, 620.0)
        .min_inner_size(560.0, 340.0)
        .center()
        .resizable(true)
        .build()
    {
        state.terminal_windows.write().await.remove(&label);
        return Err(AppError::Protocol(format!("创建独立终端窗口失败: {}", e)));
    }

    Ok(label)
}

/// 打开独立的原生操作系统本地终端窗口（无需连接远程会话，自动应用 Windows 默认终端配置）
#[tauri::command]
pub async fn open_local_terminal_window(
    app: AppHandle,
    window: WebviewWindow,
    state: State<'_, AppState>,
    cwd: Option<String>,
) -> AppResult<String> {
    state.ensure_main_window(&window, "open_local_terminal_window")?;
    let label = format!("term-local-{}", &Uuid::new_v4().simple().to_string()[..8]);
    let cur_cwd = cwd
        .filter(|s| !s.trim().is_empty())
        .unwrap_or_else(local_fs::home_dir);
    let enc_cwd = utf8_percent_encode(&cur_cwd, NON_ALPHANUMERIC).to_string();
    let url = format!("index.html?mode=terminal&kind=local&cwd={}", enc_cwd);

    state
        .terminal_windows
        .write()
        .await
        .insert(label.clone(), "__local__".to_string());

    let title = "本地终端 — xiaorui_FTP";
    if let Err(e) = WebviewWindowBuilder::new(&app, &label, WebviewUrl::App(url.into()))
        .title(title)
        .inner_size(980.0, 620.0)
        .min_inner_size(560.0, 340.0)
        .center()
        .resizable(true)
        .build()
    {
        state.terminal_windows.write().await.remove(&label);
        return Err(AppError::Protocol(format!(
            "创建独立本地终端窗口失败: {}",
            e
        )));
    }

    Ok(label)
}

/// 呼出并聚焦主窗口（供独立终端窗口一键切回文件双栏视图）
#[tauri::command]
pub fn focus_main_window(app: AppHandle) -> AppResult<()> {
    if let Some(win) = app.get_webview_window("main") {
        let _ = win.unminimize();
        let _ = win.show();
        let _ = win.set_focus();
    }
    Ok(())
}

/// 读取操作系统原生剪贴板纯文本内容（解决 Windows WebView2 权限拦截，并内置 4 次退避重试防剪贴板锁争抢）
///
/// 若当前剪贴板为空或存放的是图片/文件等非文本内容，返回空字符串而非报错。
#[tauri::command]
pub async fn clipboard_read_text() -> AppResult<String> {
    tokio::task::spawn_blocking(|| -> AppResult<String> {
        let mut last_err = String::new();
        for attempt in 0..4 {
            match arboard::Clipboard::new() {
                Ok(mut cb) => match cb.get_text() {
                    Ok(text) => return Ok(text),
                    Err(arboard::Error::ContentNotAvailable) => return Ok(String::new()),
                    Err(e) => last_err = e.to_string(),
                },
                Err(e) => last_err = e.to_string(),
            }
            if attempt < 3 {
                std::thread::sleep(std::time::Duration::from_millis(25));
            }
        }
        Err(AppError::Protocol(format!(
            "读取系统剪贴板失败: {}",
            last_err
        )))
    })
    .await
    .map_err(|e| AppError::Protocol(format!("剪贴板读取任务异常: {}", e)))?
}

/// 将纯文本写入操作系统原生剪贴板（绕过 WebView2 限制，内置 4 次退避重试消除 Windows 剪贴板锁冲突）
#[tauri::command]
pub async fn clipboard_write_text(text: String) -> AppResult<()> {
    // 限制单次剪贴板写入体积上限（8 MB），防止异常大文本导致内存峰值
    if text.len() > 8 * 1024 * 1024 {
        return Err(AppError::SecurityPolicy(
            "复制内容超过 8 MB 上限，已被安全策略拦截".into(),
        ));
    }
    tokio::task::spawn_blocking(move || -> AppResult<()> {
        let mut last_err = String::new();
        for attempt in 0..4 {
            match arboard::Clipboard::new() {
                Ok(mut cb) => match cb.set_text(text.clone()) {
                    Ok(()) => return Ok(()),
                    Err(e) => last_err = e.to_string(),
                },
                Err(e) => last_err = e.to_string(),
            }
            if attempt < 3 {
                std::thread::sleep(std::time::Duration::from_millis(25));
            }
        }
        Err(AppError::Protocol(format!(
            "写入系统剪贴板失败: {}",
            last_err
        )))
    })
    .await
    .map_err(|e| AppError::Protocol(format!("剪贴板写入任务异常: {}", e)))?
}

/// 将终端缓冲区日志或 asciinema 录制内容导出保存到本地（严格限制 `.log`/`.txt`/`.cast` 扩展名、非符号链接与 10 MB 上限）
#[tauri::command]
pub async fn save_text_file(file_path: String, content: String) -> AppResult<()> {
    if content.len() > MAX_TEXT_EXPORT_BYTES {
        return Err(AppError::SecurityPolicy(
            "导出日志超过 10 MB 大小上限，已被安全策略拦截".into(),
        ));
    }
    validate_scoped_file_path(Path::new(&file_path), &["log", "txt", "cast"])?;
    tokio::fs::write(&file_path, content).await?;
    Ok(())
}

// ============================================================================
// 应用偏好设置（最小化到系统托盘等）
// ============================================================================

/// 获取当前应用偏好设置
#[tauri::command]
pub fn get_app_settings(state: State<'_, AppState>) -> AppSettings {
    state.current_settings()
}

/// 更新「关闭时最小化到系统托盘」设置并持久化（仅主窗口可调用）
#[tauri::command]
pub fn set_minimize_to_tray(
    window: WebviewWindow,
    state: State<'_, AppState>,
    enabled: bool,
) -> AppResult<AppSettings> {
    state.ensure_main_window(&window, "set_minimize_to_tray")?;
    state.set_minimize_to_tray(enabled)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_collect_local_upload_plan_recursive() {
        let tmp = std::env::temp_dir().join(format!("xiaorui_ftp_test_{}", Uuid::new_v4()));
        let sub = tmp.join("nested");
        std::fs::create_dir_all(&sub).unwrap();
        std::fs::write(tmp.join("a.txt"), b"hello").unwrap();
        std::fs::write(sub.join("b.txt"), b"world!").unwrap();

        let plan =
            collect_local_upload_plan(&[tmp.to_string_lossy().to_string()], "/deploy").unwrap();
        let root_name = tmp.file_name().unwrap().to_string_lossy().to_string();

        assert!(plan.remote_dirs.contains(&format!("/deploy/{}", root_name)));
        assert!(plan
            .remote_dirs
            .contains(&format!("/deploy/{}/nested", root_name)));
        assert_eq!(plan.files.len(), 2);

        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[tokio::test]
    async fn test_unregister_terminal_window_closes_only_owned_ptys() {
        let tmp = std::env::temp_dir().join(format!("xiaorui_ftp_term_win_{}", Uuid::new_v4()));
        let state = AppState::new(tmp.clone()).unwrap();

        let (tx_term, mut rx_term) = tokio::sync::mpsc::channel::<PtyCommand>(4);
        let (tx_main, mut rx_main) = tokio::sync::mpsc::channel::<PtyCommand>(4);

        state
            .terminal_windows
            .write()
            .await
            .insert("term-abc".into(), "sess-1".into());
        {
            let mut ptys = state.ptys.write().await;
            ptys.insert(
                "pty-term".into(),
                ActivePty {
                    session_id: "sess-1".into(),
                    owner_window: "term-abc".into(),
                    cmd_tx: tx_term,
                },
            );
            ptys.insert(
                "pty-main".into(),
                ActivePty {
                    session_id: "sess-1".into(),
                    owner_window: "main".into(),
                    cmd_tx: tx_main,
                },
            );
        }

        state.unregister_terminal_window("term-abc").await;

        assert!(!state.terminal_windows.read().await.contains_key("term-abc"));
        assert!(!state.ptys.read().await.contains_key("pty-term"));
        assert!(state.ptys.read().await.contains_key("pty-main"));
        assert!(matches!(rx_term.try_recv(), Ok(PtyCommand::Close)));
        assert!(rx_main.try_recv().is_err());

        let _ = std::fs::remove_dir_all(&tmp);
    }
}
