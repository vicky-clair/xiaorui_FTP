/**
 * SSH 终端进阶增强纯函数模块（与 DOM 解耦，便于 Vitest 自动化测试）
 *
 * 包含：
 * 1. 高危命令防呆拦截器（Command Guard）
 * 2. 敏感命令历史脱敏过滤（绝不记录密码、Token 等凭据）
 * 3. OSC 7 远端工作目录解析（驱动 SFTP 文件面板自动跟随终端）
 * 4. ANSI 安全的终端关键词高亮引擎（跳过转义序列，绝不破坏颜色/控制码）
 */

import { containsControlChars } from "./security";

export interface DangerousCommandMatch {
  rule: string;
  reason: string;
}

const DANGEROUS_PATTERNS: { rule: string; reason: string; regex: RegExp }[] = [
  {
    rule: "rm-rf-critical",
    reason: "检测到递归强制删除根目录、主目录或通配路径的极高危指令",
    regex:
      /(^|[;&|]\s*)(sudo\s+)?rm\s+-[a-zA-Z]*[rf][a-zA-Z]*[rf][a-zA-Z]*\s+(--no-preserve-root\s+)?(\/(\s|\*|$)|~(\/|\s|$)|\*(\s|$)|\.\.?\/?(\s|$))/i,
  },
  {
    rule: "disk-format",
    reason: "检测到磁盘格式化或分区表破坏指令（mkfs / wipefs / fdisk / parted）",
    regex: /(^|[;&|]\s*)(sudo\s+)?(mkfs(\.[a-z0-9]+)?|wipefs|fdisk|parted)\b/i,
  },
  {
    rule: "dd-block-device",
    reason: "检测到使用 dd 直接覆盖底层磁盘块设备的高危指令",
    regex: /\bdd\b[^\n]*\bof=\/dev\/(sd|vd|nvme|hd|xvd|mmcblk|disk)/i,
  },
  {
    rule: "power-control",
    reason: "检测到远程关机或重启服务器指令（shutdown / reboot / poweroff / halt）",
    regex: /(^|[;&|]\s*)(sudo\s+)?(shutdown|reboot|poweroff|halt|init\s+[06])\b/i,
  },
  {
    rule: "root-perm-destroy",
    reason: "检测到递归修改根目录权限/所有者的极高危指令",
    regex: /(^|[;&|]\s*)(sudo\s+)?(chmod|chown)\s+[^\n]*-R[^\n]*\s+\/(\s|$)/i,
  },
  {
    rule: "fork-bomb",
    reason: "检测到 Fork 炸弹（将瞬间耗尽服务器进程资源）",
    regex: /:\(\)\s*\{\s*:\s*\|\s*:\s*&\s*\}\s*;\s*:/,
  },
  {
    rule: "sql-destructive",
    reason: "检测到不可逆数据库删库/删表/清表语句（DROP / TRUNCATE）",
    regex: /\b(drop\s+(database|schema|table)|truncate\s+table)\b/i,
  },
];

/** 检测待执行命令是否命中高危防呆规则；命中时返回规则说明供二次确认 */
export function matchDangerousCommand(command: string): DangerousCommandMatch | null {
  const trimmed = command.trim();
  if (!trimmed) return null;
  for (const item of DANGEROUS_PATTERNS) {
    if (item.regex.test(trimmed)) {
      return { rule: item.rule, reason: item.reason };
    }
  }
  return null;
}

/** 判断命令是否可能包含密码、API Key、Token 等敏感内容（命中时禁止记入本地命令历史） */
export function isSensitiveCommand(command: string): boolean {
  const trimmed = command.trim();
  if (trimmed.length < 2) return true;
  const sensitivePatterns = [
    /\b(passwd|chpasswd|sshpass)\b/i,
    /\b(password|passwd|secret|token|api[_-]?key|access[_-]?key)\s*[:=]/i,
    /\bexport\s+[A-Z0-9_]*(KEY|SECRET|TOKEN|PASS|PWD)[A-Z0-9_]*=/i,
    /\b(mysql|mariadb|psql)\b[^\n]*\s-p\S+/i,
    /authorization:\s*(bearer|basic)\s+/i,
  ];
  return sensitivePatterns.some((re) => re.test(trimmed));
}

/** 判断终端当前光标所在可见行是否处于密码/口令输入提示（命中时禁止记录到本地命令历史） */
export function isPasswordPromptLine(line: string): boolean {
  const trimmed = line.trimEnd();
  if (!trimmed) return false;
  return /(?:password|passphrase|passcode|pin\s*code|otp|verification\s+code|secret|密码|口令)[^\n]{0,60}[:：?？]\s*$/i.test(
    trimmed
  );
}

/**
 * 构造发送给 PTY 的粘贴文本：
 * - 当远端 Shell / nvim 开启 Bracketed Paste Mode (`\x1b[?2004h`) 时，先剔除剪贴板中可能夹带的
 *   `\x1b[200~` / `\x1b[201~` 控制序列（防范跳出粘贴边界注入命令），再以标准括号粘贴序列包裹；
 * - 未开启时将 CRLF/LF 规范为回车 `\r`，兼容老式 Shell 与 Windows CMD。
 */
export function encodeTerminalPaste(text: string, bracketedPasteMode: boolean): string {
  const normalized = text.replace(/\r\n/g, "\r").replace(/\n/g, "\r");
  if (!bracketedPasteMode) {
    return normalized;
  }
  // eslint-disable-next-line no-control-regex
  const sanitized = normalized.replace(/\x1b\[20[01]~/g, "");
  return `\x1b[200~${sanitized}\x1b[201~`;
}

/**
 * 解析终端发出的 OSC 7 目录上报载荷（如 `file://debian-host/var/www/my%20app`），
 * 返回规范化的远端 POSIX 路径；非法或包含控制字符时返回 null。
 */
export function parseOsc7Path(payload: string): string | null {
  const trimmed = payload.trim();
  if (!trimmed.toLowerCase().startsWith("file://")) return null;
  const afterScheme = trimmed.slice("file://".length);
  const slashIdx = afterScheme.indexOf("/");
  if (slashIdx < 0) return null;
  const rawPath = afterScheme.slice(slashIdx);
  let decoded: string;
  try {
    decoded = decodeURIComponent(rawPath);
  } catch {
    decoded = rawPath;
  }
  if (!decoded || containsControlChars(decoded)) return null;
  // 去掉尾部多余斜杠（根目录 `/` 除外）
  const normalized =
    decoded.length > 1 ? decoded.replace(/\/+$/, "") || "/" : decoded;
  return normalized.startsWith("/") ? normalized : `/${normalized}`;
}

/**
 * 安全解析远端终端发出的 OSC 52 剪贴板序列：
 * - 严格禁止 OSC 52 读取请求（`?`），绝不允许远端读取本地剪贴板；
 * - 仅允许合法的 Base64 写入请求（如 tmux 划选复制、nvim yank），单次上限 1 MB，完整支持 UTF-8 中文。
 */
export function parseOsc52Write(payload: string): string | null {
  const sep = payload.indexOf(";");
  if (sep < 0) return null;
  const b64 = payload.slice(sep + 1).trim();
  // 严格拦截 OSC 52 剪贴板读取查询（`?`）或空载荷或超大载荷
  if (!b64 || b64 === "?" || b64.length > 1_400_000) {
    return null;
  }
  try {
    const bin = atob(b64);
    const bytes = new Uint8Array(bin.length);
    for (let i = 0; i < bin.length; i++) {
      bytes[i] = bin.charCodeAt(i);
    }
    const text = new TextDecoder("utf-8").decode(bytes);
    return text.length > 0 ? text : null;
  } catch {
    return null;
  }
}

export interface BatchPreset {
  id: string;
  label: string;
  cmd: string;
}

export type TerminalOsKind = "linux" | "windows_ps" | "windows_cmd";

/** 根据终端类型、远端系统家族与本地 Shell 预设精确判定当前终端操作系统与命令方言 */
export function detectTerminalOs(
  mode: "ssh" | "local",
  remoteOsFamily?: string | null,
  localProfile?: string | null
): TerminalOsKind {
  if (mode === "ssh") {
    return remoteOsFamily?.toLowerCase() === "windows" ? "windows_ps" : "linux";
  }
  const prof = (localProfile || "").toLowerCase();
  if (prof === "cmd") return "windows_cmd";
  if (prof === "wsl" || prof === "gitbash" || prof === "zsh" || prof === "bash") return "linux";
  return "windows_ps";
}

/** 按操作系统返回差异化的批量执行指令预设（Linux / Windows PowerShell / Windows CMD 互不混用） */
export function getBatchPresets(osKind: TerminalOsKind): BatchPreset[] {
  if (osKind === "windows_ps") {
    return [
      { id: "wp1", label: "系统概览", cmd: "Get-CimInstance Win32_OperatingSystem | Select Caption, Version, LastBootUpTime" },
      { id: "wp2", label: "磁盘空间", cmd: "Get-PSDrive -PSProvider FileSystem | Format-Table Name, @{N='Used(GB)';E={[math]::Round($_.Used/1GB,1)}}, @{N='Free(GB)';E={[math]::Round($_.Free/1GB,1)}}" },
      { id: "wp3", label: "高占用进程", cmd: "Get-Process | Sort-Object CPU -Descending | Select-Object -First 12 Name, Id, CPU, WorkingSet" },
      { id: "wp4", label: "监听端口", cmd: "Get-NetTCPConnection -State Listen | Select-Object LocalAddress, LocalPort, OwningProcess | Sort-Object LocalPort" },
      { id: "wp5", label: "网络配置", cmd: "Get-NetIPAddress -AddressFamily IPv4 | Where-Object { $_.IPAddress -notlike '127.*' } | Format-Table InterfaceAlias, IPAddress" },
    ];
  }
  if (osKind === "windows_cmd") {
    return [
      { id: "wc1", label: "系统信息", cmd: "systeminfo | findstr /B /C:\"OS\" /C:\"System Boot\"" },
      { id: "wc2", label: "监听端口", cmd: "netstat -ano | findstr LISTENING" },
      { id: "wc3", label: "进程列表", cmd: "tasklist /FO TABLE" },
      { id: "wc4", label: "网络配置", cmd: "ipconfig /all" },
      { id: "wc5", label: "当前目录", cmd: "dir" },
    ];
  }
  return [
    { id: "lx1", label: "负载与内存", cmd: "uptime && free -h" },
    { id: "lx2", label: "磁盘与挂载", cmd: "df -hT" },
    { id: "lx3", label: "高占用进程", cmd: "ps aux --sort=-%cpu | head -n 12" },
    { id: "lx4", label: "监听端口", cmd: "ss -tulpn" },
    { id: "lx5", label: "Docker 容器", cmd: "docker ps -a" },
    { id: "lx6", label: "系统日志", cmd: "journalctl -n 25 --no-pager" },
  ];
}

/** 将多行批执行命令规范为对应平台 PTY 可执行的命令行序列 */
export function formatBatchScriptForOs(script: string, osKind: TerminalOsKind): string {
  const lines = script
    .split(/\r?\n/)
    .map((l) => l.trim())
    .filter((l) => l.length > 0 && !l.startsWith("#") && !(osKind === "windows_cmd" && l.toUpperCase().startsWith("REM ")));
  if (lines.length === 0) return "";
  return `${lines.join("\r")}\r`;
}

/** 生成与操作系统匹配的目录切换命令（拒绝控制字符注入） */
export function buildOsCdCommand(path: string, osKind: TerminalOsKind): string | null {
  if (!path || containsControlChars(path)) return null;
  if (osKind === "linux") {
    const escaped = path.replace(/'/g, "'\\''");
    return `cd '${escaped}'\r`;
  }
  // 将 SFTP 风格 `/D:/foo` 转为 Windows 路径 `D:\foo`
  let winPath = path;
  if (/^\/[A-Za-z]:/.test(winPath)) {
    winPath = winPath.slice(1);
  }
  winPath = winPath.replace(/\//g, "\\");
  if (osKind === "windows_cmd") {
    if (winPath.includes('"') || /[&|<>^%!]/.test(winPath)) return null;
    return `cd /d "${winPath}"\r`;
  }
  const escapedPs = winPath.replace(/'/g, "''");
  return `Set-Location -LiteralPath '${escapedPs}'\r`;
}

export interface RecordingFrame {
  time: number;
  text: string;
}

/** 将录制帧列表封装为标准 asciinema v2 (`.cast`) JSONL 文本 */
export function buildAsciinemaV2(
  cols: number,
  rows: number,
  title: string,
  frames: RecordingFrame[]
): string {
  const header = JSON.stringify({
    version: 2,
    width: Math.max(40, cols),
    height: Math.max(10, rows),
    timestamp: Math.floor(Date.now() / 1000),
    title,
    env: { TERM: "xterm-256color" },
  });
  const body = frames.map((f) => JSON.stringify([Number(f.time.toFixed(4)), "o", f.text]));
  return [header, ...body].join("\n") + "\n";
}

export interface HighlightRule {
  id: string;
  label: string;
  pattern: string;
  ansiCode: string;
  enabled: boolean;
}

export const DEFAULT_HIGHLIGHT_RULES: HighlightRule[] = [
  {
    id: "hl-err",
    label: "错误/异常",
    pattern: "\\b(?:ERROR|FATAL|CRITICAL|PANIC|FAILED|Exception|Traceback)\\b",
    ansiCode: "\x1b[1;31m",
    enabled: true,
  },
  {
    id: "hl-warn",
    label: "警告",
    pattern: "\\b(?:WARN|WARNING|DEPRECATED)\\b",
    ansiCode: "\x1b[1;33m",
    enabled: true,
  },
  {
    id: "hl-ok",
    label: "成功状态",
    pattern: "\\b(?:SUCCESS|PASSED|READY|ACTIVE|RUNNING)\\b",
    ansiCode: "\x1b[1;32m",
    enabled: true,
  },
  {
    id: "hl-ip",
    label: "IPv4 地址",
    pattern: "\\b(?:(?:25[0-5]|2[0-4]\\d|1\\d\\d|[1-9]?\\d)\\.){3}(?:25[0-5]|2[0-4]\\d|1\\d\\d|[1-9]?\\d)\\b",
    ansiCode: "\x1b[36m",
    enabled: true,
  },
];

/** ANSI 转义序列匹配（CSI / OSC / 单字节 ESC），用于在高亮时原样跳过避免破坏终端渲染 */
// eslint-disable-next-line no-control-regex
const ANSI_TOKEN_RE = /(\x1b\[[0-9;?]*[ -/]*[@-~]|\x1b\][^\x07\x1b]*(?:\x07|\x1b\\)|\x1b[@-Z\\-_])/g;

/** 检测文本片中是否存在被 TCP/SSH 分包截断的残缺 ANSI 转义序列 */
export function hasIncompleteAnsiTail(text: string): boolean {
  if (!text.includes("\x1b")) return false;
  const parts = text.split(ANSI_TOKEN_RE);
  for (let i = 0; i < parts.length; i += 2) {
    if (parts[i]?.includes("\x1b")) {
      return true;
    }
  }
  return false;
}

/**
 * 仅对普通文本片段应用关键词着色，完整保留所有既有 ANSI 转义序列：
 * - 遇到跨包截断的残缺转义序列时原样直出，杜绝损坏控制码；
 * - 收尾仅使用 `\x1b[39;22m` 还原前景色与粗体，绝不冲掉 Powerlevel10k 等提示符背景色块；
 * - 调用方应在进入备用屏幕（nvim / htop / tmux 全屏）时跳过本函数，确保 TUI 零失真。
 */
export function highlightTerminalText(text: string, rules: HighlightRule[]): string {
  const compiled: { re: RegExp; code: string }[] = [];
  for (const r of rules) {
    if (!r.enabled || !r.pattern.trim()) continue;
    try {
      compiled.push({ re: new RegExp(r.pattern, "g"), code: r.ansiCode });
    } catch {
      // 忽略用户误填的非法正则，绝不抛错中断终端渲染
    }
  }
  if (compiled.length === 0 || !text) return text;

  const parts = text.split(ANSI_TOKEN_RE);
  for (let i = 0; i < parts.length; i += 2) {
    if (parts[i]?.includes("\x1b")) {
      return text;
    }
  }
  for (let i = 0; i < parts.length; i += 2) {
    let seg = parts[i];
    if (!seg) continue;
    for (const { re, code } of compiled) {
      seg = seg.replace(re, (m) => `${code}${m}\x1b[39;22m`);
    }
    parts[i] = seg;
  }
  return parts.join("");
}
