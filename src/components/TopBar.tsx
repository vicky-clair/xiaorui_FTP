/**
 * 顶部工具栏组件
 *
 * 包含：侧边栏折叠按钮、品牌标识、支持 Ctrl+K 快捷键聚焦的 URI 一键快速直连输入框，
 * 以及新建站点、双栏/单栏切换、SSH 终端、传输队列与深浅主题切换入口。
 */

import React, { useEffect, useRef, useState } from "react";
import {
  Activity,
  ArrowLeftRight,
  ChevronDown,
  Columns2,
  Download,
  Laptop,
  Moon,
  PanelLeft,
  Plus,
  Server,
  Sun,
  Terminal,
  Zap,
} from "lucide-react";

interface TopBarProps {
  sidebarOpen: boolean;
  onToggleSidebar: () => void;
  showLocalPane: boolean;
  showRemotePane: boolean;
  onToggleLocalPane: () => void;
  onToggleRemotePane: () => void;
  onShowBothPanes: () => void;
  monitorOpen: boolean;
  onToggleMonitor: () => void;
  theme: "light" | "dark";
  onToggleTheme: () => void;
  transferOpen: boolean;
  activeTransferCount: number;
  onToggleTransfers: () => void;
  localTerminalOpen: boolean;
  onToggleLocalTerminal: () => void;
  canOpenTerminal: boolean;
  terminalOpen: boolean;
  onToggleTerminal: () => void;
  connecting: boolean;
  quickConnectInputRef?: React.RefObject<HTMLInputElement | null>;
  onQuickConnect: (uri: string) => Promise<void>;
  onNewSite: () => void;
}

export const TopBar: React.FC<TopBarProps> = ({
  sidebarOpen,
  onToggleSidebar,
  showLocalPane,
  showRemotePane,
  onToggleLocalPane,
  onToggleRemotePane,
  onShowBothPanes,
  monitorOpen,
  onToggleMonitor,
  theme,
  onToggleTheme,
  transferOpen,
  activeTransferCount,
  onToggleTransfers,
  localTerminalOpen,
  onToggleLocalTerminal,
  canOpenTerminal,
  terminalOpen,
  onToggleTerminal,
  connecting,
  quickConnectInputRef,
  onQuickConnect,
  onNewSite,
}) => {
  const [uri, setUri] = useState("");
  const [focused, setFocused] = useState(false);
  const [paneMenuOpen, setPaneMenuOpen] = useState(false);
  const paneMenuRef = useRef<HTMLDivElement>(null);

  useEffect(() => {
    if (!paneMenuOpen) return;
    const onDocDown = (ev: MouseEvent) => {
      if (paneMenuRef.current && !paneMenuRef.current.contains(ev.target as Node)) {
        setPaneMenuOpen(false);
      }
    };
    const onKey = (ev: KeyboardEvent) => {
      if (ev.key === "Escape") setPaneMenuOpen(false);
    };
    window.addEventListener("mousedown", onDocDown);
    window.addEventListener("keydown", onKey);
    return () => {
      window.removeEventListener("mousedown", onDocDown);
      window.removeEventListener("keydown", onKey);
    };
  }, [paneMenuOpen]);

  const paneLabel =
    showLocalPane && showRemotePane
      ? "双栏"
      : showLocalPane
        ? "仅本地"
        : showRemotePane
          ? "仅远程"
          : "无文件栏";

  /** 提交快速直连 URI */
  const handleSubmit = async (e: React.FormEvent) => {
    e.preventDefault();
    if (!uri.trim() || connecting) return;
    await onQuickConnect(uri.trim());
  };

  return (
    <header className="topbar">
      <button
        type="button"
        className={`btn-tool ${sidebarOpen ? "active" : ""}`}
        onClick={onToggleSidebar}
        title="显示或隐藏左侧站点管理栏"
      >
        <PanelLeft size={14} />
        站点
      </button>

      <div className="brand">
        <div className="brand-mark">
          <ArrowLeftRight size={14} />
        </div>
        <span>xiaorui_FTP</span>
      </div>

      {/* 顶部命令栏式快速直连输入框（Ctrl+K 聚焦） */}
      <form className="quick-connect" onSubmit={handleSubmit}>
        <Zap size={14} color={focused ? "var(--accent)" : "var(--text-muted)"} />
        <input
          ref={quickConnectInputRef}
          value={uri}
          onChange={(e) => setUri(e.target.value)}
          onFocus={() => setFocused(true)}
          onBlur={() => setFocused(false)}
          placeholder="快速直连：sftp://user@host:22/path?jump=ops@bastion · ftps:// · s3:// · webdav://"
          disabled={connecting}
        />
        {!uri && !focused && <kbd className="kbd-hint">Ctrl K</kbd>}
        <button
          type="submit"
          className="btn btn-sm btn-primary"
          disabled={connecting || !uri.trim()}
        >
          {connecting ? "连接中..." : "直连"}
        </button>
      </form>

      <div className="topbar-actions">
        <button
          type="button"
          className="btn btn-primary"
          style={{ height: 28, padding: "0 11px", fontSize: 12 }}
          onClick={onNewSite}
          title="新建连接站点 (Ctrl+N)"
        >
          <Plus size={14} />
          新建站点
        </button>

        <span className="topbar-sep" />

        <button
          type="button"
          className={`btn-tool ${localTerminalOpen ? "active" : ""}`}
          onClick={onToggleLocalTerminal}
          title="打开本地终端（自动使用 Windows 默认终端配置）"
        >
          <Laptop size={14} />
          本地终端
        </button>

        {canOpenTerminal && (
          <button
            type="button"
            className={`btn-tool ${terminalOpen ? "active" : ""}`}
            onClick={onToggleTerminal}
            title="打开当前远程主机的 SSH 交互终端"
          >
            <Terminal size={14} />
            SSH 终端
          </button>
        )}

        <button
          type="button"
          className={`btn-tool ${transferOpen ? "active" : ""}`}
          onClick={onToggleTransfers}
          title="查看上传与下载传输队列"
        >
          <Download size={14} />
          传输
          {activeTransferCount > 0 && (
            <span className="inline-badge">{activeTransferCount}</span>
          )}
        </button>

        <span className="topbar-sep" />

        <button
          type="button"
          className={`btn-tool ${monitorOpen ? "active" : ""}`}
          onClick={onToggleMonitor}
          title="显示或隐藏右侧实时系统监控与目录统计面板"
        >
          <Activity size={14} />
          监控
        </button>

        <div className="term-popover-anchor" ref={paneMenuRef}>
          <button
            type="button"
            className={`btn-tool ${paneMenuOpen || (showLocalPane && showRemotePane) ? "active" : ""}`}
            onClick={() => setPaneMenuOpen((v) => !v)}
            title="选择显示或隐藏左侧本地文件栏、右侧远程文件栏（不影响监控栏）"
          >
            <Columns2 size={14} />
            {paneLabel}
            <ChevronDown size={12} />
          </button>

          {paneMenuOpen && (
            <div className="term-settings-popover" style={{ width: 228 }}>
              <div className="term-popover-section-title">文件分栏显示（独立于监控栏）</div>

              <button
                type="button"
                className="term-popover-toggle"
                onClick={onToggleLocalPane}
              >
                <span style={{ display: "inline-flex", alignItems: "center", gap: 6 }}>
                  <Laptop size={13} />
                  左侧 · 本地文件栏
                </span>
                <span className={`term-toggle-pill ${showLocalPane ? "on" : ""}`}>
                  {showLocalPane ? "显示" : "隐藏"}
                </span>
              </button>

              <button
                type="button"
                className="term-popover-toggle"
                onClick={onToggleRemotePane}
              >
                <span style={{ display: "inline-flex", alignItems: "center", gap: 6 }}>
                  <Server size={13} />
                  右侧 · 远程文件栏
                </span>
                <span className={`term-toggle-pill ${showRemotePane ? "on" : ""}`}>
                  {showRemotePane ? "显示" : "隐藏"}
                </span>
              </button>

              {(!showLocalPane || !showRemotePane) && (
                <>
                  <div className="term-popover-sep" />
                  <button
                    type="button"
                    className="term-popover-toggle"
                    onClick={() => {
                      onShowBothPanes();
                      setPaneMenuOpen(false);
                    }}
                  >
                    <span style={{ display: "inline-flex", alignItems: "center", gap: 6 }}>
                      <Columns2 size={13} color="var(--accent)" />
                      恢复双栏（本地 + 远程）
                    </span>
                  </button>
                </>
              )}
            </div>
          )}
        </div>

        <button
          type="button"
          className="btn-tool"
          onClick={onToggleTheme}
          title={theme === "light" ? "切换为深色外观" : "切换为浅色外观"}
        >
          {theme === "light" ? <Moon size={14} /> : <Sun size={14} />}
          {theme === "light" ? "深色" : "浅色"}
        </button>
      </div>
    </header>
  );
};
