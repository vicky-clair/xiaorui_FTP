//! 跨平台本地原生交互终端（Local PTY）模块
//!
//! 基于 `portable-pty`（Windows ConPTY / macOS & Linux Unix PTY）实现，
//! 安全边界与体验特性：
//! - 严格限定内置白名单 Shell 预设（禁止前端传入任意可执行程序路径，绝不直接执行外部 JSON 中的命令行）；
//! - 在 Windows 上自动只读解析 Windows Terminal `settings.json`（支持 JSONC 注释与尾逗号），
//!   将其默认配置文件安全映射至内置白名单，并同步读取默认字体、字号、光标样式与配色方案；
//! - 复用统一 `PtyHandle` / `PtyCommand` 接口，受后端窗口归属隔离与输出限流保护；
//! - 收到 `PtyCommand::Close` 时主动终止子进程，避免残留孤儿 Shell 进程。

use std::io::{Read, Write};
use std::path::Path;

use portable_pty::{native_pty_system, CommandBuilder, PtySize};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::error::{AppError, AppResult};
use crate::protocols::{PtyCommand, PtyHandle};

/// 本机可用的本地 Shell 预设选项
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LocalShellOption {
    /// 白名单预设 ID（如 `default`、`pwsh`、`powershell`、`cmd`、`gitbash`、`wsl`）
    pub id: String,
    /// 前端展示名称
    pub label: String,
}

/// 可直接喂给 xterm.js `ITheme` 的终端配色方案
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct TerminalColorScheme {
    pub background: String,
    pub foreground: String,
    pub cursor: Option<String>,
    pub selection_background: Option<String>,
    pub black: Option<String>,
    pub red: Option<String>,
    pub green: Option<String>,
    pub yellow: Option<String>,
    pub blue: Option<String>,
    pub magenta: Option<String>,
    pub cyan: Option<String>,
    pub white: Option<String>,
    pub bright_black: Option<String>,
    pub bright_red: Option<String>,
    pub bright_green: Option<String>,
    pub bright_yellow: Option<String>,
    pub bright_blue: Option<String>,
    pub bright_magenta: Option<String>,
    pub bright_cyan: Option<String>,
    pub bright_white: Option<String>,
}

/// 本机默认终端配置（Windows 下自动读取 Windows Terminal `settings.json`，其余平台回退系统默认）
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LocalTerminalDefaults {
    /// 映射后的内置安全白名单 Shell ID（`pwsh` / `powershell` / `cmd` / `gitbash` / `wsl` / `default`）
    pub profile_id: String,
    /// 展示标签（如 `PowerShell 7` / `Windows PowerShell` / `WSL`）
    pub profile_label: String,
    /// 配置来源说明（如 `Windows Terminal 默认配置` / `系统默认配置`）
    pub source: String,
    /// 默认字体族名称（来自 Windows Terminal `font.face` 或系统默认 `Cascadia Mono`）
    pub font_family: Option<String>,
    /// 默认字号（限定在 10..=24）
    pub font_size: Option<u32>,
    /// 光标样式（`bar` / `block` / `underline`）
    pub cursor_style: Option<String>,
    /// 配色方案名称
    pub scheme_name: Option<String>,
    /// 解析后的配色方案（若来源为 Windows Terminal，则 1:1 复刻其默认主题配色）
    pub theme: Option<TerminalColorScheme>,
}

/// 去除 Windows Terminal `settings.json` 中合法的 JSONC 注释（`//`、`/* */`）与尾逗号（完整保留 UTF-8 中文字符串）
fn strip_jsonc(input: &str) -> String {
    let bytes = input.as_bytes();
    let len = bytes.len();
    let mut out = String::with_capacity(len);
    let mut i = 0;
    let mut in_string = false;
    let mut escaped = false;

    while i < len {
        let ch = bytes[i];
        if in_string {
            if ch < 0x80 {
                out.push(ch as char);
                if escaped {
                    escaped = false;
                } else if ch == b'\\' {
                    escaped = true;
                } else if ch == b'"' {
                    in_string = false;
                }
                i += 1;
            } else if let Some(c) = input[i..].chars().next() {
                // 非 ASCII UTF-8 多字节字符必须按 char 写入，避免把中文路径/名称拆成乱码
                out.push(c);
                escaped = false;
                i += c.len_utf8();
            } else {
                break;
            }
            continue;
        }

        if ch == b'"' {
            in_string = true;
            out.push('"');
            i += 1;
        } else if ch == b'/' && i + 1 < len && bytes[i + 1] == b'/' {
            i += 2;
            while i < len && bytes[i] != b'\n' {
                i += 1;
            }
        } else if ch == b'/' && i + 1 < len && bytes[i + 1] == b'*' {
            i += 2;
            while i + 1 < len && !(bytes[i] == b'*' && bytes[i + 1] == b'/') {
                i += 1;
            }
            i = (i + 2).min(len);
        } else if let Some(c) = input[i..].chars().next() {
            out.push(c);
            i += c.len_utf8();
        } else {
            break;
        }
    }

    // 清理对象/数组末尾多余逗号（如 `{"a": 1,}`）
    let mut cleaned = String::with_capacity(out.len());
    let chars: Vec<char> = out.chars().collect();
    let mut in_str = false;
    let mut esc = false;
    for idx in 0..chars.len() {
        let c = chars[idx];
        if in_str {
            cleaned.push(c);
            if esc {
                esc = false;
            } else if c == '\\' {
                esc = true;
            } else if c == '"' {
                in_str = false;
            }
            continue;
        }
        if c == '"' {
            in_str = true;
            cleaned.push(c);
            continue;
        }
        if c == ',' {
            let mut j = idx + 1;
            while j < chars.len() && chars[j].is_whitespace() {
                j += 1;
            }
            if j < chars.len() && (chars[j] == '}' || chars[j] == ']') {
                continue;
            }
        }
        cleaned.push(c);
    }
    cleaned
}

/// 内置 Windows Terminal 官方默认配色表（当 `settings.json` 仅指定内置方案名而未在 `schemes` 重定义时使用）
fn builtin_wt_scheme(name: &str) -> Option<TerminalColorScheme> {
    match name.to_ascii_lowercase().as_str() {
        "campbell" => Some(TerminalColorScheme {
            background: "#0C0C0C".into(),
            foreground: "#CCCCCC".into(),
            cursor: Some("#FFFFFF".into()),
            selection_background: Some("rgba(255, 255, 255, 0.25)".into()),
            black: Some("#0C0C0C".into()),
            red: Some("#C50F1F".into()),
            green: Some("#13A10E".into()),
            yellow: Some("#C19C00".into()),
            blue: Some("#0037DA".into()),
            magenta: Some("#881798".into()),
            cyan: Some("#3A96DD".into()),
            white: Some("#CCCCCC".into()),
            bright_black: Some("#767676".into()),
            bright_red: Some("#E74856".into()),
            bright_green: Some("#16C60C".into()),
            bright_yellow: Some("#F9F1A5".into()),
            bright_blue: Some("#3B78FF".into()),
            bright_magenta: Some("#B4009E".into()),
            bright_cyan: Some("#61D6D6".into()),
            bright_white: Some("#F2F2F2".into()),
        }),
        "campbell powershell" => Some(TerminalColorScheme {
            background: "#012456".into(),
            foreground: "#CCCCCC".into(),
            cursor: Some("#FFFFFF".into()),
            selection_background: Some("rgba(255, 255, 255, 0.28)".into()),
            black: Some("#0C0C0C".into()),
            red: Some("#C50F1F".into()),
            green: Some("#13A10E".into()),
            yellow: Some("#C19C00".into()),
            blue: Some("#0037DA".into()),
            magenta: Some("#881798".into()),
            cyan: Some("#3A96DD".into()),
            white: Some("#CCCCCC".into()),
            bright_black: Some("#767676".into()),
            bright_red: Some("#E74856".into()),
            bright_green: Some("#16C60C".into()),
            bright_yellow: Some("#F9F1A5".into()),
            bright_blue: Some("#3B78FF".into()),
            bright_magenta: Some("#B4009E".into()),
            bright_cyan: Some("#61D6D6".into()),
            bright_white: Some("#F2F2F2".into()),
        }),
        "one half dark" => Some(TerminalColorScheme {
            background: "#282C34".into(),
            foreground: "#DCDFE4".into(),
            cursor: Some("#FFFFFF".into()),
            selection_background: Some("rgba(97, 175, 239, 0.3)".into()),
            black: Some("#282C34".into()),
            red: Some("#E06C75".into()),
            green: Some("#98C379".into()),
            yellow: Some("#E5C07B".into()),
            blue: Some("#61AFEF".into()),
            magenta: Some("#C678DD".into()),
            cyan: Some("#56B6C2".into()),
            white: Some("#DCDFE4".into()),
            bright_black: Some("#5A6374".into()),
            bright_red: Some("#E06C75".into()),
            bright_green: Some("#98C379".into()),
            bright_yellow: Some("#E5C07B".into()),
            bright_blue: Some("#61AFEF".into()),
            bright_magenta: Some("#C678DD".into()),
            bright_cyan: Some("#56B6C2".into()),
            bright_white: Some("#DCDFE4".into()),
        }),
        "one half light" => Some(TerminalColorScheme {
            background: "#FAFAFA".into(),
            foreground: "#383A42".into(),
            cursor: Some("#4F525D".into()),
            selection_background: Some("rgba(1, 132, 188, 0.22)".into()),
            black: Some("#383A42".into()),
            red: Some("#E45649".into()),
            green: Some("#50A14F".into()),
            yellow: Some("#C18301".into()),
            blue: Some("#0184BC".into()),
            magenta: Some("#A626A4".into()),
            cyan: Some("#0997B3".into()),
            white: Some("#FAFAFA".into()),
            bright_black: Some("#4F525D".into()),
            bright_red: Some("#DF6C75".into()),
            bright_green: Some("#98C379".into()),
            bright_yellow: Some("#E4C07A".into()),
            bright_blue: Some("#61AFEF".into()),
            bright_magenta: Some("#C577DD".into()),
            bright_cyan: Some("#56B5C1".into()),
            bright_white: Some("#FFFFFF".into()),
        }),
        "tango dark" => Some(TerminalColorScheme {
            background: "#000000".into(),
            foreground: "#D3D7CF".into(),
            cursor: Some("#FFFFFF".into()),
            selection_background: Some("rgba(255, 255, 255, 0.24)".into()),
            black: Some("#000000".into()),
            red: Some("#CC0000".into()),
            green: Some("#4E9A06".into()),
            yellow: Some("#C4A000".into()),
            blue: Some("#3465A4".into()),
            magenta: Some("#75507B".into()),
            cyan: Some("#06989A".into()),
            white: Some("#D3D7CF".into()),
            bright_black: Some("#555753".into()),
            bright_red: Some("#EF2929".into()),
            bright_green: Some("#8AE234".into()),
            bright_yellow: Some("#FCE94F".into()),
            bright_blue: Some("#729FCF".into()),
            bright_magenta: Some("#AD7FA8".into()),
            bright_cyan: Some("#34E2E2".into()),
            bright_white: Some("#EEEEEC".into()),
        }),
        _ => None,
    }
}

fn json_str(obj: Option<&Value>, key: &str) -> Option<String> {
    obj?.get(key)?
        .as_str()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
}

fn parse_wt_scheme_value(val: &Value) -> Option<TerminalColorScheme> {
    let bg = json_str(Some(val), "background")?;
    let fg = json_str(Some(val), "foreground")?;
    Some(TerminalColorScheme {
        background: bg,
        foreground: fg,
        cursor: json_str(Some(val), "cursorColor"),
        selection_background: json_str(Some(val), "selectionBackground"),
        black: json_str(Some(val), "black"),
        red: json_str(Some(val), "red"),
        green: json_str(Some(val), "green"),
        yellow: json_str(Some(val), "yellow"),
        blue: json_str(Some(val), "blue"),
        magenta: json_str(Some(val), "purple").or_else(|| json_str(Some(val), "magenta")),
        cyan: json_str(Some(val), "cyan"),
        white: json_str(Some(val), "white"),
        bright_black: json_str(Some(val), "brightBlack"),
        bright_red: json_str(Some(val), "brightRed"),
        bright_green: json_str(Some(val), "brightGreen"),
        bright_yellow: json_str(Some(val), "brightYellow"),
        bright_blue: json_str(Some(val), "brightBlue"),
        bright_magenta: json_str(Some(val), "brightPurple")
            .or_else(|| json_str(Some(val), "brightMagenta")),
        bright_cyan: json_str(Some(val), "brightCyan"),
        bright_white: json_str(Some(val), "brightWhite"),
    })
}

/// 将 Windows Terminal 的 profile 元数据安全映射到内置白名单 Shell（绝不直接执行外部 commandline）
fn map_wt_profile_to_whitelist(profile: &Value) -> (&'static str, &'static str) {
    let source = json_str(Some(profile), "source")
        .unwrap_or_default()
        .to_ascii_lowercase();
    let cmd = json_str(Some(profile), "commandline")
        .unwrap_or_default()
        .to_ascii_lowercase();
    let name = json_str(Some(profile), "name")
        .unwrap_or_default()
        .to_ascii_lowercase();

    if source.contains("powershellcore") || cmd.contains("pwsh") {
        return ("pwsh", "PowerShell 7");
    }
    if source.contains("wsl") || cmd.contains("wsl.exe") || cmd == "wsl" {
        return ("wsl", "WSL");
    }
    if cmd.contains("git") && cmd.contains("bash")
        || name.contains("git bash")
        || source.contains("git")
    {
        return ("gitbash", "Git Bash");
    }
    if cmd.contains("cmd.exe")
        || name == "command prompt"
        || name.contains("命令提示符")
        || name == "cmd"
    {
        return ("cmd", "命令提示符 (CMD)");
    }
    ("powershell", "Windows PowerShell")
}

/// 解析 Windows Terminal `settings.json` 文本内容，提取默认 Shell（安全白名单映射）、字体、字号、光标与配色
pub fn parse_windows_terminal_settings_json(raw: &str) -> Option<LocalTerminalDefaults> {
    let cleaned = strip_jsonc(raw);
    let root: Value = serde_json::from_str(&cleaned).ok()?;
    let default_guid = json_str(Some(&root), "defaultProfile").unwrap_or_default();

    let profiles_obj = root.get("profiles");
    let defaults_obj = profiles_obj.and_then(|p| p.get("defaults"));
    let list_arr = profiles_obj
        .and_then(|p| p.get("list").or(Some(p)))
        .and_then(|v| v.as_array());

    let matched_profile = list_arr.and_then(|arr| {
        if !default_guid.is_empty() {
            if let Some(found) = arr.iter().find(|item| {
                json_str(Some(item), "guid").is_some_and(|g| g.eq_ignore_ascii_case(&default_guid))
            }) {
                return Some(found);
            }
        }
        arr.first()
    });

    let (profile_id, profile_label) = matched_profile
        .map(map_wt_profile_to_whitelist)
        .unwrap_or(("powershell", "Windows PowerShell"));

    // 字体族与字号：profile 显式配置优先，其次 profiles.defaults，兼顾旧版 fontFace/fontSize 字段
    let extract_font_face = |obj: Option<&Value>| -> Option<String> {
        obj.and_then(|o| o.get("font"))
            .and_then(|f| json_str(Some(f), "face"))
            .or_else(|| json_str(obj, "fontFace"))
    };
    let extract_font_size = |obj: Option<&Value>| -> Option<u32> {
        let raw_num = obj
            .and_then(|o| o.get("font"))
            .and_then(|f| f.get("size"))
            .or_else(|| obj.and_then(|o| o.get("fontSize")))?;
        // Windows Terminal 字号单位为 pt，xterm.js 使用 CSS px（1pt ≈ 4/3 px），按比例换算以保持视觉一致
        let pt = raw_num
            .as_f64()
            .or_else(|| raw_num.as_u64().map(|n| n as f64))?;
        let px = (pt * 4.0 / 3.0).round() as u32;
        Some(px.clamp(11, 24))
    };

    let font_family =
        extract_font_face(matched_profile).or_else(|| extract_font_face(defaults_obj));
    let font_size = extract_font_size(matched_profile).or_else(|| extract_font_size(defaults_obj));

    // 光标样式映射（bar / block / underline）
    let raw_cursor = json_str(matched_profile, "cursorShape")
        .or_else(|| json_str(defaults_obj, "cursorShape"))
        .map(|s| s.to_ascii_lowercase());
    let cursor_style = raw_cursor.as_deref().map(|s| match s {
        "filledbox" | "vintage" | "emptybox" => "block".to_string(),
        "underscore" => "underline".to_string(),
        _ => "bar".to_string(),
    });

    // 配色方案名（支持字符串或 `{ "dark": "...", "light": "..." }` 对象）
    let extract_scheme_name = |obj: Option<&Value>| -> Option<String> {
        let val = obj?.get("colorScheme")?;
        if let Some(s) = val.as_str() {
            return Some(s.trim().to_string()).filter(|x| !x.is_empty());
        }
        if let Some(map) = val.as_object() {
            if let Some(d) = map.get("dark").and_then(|v| v.as_str()) {
                return Some(d.trim().to_string()).filter(|x| !x.is_empty());
            }
            if let Some(l) = map.get("light").and_then(|v| v.as_str()) {
                return Some(l.trim().to_string()).filter(|x| !x.is_empty());
            }
        }
        None
    };

    let scheme_name = extract_scheme_name(matched_profile)
        .or_else(|| extract_scheme_name(defaults_obj))
        .unwrap_or_else(|| "Campbell".to_string());

    let custom_scheme = root
        .get("schemes")
        .and_then(|s| s.as_array())
        .and_then(|arr| {
            arr.iter().find(|s| {
                json_str(Some(s), "name").is_some_and(|n| n.eq_ignore_ascii_case(&scheme_name))
            })
        })
        .and_then(parse_wt_scheme_value);

    let mut theme = custom_scheme
        .or_else(|| builtin_wt_scheme(&scheme_name))
        .or_else(|| builtin_wt_scheme("Campbell"));

    // 若 profile 显式覆写了 background / foreground / cursorColor，合并到主题
    if let Some(t) = theme.as_mut() {
        if let Some(bg) =
            json_str(matched_profile, "background").or_else(|| json_str(defaults_obj, "background"))
        {
            t.background = bg;
        }
        if let Some(fg) =
            json_str(matched_profile, "foreground").or_else(|| json_str(defaults_obj, "foreground"))
        {
            t.foreground = fg;
        }
        if let Some(cc) = json_str(matched_profile, "cursorColor")
            .or_else(|| json_str(defaults_obj, "cursorColor"))
        {
            t.cursor = Some(cc);
        }
    }

    Some(LocalTerminalDefaults {
        profile_id: profile_id.to_string(),
        profile_label: profile_label.to_string(),
        source: "Windows Terminal 默认配置".to_string(),
        font_family,
        font_size,
        cursor_style,
        scheme_name: Some(scheme_name),
        theme,
    })
}

#[cfg(windows)]
fn read_windows_terminal_defaults() -> Option<LocalTerminalDefaults> {
    let local_app_data = std::env::var_os("LOCALAPPDATA")?;
    let base = Path::new(&local_app_data);
    let candidates = [
        base.join(r"Packages\Microsoft.WindowsTerminal_8wekyb3d8bbwe\LocalState\settings.json"),
        base.join(
            r"Packages\Microsoft.WindowsTerminalPreview_8wekyb3d8bbwe\LocalState\settings.json",
        ),
        base.join(r"Microsoft\Windows Terminal\settings.json"),
    ];

    for path in candidates {
        if let Ok(raw) = std::fs::read_to_string(&path) {
            if let Some(mut parsed) = parse_windows_terminal_settings_json(&raw) {
                // 若配置的默认 Shell 在本机暂不可用，则安全回退到已安装的默认 PowerShell
                let available = match parsed.profile_id.as_str() {
                    "pwsh" => find_pwsh_exe().is_some(),
                    "gitbash" => find_git_bash().is_some(),
                    "wsl" => find_in_path("wsl.exe").is_some(),
                    _ => true,
                };
                if !available {
                    if find_pwsh_exe().is_some() {
                        parsed.profile_id = "pwsh".into();
                        parsed.profile_label = "PowerShell 7".into();
                    } else {
                        parsed.profile_id = "powershell".into();
                        parsed.profile_label = "Windows PowerShell".into();
                    }
                }
                return Some(parsed);
            }
        }
    }
    None
}

/// 获取当前操作系统的默认本地终端配置（Windows 上自动读取 Windows Terminal 默认配置）
pub fn get_local_terminal_defaults() -> LocalTerminalDefaults {
    #[cfg(windows)]
    {
        if let Some(wt) = read_windows_terminal_defaults() {
            return wt;
        }
        let (profile_id, profile_label) = if find_pwsh_exe().is_some() {
            ("pwsh", "PowerShell 7")
        } else {
            ("powershell", "Windows PowerShell")
        };
        LocalTerminalDefaults {
            profile_id: profile_id.into(),
            profile_label: profile_label.into(),
            source: "Windows 系统默认配置".into(),
            font_family: Some("Cascadia Mono".into()),
            font_size: Some(14),
            cursor_style: Some("bar".into()),
            scheme_name: Some("Campbell".into()),
            theme: builtin_wt_scheme("Campbell"),
        }
    }

    #[cfg(not(windows))]
    {
        LocalTerminalDefaults {
            profile_id: "default".into(),
            profile_label: "系统默认 Shell".into(),
            source: "系统默认配置".into(),
            font_family: None,
            font_size: Some(13),
            cursor_style: Some("bar".into()),
            scheme_name: None,
            theme: None,
        }
    }
}

/// 枚举当前操作系统可用的本地终端预设（白名单探测，并将 Windows 默认项置顶）
pub fn list_local_shells() -> Vec<LocalShellOption> {
    #[cfg(windows)]
    {
        let defaults = get_local_terminal_defaults();
        let mut shells = vec![LocalShellOption {
            id: "default".into(),
            label: format!("{} (默认)", defaults.profile_label),
        }];
        if find_pwsh_exe().is_some() {
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
        if find_in_path("wsl.exe").is_some() {
            shells.push(LocalShellOption {
                id: "wsl".into(),
                label: "WSL".into(),
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

/// 在 PATH 中查找可执行文件（优先真实文件，同时兼容 WindowsApps MSIX App Execution Alias 重解析点）
#[cfg(windows)]
fn find_in_path(exe: &str) -> Option<std::path::PathBuf> {
    let paths = std::env::var_os("PATH")?;
    let mut alias_fallback: Option<std::path::PathBuf> = None;
    for dir in std::env::split_paths(&paths) {
        let candidate = dir.join(exe);
        if candidate.is_file() {
            return Some(candidate);
        }
        if alias_fallback.is_none() && std::fs::symlink_metadata(&candidate).is_ok() {
            alias_fallback = Some(candidate);
        }
    }
    alias_fallback
}

/// 探测本机 PowerShell 7 (`pwsh.exe`)，覆盖 PATH、MSI、Scoop 与 Microsoft Store (WindowsApps)
#[cfg(windows)]
fn find_pwsh_exe() -> Option<std::path::PathBuf> {
    if let Some(p) = find_in_path("pwsh.exe") {
        return Some(p);
    }
    let mut candidates = vec![
        std::path::PathBuf::from(r"C:\Program Files\PowerShell\7\pwsh.exe"),
        std::path::PathBuf::from(r"C:\Program Files\PowerShell\7-preview\pwsh.exe"),
    ];
    if let Some(home) = dirs::home_dir() {
        candidates.push(home.join(r"scoop\apps\pwsh\current\pwsh.exe"));
        candidates.push(home.join(r"scoop\shims\pwsh.exe"));
    }
    if let Some(local_app) = std::env::var_os("LOCALAPPDATA") {
        candidates.push(Path::new(&local_app).join(r"Microsoft\WindowsApps\pwsh.exe"));
    }
    candidates
        .into_iter()
        .find(|p| p.is_file() || std::fs::symlink_metadata(p).is_ok())
}

/// 探测本机 Git Bash (`bash.exe`)，覆盖标准安装路径、Scoop 及 PATH 中的 git 相对目录
#[cfg(windows)]
fn find_git_bash() -> Option<std::path::PathBuf> {
    let mut candidates = vec![
        std::path::PathBuf::from(r"C:\Program Files\Git\bin\bash.exe"),
        std::path::PathBuf::from(r"C:\Program Files (x86)\Git\bin\bash.exe"),
    ];
    if let Some(home) = dirs::home_dir() {
        candidates.push(home.join(r"scoop\apps\git\current\bin\bash.exe"));
    }
    if let Some(local_app) = std::env::var_os("LOCALAPPDATA") {
        candidates.push(Path::new(&local_app).join(r"Programs\Git\bin\bash.exe"));
    }
    if let Some(git_exe) = find_in_path("git.exe") {
        if let Some(git_root) = git_exe.parent().and_then(|p| p.parent()) {
            candidates.push(git_root.join(r"bin\bash.exe"));
        }
    }
    candidates.into_iter().find(|p| p.is_file())
}

/// 按严格白名单将前端传入的 `profile` 映射为安全的本地 Shell 启动命令
fn build_shell_command(profile: &str, cwd: Option<&str>) -> AppResult<CommandBuilder> {
    #[cfg(windows)]
    let effective_profile: String = if profile == "default" || profile.is_empty() {
        get_local_terminal_defaults().profile_id
    } else {
        profile.to_string()
    };

    #[cfg(windows)]
    let mut cmd = match effective_profile.as_str() {
        "pwsh" => {
            let pwsh = find_pwsh_exe().ok_or_else(|| {
                AppError::InvalidInput("本机未检测到 PowerShell 7 (pwsh.exe)".into())
            })?;
            let mut c = CommandBuilder::new(pwsh);
            c.arg("-NoLogo");
            c
        }
        "powershell" | "default" | "" => {
            let mut c = CommandBuilder::new("powershell.exe");
            c.arg("-NoLogo");
            c.arg("-NoExit");
            // 统一 Windows PowerShell 5.1 交互控制台输入/输出为 UTF-8，防止中文在 xterm.js 中乱码
            c.arg("-Command");
            c.arg("[Console]::InputEncoding = [Console]::OutputEncoding = [System.Text.UTF8Encoding]::new($false); $null = chcp 65001");
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
        "wsl" => {
            let wsl = find_in_path("wsl.exe")
                .ok_or_else(|| AppError::InvalidInput("本机未检测到 WSL (wsl.exe)".into()))?;
            CommandBuilder::new(wsl)
        }
        "cmd" => {
            let mut c = CommandBuilder::new("cmd.exe");
            // 自动切至 UTF-8 代码页（65001），消除 Windows 中文 CMD 目录与输出在 xterm.js 中的 GBK 乱码
            c.arg("/K");
            c.arg("chcp 65001>nul");
            c
        }
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
    cmd.env("PYTHONUTF8", "1");
    cmd.env("LANG", "zh_CN.UTF-8");

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

    #[test]
    fn test_parse_windows_terminal_jsonc_defaults() {
        let sample = r##"{
            // Windows Terminal 默认配置示例（含 JSONC 注释、中文路径与尾逗号）
            "defaultProfile": "{574e775e-4f2a-5b96-ac1e-a2962a402336}",
            "profiles": {
                "defaults": {
                    "backgroundImage": "%USERPROFILE%\\Pictures\\壁纸文件\\demo.jpg",
                    "font": { "face": "JetBrainsMono NFM, Cascadia Code", "size": 11 },
                    "cursorShape": "filledBox",
                    "colorScheme": "Custom Dark",
                },
                "list": [
                    {
                        "guid": "{61c54bbd-c2c6-5271-96e7-009a87ff44bf}",
                        "name": "Windows PowerShell",
                        "commandline": "powershell.exe"
                    },
                    {
                        "guid": "{574e775e-4f2a-5b96-ac1e-a2962a402336}",
                        "name": "PowerShell 7",
                        "source": "Windows.Terminal.PowershellCore",
                    },
                ],
            },
            "schemes": [
                {
                    "name": "Custom Dark",
                    "background": "#10141C",
                    "foreground": "#E2E8F0",
                    "cursorColor": "#38BDF8",
                    "purple": "#C084FC",
                }
            ]
        }"##;

        let parsed = parse_windows_terminal_settings_json(sample).expect("parse wt settings");
        assert_eq!(parsed.profile_id, "pwsh");
        assert_eq!(parsed.profile_label, "PowerShell 7");
        assert_eq!(
            parsed.font_family.as_deref(),
            Some("JetBrainsMono NFM, Cascadia Code")
        );
        // 11pt ≈ 15px
        assert_eq!(parsed.font_size, Some(15));
        assert_eq!(parsed.cursor_style.as_deref(), Some("block"));
        let theme = parsed.theme.expect("resolved theme");
        assert_eq!(theme.background, "#10141C");
        assert_eq!(theme.foreground, "#E2E8F0");
        assert_eq!(theme.cursor.as_deref(), Some("#38BDF8"));
        assert_eq!(theme.magenta.as_deref(), Some("#C084FC"));
    }
}
