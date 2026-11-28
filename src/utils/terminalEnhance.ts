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

/**
 * 仅对普通文本片段应用关键词着色，完整保留所有既有 ANSI 转义序列。
 * 提示：调用方应在进入备用屏幕（nvim / htop / tmux 全屏）时跳过本函数，确保 TUI 零失真。
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
    let seg = parts[i];
    if (!seg) continue;
    for (const { re, code } of compiled) {
      seg = seg.replace(re, (m) => `${code}${m}\x1b[0m`);
    }
    parts[i] = seg;
  }
  return parts.join("");
}
