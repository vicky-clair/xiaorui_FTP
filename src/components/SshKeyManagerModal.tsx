/**
 * 本地 SSH 密钥管理器与纯 SFTP 一键部署公钥（ssh-copy-id）弹窗
 *
 * - 扫描并展示本机 `~/.ssh` 目录下的全部公钥（算法、SHA-256 指纹、私钥状态，绝不暴露私钥明文）；
 * - 支持一键生成现代 Ed25519 密钥对（可选 Passphrase 加密保护）；
 * - 支持一键复制公钥，或在已连接 SFTP 会话时经由纯 SFTP 安全部署至远端 `~/.ssh/authorized_keys`。
 */

import React, { useEffect, useState } from "react";
import { Check, Copy, KeyRound, Plus, RefreshCw, Send, ShieldCheck, X } from "lucide-react";
import { api } from "../api/tauri";
import type { SessionInfo, SshKeyEntry } from "../types";
import { writeClipboardText } from "../utils/clipboard";

interface SshKeyManagerModalProps {
  activeSftpSession: SessionInfo | null;
  onClose: () => void;
  onNotify: (text: string, kind?: "info" | "success" | "error") => void;
}

export const SshKeyManagerModal: React.FC<SshKeyManagerModalProps> = ({
  activeSftpSession,
  onClose,
  onNotify,
}) => {
  const [keys, setKeys] = useState<SshKeyEntry[]>([]);
  const [loading, setLoading] = useState(true);
  const [creating, setCreating] = useState(false);
  const [busyGen, setBusyGen] = useState(false);
  const [deployingName, setDeployingName] = useState<string | null>(null);
  const [copiedName, setCopiedName] = useState<string | null>(null);

  const [newName, setNewName] = useState(() => {
    const d = new Date().toISOString().slice(0, 10).replace(/-/g, "");
    return `id_ed25519_${d}`;
  });
  const [newComment, setNewComment] = useState("xiaorui_FTP");
  const [newPassphrase, setNewPassphrase] = useState("");

  const refresh = async () => {
    setLoading(true);
    try {
      setKeys(await api.listSshKeys());
    } catch (err) {
      onNotify(String(err), "error");
    } finally {
      setLoading(false);
    }
  };

  useEffect(() => {
    void refresh();
  }, []);

  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if (e.key === "Escape") onClose();
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [onClose]);

  const handleGenerate = async (e: React.FormEvent) => {
    e.preventDefault();
    if (!newName.trim()) {
      onNotify("请输入密钥文件名（如 id_ed25519_server）", "error");
      return;
    }
    setBusyGen(true);
    try {
      const created = await api.generateSshKey(
        newName.trim(),
        newComment.trim(),
        newPassphrase || undefined
      );
      setNewPassphrase("");
      setCreating(false);
      await refresh();
      onNotify(`已生成 Ed25519 密钥对「${created.name}」并保存至 ~/.ssh`, "success");
    } catch (err) {
      onNotify(String(err), "error");
    } finally {
      setBusyGen(false);
    }
  };

  const handleDeploy = async (item: SshKeyEntry) => {
    if (!activeSftpSession) {
      onNotify("请先连接一个 SFTP/SSH 站点后再一键部署公钥", "info");
      return;
    }
    setDeployingName(item.name);
    try {
      const added = await api.deploySshPublicKey(
        activeSftpSession.sessionId,
        item.publicKeyLine
      );
      onNotify(
        added
          ? `已通过安全 SFTP 将「${item.name}」部署到 ${activeSftpSession.host} 的 ~/.ssh/authorized_keys`
          : `公钥「${item.name}」已存在于 ${activeSftpSession.host}，无需重复部署`,
        "success"
      );
    } catch (err) {
      onNotify(`部署公钥失败：${String(err)}`, "error");
    } finally {
      setDeployingName(null);
    }
  };

  return (
    <div className="modal-backdrop" onClick={onClose}>
      <div className="modal" style={{ maxWidth: 680 }} onClick={(e) => e.stopPropagation()}>
        <div className="modal-header">
          <span style={{ display: "inline-flex", alignItems: "center", gap: 8 }}>
            <KeyRound size={16} color="var(--accent)" />
            SSH 密钥管理与一键公钥部署
          </span>
          <button className="icon-btn" onClick={onClose}>
            <X size={16} />
          </button>
        </div>

        <div className="modal-body" style={{ maxHeight: 480, gap: 12 }}>
          <div
            style={{
              display: "flex",
              alignItems: "center",
              justifyContent: "space-between",
              gap: 8,
              flexWrap: "wrap",
            }}
          >
            <span style={{ fontSize: 12, color: "var(--text-muted)" }}>
              私钥严格保留在本地 <code>~/.ssh</code>（Unix 权限 0600），支持纯 SFTP 零注入部署公钥。
            </span>
            <div style={{ display: "inline-flex", gap: 6 }}>
              <button className="btn btn-sm" onClick={() => void refresh()}>
                <RefreshCw size={12} />
                刷新
              </button>
              <button
                className="btn btn-sm btn-primary"
                onClick={() => setCreating((v) => !v)}
              >
                <Plus size={12} />
                生成 Ed25519 密钥
              </button>
            </div>
          </div>

          {creating && (
            <form
              onSubmit={handleGenerate}
              style={{
                padding: 12,
                borderRadius: "var(--radius-sm)",
                border: "1px solid var(--border)",
                background: "var(--surface-subtle)",
                display: "flex",
                flexDirection: "column",
                gap: 8,
              }}
            >
              <div style={{ fontWeight: 600, fontSize: 12.5 }}>新建现代 Ed25519 密钥对</div>
              <div style={{ display: "grid", gridTemplateColumns: "1fr 1fr", gap: 8 }}>
                <label style={{ fontSize: 11.5, display: "flex", flexDirection: "column", gap: 4 }}>
                  密钥文件名（保存在 ~/.ssh）
                  <input
                    className="form-control"
                    value={newName}
                    onChange={(e) => setNewName(e.target.value)}
                    placeholder="id_ed25519_prod"
                    required
                  />
                </label>
                <label style={{ fontSize: 11.5, display: "flex", flexDirection: "column", gap: 4 }}>
                  公钥注释（Comment）
                  <input
                    className="form-control"
                    value={newComment}
                    onChange={(e) => setNewComment(e.target.value)}
                    placeholder="user@workstation"
                  />
                </label>
              </div>
              <label style={{ fontSize: 11.5, display: "flex", flexDirection: "column", gap: 4 }}>
                私钥保护口令（Passphrase，可选，推荐设置强口令）
                <input
                  className="form-control"
                  type="password"
                  value={newPassphrase}
                  onChange={(e) => setNewPassphrase(e.target.value)}
                  placeholder="留空则生成无口令私钥"
                />
              </label>
              <div style={{ display: "flex", justifyContent: "flex-end", gap: 8, marginTop: 4 }}>
                <button
                  type="button"
                  className="btn btn-sm"
                  onClick={() => setCreating(false)}
                  disabled={busyGen}
                >
                  取消
                </button>
                <button type="submit" className="btn btn-sm btn-primary" disabled={busyGen}>
                  <ShieldCheck size={13} />
                  {busyGen ? "生成中..." : "立即生成"}
                </button>
              </div>
            </form>
          )}

          {loading ? (
            <div style={{ padding: "24px 0", textAlign: "center", color: "var(--text-muted)" }}>
              正在扫描 ~/.ssh 目录...
            </div>
          ) : keys.length === 0 ? (
            <div style={{ padding: "28px 0", textAlign: "center", color: "var(--text-muted)" }}>
              本机 <code>~/.ssh</code> 目录下暂未找到 `.pub` 公钥文件，可点击右上角一键生成 Ed25519 密钥。
            </div>
          ) : (
            <div style={{ display: "flex", flexDirection: "column", gap: 8 }}>
              {keys.map((item) => (
                <div
                  key={item.publicKeyPath}
                  style={{
                    padding: "10px 12px",
                    borderRadius: "var(--radius-sm)",
                    border: "1px solid var(--border)",
                    background: "var(--surface-subtle)",
                    display: "flex",
                    alignItems: "center",
                    gap: 10,
                  }}
                >
                  <div style={{ flex: 1, minWidth: 0 }}>
                    <div style={{ display: "flex", alignItems: "center", gap: 6, flexWrap: "wrap" }}>
                      <strong style={{ fontFamily: "var(--font-mono)", fontSize: 12.5 }}>
                        {item.name}
                      </strong>
                      <span className="proto-badge badge-sftp">{item.algorithm}</span>
                      {item.hasPrivateKey && (
                        <span style={{ fontSize: 11, color: "var(--success)" }}>含本地私钥</span>
                      )}
                      {item.comment && (
                        <span style={{ fontSize: 11, color: "var(--text-muted)" }}>
                          · {item.comment}
                        </span>
                      )}
                    </div>
                    <div
                      style={{
                        fontFamily: "var(--font-mono)",
                        fontSize: 11,
                        color: "var(--text-secondary)",
                        marginTop: 3,
                        overflow: "hidden",
                        textOverflow: "ellipsis",
                      }}
                      title={item.fingerprint}
                    >
                      {item.fingerprint}
                    </div>
                  </div>

                  <div style={{ display: "inline-flex", gap: 6, flexShrink: 0 }}>
                    <button
                      className="btn btn-sm"
                      onClick={async () => {
                        if (await writeClipboardText(item.publicKeyLine)) {
                          setCopiedName(item.name);
                          window.setTimeout(() => setCopiedName(null), 1600);
                        }
                      }}
                      title="复制 OpenSSH 公钥内容"
                    >
                      {copiedName === item.name ? <Check size={12} /> : <Copy size={12} />}
                      {copiedName === item.name ? "已复制" : "公钥"}
                    </button>
                    <button
                      className="btn btn-sm btn-primary"
                      disabled={!activeSftpSession || deployingName === item.name}
                      onClick={() => void handleDeploy(item)}
                      title={
                        activeSftpSession
                          ? `一键部署到当前连接的 ${activeSftpSession.host} (~/.ssh/authorized_keys)`
                          : "请先连接一个 SFTP/SSH 站点"
                      }
                    >
                      <Send size={12} />
                      {deployingName === item.name
                        ? "部署中..."
                        : activeSftpSession
                          ? `部署到 ${activeSftpSession.host}`
                          : "部署（需连接）"}
                    </button>
                  </div>
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
