//! 本地 SSH 密钥管理器模块
//!
//! 职责与安全边界：
//! - 扫描本地 `~/.ssh` 目录下的公钥与私钥元信息（绝不向前端返回私钥原文内容）；
//! - 使用 `russh::keys` 原生生成 Ed25519 密钥对（支持可选 Passphrase 加密保护），
//!   私钥采用原子写入并在 Unix 平台强制收紧至 `0o600`，序列化后立即 `zeroize` 清零内存；
//! - 校验公钥格式合法性，供纯 SFTP 一键部署到远端 `~/.ssh/authorized_keys` 使用。

use std::path::{Path, PathBuf};

use rand_core::OsRng;
use russh::keys::ssh_key::LineEnding;
use russh::keys::{Algorithm, HashAlg, PrivateKey, PublicKey};
use serde::{Deserialize, Serialize};
use zeroize::Zeroize;

use crate::error::{AppError, AppResult};
use crate::security::{sanitize_local_filename, write_private_file};

/// 本地 SSH 密钥元数据（严禁包含私钥明文）
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SshKeyEntry {
    /// 密钥文件名（不含 `.pub` 后缀，例如 `id_ed25519`）
    pub name: String,
    /// 私钥文件路径（若存在）
    pub private_key_path: Option<String>,
    /// 公钥文件路径（`.pub`）
    pub public_key_path: String,
    /// 公钥算法（如 `ssh-ed25519`、`ssh-rsa`）
    pub algorithm: String,
    /// OpenSSH 标准 SHA-256 指纹
    pub fingerprint: String,
    /// 公钥注释（如 `xiaorui@workstation`）
    pub comment: String,
    /// 完整 OpenSSH 公钥行（用于复制或一键部署到远端 `authorized_keys`）
    pub public_key_line: String,
    /// 同名私钥文件是否存在
    pub has_private_key: bool,
}

/// 获取默认 `~/.ssh` 目录路径
pub fn default_ssh_dir() -> AppResult<PathBuf> {
    let home = dirs::home_dir()
        .ok_or_else(|| AppError::Storage("无法定位当前用户主目录以访问 .ssh 文件夹".into()))?;
    Ok(home.join(".ssh"))
}

/// 校验并规范化单行 OpenSSH 公钥字符串，返回 `(规范化公钥行, 算法, SHA-256 指纹)`
pub fn validate_openssh_public_key(raw: &str) -> AppResult<(String, String, String)> {
    let trimmed = raw.trim();
    if trimmed.is_empty() || trimmed.contains('\r') || trimmed.contains('\n') {
        return Err(AppError::InvalidInput(
            "SSH 公钥必须为单行合法 OpenSSH 格式（禁止换行注入）".into(),
        ));
    }
    let parsed = PublicKey::from_openssh(trimmed)
        .map_err(|e| AppError::InvalidInput(format!("SSH 公钥格式无效: {}", e)))?;
    let canonical = parsed
        .to_openssh()
        .map_err(|e| AppError::InvalidInput(format!("格式化 SSH 公钥失败: {}", e)))?;
    let algorithm = parsed.algorithm().to_string();
    let fingerprint = parsed.fingerprint(HashAlg::Sha256).to_string();
    Ok((canonical, algorithm, fingerprint))
}

/// 扫描指定 `.ssh` 目录下的所有合法 `.pub` 公钥及对应私钥存在状态
pub fn list_ssh_keys_in_dir(ssh_dir: &Path) -> AppResult<Vec<SshKeyEntry>> {
    if !ssh_dir.exists() {
        return Ok(Vec::new());
    }
    let mut keys = Vec::new();
    let entries = std::fs::read_dir(ssh_dir)?;
    for entry in entries.flatten() {
        let path = entry.path();
        if !path.is_file() {
            continue;
        }
        let Some(ext) = path.extension().and_then(|e| e.to_str()) else {
            continue;
        };
        if !ext.eq_ignore_ascii_case("pub") {
            continue;
        }
        let Ok(raw) = std::fs::read_to_string(&path) else {
            continue;
        };
        let first_line = raw.lines().find(|l| !l.trim().is_empty()).unwrap_or("");
        let Ok(parsed) = PublicKey::from_openssh(first_line.trim()) else {
            continue;
        };
        let stem = path
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("key")
            .to_string();
        let priv_path = ssh_dir.join(&stem);
        let has_private_key = priv_path.is_file();

        keys.push(SshKeyEntry {
            name: stem,
            private_key_path: if has_private_key {
                Some(priv_path.to_string_lossy().to_string())
            } else {
                None
            },
            public_key_path: path.to_string_lossy().to_string(),
            algorithm: parsed.algorithm().to_string(),
            fingerprint: parsed.fingerprint(HashAlg::Sha256).to_string(),
            comment: parsed.comment().to_string(),
            public_key_line: first_line.trim().to_string(),
            has_private_key,
        });
    }

    keys.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()));
    Ok(keys)
}

/// 扫描当前用户 `~/.ssh` 目录下的全部 SSH 密钥
pub fn list_local_ssh_keys() -> AppResult<Vec<SshKeyEntry>> {
    let dir = default_ssh_dir()?;
    list_ssh_keys_in_dir(&dir)
}

/// 在指定目录生成现代 Ed25519 SSH 密钥对（支持可选 Passphrase 加密，绝不覆盖已有同名文件）
pub fn generate_ed25519_key_in_dir(
    ssh_dir: &Path,
    name: &str,
    comment: &str,
    passphrase: Option<&str>,
) -> AppResult<SshKeyEntry> {
    let clean_name = sanitize_local_filename(name.trim())?;
    if clean_name.ends_with(".pub") {
        return Err(AppError::InvalidInput(
            "密钥名称无需包含 .pub 后缀（例如填写 id_ed25519_prod）".into(),
        ));
    }

    std::fs::create_dir_all(ssh_dir)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(ssh_dir, std::fs::Permissions::from_mode(0o700));
    }

    let priv_path = ssh_dir.join(clean_name);
    let pub_path = ssh_dir.join(format!("{}.pub", clean_name));
    if priv_path.exists() || pub_path.exists() {
        return Err(AppError::InvalidInput(format!(
            "密钥 '{}' 已存在，为防止覆盖既有密钥已拒绝生成，请更换名称",
            clean_name
        )));
    }

    let mut key = PrivateKey::random(&mut OsRng, Algorithm::Ed25519)
        .map_err(|e| AppError::Protocol(format!("生成 Ed25519 私钥失败: {}", e)))?;
    let clean_comment = comment.trim().replace(['\r', '\n'], " ");
    if !clean_comment.is_empty() {
        key.set_comment(clean_comment.clone());
    }

    let final_key = if let Some(pass) = passphrase.filter(|p| !p.is_empty()) {
        key.encrypt(&mut OsRng, pass)
            .map_err(|e| AppError::Protocol(format!("使用口令加密私钥失败: {}", e)))?
    } else {
        key
    };

    let mut private_pem = final_key
        .to_openssh(LineEnding::LF)
        .map_err(|e| AppError::Protocol(format!("序列化 OpenSSH 私钥失败: {}", e)))?
        .to_string();

    let pub_key = final_key.public_key();
    let public_line = pub_key
        .to_openssh()
        .map_err(|e| AppError::Protocol(format!("序列化 OpenSSH 公钥失败: {}", e)))?;
    let algorithm = pub_key.algorithm().to_string();
    let fingerprint = pub_key.fingerprint(HashAlg::Sha256).to_string();

    let write_res = write_private_file(&priv_path, &private_pem);
    private_pem.zeroize();
    write_res?;

    std::fs::write(&pub_path, format!("{}\n", public_line))?;

    Ok(SshKeyEntry {
        name: clean_name.to_string(),
        private_key_path: Some(priv_path.to_string_lossy().to_string()),
        public_key_path: pub_path.to_string_lossy().to_string(),
        algorithm,
        fingerprint,
        comment: clean_comment,
        public_key_line: public_line,
        has_private_key: true,
    })
}

/// 在用户 `~/.ssh` 目录生成 Ed25519 密钥对
pub fn generate_local_ed25519_key(
    name: &str,
    comment: &str,
    passphrase: Option<&str>,
) -> AppResult<SshKeyEntry> {
    let dir = default_ssh_dir()?;
    generate_ed25519_key_in_dir(&dir, name, comment, passphrase)
}

#[cfg(test)]
mod tests {
    use super::*;
    use uuid::Uuid;

    #[test]
    fn test_generate_ed25519_key_list_and_prevent_overwrite() {
        let dir = std::env::temp_dir().join(format!("xiaorui_sshkey_test_{}", Uuid::new_v4()));

        // 1. 拒绝路径穿越密钥名
        assert!(generate_ed25519_key_in_dir(&dir, "../evil", "test", None).is_err());

        // 2. 生成带 Passphrase 加密的 Ed25519 密钥对
        let created = generate_ed25519_key_in_dir(
            &dir,
            "id_ed25519_demo",
            "dev@xiaorui",
            Some("Str0ngPass!"),
        )
        .unwrap();
        assert_eq!(created.name, "id_ed25519_demo");
        assert_eq!(created.algorithm, "ssh-ed25519");
        assert!(created.fingerprint.starts_with("SHA256:"));
        assert!(created.has_private_key);

        // 3. 验证生成的私钥确实可被正确口令解密、错误口令被拒绝
        let priv_path = created.private_key_path.as_deref().unwrap();
        assert!(russh::keys::load_secret_key(priv_path, Some("Str0ngPass!")).is_ok());
        assert!(russh::keys::load_secret_key(priv_path, Some("wrong")).is_err());

        // 4. 同名再次生成必须被拒绝，防覆盖既有密钥
        assert!(generate_ed25519_key_in_dir(&dir, "id_ed25519_demo", "dev", None).is_err());

        // 5. 列表扫描与公钥合法性校验
        let listed = list_ssh_keys_in_dir(&dir).unwrap();
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].fingerprint, created.fingerprint);

        assert!(validate_openssh_public_key(&created.public_key_line).is_ok());
        assert!(validate_openssh_public_key("ssh-ed25519 AAAA\nrm -rf /").is_err());

        let _ = std::fs::remove_dir_all(&dir);
    }
}
