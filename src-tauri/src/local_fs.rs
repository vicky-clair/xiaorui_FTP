//! 跨平台本地文件系统操作模块
//!
//! 提供本地快捷位置枚举（Windows 盘符与常用目录、macOS/Linux 根目录）、
//! 异步目录浏览、新建文件夹、删除与重命名能力。

use std::path::PathBuf;
use std::time::UNIX_EPOCH;

use crate::error::{AppError, AppResult};
use crate::models::{FileEntry, LocalRoot};

/// 返回当前用户主目录路径
pub fn home_dir() -> String {
    dirs::home_dir()
        .or_else(|| std::env::current_dir().ok())
        .unwrap_or_else(|| PathBuf::from("/"))
        .to_string_lossy()
        .to_string()
}

/// 列出本地常用快捷位置与磁盘根路径
pub fn list_roots() -> Vec<LocalRoot> {
    let mut roots = Vec::new();
    if let Some(home) = dirs::home_dir() {
        roots.push(LocalRoot {
            name: "主目录".to_string(),
            path: home.to_string_lossy().to_string(),
        });
    }
    if let Some(desktop) = dirs::desktop_dir() {
        roots.push(LocalRoot {
            name: "桌面".to_string(),
            path: desktop.to_string_lossy().to_string(),
        });
    }
    if let Some(downloads) = dirs::download_dir() {
        roots.push(LocalRoot {
            name: "下载".to_string(),
            path: downloads.to_string_lossy().to_string(),
        });
    }

    // Windows 平台枚举可用驱动器盘符（A: ~ Z:）
    #[cfg(target_os = "windows")]
    {
        for letter in b'A'..=b'Z' {
            let drive = format!("{}:\\", letter as char);
            if std::path::Path::new(&drive).exists() {
                roots.push(LocalRoot {
                    name: format!("本地磁盘 ({}:)", letter as char),
                    path: drive,
                });
            }
        }
    }

    // macOS / Linux 平台提供系统根目录
    #[cfg(not(target_os = "windows"))]
    {
        roots.push(LocalRoot {
            name: "根目录 (/)".to_string(),
            path: "/".to_string(),
        });
    }

    roots
}

/// 异步读取本地指定目录的文件与子目录列表（按文件夹优先、名称升序排列）
pub async fn list_dir(path: &str) -> AppResult<Vec<FileEntry>> {
    let target = PathBuf::from(path);
    let mut read_dir = tokio::fs::read_dir(&target).await.map_err(|e| {
        AppError::Io(std::io::Error::new(
            e.kind(),
            format!("无法读取本地目录 '{}': {}", path, e),
        ))
    })?;

    let mut entries = Vec::new();
    while let Some(entry) = read_dir.next_entry().await? {
        let name = entry.file_name().to_string_lossy().to_string();
        let full_path = entry.path().to_string_lossy().to_string();
        let meta = match entry.metadata().await {
            Ok(m) => m,
            Err(_) => continue,
        };
        let is_symlink = tokio::fs::symlink_metadata(entry.path())
            .await
            .map(|m| m.file_type().is_symlink())
            .unwrap_or(false);
        let modified = meta
            .modified()
            .ok()
            .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
            .map(|d| d.as_secs() as i64);

        #[cfg(unix)]
        let (permissions, mode) = {
            use std::os::unix::fs::PermissionsExt;
            let m = meta.permissions().mode();
            (Some(crate::models::format_unix_mode(m)), Some(m & 0o777))
        };
        #[cfg(not(unix))]
        let (permissions, mode) = {
            let ro = if meta.permissions().readonly() {
                Some("只读".to_string())
            } else {
                Some("读写".to_string())
            };
            (ro, None)
        };

        entries.push(FileEntry {
            name,
            path: full_path,
            is_dir: meta.is_dir(),
            size: if meta.is_dir() { 0 } else { meta.len() },
            modified,
            permissions,
            mode,
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

/// 递归创建本地目录
pub async fn mkdir(path: &str) -> AppResult<()> {
    tokio::fs::create_dir_all(path).await?;
    Ok(())
}

/// 删除本地文件或递归删除整个目录
pub async fn delete(path: &str, is_dir: bool) -> AppResult<()> {
    if is_dir {
        tokio::fs::remove_dir_all(path).await?;
    } else {
        tokio::fs::remove_file(path).await?;
    }
    Ok(())
}

/// 重命名或移动本地文件/目录
pub async fn rename(from: &str, to: &str) -> AppResult<()> {
    tokio::fs::rename(from, to).await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use uuid::Uuid;

    #[tokio::test]
    async fn test_local_fs_lifecycle_and_sorting() {
        let base = std::env::temp_dir().join(format!("xiaorui_localfs_{}", Uuid::new_v4()));
        let sub = base.join("z_folder");
        mkdir(&sub.to_string_lossy()).await.unwrap();
        tokio::fs::write(base.join("a_file.txt"), b"abc")
            .await
            .unwrap();

        // 验证目录排在文件之前
        let list = list_dir(&base.to_string_lossy()).await.unwrap();
        assert_eq!(list.len(), 2);
        assert!(list[0].is_dir);
        assert_eq!(list[0].name, "z_folder");
        assert!(!list[1].is_dir);
        assert_eq!(list[1].name, "a_file.txt");

        // 重命名并删除
        let renamed = base.join("b_file.txt");
        rename(
            &base.join("a_file.txt").to_string_lossy(),
            &renamed.to_string_lossy(),
        )
        .await
        .unwrap();
        assert!(renamed.exists());

        delete(&renamed.to_string_lossy(), false).await.unwrap();
        assert!(!renamed.exists());

        delete(&base.to_string_lossy(), true).await.unwrap();
        assert!(!base.exists());
    }
}
