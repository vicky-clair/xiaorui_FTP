/**
 * 通用文件浏览面板组件（同时服务于左侧「本地」与右侧「远程」视图）
 *
 * 支持路径栏直达、返回上级、关键字过滤、单/多选拖拽互传、系统文件/文件夹拖放提示遮罩，
 * 以及新建目录、上传文件/文件夹、选择保存位置下载、在线编辑、CHMOD 权限修改、重命名与删除。
 */

import React, { useMemo, useState } from "react";
import {
  ArrowUp,
  Download,
  FileCode2,
  FileText,
  Folder,
  FolderPlus,
  FolderUp,
  Pencil,
  RefreshCw,
  Shield,
  Trash2,
  Upload,
} from "lucide-react";
import { formatBytes, formatTime, type FileEntry, type LocalRoot } from "../types";

interface FilePaneProps {
  mode: "local" | "remote";
  title: string;
  currentPath: string;
  entries: FileEntry[];
  loading: boolean;
  roots?: LocalRoot[];
  canTransfer: boolean;
  supportsChmod?: boolean;
  externalDropActive?: boolean;
  onNavigate: (path: string) => void;
  onRefresh: () => void;
  onMkdir: () => void;
  onRename: (entry: FileEntry) => void;
  onDelete: (entry: FileEntry) => void;
  onTransfer: (entry: FileEntry) => void;
  onUploadFromPicker?: () => void;
  onUploadFolderFromPicker?: () => void;
  onDropEntryFromOtherPane?: (entry: FileEntry) => void;
  onEditFile?: (entry: FileEntry) => void;
  onChmod?: (entry: FileEntry) => void;
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

export const FilePane: React.FC<FilePaneProps> = ({
  mode,
  title,
  currentPath,
  entries,
  loading,
  roots,
  canTransfer,
  supportsChmod,
  externalDropActive,
  onNavigate,
  onRefresh,
  onMkdir,
  onRename,
  onDelete,
  onTransfer,
  onUploadFromPicker,
  onUploadFolderFromPicker,
  onDropEntryFromOtherPane,
  onEditFile,
  onChmod,
}) => {
  const [pathInput, setPathInput] = useState(currentPath);
  const [filter, setFilter] = useState("");
  const [selectedPath, setSelectedPath] = useState<string | null>(null);
  const [internalDragOver, setInternalDragOver] = useState(false);

  React.useEffect(() => {
    setPathInput(currentPath);
  }, [currentPath]);

  const filtered = useMemo(() => {
    const q = filter.trim().toLowerCase();
    if (!q) return entries;
    return entries.filter((e) => e.name.toLowerCase().includes(q));
  }, [entries, filter]);

  const selectedEntry = useMemo(
    () => entries.find((e) => e.path === selectedPath) ?? null,
    [entries, selectedPath]
  );

  const handlePathSubmit = (e: React.FormEvent) => {
    e.preventDefault();
    if (pathInput.trim()) {
      onNavigate(pathInput.trim());
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
      // ignore invalid payload
    }
  };

  const showDropOverlay = Boolean(externalDropActive) || internalDragOver;

  return (
    <section
      className="pane"
      style={{ position: "relative" }}
      onDragOver={handleDragOver}
      onDragLeave={handleDragLeave}
      onDrop={handleDrop}
    >
      <div className="pane-toolbar">
        <span className="pane-title-chip">{title}</span>

        {mode === "local" && roots && roots.length > 0 && (
          <select
            className="root-select"
            value=""
            onChange={(e) => {
              if (e.target.value) onNavigate(e.target.value);
            }}
            title="快速切换本地位置"
          >
            <option value="" disabled>
              位置...
            </option>
            {roots.map((r) => (
              <option key={r.path} value={r.path}>
                {r.name}
              </option>
            ))}
          </select>
        )}

        <button
          className="icon-btn"
          onClick={() => onNavigate(getParentPath(currentPath, mode))}
          title="返回上级目录"
        >
          <ArrowUp size={15} />
        </button>

        <form onSubmit={handlePathSubmit} style={{ flex: 1, display: "flex", minWidth: 0 }}>
          <input
            className="path-bar"
            value={pathInput}
            onChange={(e) => setPathInput(e.target.value)}
            title="输入路径按回车跳转"
          />
        </form>

        <button className="icon-btn" onClick={onRefresh} title="刷新">
          <RefreshCw size={14} className={loading ? "spin" : ""} />
        </button>

        <button className="icon-btn" onClick={onMkdir} title="新建文件夹">
          <FolderPlus size={15} />
        </button>

        {mode === "remote" && onUploadFolderFromPicker && (
          <button
            className="icon-btn"
            onClick={onUploadFolderFromPicker}
            title="选择本地文件夹递归上传"
          >
            <FolderUp size={15} />
          </button>
        )}

        {mode === "remote" && (
          <button
            className="btn btn-sm btn-outline"
            disabled={!selectedEntry}
            onClick={() => selectedEntry && onTransfer(selectedEntry)}
            title={
              selectedEntry
                ? `下载「${selectedEntry.name}」并选择保存位置`
                : "先在下方选中文件或文件夹，即可点击选择位置下载"
            }
          >
            <Download size={13} />
            下载
          </button>
        )}

        {mode === "remote" && onUploadFromPicker && (
          <button
            className="btn btn-sm btn-primary"
            onClick={onUploadFromPicker}
            title="选择本地文件上传到当前远程目录"
          >
            <Upload size={13} />
            上传
          </button>
        )}
      </div>

      <div className="pane-subbar">
        <input
          className="pane-filter"
          value={filter}
          onChange={(e) => setFilter(e.target.value)}
          placeholder="过滤当前目录..."
        />
        <span className="pane-status">
          {loading
            ? "加载中..."
            : mode === "remote"
              ? `${filtered.length} 项 · 支持拖放文件/文件夹上传`
              : `${filtered.length} 项`}
        </span>
      </div>

      <div className="file-list">
        <table className="file-table">
          <thead>
            <tr>
              <th style={{ width: "44%" }}>名称</th>
              <th style={{ width: "16%" }}>大小</th>
              <th style={{ width: "22%" }}>修改时间</th>
              <th style={{ width: "18%", textAlign: "right" }}>操作</th>
            </tr>
          </thead>
          <tbody>
            {filtered.length === 0 ? (
              <tr>
                <td
                  colSpan={4}
                  style={{
                    textAlign: "center",
                    padding: "36px 12px",
                    color: "var(--text-muted)",
                    borderBottom: "none",
                  }}
                >
                  {loading
                    ? "正在读取目录..."
                    : mode === "remote"
                      ? "目录为空，可直接从系统拖入文件或文件夹上传"
                      : "目录为空"}
                </td>
              </tr>
            ) : (
              filtered.map((entry) => (
                <tr
                  key={entry.path}
                  className={`file-row ${selectedPath === entry.path ? "selected" : ""}`}
                  draggable={canTransfer}
                  onDragStart={(e) => handleDragStart(e, entry)}
                  onClick={() => setSelectedPath(entry.path)}
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
                      {entry.isDir ? (
                        <Folder size={15} color="var(--accent)" style={{ flexShrink: 0 }} />
                      ) : (
                        <FileText size={15} color="var(--text-muted)" style={{ flexShrink: 0 }} />
                      )}
                      <span className="file-name-text">{entry.name}</span>
                    </div>
                  </td>
                  <td style={{ color: "var(--text-secondary)" }}>
                    {entry.isDir ? "—" : formatBytes(entry.size)}
                  </td>
                  <td style={{ color: "var(--text-secondary)", fontSize: 12 }}>
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
                        title="重命名"
                        onClick={() => onRename(entry)}
                      >
                        <Pencil size={13} />
                      </button>
                      <button
                        className="icon-btn btn-danger"
                        style={{ width: 24, height: 24 }}
                        title="删除"
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
