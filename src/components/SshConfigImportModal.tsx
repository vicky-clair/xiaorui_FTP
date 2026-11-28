/**
 * OpenSSH `~/.ssh/config` 主机配置一键解析与勾选导入弹窗
 *
 * 自动读取本地 `~/.ssh/config`（支持 HostName、User、Port、IdentityFile、ProxyJump 与 Host * 默认值），
 * 勾选后批量导入至站点列表「SSH Config」分组，自动跳过重复站点，绝不覆盖既有数据。
 */

import React, { useEffect, useState } from "react";
import { FileCode2, Import, Server, X } from "lucide-react";
import { api } from "../api/tauri";
import type { SshConfigHost } from "../types";

interface SshConfigImportModalProps {
  onImported: (count: number) => void;
  onClose: () => void;
  onNotify: (text: string, kind?: "info" | "success" | "error") => void;
}

export const SshConfigImportModal: React.FC<SshConfigImportModalProps> = ({
  onImported,
  onClose,
  onNotify,
}) => {
  const [hosts, setHosts] = useState<SshConfigHost[]>([]);
  const [selected, setSelected] = useState<Set<string>>(new Set());
  const [loading, setLoading] = useState(true);
  const [busy, setBusy] = useState(false);

  useEffect(() => {
    (async () => {
      setLoading(true);
      try {
        const list = await api.discoverSshConfigHosts();
        setHosts(list);
        setSelected(new Set(list.map((h) => h.alias)));
      } catch (err) {
        onNotify(String(err), "error");
      } finally {
        setLoading(false);
      }
    })();
  }, [onNotify]);

  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if (e.key === "Escape" && !busy) onClose();
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [busy, onClose]);

  const toggle = (alias: string) => {
    setSelected((prev) => {
      const next = new Set(prev);
      if (next.has(alias)) next.delete(alias);
      else next.add(alias);
      return next;
    });
  };

  const handleImport = async () => {
    const chosen = hosts.filter((h) => selected.has(h.alias));
    if (chosen.length === 0) return;
    setBusy(true);
    try {
      const count = await api.importSshConfigHosts(chosen);
      onImported(count);
      onClose();
    } catch (err) {
      onNotify(String(err), "error");
    } finally {
      setBusy(false);
    }
  };

  return (
    <div className="modal-backdrop" onClick={() => !busy && onClose()}>
      <div className="modal" style={{ maxWidth: 620 }} onClick={(e) => e.stopPropagation()}>
        <div className="modal-header">
          <span style={{ display: "inline-flex", alignItems: "center", gap: 8 }}>
            <FileCode2 size={16} color="var(--accent)" />
            从 ~/.ssh/config 一键导入 SSH 主机
          </span>
          <button className="icon-btn" disabled={busy} onClick={onClose}>
            <X size={16} />
          </button>
        </div>

        <div className="modal-body" style={{ maxHeight: 440, gap: 10 }}>
          <div style={{ fontSize: 12, color: "var(--text-muted)" }}>
            已自动解析本机 <code>~/.ssh/config</code>（含 <code>Host *</code> 默认项、私钥路径与 <code>ProxyJump</code> 跳板机）。导入时会自动跳过已存在的相同主机，绝不覆盖既有配置。
          </div>

          {loading ? (
            <div style={{ padding: "28px 0", textAlign: "center", color: "var(--text-muted)" }}>
              正在解析 ~/.ssh/config...
            </div>
          ) : hosts.length === 0 ? (
            <div style={{ padding: "28px 0", textAlign: "center", color: "var(--text-muted)" }}>
              未在 <code>~/.ssh/config</code> 中发现可导入的具名主机条目。
            </div>
          ) : (
            <div style={{ display: "flex", flexDirection: "column", gap: 6 }}>
              {hosts.map((h) => {
                const checked = selected.has(h.alias);
                return (
                  <label
                    key={h.alias}
                    style={{
                      display: "flex",
                      alignItems: "center",
                      gap: 10,
                      padding: "9px 11px",
                      borderRadius: "var(--radius-sm)",
                      border: `1px solid ${checked ? "var(--accent)" : "var(--border)"}`,
                      background: "var(--surface-subtle)",
                      cursor: "pointer",
                    }}
                  >
                    <input
                      type="checkbox"
                      checked={checked}
                      onChange={() => toggle(h.alias)}
                    />
                    <Server size={14} color="var(--accent)" style={{ flexShrink: 0 }} />
                    <div style={{ flex: 1, minWidth: 0 }}>
                      <div style={{ display: "flex", alignItems: "center", gap: 6 }}>
                        <strong style={{ fontSize: 12.5 }}>{h.alias}</strong>
                        {h.jumpHost && <span className="site-jump-tag">跳板: {h.jumpHost}</span>}
                      </div>
                      <div
                        style={{
                          fontSize: 11.5,
                          color: "var(--text-secondary)",
                          fontFamily: "var(--font-mono)",
                        }}
                      >
                        {h.username ? `${h.username}@` : ""}
                        {h.host}:{h.port}
                        {h.privateKeyPath ? ` · 私钥: ${h.privateKeyPath}` : " · ssh-agent"}
                      </div>
                    </div>
                  </label>
                );
              })}
            </div>
          )}
        </div>

        <div className="modal-footer">
          <span style={{ marginRight: "auto", fontSize: 12, color: "var(--text-muted)" }}>
            已选 {selected.size} / {hosts.length} 台主机
          </span>
          <button className="btn" disabled={busy} onClick={onClose}>
            取消
          </button>
          <button
            className="btn btn-primary"
            disabled={busy || selected.size === 0}
            onClick={() => void handleImport()}
          >
            <Import size={13} />
            {busy ? "导入中..." : `导入所选 (${selected.size})`}
          </button>
        </div>
      </div>
    </div>
  );
};
