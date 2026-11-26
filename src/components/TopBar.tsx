/**
 * 顶部工具栏组件
 *
 * 包含：侧边栏折叠按钮、品牌标识、URI 一键快速直连输入框，
 * 以及新建站点、双栏/单栏切换、SSH 终端、传输队列与深浅主题切换入口。
 */

import React, { useState } from "react";
import {
  ArrowLeftRight,
  Columns2,
  Download,
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
  canOpenTerminal: boolean;
  terminalOpen: boolean;
  onToggleTerminal: () => void;
  connecting: boolean;
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
  canOpenTerminal,
  terminalOpen,
  onToggleTerminal,
  connecting,
  onQuickConnect,
  onNewSite,
}) => {
  const [uri, setUri] = useState("");

  /** 提交快速直连 URI */
  const handleSubmit = async (e: React.FormEvent) => {
    e.preventDefault();
    if (!uri.trim() || connecting) return;
    await onQuickConnect(uri.trim());
  };

  return (
    <header className="topbar">
      <button
        className={`icon-btn ${sidebarOpen ? "active" : ""}`}
        onClick={onToggleSidebar}
        title="显示/隐藏站点栏"
      >
        <PanelLeft size={17} />
      </button>

      <div className="brand">
        <div className="brand-mark">
          <ArrowLeftRight size={15} />
        </div>
        <span>xiaorui_FTP</span>
      </div>

      {/* 顶部快速直连栏，支持 sftp/ftps/ftp/s3/webdav 与 ?jump= 跳板机参数 */}
      <form className="quick-connect" onSubmit={handleSubmit}>
        <Zap size={14} color="var(--text-muted)" />
        <input
          value={uri}
          onChange={(e) => setUri(e.target.value)}
          placeholder="快速直连：sftp://user@host:22/path?jump=ops@bastion · ftps:// · s3:// · webdav://"
          disabled={connecting}
        />
        <button
          type="submit"
          className="btn btn-sm btn-primary"
          disabled={connecting || !uri.trim()}
        >
          {connecting ? "连接中..." : "直连"}
        </button>
      </form>

      <div className="topbar-actions">
        <button className="btn btn-primary" onClick={onNewSite}>
          <Plus size={15} />
          新建站点
        </button>

        <button
          className={`icon-btn ${dualPane ? "active" : ""}`}
          onClick={onToggleDualPane}
          title={dualPane ? "切换为单栏（仅远程）" : "切换为双栏（本地 + 远程）"}
        >
          {dualPane ? <Columns2 size={17} /> : <Square size={16} />}
        </button>

        {canOpenTerminal && (
          <button
            className={`icon-btn ${terminalOpen ? "active" : ""}`}
            onClick={onToggleTerminal}
            title="SSH 远程终端"
          >
            <Terminal size={17} />
          </button>
        )}

        <button
          className={`icon-btn ${transferOpen ? "active" : ""}`}
          onClick={onToggleTransfers}
          title="传输队列"
        >
          <Download size={17} />
          {activeTransferCount > 0 && (
            <span className="badge-count">{activeTransferCount}</span>
          )}
        </button>

        <button
          className="icon-btn"
          onClick={onToggleTheme}
          title={theme === "light" ? "切换深色模式" : "切换浅色模式"}
        >
          {theme === "light" ? <Moon size={16} /> : <Sun size={16} />}
        </button>
      </div>
    </header>
  );
};
