/**
 * 前端与 Rust 后端共享的数据类型定义及常用格式化工具
 */

/** 支持的远程协议类型 */
export type Protocol =
  | "sftp"
  | "ftps_explicit"
  | "ftps_implicit"
  | "ftp"
  | "s3"
  | "webdav";

/** 支持的身份认证方式 */
export type AuthType =
  | "password"
  | "private_key"
  | "agent"
  | "access_key"
  | "token"
  | "anonymous";

/** 已保存站点元数据（不含明文密钥） */
export interface SiteConfig {
  id: string;
  name: string;
  group: string;
  protocol: Protocol;
  host: string;
  port: number;
  username: string;
  authType: AuthType;
  privateKeyPath?: string | null;
  remotePath: string;
  endpoint?: string | null;
  bucket?: string | null;
  region?: string | null;
  pathStyle: boolean;
  tlsSkipVerify: boolean;
  jumpHost?: string | null;
  jumpPort?: number | null;
  jumpUsername?: string | null;
  jumpAuthType?: AuthType | null;
  jumpPrivateKeyPath?: string | null;
  notes: string;
  hasSavedSecret: boolean;
  updatedAt: number;
}

/** 新建/编辑站点或临时连接时提交的表单参数 */
export interface SiteInput {
  id?: string | null;
  name: string;
  group: string;
  protocol: Protocol;
  host: string;
  port?: number | null;
  username: string;
  authType: AuthType;
  secret?: string | null;
  passphrase?: string | null;
  sessionToken?: string | null;
  privateKeyPath?: string | null;
  remotePath: string;
  endpoint?: string | null;
  bucket?: string | null;
  region?: string | null;
  pathStyle: boolean;
  tlsSkipVerify: boolean;
  jumpHost?: string | null;
  jumpPort?: number | null;
  jumpUsername?: string | null;
  jumpAuthType?: AuthType | null;
  jumpPrivateKeyPath?: string | null;
  jumpSecret?: string | null;
  jumpPassphrase?: string | null;
  notes: string;
  saveSecret: boolean;
}

/** 本地 SSH Agent 检测结果 */
export interface AgentStatus {
  available: boolean;
  backend: string;
  keyCount: number;
  message: string;
}

/** 当前已连接的远程会话摘要 */
export interface SessionInfo {
  sessionId: string;
  siteId?: string | null;
  name: string;
  protocol: Protocol;
  host: string;
  port: number;
  currentPath: string;
  supportsShell: boolean;
  supportsChmod: boolean;
}

/** 统一本地/远程文件与目录条目 */
export interface FileEntry {
  name: string;
  path: string;
  isDir: boolean;
  size: number;
  modified?: number | null;
  permissions?: string | null;
  mode?: number | null;
  isSymlink: boolean;
}

export type TransferDirection = "upload" | "download";
export type TransferStatus =
  | "queued"
  | "running"
  | "completed"
  | "failed"
  | "cancelled";

/** 传输队列中的单个文件任务 */
export interface TransferTask {
  id: string;
  sessionId: string;
  direction: TransferDirection;
  localPath: string;
  remotePath: string;
  fileName: string;
  totalBytes: number;
  transferredBytes: number;
  speedBps: number;
  status: TransferStatus;
  error?: string | null;
  createdAt: number;
}

/** SSH 远程终端命令执行输出 */
export interface ShellOutput {
  stdout: string;
  stderr: string;
  exitCode: number;
}

/** 本地快捷位置或盘符项 */
export interface LocalRoot {
  name: string;
  path: string;
}

/** 各协议的展示名称、徽章样式与默认端口配置 */
export const PROTOCOL_META: Record<
  Protocol,
  { label: string; short: string; defaultPort: number; badgeClass: string }
> = {
  sftp: { label: "SFTP (SSH)", short: "SFTP", defaultPort: 22, badgeClass: "badge-sftp" },
  ftps_explicit: { label: "FTPS (显式 TLS)", short: "FTPS", defaultPort: 21, badgeClass: "badge-ftps" },
  ftps_implicit: { label: "FTPS (隐式 TLS)", short: "FTPS-I", defaultPort: 990, badgeClass: "badge-ftps" },
  ftp: { label: "FTP (明文)", short: "FTP", defaultPort: 21, badgeClass: "badge-ftp" },
  s3: { label: "S3 兼容对象存储", short: "S3", defaultPort: 443, badgeClass: "badge-s3" },
  webdav: { label: "WebDAV / HTTPS", short: "DAV", defaultPort: 443, badgeClass: "badge-webdav" },
};

/** 将字节数格式化为易读的文件大小字符串（B / KB / MB / GB / TB） */
export function formatBytes(bytes: number): string {
  if (!bytes || bytes <= 0) return "0 B";
  const units = ["B", "KB", "MB", "GB", "TB"];
  const idx = Math.min(Math.floor(Math.log(bytes) / Math.log(1024)), units.length - 1);
  const val = bytes / Math.pow(1024, idx);
  return `${val >= 100 || idx === 0 ? val.toFixed(0) : val.toFixed(1)} ${units[idx]}`;
}

/** 将 Unix 秒级时间戳格式化为 `YYYY-MM-DD HH:mm` */
export function formatTime(unixSecs?: number | null): string {
  if (!unixSecs) return "—";
  const d = new Date(unixSecs * 1000);
  const pad = (n: number) => String(n).padStart(2, "0");
  return `${d.getFullYear()}-${pad(d.getMonth() + 1)}-${pad(d.getDate())} ${pad(d.getHours())}:${pad(d.getMinutes())}`;
}
