/**
 * Xshell 风格交互式 SSH 终端工作台组件（基于 xterm.js + 后端真实 PTY）
 *
 * 核心能力：
 * - 直接启动远端默认登录 Shell（zsh / Oh-My-Zsh / bash / fish / PowerShell）
 * - 三种视图形态：底部停靠（可拖拽调高）、工作区全屏放大、弹出独立原生窗口
 * - 多标签 Shell 并行、Ctrl+F 终端搜索（支持大小写/正则）、字号实时缩放、多套配色主题
 * - 选中即复制（Xshell 经典体验）、终端缓冲区导出为 .log、自定义快捷命令栏
 * - 与本地/SFTP 双向联动：一键同步浏览目录、一键上传本地文件、接收来自文件面板的路径插入
 */

import React, { useCallback, useEffect, useImperativeHandle, useRef, useState } from "react";
import { FitAddon } from "@xterm/addon-fit";
import { SearchAddon } from "@xterm/addon-search";
import { Unicode11Addon } from "@xterm/addon-unicode11";
import { WebglAddon } from "@xterm/addon-webgl";
import { Terminal, type ITheme } from "@xterm/xterm";
import "@xterm/xterm/css/xterm.css";
import { open, save } from "@tauri-apps/plugin-dialog";
import {
  ChevronDown,
  ChevronUp,
  ClipboardPaste,
  Copy,
  ExternalLink,
  FileDown,
  FolderOpen,
  FolderSync,
  Maximize2,
  Minimize2,
  Plus,
  RotateCw,
  Search,
  Terminal as TermIcon,
  Trash2,
  Upload,
  X,
  Zap,
} from "lucide-react";
import { api } from "../api/tauri";
import { readClipboardText, writeClipboardText } from "../utils/clipboard";
import { buildSafeCdCommand, isDangerousMultilinePaste } from "../utils/security";

export interface TerminalCommandRequest {
  id: number;
  text: string;
}

interface TerminalDrawerProps {
  sessionId: string;
  sessionName: string;
  cwd: string;
  standalone?: boolean;
  maximized?: boolean;
  pendingCommand?: TerminalCommandRequest | null;
  onToggleMaximize?: () => void;
  onPopOut?: () => void;
  onNotify?: (text: string, kind?: "info" | "success" | "error") => void;
  onClose?: () => void;
}

interface QuickSnippet {
  id: string;
  label: string;
  cmd: string;
}

interface TabMeta {
  id: string;
  title: string;
  status: "connecting" | "connected" | "exited" | "error";
  sizeLabel: string;
  reconnectSeq: number;
}

interface PtyTabHandle {
  writeInput: (text: string) => Promise<boolean>;
  clear: () => void;
  focus: () => void;
  fit: () => void;
  findNext: (term: string, opts?: { caseSensitive?: boolean; regex?: boolean }) => boolean;
  findPrev: (term: string, opts?: { caseSensitive?: boolean; regex?: boolean }) => boolean;
  clearSearch: () => void;
  dumpBuffer: () => string;
}

type ThemeKey = "github_dark" | "one_dark" | "monokai" | "solarized_light" | "matrix";

const TERMINAL_THEMES: Record<ThemeKey, { label: string; bg: string; theme: ITheme }> = {
  github_dark: {
    label: "GitHub Dark",
    bg: "#0d1117",
    theme: {
      background: "#0d1117",
      foreground: "#e6edf3",
      cursor: "#58a6ff",
      cursorAccent: "#0d1117",
      selectionBackground: "rgba(56, 139, 253, 0.35)",
      black: "#484f58",
      red: "#ff7b72",
      green: "#3fb950",
      yellow: "#d29922",
      blue: "#58a6ff",
      magenta: "#bc8cff",
      cyan: "#39c5cf",
      white: "#b1bac4",
      brightBlack: "#6e7681",
      brightRed: "#ffa198",
      brightGreen: "#56d364",
      brightYellow: "#e3b341",
      brightBlue: "#79c0ff",
      brightMagenta: "#d2a8ff",
      brightCyan: "#56d4dd",
      brightWhite: "#f0f6fc",
    },
  },
  one_dark: {
    label: "One Dark",
    bg: "#1e2127",
    theme: {
      background: "#1e2127",
      foreground: "#abb2bf",
      cursor: "#528bff",
      selectionBackground: "rgba(82, 139, 255, 0.3)",
      black: "#1e2127",
      red: "#e06c75",
      green: "#98c379",
      yellow: "#d19a66",
      blue: "#61afef",
      magenta: "#c678dd",
      cyan: "#56b6c2",
      white: "#abb2bf",
      brightBlack: "#5c6370",
      brightRed: "#e06c75",
      brightGreen: "#98c379",
      brightYellow: "#e5c07b",
      brightBlue: "#61afef",
      brightMagenta: "#c678dd",
      brightCyan: "#56b6c2",
      brightWhite: "#ffffff",
    },
  },
  monokai: {
    label: "Monokai",
    bg: "#1f201b",
    theme: {
      background: "#1f201b",
      foreground: "#f8f8f2",
      cursor: "#f8f8f0",
      selectionBackground: "rgba(255, 255, 255, 0.2)",
      black: "#272822",
      red: "#f92672",
      green: "#a6e22e",
      yellow: "#f4bf75",
      blue: "#66d9ef",
      magenta: "#ae81ff",
      cyan: "#a1efe4",
      white: "#f8f8f2",
      brightBlack: "#75715e",
      brightRed: "#f92672",
      brightGreen: "#a6e22e",
      brightYellow: "#e6db74",
      brightBlue: "#66d9ef",
      brightMagenta: "#ae81ff",
      brightCyan: "#a1efe4",
      brightWhite: "#f9f8f5",
    },
  },
  solarized_light: {
    label: "Solarized 浅色",
    bg: "#fdf6e3",
    theme: {
      background: "#fdf6e3",
      foreground: "#586e75",
      cursor: "#268bd2",
      selectionBackground: "rgba(38, 139, 210, 0.22)",
      black: "#073642",
      red: "#dc322f",
      green: "#859900",
      yellow: "#b58900",
      blue: "#268bd2",
      magenta: "#d33682",
      cyan: "#2aa198",
      white: "#eee8d5",
      brightBlack: "#657b83",
      brightRed: "#cb4b16",
      brightGreen: "#586e75",
      brightYellow: "#657b83",
      brightBlue: "#839496",
      brightMagenta: "#6c71c4",
      brightCyan: "#93a1a1",
      brightWhite: "#fdf6e3",
    },
  },
  matrix: {
    label: "极客绿",
    bg: "#080f0a",
    theme: {
      background: "#080f0a",
      foreground: "#4ade80",
      cursor: "#22c55e",
      selectionBackground: "rgba(34, 197, 94, 0.28)",
      black: "#0d1a10",
      red: "#f87171",
      green: "#22c55e",
      yellow: "#facc15",
      blue: "#38bdf8",
      magenta: "#c084fc",
      cyan: "#2dd4bf",
      white: "#bbf7d0",
    },
  },
};

const DEFAULT_SNIPPETS: QuickSnippet[] = [
  { id: "s1", label: "列目录", cmd: "ls -la" },
  { id: "s2", label: "磁盘空间", cmd: "df -h" },
  { id: "s3", label: "内存占用", cmd: "free -h" },
  { id: "s4", label: "进程 Top", cmd: "ps aux --sort=-%cpu | head -n 15" },
  { id: "s5", label: "监听端口", cmd: "ss -tulpn" },
  { id: "s6", label: "Docker", cmd: "docker ps" },
];

type FontKey = "auto_nf" | "jetbrains_nf" | "caskaydia_nf" | "meslo_nf" | "fira_nf" | "consolas";

const TERMINAL_FONTS: Record<FontKey, { label: string; family: string }> = {
  auto_nf: {
    label: "Nerd Font 自动",
    family:
      '"CaskaydiaCove Nerd Font", "JetBrainsMono Nerd Font", "MesloLGS NF", "Hack Nerd Font", "FiraCode Nerd Font", "Cascadia Code", "JetBrains Mono", Consolas, "PingFang SC", "Microsoft YaHei Mono", monospace',
  },
  jetbrains_nf: {
    label: "JetBrainsMono NF",
    family: '"JetBrainsMono Nerd Font", "JetBrains Mono", Consolas, "PingFang SC", monospace',
  },
  caskaydia_nf: {
    label: "CaskaydiaCove NF",
    family: '"CaskaydiaCove Nerd Font", "Cascadia Code", Consolas, "PingFang SC", monospace',
  },
  meslo_nf: {
    label: "MesloLGS NF",
    family: '"MesloLGS NF", "MesloLGS Nerd Font", Consolas, "PingFang SC", monospace',
  },
  fira_nf: {
    label: "FiraCode NF",
    family: '"FiraCode Nerd Font", "Fira Code", Consolas, "PingFang SC", monospace',
  },
  consolas: {
    label: "Consolas / 系统等宽",
    family: 'Consolas, "Cascadia Code", "PingFang SC", monospace',
  },
};

const STORAGE_SNIPPETS = "xiaorui_term_snippets_v1";
const STORAGE_FONT = "xiaorui_term_font_size";
const STORAGE_FONT_FAMILY = "xiaorui_term_font_family";
const STORAGE_THEME = "xiaorui_term_theme";
const STORAGE_COPY_SEL = "xiaorui_term_copy_on_select";
const STORAGE_VIM_MODE = "xiaorui_term_vim_mode";

function loadSnippets(): QuickSnippet[] {
  try {
    const raw = localStorage.getItem(STORAGE_SNIPPETS);
    if (!raw) return DEFAULT_SNIPPETS;
    const parsed = JSON.parse(raw) as QuickSnippet[];
    return Array.isArray(parsed) && parsed.length > 0 ? parsed : DEFAULT_SNIPPETS;
  } catch {
    return DEFAULT_SNIPPETS;
  }
}

/** 将后端推送的 Base64 字节流无损解码为 Uint8Array（防止中文 UTF-8 跨包截断乱码） */
function base64ToBytes(b64: string): Uint8Array {
  const bin = atob(b64);
  const out = new Uint8Array(bin.length);
  for (let i = 0; i < bin.length; i++) {
    out[i] = bin.charCodeAt(i);
  }
  return out;
}

/** 单个独立 PTY 标签页实例（隐藏时保持会话活跃不中断） */
const PtyTabPane = React.forwardRef<
  PtyTabHandle,
  {
    sessionId: string;
    active: boolean;
    reconnectSeq: number;
    fontSize: number;
    fontKey: FontKey;
    themeKey: ThemeKey;
    copyOnSelect: boolean;
    vimMode: boolean;
    onStatusChange: (status: TabMeta["status"], sizeLabel?: string) => void;
    onOpenSearch: () => void;
    onZoomDelta: (delta: number | "reset") => void;
    onRequestPaste: (text: string) => void;
    onNotify?: (text: string, kind?: "info" | "success" | "error") => void;
  }
>(
  (
    {
      sessionId,
      active,
      reconnectSeq,
      fontSize,
      fontKey,
      themeKey,
      copyOnSelect,
      vimMode,
      onStatusChange,
      onOpenSearch,
      onZoomDelta,
      onRequestPaste,
      onNotify,
    },
    ref
  ) => {
    const hostRef = useRef<HTMLDivElement>(null);
    const termRef = useRef<Terminal | null>(null);
    const fitRef = useRef<FitAddon | null>(null);
    const searchRef = useRef<SearchAddon | null>(null);
    const terminalIdRef = useRef<string | null>(null);
    const copyOnSelectRef = useRef(copyOnSelect);
    const vimModeRef = useRef(vimMode);
    const onOpenSearchRef = useRef(onOpenSearch);
    const onZoomDeltaRef = useRef(onZoomDelta);
    const onStatusChangeRef = useRef(onStatusChange);
    const onRequestPasteRef = useRef(onRequestPaste);
    const onNotifyRef = useRef(onNotify);

    useEffect(() => {
      copyOnSelectRef.current = copyOnSelect;
    }, [copyOnSelect]);
    useEffect(() => {
      vimModeRef.current = vimMode;
    }, [vimMode]);
    useEffect(() => {
      onOpenSearchRef.current = onOpenSearch;
    }, [onOpenSearch]);
    useEffect(() => {
      onZoomDeltaRef.current = onZoomDelta;
    }, [onZoomDelta]);
    useEffect(() => {
      onStatusChangeRef.current = onStatusChange;
    }, [onStatusChange]);
    useEffect(() => {
      onRequestPasteRef.current = onRequestPaste;
    }, [onRequestPaste]);
    useEffect(() => {
      onNotifyRef.current = onNotify;
    }, [onNotify]);

    const fitAndNotify = useCallback(() => {
      const term = termRef.current;
      const fitAddon = fitRef.current;
      if (!term || !fitAddon) return;
      try {
        fitAddon.fit();
        if (term.cols > 0 && term.rows > 0) {
          onStatusChangeRef.current(
            terminalIdRef.current ? "connected" : "connecting",
            `${term.cols}×${term.rows}`
          );
          if (terminalIdRef.current) {
            void api.sshPtyResize(terminalIdRef.current, term.cols, term.rows);
          }
        }
      } catch {
        // 忽略布局尚未稳定时的偶发尺寸计算异常
      }
    }, []);

    useImperativeHandle(
      ref,
      () => ({
        writeInput: async (text: string) => {
          const tid = terminalIdRef.current;
          if (!tid) return false;
          await api.sshPtyWrite(tid, text);
          termRef.current?.focus();
          return true;
        },
        clear: () => {
          termRef.current?.clear();
          termRef.current?.focus();
        },
        focus: () => {
          termRef.current?.focus();
        },
        fit: fitAndNotify,
        findNext: (q, opts) => searchRef.current?.findNext(q, opts) ?? false,
        findPrev: (q, opts) => searchRef.current?.findPrevious(q, opts) ?? false,
        clearSearch: () => searchRef.current?.clearDecorations(),
        dumpBuffer: () => {
          const term = termRef.current;
          if (!term) return "";
          const buf = term.buffer.active;
          const lines: string[] = [];
          for (let i = 0; i < buf.length; i++) {
            const line = buf.getLine(i);
            if (line) lines.push(line.translateToString(true));
          }
          return lines.join("\n").trimEnd() + "\n";
        },
      }),
      [fitAndNotify]
    );

    // 动态更新字号、字体与配色主题（无需重连 PTY）
    useEffect(() => {
      const term = termRef.current;
      if (!term) return;
      term.options.fontSize = fontSize;
      term.options.fontFamily = TERMINAL_FONTS[fontKey].family;
      term.options.theme = TERMINAL_THEMES[themeKey].theme;
      if (active) {
        requestAnimationFrame(() => fitAndNotify());
      }
    }, [fontSize, fontKey, themeKey, active, fitAndNotify]);

    // 切换为激活标签时自动重新适配窗口大小并聚焦
    useEffect(() => {
      if (!active) return;
      const t = window.setTimeout(() => {
        fitAndNotify();
        termRef.current?.focus();
      }, 30);
      return () => window.clearTimeout(t);
    }, [active, fitAndNotify]);

    // 建立并维护该标签页的真实 SSH PTY 会话
    useEffect(() => {
      const container = hostRef.current;
      if (!container) return;

      let disposed = false;
      let unlistenData: (() => void) | undefined;
      let unlistenExit: (() => void) | undefined;
      let resizeTimer: number | undefined;
      let webglAddon: WebglAddon | null = null;

      const term = new Terminal({
        cursorBlink: true,
        cursorStyle: "bar",
        fontFamily: TERMINAL_FONTS[fontKey].family,
        fontSize,
        lineHeight: 1.28,
        scrollback: 8000,
        // Unicode11Addon 需要开启 proposed API 以精准计算 Nerd Font/Powerline/Emoji/中文宽字符列宽
        allowProposedApi: true,
        theme: TERMINAL_THEMES[themeKey].theme,
      });

      const fitAddon = new FitAddon();
      const searchAddon = new SearchAddon();
      const unicode11Addon = new Unicode11Addon();
      term.loadAddon(fitAddon);
      term.loadAddon(searchAddon);
      term.loadAddon(unicode11Addon);
      term.unicode.activeVersion = "11";
      term.open(container);

      // 优先启用 WebGL GPU 加速渲染（显著提升 nvim / tmux / htop 滚动帧率），GPU 上下文丢失时优雅回退 DOM 渲染
      try {
        const gl = new WebglAddon();
        gl.onContextLoss(() => {
          gl.dispose();
          webglAddon = null;
        });
        term.loadAddon(gl);
        webglAddon = gl;
      } catch {
        webglAddon = null;
      }
      // 终端输出纵深防御：显式拦截恶意远端 OSC 52（剪贴板读写）与 OSC 8（隐藏超链接）控制序列
      const osc52Disp = term.parser.registerOscHandler(52, () => true);
      const osc8Disp = term.parser.registerOscHandler(8, () => true);
      termRef.current = term;
      fitRef.current = fitAddon;
      searchRef.current = searchAddon;

      // 原生剪贴板复制与粘贴辅助（直接经由 Rust arboard 读写系统剪贴板，彻底绕过 Windows WebView2 权限拦截）
      const copySelection = async (clearAfter: boolean, silent = false) => {
        const sel = term.getSelection();
        if (!sel) return false;
        const ok = await writeClipboardText(sel);
        if (ok) {
          if (clearAfter) term.clearSelection();
          if (!silent) onNotifyRef.current?.("已复制选中内容到剪贴板", "info");
        } else if (!silent) {
          onNotifyRef.current?.("写入系统剪贴板失败", "error");
        }
        return ok;
      };

      const pasteFromClipboard = async () => {
        const txt = await readClipboardText();
        if (txt) {
          onRequestPasteRef.current(txt);
        } else {
          onNotifyRef.current?.("剪贴板为空或不含可粘贴文本", "info");
        }
      };

      // Xshell 经典交互：鼠标划选松手（mouseup）时写入系统剪贴板一次，避免拖动过程中高频争抢 Windows 剪贴板锁
      const onMouseUpCopy = (ev: MouseEvent) => {
        if (ev.button !== 0 || !copyOnSelectRef.current) return;
        window.setTimeout(() => {
          if (disposed) return;
          const sel = term.getSelection();
          if (sel && sel.trim().length > 0) {
            void writeClipboardText(sel);
          }
        }, 0);
      };
      container.addEventListener("mouseup", onMouseUpCopy);

      // 捕获经由系统编辑菜单或输入法触发的原生 paste 事件，统一走多行安全确认通道且防重复写入
      const onNativePaste = (ev: ClipboardEvent) => {
        ev.preventDefault();
        ev.stopPropagation();
        const txt = ev.clipboardData?.getData("text/plain") ?? "";
        if (txt) {
          onRequestPasteRef.current(txt);
        } else {
          void pasteFromClipboard();
        }
      };
      container.addEventListener("paste", onNativePaste, true);

      // 键盘快捷键（兼容 Neovim / tmux / zsh）：
      // - Ctrl+Shift+F 打开终端搜索（释放 Ctrl+F 给 nvim 向下翻页与 zsh 光标右移）
      // - Ctrl+=/-/0 缩放字号
      // - 非 Vim 模式下：Ctrl+C（有选区复制/无选区发 ^C）、Ctrl+V 粘贴
      // - Vim 模式下：Ctrl+C / Ctrl+V 完整透传给 nvim/tmux，复制粘贴走 Ctrl+Shift+C/V、Ctrl/Shift+Insert 或右键
      term.attachCustomKeyEventHandler((ev) => {
        if (ev.type !== "keydown") return true;
        const key = ev.key.toLowerCase();
        const mod = ev.ctrlKey || ev.metaKey;

        if (mod && ev.shiftKey && key === "f") {
          ev.preventDefault();
          ev.stopPropagation();
          onOpenSearchRef.current();
          return false;
        }
        if (mod && !ev.shiftKey) {
          if (ev.key === "=" || ev.key === "+") {
            ev.preventDefault();
            onZoomDeltaRef.current(1);
            return false;
          }
          if (ev.key === "-") {
            ev.preventDefault();
            onZoomDeltaRef.current(-1);
            return false;
          }
          if (ev.key === "0") {
            ev.preventDefault();
            onZoomDeltaRef.current("reset");
            return false;
          }
          if (!vimModeRef.current && key === "c" && term.hasSelection()) {
            ev.preventDefault();
            ev.stopPropagation();
            void copySelection(true);
            return false;
          }
          if (!vimModeRef.current && key === "v") {
            ev.preventDefault();
            ev.stopPropagation();
            void pasteFromClipboard();
            return false;
          }
          if (ev.key === "Insert") {
            ev.preventDefault();
            ev.stopPropagation();
            void copySelection(false);
            return false;
          }
        }
        if (mod && ev.shiftKey) {
          if (key === "c") {
            ev.preventDefault();
            ev.stopPropagation();
            void copySelection(false);
            return false;
          }
          if (key === "v") {
            ev.preventDefault();
            ev.stopPropagation();
            void pasteFromClipboard();
            return false;
          }
        }
        if (!mod && ev.shiftKey && ev.key === "Insert") {
          ev.preventDefault();
          ev.stopPropagation();
          void pasteFromClipboard();
          return false;
        }
        return true;
      });

      const ro = new ResizeObserver(() => {
        if (resizeTimer) window.clearTimeout(resizeTimer);
        resizeTimer = window.setTimeout(() => {
          if (!disposed && container.offsetWidth > 0 && container.offsetHeight > 0) {
            fitAndNotify();
          }
        }, 40);
      });
      ro.observe(container);

      const dataDisp = term.onData((data) => {
        const tid = terminalIdRef.current;
        if (tid) void api.sshPtyWrite(tid, data);
      });

      onStatusChangeRef.current("connecting");

      (async () => {
        try {
          const pendingChunks: { tid: string; bytes: Uint8Array }[] = [];

          unlistenData = await api.onPtyData((ev) => {
            if (disposed) return;
            const bytes = base64ToBytes(ev.dataB64);
            if (!terminalIdRef.current) {
              pendingChunks.push({ tid: ev.terminalId, bytes });
              return;
            }
            if (ev.terminalId === terminalIdRef.current) {
              term.write(bytes);
            }
          });

          unlistenExit = await api.onPtyExit((ev) => {
            if (disposed) return;
            if (ev.terminalId === terminalIdRef.current) {
              terminalIdRef.current = null;
              onStatusChangeRef.current("exited");
              term.writeln(
                "\r\n\x1b[33m[远程 Shell 会话已结束，点击工具栏「重连」可重新启动]\x1b[0m"
              );
            }
          });

          if (disposed) return;

          try {
            fitAddon.fit();
          } catch {
            // 布局未就绪时回退使用默认行列值
          }
          const cols = Math.max(40, term.cols || 100);
          const rows = Math.max(8, term.rows || 20);

          const tid = await api.sshPtyOpen(sessionId, cols, rows);
          if (disposed) {
            void api.sshPtyClose(tid);
            return;
          }

          terminalIdRef.current = tid;
          onStatusChangeRef.current("connected", `${cols}×${rows}`);

          for (const item of pendingChunks) {
            if (item.tid === tid) term.write(item.bytes);
          }
          fitAndNotify();
          term.focus();
        } catch (err) {
          if (!disposed) {
            onStatusChangeRef.current("error");
            term.writeln(`\x1b[31m打开远程交互式终端失败：${String(err)}\x1b[0m`);
          }
        }
      })();

      return () => {
        disposed = true;
        if (resizeTimer) window.clearTimeout(resizeTimer);
        ro.disconnect();
        container.removeEventListener("mouseup", onMouseUpCopy);
        container.removeEventListener("paste", onNativePaste, true);
        osc52Disp.dispose();
        osc8Disp.dispose();
        dataDisp.dispose();
        if (unlistenData) unlistenData();
        if (unlistenExit) unlistenExit();
        const tid = terminalIdRef.current;
        terminalIdRef.current = null;
        if (tid) void api.sshPtyClose(tid);
        webglAddon?.dispose();
        webglAddon = null;
        term.dispose();
        termRef.current = null;
        fitRef.current = null;
        searchRef.current = null;
      };
      // eslint-disable-next-line react-hooks/exhaustive-deps
    }, [sessionId, reconnectSeq]);

    return (
      <div
        ref={hostRef}
        className="xterm-host"
        style={{
          display: active ? "block" : "none",
          background: TERMINAL_THEMES[themeKey].bg,
        }}
        onClick={() => termRef.current?.focus()}
        onContextMenu={(e) => {
          e.preventDefault();
          const sel = termRef.current?.getSelection();
          if (sel) {
            void writeClipboardText(sel).then((ok) => {
              if (ok) {
                termRef.current?.clearSelection();
                onNotifyRef.current?.("已复制选中内容到剪贴板", "info");
              } else {
                onNotifyRef.current?.("写入系统剪贴板失败", "error");
              }
            });
          } else {
            void readClipboardText().then((txt) => {
              if (txt) {
                onRequestPasteRef.current(txt);
              } else {
                onNotifyRef.current?.("剪贴板为空或不含可粘贴文本", "info");
              }
            });
          }
        }}
      />
    );
  }
);
PtyTabPane.displayName = "PtyTabPane";

export const TerminalDrawer: React.FC<TerminalDrawerProps> = ({
  sessionId,
  sessionName,
  cwd,
  standalone = false,
  maximized = false,
  pendingCommand,
  onToggleMaximize,
  onPopOut,
  onNotify,
  onClose,
}) => {
  const [tabs, setTabs] = useState<TabMeta[]>([
    { id: "tab-1", title: "Shell 1", status: "connecting", sizeLabel: "", reconnectSeq: 0 },
  ]);
  const [activeTabId, setActiveTabId] = useState("tab-1");
  const tabSeqRef = useRef(2);
  const tabHandlesRef = useRef<Map<string, PtyTabHandle>>(new Map());

  const [drawerHeight, setDrawerHeight] = useState(310);
  const [resizing, setResizing] = useState(false);

  // Xshell 偏好设置（字号、Nerd Font 字体、主题、选中即复制、Vim/tmux 键位模式、快捷命令栏）
  const [fontSize, setFontSize] = useState<number>(() => {
    const saved = Number(localStorage.getItem(STORAGE_FONT));
    return Number.isFinite(saved) && saved >= 10 && saved <= 24 ? saved : 13;
  });
  const [fontKey, setFontKey] = useState<FontKey>(() => {
    const saved = localStorage.getItem(STORAGE_FONT_FAMILY) as FontKey | null;
    return saved && saved in TERMINAL_FONTS ? saved : "auto_nf";
  });
  const [themeKey, setThemeKey] = useState<ThemeKey>(() => {
    const saved = localStorage.getItem(STORAGE_THEME) as ThemeKey | null;
    return saved && saved in TERMINAL_THEMES ? saved : "github_dark";
  });
  const [copyOnSelect, setCopyOnSelect] = useState<boolean>(() => {
    const saved = localStorage.getItem(STORAGE_COPY_SEL);
    return saved === null ? true : saved === "true";
  });
  const [vimMode, setVimMode] = useState<boolean>(() => {
    return localStorage.getItem(STORAGE_VIM_MODE) === "true";
  });

  // 搜索条状态
  const [searchOpen, setSearchOpen] = useState(false);
  const [searchQuery, setSearchQuery] = useState("");
  const [searchCase, setSearchCase] = useState(false);
  const [searchRegex, setSearchRegex] = useState(false);
  const searchInputRef = useRef<HTMLInputElement>(null);

  // 快捷命令栏状态与多行危险粘贴二次确认状态
  const [snippetsOpen, setSnippetsOpen] = useState(true);
  const [snippets, setSnippets] = useState<QuickSnippet[]>(loadSnippets);
  const [addingSnippet, setAddingSnippet] = useState(false);
  const [newLabel, setNewLabel] = useState("");
  const [newCmd, setNewCmd] = useState("");
  const [pendingPaste, setPendingPaste] = useState<string | null>(null);

  useEffect(() => {
    localStorage.setItem(STORAGE_FONT, String(fontSize));
  }, [fontSize]);
  useEffect(() => {
    localStorage.setItem(STORAGE_FONT_FAMILY, fontKey);
  }, [fontKey]);
  useEffect(() => {
    localStorage.setItem(STORAGE_THEME, themeKey);
  }, [themeKey]);
  useEffect(() => {
    localStorage.setItem(STORAGE_COPY_SEL, String(copyOnSelect));
  }, [copyOnSelect]);
  useEffect(() => {
    localStorage.setItem(STORAGE_VIM_MODE, String(vimMode));
  }, [vimMode]);
  useEffect(() => {
    localStorage.setItem(STORAGE_SNIPPETS, JSON.stringify(snippets));
  }, [snippets]);

  const activeTab = tabs.find((t) => t.id === activeTabId) ?? tabs[0];

  const getActiveHandle = useCallback(
    () => (activeTab ? tabHandlesRef.current.get(activeTab.id) : undefined),
    [activeTab]
  );

  // 处理来自主界面 FilePane 的外部指令（例如「在终端进入目录」「将路径插入终端」）
  const lastHandledCmdRef = useRef<number>(0);
  useEffect(() => {
    if (!pendingCommand || pendingCommand.id === lastHandledCmdRef.current) return;
    let attempts = 0;
    const trySend = async () => {
      const handle = getActiveHandle();
      if (handle && (await handle.writeInput(pendingCommand.text))) {
        lastHandledCmdRef.current = pendingCommand.id;
        return;
      }
      if (attempts < 15) {
        attempts += 1;
        window.setTimeout(() => void trySend(), 200);
      }
    };
    void trySend();
  }, [pendingCommand, getActiveHandle]);

  const handleZoom = useCallback((delta: number | "reset") => {
    setFontSize((cur) => {
      if (delta === "reset") return 13;
      return Math.min(24, Math.max(10, cur + delta));
    });
  }, []);

  const handleOpenSearch = useCallback(() => {
    setSearchOpen(true);
    requestAnimationFrame(() => {
      searchInputRef.current?.focus();
      searchInputRef.current?.select();
    });
  }, []);

  /** 粘贴安全守卫：单行文本直接写入，含换行符的多行命令先弹出内联确认条防隐藏命令注入 */
  const handleRequestPaste = useCallback(
    (text: string) => {
      if (!text) return;
      if (isDangerousMultilinePaste(text)) {
        setPendingPaste(text);
        return;
      }
      void getActiveHandle()?.writeInput(text);
    },
    [getActiveHandle]
  );

  /** 同步当前 SFTP 远程浏览目录到活跃 Shell（严格拦截非法控制字符与命令注入） */
  const handleSyncCwd = useCallback(async () => {
    if (!cwd) return;
    const cmd = buildSafeCdCommand(cwd);
    if (!cmd) {
      onNotify?.("当前远程目录包含非法控制字符或危险元字符，已阻止同步至终端", "error");
      return;
    }
    const ok = await getActiveHandle()?.writeInput(cmd);
    if (!ok) onNotify?.("当前终端尚未连接就绪", "error");
  }, [cwd, getActiveHandle, onNotify]);

  /** 类 Xshell rz / Xftp 体验：从本地选择文件直接 SFTP 上传至当前远程目录 */
  const handleUploadLocalFiles = async () => {
    try {
      const picked = await open({ multiple: true, title: `选择本地文件上传到远程 ${cwd}` });
      if (!picked) return;
      const paths = Array.isArray(picked) ? picked : [picked];
      const queued = await api.enqueueUploadPaths(sessionId, paths, cwd || "/");
      onNotify?.(`已通过 SFTP 加入 ${queued.length} 个上传任务到 ${cwd || "/"}`, "success");
    } catch (err) {
      onNotify?.(`上传失败：${String(err)}`, "error");
    }
  };

  /** 将当前终端缓冲区日志导出为本地 .log 文件 */
  const handleSaveLog = async () => {
    const content = getActiveHandle()?.dumpBuffer() ?? "";
    if (!content.trim()) {
      onNotify?.("当前终端暂无可导出的日志内容", "info");
      return;
    }
    try {
      const stamp = new Date().toISOString().replace(/[:.]/g, "-").slice(0, 19);
      const target = await save({
        defaultPath: `xiaorui_FTP-${sessionName.replace(/[^\w.-]+/g, "_")}-${stamp}.log`,
        filters: [{ name: "终端日志文件", extensions: ["log", "txt"] }],
        title: "导出终端会话日志",
      });
      if (!target) return;
      await api.saveTextFile(target, content);
      onNotify?.(`终端日志已保存至 ${target}`, "success");
    } catch (err) {
      onNotify?.(`保存日志失败：${String(err)}`, "error");
    }
  };

  /** 新增一个独立 Shell 标签页 */
  const handleAddTab = () => {
    const n = tabSeqRef.current++;
    const id = `tab-${n}`;
    setTabs((prev) => [
      ...prev,
      { id, title: `Shell ${n}`, status: "connecting", sizeLabel: "", reconnectSeq: 0 },
    ]);
    setActiveTabId(id);
  };

  const handleCloseTab = (id: string) => {
    setTabs((prev) => {
      if (prev.length <= 1) {
        onClose?.();
        return prev;
      }
      const next = prev.filter((t) => t.id !== id);
      if (activeTabId === id && next.length > 0) {
        setActiveTabId(next[next.length - 1].id);
      }
      return next;
    });
  };

  const handleAddSnippet = (e: React.FormEvent) => {
    e.preventDefault();
    if (!newLabel.trim() || !newCmd.trim()) return;
    setSnippets((prev) => [
      ...prev,
      { id: `u-${Date.now()}`, label: newLabel.trim(), cmd: newCmd.trim() },
    ]);
    setNewLabel("");
    setNewCmd("");
    setAddingSnippet(false);
  };

  /** 顶部拖拽调整停靠模式终端高度 */
  const handleStartResize = (e: React.MouseEvent) => {
    if (standalone || maximized) return;
    e.preventDefault();
    setResizing(true);
    const startY = e.clientY;
    const startHeight = drawerHeight;
    const onMove = (ev: MouseEvent) => {
      const delta = startY - ev.clientY;
      const maxH = Math.max(280, window.innerHeight - 160);
      setDrawerHeight(Math.min(maxH, Math.max(180, startHeight + delta)));
    };
    const onUp = () => {
      setResizing(false);
      window.removeEventListener("mousemove", onMove);
      window.removeEventListener("mouseup", onUp);
    };
    window.addEventListener("mousemove", onMove);
    window.addEventListener("mouseup", onUp);
  };

  const containerClass = standalone
    ? "terminal-drawer terminal-standalone"
    : maximized
      ? "terminal-drawer terminal-maximized"
      : "bottom-drawer terminal-drawer";

  return (
    <div
      className={containerClass}
      style={!standalone && !maximized ? { height: drawerHeight } : undefined}
    >
      {!standalone && !maximized && (
        <div
          className={`drawer-resizer ${resizing ? "dragging" : ""}`}
          onMouseDown={handleStartResize}
          title="拖动调节终端高度"
        />
      )}

      {/* 顶部主工具栏：多标签切换 + Xshell 工具组 + 缩放/放大/独立窗口 */}
      <div className="drawer-header terminal-toolbar">
        <div className="term-tabs">
          <TermIcon size={14} color="var(--accent)" style={{ flexShrink: 0 }} />
          <span className="term-session-name" title={`${sessionName} (${cwd})`}>
            {sessionName}
          </span>

          {tabs.map((tab) => {
            const isAct = tab.id === activeTab?.id;
            return (
              <div
                key={tab.id}
                className={`term-tab ${isAct ? "active" : ""}`}
                onClick={() => setActiveTabId(tab.id)}
              >
                <span
                  className={`status-dot ${tab.status === "connected" ? "online" : ""}`}
                  style={{ width: 6, height: 6 }}
                />
                <span>{tab.title}</span>
                {tabs.length > 1 && (
                  <button
                    type="button"
                    className="session-tab-close"
                    onClick={(e) => {
                      e.stopPropagation();
                      handleCloseTab(tab.id);
                    }}
                    title="关闭此终端标签"
                  >
                    <X size={11} />
                  </button>
                )}
              </div>
            );
          })}

          <button
            type="button"
            className="icon-btn"
            style={{ width: 24, height: 24 }}
            onClick={handleAddTab}
            title="新建同会话 SSH 终端标签"
          >
            <Plus size={13} />
          </button>
        </div>

        <div className="term-actions">
          {activeTab?.sizeLabel && (
            <span className="drawer-pill tabular-nums" title="当前伪终端行列尺寸">
              {activeTab.sizeLabel}
            </span>
          )}

          <button
            type="button"
            className="btn btn-sm btn-ghost"
            onClick={() => void handleSyncCwd()}
            disabled={activeTab?.status !== "connected"}
            title={`在终端执行 cd 同步到远程目录：${cwd}`}
          >
            <FolderSync size={12} />
            同步目录
          </button>

          <button
            type="button"
            className="btn btn-sm btn-ghost"
            onClick={() => void handleUploadLocalFiles()}
            title={`选择本地文件通过 SFTP 快速上传到 ${cwd}`}
          >
            <Upload size={12} />
            上传文件
          </button>

          <button
            type="button"
            className={`icon-btn ${searchOpen ? "active" : ""}`}
            style={{ width: 25, height: 25 }}
            onClick={() => (searchOpen ? setSearchOpen(false) : handleOpenSearch())}
            title="搜索终端日志 (Ctrl+Shift+F，已释放 Ctrl+F 给 nvim/zsh)"
          >
            <Search size={13} />
          </button>

          <button
            type="button"
            className={`btn btn-sm ${vimMode ? "btn-primary" : "btn-ghost"}`}
            style={{ height: 24, padding: "0 7px", fontSize: 11 }}
            onClick={() => {
              const next = !vimMode;
              setVimMode(next);
              onNotify?.(
                next
                  ? "已开启 Vim/tmux 键位模式：Ctrl+C/V 透传给远端，复制粘贴请用 Ctrl+Shift+C/V 或右键"
                  : "已切回标准模式：支持 Ctrl+C/V、Ctrl+Shift+C/V 与右键复制粘贴",
                "info"
              );
            }}
            title={
              vimMode
                ? "Vim/tmux 键位模式已开启：Ctrl+C/V 透传给 nvim/tmux（复制粘贴用 Ctrl+Shift+C/V 或右键）"
                : "点击开启 Vim/tmux 键位模式：将 Ctrl+C/V 完整透传给 nvim 列块模式"
            }
          >
            Vim
          </button>

          <button
            type="button"
            className="icon-btn"
            style={{ width: 25, height: 25 }}
            onClick={() => {
              void readClipboardText().then((txt) => {
                if (txt) {
                  handleRequestPaste(txt);
                } else {
                  onNotify?.("剪贴板为空或不含可粘贴文本", "info");
                }
              });
            }}
            title="从系统剪贴板粘贴到当前终端 (Ctrl+V / Ctrl+Shift+V / 右键)"
          >
            <ClipboardPaste size={13} />
          </button>

          <button
            type="button"
            className={`icon-btn ${copyOnSelect ? "active" : ""}`}
            style={{ width: 25, height: 25 }}
            onClick={() => setCopyOnSelect((v) => !v)}
            title={
              copyOnSelect
                ? "选中即复制：已开启（支持 Ctrl+C/V、Ctrl+Shift+C/V、右键粘贴）"
                : "选中即复制：已关闭（支持 Ctrl+C/V、Ctrl+Shift+C/V、右键粘贴）"
            }
          >
            <Copy size={13} />
          </button>

          <div className="term-zoom-group" title="调整终端字号 (Ctrl+- / Ctrl+= / Ctrl+0)">
            <button type="button" onClick={() => handleZoom(-1)}>
              A-
            </button>
            <span className="tabular-nums">{fontSize}</span>
            <button type="button" onClick={() => handleZoom(1)}>
              A+
            </button>
          </div>

          <select
            className="term-theme-select"
            value={fontKey}
            onChange={(e) => setFontKey(e.target.value as FontKey)}
            title="切换终端字体（支持 Nerd Font / Powerlevel10k / tmux / nvim 图标）"
          >
            {(Object.keys(TERMINAL_FONTS) as FontKey[]).map((k) => (
              <option key={k} value={k}>
                {TERMINAL_FONTS[k].label}
              </option>
            ))}
          </select>

          <select
            className="term-theme-select"
            value={themeKey}
            onChange={(e) => setThemeKey(e.target.value as ThemeKey)}
            title="切换终端配色主题"
          >
            {(Object.keys(TERMINAL_THEMES) as ThemeKey[]).map((k) => (
              <option key={k} value={k}>
                {TERMINAL_THEMES[k].label}
              </option>
            ))}
          </select>

          <button
            type="button"
            className={`icon-btn ${snippetsOpen ? "active" : ""}`}
            style={{ width: 25, height: 25 }}
            onClick={() => setSnippetsOpen((v) => !v)}
            title="显示/隐藏 Xshell 快捷命令栏"
          >
            <Zap size={13} />
          </button>

          <button
            type="button"
            className="icon-btn"
            style={{ width: 25, height: 25 }}
            onClick={() => void handleSaveLog()}
            title="导出当前终端日志 (.log)"
          >
            <FileDown size={13} />
          </button>

          <button
            type="button"
            className="icon-btn"
            style={{ width: 25, height: 25 }}
            onClick={() => getActiveHandle()?.clear()}
            title="清屏"
          >
            <Trash2 size={13} />
          </button>

          <button
            type="button"
            className="icon-btn"
            style={{ width: 25, height: 25 }}
            onClick={() =>
              setTabs((prev) =>
                prev.map((t) =>
                  t.id === activeTab?.id
                    ? { ...t, status: "connecting", reconnectSeq: t.reconnectSeq + 1 }
                    : t
                )
              )
            }
            title="重新启动当前标签的远程默认 Shell"
          >
            <RotateCw size={13} />
          </button>

          {standalone ? (
            <button
              type="button"
              className="btn btn-sm btn-outline"
              onClick={() => void api.focusMainWindow()}
              title="切换回主窗口查看本地与远程文件双栏"
            >
              <FolderOpen size={12} />
              文件窗口
            </button>
          ) : (
            <>
              {onToggleMaximize && (
                <button
                  type="button"
                  className={`icon-btn ${maximized ? "active" : ""}`}
                  style={{ width: 25, height: 25 }}
                  onClick={onToggleMaximize}
                  title={maximized ? "还原为底部停靠视图" : "工作区全屏放大终端"}
                >
                  {maximized ? <Minimize2 size={13} /> : <Maximize2 size={13} />}
                </button>
              )}
              {onPopOut && (
                <button
                  type="button"
                  className="icon-btn"
                  style={{ width: 25, height: 25 }}
                  onClick={onPopOut}
                  title="弹出为独立操作系统窗口（可自由移动/缩放/放副屏）"
                >
                  <ExternalLink size={13} />
                </button>
              )}
              {onClose && (
                <button
                  type="button"
                  className="icon-btn"
                  style={{ width: 25, height: 25 }}
                  onClick={onClose}
                  title="关闭终端面板"
                >
                  <X size={14} />
                </button>
              )}
            </>
          )}
        </div>
      </div>

      {/* Ctrl+F 终端日志搜索栏 */}
      {searchOpen && (
        <div className="term-search-bar">
          <Search size={13} color="var(--text-muted)" />
          <input
            ref={searchInputRef}
            value={searchQuery}
            onChange={(e) => {
              setSearchQuery(e.target.value);
              if (e.target.value) {
                getActiveHandle()?.findNext(e.target.value, {
                  caseSensitive: searchCase,
                  regex: searchRegex,
                });
              } else {
                getActiveHandle()?.clearSearch();
              }
            }}
            onKeyDown={(e) => {
              if (e.key === "Enter") {
                e.preventDefault();
                if (!searchQuery) return;
                if (e.shiftKey) {
                  getActiveHandle()?.findPrev(searchQuery, {
                    caseSensitive: searchCase,
                    regex: searchRegex,
                  });
                } else {
                  getActiveHandle()?.findNext(searchQuery, {
                    caseSensitive: searchCase,
                    regex: searchRegex,
                  });
                }
              } else if (e.key === "Escape") {
                setSearchOpen(false);
                getActiveHandle()?.clearSearch();
                getActiveHandle()?.focus();
              }
            }}
            placeholder="搜索终端输出（Enter 下一处 / Shift+Enter 上一处 / Esc 关闭）..."
          />
          <button
            type="button"
            className={`btn btn-sm ${searchCase ? "btn-primary" : "btn-ghost"}`}
            onClick={() => setSearchCase((v) => !v)}
            title="区分大小写"
          >
            Aa
          </button>
          <button
            type="button"
            className={`btn btn-sm ${searchRegex ? "btn-primary" : "btn-ghost"}`}
            onClick={() => setSearchRegex((v) => !v)}
            title="启用正则表达式"
          >
            .*
          </button>
          <button
            type="button"
            className="icon-btn"
            style={{ width: 24, height: 24 }}
            onClick={() =>
              searchQuery &&
              getActiveHandle()?.findPrev(searchQuery, {
                caseSensitive: searchCase,
                regex: searchRegex,
              })
            }
            title="上一处"
          >
            <ChevronUp size={14} />
          </button>
          <button
            type="button"
            className="icon-btn"
            style={{ width: 24, height: 24 }}
            onClick={() =>
              searchQuery &&
              getActiveHandle()?.findNext(searchQuery, {
                caseSensitive: searchCase,
                regex: searchRegex,
              })
            }
            title="下一处"
          >
            <ChevronDown size={14} />
          </button>
          <button
            type="button"
            className="icon-btn"
            style={{ width: 24, height: 24 }}
            onClick={() => {
              setSearchOpen(false);
              getActiveHandle()?.clearSearch();
              getActiveHandle()?.focus();
            }}
          >
            <X size={13} />
          </button>
        </div>
      )}

      {/* 多行危险粘贴二次确认警示条（防范恶意网页隐藏换行命令立即执行） */}
      {pendingPaste !== null && (
        <div
          className="term-search-bar"
          style={{
            background: "rgba(217, 119, 6, 0.14)",
            borderBottom: "1px solid rgba(217, 119, 6, 0.4)",
            justifyContent: "space-between",
            flexWrap: "wrap",
            gap: 8,
          }}
        >
          <div style={{ fontSize: 12, flex: 1, minWidth: 200 }}>
            <strong style={{ color: "#d97706" }}>多行粘贴安全确认：</strong>
            剪贴板包含 {pendingPaste.split(/\r?\n/).length} 行内容（含换行回车，将立即执行）。预览：
            <code
              style={{
                marginLeft: 6,
                fontFamily: "var(--font-mono)",
                fontSize: 11,
                opacity: 0.9,
              }}
            >
              {pendingPaste.replace(/\r?\n/g, " ↵ ").slice(0, 90)}
              {pendingPaste.length > 90 ? "..." : ""}
            </code>
          </div>
          <div style={{ display: "inline-flex", gap: 6 }}>
            <button
              type="button"
              className="btn btn-sm btn-danger"
              onClick={() => {
                const txt = pendingPaste;
                setPendingPaste(null);
                void getActiveHandle()?.writeInput(txt);
              }}
            >
              确认全部粘贴
            </button>
            <button
              type="button"
              className="btn btn-sm"
              onClick={() => {
                const firstLine = pendingPaste.split(/\r?\n/)[0] ?? "";
                setPendingPaste(null);
                if (firstLine) void getActiveHandle()?.writeInput(firstLine);
              }}
            >
              仅粘贴首行（去回车）
            </button>
            <button
              type="button"
              className="btn btn-sm btn-ghost"
              onClick={() => {
                setPendingPaste(null);
                getActiveHandle()?.focus();
              }}
            >
              取消
            </button>
          </div>
        </div>
      )}

      {/* 多标签 PTY 渲染区 */}
      <div className="term-viewport-wrap">
        {tabs.map((tab) => (
          <PtyTabPane
            key={tab.id}
            ref={(el) => {
              if (el) tabHandlesRef.current.set(tab.id, el);
              else tabHandlesRef.current.delete(tab.id);
            }}
            sessionId={sessionId}
            active={tab.id === activeTab?.id}
            reconnectSeq={tab.reconnectSeq}
            fontSize={fontSize}
            fontKey={fontKey}
            themeKey={themeKey}
            copyOnSelect={copyOnSelect}
            vimMode={vimMode}
            onOpenSearch={handleOpenSearch}
            onZoomDelta={handleZoom}
            onRequestPaste={handleRequestPaste}
            onNotify={onNotify}
            onStatusChange={(status, sizeLabel) =>
              setTabs((prev) =>
                prev.map((t) =>
                  t.id === tab.id
                    ? { ...t, status, sizeLabel: sizeLabel ?? t.sizeLabel }
                    : t
                )
              )
            }
          />
        ))}
      </div>

      {/* Xshell 底部快捷命令栏 */}
      {snippetsOpen && (
        <div className="term-snippets-bar">
          <span className="term-snippets-label">快捷命令</span>
          {snippets.map((item) => (
            <span key={item.id} className="term-snippet-chip">
              <button
                type="button"
                className="term-snippet-run"
                onClick={() => void getActiveHandle()?.writeInput(`${item.cmd}\r`)}
                title={`点击立即发送执行：${item.cmd}`}
              >
                {item.label}
              </button>
              <button
                type="button"
                className="term-snippet-del"
                onClick={() => setSnippets((prev) => prev.filter((s) => s.id !== item.id))}
                title="移除该快捷命令"
              >
                <X size={10} />
              </button>
            </span>
          ))}

          {addingSnippet ? (
            <form className="term-snippet-form" onSubmit={handleAddSnippet}>
              <input
                value={newLabel}
                onChange={(e) => setNewLabel(e.target.value)}
                placeholder="按钮名"
                style={{ width: 72 }}
                autoFocus
              />
              <input
                value={newCmd}
                onChange={(e) => setNewCmd(e.target.value)}
                placeholder="命令（如 tail -f /var/log/syslog）"
                style={{ width: 190 }}
              />
              <button type="submit" className="btn btn-sm btn-primary">
                保存
              </button>
              <button
                type="button"
                className="btn btn-sm btn-ghost"
                onClick={() => setAddingSnippet(false)}
              >
                取消
              </button>
            </form>
          ) : (
            <button
              type="button"
              className="btn btn-sm btn-ghost"
              style={{ height: 22, padding: "0 7px", fontSize: 11 }}
              onClick={() => setAddingSnippet(true)}
            >
              <Plus size={11} />
              添加命令
            </button>
          )}
        </div>
      )}
    </div>
  );
};
