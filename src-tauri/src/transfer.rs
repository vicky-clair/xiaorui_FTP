//! 异步并发传输队列管理器
//!
//! 通过 `tokio::sync::Semaphore` 控制最大并发传输任务数（默认 3），
//! 支持上传/下载实时字节进度节流上报、速率估算、单任务取消与批量清理。

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Instant;

use chrono::Utc;
use tauri::{AppHandle, Emitter};
use tokio::sync::{RwLock, Semaphore};
use uuid::Uuid;

use crate::error::{AppError, AppResult};
use crate::models::{TransferDirection, TransferStatus, TransferTask};
use crate::protocols::RemoteFs;

/// 向前端推送传输任务状态更新的 Tauri 事件名
pub const TRANSFER_EVENT: &str = "transfer://update";

/// 并发传输管理器
pub struct TransferManager {
    /// 任务列表（最新入队的排在最前）
    tasks: Arc<RwLock<Vec<TransferTask>>>,
    /// 各运行/排队任务对应的原子取消标志
    cancels: Arc<RwLock<HashMap<String, Arc<AtomicBool>>>>,
    /// 并发许可信号量
    semaphore: Arc<Semaphore>,
}

impl TransferManager {
    /// 创建并发传输管理器，`max_concurrent` 为同时执行的最大任务数
    pub fn new(max_concurrent: usize) -> Self {
        Self {
            tasks: Arc::new(RwLock::new(Vec::new())),
            cancels: Arc::new(RwLock::new(HashMap::new())),
            semaphore: Arc::new(Semaphore::new(max_concurrent.max(1))),
        }
    }

    /// 获取当前所有传输任务快照
    pub async fn list(&self) -> Vec<TransferTask> {
        self.tasks.read().await.clone()
    }

    /// 取消指定 ID 的排队中或进行中任务，若状态发生变更则返回更新后的任务快照供上层广播
    pub async fn cancel(&self, task_id: &str) -> AppResult<Option<TransferTask>> {
        if let Some(flag) = self.cancels.read().await.get(task_id) {
            flag.store(true, Ordering::Relaxed);
        }
        let mut tasks = self.tasks.write().await;
        if let Some(t) = tasks.iter_mut().find(|t| t.id == task_id) {
            if matches!(t.status, TransferStatus::Queued | TransferStatus::Running) {
                t.status = TransferStatus::Cancelled;
                t.speed_bps = 0;
                return Ok(Some(t.clone()));
            }
        }
        Ok(None)
    }

    /// 取消属于指定远程会话的全部排队中/进行中传输任务（会话断开时调用），返回已取消的任务快照列表
    pub async fn cancel_by_session(&self, session_id: &str) -> Vec<TransferTask> {
        let mut affected_ids = Vec::new();
        let mut updated = Vec::new();
        {
            let mut tasks = self.tasks.write().await;
            for t in tasks.iter_mut() {
                if t.session_id == session_id
                    && matches!(t.status, TransferStatus::Queued | TransferStatus::Running)
                {
                    t.status = TransferStatus::Cancelled;
                    t.speed_bps = 0;
                    affected_ids.push(t.id.clone());
                    updated.push(t.clone());
                }
            }
        }
        let cancels = self.cancels.read().await;
        for id in affected_ids {
            if let Some(flag) = cancels.get(&id) {
                flag.store(true, Ordering::Relaxed);
            }
        }
        updated
    }

    /// 清除所有已结束（已完成、失败或已取消）的历史任务
    pub async fn clear_finished(&self) {
        let mut tasks = self.tasks.write().await;
        tasks.retain(|t| matches!(t.status, TransferStatus::Queued | TransferStatus::Running));
    }

    /// 将单个文件的上传或下载任务加入后台并发队列并立即返回任务描述
    #[allow(clippy::too_many_arguments)]
    pub async fn enqueue(
        &self,
        app: AppHandle,
        fs: Arc<dyn RemoteFs>,
        session_id: String,
        direction: TransferDirection,
        local_path: String,
        remote_path: String,
        total_bytes: u64,
    ) -> AppResult<TransferTask> {
        let file_name = PathBuf::from(&local_path)
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_else(|| remote_path.rsplit('/').next().unwrap_or("file").to_string());

        let task = TransferTask {
            id: Uuid::new_v4().to_string(),
            session_id,
            direction,
            local_path: local_path.clone(),
            remote_path: remote_path.clone(),
            file_name,
            total_bytes,
            transferred_bytes: 0,
            speed_bps: 0,
            status: TransferStatus::Queued,
            error: None,
            created_at: Utc::now().timestamp(),
        };

        let cancel_flag = Arc::new(AtomicBool::new(false));
        {
            self.tasks.write().await.insert(0, task.clone());
            self.cancels
                .write()
                .await
                .insert(task.id.clone(), cancel_flag.clone());
        }
        let _ = app.emit(TRANSFER_EVENT, &task);

        let tasks_ref = self.tasks.clone();
        let cancels_ref = self.cancels.clone();
        let sem = self.semaphore.clone();
        let task_id = task.id.clone();

        // 在后台异步协程中等待并发许可并执行实际文件传输
        tokio::spawn(async move {
            let Ok(_permit) = sem.acquire_owned().await else {
                return;
            };

            if cancel_flag.load(Ordering::Relaxed) {
                Self::update_status(&tasks_ref, &app, &task_id, TransferStatus::Cancelled, None)
                    .await;
                cancels_ref.write().await.remove(&task_id);
                return;
            }

            Self::update_status(&tasks_ref, &app, &task_id, TransferStatus::Running, None).await;

            let started = Instant::now();
            let last_emit = Arc::new(std::sync::Mutex::new(Instant::now()));
            let tasks_cb = tasks_ref.clone();
            let app_cb = app.clone();
            let id_cb = task_id.clone();

            // 节流进度回调：每 120ms 或传输完成时向前端推送一次进度与平均速率
            let progress_cb = Arc::new(move |done: u64, total: u64| {
                let mut should_emit = false;
                if let Ok(mut last) = last_emit.lock() {
                    if last.elapsed().as_millis() >= 120 || (total > 0 && done >= total) {
                        *last = Instant::now();
                        should_emit = true;
                    }
                }
                if !should_emit {
                    return;
                }
                let elapsed = started.elapsed().as_secs_f64().max(0.05);
                let speed = (done as f64 / elapsed) as u64;
                let tasks_inner = tasks_cb.clone();
                let app_inner = app_cb.clone();
                let tid = id_cb.clone();
                tokio::spawn(async move {
                    let mut guard = tasks_inner.write().await;
                    if let Some(t) = guard.iter_mut().find(|x| x.id == tid) {
                        // 仅在仍处于 Running 状态时更新进度，防止延迟回调覆盖已完成或已取消状态
                        if t.status != TransferStatus::Running {
                            return;
                        }
                        t.transferred_bytes = done;
                        t.total_bytes = total.max(t.total_bytes);
                        t.speed_bps = speed;
                        let _ = app_inner.emit(TRANSFER_EVENT, t.clone());
                    }
                });
            });

            let lp = PathBuf::from(&local_path);
            let result = match direction {
                TransferDirection::Download => {
                    fs.download_file(&remote_path, &lp, progress_cb, cancel_flag.clone())
                        .await
                }
                TransferDirection::Upload => {
                    fs.upload_file(&lp, &remote_path, progress_cb, cancel_flag.clone())
                        .await
                }
            };

            match result {
                Ok(bytes) => {
                    let mut guard = tasks_ref.write().await;
                    if let Some(t) = guard.iter_mut().find(|x| x.id == task_id) {
                        t.transferred_bytes = bytes;
                        t.total_bytes = t.total_bytes.max(bytes);
                        t.speed_bps = 0;
                        t.status = TransferStatus::Completed;
                        let _ = app.emit(TRANSFER_EVENT, t.clone());
                    }
                }
                Err(AppError::Cancelled) => {
                    Self::update_status(
                        &tasks_ref,
                        &app,
                        &task_id,
                        TransferStatus::Cancelled,
                        None,
                    )
                    .await;
                }
                Err(e) => {
                    Self::update_status(
                        &tasks_ref,
                        &app,
                        &task_id,
                        TransferStatus::Failed,
                        Some(e.to_string()),
                    )
                    .await;
                }
            }

            cancels_ref.write().await.remove(&task_id);
        });

        Ok(task)
    }

    /// 更新指定任务的状态并向前端广播事件
    async fn update_status(
        tasks_ref: &Arc<RwLock<Vec<TransferTask>>>,
        app: &AppHandle,
        task_id: &str,
        status: TransferStatus,
        error: Option<String>,
    ) {
        let mut guard = tasks_ref.write().await;
        if let Some(t) = guard.iter_mut().find(|x| x.id == task_id) {
            t.status = status;
            t.speed_bps = 0;
            t.error = error;
            let _ = app.emit(TRANSFER_EVENT, t.clone());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_transfer_manager_cancel_and_clear_finished() {
        let mgr = TransferManager::new(2);
        let flag = Arc::new(AtomicBool::new(false));
        {
            let mut tasks = mgr.tasks.write().await;
            tasks.push(TransferTask {
                id: "t-run".into(),
                session_id: "s1".into(),
                direction: TransferDirection::Upload,
                local_path: "/tmp/a".into(),
                remote_path: "/a".into(),
                file_name: "a".into(),
                total_bytes: 100,
                transferred_bytes: 50,
                speed_bps: 10,
                status: TransferStatus::Running,
                error: None,
                created_at: 1,
            });
            tasks.push(TransferTask {
                id: "t-done".into(),
                session_id: "s1".into(),
                direction: TransferDirection::Download,
                local_path: "/tmp/b".into(),
                remote_path: "/b".into(),
                file_name: "b".into(),
                total_bytes: 100,
                transferred_bytes: 100,
                speed_bps: 0,
                status: TransferStatus::Completed,
                error: None,
                created_at: 2,
            });
            mgr.cancels
                .write()
                .await
                .insert("t-run".into(), flag.clone());
        }

        // 清除已完成时，进行中任务必须保留
        mgr.clear_finished().await;
        assert_eq!(mgr.list().await.len(), 1);
        assert_eq!(mgr.list().await[0].id, "t-run");

        // 取消进行中任务应同时置位原子取消标志与状态，并返回更新后的任务快照
        let cancelled = mgr.cancel("t-run").await.unwrap();
        assert!(cancelled.is_some());
        assert!(flag.load(Ordering::Relaxed));
        assert_eq!(mgr.list().await[0].status, TransferStatus::Cancelled);

        // 再次清除已完成时，已取消任务被移除
        mgr.clear_finished().await;
        assert!(mgr.list().await.is_empty());

        // 验证按会话批量取消仅影响目标会话任务，不误伤其他会话
        let flag_s1 = Arc::new(AtomicBool::new(false));
        let flag_s2 = Arc::new(AtomicBool::new(false));
        {
            let mut tasks = mgr.tasks.write().await;
            tasks.push(TransferTask {
                id: "t-s1".into(),
                session_id: "s1".into(),
                direction: TransferDirection::Download,
                local_path: "/tmp/1".into(),
                remote_path: "/1".into(),
                file_name: "1".into(),
                total_bytes: 100,
                transferred_bytes: 0,
                speed_bps: 0,
                status: TransferStatus::Queued,
                error: None,
                created_at: 3,
            });
            tasks.push(TransferTask {
                id: "t-s2".into(),
                session_id: "s2".into(),
                direction: TransferDirection::Download,
                local_path: "/tmp/2".into(),
                remote_path: "/2".into(),
                file_name: "2".into(),
                total_bytes: 100,
                transferred_bytes: 0,
                speed_bps: 0,
                status: TransferStatus::Running,
                error: None,
                created_at: 4,
            });
            let mut cancels = mgr.cancels.write().await;
            cancels.insert("t-s1".into(), flag_s1.clone());
            cancels.insert("t-s2".into(), flag_s2.clone());
        }
        let cancelled_s1 = mgr.cancel_by_session("s1").await;
        assert_eq!(cancelled_s1.len(), 1);
        assert!(flag_s1.load(Ordering::Relaxed));
        assert!(!flag_s2.load(Ordering::Relaxed));
    }
}
