/**
 * Tauri IPC 类型安全封装层
 *
 * 集中管理所有前后端命令调用与事件订阅，避免业务组件直接拼接字符串命令名。
 */

import { invoke } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";
import type {
  AgentStatus,
  AppSettings,
  FileEntry,
  KnownHostEntry,
  LocalRoot,
  PtyDataEvent,
  PtyExitEvent,
  ServerMetrics,
  SessionInfo,
  ShellOutput,
  SiteConfig,
  SiteInput,
  TransferTask,
} from "../types";

export const api = {
  // ---------------- 站点管理与凭据检测 ----------------
  listSites: () => invoke<SiteConfig[]>("list_sites"),
  saveSite: (input: SiteInput) => invoke<SiteConfig>("save_site", { input }),
  deleteSite: (siteId: string) => invoke<void>("delete_site", { siteId }),
  exportSitesToFile: (filePath: string) =>
    invoke<void>("export_sites_to_file", { filePath }),
  importSitesFromFile: (filePath: string) =>
    invoke<number>("import_sites_from_file", { filePath }),
  checkSshAgentStatus: () => invoke<AgentStatus>("check_ssh_agent_status"),
  listKnownHosts: () => invoke<KnownHostEntry[]>("list_known_hosts"),
  removeKnownHost: (hostPort: string) =>
    invoke<boolean>("remove_known_host", { hostPort }),
  getAppSettings: () => invoke<AppSettings>("get_app_settings"),
  setMinimizeToTray: (enabled: boolean) =>
    invoke<AppSettings>("set_minimize_to_tray", { enabled }),

  // ---------------- 会话连接管理 ----------------
  connectSavedSite: (
    siteId: string,
    overrideSecret?: string,
    overridePassphrase?: string,
    overrideJumpSecret?: string,
    overrideJumpPassphrase?: string
  ) =>
    invoke<SessionInfo>("connect_saved_site", {
      siteId,
      overrideSecret: overrideSecret || null,
      overridePassphrase: overridePassphrase || null,
      overrideJumpSecret: overrideJumpSecret || null,
      overrideJumpPassphrase: overrideJumpPassphrase || null,
    }),
  connectAdhoc: (input: SiteInput) => invoke<SessionInfo>("connect_adhoc", { input }),
  quickConnectUri: (uri: string) => invoke<SessionInfo>("quick_connect_uri", { uri }),
  disconnectSession: (sessionId: string) =>
    invoke<void>("disconnect_session", { sessionId }),
  listSessions: () => invoke<SessionInfo[]>("list_sessions"),

  // ---------------- 远程文件系统 ----------------
  remoteListDir: (sessionId: string, path: string) =>
    invoke<FileEntry[]>("remote_list_dir", { sessionId, path }),
  remoteMkdir: (sessionId: string, path: string) =>
    invoke<void>("remote_mkdir", { sessionId, path }),
  remoteDelete: (sessionId: string, path: string, isDir: boolean) =>
    invoke<void>("remote_delete", { sessionId, path, isDir }),
  remoteRename: (sessionId: string, from: string, to: string) =>
    invoke<void>("remote_rename", { sessionId, from, to }),
  remoteChmod: (sessionId: string, path: string, mode: number) =>
    invoke<void>("remote_chmod", { sessionId, path, mode }),
  remoteReadText: (sessionId: string, path: string) =>
    invoke<string>("remote_read_text", { sessionId, path }),
  remoteWriteText: (sessionId: string, path: string, content: string) =>
    invoke<void>("remote_write_text", { sessionId, path, content }),

  // ---------------- 本地文件系统 ----------------
  localHomeDir: () => invoke<string>("local_home_dir"),
  localListRoots: () => invoke<LocalRoot[]>("local_list_roots"),
  localListDir: (path: string) => invoke<FileEntry[]>("local_list_dir", { path }),
  localMkdir: (path: string) => invoke<void>("local_mkdir", { path }),
  localDelete: (path: string, isDir: boolean) =>
    invoke<void>("local_delete", { path, isDir }),
  localRename: (from: string, to: string) =>
    invoke<void>("local_rename", { from, to }),

  // ---------------- 并发传输队列 ----------------
  enqueueUpload: (sessionId: string, localPath: string, remoteDir: string) =>
    invoke<TransferTask[]>("enqueue_upload", { sessionId, localPath, remoteDir }),
  enqueueUploadPaths: (sessionId: string, localPaths: string[], remoteDir: string) =>
    invoke<TransferTask[]>("enqueue_upload_paths", { sessionId, localPaths, remoteDir }),
  enqueueDownload: (
    sessionId: string,
    remotePath: string,
    options: {
      localDir?: string;
      localFilePath?: string;
      isDir?: boolean;
      sizeHint?: number;
    }
  ) =>
    invoke<TransferTask[]>("enqueue_download", {
      request: {
        sessionId,
        remotePath,
        localDir: options.localDir ?? null,
        localFilePath: options.localFilePath ?? null,
        isDir: options.isDir ?? false,
        sizeHint: options.sizeHint ?? null,
      },
    }),
  listTransfers: () => invoke<TransferTask[]>("list_transfers"),
  cancelTransfer: (taskId: string) => invoke<void>("cancel_transfer", { taskId }),
  clearFinishedTransfers: () => invoke<void>("clear_finished_transfers"),
  /** 订阅后端推送的传输任务实时进度与状态变更事件 */
  onTransferUpdate: (cb: (task: TransferTask) => void): Promise<UnlistenFn> =>
    listen<TransferTask>("transfer://update", (ev) => cb(ev.payload)),

  // ---------------- SSH 远程终端、交互式 PTY 与服务器硬件监测 ----------------
  sshExecCommand: (sessionId: string, command: string, cwd?: string) =>
    invoke<ShellOutput>("ssh_exec_command", {
      sessionId,
      command,
      cwd: cwd ?? null,
    }),
  remoteServerMetrics: (sessionId: string) =>
    invoke<ServerMetrics>("remote_server_metrics", { sessionId }),
  sshPtyOpen: (sessionId: string, cols: number, rows: number) =>
    invoke<string>("ssh_pty_open", { sessionId, cols, rows }),
  sshPtyWrite: (terminalId: string, data: string) =>
    invoke<void>("ssh_pty_write", { terminalId, data }),
  sshPtyResize: (terminalId: string, cols: number, rows: number) =>
    invoke<void>("ssh_pty_resize", { terminalId, cols, rows }),
  sshPtyClose: (terminalId: string) =>
    invoke<void>("ssh_pty_close", { terminalId }),
  onPtyData: (cb: (ev: PtyDataEvent) => void): Promise<UnlistenFn> =>
    listen<PtyDataEvent>("pty://data", (ev) => cb(ev.payload)),
  onPtyExit: (cb: (ev: PtyExitEvent) => void): Promise<UnlistenFn> =>
    listen<PtyExitEvent>("pty://exit", (ev) => cb(ev.payload)),
};
