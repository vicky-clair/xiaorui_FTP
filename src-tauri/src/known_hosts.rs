//! SSH 主机公钥指纹仓库（显式首次确认 + 变更严格阻断 + Fail Secure）
//!
//! 独立持久化于应用数据目录下的 `known_hosts.json`，与用户站点配置 `sites.json`
//! 及系统凭据库完全隔离，避免影响既有用户数据。
//!
//! 安全工作机制（严格遵循零信任与 Fail Secure 原则）：
//! 1. 首次连接未知 `host:port` 时，绝不自动信任，而是返回 `HostKeyCheckError::Unknown`
//!    由前端弹窗展示公钥算法与 SHA-256 指纹，经用户明确确认后才调用 `trust_host` 落盘；
//! 2. 后续连接严格比对已持久化指纹；
//! 3. 若服务端公钥发生变化（可能存在中间人攻击或服务器重装），立即返回 `Mismatch` 阻断连接；
//! 4. 若 `known_hosts.json` 损坏或无法解析，采用 Fail Secure 策略直接拒绝连接，绝不静默清空重试；
//! 5. `trust_host` 禁止静默覆盖已存在的不同指纹，用户核实服务器重装后必须显式移除旧指纹。

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Mutex;

use chrono::Utc;
use serde::{Deserialize, Serialize};

use crate::error::{AppError, AppResult};
use crate::security::write_private_file;

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

/// 首次连接未知主机时的待确认指纹信息（供前端安全确认弹窗展示）
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct HostKeyUnknown {
    pub host_port: String,
    pub algorithm: String,
    pub fingerprint: String,
    pub is_jump_host: bool,
}

/// 主机密钥不匹配详情（供前端展示阻断性中间人攻击告警）
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct HostKeyMismatch {
    pub host_port: String,
    pub expected_algorithm: String,
    pub expected_fingerprint: String,
    pub actual_algorithm: String,
    pub actual_fingerprint: String,
    pub is_jump_host: bool,
}

/// 严格主机密钥校验失败原因
#[derive(Debug, Clone)]
pub enum HostKeyCheckError {
    /// 未知主机：必须由用户显式核对算法与 SHA-256 指纹后确认信任
    Unknown(HostKeyUnknown),
    /// 指纹发生变更：疑似中间人攻击或服务器重装，立即阻断
    Mismatch(Box<HostKeyMismatch>),
    /// 本地已知主机仓库损坏或锁异常（Fail Secure：拒绝连接）
    StorageCorrupted(String),
}

impl std::fmt::Display for HostKeyMismatch {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "SSH 主机公钥指纹发生变化（{}），已阻断连接以防范中间人攻击！\n已记录指纹: {} ({})\n当前指纹:   {} ({})\n若服务器确实重装或更换了密钥，请在左侧「主机」中核实并移除旧指纹后重试。",
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

    /// 原子持久化已知主机映射（Unix 平台自动收紧权限至 `0o600`）
    fn save_map_unlocked(&self, map: &HashMap<String, KnownHostEntry>) -> AppResult<()> {
        let mut list: Vec<&KnownHostEntry> = map.values().collect();
        list.sort_by(|a, b| a.host_port.cmp(&b.host_port));
        let json = serde_json::to_string_pretty(&list)
            .map_err(|e| AppError::Storage(format!("序列化 known_hosts.json 失败: {}", e)))?;
        write_private_file(&self.file_path, &json)
    }

    /// 按最近校验时间降序列出所有已信任主机记录
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

    /// 移除指定 `host:port` 的指纹记录（用于服务器合法重装后重新核对信任）
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

    /// 经用户明确确认后，信任并持久化指定 SSH 主机的公钥指纹。
    /// 安全约束：禁止静默覆盖已记录的不同指纹，若需更换必须先显式移除旧记录。
    pub fn trust_host(
        &self,
        host_port: &str,
        algorithm: &str,
        fingerprint: &str,
    ) -> AppResult<KnownHostEntry> {
        let hp = host_port.trim();
        let alg = algorithm.trim();
        let fp = fingerprint.trim();
        if hp.is_empty() || alg.is_empty() || !fp.starts_with("SHA256:") {
            return Err(AppError::InvalidInput(
                "无效的 SSH 主机公钥指纹参数（指纹必须为 SHA256:... 格式）".into(),
            ));
        }

        let _guard = self
            .lock
            .lock()
            .map_err(|_| AppError::Storage("已知主机仓库锁损坏".into()))?;
        let mut map = self.load_map_unlocked()?;
        let now = Utc::now().timestamp();

        if let Some(existing) = map.get_mut(hp) {
            if existing.fingerprint != fp {
                return Err(AppError::SecurityPolicy(format!(
                    "主机 {} 已存在不同的受信任指纹，禁止静默覆盖；请先在「主机」管理中核实并移除旧指纹",
                    hp
                )));
            }
            existing.last_seen_at = now;
            let cloned = existing.clone();
            self.save_map_unlocked(&map)?;
            return Ok(cloned);
        }

        let entry = KnownHostEntry {
            host_port: hp.to_string(),
            algorithm: alg.to_string(),
            fingerprint: fp.to_string(),
            first_seen_at: now,
            last_seen_at: now,
        };
        map.insert(hp.to_string(), entry.clone());
        self.save_map_unlocked(&map)?;
        Ok(entry)
    }

    /// 执行严格主机公钥校验（绝不自动信任未知主机，仓库损坏时 Fail Secure 拒绝连接）：
    /// - 仓库解析/加锁异常 -> `Err(StorageCorrupted)`；
    /// - 未知主机 -> `Err(Unknown)`，等待用户明确确认；
    /// - 已有记录且指纹一致 -> 刷新 `last_seen_at` 并返回 `Ok(())`；
    /// - 已有记录但指纹不同 -> `Err(Mismatch)` 阻断连接。
    pub fn verify_strict(
        &self,
        host_port: &str,
        algorithm: &str,
        fingerprint: &str,
        is_jump_host: bool,
    ) -> Result<(), HostKeyCheckError> {
        let Ok(_guard) = self.lock.lock() else {
            return Err(HostKeyCheckError::StorageCorrupted(
                "已知主机仓库内部锁异常，已按 Fail Secure 策略阻断连接".into(),
            ));
        };

        let mut map = self.load_map_unlocked().map_err(|e| {
            HostKeyCheckError::StorageCorrupted(format!(
                "无法读取 known_hosts.json，已拒绝连接以防指纹校验绕过: {}",
                e
            ))
        })?;
        let now = Utc::now().timestamp();

        if let Some(existing) = map.get_mut(host_port) {
            if existing.fingerprint == fingerprint {
                existing.last_seen_at = now;
                // 刷新最近校验时间失败不影响本次已通过的比对，但仍尽量落盘
                let _ = self.save_map_unlocked(&map);
                return Ok(());
            }
            return Err(HostKeyCheckError::Mismatch(Box::new(HostKeyMismatch {
                host_port: host_port.to_string(),
                expected_algorithm: existing.algorithm.clone(),
                expected_fingerprint: existing.fingerprint.clone(),
                actual_algorithm: algorithm.to_string(),
                actual_fingerprint: fingerprint.to_string(),
                is_jump_host,
            })));
        }

        Err(HostKeyCheckError::Unknown(HostKeyUnknown {
            host_port: host_port.to_string(),
            algorithm: algorithm.to_string(),
            fingerprint: fingerprint.to_string(),
            is_jump_host,
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use uuid::Uuid;

    #[test]
    fn test_strict_host_key_confirm_match_mismatch_and_fail_secure() {
        let dir = std::env::temp_dir().join(format!("xiaorui_kh_test_{}", Uuid::new_v4()));
        let store = KnownHostsStore::new(dir.clone()).unwrap();

        // 1. 首次未知主机必须拒绝并要求显式确认（绝不静默自动信任）
        let err_unknown = store
            .verify_strict("10.0.0.1:22", "ssh-ed25519", "SHA256:aaa111", false)
            .unwrap_err();
        assert!(matches!(err_unknown, HostKeyCheckError::Unknown(_)));

        // 2. 用户明确确认后写入信任
        let trusted = store
            .trust_host("10.0.0.1:22", "ssh-ed25519", "SHA256:aaa111")
            .unwrap();
        assert_eq!(trusted.fingerprint, "SHA256:aaa111");

        // 3. 再次连接且指纹一致 -> 放行
        store
            .verify_strict("10.0.0.1:22", "ssh-ed25519", "SHA256:aaa111", false)
            .unwrap();

        // 4. 禁止通过 trust_host 静默覆盖已有不同指纹
        assert!(store
            .trust_host("10.0.0.1:22", "ssh-ed25519", "SHA256:evil999")
            .is_err());

        // 5. 重新实例化模拟重启，指纹被篡改 -> 必须阻断
        let reloaded = KnownHostsStore::new(dir.clone()).unwrap();
        let err_mismatch = reloaded
            .verify_strict("10.0.0.1:22", "ssh-ed25519", "SHA256:evil999", false)
            .unwrap_err();
        match err_mismatch {
            HostKeyCheckError::Mismatch(m) => {
                assert_eq!(m.expected_fingerprint, "SHA256:aaa111");
                assert_eq!(m.actual_fingerprint, "SHA256:evil999");
            }
            other => panic!("expected mismatch, got {:?}", other),
        }

        // 6. 用户主动移除旧指纹后可重新确认信任新指纹
        assert!(reloaded.remove("10.0.0.1:22").unwrap());
        reloaded
            .trust_host("10.0.0.1:22", "ssh-ed25519", "SHA256:new222")
            .unwrap();
        reloaded
            .verify_strict("10.0.0.1:22", "ssh-ed25519", "SHA256:new222", false)
            .unwrap();

        // 7. 若 known_hosts.json 文件损坏，必须按 Fail Secure 拒绝连接而非清空信任表
        std::fs::write(dir.join("known_hosts.json"), "{corrupted-json").unwrap();
        let err_corrupt = reloaded
            .verify_strict("10.0.0.1:22", "ssh-ed25519", "SHA256:new222", false)
            .unwrap_err();
        assert!(matches!(
            err_corrupt,
            HostKeyCheckError::StorageCorrupted(_)
        ));

        let _ = std::fs::remove_dir_all(&dir);
    }
}
