/**
 * SFTP 交互式远程 SSH 终端抽屉组件（基于 xterm.js + 后端真实 PTY）
 *
 * 直接调用远端服务器默认登录 Shell（如 zsh / Oh-My-Zsh / bash / fish / PowerShell），
 * 支持完整 ANSI 256 色与真彩色渲染、Tab 补全、Ctrl+C / Ctrl+R、vim / htop / tmux 交互程序、
 * 顶部拖拽自由调整终端高度，以及一键将 Shell 切换至当前远程浏览目录。
 */

import React, { useCallback, useEffect, useRef, useState } from "react";
import { FitAddon } from "@xterm/addon-fit";
import { Terminal } from "@xterm/xterm";
import "@xterm/xterm/css/xterm.css";
import { FolderSync, RotateCw, Terminal as TermIcon, Trash2, X } from "lucide-react";
import { api } from "../api/tauri";

interface TerminalDrawerProps {
  sessionId: string;
  sessionName: string;
  cwd: string;
  onClose: () => void;
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

export const TerminalDrawer: React.FC<TerminalDrawerProps> = ({
  sessionId,
  sessionName,
  cwd,
  onClose,
}) => {
  const hostRef = useRef<HTMLDivElement>(null);
  const termRef = useRef<Terminal | null>(null);
  const fitRef = useRef<FitAddon | null>(null);
  const terminalIdRef = useRef<string | null>(null);

  const [status, setStatus] = useState<"connecting" | "connected" | "exited" | "error">(
    "connecting"
  );
  const [drawerHeight, setDrawerHeight] = useState(296);
  const [resizing, setResizing] = useState(false);
  const [reconnectSeq, setReconnectSeq] = useState(0);

  /** 向当前交互式 PTY 同步目录（同时兼容 Linux/macOS POSIX 路径与 Windows 盘符路径） */
  const handleSyncCwd = useCallback(async () => {
    const tid = terminalIdRef.current;
    if (!tid || !cwd) return;
    const isWinDrive = /^\/[A-Za-z]:/.test(cwd);
    let cmd: string;
    if (isWinDrive) {
      const winPath = cwd.slice(1).replace(/\//g, "\\");
      cmd = `cd "${winPath}"\r`;
    } else {
      const escaped = cwd.replace(/'/g, "'\\''");
      cmd = `cd '${escaped}'\r`;
    }
    try {
      await api.sshPtyWrite(tid, cmd);
      termRef.current?.focus();
    } catch {
      // ignore if exited
    }
  }, [cwd]);

  /** 顶部拖拽调整终端抽屉高度 */
  const handleStartResize = (e: React.MouseEvent) => {
    e.preventDefault();
    setResizing(true);
    const startY = e.clientY;
    const startHeight = drawerHeight;
    const onMove = (ev: MouseEvent) => {
      const delta = startY - ev.clientY;
      const maxH = Math.max(260, window.innerHeight - 190);
      setDrawerHeight(Math.min(maxH, Math.max(170, startHeight + delta)));
    };
    const onUp = () => {
      setResizing(false);
      window.removeEventListener("mousemove", onMove);
      window.removeEventListener("mouseup", onUp);
    };
    window.addEventListener("mousemove", onMove);
    window.addEventListener("mouseup", onUp);
  };

  useEffect(() => {
    const container = hostRef.current;
    if (!container) return;

    let disposed = false;
    let unlistenData: (() => void) | undefined;
    let unlistenExit: (() => void) | undefined;
    let resizeTimer: number | undefined;

    const term = new Terminal({
      cursorBlink: true,
      cursorStyle: "bar",
      fontFamily: '"Cascadia Code", "JetBrains Mono", Consolas, "PingFang SC", monospace',
      fontSize: 13,
      lineHeight: 1.32,
      scrollback: 5000,
      allowProposedApi: false,
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
    });

    const fitAddon = new FitAddon();
    term.loadAddon(fitAddon);
    term.open(container);
    termRef.current = term;
    fitRef.current = fitAddon;

    // 快捷键支持：Ctrl+Shift+C 复制选中文本、Ctrl+Shift+V 粘贴到远端 Shell
    term.attachCustomKeyEventHandler((ev) => {
      if (ev.type === "keydown" && (ev.ctrlKey || ev.metaKey) && ev.shiftKey) {
        const key = ev.key.toLowerCase();
        if (key === "c") {
          const sel = term.getSelection();
          if (sel) {
            void navigator.clipboard.writeText(sel).catch(() => {});
          }
          return false;
        }
        if (key === "v") {
          void navigator.clipboard
            .readText()
            .then((txt) => {
              const tid = terminalIdRef.current;
              if (tid && txt) void api.sshPtyWrite(tid, txt);
            })
            .catch(() => {});
          return false;
        }
      }
      return true;
    });

    const fitAndNotify = () => {
      if (disposed) return;
      try {
        fitAddon.fit();
        const tid = terminalIdRef.current;
        if (tid && term.cols > 0 && term.rows > 0) {
          void api.sshPtyResize(tid, term.cols, term.rows);
        }
      } catch {
        // ignore resize before layout
      }
    };

    const ro = new ResizeObserver(() => {
      if (resizeTimer) window.clearTimeout(resizeTimer);
      resizeTimer = window.setTimeout(fitAndNotify, 40);
    });
    ro.observe(container);

    // 转发用户按键/粘贴数据到远端 PTY
    const dataDisp = term.onData((data) => {
      const tid = terminalIdRef.current;
      if (tid) {
        void api.sshPtyWrite(tid, data);
      }
    });

    setStatus("connecting");

    (async () => {
      try {
        // 先订阅输出与退出事件，防止丢失远端登录欢迎语与首个提示符
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
            setStatus("exited");
            term.writeln("\r\n\x1b[33m[远程 Shell 会话已结束，点击右上角「重连」可重新打开]\x1b[0m");
          }
        });

        if (disposed) return;

        try {
          fitAddon.fit();
        } catch {
          // fallback defaults
        }
        const cols = Math.max(40, term.cols || 100);
        const rows = Math.max(8, term.rows || 18);

        const tid = await api.sshPtyOpen(sessionId, cols, rows);
        if (disposed) {
          void api.sshPtyClose(tid);
          return;
        }

        terminalIdRef.current = tid;
        setStatus("connected");

        // 回放握手瞬间提前到达的首批输出
        for (const item of pendingChunks) {
          if (item.tid === tid) term.write(item.bytes);
        }
        fitAndNotify();
        term.focus();
      } catch (err) {
        if (!disposed) {
          setStatus("error");
          term.writeln(`\x1b[31m打开远程交互式终端失败：${String(err)}\x1b[0m`);
        }
      }
    })();

    return () => {
      disposed = true;
      if (resizeTimer) window.clearTimeout(resizeTimer);
      ro.disconnect();
      dataDisp.dispose();
      if (unlistenData) unlistenData();
      if (unlistenExit) unlistenExit();
      const tid = terminalIdRef.current;
      terminalIdRef.current = null;
      if (tid) {
        void api.sshPtyClose(tid);
      }
      term.dispose();
      termRef.current = null;
      fitRef.current = null;
    };
  }, [sessionId, reconnectSeq]);

  return (
    <div className="bottom-drawer terminal-drawer" style={{ height: drawerHeight }}>
      <div
        className={`drawer-resizer ${resizing ? "dragging" : ""}`}
        onMouseDown={handleStartResize}
        title="拖动调节终端高度"
      />
      <div className="drawer-header">
        <TermIcon size={14} color="var(--accent)" />
        <span>SSH 交互终端 — {sessionName}</span>
        <span
          className={`drawer-pill ${
            status === "connected" ? "active" : ""
          }`}
        >
          {status === "connecting"
            ? "启动默认 Shell..."
            : status === "connected"
              ? "默认 Shell · PTY"
              : status === "exited"
                ? "已退出"
                : "连接失败"}
        </span>

        <div style={{ marginLeft: "auto", display: "flex", alignItems: "center", gap: 6 }}>
          <button
            type="button"
            className="btn btn-sm btn-ghost"
            onClick={() => void handleSyncCwd()}
            disabled={status !== "connected"}
            title={`在终端中执行 cd 跳转至当前浏览目录：${cwd}`}
          >
            <FolderSync size={12} />
            同步当前目录
          </button>
          <button
            type="button"
            className="btn btn-sm btn-ghost"
            onClick={() => {
              termRef.current?.clear();
              termRef.current?.focus();
            }}
          >
            <Trash2 size={12} />
            清屏
          </button>
          <button
            type="button"
            className="btn btn-sm btn-ghost"
            onClick={() => setReconnectSeq((n) => n + 1)}
            title="重新启动远程默认登录 Shell"
          >
            <RotateCw size={12} />
            重连
          </button>
          <button
            type="button"
            className="icon-btn"
            style={{ width: 24, height: 24 }}
            onClick={onClose}
            title="关闭终端"
          >
            <X size={14} />
          </button>
        </div>
      </div>

      <div
        ref={hostRef}
        className="xterm-host"
        onClick={() => termRef.current?.focus()}
        onContextMenu={(e) => {
          e.preventDefault();
          const sel = termRef.current?.getSelection();
          if (sel) {
            void navigator.clipboard.writeText(sel).catch(() => {});
            termRef.current?.clearSelection();
          } else {
            void navigator.clipboard
              .readText()
              .then((txt) => {
                const tid = terminalIdRef.current;
                if (tid && txt) void api.sshPtyWrite(tid, txt);
              })
              .catch(() => {});
          }
        }}
      />
    </div>
  );
};
