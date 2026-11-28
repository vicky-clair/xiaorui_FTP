//! 跨平台本地原生交互终端（Local PTY）模块
//!
//! 基于 `portable-pty`（Windows ConPTY / macOS & Linux Unix PTY）实现，
//! 安全边界：
//! - 严格限定内置白名单 Shell 预设（禁止前端传入任意可执行程序路径）；
//! - 复用统一 `PtyHandle` / `PtyCommand` 接口，受后端窗口归属隔离与输出限流保护；
//! - 收到 `PtyCommand::Close` 时主动终止子进程，避免残留孤儿 Shell 进程。

use std::io::{Read, Write};
use std::path::Path;

use portable_pty::{native_pty_system, CommandBuilder, PtySize};
use serde::{Deserialize, Serialize};

use crate::error::{AppError, AppResult};
use crate::protocols::{PtyCommand, PtyHandle};

/// 本机可用的本地 Shell 预设选项
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LocalShellOption {
    /// 白名单预设 ID（如 `powershell`、`pwsh`、`cmd`、`gitbash`、`default`）
    pub id: String,
    /// 前端展示名称
    pub label: String,
}

/// 枚举当前操作系统可用的本地终端预设（白名单探测）
pub fn list_local_shells() -> Vec<LocalShellOption> {
    #[cfg(windows)]
    {
        let mut shells = Vec::new();
        if which_in_path("pwsh.exe") {
            shells.push(LocalShellOption {
                id: "pwsh".into(),
                label: "PowerShell 7".into(),
            });
        }
        shells.push(LocalShellOption {
            id: "powershell".into(),
            label: "Windows PowerShell".into(),
        });
        if find_git_bash().is_some() {
            shells.push(LocalShellOption {
                id: "gitbash".into(),
                label: "Git Bash".into(),
            });
        }
        shells.push(LocalShellOption {
            id: "cmd".into(),
            label: "命令提示符 (CMD)".into(),
        });
        shells
    }

    #[cfg(not(windows))]
    {
        let mut shells = vec![LocalShellOption {
            id: "default".into(),
            label: "系统默认 Shell".into(),
        }];
        if Path::new("/bin/zsh").exists() || Path::new("/usr/bin/zsh").exists() {
            shells.push(LocalShellOption {
                id: "zsh".into(),
                label: "Zsh".into(),
            });
        }
        if Path::new("/bin/bash").exists() || Path::new("/usr/bin/bash").exists() {
            shells.push(LocalShellOption {
                id: "bash".into(),
                label: "Bash".into(),
            });
        }
        shells
    }
}

#[cfg(windows)]
fn which_in_path(exe: &str) -> bool {
    let Some(paths) = std::env::var_os("PATH") else {
        return false;
    };
    std::env::split_paths(&paths).any(|dir| dir.join(exe).is_file())
}

#[cfg(windows)]
fn find_git_bash() -> Option<String> {
    let candidates = [
        r"C:\Program Files\Git\bin\bash.exe",
        r"C:\Program Files (x86)\Git\bin\bash.exe",
    ];
    for p in candidates {
        if Path::new(p).is_file() {
            return Some(p.to_string());
        }
    }
    None
}

/// 按严格白名单将前端传入的 `profile` 映射为安全的本地 Shell 启动命令
fn build_shell_command(profile: &str, cwd: Option<&str>) -> AppResult<CommandBuilder> {
    #[cfg(windows)]
    let mut cmd = match profile {
        "pwsh" => {
            if !which_in_path("pwsh.exe") {
                return Err(AppError::InvalidInput(
                    "本机未检测到 PowerShell 7 (pwsh.exe)".into(),
                ));
            }
            let mut c = CommandBuilder::new("pwsh.exe");
            c.arg("-NoLogo");
            c
        }
        "powershell" | "default" | "" => {
            let mut c = CommandBuilder::new("powershell.exe");
            c.arg("-NoLogo");
            c
        }
        "gitbash" => {
            let bash = find_git_bash()
                .ok_or_else(|| AppError::InvalidInput("本机未检测到 Git Bash".into()))?;
            let mut c = CommandBuilder::new(bash);
            c.arg("--login");
            c.arg("-i");
            c
        }
        "cmd" => CommandBuilder::new("cmd.exe"),
        other => {
            return Err(AppError::SecurityPolicy(format!(
                "不允许启动非白名单本地 Shell 预设 '{}'",
                other
            )));
        }
    };

    #[cfg(not(windows))]
    let mut cmd = match profile {
        "default" | "" => {
            let shell = std::env::var("SHELL").unwrap_or_else(|_| "/bin/sh".to_string());
            let mut c = CommandBuilder::new(shell);
            c.arg("-l");
            c
        }
        "zsh" => {
            let path = if Path::new("/bin/zsh").exists() {
                "/bin/zsh"
            } else {
                "/usr/bin/zsh"
            };
            let mut c = CommandBuilder::new(path);
            c.arg("-l");
            c
        }
        "bash" => {
            let path = if Path::new("/bin/bash").exists() {
                "/bin/bash"
            } else {
                "/usr/bin/bash"
            };
            let mut c = CommandBuilder::new(path);
            c.arg("-l");
            c
        }
        other => {
            return Err(AppError::SecurityPolicy(format!(
                "不允许启动非白名单本地 Shell 预设 '{}'",
                other
            )));
        }
    };

    cmd.env("TERM", "xterm-256color");
    cmd.env("COLORTERM", "truecolor");

    if let Some(dir) = cwd.filter(|d| !d.trim().is_empty()) {
        if Path::new(dir).is_dir() {
            cmd.cwd(dir);
        }
    } else if let Some(home) = dirs::home_dir() {
        cmd.cwd(home);
    }

    Ok(cmd)
}

/// 启动原生本地 PTY 伪终端并返回与 SSH PTY 完全一致的 `PtyHandle`
pub fn open_local_pty(
    profile: &str,
    cwd: Option<&str>,
    cols: u32,
    rows: u32,
) -> AppResult<PtyHandle> {
    let safe_cols = cols.clamp(20, 500) as u16;
    let safe_rows = rows.clamp(5, 200) as u16;

    let cmd = build_shell_command(profile, cwd)?;
    let pty_system = native_pty_system();
    let pair = pty_system
        .openpty(PtySize {
            rows: safe_rows,
            cols: safe_cols,
            pixel_width: 0,
            pixel_height: 0,
        })
        .map_err(|e| AppError::Protocol(format!("创建本地 PTY 伪终端失败: {}", e)))?;

    let mut child = pair
        .slave
        .spawn_command(cmd)
        .map_err(|e| AppError::Protocol(format!("启动本地 Shell 进程失败: {}", e)))?;
    // 父进程释放 slave 句柄，确保子进程退出时 reader 能够收到 EOF
    drop(pair.slave);

    let mut reader = pair
        .master
        .try_clone_reader()
        .map_err(|e| AppError::Protocol(format!("打开本地 PTY 读取流失败: {}", e)))?;
    let mut writer = pair
        .master
        .take_writer()
        .map_err(|e| AppError::Protocol(format!("打开本地 PTY 写入流失败: {}", e)))?;
    let master = pair.master;

    let (cmd_tx, mut cmd_rx) = tokio::sync::mpsc::channel::<PtyCommand>(256);
    let (output_tx, output_rx) = tokio::sync::mpsc::channel::<Vec<u8>>(256);

    // 独立阻塞线程持续读取本地 PTY 输出字节流
    std::thread::spawn(move || {
        let mut buf = [0u8; 4096];
        loop {
            match reader.read(&mut buf) {
                Ok(0) | Err(_) => break,
                Ok(n) => {
                    if output_tx.blocking_send(buf[..n].to_vec()).is_err() {
                        break;
                    }
                }
            }
        }
    });

    // 独立阻塞线程处理写入、Resize 与安全退出子进程
    std::thread::spawn(move || {
        while let Some(cmd) = cmd_rx.blocking_recv() {
            match cmd {
                PtyCommand::Input(bytes) => {
                    if writer.write_all(&bytes).is_err() || writer.flush().is_err() {
                        break;
                    }
                }
                PtyCommand::Resize { cols, rows } => {
                    let _ = master.resize(PtySize {
                        rows: rows.clamp(5, 200) as u16,
                        cols: cols.clamp(20, 500) as u16,
                        pixel_width: 0,
                        pixel_height: 0,
                    });
                }
                PtyCommand::Close => {
                    let _ = child.kill();
                    let _ = child.wait();
                    return;
                }
            }
        }
        let _ = child.kill();
        let _ = child.wait();
    });

    Ok(PtyHandle { cmd_tx, output_rx })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_local_shell_whitelist_rejects_arbitrary_binary() {
        assert!(!list_local_shells().is_empty());
        assert!(build_shell_command("calc.exe", None).is_err());
        assert!(build_shell_command("/bin/rm -rf /", None).is_err());
        assert!(build_shell_command("default", None).is_ok());
    }
}
