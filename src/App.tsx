/**
 * xiaorui_FTP 主工作台组件
 *
 * 负责统筹全局主题、多标签远程会话、本地/远程双栏状态、系统原生拖拽上传监听、
 * 传输队列事件同步（含完成后自动刷新目录）、在线文本编辑器与内置 SSH 终端。
 */

import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { getCurrentWebview } from "@tauri-apps/api/webview";
import { open, save } from "@tauri-apps/plugin-dialog";
import {
  AlertCircle,
  ArrowDownUp,
  CheckCircle2,
  Cloud,
  Cpu,
  Globe,
  HardDrive,
  Info,
  Lock,
  MemoryStick,
  Minimize2,
  Server,
  ShieldCheck,
  Sparkles,
  X,
} from "lucide-react";
import { api } from "./api/tauri";
import { DialogModal, type DialogRequest } from "./components/DialogModal";
import { EditorModal } from "./components/EditorModal";
import { FilePane } from "./components/FilePane";
import { KnownHostsModal } from "./components/KnownHostsModal";
import { SiteModal } from "./components/SiteModal";
import { SiteSidebar } from "./components/SiteSidebar";
import { TerminalDrawer, type TerminalCommandRequest } from "./components/TerminalDrawer";
import { TopBar } from "./components/TopBar";
import { TransferDrawer } from "./components/TransferDrawer";
import {
  formatBytes,
  PROTOCOL_META,
  type FileEntry,
  type LocalRoot,
  type Protocol,
  type RemoteDisk,
  type ServerMetrics,
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
  const [terminalMaximized, setTerminalMaximized] = useState(false);
  const [terminalCommand, setTerminalCommand] = useState<TerminalCommandRequest | null>(null);

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

  // 跨平台确认/输入弹窗、SSH 已知主机管理弹窗、托盘偏好与远端硬件指标
  const [dialog, setDialog] = useState<DialogRequest | null>(null);
  const [knownHostsOpen, setKnownHostsOpen] = useState(false);
  const [minimizeToTray, setMinimizeToTray] = useState(true);
  const [serverMetrics, setServerMetrics] = useState<ServerMetrics | null>(null);

  // 可拖拽双栏宽度比例（左侧本地栏百分比）与命令栏聚焦引用
  const [splitRatio, setSplitRatio] = useState(46);
  const [resizingPanes, setResizingPanes] = useState(false);
  const panesRef = useRef<HTMLDivElement>(null);
  const quickConnectInputRef = useRef<HTMLInputElement>(null);

  const openConfirm = useCallback(
    (title: string, message: string, options?: { confirmLabel?: string; danger?: boolean }) =>
      new Promise<boolean>((resolve) => {
        setDialog({
          kind: "confirm",
          title,
          message,
          confirmLabel: options?.confirmLabel,
          danger: options?.danger,
          resolve,
        });
      }),
    []
  );

  const openPrompt = useCallback(
    (
      title: string,
      options?: {
        message?: string;
        defaultValue?: string;
        placeholder?: string;
        confirmLabel?: string;
      }
    ) =>
      new Promise<string | null>((resolve) => {
        setDialog({
          kind: "prompt",
          title,
          message: options?.message,
          defaultValue: options?.defaultValue,
          placeholder: options?.placeholder,
          confirmLabel: options?.confirmLabel,
          resolve,
        });
      }),
    []
  );

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

  // SFTP/SSH 会话激活时，自动采集远端服务器硬件规格、资源占用与多磁盘挂载点（每 15 秒轻量刷新）
  useEffect(() => {
    if (!activeSession?.supportsShell) {
      setServerMetrics(null);
      return;
    }
    let cancelled = false;
    const sid = activeSession.sessionId;
    const fetchMetrics = async () => {
      try {
        const m = await api.remoteServerMetrics(sid);
        if (!cancelled) setServerMetrics(m);
      } catch {
        // 受限 SFTP-only 账号不支持 exec 通道时静默忽略，不影响文件浏览
      }
    };
    void fetchMetrics();
    const timer = window.setInterval(() => {
      void fetchMetrics();
    }, 15000);
    return () => {
      cancelled = true;
      window.clearInterval(timer);
    };
  }, [activeSession?.sessionId, activeSession?.supportsShell]);

  /** 从远程文件面板一键在 SSH 终端进入指定目录 */
  const handleOpenDirInTerminal = useCallback((dirPath: string) => {
    setTransferOpen(false);
    setTerminalOpen(true);
    const isWinDrive = /^\/[A-Za-z]:/.test(dirPath);
    const cmd = isWinDrive
      ? `cd "${dirPath.slice(1).replace(/\//g, "\\")}"\r`
      : `cd '${dirPath.replace(/'/g, "'\\''")}'\r`;
    setTerminalCommand({ id: Date.now() + Math.random(), text: cmd });
  }, []);

  /** 从远程文件面板将文件/目录完整路径插入 SSH 终端当前光标位置 */
  const handleInsertPathToTerminal = useCallback((filePath: string) => {
    setTransferOpen(false);
    setTerminalOpen(true);
    const needsQuote = /\s|['"\\$`&;|<>()]/.test(filePath);
    const formatted = needsQuote ? `'${filePath.replace(/'/g, "'\\''")}' ` : `${filePath} `;
    setTerminalCommand({ id: Date.now() + Math.random(), text: formatted });
  }, []);

  /** 将当前 SSH 会话终端弹出为独立原生操作系统窗口 */
  const handlePopOutTerminal = async () => {
    if (!activeSession?.supportsShell) return;
    try {
      await api.openTerminalWindow(activeSession.sessionId, remotePath);
      setTerminalOpen(false);
      setTerminalMaximized(false);
      pushToast(`已打开独立终端窗口：${activeSession.name}`, "info");
    } catch (err) {
      pushToast(String(err), "error");
    }
  };

  /** 切换「关闭窗口时最小化到系统托盘」偏好并持久化 */
  const handleToggleMinimizeToTray = async () => {
    const next = !minimizeToTray;
    try {
      const updated = await api.setMinimizeToTray(next);
      setMinimizeToTray(updated.minimizeToTray);
      pushToast(
        updated.minimizeToTray
          ? "已开启：关闭窗口时将最小化到系统托盘"
          : "已关闭：点击关闭按钮将直接退出程序",
        "info"
      );
    } catch (err) {
      pushToast(String(err), "error");
    }
  };

  /** 根据当前远程路径匹配所属远端磁盘（最长前缀优先） */
  const activeRemoteDisk: RemoteDisk | null = useMemo(() => {
    const disks = serverMetrics?.disks;
    if (!disks || disks.length === 0) return null;
    const norm = remotePath || "/";
    let best: RemoteDisk | null = null;
    for (const d of disks) {
      const mp = d.mountPath.endsWith("/") && d.mountPath.length > 1
        ? d.mountPath.slice(0, -1)
        : d.mountPath;
      const matches =
        mp === "/" || norm === mp || norm.startsWith(`${mp}/`);
      if (matches && (!best || mp.length > best.mountPath.length)) {
        best = d;
      }
    }
    return best ?? disks[0];
  }, [serverMetrics, remotePath]);

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
        const [siteList, roots, home, existingTransfers, appSettings] = await Promise.all([
          api.listSites(),
          api.localListRoots(),
          api.localHomeDir(),
          api.listTransfers(),
          api.getAppSettings().catch(() => ({ minimizeToTray: true })),
        ]);
        setSites(siteList);
        setLocalRoots(roots);
        setTransfers(existingTransfers);
        setMinimizeToTray(appSettings.minimizeToTray);
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
              if (queued.length > 0) {
                setTransferOpen(true);
                pushToast(`已扫描并加入 ${queued.length} 个上传任务`, "success");
              } else {
                await loadRemoteDir(sid, targetDir);
                pushToast("已在远程创建空文件夹", "success");
              }
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
    let saved: SiteConfig;
    try {
      saved = await api.saveSite(input);
      const list = await api.listSites();
      setSites(list);
      pushToast(`已保存站点「${saved.name}」`, "success");
    } catch (err) {
      pushToast(String(err), "error");
      throw err;
    }

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
    const ok = await openConfirm(
      "删除站点",
      `确认删除站点「${site.name}」及其保存在系统凭据库中的密钥？`,
      { confirmLabel: "删除", danger: true }
    );
    if (!ok) return;
    try {
      await api.deleteSite(site.id);
      setSites((prev) => prev.filter((s) => s.id !== site.id));
      pushToast(`已删除站点「${site.name}」`);
    } catch (err) {
      pushToast(String(err), "error");
    }
  };

  const handleDisconnect = async (sessionId: string) => {
    try {
      await api.disconnectSession(sessionId);
    } catch (err) {
      pushToast(String(err), "error");
    }
    setSessions((prev) => {
      const next = prev.filter((s) => s.sessionId !== sessionId);
      if (activeSessionId === sessionId) {
        const fallback = next[0] ?? null;
        setActiveSessionId(fallback ? fallback.sessionId : null);
        if (fallback) {
          if (!fallback.supportsShell) setTerminalOpen(false);
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
      if (queued.length > 0) {
        setTransferOpen(true);
        pushToast(
          entry.isDir
            ? `已递归加入文件夹「${entry.name}」共 ${queued.length} 个文件上传`
            : `已加入上传队列：${entry.name}`,
          "success"
        );
      } else if (entry.isDir) {
        await loadRemoteDir(activeSession.sessionId, remotePath);
        pushToast(`已在远程创建空文件夹「${entry.name}」`, "success");
      }
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

      if (queued.length > 0) {
        setTransferOpen(true);
        pushToast(
          entry.isDir
            ? `已递归加入文件夹「${entry.name}」共 ${queued.length} 个文件下载`
            : `已加入下载队列：${entry.name}`,
          "success"
        );
      } else if (entry.isDir) {
        if (localPath) void loadLocalDir(localPath);
        pushToast(`已在本地创建空文件夹「${entry.name}」`, "success");
      }
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
      if (queued.length > 0) {
        setTransferOpen(true);
        pushToast(`已递归扫描并加入 ${queued.length} 个上传任务`, "success");
      } else {
        await loadRemoteDir(activeSession.sessionId, remotePath);
        pushToast("已在远程创建所选空文件夹", "success");
      }
    } catch (err) {
      pushToast(String(err), "error");
    }
  };

  // Remote actions
  const handleRemoteMkdir = async () => {
    if (!activeSession) return;
    const name = await openPrompt("新建远程文件夹", {
      placeholder: "请输入新文件夹名称",
      confirmLabel: "创建",
    });
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
    const nextName = await openPrompt("重命名远程项", {
      defaultValue: entry.name,
      confirmLabel: "重命名",
    });
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
    const ok = await openConfirm(
      "删除远程项",
      `确认删除远程${entry.isDir ? "目录（含全部子内容）" : "文件"}「${entry.name}」？此操作不可撤销。`,
      { confirmLabel: "删除", danger: true }
    );
    if (!ok) return;
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
    const input = await openPrompt(`修改「${entry.name}」权限 (CHMOD)`, {
      message: "请输入 3 位八进制权限值（例如 755 或 644）：",
      defaultValue: defaultOctal,
      confirmLabel: "应用",
    });
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
    const name = await openPrompt("新建本地文件夹", {
      placeholder: "请输入新文件夹名称",
      confirmLabel: "创建",
    });
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
    const nextName = await openPrompt("重命名本地项", {
      defaultValue: entry.name,
      confirmLabel: "重命名",
    });
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
    const ok = await openConfirm(
      "删除本地项",
      `确认删除本地${entry.isDir ? "目录（含全部子内容）" : "文件"}「${entry.name}」？此操作不可撤销。`,
      { confirmLabel: "删除", danger: true }
    );
    if (!ok) return;
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

  const totalTransferSpeed = transfers.reduce(
    (sum, t) => (t.status === "running" ? sum + t.speedBps : sum),
    0
  );

  const connectedKeys = useMemo(
    () => new Set(sessions.map((s) => `${s.protocol}:${s.host}`)),
    [sessions]
  );

  // 全局桌面快捷键：Ctrl+K 聚焦快速直连，Ctrl+N 新建站点，F5 刷新目录
  useEffect(() => {
    const anyModalOpen = siteModal.open || Boolean(editor) || Boolean(dialog) || knownHostsOpen;
    const onKeyDown = (e: KeyboardEvent) => {
      if ((e.ctrlKey || e.metaKey) && e.key.toLowerCase() === "k" && !anyModalOpen) {
        e.preventDefault();
        quickConnectInputRef.current?.focus();
        quickConnectInputRef.current?.select();
      } else if ((e.ctrlKey || e.metaKey) && e.key.toLowerCase() === "n" && !anyModalOpen) {
        e.preventDefault();
        setSiteModal({ open: true, editing: null, defaultProtocol: "sftp" });
      } else if (e.key === "F5" && !anyModalOpen) {
        e.preventDefault();
        if (activeSessionIdRef.current) {
          void loadRemoteDir(activeSessionIdRef.current, remotePathRef.current);
        } else if (localPathRef.current) {
          void loadLocalDir(localPathRef.current);
        }
      }
    };
    window.addEventListener("keydown", onKeyDown);
    return () => window.removeEventListener("keydown", onKeyDown);
  }, [dialog, editor, knownHostsOpen, loadLocalDir, loadRemoteDir, siteModal.open]);

  // 拖拽双栏分割条实时调整左右宽度
  const handleStartResize = (e: React.MouseEvent) => {
    e.preventDefault();
    setResizingPanes(true);
    const onMove = (ev: MouseEvent) => {
      const rect = panesRef.current?.getBoundingClientRect();
      if (!rect || rect.width <= 0) return;
      const pct = ((ev.clientX - rect.left) / rect.width) * 100;
      setSplitRatio(Math.min(76, Math.max(24, pct)));
    };
    const onUp = () => {
      setResizingPanes(false);
      window.removeEventListener("mousemove", onMove);
      window.removeEventListener("mouseup", onUp);
    };
    window.addEventListener("mousemove", onMove);
    window.addEventListener("mouseup", onUp);
  };

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
        quickConnectInputRef={quickConnectInputRef}
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
            connectedKeys={connectedKeys}
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
            onOpenKnownHosts={() => setKnownHostsOpen(true)}
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
                      if (!s.supportsShell) setTerminalOpen(false);
                      void loadRemoteDir(s.sessionId, s.currentPath);
                    }}
                  >
                    <span className="status-dot online" />
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

          <div
            className="panes"
            ref={panesRef}
            style={terminalOpen && terminalMaximized ? { display: "none" } : undefined}
          >
            {dualPane && (
              <FilePane
                mode="local"
                title="本地"
                currentPath={localPath}
                entries={localEntries}
                loading={localLoading}
                roots={localRoots}
                canTransfer={Boolean(activeSession)}
                style={{ flex: `0 0 ${splitRatio}%` }}
                onNavigate={(p) => void loadLocalDir(p)}
                onRefresh={() => void loadLocalDir(localPath)}
                onMkdir={() => void handleLocalMkdir()}
                onRename={(e) => void handleLocalRename(e)}
                onDelete={(e) => void handleLocalDelete(e)}
                onTransfer={(e) => void handleUploadEntry(e)}
                onCopyPath={(p) => pushToast(`已复制路径：${p}`, "info")}
                onDropEntryFromOtherPane={(e) => void handleDownloadEntry(e, false)}
              />
            )}

            {dualPane && (
              <div
                className={`pane-resizer ${resizingPanes ? "dragging" : ""}`}
                onMouseDown={handleStartResize}
                title="拖动调节本地与远程分栏比例（双击重置）"
                onDoubleClick={() => setSplitRatio(46)}
              />
            )}

            {activeSession ? (
              <FilePane
                mode="remote"
                title={`${PROTOCOL_META[activeSession.protocol].short} · ${activeSession.host}`}
                currentPath={remotePath}
                entries={remoteEntries}
                loading={remoteLoading}
                remoteDisks={serverMetrics?.disks}
                canTransfer={true}
                supportsChmod={activeSession.supportsChmod}
                externalDropActive={osDragOver}
                onNavigate={(p) => void loadRemoteDir(activeSession.sessionId, p)}
                onRefresh={() => void loadRemoteDir(activeSession.sessionId, remotePath)}
                onMkdir={() => void handleRemoteMkdir()}
                onRename={(e) => void handleRemoteRename(e)}
                onDelete={(e) => void handleRemoteDelete(e)}
                onTransfer={(e) => void handleDownloadEntry(e, true)}
                onCopyPath={(p) => pushToast(`已复制远程路径：${p}`, "info")}
                onUploadFromPicker={() => void handleUploadFromPicker()}
                onUploadFolderFromPicker={() => void handleUploadFolderFromPicker()}
                onDropEntryFromOtherPane={(e) => void handleUploadEntry(e)}
                onEditFile={(e) => void handleOpenEditor(e)}
                onChmod={(e) => void handleRemoteChmod(e)}
                onOpenInTerminal={
                  activeSession.supportsShell ? handleOpenDirInTerminal : undefined
                }
                onInsertPathToTerminal={
                  activeSession.supportsShell ? handleInsertPathToTerminal : undefined
                }
              />
            ) : (
              <section className="pane">
                <div className="empty-state">
                  <div className="brand-mark" style={{ width: 48, height: 48, borderRadius: 13 }}>
                    <Server size={24} />
                  </div>
                  <div className="empty-title">极简、安全的现代文件传输工作台</div>
                  <p className="empty-desc">
                    选择一种协议快速新建连接，或按 <kbd className="kbd-hint">Ctrl K</kbd> 在顶部直连栏输入 URI 一键连接。纯 Rust 原生 x64 引擎，零 OpenSSL 依赖。
                  </p>

                  <div className="quick-proto-grid">
                    <button
                      className="quick-proto-card"
                      onClick={() =>
                        setSiteModal({ open: true, editing: null, defaultProtocol: "sftp" })
                      }
                    >
                      <div
                        className="quick-proto-icon"
                        style={{ background: "rgba(16, 185, 129, 0.14)", color: "#059669" }}
                      >
                        <Lock size={17} />
                      </div>
                      <h4>SFTP / SSH 终端</h4>
                      <p>Ed25519 · ProxyJump 跳板机 · TOFU 指纹校验</p>
                    </button>

                    <button
                      className="quick-proto-card"
                      onClick={() =>
                        setSiteModal({ open: true, editing: null, defaultProtocol: "s3" })
                      }
                    >
                      <div
                        className="quick-proto-icon"
                        style={{ background: "rgba(124, 58, 237, 0.14)", color: "#7c3aed" }}
                      >
                        <Cloud size={17} />
                      </div>
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
                      <div
                        className="quick-proto-icon"
                        style={{ background: "rgba(37, 99, 235, 0.14)", color: "#2563eb" }}
                      >
                        <Server size={17} />
                      </div>
                      <h4>FTPS / FTP</h4>
                      <p>纯 Rust TLS 1.2 / TLS 1.3 现代加密传输</p>
                    </button>

                    <button
                      className="quick-proto-card"
                      onClick={() =>
                        setSiteModal({ open: true, editing: null, defaultProtocol: "webdav" })
                      }
                    >
                      <div
                        className="quick-proto-icon"
                        style={{ background: "rgba(8, 145, 178, 0.14)", color: "#0891b2" }}
                      >
                        <Globe size={17} />
                      </div>
                      <h4>WebDAV / 网盘</h4>
                      <p>Nextcloud · Alist · 群晖 NAS · Bearer Token</p>
                    </button>
                  </div>

                  <div className="empty-tips">
                    <span className="empty-tip-pill">
                      <ShieldCheck size={12} color="var(--success)" />
                      系统原生凭据库托管
                    </span>
                    <span className="empty-tip-pill">
                      <Sparkles size={12} color="var(--accent)" />
                      支持系统文件夹拖放递归上传
                    </span>
                  </div>
                </div>
              </section>
            )}
          </div>

          {transferOpen && (
            <TransferDrawer
              tasks={transfers}
              onCancel={async (id) => {
                try {
                  await api.cancelTransfer(id);
                } catch (err) {
                  pushToast(String(err), "error");
                }
              }}
              onClearFinished={async () => {
                try {
                  await api.clearFinishedTransfers();
                  setTransfers(await api.listTransfers());
                } catch (err) {
                  pushToast(String(err), "error");
                }
              }}
              onClose={() => setTransferOpen(false)}
            />
          )}

          {terminalOpen && activeSession?.supportsShell && (
            <TerminalDrawer
              key={activeSession.sessionId}
              sessionId={activeSession.sessionId}
              sessionName={activeSession.name}
              cwd={remotePath}
              maximized={terminalMaximized}
              pendingCommand={terminalCommand}
              onToggleMaximize={() => setTerminalMaximized((v) => !v)}
              onPopOut={() => void handlePopOutTerminal()}
              onNotify={pushToast}
              onClose={() => {
                setTerminalOpen(false);
                setTerminalMaximized(false);
              }}
            />
          )}
        </main>
      </div>

      {/* 底部现代桌面状态栏（含远端硬件规格/占用、多磁盘快切与托盘模式开关） */}
      <footer className="status-bar">
        <span className="status-bar-item" title={serverMetrics?.osName || undefined}>
          <span className={`status-dot ${activeSession ? "online" : ""}`} />
          {activeSession
            ? `${PROTOCOL_META[activeSession.protocol].short} · ${activeSession.host}`
            : "就绪 · 未连接远程站点"}
        </span>

        {activeSession?.supportsShell && serverMetrics && (
          <div className="metrics-strip">
            <span
              className={`metric-pill tabular-nums ${
                (serverMetrics.cpuUsagePct ?? 0) >= 85
                  ? "hot"
                  : (serverMetrics.cpuUsagePct ?? 0) >= 65
                    ? "warn"
                    : ""
              }`}
              title={`系统：${serverMetrics.osName}${serverMetrics.loadAvg ? ` · 1分钟负载：${serverMetrics.loadAvg}` : ""}`}
            >
              <Cpu size={11} />
              {serverMetrics.cpuCores > 0 ? `${serverMetrics.cpuCores}C` : "CPU"}
              {serverMetrics.cpuUsagePct != null
                ? ` ${Math.round(serverMetrics.cpuUsagePct)}%`
                : serverMetrics.loadAvg
                  ? ` L:${serverMetrics.loadAvg}`
                  : ""}
            </span>

            {serverMetrics.memTotalBytes > 0 && (() => {
              const memPct = Math.min(
                100,
                Math.round((serverMetrics.memUsedBytes / serverMetrics.memTotalBytes) * 100)
              );
              return (
                <span
                  className={`metric-pill tabular-nums ${
                    memPct >= 88 ? "hot" : memPct >= 72 ? "warn" : ""
                  }`}
                  title={`远端内存：已用 ${formatBytes(serverMetrics.memUsedBytes)} / 总计 ${formatBytes(serverMetrics.memTotalBytes)} (${memPct}%)`}
                >
                  <MemoryStick size={11} />
                  {formatBytes(serverMetrics.memUsedBytes)}/{formatBytes(serverMetrics.memTotalBytes)} ({memPct}%)
                </span>
              );
            })()}

            {activeRemoteDisk && (
              <button
                type="button"
                className={`metric-pill metric-pill-btn tabular-nums ${
                  activeRemoteDisk.usePercent >= 90
                    ? "hot"
                    : activeRemoteDisk.usePercent >= 78
                      ? "warn"
                      : ""
                }`}
                onClick={() => {
                  const disks = serverMetrics.disks;
                  if (disks.length <= 1) {
                    void loadRemoteDir(activeSession.sessionId, activeRemoteDisk.mountPath);
                    return;
                  }
                  const idx = disks.findIndex((d) => d.mountPath === activeRemoteDisk.mountPath);
                  const next = disks[(idx + 1) % disks.length];
                  void loadRemoteDir(activeSession.sessionId, next.mountPath);
                  pushToast(`已切换至远端磁盘 ${next.label} (${next.mountPath})`, "info");
                }}
                title={
                  `远端磁盘（点击${serverMetrics.disks.length > 1 ? "切换下一块磁盘" : "进入挂载根目录"}）：\n` +
                  serverMetrics.disks
                    .map(
                      (d) =>
                        `${d.label}  ${d.usePercent}%  (${formatBytes(d.usedBytes)} / ${formatBytes(d.totalBytes)}，剩余 ${formatBytes(d.availBytes)})`
                    )
                    .join("\n")
                }
              >
                <HardDrive size={11} />
                {activeRemoteDisk.label} {activeRemoteDisk.usePercent}%
                {serverMetrics.disks.length > 1 ? ` · 共${serverMetrics.disks.length}盘` : ""}
              </button>
            )}
          </div>
        )}

        <div className="status-bar-right">
          <span className="status-bar-item tabular-nums">
            {dualPane ? `本地 ${localEntries.length}` : ""}
            {dualPane && activeSession ? " / " : ""}
            {activeSession ? `远程 ${remoteEntries.length}` : ""}
          </span>

          <button
            type="button"
            className="status-bar-btn tabular-nums"
            onClick={() => {
              setTransferOpen((v) => !v);
              if (!transferOpen) setTerminalOpen(false);
            }}
            title="展开或收起传输队列"
          >
            <ArrowDownUp size={12} color={activeTransferCount > 0 ? "var(--accent)" : undefined} />
            {activeTransferCount > 0
              ? `传输中 ${activeTransferCount}${totalTransferSpeed > 0 ? ` · ${formatBytes(totalTransferSpeed)}/s` : ""}`
              : `队列 (${transfers.length})`}
          </button>

          <button
            type="button"
            className={`status-bar-btn ${minimizeToTray ? "active" : ""}`}
            onClick={() => void handleToggleMinimizeToTray()}
            title={
              minimizeToTray
                ? "当前：点击关闭按钮将最小化到系统托盘（点击切换为直接退出）"
                : "当前：点击关闭按钮将直接退出程序（点击开启关闭最小化到托盘）"
            }
          >
            <Minimize2 size={11} />
            {minimizeToTray ? "关闭→托盘" : "关闭→退出"}
          </button>
        </div>
      </footer>

      {siteModal.open && (
        <SiteModal
          key={siteModal.editing?.id ?? siteModal.defaultProtocol}
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
            try {
              await api.remoteWriteText(activeSession.sessionId, editor.path, nextContent);
              await loadRemoteDir(activeSession.sessionId, remotePath);
              pushToast(`已保存并回传 ${editor.path}`, "success");
            } catch (err) {
              pushToast(String(err), "error");
              throw err;
            }
          }}
        />
      )}

      {knownHostsOpen && (
        <KnownHostsModal
          onClose={() => setKnownHostsOpen(false)}
          onNotify={pushToast}
        />
      )}

      {dialog && (
        <DialogModal dialog={dialog} onClose={() => setDialog(null)} />
      )}

      {toasts.length > 0 && (
        <div className="toast-container">
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
