/**
 * xiaorui_FTP 主工作台组件
 *
 * 负责统筹全局主题、多标签远程会话、本地/远程双栏状态、系统原生拖拽上传监听、
 * 传输队列事件同步（含完成后自动刷新目录）、在线文本编辑器与内置 SSH 终端。
 */

import { useCallback, useEffect, useRef, useState } from "react";
import { getCurrentWebview } from "@tauri-apps/api/webview";
import { open, save } from "@tauri-apps/plugin-dialog";
import { Cloud, Globe, Lock, Server, X } from "lucide-react";
import { api } from "./api/tauri";
import { EditorModal } from "./components/EditorModal";
import { FilePane } from "./components/FilePane";
import { SiteModal } from "./components/SiteModal";
import { SiteSidebar } from "./components/SiteSidebar";
import { TerminalDrawer } from "./components/TerminalDrawer";
import { TopBar } from "./components/TopBar";
import { TransferDrawer } from "./components/TransferDrawer";
import {
  PROTOCOL_META,
  type FileEntry,
  type LocalRoot,
  type Protocol,
  type SessionInfo,
  type SiteConfig,
  type SiteInput,
  type TransferTask,
} from "./types";

interface Toast {
  id: number;
  kind: "info" | "success" | "error";
  text: string;
}

export default function App() {
  const [theme, setTheme] = useState<"light" | "dark">(() =>
    window.matchMedia("(prefers-color-scheme: dark)").matches ? "dark" : "light"
  );
  const [sidebarOpen, setSidebarOpen] = useState(true);
  const [dualPane, setDualPane] = useState(true);
  const [transferOpen, setTransferOpen] = useState(false);
  const [terminalOpen, setTerminalOpen] = useState(false);

  // Sites & Modal
  const [sites, setSites] = useState<SiteConfig[]>([]);
  const [siteModal, setSiteModal] = useState<{
    open: boolean;
    editing: SiteConfig | null;
    defaultProtocol: Protocol;
  }>({ open: false, editing: null, defaultProtocol: "sftp" });

  // Sessions
  const [sessions, setSessions] = useState<SessionInfo[]>([]);
  const [activeSessionId, setActiveSessionId] = useState<string | null>(null);
  const [connecting, setConnecting] = useState(false);
  const [connectingSiteId, setConnectingSiteId] = useState<string | null>(null);

  // Local FS
  const [localRoots, setLocalRoots] = useState<LocalRoot[]>([]);
  const [localPath, setLocalPath] = useState("");
  const [localEntries, setLocalEntries] = useState<FileEntry[]>([]);
  const [localLoading, setLocalLoading] = useState(false);

  // Remote FS
  const [remotePath, setRemotePath] = useState("/");
  const [remoteEntries, setRemoteEntries] = useState<FileEntry[]>([]);
  const [remoteLoading, setRemoteLoading] = useState(false);

  // Transfers
  const [transfers, setTransfers] = useState<TransferTask[]>([]);
  const [osDragOver, setOsDragOver] = useState(false);

  // Refs for OS drag-and-drop & transfer completion callbacks
  const activeSessionIdRef = useRef<string | null>(null);
  const remotePathRef = useRef<string>("/");
  const localPathRef = useRef<string>("");

  // Editor
  const [editor, setEditor] = useState<{
    path: string;
    content: string;
  } | null>(null);

  // Toasts
  const [toasts, setToasts] = useState<Toast[]>([]);

  const pushToast = useCallback((text: string, kind: Toast["kind"] = "info") => {
    const id = Date.now() + Math.random();
    setToasts((prev) => [...prev.slice(-3), { id, kind, text }]);
    window.setTimeout(() => {
      setToasts((prev) => prev.filter((t) => t.id !== id));
    }, 3600);
  }, []);

  useEffect(() => {
    document.documentElement.setAttribute("data-theme", theme);
  }, [theme]);

  const activeSession = sessions.find((s) => s.sessionId === activeSessionId) ?? null;

  useEffect(() => {
    activeSessionIdRef.current = activeSessionId;
  }, [activeSessionId]);

  useEffect(() => {
    remotePathRef.current = remotePath;
  }, [remotePath]);

  useEffect(() => {
    localPathRef.current = localPath;
  }, [localPath]);

  const loadLocalDir = useCallback(
    async (path: string) => {
      setLocalLoading(true);
      try {
        const list = await api.localListDir(path);
        setLocalPath(path);
        setLocalEntries(list);
      } catch (err) {
        pushToast(String(err), "error");
      } finally {
        setLocalLoading(false);
      }
    },
    [pushToast]
  );

  const loadRemoteDir = useCallback(
    async (sessionId: string, path: string) => {
      setRemoteLoading(true);
      try {
        const list = await api.remoteListDir(sessionId, path);
        setRemotePath(path);
        setRemoteEntries(list);
        setSessions((prev) =>
          prev.map((s) => (s.sessionId === sessionId ? { ...s, currentPath: path } : s))
        );
      } catch (err) {
        pushToast(String(err), "error");
      } finally {
        setRemoteLoading(false);
      }
    },
    [pushToast]
  );

  // Initial bootstrap
  useEffect(() => {
    let unlisten: (() => void) | undefined;

    (async () => {
      try {
        const [siteList, roots, home, existingTransfers] = await Promise.all([
          api.listSites(),
          api.localListRoots(),
          api.localHomeDir(),
          api.listTransfers(),
        ]);
        setSites(siteList);
        setLocalRoots(roots);
        setTransfers(existingTransfers);
        await loadLocalDir(home);
      } catch (err) {
        pushToast(`初始化失败: ${String(err)}`, "error");
      }

      unlisten = await api.onTransferUpdate((task) => {
        setTransfers((prev) => {
          const idx = prev.findIndex((t) => t.id === task.id);
          if (idx >= 0) {
            const copy = [...prev];
            copy[idx] = task;
            return copy;
          }
          return [task, ...prev];
        });
        if (task.status === "completed") {
          if (task.direction === "upload" && activeSessionIdRef.current === task.sessionId) {
            void loadRemoteDir(task.sessionId, remotePathRef.current);
          } else if (task.direction === "download" && localPathRef.current) {
            void loadLocalDir(localPathRef.current);
          }
        }
      });
    })();

    return () => {
      if (unlisten) unlisten();
    };
  }, [loadLocalDir, loadRemoteDir, pushToast]);

  // Native OS File Explorer / Finder drag-and-drop -> recursive folder & file upload
  useEffect(() => {
    let unlistenDrop: (() => void) | undefined;
    (async () => {
      try {
        unlistenDrop = await getCurrentWebview().onDragDropEvent(async (event) => {
          const payload = event.payload;
          if (payload.type === "enter" || payload.type === "over") {
            if (activeSessionIdRef.current) {
              setOsDragOver(true);
            }
          } else if (payload.type === "leave") {
            setOsDragOver(false);
          } else if (payload.type === "drop") {
            setOsDragOver(false);
            const sid = activeSessionIdRef.current;
            const targetDir = remotePathRef.current;
            if (!sid) {
              pushToast("请先连接远程站点，再拖入文件或文件夹上传", "error");
              return;
            }
            if (payload.paths.length === 0) return;
            try {
              const queued = await api.enqueueUploadPaths(sid, payload.paths, targetDir);
              setTransferOpen(true);
              pushToast(`已扫描并加入 ${queued.length} 个上传任务`, "success");
            } catch (err) {
              pushToast(String(err), "error");
            }
          }
        });
      } catch {
        // Non-Tauri browser preview mode
      }
    })();

    return () => {
      if (unlistenDrop) unlistenDrop();
    };
  }, [pushToast]);

  const activateSession = useCallback(
    async (sess: SessionInfo) => {
      setSessions((prev) => {
        if (prev.some((s) => s.sessionId === sess.sessionId)) return prev;
        return [...prev, sess];
      });
      setActiveSessionId(sess.sessionId);
      setTerminalOpen(false);
      await loadRemoteDir(sess.sessionId, sess.currentPath || "/");
    },
    [loadRemoteDir]
  );

  const handleQuickConnect = async (uri: string) => {
    setConnecting(true);
    try {
      const sess = await api.quickConnectUri(uri);
      await activateSession(sess);
      pushToast(`已连接到 ${sess.host}`, "success");
    } catch (err) {
      pushToast(String(err), "error");
    } finally {
      setConnecting(false);
    }
  };

  const handleConnectSite = async (site: SiteConfig) => {
    setConnectingSiteId(site.id);
    try {
      const sess = await api.connectSavedSite(site.id);
      await activateSession(sess);
      pushToast(`已连接到 ${site.name}`, "success");
    } catch (err) {
      pushToast(String(err), "error");
    } finally {
      setConnectingSiteId(null);
    }
  };

  const handleSaveSite = async (input: SiteInput, connectAfter: boolean) => {
    const saved = await api.saveSite(input);
    const list = await api.listSites();
    setSites(list);
    pushToast(`已保存站点「${saved.name}」`, "success");

    if (connectAfter) {
      setConnectingSiteId(saved.id);
      try {
        const sess = await api.connectSavedSite(
          saved.id,
          input.secret ?? undefined,
          input.passphrase ?? undefined,
          input.jumpSecret ?? undefined,
          input.jumpPassphrase ?? undefined
        );
        await activateSession(sess);
      } catch (err) {
        pushToast(String(err), "error");
      } finally {
        setConnectingSiteId(null);
      }
    }
  };

  const handleDeleteSite = async (site: SiteConfig) => {
    if (!window.confirm(`确认删除站点「${site.name}」及其保存的凭据？`)) return;
    await api.deleteSite(site.id);
    setSites((prev) => prev.filter((s) => s.id !== site.id));
    pushToast(`已删除站点「${site.name}」`);
  };

  const handleDisconnect = async (sessionId: string) => {
    await api.disconnectSession(sessionId);
    setSessions((prev) => {
      const next = prev.filter((s) => s.sessionId !== sessionId);
      if (activeSessionId === sessionId) {
        const fallback = next[0] ?? null;
        setActiveSessionId(fallback ? fallback.sessionId : null);
        if (fallback) {
          void loadRemoteDir(fallback.sessionId, fallback.currentPath);
        } else {
          setRemoteEntries([]);
          setTerminalOpen(false);
        }
      }
      return next;
    });
  };

  // 站点配置本地 JSON 文件备份导出与导入
  const handleExportSites = async () => {
    try {
      const target = await save({
        defaultPath: "xiaorui_FTP-sites-backup.json",
        filters: [{ name: "JSON 备份文件", extensions: ["json"] }],
        title: "导出站点备份（不含敏感密钥）",
      });
      if (!target) return;
      await api.exportSitesToFile(target);
      pushToast(`站点备份已保存至 ${target}`, "success");
    } catch (err) {
      pushToast(String(err), "error");
    }
  };

  const handleImportSites = async () => {
    try {
      const picked = await open({
        multiple: false,
        filters: [{ name: "JSON 备份文件", extensions: ["json"] }],
        title: "选择要导入的站点备份 JSON 文件",
      });
      if (typeof picked !== "string" || !picked) return;
      const count = await api.importSitesFromFile(picked);
      setSites(await api.listSites());
      pushToast(`成功导入 ${count} 个站点（密钥需按需重新填写）`, "success");
    } catch (err) {
      pushToast(String(err), "error");
    }
  };

  // Transfers (single file + recursive folder upload/download)
  const handleUploadEntry = async (entry: FileEntry) => {
    if (!activeSession) return;
    try {
      const queued = await api.enqueueUpload(activeSession.sessionId, entry.path, remotePath);
      setTransferOpen(true);
      pushToast(
        entry.isDir
          ? `已递归加入文件夹「${entry.name}」共 ${queued.length} 个文件上传`
          : `已加入上传队列：${entry.name}`,
        "success"
      );
    } catch (err) {
      pushToast(String(err), "error");
    }
  };

  const handleDownloadEntry = async (entry: FileEntry, askLocation = true) => {
    if (!activeSession) return;
    try {
      let queued: TransferTask[];
      if (askLocation) {
        if (entry.isDir) {
          const pickedDir = await open({
            directory: true,
            multiple: false,
            defaultPath: localPath || undefined,
            title: `选择文件夹「${entry.name}」的下载保存目录`,
          });
          if (typeof pickedDir !== "string" || !pickedDir) return;
          queued = await api.enqueueDownload(activeSession.sessionId, entry.path, {
            localDir: pickedDir,
            isDir: true,
            sizeHint: entry.size,
          });
        } else {
          const sep = localPath.includes("\\") ? "\\" : "/";
          const defaultSavePath = localPath
            ? `${localPath.endsWith(sep) ? localPath : localPath + sep}${entry.name}`
            : entry.name;
          const pickedFile = await save({
            defaultPath: defaultSavePath,
            title: `选择「${entry.name}」的保存位置`,
          });
          if (!pickedFile) return;
          queued = await api.enqueueDownload(activeSession.sessionId, entry.path, {
            localFilePath: pickedFile,
            isDir: false,
            sizeHint: entry.size,
          });
        }
      } else {
        if (!localPath) return;
        queued = await api.enqueueDownload(activeSession.sessionId, entry.path, {
          localDir: localPath,
          isDir: entry.isDir,
          sizeHint: entry.size,
        });
      }

      setTransferOpen(true);
      pushToast(
        entry.isDir
          ? `已递归加入文件夹「${entry.name}」共 ${queued.length} 个文件下载`
          : `已加入下载队列：${entry.name}`,
        "success"
      );
    } catch (err) {
      pushToast(String(err), "error");
    }
  };

  const handleUploadFromPicker = async () => {
    if (!activeSession) return;
    const picked = await open({ multiple: true, title: "选择要上传的文件" });
    if (!picked) return;
    const paths = Array.isArray(picked) ? picked : [picked];
    try {
      const queued = await api.enqueueUploadPaths(activeSession.sessionId, paths, remotePath);
      setTransferOpen(true);
      pushToast(`已加入 ${queued.length} 个文件上传任务`, "success");
    } catch (err) {
      pushToast(String(err), "error");
    }
  };

  const handleUploadFolderFromPicker = async () => {
    if (!activeSession) return;
    const picked = await open({
      directory: true,
      multiple: true,
      title: "选择要递归上传的文件夹",
    });
    if (!picked) return;
    const paths = Array.isArray(picked) ? picked : [picked];
    try {
      const queued = await api.enqueueUploadPaths(activeSession.sessionId, paths, remotePath);
      setTransferOpen(true);
      pushToast(`已递归扫描并加入 ${queued.length} 个上传任务`, "success");
    } catch (err) {
      pushToast(String(err), "error");
    }
  };

  // Remote actions
  const handleRemoteMkdir = async () => {
    if (!activeSession) return;
    const name = window.prompt("请输入新文件夹名称：");
    if (!name?.trim()) return;
    const target = remotePath === "/" ? `/${name.trim()}` : `${remotePath}/${name.trim()}`;
    try {
      await api.remoteMkdir(activeSession.sessionId, target);
      await loadRemoteDir(activeSession.sessionId, remotePath);
    } catch (err) {
      pushToast(String(err), "error");
    }
  };

  const handleRemoteRename = async (entry: FileEntry) => {
    if (!activeSession) return;
    const nextName = window.prompt("重命名为：", entry.name);
    if (!nextName?.trim() || nextName.trim() === entry.name) return;
    const parent = remotePath === "/" ? "" : remotePath;
    try {
      await api.remoteRename(activeSession.sessionId, entry.path, `${parent}/${nextName.trim()}`);
      await loadRemoteDir(activeSession.sessionId, remotePath);
    } catch (err) {
      pushToast(String(err), "error");
    }
  };

  const handleRemoteDelete = async (entry: FileEntry) => {
    if (!activeSession) return;
    if (!window.confirm(`确认删除远程${entry.isDir ? "目录" : "文件"}「${entry.name}」？`)) return;
    try {
      await api.remoteDelete(activeSession.sessionId, entry.path, entry.isDir);
      await loadRemoteDir(activeSession.sessionId, remotePath);
    } catch (err) {
      pushToast(String(err), "error");
    }
  };

  const handleRemoteChmod = async (entry: FileEntry) => {
    if (!activeSession) return;
    const defaultOctal = entry.mode != null ? entry.mode.toString(8).padStart(3, "0") : "644";
    const input = window.prompt(`修改「${entry.name}」权限（八进制，例如 755 或 644）：`, defaultOctal);
    if (!input?.trim()) return;
    const parsed = parseInt(input.trim(), 8);
    if (Number.isNaN(parsed)) {
      pushToast("请输入有效的八进制权限值（如 755）", "error");
      return;
    }
    try {
      await api.remoteChmod(activeSession.sessionId, entry.path, parsed);
      await loadRemoteDir(activeSession.sessionId, remotePath);
      pushToast(`已更新权限为 ${input.trim()}`, "success");
    } catch (err) {
      pushToast(String(err), "error");
    }
  };

  const handleOpenEditor = async (entry: FileEntry) => {
    if (!activeSession) return;
    try {
      const text = await api.remoteReadText(activeSession.sessionId, entry.path);
      setEditor({ path: entry.path, content: text });
    } catch (err) {
      pushToast(String(err), "error");
    }
  };

  // Local actions
  const handleLocalMkdir = async () => {
    const name = window.prompt("请输入本地新文件夹名称：");
    if (!name?.trim()) return;
    const sep = localPath.includes("\\") ? "\\" : "/";
    const base = localPath.endsWith(sep) ? localPath : `${localPath}${sep}`;
    try {
      await api.localMkdir(`${base}${name.trim()}`);
      await loadLocalDir(localPath);
    } catch (err) {
      pushToast(String(err), "error");
    }
  };

  const handleLocalRename = async (entry: FileEntry) => {
    const nextName = window.prompt("重命名为：", entry.name);
    if (!nextName?.trim() || nextName.trim() === entry.name) return;
    const sep = localPath.includes("\\") ? "\\" : "/";
    const base = localPath.endsWith(sep) ? localPath : `${localPath}${sep}`;
    try {
      await api.localRename(entry.path, `${base}${nextName.trim()}`);
      await loadLocalDir(localPath);
    } catch (err) {
      pushToast(String(err), "error");
    }
  };

  const handleLocalDelete = async (entry: FileEntry) => {
    if (!window.confirm(`确认删除本地${entry.isDir ? "目录" : "文件"}「${entry.name}」？`)) return;
    try {
      await api.localDelete(entry.path, entry.isDir);
      await loadLocalDir(localPath);
    } catch (err) {
      pushToast(String(err), "error");
    }
  };

  const activeTransferCount = transfers.filter(
    (t) => t.status === "running" || t.status === "queued"
  ).length;

  return (
    <div className="app-shell">
      <TopBar
        sidebarOpen={sidebarOpen}
        onToggleSidebar={() => setSidebarOpen((v) => !v)}
        dualPane={dualPane}
        onToggleDualPane={() => setDualPane((v) => !v)}
        theme={theme}
        onToggleTheme={() => setTheme((t) => (t === "light" ? "dark" : "light"))}
        transferOpen={transferOpen}
        activeTransferCount={activeTransferCount}
        onToggleTransfers={() => {
          setTransferOpen((v) => !v);
          if (!transferOpen) setTerminalOpen(false);
        }}
        canOpenTerminal={Boolean(activeSession?.supportsShell)}
        terminalOpen={terminalOpen}
        onToggleTerminal={() => {
          setTerminalOpen((v) => !v);
          if (!terminalOpen) setTransferOpen(false);
        }}
        connecting={connecting}
        onQuickConnect={handleQuickConnect}
        onNewSite={() =>
          setSiteModal({ open: true, editing: null, defaultProtocol: "sftp" })
        }
      />

      <div className="app-body">
        {sidebarOpen && (
          <SiteSidebar
            sites={sites}
            connectingSiteId={connectingSiteId}
            onConnect={handleConnectSite}
            onEdit={(site) =>
              setSiteModal({
                open: true,
                editing: site,
                defaultProtocol: site.protocol,
              })
            }
            onDelete={handleDeleteSite}
            onExport={handleExportSites}
            onImport={handleImportSites}
          />
        )}

        <main className="workspace">
          {sessions.length > 0 && (
            <div className="session-tabs">
              {sessions.map((s) => {
                const meta = PROTOCOL_META[s.protocol];
                const isActive = s.sessionId === activeSessionId;
                return (
                  <div
                    key={s.sessionId}
                    className={`session-tab ${isActive ? "active" : ""}`}
                    onClick={() => {
                      setActiveSessionId(s.sessionId);
                      void loadRemoteDir(s.sessionId, s.currentPath);
                    }}
                  >
                    <span className={`proto-badge ${meta.badgeClass}`}>{meta.short}</span>
                    <span
                      style={{
                        overflow: "hidden",
                        textOverflow: "ellipsis",
                        whiteSpace: "nowrap",
                      }}
                    >
                      {s.name}
                    </span>
                    <button
                      className="session-tab-close"
                      onClick={(e) => {
                        e.stopPropagation();
                        void handleDisconnect(s.sessionId);
                      }}
                      title="断开连接"
                    >
                      <X size={12} />
                    </button>
                  </div>
                );
              })}
            </div>
          )}

          <div className="panes">
            {dualPane && (
              <FilePane
                mode="local"
                title="本地"
                currentPath={localPath}
                entries={localEntries}
                loading={localLoading}
                roots={localRoots}
                canTransfer={Boolean(activeSession)}
                onNavigate={(p) => void loadLocalDir(p)}
                onRefresh={() => void loadLocalDir(localPath)}
                onMkdir={() => void handleLocalMkdir()}
                onRename={(e) => void handleLocalRename(e)}
                onDelete={(e) => void handleLocalDelete(e)}
                onTransfer={(e) => void handleUploadEntry(e)}
                onDropEntryFromOtherPane={(e) => void handleDownloadEntry(e, false)}
              />
            )}

            {activeSession ? (
              <FilePane
                mode="remote"
                title={`${PROTOCOL_META[activeSession.protocol].short} · ${activeSession.host}`}
                currentPath={remotePath}
                entries={remoteEntries}
                loading={remoteLoading}
                canTransfer={true}
                supportsChmod={activeSession.supportsChmod}
                externalDropActive={osDragOver}
                onNavigate={(p) => void loadRemoteDir(activeSession.sessionId, p)}
                onRefresh={() => void loadRemoteDir(activeSession.sessionId, remotePath)}
                onMkdir={() => void handleRemoteMkdir()}
                onRename={(e) => void handleRemoteRename(e)}
                onDelete={(e) => void handleRemoteDelete(e)}
                onTransfer={(e) => void handleDownloadEntry(e, true)}
                onUploadFromPicker={() => void handleUploadFromPicker()}
                onUploadFolderFromPicker={() => void handleUploadFolderFromPicker()}
                onDropEntryFromOtherPane={(e) => void handleUploadEntry(e)}
                onEditFile={(e) => void handleOpenEditor(e)}
                onChmod={(e) => void handleRemoteChmod(e)}
              />
            ) : (
              <section className="pane">
                <div className="empty-state">
                  <div className="brand-mark" style={{ width: 44, height: 44, borderRadius: 12 }}>
                    <Server size={22} />
                  </div>
                  <div className="empty-title">极简、安全的现代文件传输工作台</div>
                  <p className="empty-desc">
                    选择一种访问方式新建连接，或在顶部地址栏直接输入 URI 一键直连。原生 x64 构架，全面支持现代加密与云存储。
                  </p>

                  <div className="quick-proto-grid">
                    <button
                      className="quick-proto-card"
                      onClick={() =>
                        setSiteModal({ open: true, editing: null, defaultProtocol: "sftp" })
                      }
                    >
                      <Lock size={18} color="#059669" />
                      <h4>SFTP / SSH 终端</h4>
                      <p>Ed25519 / Curve25519 / 密钥登录与远程命令行</p>
                    </button>

                    <button
                      className="quick-proto-card"
                      onClick={() =>
                        setSiteModal({ open: true, editing: null, defaultProtocol: "s3" })
                      }
                    >
                      <Cloud size={18} color="#7c3aed" />
                      <h4>S3 兼容对象存储</h4>
                      <p>AWS S3 · 阿里云 OSS · 腾讯云 COS · MinIO · R2</p>
                    </button>

                    <button
                      className="quick-proto-card"
                      onClick={() =>
                        setSiteModal({
                          open: true,
                          editing: null,
                          defaultProtocol: "ftps_explicit",
                        })
                      }
                    >
                      <Server size={18} color="#2563eb" />
                      <h4>FTPS / FTP</h4>
                      <p>TLS 1.2 / TLS 1.3 现代加密传输（显式/隐式）</p>
                    </button>

                    <button
                      className="quick-proto-card"
                      onClick={() =>
                        setSiteModal({ open: true, editing: null, defaultProtocol: "webdav" })
                      }
                    >
                      <Globe size={18} color="#0891b2" />
                      <h4>WebDAV / 网盘</h4>
                      <p>Nextcloud · Alist · 群晖 NAS · Bearer Token</p>
                    </button>
                  </div>
                </div>
              </section>
            )}
          </div>

          {transferOpen && (
            <TransferDrawer
              tasks={transfers}
              onCancel={(id) => void api.cancelTransfer(id)}
              onClearFinished={async () => {
                await api.clearFinishedTransfers();
                setTransfers(await api.listTransfers());
              }}
              onClose={() => setTransferOpen(false)}
            />
          )}

          {terminalOpen && activeSession?.supportsShell && (
            <TerminalDrawer
              sessionId={activeSession.sessionId}
              sessionName={activeSession.name}
              cwd={remotePath}
              onClose={() => setTerminalOpen(false)}
            />
          )}
        </main>
      </div>

      {siteModal.open && (
        <SiteModal
          initial={siteModal.editing}
          defaultProtocol={siteModal.defaultProtocol}
          onClose={() => setSiteModal((s) => ({ ...s, open: false }))}
          onSaveAndConnect={handleSaveSite}
        />
      )}

      {editor && activeSession && (
        <EditorModal
          filePath={editor.path}
          initialContent={editor.content}
          onClose={() => setEditor(null)}
          onSave={async (nextContent) => {
            await api.remoteWriteText(activeSession.sessionId, editor.path, nextContent);
            await loadRemoteDir(activeSession.sessionId, remotePath);
            pushToast(`已保存并回传 ${editor.path}`, "success");
          }}
        />
      )}

      {toasts.length > 0 && (
        <div className="toast-container">
          {toasts.map((t) => (
            <div key={t.id} className={`toast ${t.kind}`}>
              {t.text}
            </div>
          ))}
        </div>
      )}
    </div>
  );
}
