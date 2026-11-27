# xiaorui_FTP 威胁模型文档（THREAT_MODEL.md）

> 本文档对照《Secure Remote Client 项目开发总规范》第 3.4 节编制，记录 `xiaorui_FTP` 的核心资产、信任边界、攻击面、缓解措施、残余风险及验证手段。每当新增涉及认证、文件系统、网络协议或系统权限的能力时，需同步更新本文件。

---

## 1. 保护资产清单

| 资产 | 安全级别 | 持久化位置 | 保护方式 |
|---|---|---|---|
| SSH 登录密码 / 私钥 Passphrase | 极高 | OS 原生凭据库（`keyring`） | 绝不写入 `sites.json` 或日志；内存 `zeroize` 清零；脱敏 `Debug` |
| S3 Secret Key / STS 令牌 / WebDAV Token | 极高 | OS 原生凭据库（`keyring`） | 仅后端访问；`S3Client`/`WebDavClient` 释放时 `zeroize` 清零 |
| 本地 SSH Agent 访问能力 | 极高 | Windows OpenSSH 命名管道 / Pageant / `SSH_AUTH_SOCK` | 仅在用户主动发起 Agent 认证时由 Rust 后端调用，前端不可直接访问 |
| SSH 主机身份指纹库 | 高 | `<appDataDir>/known_hosts.json` | 首次显式核对确认、变更阻断、损坏 Fail Secure、Unix `0o600` |
| 站点连接元数据 | 中 | `<appDataDir>/sites.json` | 原子写入、Unix `0o600`、导出备份不含任何密钥 |
| 文件传输内容与本地目录 | 高 | 传输通道 / 本地文件系统 | 强加密传输（SSH/TLS）、`.part` 原子提交、严格路径与软链接校验 |
| 本地安全审计日志 | 高 | `<appDataDir>/audit.log.jsonl` | 严格脱敏、防换行伪造、2 MB 自动轮转、Unix `0o600` |
| 安装包与依赖供应链 | 极高 | Release 制品 / `Cargo.lock` / `package-lock.json` | SHA-256 校验、CI `cargo-audit`/`cargo-deny`/`npm audit`/CodeQL/Gitleaks/SBOM |

---

## 2. 信任边界

```text
+-----------------------------------------------------------------------+
| 本地操作系统（受信任基础：内核、OS 凭据库、文件系统权限）                 |
|  +-----------------------------------------------------------------+  |
|  | Rust 后端（核心安全执行边界）                                     |  |
|  | - SSH/SFTP/FTPS/S3/WebDAV 协议栈   - Keyring 凭据读写与 Zeroize  |  |
|  | - Host Key 严格核验                - 路径/软链接/窗口权限守卫    |  |
|  | - 脱敏审计日志                     - PTY 限流与生命周期管理      |  |
|  +---------------------------▲-------------------------------------+  |
|                              │ Tauri IPC（按窗口隔离 + 后端复核）      |
|  +---------------------------▼-------------------------------------+  |
|  | WebView 前端（不可信输入边界：主窗口 main / 独立终端 term-*）     |  |
|  | - 严格 CSP（禁用内联脚本与远程脚本）                              |  |
|  | - 不持久化敏感密钥，仅在表单提交时瞬态传递                         |  |
|  +-----------------------------------------------------------------+  |
+-----------------------------------------------------------------------+
                               │ 不可信网络（公网 / 内网）
                               ▼
                 +---------------------------+
                 | 远程服务器（默认不可信）    |
                 | - 主机公钥、目录项、文件名  |
                 | - 终端转义序列、响应报文    |
                 +---------------------------+
```

---

## 3. 攻击面、攻击路径与缓解措施

| 编号 | 攻击场景 / 威胁来源 | 风险等级 | 潜在攻击路径 | 已落地缓解措施 | 残余风险 |
|---|---|---|---|---|---|
| T1 | **中间人攻击（MITM）与 DNS 欺骗** | 极高 | 攻击者劫持 SSH 连接并伪造服务器公钥窃取密码或篡改传输文件 | `known_hosts.rs` 实现**首次连接弹窗显式核对算法与 SHA-256 指纹**、**变更高危阻断弹窗（禁止静默覆盖）**、**仓库损坏 Fail Secure 拒绝连接**；ProxyJump 跳板机与目标主机分别独立校验 | 用户若在首次弹窗未通过带外渠道核对指纹而盲目点击确认，仍可能遭受首次连接 MITM |
| T2 | **恶意 SFTP/FTP 服务端文件名路径穿越与本地覆盖** | 高 | 恶意服务端返回 `../../.ssh/authorized_keys`、`C:evil.dll`、`CON` 或利用本地已存在符号链接覆写敏感文件 | `security.rs` 统一实施 `sanitize_local_filename` + `resolve_safe_download_path` + `ensure_not_symlink_target`；下载先写同目录 `.part` 暂存完成后原子提交，异常自动清理 | 用户手动在系统「另存为」对话框主动选择覆盖已有普通文件属于预期行为 |
| T3 | **恶意远程目录/文件名触发终端或 FTP 命令注入** | 高 | 远程目录名含 `\r\n`、`"`、`$()`、反引号，诱导点击「在终端进入」/「插入路径」或执行 `SITE CHMOD` 注入命令 | 前端 `buildSafeCdCommand`/`buildSafeInsertPath` 拒绝所有控制字符与 Windows 元字符；后端 `validate_remote_path_no_control` 拦截所有协议路径中的 `\r`/`\n`/`\0`；移除冗余 `ssh_exec_command` IPC | 用户在交互终端内主动输入或明确确认执行的多行命令由用户自行负责 |
| T4 | **恶意服务器终端输出（OSC 52 / 刷屏 DoS / 多行粘贴劫持）** | 中-高 | 远端输出 OSC 52 读取本地剪贴板，或高频刷屏导致前端 OOM，或网页暗藏换行命令借剪贴板自动执行 | `xterm.js` 注册拦截 OSC 52 与 OSC 8 序列；后端 PTY 实施 16 KB 分片 + 1 MB/s 限流；剪贴板粘贴含换行符时弹出多行预览二次确认条 | 终端内部仍完整支持标准 ANSI 渲染，需保持 `@xterm/xterm` 及时升级 |
| T5 | **远程软链接递归删除逃逸** | 高 | 恶意或误传 `is_dir=true` 导致递归删除跟随远程目录软链接清空外部数据 | `SftpClient::remove` 在后端先用 `symlink_metadata`（lstat）核实真实类型，软链接一律仅删链接本身；禁止删除 `/` | FTP 协议依赖服务端 `LIST` 输出标识软链接 |
| T6 | **前端 WebView 越权调用本地写删或跨会话劫持** | 高 | 若前端被注入恶意脚本，试图通过 `term-*` 窗口删站点、删本地系统目录或覆写可执行文件 | 拆分 `main`/`term-*` Capability、移除 `opener` 插件；后端 `ensure_main_window` 与 `ensure_window_session_access` 强制隔离；`local_delete` 拦截根目录/主目录/系统目录；导出文件限制 `.json`/`.log`/`.txt` 白名单与 10 MB 上限 | 主窗口仍具备合法管理用户工作目录文件的能力 |
| T7 | **本地凭据泄露与内存残留** | 高 | 普通配置文件或日志中泄露明文密码，或凭据库不可用时降级明文保存 | 密钥仅存 OS Keyring；`load_secrets_strict` 在凭据库不可用时明确报错不降级；`SiteSecrets`/`SiteInput`/`JumpHostConfig`/`S3Client`/`WebDavClient` 实现 `zeroize` 清零与脱敏 `Debug`；Unix 私有文件 `0o600` | Rust 堆分配器/OS 交换分区及浏览器输入态在短时间内仍可能存在瞬态内存副本 |
| T8 | **明文传输与跳过证书校验风险** | 中 | 用户使用传统 FTP 或开启 `tls_skip_verify` 导致流量被窃听或劫持 | 默认优先 SFTP/FTPS；会话启用明文或跳过 TLS 校验时，底部状态栏持续显示高危警示徽章并写入 `Warn` 级审计日志 | 为兼容内网老旧设备仍保留可选入口 |
| T9 | **第三方依赖投毒与发布制品篡改** | 高 | 上游 npm/Cargo 包存在已知 CVE 或构建产物被篡改 | CI 集成 `npm audit`、`cargo-audit`、`cargo-deny`、Gitleaks、CodeQL、CycloneDX SBOM；本地发布脚本自动计算 SHA-256 | 尚未配置商业代码签名证书与自动验签更新器（计划后续阶段接入） |

---

## 4. 安全测试矩阵

| 测试领域 | 测试实现位置 | 执行状态 |
|---|---|---|
| Host Key 未知拒绝、显式信任、禁止静默覆盖、变更阻断、损坏 Fail Secure | `src-tauri/src/known_hosts.rs` | 已自动化（`cargo test`） |
| 远程路径 CRLF / NUL 注入拦截 | `src-tauri/src/security.rs` | 已自动化（`cargo test`） |
| 本地文件名穿越、盘符覆盖、NTFS ADS、Windows 保留设备名拦截 | `src-tauri/src/security.rs` | 已自动化（`cargo test`） |
| `.part` 临时文件原子提交与覆盖校验 | `src-tauri/src/security.rs` | 已自动化（`cargo test`） |
| 根目录 / 用户主目录 / 系统目录本地删除拦截 | `src-tauri/src/security.rs` | 已自动化（`cargo test`） |
| 审计日志写入、脱敏防换行伪造与倒序查询 | `src-tauri/src/audit.rs` | 已自动化（`cargo test`） |
| 按会话批量取消传输（断连清理） | `src-tauri/src/transfer.rs` | 已自动化（`cargo test`） |
| 前端 Host Key 报错解析、终端 `cd`/路径插入注入防御、多行粘贴检测 | `src/utils/security.test.ts` | 已自动化（`npm test`） |
| 模糊测试（Fuzz Testing）与三端真实 OpenSSH 集成回归 | 待后续专项补齐 | 未执行（已如实标注） |
