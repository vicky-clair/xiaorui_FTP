//! 站点持久化与操作系统凭据库安全托管模块
//!
//! - 非敏感站点元数据保存至应用数据目录下的 `sites.json`；
//! - 密码、私钥口令、S3 Secret Key、STS 令牌与跳板机密钥通过 `keyring`
//!   安全写入操作系统原生凭据库（Windows 凭据管理器 / macOS Keychain / Linux Secret Service）。

use std::path::PathBuf;

use chrono::Utc;
use keyring::Entry;
use uuid::Uuid;

use crate::error::{AppError, AppResult};
use crate::models::{normalize_remote_path, AuthType, SiteConfig, SiteInput, SiteSecrets};

/// 操作系统凭据库中的服务命名空间
const KEYRING_SERVICE: &str = "com.xiaorui.ftp";

/// 本地站点配置仓库
pub struct SiteStore {
    sites_file: PathBuf,
}

impl SiteStore {
    /// 初始化站点配置仓库并确保目录存在
    pub fn new(app_data_dir: PathBuf) -> AppResult<Self> {
        std::fs::create_dir_all(&app_data_dir)?;
        Ok(Self {
            sites_file: app_data_dir.join("sites.json"),
        })
    }

    /// 读取全部已保存站点（按最近更新时间降序排列）
    pub fn list(&self) -> AppResult<Vec<SiteConfig>> {
        if !self.sites_file.exists() {
            return Ok(Vec::new());
        }
        let raw = std::fs::read_to_string(&self.sites_file)?;
        let mut sites: Vec<SiteConfig> = serde_json::from_str(&raw)
            .map_err(|e| AppError::Storage(format!("解析站点配置失败: {}", e)))?;
        sites.sort_by(|a, b| b.updated_at.cmp(&a.updated_at));
        Ok(sites)
    }

    /// 原子方式将站点列表写回磁盘（先写临时文件再替换，避免异常中断损坏 sites.json）
    fn write_all(&self, sites: &[SiteConfig]) -> AppResult<()> {
        let json = serde_json::to_string_pretty(sites)
            .map_err(|e| AppError::Storage(format!("序列化站点配置失败: {}", e)))?;
        let tmp = self.sites_file.with_extension("json.tmp");
        std::fs::write(&tmp, &json)?;
        if std::fs::rename(&tmp, &self.sites_file).is_err() {
            // Windows 目标文件存在时回退覆盖写入并清理临时文件
            std::fs::write(&self.sites_file, &json)?;
            let _ = std::fs::remove_file(&tmp);
        }
        Ok(())
    }

    /// 按站点 ID 查询单个站点配置
    pub fn get(&self, id: &str) -> AppResult<SiteConfig> {
        self.list()?
            .into_iter()
            .find(|s| s.id == id)
            .ok_or_else(|| AppError::NotFound(format!("未找到站点 {}", id)))
    }

    /// 新增或更新站点配置，并在启用保存密钥时将敏感凭据增量合并写入系统凭据库
    pub fn save(&self, input: SiteInput) -> AppResult<SiteConfig> {
        let mut sites = self.list()?;
        let id = input
            .id
            .filter(|s| !s.trim().is_empty())
            .unwrap_or_else(|| Uuid::new_v4().to_string());

        let existing_has_secret = sites
            .iter()
            .find(|s| s.id == id)
            .map(|s| s.has_saved_secret)
            .unwrap_or(false);

        let mut has_saved_secret = existing_has_secret;
        if input.save_secret {
            let has_new_secret = [
                &input.secret,
                &input.passphrase,
                &input.session_token,
                &input.jump_secret,
                &input.jump_passphrase,
            ]
            .iter()
            .any(|v| v.as_deref().is_some_and(|s| !s.is_empty()));

            if has_new_secret {
                // 增量合并密钥，避免编辑站点时留空字段覆盖已存凭据
                let mut merged = if existing_has_secret {
                    load_secrets(&id)
                } else {
                    SiteSecrets::default()
                };
                if input.secret.as_deref().is_some_and(|s| !s.is_empty()) {
                    merged.secret = input.secret.clone();
                }
                if input.passphrase.as_deref().is_some_and(|s| !s.is_empty()) {
                    merged.passphrase = input.passphrase.clone();
                }
                if input
                    .session_token
                    .as_deref()
                    .is_some_and(|s| !s.is_empty())
                {
                    merged.session_token = input.session_token.clone();
                }
                if input.jump_secret.as_deref().is_some_and(|s| !s.is_empty()) {
                    merged.jump_secret = input.jump_secret.clone();
                }
                if input
                    .jump_passphrase
                    .as_deref()
                    .is_some_and(|s| !s.is_empty())
                {
                    merged.jump_passphrase = input.jump_passphrase.clone();
                }
                save_secrets(&id, &merged)?;
                has_saved_secret = true;
            }
        }

        let jump_host = input.jump_host.filter(|s| !s.trim().is_empty());
        let cfg = SiteConfig {
            id: id.clone(),
            name: if input.name.trim().is_empty() {
                if input.username.trim().is_empty() {
                    input.host.trim().to_string()
                } else {
                    format!("{}@{}", input.username.trim(), input.host.trim())
                }
            } else {
                input.name.trim().to_string()
            },
            group: if input.group.trim().is_empty() {
                "默认分组".to_string()
            } else {
                input.group.trim().to_string()
            },
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
            jump_auth_type: jump_host
                .as_ref()
                .map(|_| input.jump_auth_type.unwrap_or(AuthType::Agent)),
            jump_private_key_path: jump_host
                .as_ref()
                .and_then(|_| input.jump_private_key_path.filter(|s| !s.trim().is_empty())),
            jump_host,
            notes: input.notes,
            has_saved_secret,
            updated_at: Utc::now().timestamp(),
        };

        if let Some(pos) = sites.iter().position(|s| s.id == id) {
            sites[pos] = cfg.clone();
        } else {
            sites.push(cfg.clone());
        }

        self.write_all(&sites)?;
        Ok(cfg)
    }

    /// 删除指定站点并同步清理系统凭据库中对应的密钥
    pub fn delete(&self, id: &str) -> AppResult<()> {
        let mut sites = self.list()?;
        sites.retain(|s| s.id != id);
        self.write_all(&sites)?;
        let _ = delete_secrets(id);
        Ok(())
    }

    /// 导出全部站点元数据为格式化 JSON 字符串（不包含敏感密钥）
    pub fn export_json(&self) -> AppResult<String> {
        let sites = self.list()?;
        serde_json::to_string_pretty(&sites)
            .map_err(|e| AppError::Storage(format!("导出站点失败: {}", e)))
    }

    /// 从 JSON 字符串批量导入站点配置（导入后需用户按需重新填写凭据）
    pub fn import_json(&self, json: &str) -> AppResult<usize> {
        let imported: Vec<SiteConfig> = serde_json::from_str(json)
            .map_err(|e| AppError::InvalidInput(format!("导入 JSON 格式错误: {}", e)))?;
        let mut existing = self.list()?;
        let mut count = 0;
        for mut site in imported {
            site.has_saved_secret = false;
            site.updated_at = Utc::now().timestamp();
            if let Some(pos) = existing.iter().position(|s| s.id == site.id) {
                existing[pos] = site;
            } else {
                if site.id.is_empty() {
                    site.id = Uuid::new_v4().to_string();
                }
                existing.push(site);
            }
            count += 1;
        }
        self.write_all(&existing)?;
        Ok(count)
    }
}

/// 将站点密钥安全写入操作系统凭据库
pub fn save_secrets(site_id: &str, secrets: &SiteSecrets) -> AppResult<()> {
    let entry = Entry::new(KEYRING_SERVICE, site_id)
        .map_err(|e| AppError::Storage(format!("初始化系统凭据库失败: {}", e)))?;
    let payload = serde_json::to_string(secrets)
        .map_err(|e| AppError::Storage(format!("序列化凭据失败: {}", e)))?;
    entry
        .set_password(&payload)
        .map_err(|e| AppError::Storage(format!("写入系统凭据库失败: {}", e)))?;
    Ok(())
}

/// 从操作系统凭据库读取站点密钥（读取不到时返回空默认值）
pub fn load_secrets(site_id: &str) -> SiteSecrets {
    let Ok(entry) = Entry::new(KEYRING_SERVICE, site_id) else {
        return SiteSecrets::default();
    };
    match entry.get_password() {
        Ok(raw) => serde_json::from_str(&raw).unwrap_or_default(),
        Err(_) => SiteSecrets::default(),
    }
}

/// 从操作系统凭据库删除指定站点的密钥
pub fn delete_secrets(site_id: &str) -> AppResult<()> {
    if let Ok(entry) = Entry::new(KEYRING_SERVICE, site_id) {
        let _ = entry.delete_credential();
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::Protocol;

    #[test]
    fn test_site_store_legacy_compatibility_and_backup_roundtrip() {
        let dir = std::env::temp_dir().join(format!("xiaorui_store_test_{}", Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();

        // 模拟旧版 sites.json（缺少跳板机 jump_* 字段），确保升级后读取绝不丢失既有站点数据
        let legacy_json = r#"[
          {
            "id": "legacy-site-1",
            "name": "生产机",
            "group": "核心",
            "protocol": "sftp",
            "host": "10.0.0.10",
            "port": 22,
            "username": "root",
            "authType": "password",
            "remotePath": "/var/www",
            "pathStyle": false,
            "tlsSkipVerify": false,
            "notes": "legacy",
            "hasSavedSecret": true,
            "updatedAt": 1700000000
          }
        ]"#;
        std::fs::write(dir.join("sites.json"), legacy_json).unwrap();

        let store = SiteStore::new(dir.clone()).unwrap();
        let list = store.list().unwrap();
        assert_eq!(list.len(), 1);
        assert_eq!(list[0].id, "legacy-site-1");
        assert!(list[0].has_saved_secret);
        assert!(list[0].jump_host.is_none());

        // 新增站点（不触碰系统 keyring：save_secret = false）并验证导出/导入逻辑
        let created = store
            .save(SiteInput {
                id: None,
                name: "测试 MinIO".into(),
                group: "云存储".into(),
                protocol: Protocol::S3,
                host: "127.0.0.1".into(),
                port: Some(9000),
                username: "minioadmin".into(),
                auth_type: AuthType::AccessKey,
                secret: Some("super-secret".into()),
                passphrase: None,
                session_token: None,
                private_key_path: None,
                remote_path: "/".into(),
                endpoint: Some("http://127.0.0.1:9000".into()),
                bucket: Some("assets".into()),
                region: Some("us-east-1".into()),
                path_style: true,
                tls_skip_verify: false,
                jump_host: None,
                jump_port: None,
                jump_username: None,
                jump_auth_type: None,
                jump_private_key_path: None,
                jump_secret: None,
                jump_passphrase: None,
                notes: String::new(),
                save_secret: false,
            })
            .unwrap();
        assert_eq!(store.list().unwrap().len(), 2);

        // 导出的备份 JSON 绝不包含明文密钥
        let exported = store.export_json().unwrap();
        assert!(!exported.contains("super-secret"));

        // 导入至全新目录时 has_saved_secret 重置为 false 且站点完整保留
        let dir2 = std::env::temp_dir().join(format!("xiaorui_store_import_{}", Uuid::new_v4()));
        let store2 = SiteStore::new(dir2.clone()).unwrap();
        let imported_count = store2.import_json(&exported).unwrap();
        assert_eq!(imported_count, 2);
        assert!(store2.list().unwrap().iter().all(|s| !s.has_saved_secret));

        store.delete(&created.id).unwrap();
        assert_eq!(store.list().unwrap().len(), 1);

        let _ = std::fs::remove_dir_all(&dir);
        let _ = std::fs::remove_dir_all(&dir2);
    }
}
