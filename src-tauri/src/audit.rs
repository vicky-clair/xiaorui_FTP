//! 本地安全审计与连接事件日志模块
//!
//! 职责与安全边界（遵循第 10 章《日志与安全审计》规范）：
//! - 以结构化 JSONL 持久化至应用数据目录 `audit.log.jsonl`（Unix 平台 `0o600` 权限）；
//! - 严格脱敏与禁止项：绝不记录密码、私钥内容、Passphrase、Token、文件内容或终端按键流；
//! - 自动按体积轮转（单文件超过 2 MB 自动归档至 `audit.log.1.jsonl`，防止磁盘耗尽）；
//! - 明确标注为本地客户端运行审计日志，不伪装成不可篡改的远程审计系统。

use std::fs::OpenOptions;
use std::io::Write;
use std::path::PathBuf;
use std::sync::Mutex;

use chrono::Utc;
use serde::{Deserialize, Serialize};

use crate::error::{AppError, AppResult};

/// 单日志文件的最大字节阈值（2 MB），超出后自动轮转保留最近一份历史归档
const MAX_AUDIT_FILE_BYTES: u64 = 2 * 1024 * 1024;

/// 审计事件严重级别
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AuditSeverity {
    Info,
    Warn,
    High,
    Critical,
}

/// 审计事件分类
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AuditCategory {
    HostKey,
    Session,
    Credential,
    FileOp,
    SecurityPolicy,
}

/// 单条脱敏结构化安全审计记录
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AuditEntry {
    /// 事件时间戳（Unix 秒）
    pub timestamp: i64,
    /// 严重级别
    pub severity: AuditSeverity,
    /// 事件类别
    pub category: AuditCategory,
    /// 事件动作代码（如 `host_key_mismatch`、`remote_delete`）
    pub event: String,
    /// 关联目标（如 `192.168.1.10:22`、`/var/www/app.conf`，不含任何密钥）
    pub target: String,
    /// 脱敏后的中文可读摘要
    pub detail: String,
}

/// 对日志字段进行控制字符清理与长度截断，防止日志伪造（Log Injection）与超长撑爆
fn sanitize_log_text(input: &str, max_chars: usize) -> String {
    let cleaned: String = input
        .chars()
        .map(|c| {
            if c == '\r' || c == '\n' || c == '\t' {
                ' '
            } else if (c as u32) < 0x20 || c == '\u{7f}' {
                '?'
            } else {
                c
            }
        })
        .collect();
    if cleaned.chars().count() > max_chars {
        let mut s: String = cleaned.chars().take(max_chars).collect();
        s.push_str("...");
        s
    } else {
        cleaned
    }
}

/// 线程安全的本地安全审计日志仓库
pub struct AuditLogger {
    file_path: PathBuf,
    rotated_path: PathBuf,
    lock: Mutex<()>,
}

impl AuditLogger {
    /// 初始化审计日志仓库并确保父目录存在
    pub fn new(app_data_dir: PathBuf) -> AppResult<Self> {
        std::fs::create_dir_all(&app_data_dir)?;
        Ok(Self {
            file_path: app_data_dir.join("audit.log.jsonl"),
            rotated_path: app_data_dir.join("audit.log.1.jsonl"),
            lock: Mutex::new(()),
        })
    }

    /// 写入一条脱敏安全审计记录（写入失败不中断主业务流程，但内部保证串行与轮转）
    pub fn record(
        &self,
        severity: AuditSeverity,
        category: AuditCategory,
        event: &str,
        target: &str,
        detail: &str,
    ) {
        let Ok(_guard) = self.lock.lock() else {
            return;
        };

        // 达到体积上限时自动轮转
        if let Ok(meta) = std::fs::metadata(&self.file_path) {
            if meta.len() >= MAX_AUDIT_FILE_BYTES {
                let _ = std::fs::remove_file(&self.rotated_path);
                let _ = std::fs::rename(&self.file_path, &self.rotated_path);
            }
        }

        let entry = AuditEntry {
            timestamp: Utc::now().timestamp(),
            severity,
            category,
            event: sanitize_log_text(event, 64),
            target: sanitize_log_text(target, 200),
            detail: sanitize_log_text(detail, 400),
        };

        let Ok(line) = serde_json::to_string(&entry) else {
            return;
        };

        if let Ok(mut file) = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&self.file_path)
        {
            let _ = writeln!(file, "{}", line);
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                let _ = std::fs::set_permissions(
                    &self.file_path,
                    std::fs::Permissions::from_mode(0o600),
                );
            }
        }
    }

    /// 读取最近 `limit` 条安全审计记录（按时间倒序排列）
    pub fn recent(&self, limit: usize) -> AppResult<Vec<AuditEntry>> {
        let _guard = self
            .lock
            .lock()
            .map_err(|_| AppError::Storage("审计日志锁损坏".into()))?;
        let cap = limit.clamp(1, 1000);
        if !self.file_path.exists() {
            return Ok(Vec::new());
        }
        let raw = std::fs::read_to_string(&self.file_path)?;
        let mut entries: Vec<AuditEntry> = raw
            .lines()
            .filter_map(|line| serde_json::from_str::<AuditEntry>(line).ok())
            .collect();
        entries.reverse();
        entries.truncate(cap);
        Ok(entries)
    }

    /// 导出最近审计日志为脱敏格式化 JSON（供排查问题或归档）
    pub fn export_json(&self, limit: usize) -> AppResult<String> {
        let items = self.recent(limit)?;
        serde_json::to_string_pretty(&items)
            .map_err(|e| AppError::Storage(format!("序列化审计日志失败: {}", e)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use uuid::Uuid;

    #[test]
    fn test_audit_logger_record_sanitization_and_recent_order() {
        let dir = std::env::temp_dir().join(format!("xiaorui_audit_test_{}", Uuid::new_v4()));
        let logger = AuditLogger::new(dir.clone()).unwrap();

        logger.record(
            AuditSeverity::Info,
            AuditCategory::Session,
            "session_connected",
            "10.0.0.1:22",
            "通过 SFTP 建立连接",
        );
        // 含换行符的恶意注入输入应被清理为单行空格，防止伪造 JSONL 记录
        logger.record(
            AuditSeverity::Critical,
            AuditCategory::HostKey,
            "host_key_mismatch",
            "10.0.0.1:22\n{\"fake\":true}",
            "检测到主机指纹变化\r\n已阻断连接",
        );

        let recent = logger.recent(10).unwrap();
        assert_eq!(recent.len(), 2);
        assert_eq!(recent[0].event, "host_key_mismatch");
        assert!(!recent[0].target.contains('\n'));
        assert!(!recent[0].detail.contains('\r'));
        assert_eq!(recent[1].event, "session_connected");

        let exported = logger.export_json(10).unwrap();
        assert!(exported.contains("host_key_mismatch"));

        let _ = std::fs::remove_dir_all(&dir);
    }
}
