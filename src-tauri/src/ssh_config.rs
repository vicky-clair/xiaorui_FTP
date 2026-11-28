//! OpenSSH `~/.ssh/config` 解析与一键导入模块
//!
//! 支持解析常见 OpenSSH 客户端配置项：
//! - `Host`（自动忽略 `*` / `?` 通配符或 `!` 排除项，并将 `Host *` 块内的配置作为全局默认值继承）
//! - `HostName`、`User`、`Port`、`IdentityFile`（自动展开 `~/` 为用户主目录）
//! - `ProxyJump`（解析 `user@bastion:port` 映射为跳板机配置）

use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::error::AppResult;

/// 从 `~/.ssh/config` 解析出的单条主机候选记录
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct SshConfigHost {
    /// OpenSSH `Host` 别名（用作站点展示名称）
    pub alias: String,
    /// 实际连接地址（`HostName`，缺省时回退 `alias`）
    pub host: String,
    /// SSH 端口（默认 22）
    pub port: u16,
    /// 登录用户名
    pub username: String,
    /// 本地私钥文件路径（已展开 `~/`）
    pub private_key_path: Option<String>,
    /// 跳板机主机名（来自 `ProxyJump`）
    pub jump_host: Option<String>,
    /// 跳板机端口
    pub jump_port: Option<u16>,
    /// 跳板机用户名
    pub jump_username: Option<String>,
}

#[derive(Default, Clone)]
struct RawBlock {
    patterns: Vec<String>,
    hostname: Option<String>,
    user: Option<String>,
    port: Option<u16>,
    identity_file: Option<String>,
    proxy_jump: Option<String>,
}

/// 将 `~/` 前缀展开为本地用户主目录
fn expand_tilde(raw: &str, home_dir: Option<&Path>) -> String {
    let trimmed = raw.trim().trim_matches('"').trim_matches('\'');
    if let Some(rest) = trimmed
        .strip_prefix("~/")
        .or_else(|| trimmed.strip_prefix("~\\"))
    {
        if let Some(home) = home_dir {
            return home.join(rest).to_string_lossy().to_string();
        }
    }
    trimmed.to_string()
}

/// 解析 `ProxyJump` 字段（如 `ops@bastion.example.com:2222`），多跳取第一跳
fn parse_proxy_jump(raw: &str) -> (Option<String>, Option<u16>, Option<String>) {
    let first_hop = raw.split(',').next().unwrap_or("").trim();
    if first_hop.is_empty() || first_hop.eq_ignore_ascii_case("none") {
        return (None, None, None);
    }
    let (user_part, host_port) = match first_hop.rsplit_once('@') {
        Some((u, hp)) => (
            Some(u.trim().to_string()).filter(|s| !s.is_empty()),
            hp.trim(),
        ),
        None => (None, first_hop),
    };
    let (host, port) = if let Some(rest) = host_port.strip_prefix('[') {
        // IPv6: [::1]:2222
        if let Some((h, p)) = rest.split_once("]:") {
            (h.to_string(), p.parse::<u16>().ok())
        } else {
            (rest.trim_end_matches(']').to_string(), None)
        }
    } else if let Some((h, p)) = host_port.rsplit_once(':') {
        if let Ok(port_num) = p.parse::<u16>() {
            (h.to_string(), Some(port_num))
        } else {
            (host_port.to_string(), None)
        }
    } else {
        (host_port.to_string(), None)
    };
    if host.is_empty() {
        return (None, None, None);
    }
    (Some(host), port.or(Some(22)), user_part)
}

/// 拆分单行 OpenSSH 指令为 `(key, value)`，支持空格分隔与 `=` 分隔并剥离行尾注释
fn split_directive(line: &str) -> Option<(String, String)> {
    let mut in_quote = false;
    let mut clean = String::with_capacity(line.len());
    for ch in line.chars() {
        if ch == '"' {
            in_quote = !in_quote;
            clean.push(ch);
        } else if ch == '#' && !in_quote {
            break;
        } else {
            clean.push(ch);
        }
    }
    let trimmed = clean.trim();
    if trimmed.is_empty() {
        return None;
    }

    // 兼容 `Key=Value` 与 `Key Value` 两种 OpenSSH 语法
    let idx = trimmed.find(|c: char| c.is_whitespace() || c == '=')?;
    let key = trimmed[..idx].trim().to_ascii_lowercase();
    let val = trimmed[idx..]
        .trim_start_matches(|c: char| c.is_whitespace() || c == '=')
        .trim()
        .to_string();
    if key.is_empty() || val.is_empty() {
        return None;
    }
    Some((key, val))
}

/// 解析 OpenSSH `config` 文本内容为主机配置列表
pub fn parse_ssh_config_str(content: &str, home_dir: Option<&Path>) -> Vec<SshConfigHost> {
    let mut blocks: Vec<RawBlock> = Vec::new();
    let mut current: Option<RawBlock> = None;

    for raw_line in content.lines() {
        let Some((key, val)) = split_directive(raw_line) else {
            continue;
        };
        match key.as_str() {
            "host" => {
                if let Some(prev) = current.take() {
                    blocks.push(prev);
                }
                let patterns: Vec<String> = val
                    .split_whitespace()
                    .map(|s| s.trim_matches('"').to_string())
                    .filter(|s| !s.is_empty())
                    .collect();
                current = Some(RawBlock {
                    patterns,
                    ..Default::default()
                });
            }
            "match" => {
                // 暂不合并复杂条件 Match 块，避免污染上一个 Host 块
                if let Some(prev) = current.take() {
                    blocks.push(prev);
                }
            }
            _ => {
                if let Some(block) = current.as_mut() {
                    match key.as_str() {
                        "hostname" if block.hostname.is_none() => {
                            block.hostname = Some(val.trim_matches('"').to_string());
                        }
                        "user" if block.user.is_none() => {
                            block.user = Some(val.trim_matches('"').to_string());
                        }
                        "port" if block.port.is_none() => {
                            block.port = val.trim_matches('"').parse::<u16>().ok();
                        }
                        "identityfile" if block.identity_file.is_none() => {
                            block.identity_file = Some(expand_tilde(&val, home_dir));
                        }
                        "proxyjump" if block.proxy_jump.is_none() => {
                            block.proxy_jump = Some(val.trim_matches('"').to_string());
                        }
                        _ => {}
                    }
                }
            }
        }
    }
    if let Some(last) = current {
        blocks.push(last);
    }

    // 收集 `Host *` 全局默认值
    let mut default_user: Option<String> = None;
    let mut default_port: Option<u16> = None;
    let mut default_key: Option<String> = None;
    let mut default_jump: Option<String> = None;
    for b in &blocks {
        if b.patterns.iter().any(|p| p == "*") {
            if default_user.is_none() {
                default_user = b.user.clone();
            }
            if default_port.is_none() {
                default_port = b.port;
            }
            if default_key.is_none() {
                default_key = b.identity_file.clone();
            }
            if default_jump.is_none() {
                default_jump = b.proxy_jump.clone();
            }
        }
    }

    let mut result = Vec::new();
    let mut seen_aliases = std::collections::HashSet::new();

    for b in blocks {
        for pat in &b.patterns {
            if pat.contains('*') || pat.contains('?') || pat.starts_with('!') {
                continue;
            }
            if !seen_aliases.insert(pat.clone()) {
                continue;
            }
            let host = b.hostname.clone().unwrap_or_else(|| pat.clone());
            let port = b.port.or(default_port).unwrap_or(22);
            let username = b
                .user
                .clone()
                .or_else(|| default_user.clone())
                .unwrap_or_default();
            let private_key_path = b.identity_file.clone().or_else(|| default_key.clone());
            let jump_raw = b.proxy_jump.as_deref().or(default_jump.as_deref());
            let (jump_host, jump_port, jump_username) =
                jump_raw.map(parse_proxy_jump).unwrap_or((None, None, None));

            result.push(SshConfigHost {
                alias: pat.clone(),
                host,
                port,
                username,
                private_key_path,
                jump_host,
                jump_port,
                jump_username,
            });
        }
    }

    result
}

/// 读取当前用户 `~/.ssh/config` 并解析全部可用主机
pub fn discover_local_ssh_config() -> AppResult<Vec<SshConfigHost>> {
    let Some(home) = dirs::home_dir() else {
        return Ok(Vec::new());
    };
    let config_path = home.join(".ssh").join("config");
    if !config_path.is_file() {
        return Ok(Vec::new());
    }
    let content = std::fs::read_to_string(&config_path)?;
    Ok(parse_ssh_config_str(&content, Some(&home)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn test_parse_ssh_config_full_features() {
        let sample = r#"
# 全局默认配置
Host *
    User ubuntu
    Port 2222
    IdentityFile ~/.ssh/id_ed25519

Host prod-web
    HostName 10.0.1.50
    User deploy # 行尾注释
    Port=22
    ProxyJump ops@bastion.internal:2200

Host db-node
    HostName 10.0.1.80
    IdentityFile "~/.ssh/db_key"

Host *.dev.local
    User tester
"#;
        let home = PathBuf::from("/home/alice");
        let hosts = parse_ssh_config_str(sample, Some(&home));
        assert_eq!(hosts.len(), 2);

        assert_eq!(hosts[0].alias, "prod-web");
        assert_eq!(hosts[0].host, "10.0.1.50");
        assert_eq!(hosts[0].username, "deploy");
        assert_eq!(hosts[0].port, 22);
        assert_eq!(hosts[0].jump_host.as_deref(), Some("bastion.internal"));
        assert_eq!(hosts[0].jump_port, Some(2200));
        assert_eq!(hosts[0].jump_username.as_deref(), Some("ops"));
        assert!(hosts[0]
            .private_key_path
            .as_deref()
            .unwrap()
            .ends_with("id_ed25519"));

        assert_eq!(hosts[1].alias, "db-node");
        assert_eq!(hosts[1].host, "10.0.1.80");
        assert_eq!(hosts[1].username, "ubuntu");
        assert_eq!(hosts[1].port, 2222);
        assert!(hosts[1]
            .private_key_path
            .as_deref()
            .unwrap()
            .ends_with("db_key"));
    }
}
