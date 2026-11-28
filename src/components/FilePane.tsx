/**
 * 通用文件浏览面板组件（同时服务于左侧「本地」与右侧「远程」视图）
 *
 * 现代桌面交互能力：
 * - 可交互路径面包屑（逐级点击跳转，点击空白处切换为手输路径直达）
 * - 点击列头按名称 / 大小 / 修改时间升降序排序（目录始终优先置顶）
 * - 语义化文件类型图标 + 权限标签
 * - 右键上下文菜单（上传/下载、在线编辑、修改权限、复制路径、重命名、删除）
 * - 支持双栏拖拽互传、系统文件/文件夹拖放上传遮罩与 F2/Delete 键盘快捷键
 */

import React, { useEffect, useMemo, useRef, useState } from "react";
import {
  ArrowDown,
  ArrowUp,
  Check,
  ChevronDown,
  ChevronRight,
  Copy,
  Download,
  FileArchive,
  FileAudio,
  FileCode2,
  FileImage,
  FileJson,
  FileSpreadsheet,
  FileText,
  FileVideo,
  Folder,
  FolderPlus,
  FolderUp,
  HardDrive,
  Pencil,
  RefreshCw,
  Search,
  Shield,
  Terminal,
  Trash2,
  Upload,
} from "lucide-react";
import { formatBytes, formatTime, type FileEntry, type LocalRoot, type RemoteDisk } from "../types";
import { writeClipboardText } from "../utils/clipboard";

interface FilePaneProps {
  mode: "local" | "remote";
  title: string;
  currentPath: string;
  entries: FileEntry[];
  loading: boolean;
  roots?: LocalRoot[];
  remoteDisks?: RemoteDisk[];
  canTransfer: boolean;
  supportsChmod?: boolean;
  externalDropActive?: boolean;
  style?: React.CSSProperties;
  onNavigate: (path: string) => void;
  onRefresh: () => void;
  onMkdir: () => void;
  onRename: (entry: FileEntry) => void;
  onDelete: (entry: FileEntry) => void;
  onTransfer: (entry: FileEntry) => void;
  onCopyPath?: (path: string) => void;
  onUploadFromPicker?: () => void;
  onUploadFolderFromPicker?: () => void;
  onDropEntryFromOtherPane?: (entry: FileEntry) => void;
  onEditFile?: (entry: FileEntry) => void;
  onChmod?: (entry: FileEntry) => void;
  onOpenInTerminal?: (dirPath: string) => void;
  onInsertPathToTerminal?: (path: string) => void;
}

type SortKey = "name" | "size" | "modified";
type SortDir = "asc" | "desc";

interface BreadcrumbSegment {
  label: string;
  path: string;
}

interface ContextMenuState {
  x: number;
  y: number;
  entry: FileEntry;
}

/** 应用内双栏拖拽传输使用的自定义 MIME 类型 */
const INTERNAL_DRAG_MIME = "application/x-xiaorui-ftp-entry";

/** 计算当前路径的上一级目录（同时兼容 POSIX 远程路径与 Windows 本地盘符路径） */
function getParentPath(path: string, mode: "local" | "remote"): string {
  if (mode === "remote") {
    const clean = path.replace(/\/+$/, "");
    if (!clean || clean === "/") return "/";
    const idx = clean.lastIndexOf("/");
    return idx <= 0 ? "/" : clean.slice(0, idx);
  }
  const trimmed = path.replace(/[\\/]+$/, "");
  if (/^[A-Za-z]:$/.test(trimmed)) return `${trimmed}\\`;
  const sep = trimmed.includes("\\") ? "\\" : "/";
  const idx = trimmed.lastIndexOf(sep);
  if (idx < 0) return path;
  if (idx === 0) return "/";
  const parent = trimmed.slice(0, idx);
  return /^[A-Za-z]:$/.test(parent) ? `${parent}\\` : parent;
}

/** 将 POSIX 或 Windows 路径解析为可点击的面包屑段 */
function parseBreadcrumbs(path: string, mode: "local" | "remote"): BreadcrumbSegment[] {
  if (!path) return [];
  if (mode === "remote") {
    const clean = path.replace(/\/+$/, "");
    const segs: BreadcrumbSegment[] = [{ label: "/", path: "/" }];
    if (!clean || clean === "/") return segs;
    const parts = clean.split("/").filter(Boolean);
    let acc = "";
    for (const part of parts) {
      acc += `/${part}`;
      segs.push({ label: part, path: acc });
    }
    return segs;
  }

  // Windows 本地路径（如 C:\Users\name\Documents）
  if (/^[A-Za-z]:/.test(path)) {
    const drive = path.slice(0, 2);
    const rest = path.slice(2).replace(/^[\\/]+/, "").replace(/[\\/]+$/, "");
    const segs: BreadcrumbSegment[] = [{ label: `${drive}\\`, path: `${drive}\\` }];
    if (!rest) return segs;
    const parts = rest.split(/[\\/]+/).filter(Boolean);
    let acc = `${drive}\\`;
    for (const part of parts) {
      acc = acc.endsWith("\\") ? `${acc}${part}` : `${acc}\\${part}`;
      segs.push({ label: part, path: acc });
    }
    return segs;
  }

  // Unix 本地路径
  const clean = path.replace(/\/+$/, "");
  const segs: BreadcrumbSegment[] = [{ label: "/", path: "/" }];
  if (!clean || clean === "/") return segs;
  const parts = clean.split("/").filter(Boolean);
  let acc = "";
  for (const part of parts) {
    acc += `/${part}`;
    segs.push({ label: part, path: acc });
  }
  return segs;
}

/** 根据文件扩展名返回语义化图标，提升列表可扫描性 */
function renderFileIcon(entry: FileEntry): React.ReactNode {
  if (entry.isDir) {
    return <Folder size={15} color="var(--accent)" style={{ flexShrink: 0 }} />;
  }
  const dot = entry.name.lastIndexOf(".");
  const ext = dot >= 0 ? entry.name.slice(dot + 1).toLowerCase() : "";

  if (["json", "jsonc", "toml", "yaml", "yml", "xml", "ini", "env", "conf"].includes(ext)) {
    return <FileJson size={15} color="#d97706" style={{ flexShrink: 0 }} />;
  }
  if (
    [
      "ts",
      "tsx",
      "js",
      "jsx",
      "rs",
      "py",
      "go",
      "java",
      "c",
      "cpp",
      "h",
      "cs",
      "sh",
      "ps1",
      "sql",
      "html",
      "css",
      "scss",
      "vue",
      "php",
      "rb",
    ].includes(ext)
  ) {
    return <FileCode2 size={15} color="#2563eb" style={{ flexShrink: 0 }} />;
  }
  if (["png", "jpg", "jpeg", "gif", "webp", "svg", "ico", "bmp", "avif"].includes(ext)) {
    return <FileImage size={15} color="#059669" style={{ flexShrink: 0 }} />;
  }
  if (["zip", "tar", "gz", "tgz", "bz2", "xz", "7z", "rar", "zst"].includes(ext)) {
    return <FileArchive size={15} color="#7c3aed" style={{ flexShrink: 0 }} />;
  }
  if (["mp4", "mkv", "mov", "avi", "webm", "flv"].includes(ext)) {
    return <FileVideo size={15} color="#db2777" style={{ flexShrink: 0 }} />;
  }
  if (["mp3", "wav", "flac", "ogg", "m4a", "aac"].includes(ext)) {
    return <FileAudio size={15} color="#0891b2" style={{ flexShrink: 0 }} />;
  }
  if (["csv", "xlsx", "xls", "parquet"].includes(ext)) {
    return <FileSpreadsheet size={15} color="#16a34a" style={{ flexShrink: 0 }} />;
  }
  return <FileText size={15} color="var(--text-muted)" style={{ flexShrink: 0 }} />;
}

export const FilePane: React.FC<FilePaneProps> = ({
  mode,
  title,
  currentPath,
  entries,
  loading,
  roots,
  remoteDisks,
  canTransfer,
  supportsChmod,
  externalDropActive,
  style,
  onNavigate,
  onRefresh,
  onMkdir,
  onRename,
  onDelete,
  onTransfer,
  onCopyPath,
  onUploadFromPicker,
  onUploadFolderFromPicker,
  onDropEntryFromOtherPane,
  onEditFile,
  onChmod,
  onOpenInTerminal,
  onInsertPathToTerminal,
}) => {
  const [pathInput, setPathInput] = useState(currentPath);
  const [editingPath, setEditingPath] = useState(false);
  const [filter, setFilter] = useState("");
  const [sortKey, setSortKey] = useState<SortKey>("name");
  const [sortDir, setSortDir] = useState<SortDir>("asc");
  const [selectedPath, setSelectedPath] = useState<string | null>(null);
  const [internalDragOver, setInternalDragOver] = useState(false);
  const [contextMenu, setContextMenu] = useState<ContextMenuState | null>(null);
  const [diskMenuOpen, setDiskMenuOpen] = useState(false);

  const pathInputRef = useRef<HTMLInputElement>(null);

  useEffect(() => {
    setPathInput(currentPath);
    setSelectedPath(null);
    setEditingPath(false);
    setContextMenu(null);
    setDiskMenuOpen(false);
  }, [currentPath]);

  // 点击空白或按 Esc 关闭磁盘选择浮层
  useEffect(() => {
    if (!diskMenuOpen) return;
    const close = () => setDiskMenuOpen(false);
    const onKey = (e: KeyboardEvent) => {
      if (e.key === "Escape") close();
    };
    window.addEventListener("click", close);
    window.addEventListener("keydown", onKey);
    return () => {
      window.removeEventListener("click", close);
      window.removeEventListener("keydown", onKey);
    };
  }, [diskMenuOpen]);

  /** 匹配当前远程路径所属磁盘（最长挂载前缀优先） */
  const activeRemoteDisk = useMemo(() => {
    if (!remoteDisks || remoteDisks.length === 0) return null;
    const norm = currentPath || "/";
    let best: RemoteDisk | null = null;
    for (const d of remoteDisks) {
      const mp =
        d.mountPath.endsWith("/") && d.mountPath.length > 1
          ? d.mountPath.slice(0, -1)
          : d.mountPath;
      const matches = mp === "/" || norm === mp || norm.startsWith(`${mp}/`);
      if (matches && (!best || mp.length > best.mountPath.length)) {
        best = d;
      }
    }
    return best ?? remoteDisks[0];
  }, [remoteDisks, currentPath]);

  useEffect(() => {
    if (editingPath) {
      requestAnimationFrame(() => {
        pathInputRef.current?.focus();
        pathInputRef.current?.select();
      });
    }
  }, [editingPath]);

  // 点击空白、滚动或按 Esc 时关闭右键菜单
  useEffect(() => {
    if (!contextMenu) return;
    const close = () => setContextMenu(null);
    const onKey = (e: KeyboardEvent) => {
      if (e.key === "Escape") close();
    };
    window.addEventListener("click", close);
    window.addEventListener("scroll", close, true);
    window.addEventListener("keydown", onKey);
    return () => {
      window.removeEventListener("click", close);
      window.removeEventListener("scroll", close, true);
      window.removeEventListener("keydown", onKey);
    };
  }, [contextMenu]);

  const breadcrumbs = useMemo(
    () => parseBreadcrumbs(currentPath, mode),
    [currentPath, mode]
  );

  /** 过滤并按当前列头排序（文件夹始终排在文件之前） */
  const sortedEntries = useMemo(() => {
    const q = filter.trim().toLowerCase();
    const base = q
      ? entries.filter((e) => e.name.toLowerCase().includes(q))
      : [...entries];

    const factor = sortDir === "asc" ? 1 : -1;
    base.sort((a, b) => {
      if (a.isDir !== b.isDir) return a.isDir ? -1 : 1;
      if (sortKey === "size") {
        return (a.size - b.size) * factor || a.name.localeCompare(b.name);
      }
      if (sortKey === "modified") {
        return ((a.modified ?? 0) - (b.modified ?? 0)) * factor || a.name.localeCompare(b.name);
      }
      return a.name.localeCompare(b.name, undefined, { numeric: true, sensitivity: "base" }) * factor;
    });
    return base;
  }, [entries, filter, sortKey, sortDir]);

  const selectedEntry = useMemo(
    () => entries.find((e) => e.path === selectedPath) ?? null,
    [entries, selectedPath]
  );

  const toggleSort = (key: SortKey) => {
    if (sortKey === key) {
      setSortDir((d) => (d === "asc" ? "desc" : "asc"));
    } else {
      setSortKey(key);
      setSortDir(key === "name" ? "asc" : "desc");
    }
  };

  const handlePathSubmit = (e: React.FormEvent) => {
    e.preventDefault();
    const target = pathInput.trim();
    setEditingPath(false);
    if (target && target !== currentPath) {
      onNavigate(target);
    }
  };

  const handleDragStart = (e: React.DragEvent, entry: FileEntry) => {
    e.dataTransfer.setData(
      INTERNAL_DRAG_MIME,
      JSON.stringify({ sourceMode: mode, entry })
    );
    e.dataTransfer.effectAllowed = "copy";
  };

  const handleDragOver = (e: React.DragEvent) => {
    if (!canTransfer || !onDropEntryFromOtherPane) return;
    if (e.dataTransfer.types.includes(INTERNAL_DRAG_MIME)) {
      e.preventDefault();
      e.dataTransfer.dropEffect = "copy";
      setInternalDragOver(true);
    }
  };

  const handleDragLeave = (e: React.DragEvent) => {
    if (e.currentTarget.contains(e.relatedTarget as Node | null)) return;
    setInternalDragOver(false);
  };

  const handleDrop = (e: React.DragEvent) => {
    setInternalDragOver(false);
    if (!canTransfer || !onDropEntryFromOtherPane) return;
    const raw = e.dataTransfer.getData(INTERNAL_DRAG_MIME);
    if (!raw) return;
    e.preventDefault();
    try {
      const parsed = JSON.parse(raw) as { sourceMode: "local" | "remote"; entry: FileEntry };
      if (parsed.sourceMode !== mode && parsed.entry) {
        onDropEntryFromOtherPane(parsed.entry);
      }
    } catch {
      // 忽略无效的拖拽载荷
    }
  };

  const handleRowContextMenu = (e: React.MouseEvent, entry: FileEntry) => {
    e.preventDefault();
    setSelectedPath(entry.path);
    const menuWidth = 196;
    const menuHeight = 220;
    const x = Math.min(e.clientX, window.innerWidth - menuWidth - 10);
    const y = Math.min(e.clientY, window.innerHeight - menuHeight - 10);
    setContextMenu({ x: Math.max(8, x), y: Math.max(8, y), entry });
  };

  const handlePaneKeyDown = (e: React.KeyboardEvent<HTMLElement>) => {
    const tag = (e.target as HTMLElement | null)?.tagName;
    if (tag === "INPUT" || tag === "TEXTAREA" || tag === "SELECT") return;
    if (!selectedEntry) return;
    if (e.key === "F2") {
      e.preventDefault();
      onRename(selectedEntry);
    } else if (e.key === "Delete") {
      e.preventDefault();
      onDelete(selectedEntry);
    }
  };

  const handleCopyPath = async (path: string) => {
    if (await writeClipboardText(path)) {
      onCopyPath?.(path);
      return;
    }
    try {
      const ta = document.createElement("textarea");
      ta.value = path;
      ta.style.position = "fixed";
      ta.style.opacity = "0";
      document.body.appendChild(ta);
      ta.select();
      document.execCommand("copy");
      document.body.removeChild(ta);
      onCopyPath?.(path);
    } catch {
      // 剪贴板不可用时静默忽略
    }
  };

  const renderSortIcon = (key: SortKey) => {
    if (sortKey !== key) return null;
    return sortDir === "asc" ? <ArrowUp size={11} /> : <ArrowDown size={11} />;
  };

  const showDropOverlay = Boolean(externalDropActive) || internalDragOver;

  return (
    <section
      className="pane"
      tabIndex={0}
      style={{ position: "relative", ...style }}
      onKeyDown={handlePaneKeyDown}
      onDragOver={handleDragOver}
      onDragLeave={handleDragLeave}
      onDrop={handleDrop}
    >
      {/* 第一行：面板标识、磁盘/位置切换与主操作按钮（绝不挤压路径栏或裁切按钮） */}
      <div className="pane-toolbar">
        <span className={`pane-title-chip ${mode === "remote" ? "remote" : ""}`} title={title}>
          {title}
        </span>

        {mode === "local" && roots && roots.length > 0 && (
          <select
            className="root-select"
            value=""
            onChange={(e) => {
              if (e.target.value) onNavigate(e.target.value);
            }}
            title="快速切换本地位置或盘符"
          >
            <option value="" disabled>
              切换位置...
            </option>
            {roots.map((r) => (
              <option key={r.path} value={r.path}>
                {r.name}
              </option>
            ))}
          </select>
        )}

        {mode === "remote" && remoteDisks && remoteDisks.length > 0 && (
          <div className="disk-popover-wrap" onClick={(e) => e.stopPropagation()}>
            <button
              type="button"
              className={`disk-trigger-btn ${diskMenuOpen ? "active" : ""}`}
              onClick={() => setDiskMenuOpen((v) => !v)}
              title="点击查看或切换远端服务器磁盘 / 挂载点"
            >
              <HardDrive size={12} />
              <span className="disk-trigger-label">
                {activeRemoteDisk ? activeRemoteDisk.label : "/"}
              </span>
              {remoteDisks.length > 1 && (
                <span className="disk-count-tag tabular-nums">{remoteDisks.length}盘</span>
              )}
              <ChevronDown size={12} />
            </button>

            {diskMenuOpen && (
              <div className="disk-popover">
                <div className="disk-popover-header">
                  远端磁盘与挂载点（共 {remoteDisks.length} 个）
                </div>
                {remoteDisks.map((d) => {
                  const isCurrent = activeRemoteDisk?.mountPath === d.mountPath;
                  const levelClass =
                    d.usePercent >= 90 ? "hot" : d.usePercent >= 78 ? "warn" : "";
                  return (
                    <button
                      key={d.mountPath}
                      type="button"
                      className={`disk-option ${isCurrent ? "current" : ""}`}
                      onClick={() => {
                        setDiskMenuOpen(false);
                        onNavigate(d.mountPath);
                      }}
                    >
                      <div className="disk-option-top">
                        <span className="disk-option-name">
                          <HardDrive size={13} color={isCurrent ? "var(--accent)" : undefined} />
                          <span>{d.label}</span>
                        </span>
                        <span className="disk-option-pct tabular-nums">
                          {d.usePercent}%
                          {isCurrent && <Check size={12} color="var(--accent)" />}
                        </span>
                      </div>
                      <div className="disk-option-bar">
                        <div
                          className={`disk-option-fill ${levelClass}`}
                          style={{ width: `${d.usePercent}%` }}
                        />
                      </div>
                      <div className="disk-option-sub tabular-nums">
                        已用 {formatBytes(d.usedBytes)} / {formatBytes(d.totalBytes)} · 剩余{" "}
                        {formatBytes(d.availBytes)}
                      </div>
                    </button>
                  );
                })}
              </div>
            )}
          </div>
        )}

        <div className="pane-toolbar-actions">
          <button
            type="button"
            className="btn-tool btn-tool-xs"
            onClick={onRefresh}
            title="刷新当前目录 (F5)"
          >
            <RefreshCw size={12} className={loading ? "spin" : ""} />
            刷新
          </button>

          <button
            type="button"
            className="btn-tool btn-tool-xs"
            onClick={onMkdir}
            title="在当前目录新建文件夹"
          >
            <FolderPlus size={13} />
            新建
          </button>

          {mode === "remote" && (
            <button
              type="button"
              className="btn-tool btn-tool-xs"
              disabled={!selectedEntry}
              onClick={() => selectedEntry && onTransfer(selectedEntry)}
              title={
                selectedEntry
                  ? `下载「${selectedEntry.name}」并选择保存位置`
                  : "先在下方选中文件或文件夹，即可点击下载"
              }
            >
              <Download size={12} />
              下载
            </button>
          )}

          {mode === "remote" && onUploadFolderFromPicker && (
            <button
              type="button"
              className="btn-tool btn-tool-xs"
              onClick={onUploadFolderFromPicker}
              title="选择本地文件夹递归上传到当前远程目录"
            >
              <FolderUp size={13} />
              传目录
            </button>
          )}

          {mode === "remote" && onUploadFromPicker && (
            <button
              type="button"
              className="btn btn-sm btn-primary"
              style={{ height: 24, padding: "0 8px", fontSize: 11.5 }}
              onClick={onUploadFromPicker}
              title="选择本地文件上传到当前远程目录"
            >
              <Upload size={12} />
              上传
            </button>
          )}
        </div>
      </div>

      {/* 第二行：独立全宽路径面包屑导航 + 目录即时过滤 */}
      <div className="pane-subbar">
        <button
          type="button"
          className="icon-btn"
          style={{ width: 26, height: 26 }}
          onClick={() => onNavigate(getParentPath(currentPath, mode))}
          title="返回上级目录"
        >
          <ArrowUp size={14} />
        </button>

        {/* 可交互路径面包屑，点击空白处切换为手输路径 */}
        {editingPath ? (
          <form onSubmit={handlePathSubmit} style={{ flex: 1, display: "flex", minWidth: 0 }}>
            <input
              ref={pathInputRef}
              className="path-bar"
              value={pathInput}
              onChange={(e) => setPathInput(e.target.value)}
              onBlur={() => setEditingPath(false)}
              onKeyDown={(e) => {
                if (e.key === "Escape") {
                  setPathInput(currentPath);
                  setEditingPath(false);
                }
              }}
              title="输入路径按回车跳转，Esc 取消"
            />
          </form>
        ) : (
          <div
            className="breadcrumb-bar"
            title="点击任意目录层级跳转，或点击空白处直接输入路径"
            onClick={() => setEditingPath(true)}
          >
            {breadcrumbs.map((seg, idx) => {
              const isLast = idx === breadcrumbs.length - 1;
              return (
                <React.Fragment key={seg.path}>
                  {idx > 0 && (
                    <span className="breadcrumb-sep">
                      <ChevronRight size={12} />
                    </span>
                  )}
                  <button
                    type="button"
                    className={`breadcrumb-seg ${isLast ? "current" : ""}`}
                    onClick={(e) => {
                      e.stopPropagation();
                      if (!isLast) onNavigate(seg.path);
                      else setEditingPath(true);
                    }}
                  >
                    {seg.label}
                  </button>
                </React.Fragment>
              );
            })}
            <span className="breadcrumb-spacer" />
          </div>
        )}

        <div className="pane-filter-wrap">
          <Search size={12} color="var(--text-muted)" />
          <input
            className="pane-filter"
            value={filter}
            onChange={(e) => setFilter(e.target.value)}
            placeholder="过滤..."
          />
        </div>

        <span className="pane-status tabular-nums">
          {loading ? "加载中..." : `${sortedEntries.length} 项`}
        </span>
      </div>

      <div className="file-list">
        <table className="file-table">
          <thead>
            <tr>
              <th style={{ width: "45%" }}>
                <button
                  type="button"
                  className={`th-sort-btn ${sortKey === "name" ? "active" : ""}`}
                  onClick={() => toggleSort("name")}
                >
                  名称 {renderSortIcon("name")}
                </button>
              </th>
              <th style={{ width: "15%" }}>
                <button
                  type="button"
                  className={`th-sort-btn ${sortKey === "size" ? "active" : ""}`}
                  onClick={() => toggleSort("size")}
                >
                  大小 {renderSortIcon("size")}
                </button>
              </th>
              <th style={{ width: "22%" }}>
                <button
                  type="button"
                  className={`th-sort-btn ${sortKey === "modified" ? "active" : ""}`}
                  onClick={() => toggleSort("modified")}
                >
                  修改时间 {renderSortIcon("modified")}
                </button>
              </th>
              <th style={{ width: "18%", textAlign: "right" }}>操作</th>
            </tr>
          </thead>
          <tbody>
            {sortedEntries.length === 0 ? (
              <tr>
                <td
                  colSpan={4}
                  style={{
                    textAlign: "center",
                    padding: "40px 12px",
                    color: "var(--text-muted)",
                    borderBottom: "none",
                  }}
                >
                  {loading
                    ? "正在读取目录..."
                    : filter
                      ? "没有匹配当前过滤条件的文件"
                      : mode === "remote"
                        ? "目录为空，可直接从系统拖入文件或文件夹上传"
                        : "目录为空"}
                </td>
              </tr>
            ) : (
              sortedEntries.map((entry) => (
                <tr
                  key={entry.path}
                  className={`file-row ${selectedPath === entry.path ? "selected" : ""}`}
                  draggable={canTransfer}
                  onDragStart={(e) => handleDragStart(e, entry)}
                  onClick={() => setSelectedPath(entry.path)}
                  onContextMenu={(e) => handleRowContextMenu(e, entry)}
                  onDoubleClick={() => {
                    if (entry.isDir) {
                      onNavigate(entry.path);
                    } else if (mode === "remote" && onEditFile && entry.size <= 512 * 1024) {
                      onEditFile(entry);
                    } else if (canTransfer) {
                      onTransfer(entry);
                    }
                  }}
                >
                  <td>
                    <div className="file-name-cell" title={entry.path}>
                      {renderFileIcon(entry)}
                      <span className="file-name-text">{entry.name}</span>
                      {mode === "remote" && entry.permissions && (
                        <span className="file-perm-chip">{entry.permissions}</span>
                      )}
                    </div>
                  </td>
                  <td className="tabular-nums" style={{ color: "var(--text-secondary)" }}>
                    {entry.isDir ? "—" : formatBytes(entry.size)}
                  </td>
                  <td
                    className="tabular-nums"
                    style={{ color: "var(--text-secondary)", fontSize: 12 }}
                  >
                    {formatTime(entry.modified)}
                  </td>
                  <td style={{ textAlign: "right" }} onClick={(e) => e.stopPropagation()}>
                    <div className="row-actions">
                      {canTransfer && (
                        <button
                          className="icon-btn"
                          style={{ width: 24, height: 24 }}
                          title={
                            mode === "local"
                              ? entry.isDir
                                ? "递归上传整个文件夹到远程"
                                : "上传到远程当前目录"
                              : entry.isDir
                                ? "递归下载文件夹（选择保存目录...）"
                                : "下载文件（选择保存位置...）"
                          }
                          onClick={() => onTransfer(entry)}
                        >
                          {mode === "local" ? <Upload size={13} /> : <Download size={13} />}
                        </button>
                      )}
                      {mode === "remote" && !entry.isDir && onEditFile && (
                        <button
                          className="icon-btn"
                          style={{ width: 24, height: 24 }}
                          title="在线查看 / 编辑文本"
                          onClick={() => onEditFile(entry)}
                        >
                          <FileCode2 size={13} />
                        </button>
                      )}
                      {mode === "remote" && supportsChmod && onChmod && (
                        <button
                          className="icon-btn"
                          style={{ width: 24, height: 24 }}
                          title={`修改权限${entry.permissions ? " (" + entry.permissions + ")" : ""}`}
                          onClick={() => onChmod(entry)}
                        >
                          <Shield size={13} />
                        </button>
                      )}
                      <button
                        className="icon-btn"
                        style={{ width: 24, height: 24 }}
                        title="重命名 (F2)"
                        onClick={() => onRename(entry)}
                      >
                        <Pencil size={13} />
                      </button>
                      <button
                        className="icon-btn btn-danger"
                        style={{ width: 24, height: 24 }}
                        title="删除 (Delete)"
                        onClick={() => onDelete(entry)}
                      >
                        <Trash2 size={13} />
                      </button>
                    </div>
                  </td>
                </tr>
              ))
            )}
          </tbody>
        </table>
      </div>

      {/* 原生风格右键上下文菜单 */}
      {contextMenu && (
        <div
          className="context-menu"
          style={{ left: contextMenu.x, top: contextMenu.y }}
          onClick={(e) => e.stopPropagation()}
        >
          {contextMenu.entry.isDir && (
            <button
              className="context-menu-item"
              onClick={() => {
                onNavigate(contextMenu.entry.path);
                setContextMenu(null);
              }}
            >
              <Folder size={14} color="var(--accent)" />
              打开文件夹
            </button>
          )}
          {canTransfer && (
            <button
              className="context-menu-item"
              onClick={() => {
                onTransfer(contextMenu.entry);
                setContextMenu(null);
              }}
            >
              {mode === "local" ? <Upload size={14} /> : <Download size={14} />}
              {mode === "local"
                ? contextMenu.entry.isDir
                  ? "递归上传文件夹"
                  : "上传到远程"
                : contextMenu.entry.isDir
                  ? "递归下载文件夹..."
                  : "下载并选择保存位置..."}
            </button>
          )}
          {mode === "remote" && !contextMenu.entry.isDir && onEditFile && (
            <button
              className="context-menu-item"
              onClick={() => {
                onEditFile(contextMenu.entry);
                setContextMenu(null);
              }}
            >
              <FileCode2 size={14} />
              在线查看 / 编辑
            </button>
          )}
          {mode === "remote" && supportsChmod && onChmod && (
            <button
              className="context-menu-item"
              onClick={() => {
                onChmod(contextMenu.entry);
                setContextMenu(null);
              }}
            >
              <Shield size={14} />
              修改权限 (CHMOD)
            </button>
          )}
          {onOpenInTerminal && contextMenu.entry.isDir && (
            <button
              className="context-menu-item"
              onClick={() => {
                onOpenInTerminal(contextMenu.entry.path);
                setContextMenu(null);
              }}
            >
              <Terminal size={14} color="var(--accent)" />
              {mode === "local" ? "在此目录打开本地终端" : "在此目录打开 SSH 终端"}
            </button>
          )}
          {mode === "remote" && onInsertPathToTerminal && (
            <button
              className="context-menu-item"
              onClick={() => {
                onInsertPathToTerminal(contextMenu.entry.path);
                setContextMenu(null);
              }}
            >
              <Terminal size={14} />
              将路径插入终端光标
            </button>
          )}
          <div className="context-menu-sep" />
          <button
            className="context-menu-item"
            onClick={() => {
              void handleCopyPath(contextMenu.entry.path);
              setContextMenu(null);
            }}
          >
            <Copy size={14} />
            复制完整路径
          </button>
          <button
            className="context-menu-item"
            onClick={() => {
              onRename(contextMenu.entry);
              setContextMenu(null);
            }}
          >
            <Pencil size={14} />
            重命名
          </button>
          <button
            className="context-menu-item danger"
            onClick={() => {
              onDelete(contextMenu.entry);
              setContextMenu(null);
            }}
          >
            <Trash2 size={14} />
            删除
          </button>
        </div>
      )}

      {showDropOverlay && (
        <div className="drop-overlay">
          <div className="drop-overlay-card">
            {mode === "remote" ? <FolderUp size={26} /> : <Download size={26} />}
            <div style={{ fontWeight: 600, fontSize: 14 }}>
              {mode === "remote"
                ? "松开鼠标，递归上传到此远程目录"
                : "松开鼠标，递归下载到此本地目录"}
            </div>
            <div style={{ fontSize: 12, color: "var(--text-muted)", fontFamily: "var(--font-mono)" }}>
              {currentPath}
            </div>
          </div>
        </div>
      )}
    </section>
  );
};
