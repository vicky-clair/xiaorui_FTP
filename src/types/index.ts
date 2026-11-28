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

/** 已信任的 SSH 主机公钥指纹记录 */
export interface KnownHostEntry {
  hostPort: string;
  algorithm: string;
  fingerprint: string;
  firstSeenAt: number;
  lastSeenAt: number;
}

/** 首次连接未知主机时的待确认指纹信息 */
export interface HostKeyUnknown {
  hostPort: string;
  algorithm: string;
  fingerprint: string;
  isJumpHost: boolean;
}

/** SSH 主机公钥指纹发生变更时的阻断详情 */
export interface HostKeyMismatch {
  hostPort: string;
  expectedAlgorithm: string;
  expectedFingerprint: string;
  actualAlgorithm: string;
  actualFingerprint: string;
  isJumpHost: boolean;
}

/** 本地安全审计事件严重级别 */
export type AuditSeverity = "info" | "warn" | "high" | "critical";

/** 本地安全审计事件分类 */
export type AuditCategory =
  | "host_key"
  | "session"
  | "credential"
  | "file_op"
  | "security_policy";

/** 单条脱敏安全审计日志记录 */
export interface AuditEntry {
  timestamp: number;
  severity: AuditSeverity;
  category: AuditCategory;
  event: string;
  target: string;
  detail: string;
}

/** 本地 SSH 密钥元数据（不含私钥明文） */
export interface SshKeyEntry {
  name: string;
  privateKeyPath?: string | null;
  publicKeyPath: string;
  algorithm: string;
  fingerprint: string;
  comment: string;
  publicKeyLine: string;
  hasPrivateKey: boolean;
}

/** 从 `~/.ssh/config` 解析出的主机条目 */
export interface SshConfigHost {
  alias: string;
  host: string;
  port: number;
  username: string;
  privateKeyPath?: string | null;
  jumpHost?: string | null;
  jumpPort?: number | null;
  jumpUsername?: string | null;
}

/** 本机可用的本地终端 Shell 预设 */
export interface LocalShellOption {
  id: string;
  label: string;
}

/** 从 Windows Terminal / 系统默认读取的终端配色方案 */
export interface TerminalColorScheme {
  background: string;
  foreground: string;
  cursor?: string | null;
  selectionBackground?: string | null;
  black?: string | null;
  red?: string | null;
  green?: string | null;
  yellow?: string | null;
  blue?: string | null;
  magenta?: string | null;
  cyan?: string | null;
  white?: string | null;
  brightBlack?: string | null;
  brightRed?: string | null;
  brightGreen?: string | null;
  brightYellow?: string | null;
  brightBlue?: string | null;
  brightMagenta?: string | null;
  brightCyan?: string | null;
  brightWhite?: string | null;
}

/** 当前操作系统的默认本地终端配置（Windows 下自动同步 Windows Terminal 默认配置） */
export interface LocalTerminalDefaults {
  profileId: string;
  profileLabel: string;
  source: string;
  fontFamily?: string | null;
  fontSize?: number | null;
  cursorStyle?: "bar" | "block" | "underline" | string | null;
  schemeName?: string | null;
  theme?: TerminalColorScheme | null;
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
  /** 是否使用明文传输协议（如 FTP / HTTP） */
  insecureTransport?: boolean;
  /** 是否跳过了 TLS 证书校验 */
  tlsSkipVerify?: boolean;
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

/** 交互式 SSH PTY 输出流事件（Base64 原始字节） */
export interface PtyDataEvent {
  terminalId: string;
  dataB64: string;
}

/** 交互式 SSH PTY 退出事件 */
export interface PtyExitEvent {
  terminalId: string;
}

/** 本地快捷位置或盘符项 */
export interface LocalRoot {
  name: string;
  path: string;
}

/** 应用轻量偏好设置 */
export interface AppSettings {
  minimizeToTray: boolean;
}

/** 远端服务器物理磁盘 / 挂载点信息 */
export interface RemoteDisk {
  label: string;
  mountPath: string;
  totalBytes: number;
  usedBytes: number;
  availBytes: number;
  usePercent: number;
}

/** 远端或本机单个网络接口实时流量与地址信息 */
export interface NetInterface {
  name: string;
  ip?: string | null;
  rxBytesPerSec: number;
  txBytesPerSec: number;
}

/** 活跃 SSH 本地端口转发隧道摘要 */
export interface SshTunnelInfo {
  id: string;
  sessionId: string;
  localPort: number;
  remoteHost: string;
  remotePort: number;
  activeConnections: number;
  createdAt: number;
}

/** 远端或本机服务器硬件规格与实时资源占用快照（支持 Linux / Windows / macOS） */
export interface ServerMetrics {
  osFamily: "linux" | "windows" | "macos" | string;
  osName: string;
  hostname?: string | null;
  uptimeSecs?: number | null;
  cpuCores: number;
  cpuUsagePct?: number | null;
  cpuUserPct?: number | null;
  cpuSysPct?: number | null;
  loadAvg?: string | null;
  memUsedBytes: number;
  memTotalBytes: number;
  swapUsedBytes: number;
  swapTotalBytes: number;
  gpuName?: string | null;
  gpuUsagePct?: number | null;
  gpuMemUsedMb?: number | null;
  gpuMemTotalMb?: number | null;
  defaultGateway?: string | null;
  netRxBps: number;
  netTxBps: number;
  netInterfaces: NetInterface[];
  disks: RemoteDisk[];
  timestamp: number;
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
