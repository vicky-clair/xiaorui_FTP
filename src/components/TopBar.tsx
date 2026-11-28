/**
 * 顶部工具栏组件
 *
 * 包含：侧边栏折叠按钮、品牌标识、支持 Ctrl+K 快捷键聚焦的 URI 一键快速直连输入框，
 * 以及新建站点、双栏/单栏切换、SSH 终端、传输队列与深浅主题切换入口。
 */

import React, { useState } from "react";
import {
  ArrowLeftRight,
  Columns2,
  Download,
  Laptop,
  Moon,
  PanelLeft,
  Plus,
  Square,
  Sun,
  Terminal,
  Zap,
} from "lucide-react";

interface TopBarProps {
  sidebarOpen: boolean;
  onToggleSidebar: () => void;
  dualPane: boolean;
  onToggleDualPane: () => void;
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
  dualPane,
  onToggleDualPane,
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
          className={`btn-tool ${dualPane ? "active" : ""}`}
          onClick={onToggleDualPane}
          title={dualPane ? "当前为本地+远程双栏，点击切换为仅远程单栏" : "当前为单栏，点击切换为本地+远程双栏"}
        >
          {dualPane ? <Columns2 size={14} /> : <Square size={13} />}
          {dualPane ? "双栏" : "单栏"}
        </button>

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
