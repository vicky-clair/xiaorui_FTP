//! SSH 主机公钥指纹仓库（TOFU：首次信任后固定）
//!
//! 独立持久化于应用数据目录下的 `known_hosts.json`，与用户站点配置 `sites.json`
//! 及系统凭据库完全隔离，避免影响既有用户数据。
//!
//! 工作机制：
//! 1. 首次连接某个 `host:port` 时，记录其公钥算法与 SHA-256 指纹；
//! 2. 后续连接严格比对已持久化指纹；
//! 3. 若服务端公钥发生变化（可能存在中间人攻击或服务器重装），立刻阻断连接并给出清晰告警；
//! 4. 用户确认服务器确系合法重装后，可在「已知主机管理」中移除旧指纹后重新连接。

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Mutex;

use chrono::Utc;
use serde::{Deserialize, Serialize};

use crate::error::{AppError, AppResult};

/// 单条已知 SSH 主机公钥指纹记录
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct KnownHostEntry {
    /// 主机与端口标识，例如 `192.168.1.10:22`
    pub host_port: String,
    /// 公钥算法名称，例如 `ssh-ed25519`、`ssh-rsa`
    pub algorithm: String,
    /// OpenSSH 标准 SHA-256 指纹，例如 `SHA256:...`
    pub fingerprint: String,
    /// 首次信任时间戳（Unix 秒）
    pub first_seen_at: i64,
    /// 最近一次验证通过时间戳（Unix 秒）
    pub last_seen_at: i64,
}

/// 主机密钥校验通过类型
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HostKeyOutcome {
    /// 首次连接，已执行 TOFU 持久化
    FirstTrust,
    /// 与既有固定指纹完全匹配
    Matched,
}

/// 主机密钥不匹配详情（用于向前端展示可读的中间人告警）
#[derive(Debug, Clone)]
pub struct HostKeyMismatch {
    pub host_port: String,
    pub expected_algorithm: String,
    pub expected_fingerprint: String,
    pub actual_algorithm: String,
    pub actual_fingerprint: String,
}

impl std::fmt::Display for HostKeyMismatch {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "SSH 主机公钥指纹发生变化（{}），已阻断连接以防范中间人攻击！\n已记录指纹: {} ({})\n当前指纹:   {} ({})\n若服务器确实重装或更换了密钥，请在左侧「已知主机」中移除旧指纹后重试。",
            self.host_port,
            self.expected_fingerprint,
            self.expected_algorithm,
            self.actual_fingerprint,
            self.actual_algorithm
        )
    }
}

/// 线程安全的本地 `known_hosts.json` 存储管理器
pub struct KnownHostsStore {
    file_path: PathBuf,
    lock: Mutex<()>,
}

impl KnownHostsStore {
    /// 初始化已知主机仓库并确保父目录存在
    pub fn new(app_data_dir: PathBuf) -> AppResult<Self> {
        std::fs::create_dir_all(&app_data_dir)?;
        Ok(Self {
            file_path: app_data_dir.join("known_hosts.json"),
            lock: Mutex::new(()),
        })
    }

    /// 读取所有已知主机映射（调用方需持有 `self.lock`）
    fn load_map_unlocked(&self) -> AppResult<HashMap<String, KnownHostEntry>> {
        if !self.file_path.exists() {
            return Ok(HashMap::new());
        }
        let raw = std::fs::read_to_string(&self.file_path)?;
        if raw.trim().is_empty() {
            return Ok(HashMap::new());
        }
        let entries: Vec<KnownHostEntry> = serde_json::from_str(&raw)
            .map_err(|e| AppError::Storage(format!("解析 known_hosts.json 失败: {}", e)))?;
        Ok(entries
            .into_iter()
            .map(|e| (e.host_port.clone(), e))
            .collect())
    }

    /// 持久化已知主机映射（采用临时文件写入后原子替换，防止异常断电损坏）
    fn save_map_unlocked(&self, map: &HashMap<String, KnownHostEntry>) -> AppResult<()> {
        let mut list: Vec<&KnownHostEntry> = map.values().collect();
        list.sort_by(|a, b| a.host_port.cmp(&b.host_port));
        let json = serde_json::to_string_pretty(&list)
            .map_err(|e| AppError::Storage(format!("序列化 known_hosts.json 失败: {}", e)))?;
        let tmp_path = self.file_path.with_extension("json.tmp");
        std::fs::write(&tmp_path, json)?;
        // Windows 上 rename 覆盖已有文件前先移除旧目标文件以保证兼容性
        if self.file_path.exists() {
            let _ = std::fs::remove_file(&self.file_path);
        }
        std::fs::rename(&tmp_path, &self.file_path)?;
        Ok(())
    }

    /// 按主机名排序列出所有已信任主机记录
    pub fn list(&self) -> AppResult<Vec<KnownHostEntry>> {
        let _guard = self
            .lock
            .lock()
            .map_err(|_| AppError::Storage("已知主机仓库锁损坏".into()))?;
        let map = self.load_map_unlocked()?;
        let mut list: Vec<KnownHostEntry> = map.into_values().collect();
        list.sort_by(|a, b| b.last_seen_at.cmp(&a.last_seen_at));
        Ok(list)
    }

    /// 移除指定 `host:port` 的指纹记录（用于服务器合法重装后重新信任）
    pub fn remove(&self, host_port: &str) -> AppResult<bool> {
        let _guard = self
            .lock
            .lock()
            .map_err(|_| AppError::Storage("已知主机仓库锁损坏".into()))?;
        let mut map = self.load_map_unlocked()?;
        let removed = map.remove(host_port).is_some();
        if removed {
            self.save_map_unlocked(&map)?;
        }
        Ok(removed)
    }

    /// 执行 TOFU 主机公钥校验：
    /// - 首次遇到：写入持久化并返回 `Ok(HostKeyOutcome::FirstTrust)`；
    /// - 已有记录且指纹一致：刷新 `last_seen_at` 并返回 `Ok(HostKeyOutcome::Matched)`；
    /// - 已有记录但指纹变化：返回 `Err(HostKeyMismatch)` 拒绝连接。
    pub fn verify_or_trust(
        &self,
        host_port: &str,
        algorithm: &str,
        fingerprint: &str,
    ) -> Result<HostKeyOutcome, HostKeyMismatch> {
        let Ok(_guard) = self.lock.lock() else {
            return Err(HostKeyMismatch {
                host_port: host_port.to_string(),
                expected_algorithm: "unknown".to_string(),
                expected_fingerprint: "lock-poisoned".to_string(),
                actual_algorithm: algorithm.to_string(),
                actual_fingerprint: fingerprint.to_string(),
            });
        };

        let mut map = self.load_map_unlocked().unwrap_or_default();
        let now = Utc::now().timestamp();

        if let Some(existing) = map.get_mut(host_port) {
            if existing.fingerprint == fingerprint {
                existing.last_seen_at = now;
                let _ = self.save_map_unlocked(&map);
                return Ok(HostKeyOutcome::Matched);
            }
            return Err(HostKeyMismatch {
                host_port: host_port.to_string(),
                expected_algorithm: existing.algorithm.clone(),
                expected_fingerprint: existing.fingerprint.clone(),
                actual_algorithm: algorithm.to_string(),
                actual_fingerprint: fingerprint.to_string(),
            });
        }

        // 首次连接该主机，记录其指纹（TOFU）
        map.insert(
            host_port.to_string(),
            KnownHostEntry {
                host_port: host_port.to_string(),
                algorithm: algorithm.to_string(),
                fingerprint: fingerprint.to_string(),
                first_seen_at: now,
                last_seen_at: now,
            },
        );
        let _ = self.save_map_unlocked(&map);
        Ok(HostKeyOutcome::FirstTrust)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use uuid::Uuid;

    #[test]
    fn test_tofu_trust_match_mismatch_and_remove() {
        let dir = std::env::temp_dir().join(format!("xiaorui_kh_test_{}", Uuid::new_v4()));
        let store = KnownHostsStore::new(dir.clone()).unwrap();

        // 1. 首次信任
        let r1 = store
            .verify_or_trust("10.0.0.1:22", "ssh-ed25519", "SHA256:aaa111")
            .unwrap();
        assert_eq!(r1, HostKeyOutcome::FirstTrust);

        // 2. 再次连接且指纹一致 -> 放行
        let r2 = store
            .verify_or_trust("10.0.0.1:22", "ssh-ed25519", "SHA256:aaa111")
            .unwrap();
        assert_eq!(r2, HostKeyOutcome::Matched);

        // 3. 重新实例化模拟重启，指纹被篡改 -> 必须阻断
        let reloaded = KnownHostsStore::new(dir.clone()).unwrap();
        let err = reloaded
            .verify_or_trust("10.0.0.1:22", "ssh-ed25519", "SHA256:evil999")
            .unwrap_err();
        assert_eq!(err.expected_fingerprint, "SHA256:aaa111");
        assert_eq!(err.actual_fingerprint, "SHA256:evil999");

        // 4. 用户主动移除旧指纹后可重新信任新指纹
        assert!(reloaded.remove("10.0.0.1:22").unwrap());
        let r3 = reloaded
            .verify_or_trust("10.0.0.1:22", "ssh-ed25519", "SHA256:new222")
            .unwrap();
        assert_eq!(r3, HostKeyOutcome::FirstTrust);

        let _ = std::fs::remove_dir_all(&dir);
    }
}
