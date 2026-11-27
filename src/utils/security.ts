/**
 * 前端安全校验与转义工具模块
 *
 * 负责：
 * - 解析后端返回的结构化 SSH 主机公钥校验错误（未知主机首次确认 / 指纹变更阻断告警）；
 * - 构造安全的终端目录切换与路径插入文本，严格拦截换行符、NUL 及危险 Shell 控制字符；
 * - 检测剪贴板多行危险粘贴内容。
 */

import type { HostKeyMismatch, HostKeyUnknown } from "../types";

export type HostKeyPrompt =
  | { kind: "unknown"; info: HostKeyUnknown }
  | { kind: "mismatch"; info: HostKeyMismatch };

const UNKNOWN_PREFIX = "HOST_KEY_UNKNOWN::";
const MISMATCH_PREFIX = "HOST_KEY_MISMATCH::";

/** 从 Tauri IPC 错误字符串中解析结构化 SSH 主机公钥校验弹窗数据 */
export function parseHostKeyError(err: unknown): HostKeyPrompt | null {
  const raw = String(err ?? "");
  const unknownIdx = raw.indexOf(UNKNOWN_PREFIX);
  if (unknownIdx >= 0) {
    try {
      const json = raw.slice(unknownIdx + UNKNOWN_PREFIX.length).trim();
      const info = JSON.parse(json) as HostKeyUnknown;
      if (info && info.hostPort && info.fingerprint) {
        return { kind: "unknown", info };
      }
    } catch {
      return null;
    }
  }

  const mismatchIdx = raw.indexOf(MISMATCH_PREFIX);
  if (mismatchIdx >= 0) {
    try {
      const json = raw.slice(mismatchIdx + MISMATCH_PREFIX.length).trim();
      const info = JSON.parse(json) as HostKeyMismatch;
      if (info && info.hostPort && info.expectedFingerprint && info.actualFingerprint) {
        return { kind: "mismatch", info };
      }
    } catch {
      return null;
    }
  }

  return null;
}

/** 检测字符串中是否包含 ASCII 控制字符（含 `\r`、`\n`、`\0`、ESC 等） */
export function containsControlChars(input: string): boolean {
  for (let i = 0; i < input.length; i++) {
    const code = input.charCodeAt(i);
    if (code < 0x20 || code === 0x7f) {
      return true;
    }
  }
  return false;
}

/**
 * 构造安全的交互式终端 `cd` 指令，严格防范恶意远端目录名导致的命令注入：
 * - 拒绝包含 `\r`、`\n`、`\0` 或任意控制字符的路径；
 * - Windows 盘符路径严格拦截 `"`, `` ` ``, `$`, `%`, `&`, `|`, `;`, `<`, `>`, `^` 等命令元字符；
 * - POSIX 路径使用单引号完整包裹并安全转义单引号（`'` -> `'\''`）。
 */
export function buildSafeCdCommand(dirPath: string): string | null {
  if (!dirPath || containsControlChars(dirPath)) {
    return null;
  }
  const isWinDrive = /^\/[A-Za-z]:(\/|$)/.test(dirPath);
  if (isWinDrive) {
    const winPath = dirPath.slice(1).replace(/\//g, "\\");
    if (/["`$%&|;<>^!]/.test(winPath)) {
      return null;
    }
    return `cd "${winPath}"\r`;
  }
  return `cd '${dirPath.replace(/'/g, "'\\''")}'\r`;
}

/**
 * 构造安全的终端路径插入文本（绝不自动追加回车，严格拦截换行与控制字符）
 */
export function buildSafeInsertPath(filePath: string): string | null {
  if (!filePath || containsControlChars(filePath)) {
    return null;
  }
  const needsQuote = /\s|['"\\$`&;|<>()!*?[\]{}~#]/.test(filePath);
  return needsQuote ? `'${filePath.replace(/'/g, "'\\''")}' ` : `${filePath} `;
}

/** 检测待粘贴文本是否包含换行符（多行内容直接写入交互终端会立刻逐行执行命令） */
export function isDangerousMultilinePaste(text: string): boolean {
  return /[\r\n]/.test(text);
}
