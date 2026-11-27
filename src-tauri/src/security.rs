//! 安全核心校验工具模块
//!
//! 集中实现与平台无关的防御性校验逻辑，落实纵深防御与 Fail Secure 原则：
//! - 远程路径控制字符拦截（防御 FTP CRLF 命令注入与 Shell 控制字符注入）；
//! - 本地文件名与下载路径穿越/盘符覆盖/保留设备名/符号链接覆盖防御；
//! - 本地下载 `.part` 临时文件原子提交；
//! - 本地危险删除路径拦截（防止误删根目录、主目录或系统目录）；
//! - 敏感配置文件原子写入与 Unix `0o600` 最小权限保护。

use std::path::{Component, Path, PathBuf};

use uuid::Uuid;

use crate::error::{AppError, AppResult};

/// 校验远程路径中不包含控制字符（`\0`、`\r`、`\n` 及 ASCII 控制字符），
/// 防御 FTP 控制通道 CRLF 注入与远程 Shell 控制字符注入。
pub fn validate_remote_path_no_control(path: &str) -> AppResult<()> {
    for ch in path.chars() {
        if ch == '\0' || ch == '\r' || ch == '\n' || (ch as u32) < 0x20 || ch == '\u{7f}' {
            return Err(AppError::SecurityPolicy(
                "远程路径包含非法控制字符或换行符，已被安全策略拦截".into(),
            ));
        }
    }
    Ok(())
}

/// Windows 平台保留设备文件名集合（大小写不敏感，带扩展名如 `CON.txt` 同样非法）
const WINDOWS_RESERVED_NAMES: &[&str] = &[
    "CON", "PRN", "AUX", "NUL", "COM1", "COM2", "COM3", "COM4", "COM5", "COM6", "COM7", "COM8",
    "COM9", "LPT1", "LPT2", "LPT3", "LPT4", "LPT5", "LPT6", "LPT7", "LPT8", "LPT9",
];

/// 严格校验来自不可信远程服务器的单个文件名或目录名，防止本地路径穿越与平台特殊文件攻击
pub fn sanitize_local_filename(name: &str) -> AppResult<&str> {
    if name.is_empty() || name == "." || name == ".." {
        return Err(AppError::SecurityPolicy(format!(
            "远程文件名 '{}' 不合法，已被安全策略拦截",
            name
        )));
    }
    if name.contains('/') || name.contains('\\') || name.contains(':') {
        return Err(AppError::SecurityPolicy(format!(
            "远程文件名 '{}' 包含路径分隔符或盘符冒号，已被安全策略拦截",
            name
        )));
    }
    if name
        .chars()
        .any(|c| c == '\0' || (c as u32) < 0x20 || c == '\u{7f}')
    {
        return Err(AppError::SecurityPolicy(
            "远程文件名包含非法控制字符，已被安全策略拦截".into(),
        ));
    }
    // 防御 Windows 尾随空格或点号导致的路径混淆绕过
    if name.ends_with('.') || name.ends_with(' ') {
        return Err(AppError::SecurityPolicy(format!(
            "远程文件名 '{}' 包含非法尾随空格或点号，已被安全策略拦截",
            name
        )));
    }
    let stem = name.split('.').next().unwrap_or(name).to_ascii_uppercase();
    if WINDOWS_RESERVED_NAMES.contains(&stem.as_str()) {
        return Err(AppError::SecurityPolicy(format!(
            "远程文件名 '{}' 命中操作系统保留设备名，已被安全策略拦截",
            name
        )));
    }
    Ok(name)
}

/// 将不可信的远程文件名安全解析到指定的本地基目录内，确保结果严格位于 `base_dir` 下且不发生路径穿越
pub fn resolve_safe_download_path(base_dir: &Path, entry_name: &str) -> AppResult<PathBuf> {
    let clean = sanitize_local_filename(entry_name)?;
    let candidate = base_dir.join(clean);

    // 二次验证拼接结果仅比 base_dir 多出一个 Normal 组件，杜绝任何前缀覆盖或 `..` 逃逸
    let rel = candidate.strip_prefix(base_dir).map_err(|_| {
        AppError::SecurityPolicy("计算出的本地下载路径逃逸出目标目录，已被拦截".into())
    })?;
    let mut comps = rel.components();
    match (comps.next(), comps.next()) {
        (Some(Component::Normal(_)), None) => {}
        _ => {
            return Err(AppError::SecurityPolicy(
                "本地下载路径结构异常，已被安全策略拦截".into(),
            ));
        }
    }

    ensure_not_symlink_target(&candidate)?;
    Ok(candidate)
}

/// 校验本地目标路径本身不是符号链接（防止下载覆写符号链接指向的系统敏感文件）
pub fn ensure_not_symlink_target(path: &Path) -> AppResult<()> {
    match std::fs::symlink_metadata(path) {
        Ok(meta) => {
            if meta.file_type().is_symlink() {
                return Err(AppError::SecurityPolicy(format!(
                    "目标路径 '{}' 是符号链接，已拒绝直接覆盖以防范符号链接逃逸攻击",
                    path.display()
                )));
            }
            Ok(())
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(AppError::Io(e)),
    }
}

/// 为本地下载目标生成同目录下的临时 `.part` 暂存文件路径，便于下载完成后原子提交
pub fn staging_part_path(target_path: &Path) -> PathBuf {
    let file_name = target_path
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_else(|| "download".to_string());
    let suffix = &Uuid::new_v4().simple().to_string()[..8];
    let part_name = format!(".{}.xiaorui-{}.part", file_name, suffix);
    match target_path.parent() {
        Some(parent) => parent.join(part_name),
        None => PathBuf::from(part_name),
    }
}

/// 将已完整落盘的 `.part` 临时文件原子提交为正式目标文件，提交前再次确认目标非符号链接
pub fn commit_staging_file(part_path: &Path, target_path: &Path) -> AppResult<()> {
    ensure_not_symlink_target(target_path)?;
    if let Err(first_err) = std::fs::rename(part_path, target_path) {
        // Windows 上目标文件已存在时 rename 会失败，再次校验非符号链接后替换
        if target_path.exists() {
            ensure_not_symlink_target(target_path)?;
            std::fs::remove_file(target_path)?;
            if let Err(e2) = std::fs::rename(part_path, target_path) {
                let _ = std::fs::remove_file(part_path);
                return Err(AppError::Io(e2));
            }
            return Ok(());
        }
        let _ = std::fs::remove_file(part_path);
        return Err(AppError::Io(first_err));
    }
    Ok(())
}

/// 拦截针对根目录、用户主目录及核心操作系统目录的危险本地删除操作
pub fn ensure_safe_local_delete_target(path: &Path) -> AppResult<()> {
    let raw = path.to_string_lossy();
    if raw.trim().is_empty() {
        return Err(AppError::InvalidInput("删除路径不能为空".into()));
    }

    // 1. 禁止删除任何文件系统根目录（如 `/`、`C:\`）
    if path.parent().is_none() {
        return Err(AppError::SecurityPolicy(format!(
            "禁止删除文件系统根目录 '{}'",
            path.display()
        )));
    }

    let resolved = std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
    if resolved.parent().is_none() {
        return Err(AppError::SecurityPolicy(format!(
            "禁止删除文件系统根目录 '{}'",
            resolved.display()
        )));
    }

    // 2. 禁止直接删除当前用户主目录
    if let Some(home) = dirs::home_dir() {
        let home_resolved = std::fs::canonicalize(&home).unwrap_or(home);
        if resolved == home_resolved {
            return Err(AppError::SecurityPolicy(
                "禁止直接删除当前用户主目录".into(),
            ));
        }
    }

    // 3. 禁止删除关键系统目录
    let norm_lower = resolved
        .to_string_lossy()
        .replace('\\', "/")
        .trim_end_matches('/')
        .to_ascii_lowercase();
    let stripped = norm_lower.strip_prefix("//?/").unwrap_or(&norm_lower);

    let blocked_unix = [
        "", "/etc", "/bin", "/sbin", "/usr", "/var", "/lib", "/lib64", "/boot", "/dev", "/proc",
        "/sys", "/root", "/home", "/system", "/library", "/users",
    ];
    if blocked_unix.contains(&stripped) {
        return Err(AppError::SecurityPolicy(format!(
            "禁止删除受保护的系统目录 '{}'",
            path.display()
        )));
    }

    // Windows 核心系统目录拦截（如 c:/windows、c:/program files、c:/users）
    if stripped.len() >= 3 && stripped.as_bytes()[1] == b':' {
        let after_drive = &stripped[2..];
        let blocked_win = [
            "",
            "/windows",
            "/windows/system32",
            "/program files",
            "/program files (x86)",
            "/programdata",
            "/users",
        ];
        if blocked_win.contains(&after_drive) {
            return Err(AppError::SecurityPolicy(format!(
                "禁止删除受保护的 Windows 系统目录 '{}'",
                path.display()
            )));
        }
    }

    Ok(())
}

/// 校验前端导出/导入文件路径的扩展名白名单与非符号链接属性，防止覆写可执行文件或敏感系统配置
pub fn validate_scoped_file_path(path: &Path, allowed_exts: &[&str]) -> AppResult<()> {
    let raw = path.to_string_lossy();
    if raw.trim().is_empty() || raw.contains('\0') {
        return Err(AppError::InvalidInput(
            "文件路径不能为空或包含非法字符".into(),
        ));
    }
    ensure_not_symlink_target(path)?;
    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .map(|s| s.to_ascii_lowercase())
        .unwrap_or_default();
    if !allowed_exts.iter().any(|allowed| *allowed == ext) {
        return Err(AppError::SecurityPolicy(format!(
            "文件扩展名 '.{}' 不在允许列表 {:?} 中，已被安全策略拦截",
            ext, allowed_exts
        )));
    }
    Ok(())
}

/// 原子写入应用私有配置文件，并在 Unix 平台上显式收紧文件权限为 `0o600`（仅当前用户可读写）
pub fn write_private_file(path: &Path, content: &str) -> AppResult<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let _ = std::fs::set_permissions(parent, std::fs::Permissions::from_mode(0o700));
        }
    }

    let tmp = path.with_extension(format!("tmp.{}", &Uuid::new_v4().simple().to_string()[..8]));
    std::fs::write(&tmp, content)?;

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&tmp, std::fs::Permissions::from_mode(0o600))?;
    }

    if let Err(err) = std::fs::rename(&tmp, path) {
        // Windows 目标文件存在时先移除旧文件再原子替换
        if path.exists() {
            std::fs::remove_file(path)?;
            if let Err(err2) = std::fs::rename(&tmp, path) {
                let _ = std::fs::remove_file(&tmp);
                return Err(AppError::Io(err2));
            }
        } else {
            let _ = std::fs::remove_file(&tmp);
            return Err(AppError::Io(err));
        }
    }

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600));
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_validate_remote_path_rejects_crlf_and_nul() {
        assert!(validate_remote_path_no_control("/var/www/app.conf").is_ok());
        assert!(validate_remote_path_no_control("/中文目录/测试 文件.txt").is_ok());
        assert!(validate_remote_path_no_control("/tmp/a\r\nDELE /etc/passwd").is_err());
        assert!(validate_remote_path_no_control("/tmp/a\nrm -rf /").is_err());
        assert!(validate_remote_path_no_control("/tmp/a\0b").is_err());
    }

    #[test]
    fn test_sanitize_local_filename_blocks_traversal_and_reserved() {
        assert!(sanitize_local_filename("report_2026.pdf").is_ok());
        assert!(sanitize_local_filename("中文文件.tar.gz").is_ok());

        for bad in [
            "",
            ".",
            "..",
            "../evil.txt",
            "a/b.txt",
            "a\\b.txt",
            "C:evil.exe",
            "notes.txt:ads",
            "CON",
            "nul.txt",
            "COM1",
            "LPT9.log",
            "trailing.",
            "trailing ",
            "bad\0name",
            "bad\nname",
        ] {
            assert!(
                sanitize_local_filename(bad).is_err(),
                "expected '{}' to be rejected",
                bad
            );
        }
    }

    #[test]
    fn test_resolve_safe_download_path_and_atomic_commit() {
        let dir = std::env::temp_dir().join(format!("xiaorui_sec_test_{}", Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();

        let target = resolve_safe_download_path(&dir, "safe.txt").unwrap();
        assert_eq!(target, dir.join("safe.txt"));
        assert!(resolve_safe_download_path(&dir, "..").is_err());
        assert!(resolve_safe_download_path(&dir, "C:hijack.dll").is_err());

        // 测试 .part 临时文件原子提交（包括覆盖已有普通文件）
        let part1 = staging_part_path(&target);
        std::fs::write(&part1, b"v1").unwrap();
        commit_staging_file(&part1, &target).unwrap();
        assert_eq!(std::fs::read(&target).unwrap(), b"v1");
        assert!(!part1.exists());

        let part2 = staging_part_path(&target);
        std::fs::write(&part2, b"v2-updated").unwrap();
        commit_staging_file(&part2, &target).unwrap();
        assert_eq!(std::fs::read(&target).unwrap(), b"v2-updated");
        assert!(!part2.exists());

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_ensure_safe_local_delete_target_blocks_critical_paths() {
        assert!(ensure_safe_local_delete_target(Path::new("/")).is_err());
        #[cfg(windows)]
        {
            assert!(ensure_safe_local_delete_target(Path::new("C:\\")).is_err());
            assert!(ensure_safe_local_delete_target(Path::new("C:\\Windows")).is_err());
        }
        if let Some(home) = dirs::home_dir() {
            assert!(ensure_safe_local_delete_target(&home).is_err());
        }
        let temp_child = std::env::temp_dir().join("xiaorui_safe_delete_child_demo");
        assert!(ensure_safe_local_delete_target(&temp_child).is_ok());
    }
}
