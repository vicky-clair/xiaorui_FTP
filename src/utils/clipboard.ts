/**
 * 跨平台原生剪贴板工具模块
 *
 * 优先调用 Rust 后端基于 `arboard` 的操作系统原生剪贴板 IPC（直接走 Win32 / NSPasteboard / X11），
 * 彻底绕开 Windows WebView2 在 `http://tauri.localhost` 下的 `navigator.clipboard` 权限拦截；
 * 在非 Tauri 浏览器预览环境中自动降级回退到 Web Clipboard API。
 */

import { api } from "../api/tauri";

/** 将文本可靠写入系统剪贴板，返回是否成功 */
export async function writeClipboardText(text: string): Promise<boolean> {
  if (!text) return false;
  try {
    await api.clipboardWriteText(text);
    return true;
  } catch {
    try {
      await navigator.clipboard.writeText(text);
      return true;
    } catch {
      return false;
    }
  }
}

/** 从系统剪贴板可靠读取纯文本内容（若剪贴板为空或非文本内容，返回空字符串） */
export async function readClipboardText(): Promise<string> {
  try {
    return await api.clipboardReadText();
  } catch {
    try {
      return await navigator.clipboard.readText();
    } catch {
      return "";
    }
  }
}
