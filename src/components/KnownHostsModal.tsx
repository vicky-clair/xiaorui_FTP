/**
 * SSH 已知主机公钥指纹管理弹窗（TOFU）
 *
 * 展示本地 `known_hosts.json` 中已固定的 SSH 主机 SHA-256 指纹，
 * 当目标服务器合法重装系统或更换主机密钥时，用户可在此精准移除对应旧指纹后重新连接。
 */

import React, { useEffect, useState } from "react";
import { ShieldCheck, Trash2, X } from "lucide-react";
import { api } from "../api/tauri";
import { formatTime, type KnownHostEntry } from "../types";

interface KnownHostsModalProps {
  onClose: () => void;
  onNotify: (text: string, kind?: "info" | "success" | "error") => void;
}

export const KnownHostsModal: React.FC<KnownHostsModalProps> = ({
  onClose,
  onNotify,
}) => {
  const [entries, setEntries] = useState<KnownHostEntry[]>([]);
  const [loading, setLoading] = useState(true);

  const refresh = async () => {
    setLoading(true);
    try {
      setEntries(await api.listKnownHosts());
    } catch (err) {
      onNotify(String(err), "error");
    } finally {
      setLoading(false);
    }
  };

  useEffect(() => {
    void refresh();
  }, []);

  const handleRemove = async (hostPort: string) => {
    try {
      await api.removeKnownHost(hostPort);
      setEntries((prev) => prev.filter((e) => e.hostPort !== hostPort));
      onNotify(`已移除主机 ${hostPort} 的指纹记录`, "success");
    } catch (err) {
      onNotify(String(err), "error");
    }
  };

  return (
    <div className="modal-backdrop" onClick={onClose}>
      <div className="modal" style={{ maxWidth: 640 }} onClick={(e) => e.stopPropagation()}>
        <div className="modal-header">
          <span style={{ display: "inline-flex", alignItems: "center", gap: 8 }}>
            <ShieldCheck size={16} color="var(--accent)" />
            SSH 已知主机指纹管理（TOFU）
          </span>
          <button className="icon-btn" onClick={onClose}>
            <X size={16} />
          </button>
        </div>

        <div className="modal-body" style={{ maxHeight: 420 }}>
          <div style={{ fontSize: 12, color: "var(--text-muted)" }}>
            首次连接 SSH 主机时会自动记录其公钥 SHA-256 指纹并在后续连接中严格核验。若某台服务器已重装系统导致指纹变化，请在此移除旧记录后再重连。
          </div>

          {loading ? (
            <div style={{ padding: "24px 0", textAlign: "center", color: "var(--text-muted)" }}>
              加载中...
            </div>
          ) : entries.length === 0 ? (
            <div style={{ padding: "28px 0", textAlign: "center", color: "var(--text-muted)" }}>
              暂无已记录的 SSH 主机指纹。
            </div>
          ) : (
            <div style={{ display: "flex", flexDirection: "column", gap: 8 }}>
              {entries.map((item) => (
                <div
                  key={item.hostPort}
                  style={{
                    display: "flex",
                    alignItems: "center",
                    gap: 10,
                    padding: "10px 12px",
                    borderRadius: "var(--radius-sm)",
                    border: "1px solid var(--border)",
                    background: "var(--surface-subtle)",
                  }}
                >
                  <div style={{ flex: 1, minWidth: 0 }}>
                    <div style={{ fontWeight: 600, fontFamily: "var(--font-mono)" }}>
                      {item.hostPort}{" "}
                      <span
                        style={{
                          fontWeight: 400,
                          fontSize: 11.5,
                          color: "var(--text-muted)",
                        }}
                      >
                        ({item.algorithm})
                      </span>
                    </div>
                    <div
                      style={{
                        fontFamily: "var(--font-mono)",
                        fontSize: 11.5,
                        color: "var(--text-secondary)",
                        overflow: "hidden",
                        textOverflow: "ellipsis",
                      }}
                      title={item.fingerprint}
                    >
                      {item.fingerprint}
                    </div>
                    <div style={{ fontSize: 11, color: "var(--text-muted)", marginTop: 2 }}>
                      首次信任：{formatTime(item.firstSeenAt)} · 最近校验：{formatTime(item.lastSeenAt)}
                    </div>
                  </div>
                  <button
                    className="btn btn-sm btn-danger"
                    onClick={() => void handleRemove(item.hostPort)}
                    title="移除该主机指纹记录"
                  >
                    <Trash2 size={13} />
                    移除
                  </button>
                </div>
              ))}
            </div>
          )}
        </div>

        <div className="modal-footer">
          <button className="btn btn-primary" onClick={onClose}>
            完成
          </button>
        </div>
      </div>
    </div>
  );
};
