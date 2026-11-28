/**
 * SSH 主机公钥指纹安全确认与中间人攻击阻断告警弹窗
 *
 * - 首次连接未知主机（`unknown`）：展示主机地址、跳板机标识、公钥算法与 SHA-256 指纹，
 *   经用户核实并明确确认后才写入本地 `known_hosts.json` 并继续连接；
 * - 主机指纹发生变更（`mismatch`）：高危阻断性告警弹窗，对比已记录指纹与当前指纹，
 *   禁止一键静默覆盖，用户必须先经可信渠道核实并在「已知主机管理」中显式移除旧记录。
 */

import React, { useEffect, useState } from "react";
import { AlertOctagon, Copy, ShieldAlert, ShieldCheck, X } from "lucide-react";
import { writeClipboardText } from "../utils/clipboard";
import type { HostKeyPrompt } from "../utils/security";

interface HostKeyModalProps {
  prompt: HostKeyPrompt;
  onTrustUnknown: (hostPort: string, algorithm: string, fingerprint: string) => Promise<void>;
  onOpenKnownHosts: () => void;
  onClose: () => void;
}

export const HostKeyModal: React.FC<HostKeyModalProps> = ({
  prompt,
  onTrustUnknown,
  onOpenKnownHosts,
  onClose,
}) => {
  const [busy, setBusy] = useState(false);
  const [copied, setCopied] = useState(false);

  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if (e.key === "Escape" && !busy) onClose();
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [busy, onClose]);

  const copyText = (text: string) => {
    void writeClipboardText(text).then((ok) => {
      if (!ok) return;
      setCopied(true);
      window.setTimeout(() => setCopied(false), 1800);
    });
  };

  if (prompt.kind === "mismatch") {
    const { info } = prompt;
    return (
      <div className="modal-backdrop" onClick={onClose}>
        <div
          className="modal"
          style={{
            maxWidth: 560,
            border: "1.5px solid var(--danger)",
          }}
          onClick={(e) => e.stopPropagation()}
        >
          <div
            className="modal-header"
            style={{ background: "rgba(239, 68, 68, 0.12)", color: "var(--danger)" }}
          >
            <span style={{ display: "inline-flex", alignItems: "center", gap: 8, fontWeight: 700 }}>
              <AlertOctagon size={18} />
              高危安全阻断：SSH 主机公钥指纹已发生变化
            </span>
            <button className="icon-btn" onClick={onClose}>
              <X size={16} />
            </button>
          </div>

          <div className="modal-body" style={{ gap: 12 }}>
            <div
              style={{
                padding: "10px 12px",
                borderRadius: "var(--radius-sm)",
                background: "rgba(239, 68, 68, 0.08)",
                border: "1px solid rgba(239, 68, 68, 0.3)",
                fontSize: 12.5,
                lineHeight: 1.6,
              }}
            >
              <strong>连接已被立即阻断！</strong>
              {info.isJumpHost ? "跳板机 " : "目标服务器 "}
              <code style={{ fontFamily: "var(--font-mono)" }}>{info.hostPort}</code> 返回的公钥与本地已固定指纹不一致。
              这可能意味着您正在遭受<strong>中间人攻击（MITM）</strong>或 DNS 劫持，也可能是服务器近期重装系统更换了密钥。
            </div>

            <div style={{ display: "flex", flexDirection: "column", gap: 8, fontSize: 12 }}>
              <div
                style={{
                  padding: "9px 11px",
                  borderRadius: "var(--radius-sm)",
                  background: "var(--surface-subtle)",
                  border: "1px solid var(--border)",
                }}
              >
                <div style={{ color: "var(--text-muted)", marginBottom: 3 }}>
                  已记录的受信任指纹（{info.expectedAlgorithm}）
                </div>
                <div style={{ fontFamily: "var(--font-mono)", wordBreak: "break-all" }}>
                  {info.expectedFingerprint}
                </div>
              </div>

              <div
                style={{
                  padding: "9px 11px",
                  borderRadius: "var(--radius-sm)",
                  background: "rgba(239, 68, 68, 0.06)",
                  border: "1px solid rgba(239, 68, 68, 0.35)",
                }}
              >
                <div style={{ color: "var(--danger)", marginBottom: 3, fontWeight: 600 }}>
                  当前服务器返回的指纹（{info.actualAlgorithm}）
                </div>
                <div style={{ fontFamily: "var(--font-mono)", wordBreak: "break-all" }}>
                  {info.actualFingerprint}
                </div>
              </div>
            </div>

            <div style={{ fontSize: 12, color: "var(--text-muted)", lineHeight: 1.55 }}>
              为保障安全，本客户端<strong>禁止静默覆盖</strong>已记录指纹。若您已通过控制台或带外渠道确认服务器确实合法重装，请点击下方打开「已知主机管理」，手动移除旧记录后再重新连接。
            </div>
          </div>

          <div className="modal-footer">
            <button
              className="btn"
              onClick={() => {
                onClose();
                onOpenKnownHosts();
              }}
            >
              打开已知主机管理
            </button>
            <button className="btn btn-danger" onClick={onClose}>
              保持阻断并关闭
            </button>
          </div>
        </div>
      </div>
    );
  }

  const { info } = prompt;
  return (
    <div className="modal-backdrop" onClick={() => !busy && onClose()}>
      <div className="modal" style={{ maxWidth: 540 }} onClick={(e) => e.stopPropagation()}>
        <div className="modal-header">
          <span style={{ display: "inline-flex", alignItems: "center", gap: 8 }}>
            <ShieldAlert size={17} color="var(--warning, #d97706)" />
            首次连接：请核对 SSH 主机公钥指纹
          </span>
          <button className="icon-btn" disabled={busy} onClick={onClose}>
            <X size={16} />
          </button>
        </div>

        <div className="modal-body" style={{ gap: 12 }}>
          <div style={{ fontSize: 12.5, lineHeight: 1.6, color: "var(--text-secondary)" }}>
            您正在首次连接{info.isJumpHost ? " SSH 跳板机 " : " SSH 服务器 "}
            <strong style={{ fontFamily: "var(--font-mono)" }}>{info.hostPort}</strong>。
            系统无法自动验证该服务器的真实身份，请在服务器控制台核对以下指纹无误后再确认信任：
          </div>

          <div
            style={{
              padding: "10px 12px",
              borderRadius: "var(--radius-sm)",
              background: "var(--surface-subtle)",
              border: "1px solid var(--border)",
              display: "flex",
              flexDirection: "column",
              gap: 6,
              fontSize: 12,
            }}
          >
            <div>
              <span style={{ color: "var(--text-muted)" }}>主机地址：</span>
              <span style={{ fontFamily: "var(--font-mono)", fontWeight: 600 }}>
                {info.hostPort}
              </span>
            </div>
            <div>
              <span style={{ color: "var(--text-muted)" }}>公钥算法：</span>
              <span style={{ fontFamily: "var(--font-mono)" }}>{info.algorithm}</span>
            </div>
            <div>
              <span style={{ color: "var(--text-muted)" }}>SHA-256 指纹：</span>
              <div
                style={{
                  marginTop: 4,
                  display: "flex",
                  alignItems: "center",
                  gap: 8,
                  padding: "6px 8px",
                  borderRadius: 4,
                  background: "var(--bg)",
                  border: "1px solid var(--border)",
                }}
              >
                <code
                  style={{
                    flex: 1,
                    fontFamily: "var(--font-mono)",
                    fontSize: 11.5,
                    wordBreak: "break-all",
                  }}
                >
                  {info.fingerprint}
                </code>
                <button
                  type="button"
                  className="btn btn-sm btn-ghost"
                  onClick={() => copyText(info.fingerprint)}
                  title="复制 SHA-256 指纹"
                >
                  <Copy size={12} />
                  {copied ? "已复制" : "复制"}
                </button>
              </div>
            </div>
          </div>

          <div style={{ fontSize: 11.5, color: "var(--text-muted)" }}>
            确认后，该指纹将保存至本地已知主机仓库；后续每次连接都会严格比对，若发生变更将立即阻断连接。
          </div>
        </div>

        <div className="modal-footer">
          <button className="btn" disabled={busy} onClick={onClose}>
            取消连接
          </button>
          <button
            className="btn btn-primary"
            disabled={busy}
            onClick={async () => {
              setBusy(true);
              try {
                await onTrustUnknown(info.hostPort, info.algorithm, info.fingerprint);
              } finally {
                setBusy(false);
              }
            }}
          >
            <ShieldCheck size={14} />
            {busy ? "正在信任并连接..." : "确认指纹并继续连接"}
          </button>
        </div>
      </div>
    </div>
  );
};
