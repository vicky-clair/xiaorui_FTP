# 更新日志（CHANGELOG）

所有重要变更均记录于此文件。

## [1.0.0] - 2026-11-28

### SSH 终端进阶能力与体验升级（第一/二/三梯队落地）
- **Windows 原生剪贴板修复**：接入 `arboard` 操作系统原生剪贴板 IPC，彻底解决 Windows WebView2 下无法复制/粘贴的问题；支持 `Ctrl+C/V`、`Ctrl+Shift+C/V`、`Ctrl/Shift+Insert`、左键松手复制（`mouseup`）与右键复制/粘贴。
- **`zsh + tmux + nvim` 专项优化**：透传 `COLORTERM=truecolor`，接入 `@xterm/addon-unicode11`（宽字符对齐）与 `@xterm/addon-webgl`（GPU 加速渲染），新增 Nerd Font 字体选择器、`Ctrl+Shift+F` 搜索及一键 **Vim/tmux 键位模式**。
- **第二梯队进阶运维能力**：
  - **终端 ↔ SFTP 双向目录跟随**：解析远端 Shell `OSC 7` 目录上报并自动同步 SFTP 面板；
  - **SSH 密钥管理器 & 纯 SFTP 一键部署公钥**：扫描 `~/.ssh`、原生生成 Ed25519 密钥对（可选 Passphrase，Unix `0o600`），通过纯 SFTP 幂等写入远端 `authorized_keys`（零 Shell 注入）；
  - **命令历史模糊检索（`Ctrl+Shift+H`）与终端关键词高亮**：历史自动脱敏跳过密码/Token；高亮引擎在 `nvim`/`tmux` 备用屏幕下自动暂停防失真；
  - **一键导入 `~/.ssh/config`**：支持 `Host *` 默认项、`IdentityFile` 展开与 `ProxyJump` 跳板机勾选导入。
- **第三梯队安全防呆与本地终端**：
  - **高危命令防呆拦截（Command Guard）**：提交前拦截 `rm -rf /`、`mkfs`、`dd of=/dev/`、`shutdown`/`reboot`、`DROP DATABASE` 等高危指令并弹窗确认；
  - **原生本地终端标签（`+本地`）**：基于 `portable-pty`（Windows ConPTY / Unix PTY）按白名单启动本地 PowerShell / Git Bash / CMD / Unix Shell。

### 安全加固（P0 / P1 / P2 全量完成）
- **SSH Host Key 零信任与 Fail Secure**：
  - 首次连接未知 SSH 主机（含跳板机）改为弹窗展示公钥算法与 SHA-256 指纹，用户明确确认后方可信任；
  - 主机指纹变更升级为高危阻断性弹窗，禁止静默覆盖；
  - `known_hosts.json` 解析损坏时按 Fail Secure 直接拒绝连接，不再静默回退为空表。
- **路径穿越、命令注入与文件传输原子提交**：
  - 新增 `src-tauri/src/security.rs` 统一防御远程路径控制字符（防 FTP `SITE CHMOD` CRLF 注入与 Shell 注入）、Windows 保留设备名（`CON`/`NUL`/`COM1` 等）、NTFS ADS 冒号、盘符覆盖及本地符号链接覆写；
  - 四大协议下载均改为 `.part` 同目录暂存并原子提交，异常中断或取消自动清理残缺文件；
  - `SftpClient::remove` 在后端通过 `symlink_metadata` 核实真实类型，杜绝误删软链接目标目录；
  - 拦截针对根目录、用户主目录及核心系统目录的本地危险删除；
  - 终端 `cd` 目录同步与路径插入严格转义并拦截控制字符与危险元字符。
- **凭据内存安全与系统凭据库健壮性**：
  - 为 `SiteInput`、`JumpHostConfig`、`S3Client`、`WebDavClient` 补充 `zeroize` 内存清零与脱敏 `Debug`；
  - 升级 `load_secrets_strict`，系统凭据库不可用时明确报错提示，绝不静默吞错或回退明文；
  - Unix 平台 `sites.json`、`known_hosts.json`、`settings.json`、`audit.log.jsonl` 强制 `0o600` 权限。
- **Tauri 2 最小权限隔离与终端防护**：
  - 拆分主窗口（`main`）与独立终端窗口（`term-*`）Capability，移除未使用的 `tauri-plugin-opener` 与冗余 `ssh_exec_command` IPC；
  - 后端 IPC 强制校验主窗口来源、独立终端会话绑定与 PTY 归属，并限制导出文件扩展名白名单与 10 MB 上限；
  - 会话断开时自动取消关联传输任务并关闭绑定独立终端窗口；
  - `xterm.js` 显式拦截 OSC 52 剪贴板序列与 OSC 8 隐藏链接，新增多行危险粘贴二次确认及后端 PTY 1 MB/s 刷屏限流；
  - 明文连接与跳过 TLS 校验会话在底部状态栏持续显示高危警示徽章。
- **本地脱敏安全审计日志**：
  - 新增 `src-tauri/src/audit.rs`（2 MB 自动轮转、防日志换行注入）与前端「安全审计日志」查看/导出弹窗。
- **供应链安全、测试与治理文档**：
  - 新增前端 Vitest 安全单测、`src-tauri/deny.toml`、`.github/dependabot.yml`、`.github/workflows/security-scan.yml`（RustSec `cargo-audit` / `cargo-deny` / Gitleaks / CodeQL / CycloneDX SBOM）；
  - 补齐 `SECURITY.md`、`LICENSE`、`docs/security/THREAT_MODEL.md`。

### 核心功能
- 多协议原生引擎：SFTP（含 ProxyJump 跳板机与 SSH Agent/Pageant）、FTPS/FTP、S3 兼容对象存储、WebDAV/HTTPS；
- Xshell 风格交互式 SSH PTY 终端（支持远端默认 `zsh`/`bash`/`PowerShell`、多标签、停靠/放大/独立原生窗口、搜索、缩放、主题、快捷命令、`.log` 导出与 SFTP 双向联动）；
- 现代双栏桌面 UI、远端硬件规格/占用监测、多磁盘可视化切换、系统托盘最小化与并发传输队列。
