/**
 * 独立原生操作系统 SSH 终端窗口应用
 *
 * 通过 URL 参数 `?mode=terminal&sessionId=...` 启动，复用主进程的活跃 SSH 连接，
 * 支持自由拖动到副屏、原生最大化/缩小、本地文件拖放直传 SFTP，以及一键切回主文件窗口。
 */

import { useCallback, useEffect, useState } from "react";
import { getCurrentWebview } from "@tauri-apps/api/webview";
import { AlertCircle, CheckCircle2, FolderUp, Info } from "lucide-react";
import { api } from "../api/tauri";
import { TerminalDrawer } from "./TerminalDrawer";

interface Toast {
  id: number;
  kind: "info" | "success" | "error";
  text: string;
}

export function StandaloneTerminalApp() {
  const params = new URLSearchParams(window.location.search);
  const terminalKind = params.get("kind") === "local" ? "local" : "ssh";
  const sessionId = params.get("sessionId") ?? (terminalKind === "local" ? "__local__" : "");
  const sessionName =
    terminalKind === "local"
      ? "本地终端"
      : params.get("name") || params.get("host") || "SSH 终端";
  const cwd = params.get("cwd") || (terminalKind === "local" ? "" : "/");

  const [dragOver, setDragOver] = useState(false);
  const [toasts, setToasts] = useState<Toast[]>([]);

  const pushToast = useCallback((text: string, kind: Toast["kind"] = "info") => {
    const id = Date.now() + Math.random();
    setToasts((prev) => [...prev.slice(-3), { id, kind, text }]);
    window.setTimeout(() => {
      setToasts((prev) => prev.filter((t) => t.id !== id));
    }, 3600);
  }, []);

  // 跟随系统深浅色主题
  useEffect(() => {
    const dark = window.matchMedia("(prefers-color-scheme: dark)").matches;
    document.documentElement.setAttribute("data-theme", dark ? "dark" : "light");
  }, []);

  // 独立 SSH 终端窗口支持直接拖入本地文件/文件夹，自动走 SFTP 队列递归上传到远端目录
  useEffect(() => {
    if (terminalKind !== "ssh" || !sessionId) return;
    let unlisten: (() => void) | undefined;
    (async () => {
      try {
        unlisten = await getCurrentWebview().onDragDropEvent(async (event) => {
          const payload = event.payload;
          if (payload.type === "enter" || payload.type === "over") {
            setDragOver(true);
          } else if (payload.type === "leave") {
            setDragOver(false);
          } else if (payload.type === "drop") {
            setDragOver(false);
            if (payload.paths.length === 0) return;
            try {
              const queued = await api.enqueueUploadPaths(sessionId, payload.paths, cwd);
              pushToast(
                queued.length > 0
                  ? `已通过 SFTP 加入 ${queued.length} 个上传任务到 ${cwd}`
                  : `已在远端 ${cwd} 创建空文件夹`,
                "success"
              );
            } catch (err) {
              pushToast(`拖放上传失败：${String(err)}`, "error");
            }
          }
        });
      } catch {
        // 非 Tauri 浏览器预览模式忽略原生拖放监听
      }
    })();
    return () => {
      if (unlisten) unlisten();
    };
  }, [sessionId, cwd, pushToast]);

  if (terminalKind === "ssh" && !sessionId) {
    return (
      <div className="empty-state" style={{ height: "100vh" }}>
        <div className="empty-title">缺少 SSH 会话参数</div>
        <p className="empty-desc">请从主窗口已连接的 SFTP 会话点击「独立窗口」打开终端。</p>
      </div>
    );
  }

  return (
    <div style={{ width: "100%", height: "100%", position: "relative" }}>
      <TerminalDrawer
        mode={terminalKind}
        sessionId={sessionId}
        sessionName={sessionName}
        cwd={cwd}
        localCwd={terminalKind === "local" ? cwd : undefined}
        standalone={true}
        onNotify={pushToast}
      />

      {dragOver && (
        <div className="drop-overlay">
          <div className="drop-overlay-card">
            <FolderUp size={26} />
            <div style={{ fontWeight: 600, fontSize: 14 }}>松开鼠标，通过 SFTP 递归上传到远端目录</div>
            <div style={{ fontSize: 12, color: "var(--text-muted)", fontFamily: "var(--font-mono)" }}>
              {cwd}
            </div>
          </div>
        </div>
      )}

      {toasts.length > 0 && (
        <div className="toast-container" style={{ bottom: 14 }}>
          {toasts.map((t) => (
            <div key={t.id} className={`toast ${t.kind}`}>
              {t.kind === "success" ? (
                <CheckCircle2 size={15} color="var(--success)" style={{ flexShrink: 0 }} />
              ) : t.kind === "error" ? (
                <AlertCircle size={15} color="var(--danger)" style={{ flexShrink: 0 }} />
              ) : (
                <Info size={15} color="var(--accent)" style={{ flexShrink: 0 }} />
              )}
              <span>{t.text}</span>
            </div>
          ))}
        </div>
      )}
    </div>
  );
}
