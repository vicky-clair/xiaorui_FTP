/**
 * 右侧直观系统监控与当前目录概览面板
 *
 * 支持远端 Linux / Windows SSH 服务器及本机 Windows 实时监控：
 * - 顶部分类视图：综合 / 处理器 / 显卡 / 内存 / 磁盘 / 网络
 * - 主机摘要卡（主机名、IP、用户名、OS、运行时间）
 * - 2×2 核心指标卡片（CPU User/Sys 占比、内存、磁盘、实时上下行网速与网关）
 * - CPU 负载趋势实时平滑折线图（1分钟 / 5分钟 / 15分钟窗口）
 * - 磁盘分区进度条列表（点击可直接跳转对应目录）与网络接口明细
 * - 底部当前目录 4 格统计（PATH / DIR / FILE / TOTAL）
 */

import React, { useMemo, useState } from "react";
import {
  Activity,
  Cpu,
  FileText,
  Folder,
  FolderOpen,
  HardDrive,
  Layers,
  MemoryStick,
  Monitor,
  PanelRightClose,
  RefreshCw,
  Server,
  Wifi,
} from "lucide-react";
import { formatBytes, type FileEntry, type ServerMetrics } from "../types";

export interface CpuTrendPoint {
  ts: number;
  cpu: number;
}

interface MonitorPanelProps {
  mode: "remote" | "local";
  title: string;
  hostLabel: string;
  userLabel?: string;
  metrics: ServerMetrics | null;
  loading?: boolean;
  cpuHistory: CpuTrendPoint[];
  currentPath: string;
  entries: FileEntry[];
  onRefresh: () => void;
  onNavigateDisk?: (mountPath: string) => void;
  onClose: () => void;
}

type MonitorTab = "overview" | "cpu" | "gpu" | "memory" | "disk" | "network";
type TrendWindow = 60 | 300 | 900;

/** 将运行秒数格式化为 `16d 12h` / `4h 22m` / `18m` */
function formatUptime(secs?: number | null): string {
  if (!secs || secs <= 0) return "—";
  const d = Math.floor(secs / 86400);
  const h = Math.floor((secs % 86400) / 3600);
  const m = Math.floor((secs % 3600) / 60);
  if (d > 0) return `${d}d ${h}h`;
  if (h > 0) return `${h}h ${m}m`;
  return `${Math.max(1, m)}m`;
}

/** 格式化实时网速（B/s、KB/s、MB/s） */
function formatSpeed(bps: number): string {
  if (!bps || bps <= 0) return "0 KB/s";
  if (bps < 1024) return `${bps} B/s`;
  if (bps < 1024 * 1024) return `${Math.round(bps / 1024)} KB/s`;
  return `${(bps / (1024 * 1024)).toFixed(1)} MB/s`;
}

function formatClock(ts: number): string {
  const d = new Date(ts * 1000);
  const pad = (n: number) => String(n).padStart(2, "0");
  return `${pad(d.getHours())}:${pad(d.getMinutes())}:${pad(d.getSeconds())}`;
}

export const MonitorPanel: React.FC<MonitorPanelProps> = ({
  mode,
  title,
  hostLabel,
  userLabel,
  metrics,
  loading = false,
  cpuHistory,
  currentPath,
  entries,
  onRefresh,
  onNavigateDisk,
  onClose,
}) => {
  const [tab, setTab] = useState<MonitorTab>("overview");
  const [trendWindow, setTrendWindow] = useState<TrendWindow>(300);

  const dirStats = useMemo(() => {
    let dirs = 0;
    let files = 0;
    let totalBytes = 0;
    for (const e of entries) {
      if (e.isDir) dirs += 1;
      else {
        files += 1;
        totalBytes += e.size || 0;
      }
    }
    return { dirs, files, totalBytes };
  }, [entries]);

  const memPct =
    metrics && metrics.memTotalBytes > 0
      ? Math.min(100, Math.round((metrics.memUsedBytes / metrics.memTotalBytes) * 100))
      : 0;

  const swapPct =
    metrics && metrics.swapTotalBytes > 0
      ? Math.min(100, Math.round((metrics.swapUsedBytes / metrics.swapTotalBytes) * 100))
      : 0;

  const primaryDisk = metrics?.disks[0] ?? null;
  const cpuVal = metrics?.cpuUsagePct ?? 0;
  const cpuUser = metrics?.cpuUserPct ?? null;
  const cpuSys = metrics?.cpuSysPct ?? null;

  // 计算指定时间窗口内的平滑 SVG 折线图路径
  const chartData = useMemo(() => {
    const now = Math.floor(Date.now() / 1000);
    const filtered = cpuHistory.filter((p) => now - p.ts <= trendWindow);
    const pts = filtered.length >= 2 ? filtered : cpuHistory.slice(-24);
    if (pts.length === 0) return null;

    const width = 280;
    const height = 74;
    const padY = 6;
    const usableH = height - padY * 2;
    const coords = pts.map((p, idx) => {
      const x = pts.length === 1 ? width : (idx / (pts.length - 1)) * width;
      const y = padY + usableH * (1 - Math.min(100, Math.max(0, p.cpu)) / 100);
      return { x, y, ts: p.ts };
    });

    const linePath = coords
      .map((c, idx) => `${idx === 0 ? "M" : "L"}${c.x.toFixed(1)},${c.y.toFixed(1)}`)
      .join(" ");
    const areaPath = `${linePath} L${coords[coords.length - 1].x.toFixed(1)},${height} L${coords[0].x.toFixed(1)},${height} Z`;

    const labelCount = Math.min(4, coords.length);
    const labels: string[] = [];
    for (let i = 0; i < labelCount; i++) {
      const pos =
        labelCount === 1
          ? coords.length - 1
          : Math.round((i * (coords.length - 1)) / (labelCount - 1));
      labels.push(formatClock(coords[pos].ts));
    }

    return { width, height, linePath, areaPath, labels };
  }, [cpuHistory, trendWindow]);

  const osBadgeLabel =
    metrics?.osFamily === "windows"
      ? "Windows"
      : metrics?.osFamily === "macos"
        ? "macOS"
        : "Linux";

  const tabs: { id: MonitorTab; label: string; icon: React.ReactNode }[] = [
    { id: "overview", label: "综合", icon: <Layers size={12} /> },
    { id: "cpu", label: "处理器", icon: <Cpu size={12} /> },
    { id: "gpu", label: "显卡", icon: <Monitor size={12} /> },
    { id: "memory", label: "内存", icon: <MemoryStick size={12} /> },
    { id: "disk", label: "磁盘", icon: <HardDrive size={12} /> },
    { id: "network", label: "网络", icon: <Wifi size={12} /> },
  ];

  return (
    <aside className="monitor-panel">
      {/* 顶部标题与控制 */}
      <div className="monitor-top-header">
        <div style={{ display: "flex", alignItems: "center", gap: 6 }}>
          <Activity size={14} color="var(--accent)" />
          <span style={{ fontWeight: 650, fontSize: 12.5 }}>
            {mode === "remote" ? "远端系统监控" : "本机系统监控"}
          </span>
        </div>
        <div style={{ display: "flex", alignItems: "center", gap: 4 }}>
          <button
            type="button"
            className="btn-tool btn-tool-xs"
            onClick={onRefresh}
            title="立即刷新系统监控指标"
          >
            <RefreshCw size={11} className={loading ? "spin" : ""} />
            刷新
          </button>
          <button
            type="button"
            className="btn-tool btn-tool-xs"
            onClick={onClose}
            title="收起系统监控栏"
          >
            <PanelRightClose size={12} />
            收起
          </button>
        </div>
      </div>

      {/* 分类标签条 */}
      <div className="monitor-tabs">
        {tabs.map((item) => (
          <button
            key={item.id}
            type="button"
            className={`monitor-tab ${tab === item.id ? "active" : ""}`}
            onClick={() => setTab(item.id)}
          >
            {item.icon}
            <span>{item.label}</span>
          </button>
        ))}
      </div>

      {/* 主体监控卡片区 */}
      <div className="monitor-scroll">
        {/* 主机概览头卡片 */}
        <div className="monitor-card monitor-host-card">
          <div className="monitor-host-icon">
            <Server size={16} color="var(--success)" />
          </div>
          <div className="monitor-host-main">
            <div className="monitor-host-row">
              <span className="monitor-host-name" title={title}>
                {title}
              </span>
              <span className="monitor-ip-pill tabular-nums">{hostLabel}</span>
            </div>
            <div className="monitor-host-sub">
              <span className="monitor-user-pill">
                {userLabel || osBadgeLabel}
              </span>
              <span className="monitor-os-text" title={metrics?.osName || osBadgeLabel}>
                {metrics?.osName || (loading ? "正在采集系统信息..." : osBadgeLabel)}
              </span>
            </div>
          </div>
          <div className="monitor-uptime-box">
            <span className="monitor-uptime-label">运行时间</span>
            <span className="monitor-uptime-val tabular-nums">
              {formatUptime(metrics?.uptimeSecs)}
            </span>
          </div>
        </div>

        {/* 综合视图 */}
        {tab === "overview" && (
          <>
            <div className="monitor-grid-2">
              {/* CPU 卡片 */}
              <div className="monitor-card monitor-kpi-card">
                <div className="monitor-kpi-top">
                  <span className="monitor-kpi-title">
                    <Cpu size={13} color="#38bdf8" />
                    CPU
                  </span>
                  <span className="monitor-kpi-val tabular-nums" style={{ color: "#38bdf8" }}>
                    {metrics?.cpuUsagePct != null ? `${cpuVal.toFixed(1)}%` : "—"}
                  </span>
                </div>
                <div className="monitor-bar-track">
                  {cpuUser != null && cpuSys != null ? (
                    <>
                      <div
                        className="monitor-bar-fill"
                        style={{ width: `${Math.min(100, cpuUser)}%`, background: "#38bdf8" }}
                      />
                      <div
                        className="monitor-bar-fill"
                        style={{
                          width: `${Math.min(100 - Math.min(100, cpuUser), cpuSys)}%`,
                          background: "#818cf8",
                        }}
                      />
                    </>
                  ) : (
                    <div
                      className="monitor-bar-fill"
                      style={{ width: `${Math.min(100, cpuVal)}%`, background: "#38bdf8" }}
                    />
                  )}
                </div>
                <div className="monitor-kpi-sub tabular-nums">
                  {cpuUser != null && cpuSys != null ? (
                    <>
                      <span>User: {cpuUser.toFixed(1)}%</span>
                      <span>Sys: {cpuSys.toFixed(1)}%</span>
                    </>
                  ) : (
                    <>
                      <span>{metrics?.cpuCores ? `${metrics.cpuCores} 核心` : "核心 —"}</span>
                      <span>{osBadgeLabel}</span>
                    </>
                  )}
                </div>
              </div>

              {/* 内存卡片 */}
              <div className="monitor-card monitor-kpi-card">
                <div className="monitor-kpi-top">
                  <span className="monitor-kpi-title">
                    <MemoryStick size={13} color="#a78bfa" />
                    内存
                  </span>
                  <span className="monitor-kpi-val tabular-nums" style={{ color: "#a78bfa" }}>
                    {metrics && metrics.memTotalBytes > 0 ? `${memPct}%` : "—"}
                  </span>
                </div>
                <div className="monitor-bar-track">
                  <div
                    className="monitor-bar-fill"
                    style={{
                      width: `${memPct}%`,
                      background: memPct >= 88 ? "var(--danger)" : "#a78bfa",
                    }}
                  />
                </div>
                <div className="monitor-kpi-sub tabular-nums">
                  <span>{metrics ? formatBytes(metrics.memUsedBytes) : "—"}</span>
                  <span>{metrics ? formatBytes(metrics.memTotalBytes) : "—"}</span>
                </div>
              </div>

              {/* 磁盘卡片 */}
              <div className="monitor-card monitor-kpi-card">
                <div className="monitor-kpi-top">
                  <span className="monitor-kpi-title">
                    <HardDrive size={13} color="#f59e0b" />
                    磁盘
                  </span>
                  <span
                    className="monitor-kpi-val tabular-nums"
                    style={{
                      color:
                        (primaryDisk?.usePercent ?? 0) >= 88 ? "var(--danger)" : "#f59e0b",
                    }}
                  >
                    {primaryDisk ? `${primaryDisk.usePercent}%` : "—"}
                  </span>
                </div>
                <div className="monitor-bar-track">
                  <div
                    className="monitor-bar-fill"
                    style={{
                      width: `${primaryDisk?.usePercent ?? 0}%`,
                      background:
                        (primaryDisk?.usePercent ?? 0) >= 88 ? "var(--danger)" : "#f59e0b",
                    }}
                  />
                </div>
                <div className="monitor-kpi-sub tabular-nums">
                  <span>{primaryDisk ? formatBytes(primaryDisk.usedBytes) : "—"}</span>
                  <span>{primaryDisk ? formatBytes(primaryDisk.totalBytes) : "—"}</span>
                </div>
              </div>

              {/* 网络卡片 */}
              <div className="monitor-card monitor-kpi-card">
                <div className="monitor-kpi-top">
                  <span className="monitor-kpi-title">
                    <Wifi size={13} color="#4ade80" />
                    网络
                  </span>
                  <span className="monitor-kpi-sub-inline tabular-nums">
                    {metrics?.netInterfaces.length ?? 0} 接口
                  </span>
                </div>
                <div className="monitor-net-speed-row tabular-nums">
                  <span style={{ color: "#4ade80" }}>↓ {formatSpeed(metrics?.netRxBps ?? 0)}</span>
                  <span style={{ color: "#60a5fa" }}>↑ {formatSpeed(metrics?.netTxBps ?? 0)}</span>
                </div>
                <div className="monitor-kpi-sub tabular-nums">
                  <span>默认网关</span>
                  <span>{metrics?.defaultGateway || "—"}</span>
                </div>
              </div>
            </div>

            {/* CPU 负载趋势折线图 */}
            <div className="monitor-card monitor-chart-card">
              <div className="monitor-chart-header">
                <span className="monitor-kpi-title">
                  <Activity size={13} color="#38bdf8" />
                  CPU 负载趋势
                </span>
                <div className="monitor-window-pills">
                  {([
                    [60, "1分钟"],
                    [300, "5分钟"],
                    [900, "15分钟"],
                  ] as const).map(([sec, lbl]) => (
                    <button
                      key={sec}
                      type="button"
                      className={trendWindow === sec ? "active" : ""}
                      onClick={() => setTrendWindow(sec)}
                    >
                      {lbl}
                    </button>
                  ))}
                </div>
              </div>
              {chartData ? (
                <>
                  <svg
                    className="monitor-chart-svg"
                    viewBox={`0 0 ${chartData.width} ${chartData.height}`}
                    preserveAspectRatio="none"
                  >
                    <defs>
                      <linearGradient id="cpuAreaGrad" x1="0" y1="0" x2="0" y2="1">
                        <stop offset="0%" stopColor="#38bdf8" stopOpacity="0.34" />
                        <stop offset="100%" stopColor="#38bdf8" stopOpacity="0.02" />
                      </linearGradient>
                    </defs>
                    <line x1="0" y1="25%" x2="100%" y2="25%" className="monitor-chart-grid" />
                    <line x1="0" y1="50%" x2="100%" y2="50%" className="monitor-chart-grid" />
                    <line x1="0" y1="75%" x2="100%" y2="75%" className="monitor-chart-grid" />
                    <path d={chartData.areaPath} fill="url(#cpuAreaGrad)" />
                    <path
                      d={chartData.linePath}
                      fill="none"
                      stroke="#38bdf8"
                      strokeWidth="1.8"
                      strokeLinecap="round"
                      strokeLinejoin="round"
                    />
                  </svg>
                  <div className="monitor-chart-times tabular-nums">
                    {chartData.labels.map((lbl, idx) => (
                      <span key={`${lbl}-${idx}`}>{lbl}</span>
                    ))}
                  </div>
                </>
              ) : (
                <div className="monitor-empty-hint">等待采集趋势数据...</div>
              )}
            </div>

            {/* 磁盘分区列表 */}
            <div className="monitor-section">
              <div className="monitor-section-title">
                <HardDrive size={12} color="#f59e0b" />
                磁盘分区
              </div>
              <div className="monitor-list">
                {metrics && metrics.disks.length > 0 ? (
                  metrics.disks.map((d) => {
                    const hot = d.usePercent >= 88;
                    const warn = !hot && d.usePercent >= 75;
                    return (
                      <button
                        key={d.mountPath}
                        type="button"
                        className="monitor-card monitor-disk-item"
                        onClick={() => onNavigateDisk?.(d.mountPath)}
                        title={`点击跳转到 ${d.mountPath}（已用 ${formatBytes(d.usedBytes)} / ${formatBytes(d.totalBytes)}，剩余 ${formatBytes(d.availBytes)}）`}
                      >
                        <div className="monitor-disk-top">
                          <span className="monitor-disk-label">{d.label}</span>
                          <span
                            className="tabular-nums"
                            style={{
                              fontWeight: 650,
                              color: hot
                                ? "var(--danger)"
                                : warn
                                  ? "var(--warning)"
                                  : "var(--text-secondary)",
                            }}
                          >
                            {d.usePercent}%
                          </span>
                        </div>
                        <div className="monitor-bar-track">
                          <div
                            className="monitor-bar-fill"
                            style={{
                              width: `${d.usePercent}%`,
                              background: hot ? "var(--danger)" : "#f59e0b",
                            }}
                          />
                        </div>
                      </button>
                    );
                  })
                ) : (
                  <div className="monitor-empty-hint">暂无磁盘分区信息</div>
                )}
              </div>
            </div>

            {/* 网络接口列表 */}
            <div className="monitor-section">
              <div className="monitor-section-title">
                <Wifi size={12} color="#4ade80" />
                网络接口
              </div>
              <div className="monitor-list">
                {metrics && metrics.netInterfaces.length > 0 ? (
                  metrics.netInterfaces.map((ni) => (
                    <div key={ni.name} className="monitor-card monitor-net-item">
                      <div className="monitor-net-left">
                        <span className="monitor-net-name">{ni.name}</span>
                        <span className="monitor-net-ip tabular-nums">
                          {ni.ip || "未分配 IP"}
                        </span>
                      </div>
                      <div className="monitor-net-right tabular-nums">
                        <span style={{ color: "#4ade80" }}>↓ {formatSpeed(ni.rxBytesPerSec)}</span>
                        <span style={{ color: "#60a5fa" }}>↑ {formatSpeed(ni.txBytesPerSec)}</span>
                      </div>
                    </div>
                  ))
                ) : (
                  <div className="monitor-empty-hint">暂无网络接口信息</div>
                )}
              </div>
            </div>
          </>
        )}

        {/* 处理器详情视图 */}
        {tab === "cpu" && (
          <div className="monitor-list">
            <div className="monitor-card monitor-detail-card">
              <div className="monitor-detail-row">
                <span>系统平台</span>
                <strong>{osBadgeLabel}</strong>
              </div>
              <div className="monitor-detail-row">
                <span>逻辑核心数</span>
                <strong className="tabular-nums">{metrics?.cpuCores || "—"} 核</strong>
              </div>
              <div className="monitor-detail-row">
                <span>当前总占用</span>
                <strong className="tabular-nums" style={{ color: "#38bdf8" }}>
                  {metrics?.cpuUsagePct != null ? `${cpuVal.toFixed(1)}%` : "—"}
                </strong>
              </div>
              {cpuUser != null && (
                <div className="monitor-detail-row">
                  <span>用户态 (User)</span>
                  <strong className="tabular-nums">{cpuUser.toFixed(1)}%</strong>
                </div>
              )}
              {cpuSys != null && (
                <div className="monitor-detail-row">
                  <span>内核态 (Sys)</span>
                  <strong className="tabular-nums">{cpuSys.toFixed(1)}%</strong>
                </div>
              )}
              {metrics?.loadAvg && (
                <div className="monitor-detail-row">
                  <span>平均负载 (1/5/15m)</span>
                  <strong className="tabular-nums">{metrics.loadAvg}</strong>
                </div>
              )}
              {metrics?.kernelVersion && (
                <div className="monitor-detail-row">
                  <span>内核与架构</span>
                  <strong className="tabular-nums">{metrics.kernelVersion}</strong>
                </div>
              )}
              {metrics && metrics.temperatures.length > 0 && (
                <>
                  <div className="term-popover-sep" />
                  {metrics.temperatures.map((t) => (
                    <div key={t.label} className="monitor-detail-row">
                      <span>{t.label} 温度</span>
                      <strong
                        className="tabular-nums"
                        style={{
                          color:
                            t.tempC >= 80
                              ? "var(--danger)"
                              : t.tempC >= 68
                                ? "var(--warning)"
                                : "var(--success)",
                        }}
                      >
                        {t.tempC.toFixed(1)}°C
                      </strong>
                    </div>
                  ))}
                </>
              )}
            </div>
          </div>
        )}

        {/* 显卡详情视图 */}
        {tab === "gpu" && (
          <div className="monitor-card monitor-detail-card">
            {metrics?.gpuName ? (
              <>
                <div className="monitor-detail-row">
                  <span>GPU 型号</span>
                  <strong>{metrics.gpuName}</strong>
                </div>
                <div className="monitor-detail-row">
                  <span>GPU 占用</span>
                  <strong className="tabular-nums">
                    {metrics.gpuUsagePct != null ? `${metrics.gpuUsagePct.toFixed(1)}%` : "—"}
                  </strong>
                </div>
                {metrics.gpuTempC != null && (
                  <div className="monitor-detail-row">
                    <span>GPU 温度</span>
                    <strong
                      className="tabular-nums"
                      style={{
                        color:
                          metrics.gpuTempC >= 82
                            ? "var(--danger)"
                            : metrics.gpuTempC >= 70
                              ? "var(--warning)"
                              : "var(--success)",
                      }}
                    >
                      {metrics.gpuTempC.toFixed(1)}°C
                    </strong>
                  </div>
                )}
                <div className="monitor-detail-row">
                  <span>显存容量</span>
                  <strong className="tabular-nums">
                    {metrics.gpuMemTotalMb
                      ? `${metrics.gpuMemUsedMb ?? 0} MB / ${metrics.gpuMemTotalMb} MB`
                      : "—"}
                  </strong>
                </div>
              </>
            ) : (
              <div className="monitor-empty-hint">
                当前主机未检测到独立 GPU 或未安装 `nvidia-smi`
              </div>
            )}
          </div>
        )}

        {/* 内存详情视图 */}
        {tab === "memory" && (
          <div className="monitor-card monitor-detail-card">
            <div className="monitor-detail-row">
              <span>物理内存使用率</span>
              <strong className="tabular-nums" style={{ color: "#a78bfa" }}>
                {memPct}%
              </strong>
            </div>
            <div className="monitor-bar-track" style={{ margin: "4px 0 8px" }}>
              <div
                className="monitor-bar-fill"
                style={{ width: `${memPct}%`, background: "#a78bfa" }}
              />
            </div>
            <div className="monitor-detail-row">
              <span>已用 / 总计</span>
              <strong className="tabular-nums">
                {metrics
                  ? `${formatBytes(metrics.memUsedBytes)} / ${formatBytes(metrics.memTotalBytes)}`
                  : "—"}
              </strong>
            </div>
            <div className="monitor-detail-row">
              <span>可用物理内存</span>
              <strong className="tabular-nums">
                {metrics
                  ? formatBytes(Math.max(0, metrics.memTotalBytes - metrics.memUsedBytes))
                  : "—"}
              </strong>
            </div>
            <div className="monitor-detail-row">
              <span>{metrics?.osFamily === "windows" ? "虚拟页面文件" : "Swap 交换分区"}</span>
              <strong className="tabular-nums">
                {metrics && metrics.swapTotalBytes > 0
                  ? `${formatBytes(metrics.swapUsedBytes)} / ${formatBytes(metrics.swapTotalBytes)} (${swapPct}%)`
                  : "未启用"}
              </strong>
            </div>
          </div>
        )}

        {/* 磁盘详情视图 */}
        {tab === "disk" && (
          <div className="monitor-list">
            {metrics && metrics.disks.length > 0 ? (
              metrics.disks.map((d) => (
                <button
                  key={d.mountPath}
                  type="button"
                  className="monitor-card monitor-disk-item"
                  onClick={() => onNavigateDisk?.(d.mountPath)}
                >
                  <div className="monitor-disk-top">
                    <span className="monitor-disk-label">{d.label}</span>
                    <span className="tabular-nums" style={{ fontWeight: 650 }}>
                      {d.usePercent}%
                    </span>
                  </div>
                  <div className="monitor-bar-track">
                    <div
                      className="monitor-bar-fill"
                      style={{
                        width: `${d.usePercent}%`,
                        background: d.usePercent >= 88 ? "var(--danger)" : "#f59e0b",
                      }}
                    />
                  </div>
                  <div className="monitor-kpi-sub tabular-nums">
                    <span>
                      已用 {formatBytes(d.usedBytes)} / {formatBytes(d.totalBytes)}
                    </span>
                    <span>剩余 {formatBytes(d.availBytes)}</span>
                  </div>
                </button>
              ))
            ) : (
              <div className="monitor-empty-hint">暂无磁盘信息</div>
            )}
          </div>
        )}

        {/* 网络详情视图 */}
        {tab === "network" && (
          <div className="monitor-list">
            <div className="monitor-card monitor-detail-card">
              <div className="monitor-detail-row">
                <span>默认网关</span>
                <strong className="tabular-nums">{metrics?.defaultGateway || "—"}</strong>
              </div>
              <div className="monitor-detail-row">
                <span>汇总下行</span>
                <strong className="tabular-nums" style={{ color: "#4ade80" }}>
                  ↓ {formatSpeed(metrics?.netRxBps ?? 0)}
                </strong>
              </div>
              <div className="monitor-detail-row">
                <span>汇总上行</span>
                <strong className="tabular-nums" style={{ color: "#60a5fa" }}>
                  ↑ {formatSpeed(metrics?.netTxBps ?? 0)}
                </strong>
              </div>
            </div>
            {metrics?.netInterfaces.map((ni) => (
              <div key={ni.name} className="monitor-card monitor-net-item">
                <div className="monitor-net-left">
                  <span className="monitor-net-name">{ni.name}</span>
                  <span className="monitor-net-ip tabular-nums">{ni.ip || "未分配 IP"}</span>
                </div>
                <div className="monitor-net-right tabular-nums">
                  <span style={{ color: "#4ade80" }}>↓ {formatSpeed(ni.rxBytesPerSec)}</span>
                  <span style={{ color: "#60a5fa" }}>↑ {formatSpeed(ni.txBytesPerSec)}</span>
                </div>
              </div>
            ))}
          </div>
        )}
      </div>

      {/* 底部当前目录 4 格统计（PATH / DIR / FILE / TOTAL） */}
      <div className="monitor-dir-footer">
        <div className="monitor-dir-stat" title={currentPath || "/"}>
          <span className="monitor-dir-label">
            <FolderOpen size={11} color="#60a5fa" />
            PATH
          </span>
          <span className="monitor-dir-val monitor-dir-path">{currentPath || "/"}</span>
        </div>
        <div className="monitor-dir-stat">
          <span className="monitor-dir-label">
            <Folder size={11} color="#818cf8" />
            DIR
          </span>
          <span className="monitor-dir-val tabular-nums" style={{ color: "#60a5fa" }}>
            {dirStats.dirs}
          </span>
        </div>
        <div className="monitor-dir-stat">
          <span className="monitor-dir-label">
            <FileText size={11} color="var(--text-muted)" />
            FILE
          </span>
          <span className="monitor-dir-val tabular-nums">{dirStats.files}</span>
        </div>
        <div className="monitor-dir-stat">
          <span className="monitor-dir-label">
            <HardDrive size={11} color="#22c55e" />
            TOTAL
          </span>
          <span className="monitor-dir-val tabular-nums" style={{ color: "#22c55e" }}>
            {formatBytes(dirStats.totalBytes)}
          </span>
        </div>
      </div>
    </aside>
  );
};
