/**
 * 底部可折叠并发传输队列抽屉组件
 *
 * 实时展示上传/下载任务进度条、已传输字节、实时传输速率与错误信息，
 * 支持取消正在进行的任务和一键清除已结束记录。
 */

import React from "react";
import { ArrowDown, ArrowUp, Trash2, X } from "lucide-react";
import { formatBytes, type TransferTask } from "../types";

interface TransferDrawerProps {
  tasks: TransferTask[];
  onCancel: (taskId: string) => void;
  onClearFinished: () => void;
  onClose: () => void;
}

/** 传输状态中文展示映射 */
const STATUS_LABEL: Record<TransferTask["status"], string> = {
  queued: "排队中",
  running: "传输中",
  completed: "已完成",
  failed: "失败",
  cancelled: "已取消",
};

export const TransferDrawer: React.FC<TransferDrawerProps> = ({
  tasks,
  onCancel,
  onClearFinished,
  onClose,
}) => {
  const activeCount = tasks.filter(
    (t) => t.status === "running" || t.status === "queued"
  ).length;
  const doneCount = tasks.filter((t) => t.status === "completed").length;

  return (
    <div className="bottom-drawer">
      <div className="drawer-header">
        <span>传输队列</span>
        <span className={`drawer-pill ${activeCount > 0 ? "active" : ""}`}>
          进行中 {activeCount}
        </span>
        <span className="drawer-pill">已完成 {doneCount}</span>
        <div style={{ marginLeft: "auto", display: "flex", gap: 6 }}>
          <button className="btn btn-sm btn-ghost" onClick={onClearFinished}>
            <Trash2 size={12} />
            清除已结束
          </button>
          <button className="icon-btn" style={{ width: 24, height: 24 }} onClick={onClose}>
            <X size={14} />
          </button>
        </div>
      </div>

      <div className="drawer-body">
        {tasks.length === 0 ? (
          <div style={{ padding: "28px 0", textAlign: "center", color: "var(--text-muted)" }}>
            暂无传输任务。点击上传/下载按钮或直接拖入文件/文件夹即可开始并发传输。
          </div>
        ) : (
          tasks.map((t) => {
            const pct =
              t.status === "completed"
                ? 100
                : t.totalBytes > 0
                  ? Math.min(100, Math.round((t.transferredBytes / t.totalBytes) * 100))
                  : 0;

            return (
              <div key={t.id} className="transfer-row">
                <span
                  style={{
                    color: t.direction === "upload" ? "var(--accent)" : "var(--success)",
                    display: "grid",
                    placeItems: "center",
                  }}
                  title={t.direction === "upload" ? "上传" : "下载"}
                >
                  {t.direction === "upload" ? <ArrowUp size={15} /> : <ArrowDown size={15} />}
                </span>

                <div style={{ minWidth: 0 }}>
                  <div
                    style={{
                      fontWeight: 500,
                      overflow: "hidden",
                      textOverflow: "ellipsis",
                      whiteSpace: "nowrap",
                    }}
                    title={`${t.localPath} ↔ ${t.remotePath}`}
                  >
                    {t.fileName}
                  </div>
                  {t.error && (
                    <div style={{ fontSize: 11, color: "var(--danger)" }}>{t.error}</div>
                  )}
                </div>

                <div>
                  <div className="progress-track">
                    <div
                      className={`progress-fill ${t.status}`}
                      style={{ width: `${pct}%` }}
                    />
                  </div>
                </div>

                <span
                  className="tabular-nums"
                  style={{ color: "var(--text-secondary)", textAlign: "right" }}
                >
                  {formatBytes(t.transferredBytes)}
                  {t.totalBytes > 0 ? ` / ${formatBytes(t.totalBytes)}` : ""}
                </span>

                <span
                  className="tabular-nums"
                  style={{ color: "var(--text-muted)", textAlign: "right" }}
                >
                  {t.status === "running" && t.speedBps > 0
                    ? `${formatBytes(t.speedBps)}/s`
                    : STATUS_LABEL[t.status]}
                </span>

                <div>
                  {(t.status === "running" || t.status === "queued") && (
                    <button
                      className="icon-btn btn-danger"
                      style={{ width: 24, height: 24 }}
                      onClick={() => onCancel(t.id)}
                      title="取消任务"
                    >
                      <X size={13} />
                    </button>
                  )}
                </div>
              </div>
            );
          })
        )}
      </div>
    </div>
  );
};
