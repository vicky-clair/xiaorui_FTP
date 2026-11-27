/**
 * 本地脱敏安全审计日志查看与导出弹窗
 *
 * 展示后端 `audit.log.jsonl` 中记录的主机指纹核验、会话连接、凭据操作、高危文件变更
 * 及安全策略拦截事件，支持按分类筛选、关键词检索与导出为脱敏 JSON。
 */

import React, { useEffect, useMemo, useState } from "react";
import { save } from "@tauri-apps/plugin-dialog";
import { Download, FileClock, RefreshCw, Search, X } from "lucide-react";
import { api } from "../api/tauri";
import {
  formatTime,
  type AuditCategory,
  type AuditEntry,
  type AuditSeverity,
} from "../types";

interface AuditLogModalProps {
  onClose: () => void;
  onNotify: (text: string, kind?: "info" | "success" | "error") => void;
}

const SEVERITY_META: Record<AuditSeverity, { label: string; color: string; bg: string }> = {
  info: { label: "信息", color: "var(--accent)", bg: "rgba(59, 130, 246, 0.12)" },
  warn: { label: "警告", color: "#d97706", bg: "rgba(217, 119, 6, 0.14)" },
  high: { label: "高危", color: "#ea580c", bg: "rgba(234, 88, 12, 0.16)" },
  critical: { label: "阻断", color: "var(--danger)", bg: "rgba(239, 68, 68, 0.16)" },
};

const CATEGORY_LABEL: Record<AuditCategory, string> = {
  host_key: "主机指纹",
  session: "会话连接",
  credential: "站点凭据",
  file_op: "文件操作",
  security_policy: "安全策略",
};

export const AuditLogModal: React.FC<AuditLogModalProps> = ({ onClose, onNotify }) => {
  const [entries, setEntries] = useState<AuditEntry[]>([]);
  const [loading, setLoading] = useState(true);
  const [categoryFilter, setCategoryFilter] = useState<AuditCategory | "all">("all");
  const [query, setQuery] = useState("");

  const loadLogs = async () => {
    setLoading(true);
    try {
      setEntries(await api.listAuditLogs(300));
    } catch (err) {
      onNotify(String(err), "error");
    } finally {
      setLoading(false);
    }
  };

  useEffect(() => {
    void loadLogs();
  }, []);

  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if (e.key === "Escape") onClose();
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [onClose]);

  const filtered = useMemo(() => {
    const q = query.trim().toLowerCase();
    return entries.filter((item) => {
      if (categoryFilter !== "all" && item.category !== categoryFilter) return false;
      if (!q) return true;
      return (
        item.event.toLowerCase().includes(q) ||
        item.target.toLowerCase().includes(q) ||
        item.detail.toLowerCase().includes(q)
      );
    });
  }, [entries, categoryFilter, query]);

  const handleExport = async () => {
    try {
      const stamp = new Date().toISOString().replace(/[:.]/g, "-").slice(0, 19);
      const target = await save({
        defaultPath: `xiaorui_FTP-audit-${stamp}.json`,
        filters: [{ name: "JSON 日志文件", extensions: ["json"] }],
        title: "导出脱敏安全审计日志",
      });
      if (!target) return;
      await api.exportAuditLogs(target);
      onNotify(`安全日志已导出至 ${target}`, "success");
    } catch (err) {
      onNotify(String(err), "error");
    }
  };

  return (
    <div className="modal-backdrop" onClick={onClose}>
      <div className="modal" style={{ maxWidth: 760 }} onClick={(e) => e.stopPropagation()}>
        <div className="modal-header">
          <span style={{ display: "inline-flex", alignItems: "center", gap: 8 }}>
            <FileClock size={16} color="var(--accent)" />
            本地安全审计日志（严格脱敏）
          </span>
          <button className="icon-btn" onClick={onClose}>
            <X size={16} />
          </button>
        </div>

        <div className="modal-body" style={{ maxHeight: 470, gap: 10 }}>
          <div style={{ fontSize: 12, color: "var(--text-muted)" }}>
            记录本地主机指纹核验、会话连接、凭据增删、高危文件变更及安全拦截事件；永不记录密码、私钥或终端输入内容，并自动按 2 MB 轮转。
          </div>

          <div style={{ display: "flex", alignItems: "center", gap: 8, flexWrap: "wrap" }}>
            <div className="search-box" style={{ flex: 1, minWidth: 180 }}>
              <Search size={13} color="var(--text-muted)" />
              <input
                value={query}
                onChange={(e) => setQuery(e.target.value)}
                placeholder="筛选事件、主机或详情..."
              />
            </div>

            <select
              className="form-control"
              style={{ width: 132, height: 30, fontSize: 12 }}
              value={categoryFilter}
              onChange={(e) => setCategoryFilter(e.target.value as AuditCategory | "all")}
            >
              <option value="all">全部分类</option>
              <option value="host_key">主机指纹</option>
              <option value="session">会话连接</option>
              <option value="credential">站点凭据</option>
              <option value="file_op">文件操作</option>
              <option value="security_policy">安全策略</option>
            </select>

            <button className="btn btn-sm" onClick={() => void loadLogs()} title="刷新日志">
              <RefreshCw size={13} />
              刷新
            </button>
            <button className="btn btn-sm" onClick={() => void handleExport()} title="导出 JSON">
              <Download size={13} />
              导出
            </button>
          </div>

          {loading ? (
            <div style={{ padding: "28px 0", textAlign: "center", color: "var(--text-muted)" }}>
              加载中...
            </div>
          ) : filtered.length === 0 ? (
            <div style={{ padding: "28px 0", textAlign: "center", color: "var(--text-muted)" }}>
              暂无匹配的安全审计记录。
            </div>
          ) : (
            <div style={{ display: "flex", flexDirection: "column", gap: 6 }}>
              {filtered.map((item, idx) => {
                const sev = SEVERITY_META[item.severity] ?? SEVERITY_META.info;
                return (
                  <div
                    key={`${item.timestamp}-${item.event}-${idx}`}
                    style={{
                      padding: "8px 10px",
                      borderRadius: "var(--radius-sm)",
                      border: "1px solid var(--border)",
                      background: "var(--surface-subtle)",
                      fontSize: 12,
                      display: "flex",
                      flexDirection: "column",
                      gap: 3,
                    }}
                  >
                    <div style={{ display: "flex", alignItems: "center", gap: 8, flexWrap: "wrap" }}>
                      <span
                        style={{
                          padding: "1px 6px",
                          borderRadius: 4,
                          fontSize: 11,
                          fontWeight: 600,
                          color: sev.color,
                          background: sev.bg,
                        }}
                      >
                        {sev.label}
                      </span>
                      <span
                        style={{
                          fontSize: 11,
                          color: "var(--text-muted)",
                          padding: "1px 6px",
                          borderRadius: 4,
                          border: "1px solid var(--border)",
                        }}
                      >
                        {CATEGORY_LABEL[item.category] ?? item.category}
                      </span>
                      <code style={{ fontFamily: "var(--font-mono)", fontSize: 11.5 }}>
                        {item.event}
                      </code>
                      <span
                        style={{
                          fontFamily: "var(--font-mono)",
                          fontSize: 11.5,
                          color: "var(--text-secondary)",
                        }}
                      >
                        {item.target}
                      </span>
                      <span
                        style={{
                          marginLeft: "auto",
                          fontSize: 11,
                          color: "var(--text-muted)",
                        }}
                      >
                        {formatTime(item.timestamp)}
                      </span>
                    </div>
                    <div style={{ color: "var(--text-secondary)", lineHeight: 1.45 }}>
                      {item.detail}
                    </div>
                  </div>
                );
              })}
            </div>
          )}
        </div>

        <div className="modal-footer">
          <span style={{ marginRight: "auto", fontSize: 11.5, color: "var(--text-muted)" }}>
            共显示 {filtered.length} / {entries.length} 条记录
          </span>
          <button className="btn btn-primary" onClick={onClose}>
            关闭
          </button>
        </div>
      </div>
    </div>
  );
};
