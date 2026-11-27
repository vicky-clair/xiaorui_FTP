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
use serde::Deserialize;
use tauri::{AppHandle, Emitter, State};
use tokio::sync::RwLock;
use uuid::Uuid;

use crate::error::{AppError, AppResult};
use crate::known_hosts::{KnownHostEntry, KnownHostsStore};
use crate::local_fs;
use crate::models::parse_quick_connect_uri;
use crate::models::{
    join_remote_path, normalize_remote_path, AgentStatus, AppSettings, FileEntry, LocalRoot,
    PtyDataEvent, PtyExitEvent, ServerMetrics, SessionInfo, ShellOutput, SiteConfig, SiteInput,
    SiteSecrets, TransferDirection, TransferTask,
};
use crate::protocols::{connect_protocol, sftp, PtyCommand, RemoteFs};
use crate::storage::{load_secrets, SiteStore};
use crate::transfer::{TransferManager, TRANSFER_EVENT};

/// 向前端推送交互式 SSH PTY 原始输出与退出状态的事件名
pub const PTY_DATA_EVENT: &str = "pty://data";
pub const PTY_EXIT_EVENT: &str = "pty://exit";

/// 活跃交互式 SSH PTY 实例
pub struct ActivePty {
    pub session_id: String,
    pub cmd_tx: tokio::sync::mpsc::Sender<PtyCommand>,
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
    /// 轻量偏好配置文件路径（`settings.json`）
    settings_file: PathBuf,
    /// 关闭主窗口时是否最小化到系统托盘（原子无锁读取，供窗口事件钩子安全使用）
    minimize_to_tray: AtomicBool,
    /// 当前所有活跃的远程会话映射表（session_id -> ActiveSession）
    pub sessions: RwLock<HashMap<String, ActiveSession>>,
    /// 当前所有活跃的交互式 SSH PTY 伪终端映射表（terminal_id -> ActivePty）
    pub ptys: Arc<RwLock<HashMap<String, ActivePty>>>,
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
            known_hosts: Arc::new(KnownHostsStore::new(app_data_dir)?),
            settings_file,
            minimize_to_tray: AtomicBool::new(loaded_settings.minimize_to_tray),
            sessions: RwLock::new(HashMap::new()),
            ptys: Arc::new(RwLock::new(HashMap::new())),
            transfers: TransferManager::new(3),
        })
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

    /// 持久化偏好设置至独立文件 `settings.json`
    pub fn set_minimize_to_tray(&self, enabled: bool) -> AppResult<AppSettings> {
        self.minimize_to_tray.store(enabled, Ordering::Relaxed);
        let settings = self.current_settings();
        let json = serde_json::to_string_pretty(&settings)
            .map_err(|e| AppError::Storage(format!("序列化偏好设置失败: {}", e)))?;
        let tmp = self.settings_file.with_extension("json.tmp");
        std::fs::write(&tmp, &json)?;
        if std::fs::rename(&tmp, &self.settings_file).is_err() {
            std::fs::write(&self.settings_file, &json)?;
            let _ = std::fs::remove_file(&tmp);
        }
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

/// 新增或更新站点配置（密钥自动托管至操作系统凭据库）
#[tauri::command]
pub async fn save_site(state: State<'_, AppState>, input: SiteInput) -> AppResult<SiteConfig> {
    state.store.save(input)
}

/// 删除站点配置并清理对应的系统凭据
#[tauri::command]
pub async fn delete_site(state: State<'_, AppState>, site_id: String) -> AppResult<()> {
    state.store.delete(&site_id)
}

/// 将站点列表导出为本地 JSON 备份文件（不包含敏感密钥）
#[tauri::command]
pub async fn export_sites_to_file(state: State<'_, AppState>, file_path: String) -> AppResult<()> {
    let json = state.store.export_json()?;
    tokio::fs::write(&file_path, json).await?;
    Ok(())
}

/// 从本地 JSON 备份文件导入站点配置，返回成功导入的站点数量
#[tauri::command]
pub async fn import_sites_from_file(
    state: State<'_, AppState>,
    file_path: String,
) -> AppResult<usize> {
    let json = tokio::fs::read_to_string(&file_path).await?;
    state.store.import_json(&json)
}

/// 检测本地 SSH Agent（Windows OpenSSH Agent / Pageant / Unix ssh-agent）运行状态与密钥数量
#[tauri::command]
pub async fn check_ssh_agent_status() -> AgentStatus {
    sftp::check_ssh_agent_status().await
}

/// 列出所有已信任的 SSH 主机公钥指纹记录（TOFU）
#[tauri::command]
pub async fn list_known_hosts(state: State<'_, AppState>) -> AppResult<Vec<KnownHostEntry>> {
    state.known_hosts.list()
}

/// 移除指定 `host:port` 的已知 SSH 主机公钥指纹（用于服务器重装后重新信任）
#[tauri::command]
pub async fn remove_known_host(state: State<'_, AppState>, host_port: String) -> AppResult<bool> {
    state.known_hosts.remove(&host_port)
}

// ============================================================================
// 会话连接与生命周期管理
// ============================================================================

/// 连接已保存站点（优先读取系统凭据库中的密钥，支持本次临时覆盖）
#[tauri::command]
pub async fn connect_saved_site(
    state: State<'_, AppState>,
    site_id: String,
    override_secret: Option<String>,
    override_passphrase: Option<String>,
    override_jump_secret: Option<String>,
    override_jump_passphrase: Option<String>,
) -> AppResult<SessionInfo> {
    let site = state.store.get(&site_id)?;
    let mut secrets = load_secrets(&site_id);
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

/// 使用临时表单参数建立一次性连接（不落盘保存）
#[tauri::command]
pub async fn connect_adhoc(state: State<'_, AppState>, input: SiteInput) -> AppResult<SessionInfo> {
    let secrets = SiteSecrets {
        secret: input.secret.clone(),
        passphrase: input.passphrase.clone(),
        session_token: input.session_token.clone(),
        jump_secret: input.jump_secret.clone(),
        jump_passphrase: input.jump_passphrase.clone(),
    };
    let jump_host = input.jump_host.filter(|s| !s.trim().is_empty());
    let site = SiteConfig {
        id: input.id.unwrap_or_default(),
        name: if input.name.trim().is_empty() {
            format!("{}@{}", input.username, input.host)
        } else {
            input.name.trim().to_string()
        },
        group: input.group,
        protocol: input.protocol,
        host: input.host.trim().to_string(),
        port: input.port.unwrap_or_else(|| input.protocol.default_port()),
        username: input.username.trim().to_string(),
        auth_type: input.auth_type,
        private_key_path: input.private_key_path.filter(|s| !s.trim().is_empty()),
        remote_path: normalize_remote_path(&input.remote_path),
        endpoint: input.endpoint.filter(|s| !s.trim().is_empty()),
        bucket: input.bucket.filter(|s| !s.trim().is_empty()),
        region: input.region.filter(|s| !s.trim().is_empty()),
        path_style: input.path_style,
        tls_skip_verify: input.tls_skip_verify,
        jump_port: jump_host.as_ref().map(|_| input.jump_port.unwrap_or(22)),
        jump_username: jump_host
            .as_ref()
            .and_then(|_| input.jump_username.filter(|s| !s.trim().is_empty())),
        jump_auth_type: jump_host.as_ref().and(input.jump_auth_type),
        jump_private_key_path: jump_host
            .as_ref()
            .and_then(|_| input.jump_private_key_path.filter(|s| !s.trim().is_empty())),
        jump_host,
        notes: input.notes,
        has_saved_secret: false,
        updated_at: Utc::now().timestamp(),
    };
    create_session(&state, site, secrets).await
}

/// 解析地址栏快速直连 URI 并立即建立远程会话
#[tauri::command]
pub async fn quick_connect_uri(state: State<'_, AppState>, uri: String) -> AppResult<SessionInfo> {
    let parsed = parse_quick_connect_uri(&uri)?;
    connect_adhoc(state, parsed).await
}

/// 建立远程连接并注册到全局活跃会话池
async fn create_session(
    state: &State<'_, AppState>,
    site: SiteConfig,
    secrets: SiteSecrets,
) -> AppResult<SessionInfo> {
    let site_for_connect = site.clone();
    let secrets_for_connect = secrets.clone();
    let known_hosts = state.known_hosts.clone();
    // 在独立异步任务中完成网络握手，避免连接过程阻塞状态锁
    let fs = tokio::spawn(async move {
        connect_protocol(site_for_connect, secrets_for_connect, known_hosts).await
    })
    .await
    .map_err(|e| AppError::Connection(format!("连接任务异常: {}", e)))??;

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
    };

    state.sessions.write().await.insert(
        session_id,
        ActiveSession {
            info: info.clone(),
            fs,
        },
    );
    Ok(info)
}

/// 主动断开指定远程会话并释放底层连接及关联的 PTY 终端资源
#[tauri::command]
pub async fn disconnect_session(state: State<'_, AppState>, session_id: String) -> AppResult<()> {
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
    state.sessions.write().await.remove(&session_id);
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

/// 删除远程文件或目录
#[tauri::command]
pub async fn remote_delete(
    state: State<'_, AppState>,
    session_id: String,
    path: String,
    is_dir: bool,
) -> AppResult<()> {
    let fs = state.get_fs(&session_id).await?;
    fs.remove(&path, is_dir).await
}

/// 重命名或移动远程文件/目录
#[tauri::command]
pub async fn remote_rename(
    state: State<'_, AppState>,
    session_id: String,
    from: String,
    to: String,
) -> AppResult<()> {
    let fs = state.get_fs(&session_id).await?;
    fs.rename(&from, &to).await
}

/// 修改远程文件/目录 Unix 权限（CHMOD）
#[tauri::command]
pub async fn remote_chmod(
    state: State<'_, AppState>,
    session_id: String,
    path: String,
    mode: u32,
) -> AppResult<()> {
    let fs = state.get_fs(&session_id).await?;
    fs.chmod(&path, mode).await
}

/// 读取远程文本文件内容用于在线快速预览与编辑（限制 2 MB 以防内存溢出）
#[tauri::command]
pub async fn remote_read_text(
    state: State<'_, AppState>,
    session_id: String,
    path: String,
) -> AppResult<String> {
    let fs = state.get_fs(&session_id).await?;
    let bytes = fs.read_bytes(&path, 2 * 1024 * 1024).await?;
    Ok(String::from_utf8_lossy(&bytes).to_string())
}

/// 将在线编辑后的文本内容直接回传保存至远程文件
#[tauri::command]
pub async fn remote_write_text(
    state: State<'_, AppState>,
    session_id: String,
    path: String,
    content: String,
) -> AppResult<()> {
    let fs = state.get_fs(&session_id).await?;
    fs.write_bytes(&path, content.as_bytes()).await
}

// ============================================================================
// 本地文件系统操作
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

/// 列举本地目录内容
#[tauri::command]
pub async fn local_list_dir(path: String) -> AppResult<Vec<FileEntry>> {
    local_fs::list_dir(&path).await
}

/// 在本地创建文件夹
#[tauri::command]
pub async fn local_mkdir(path: String) -> AppResult<()> {
    local_fs::mkdir(&path).await
}

/// 删除本地文件或文件夹
#[tauri::command]
pub async fn local_delete(path: String, is_dir: bool) -> AppResult<()> {
    local_fs::delete(&path, is_dir).await
}

/// 重命名本地文件或文件夹
#[tauri::command]
pub async fn local_rename(from: String, to: String) -> AppResult<()> {
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

/// 批量上传本地文件或文件夹（支持拖拽与多选，自动递归建目录并入队）
#[tauri::command]
pub async fn enqueue_upload_paths(
    app: AppHandle,
    state: State<'_, AppState>,
    session_id: String,
    local_paths: Vec<String>,
    remote_dir: String,
) -> AppResult<Vec<TransferTask>> {
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
    state: State<'_, AppState>,
    session_id: String,
    local_path: String,
    remote_dir: String,
) -> AppResult<Vec<TransferTask>> {
    enqueue_upload_paths(app, state, session_id, vec![local_path], remote_dir).await
}

/// 下载远程单个文件（支持「另存为」精确路径）或递归下载整个远程文件夹
#[tauri::command]
pub async fn enqueue_download(
    app: AppHandle,
    state: State<'_, AppState>,
    request: DownloadRequest,
) -> AppResult<Vec<TransferTask>> {
    let fs = state.get_fs(&request.session_id).await?;
    let norm = normalize_remote_path(&request.remote_path);
    let file_name = norm
        .rsplit('/')
        .next()
        .filter(|s| !s.is_empty())
        .ok_or_else(|| AppError::InvalidInput("无法解析远程名称".into()))?;

    // 单文件下载模式
    if !request.is_dir {
        let local_path =
            if let Some(exact) = request.local_file_path.filter(|s| !s.trim().is_empty()) {
                exact
            } else {
                let dir = request
                    .local_dir
                    .filter(|s| !s.trim().is_empty())
                    .ok_or_else(|| AppError::InvalidInput("请指定本地下载保存路径".into()))?;
                PathBuf::from(&dir)
                    .join(file_name)
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

    // 远程文件夹递归下载模式
    let target_dir = request
        .local_dir
        .filter(|s| !s.trim().is_empty())
        .ok_or_else(|| AppError::InvalidInput("请指定本地下载保存目录".into()))?;
    let root_local = PathBuf::from(&target_dir).join(file_name);
    tokio::fs::create_dir_all(&root_local).await?;

    let mut stack = vec![(norm, root_local)];
    let mut queued = Vec::new();

    while let Some((cur_remote, cur_local)) = stack.pop() {
        let entries = fs.list_dir(&cur_remote).await?;
        for entry in entries {
            // 防御异常服务端返回 `..` 或带路径分隔符的文件名造成目录穿越
            if entry.name.is_empty()
                || entry.name == "."
                || entry.name == ".."
                || entry.name.contains('/')
                || entry.name.contains('\\')
            {
                continue;
            }
            let next_local = cur_local.join(&entry.name);
            if entry.is_dir && !entry.is_symlink {
                tokio::fs::create_dir_all(&next_local).await?;
                stack.push((entry.path, next_local));
            } else if !entry.is_dir {
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
// 内置 SSH 远程命令终端
// ============================================================================

/// 在当前 SFTP/SSH 会话的指定远程目录下执行 Shell 命令
#[tauri::command]
pub async fn ssh_exec_command(
    state: State<'_, AppState>,
    session_id: String,
    command: String,
    cwd: Option<String>,
) -> AppResult<ShellOutput> {
    let fs = state.get_fs(&session_id).await?;
    fs.exec_shell(&command, cwd.as_deref()).await
}

/// 采集当前 SFTP/SSH 会话远端服务器的硬件规格、资源占用与多磁盘挂载点
#[tauri::command]
pub async fn remote_server_metrics(
    state: State<'_, AppState>,
    session_id: String,
) -> AppResult<ServerMetrics> {
    let fs = state.get_fs(&session_id).await?;
    fs.server_metrics().await
}

/// 打开交互式 SSH PTY 伪终端（直接启动远端服务器默认登录 Shell，如 zsh/bash/fish/PowerShell）
#[tauri::command]
pub async fn ssh_pty_open(
    app: AppHandle,
    state: State<'_, AppState>,
    session_id: String,
    cols: u32,
    rows: u32,
) -> AppResult<String> {
    let fs = state.get_fs(&session_id).await?;
    let mut handle = fs.open_pty(cols, rows).await?;

    let terminal_id = Uuid::new_v4().to_string();
    state.ptys.write().await.insert(
        terminal_id.clone(),
        ActivePty {
            session_id,
            cmd_tx: handle.cmd_tx,
        },
    );

    let ptys_ref = state.ptys.clone();
    let tid = terminal_id.clone();
    tokio::spawn(async move {
        while let Some(chunk) = handle.output_rx.recv().await {
            let data_b64 = base64::engine::general_purpose::STANDARD.encode(&chunk);
            let _ = app.emit(
                PTY_DATA_EVENT,
                PtyDataEvent {
                    terminal_id: tid.clone(),
                    data_b64,
                },
            );
        }
        ptys_ref.write().await.remove(&tid);
        let _ = app.emit(PTY_EXIT_EVENT, PtyExitEvent { terminal_id: tid });
    });

    Ok(terminal_id)
}

/// 向活跃的交互式 SSH PTY 写入用户输入数据（支持按键、Tab、Ctrl+C、粘贴与中文）
#[tauri::command]
pub async fn ssh_pty_write(
    state: State<'_, AppState>,
    terminal_id: String,
    data: String,
) -> AppResult<()> {
    let tx = state
        .ptys
        .read()
        .await
        .get(&terminal_id)
        .map(|p| p.cmd_tx.clone())
        .ok_or_else(|| AppError::SessionNotFound("终端会话已关闭".into()))?;
    tx.send(PtyCommand::Input(data.into_bytes()))
        .await
        .map_err(|_| AppError::Protocol("终端输入通道已断开".into()))
}

/// 同步调整交互式 SSH PTY 窗口行列尺寸
#[tauri::command]
pub async fn ssh_pty_resize(
    state: State<'_, AppState>,
    terminal_id: String,
    cols: u32,
    rows: u32,
) -> AppResult<()> {
    if let Some(tx) = state
        .ptys
        .read()
        .await
        .get(&terminal_id)
        .map(|p| p.cmd_tx.clone())
    {
        let _ = tx.send(PtyCommand::Resize { cols, rows }).await;
    }
    Ok(())
}

/// 主动关闭指定交互式 SSH PTY 伪终端
#[tauri::command]
pub async fn ssh_pty_close(state: State<'_, AppState>, terminal_id: String) -> AppResult<()> {
    if let Some(active) = state.ptys.write().await.remove(&terminal_id) {
        let _ = active.cmd_tx.send(PtyCommand::Close).await;
    }
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

/// 更新「关闭时最小化到系统托盘」设置并持久化
#[tauri::command]
pub fn set_minimize_to_tray(state: State<'_, AppState>, enabled: bool) -> AppResult<AppSettings> {
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
}
