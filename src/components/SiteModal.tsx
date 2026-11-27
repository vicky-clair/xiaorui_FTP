/**
 * 新建与编辑站点弹窗组件
 *
 * 根据所选协议（SFTP / FTPS / FTP / S3 / WebDAV）动态展示精简表单字段，
 * 支持 S3 常见云厂商一键预设、本地 ssh-agent 状态实时检测以及可折叠的 SSH 跳板机（ProxyJump）配置。
 */

import React, { useEffect, useState } from "react";
import { open } from "@tauri-apps/plugin-dialog";
import { Cloud, FolderOpen, Globe, KeyRound, Lock, Network, Server, Shield, X } from "lucide-react";
import { api } from "../api/tauri";
import {
  PROTOCOL_META,
  type AgentStatus,
  type AuthType,
  type Protocol,
  type SiteConfig,
  type SiteInput,
} from "../types";

interface SiteModalProps {
  initial?: SiteConfig | null;
  defaultProtocol?: Protocol;
  onClose: () => void;
  onSaveAndConnect: (input: SiteInput, connectAfter: boolean) => Promise<void>;
}

const S3_PRESETS = [
  { label: "自定义 / MinIO", endpoint: "http://127.0.0.1:9000", region: "us-east-1", pathStyle: true },
  { label: "AWS S3", endpoint: "https://s3.amazonaws.com", region: "us-east-1", pathStyle: false },
  { label: "阿里云 OSS", endpoint: "https://oss-cn-hangzhou.aliyuncs.com", region: "oss-cn-hangzhou", pathStyle: false },
  { label: "腾讯云 COS", endpoint: "https://cos.ap-shanghai.myqcloud.com", region: "ap-shanghai", pathStyle: false },
  { label: "Cloudflare R2", endpoint: "https://<account-id>.r2.cloudflarestorage.com", region: "auto", pathStyle: true },
];

export const SiteModal: React.FC<SiteModalProps> = ({
  initial,
  defaultProtocol = "sftp",
  onClose,
  onSaveAndConnect,
}) => {
  const initProto = initial?.protocol ?? defaultProtocol;
  const [name, setName] = useState(initial?.name ?? "");
  const [group, setGroup] = useState(initial?.group ?? "常用站点");
  const [protocol, setProtocol] = useState<Protocol>(initProto);
  const [host, setHost] = useState(initial?.host ?? "");
  const [port, setPort] = useState<string>(
    String(initial?.port ?? PROTOCOL_META[initProto].defaultPort)
  );
  const [username, setUsername] = useState(initial?.username ?? "");
  const [authType, setAuthType] = useState<AuthType>(
    initial?.authType ?? (initProto === "s3" ? "access_key" : "password")
  );
  const [secret, setSecret] = useState("");
  const [passphrase, setPassphrase] = useState("");
  const [sessionToken, setSessionToken] = useState("");
  const [privateKeyPath, setPrivateKeyPath] = useState(initial?.privateKeyPath ?? "");
  const [remotePath, setRemotePath] = useState(initial?.remotePath ?? "/");
  const [endpoint, setEndpoint] = useState(initial?.endpoint ?? "");
  const [bucket, setBucket] = useState(initial?.bucket ?? "");
  const [region, setRegion] = useState(initial?.region ?? "us-east-1");
  const [pathStyle, setPathStyle] = useState(initial?.pathStyle ?? true);
  const [tlsSkipVerify, setTlsSkipVerify] = useState(initial?.tlsSkipVerify ?? false);

  // ProxyJump (Bastion)
  const [useJump, setUseJump] = useState(Boolean(initial?.jumpHost));
  const [jumpHost, setJumpHost] = useState(initial?.jumpHost ?? "");
  const [jumpPort, setJumpPort] = useState(String(initial?.jumpPort ?? 22));
  const [jumpUsername, setJumpUsername] = useState(initial?.jumpUsername ?? "");
  const [jumpAuthType, setJumpAuthType] = useState<AuthType>(initial?.jumpAuthType ?? "agent");
  const [jumpPrivateKeyPath, setJumpPrivateKeyPath] = useState(initial?.jumpPrivateKeyPath ?? "");
  const [jumpSecret, setJumpSecret] = useState("");
  const [jumpPassphrase, setJumpPassphrase] = useState("");

  // Local ssh-agent status
  const [agentStatus, setAgentStatus] = useState<AgentStatus | null>(null);
  const [saveSecret, setSaveSecret] = useState(true);
  const [submitting, setSubmitting] = useState(false);

  const needsAgent =
    protocol === "sftp" && (authType === "agent" || (useJump && jumpAuthType === "agent"));

  useEffect(() => {
    if (!needsAgent) return;
    let active = true;
    api
      .checkSshAgentStatus()
      .then((res) => {
        if (active) setAgentStatus(res);
      })
      .catch(() => {});
    return () => {
      active = false;
    };
  }, [needsAgent]);

  const handleProtocolChange = (next: Protocol) => {
    setProtocol(next);
    setPort(String(PROTOCOL_META[next].defaultPort));
    if (next === "s3") {
      setAuthType("access_key");
      if (!endpoint) setEndpoint("https://s3.amazonaws.com");
    } else {
      setAuthType("password");
    }
    if (next !== "sftp") {
      setUseJump(false);
    }
  };

  const pickKeyFile = async (target: "main" | "jump") => {
    const selected = await open({
      multiple: false,
      title: "选择 SSH 私钥文件 (id_ed25519 / id_rsa / .pem)",
    });
    if (typeof selected === "string") {
      if (target === "main") setPrivateKeyPath(selected);
      else setJumpPrivateKeyPath(selected);
    }
  };

  const buildInput = (): SiteInput => ({
    id: initial?.id ?? null,
    name: name.trim() || (protocol === "s3" ? bucket || endpoint : host),
    group: group.trim() || "默认分组",
    protocol,
    host: protocol === "s3" ? host.trim() || endpoint.trim() : host.trim(),
    port: Number(port) || PROTOCOL_META[protocol].defaultPort,
    username: username.trim(),
    authType,
    secret: secret || null,
    passphrase: passphrase || null,
    sessionToken: sessionToken || null,
    privateKeyPath: privateKeyPath.trim() || null,
    remotePath: remotePath.trim() || "/",
    endpoint: endpoint.trim() || null,
    bucket: bucket.trim() || null,
    region: region.trim() || null,
    pathStyle,
    tlsSkipVerify,
    jumpHost: protocol === "sftp" && useJump ? jumpHost.trim() || null : null,
    jumpPort: protocol === "sftp" && useJump ? Number(jumpPort) || 22 : null,
    jumpUsername: protocol === "sftp" && useJump ? jumpUsername.trim() || null : null,
    jumpAuthType: protocol === "sftp" && useJump ? jumpAuthType : null,
    jumpPrivateKeyPath:
      protocol === "sftp" && useJump ? jumpPrivateKeyPath.trim() || null : null,
    jumpSecret: protocol === "sftp" && useJump ? jumpSecret || null : null,
    jumpPassphrase: protocol === "sftp" && useJump ? jumpPassphrase || null : null,
    notes: "",
    saveSecret,
  });

  const submit = async (connectAfter: boolean) => {
    setSubmitting(true);
    try {
      await onSaveAndConnect(buildInput(), connectAfter);
      onClose();
    } finally {
      setSubmitting(false);
    }
  };

  // 按 Esc 快捷键关闭弹窗
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if (e.key === "Escape" && !submitting) onClose();
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [onClose, submitting]);

  const PROTO_TABS: { id: Protocol; label: string; icon: React.ReactNode }[] = [
    { id: "sftp", label: "SFTP", icon: <Lock size={14} /> },
    { id: "ftps_explicit", label: "FTPS", icon: <Shield size={14} /> },
    { id: "ftp", label: "FTP", icon: <Server size={14} /> },
    { id: "s3", label: "S3 对象", icon: <Cloud size={14} /> },
    { id: "webdav", label: "WebDAV", icon: <Globe size={14} /> },
  ];

  return (
    <div className="modal-backdrop" onClick={onClose}>
      <div className="modal" onClick={(e) => e.stopPropagation()}>
        <div className="modal-header">
          <span>{initial ? "编辑站点" : "新建连接站点"}</span>
          <button className="icon-btn" onClick={onClose}>
            <X size={16} />
          </button>
        </div>

        <div className="modal-body">
          <div className="form-grid">
            <div className="form-field full">
              <label>协议类型</label>
              <div className="proto-segment-grid">
                {PROTO_TABS.map((item) => {
                  const active =
                    protocol === item.id ||
                    (item.id === "ftps_explicit" && protocol === "ftps_implicit");
                  return (
                    <button
                      key={item.id}
                      type="button"
                      className={`proto-segment-btn ${active ? "active" : ""}`}
                      onClick={() => handleProtocolChange(item.id)}
                    >
                      {item.icon}
                      <span>{item.label}</span>
                    </button>
                  );
                })}
              </div>
              {(protocol === "ftps_explicit" || protocol === "ftps_implicit") && (
                <div style={{ display: "flex", gap: 14, marginTop: 4 }}>
                  <label className="checkbox-row">
                    <input
                      type="radio"
                      name="ftpsMode"
                      checked={protocol === "ftps_explicit"}
                      onChange={() => handleProtocolChange("ftps_explicit")}
                    />
                    显式 TLS (端口 21，推荐)
                  </label>
                  <label className="checkbox-row">
                    <input
                      type="radio"
                      name="ftpsMode"
                      checked={protocol === "ftps_implicit"}
                      onChange={() => handleProtocolChange("ftps_implicit")}
                    />
                    隐式 TLS (端口 990)
                  </label>
                </div>
              )}
            </div>

            <div className="form-field full">
              <label>站点名称</label>
              <input
                className="form-input"
                value={name}
                onChange={(e) => setName(e.target.value)}
                placeholder="例如：生产服务器 / 阿里云 OSS"
              />
            </div>

            {protocol === "s3" ? (
              <>
                <div className="form-field full">
                  <label>常用云存储预设（一键填充）</label>
                  <div style={{ display: "flex", flexWrap: "wrap", gap: 6 }}>
                    {S3_PRESETS.map((preset) => (
                      <button
                        type="button"
                        key={preset.label}
                        className="btn btn-sm btn-outline"
                        onClick={() => {
                          setEndpoint(preset.endpoint);
                          setRegion(preset.region);
                          setPathStyle(preset.pathStyle);
                        }}
                      >
                        {preset.label}
                      </button>
                    ))}
                  </div>
                </div>

                <div className="form-field full">
                  <label>Endpoint 地址</label>
                  <input
                    className="form-input"
                    value={endpoint}
                    onChange={(e) => setEndpoint(e.target.value)}
                    placeholder="https://s3.amazonaws.com 或 https://oss-cn-hangzhou.aliyuncs.com"
                  />
                </div>

                <div className="form-field">
                  <label>Bucket 名称</label>
                  <input
                    className="form-input"
                    value={bucket}
                    onChange={(e) => setBucket(e.target.value)}
                    placeholder="my-bucket"
                  />
                </div>

                <div className="form-field">
                  <label>Region 区域</label>
                  <input
                    className="form-input"
                    value={region}
                    onChange={(e) => setRegion(e.target.value)}
                    placeholder="us-east-1"
                  />
                </div>

                <div className="form-field">
                  <label>Access Key ID</label>
                  <input
                    className="form-input"
                    value={username}
                    onChange={(e) => setUsername(e.target.value)}
                    placeholder="AKIA..."
                  />
                </div>

                <div className="form-field">
                  <label>
                    Secret Access Key {initial?.hasSavedSecret ? "（留空保持已存密钥）" : ""}
                  </label>
                  <input
                    type="password"
                    className="form-input"
                    value={secret}
                    onChange={(e) => setSecret(e.target.value)}
                    placeholder="••••••••"
                  />
                </div>

                <div className="form-field full">
                  <label>STS Session Token（可选，临时凭证）</label>
                  <input
                    type="password"
                    className="form-input"
                    value={sessionToken}
                    onChange={(e) => setSessionToken(e.target.value)}
                    placeholder="可选"
                  />
                </div>

                <div className="form-field full">
                  <label className="checkbox-row">
                    <input
                      type="checkbox"
                      checked={pathStyle}
                      onChange={(e) => setPathStyle(e.target.checked)}
                    />
                    使用 Path-Style 访问（MinIO / 自建对象存储建议开启）
                  </label>
                </div>
              </>
            ) : protocol === "webdav" ? (
              <>
                <div className="form-field full">
                  <label>WebDAV 服务 URL</label>
                  <input
                    className="form-input"
                    value={endpoint}
                    onChange={(e) => {
                      setEndpoint(e.target.value);
                      try {
                        const u = new URL(e.target.value);
                        setHost(u.hostname);
                        setPort(u.port || (u.protocol === "http:" ? "80" : "443"));
                      } catch {
                        // typing in progress
                      }
                    }}
                    placeholder="https://nas.example.com/dav 或 https://cloud.example.com/remote.php/dav/files/user"
                  />
                </div>

                <div className="form-field">
                  <label>认证方式</label>
                  <select
                    className="form-input"
                    value={authType}
                    onChange={(e) => setAuthType(e.target.value as AuthType)}
                  >
                    <option value="password">用户名 + 密码 (Basic)</option>
                    <option value="token">Bearer Token / OAuth2 令牌</option>
                    <option value="anonymous">匿名访问</option>
                  </select>
                </div>

                {authType === "password" && (
                  <div className="form-field">
                    <label>用户名</label>
                    <input
                      className="form-input"
                      value={username}
                      onChange={(e) => setUsername(e.target.value)}
                    />
                  </div>
                )}

                {authType !== "anonymous" && (
                  <div className="form-field full">
                    <label>
                      {authType === "token" ? "Access Token" : "密码 / 应用专用密码"}
                      {initial?.hasSavedSecret ? "（留空保持已存凭据）" : ""}
                    </label>
                    <input
                      type="password"
                      className="form-input"
                      value={secret}
                      onChange={(e) => setSecret(e.target.value)}
                    />
                  </div>
                )}
              </>
            ) : (
              <>
                <div className="form-field">
                  <label>主机地址</label>
                  <input
                    className="form-input"
                    value={host}
                    onChange={(e) => setHost(e.target.value)}
                    placeholder="192.168.1.10 或 example.com"
                  />
                </div>

                <div className="form-field">
                  <label>端口</label>
                  <input
                    type="number"
                    className="form-input"
                    value={port}
                    onChange={(e) => setPort(e.target.value)}
                  />
                </div>

                <div className="form-field">
                  <label>用户名</label>
                  <input
                    className="form-input"
                    value={username}
                    onChange={(e) => setUsername(e.target.value)}
                    placeholder={protocol === "sftp" ? "root / ubuntu" : "anonymous"}
                  />
                </div>

                {protocol === "sftp" ? (
                  <div className="form-field">
                    <label>认证方式</label>
                    <select
                      className="form-input"
                      value={authType}
                      onChange={(e) => setAuthType(e.target.value as AuthType)}
                    >
                      <option value="password">密码认证</option>
                      <option value="private_key">SSH 私钥文件 (Ed25519 / RSA / ECDSA)</option>
                      <option value="agent">本地 ssh-agent / Pageant</option>
                    </select>
                  </div>
                ) : (
                  <div className="form-field">
                    <label>
                      密码 {initial?.hasSavedSecret ? "（留空保持已存密码）" : ""}
                    </label>
                    <input
                      type="password"
                      className="form-input"
                      value={secret}
                      onChange={(e) => setSecret(e.target.value)}
                    />
                  </div>
                )}

                {protocol === "sftp" && authType === "password" && (
                  <div className="form-field full">
                    <label>
                      密码 {initial?.hasSavedSecret ? "（留空保持已存密码）" : ""}
                    </label>
                    <input
                      type="password"
                      className="form-input"
                      value={secret}
                      onChange={(e) => setSecret(e.target.value)}
                    />
                  </div>
                )}

                {protocol === "sftp" && authType === "private_key" && (
                  <>
                    <div className="form-field full">
                      <label>私钥文件路径</label>
                      <div style={{ display: "flex", gap: 6 }}>
                        <input
                          className="form-input"
                          style={{ flex: 1 }}
                          value={privateKeyPath}
                          onChange={(e) => setPrivateKeyPath(e.target.value)}
                          placeholder="C:\Users\you\.ssh\id_ed25519"
                        />
                        <button
                          type="button"
                          className="btn btn-outline"
                          onClick={() => void pickKeyFile("main")}
                        >
                          <FolderOpen size={14} />
                          浏览
                        </button>
                      </div>
                    </div>
                    <div className="form-field full">
                      <label>私钥口令 (Passphrase，无口令可留空)</label>
                      <input
                        type="password"
                        className="form-input"
                        value={passphrase}
                        onChange={(e) => setPassphrase(e.target.value)}
                      />
                    </div>
                  </>
                )}

                {needsAgent && agentStatus && (
                  <div className="form-field full">
                    <div className={`agent-pill ${agentStatus.available ? "ok" : "warn"}`}>
                      <KeyRound size={14} style={{ flexShrink: 0 }} />
                      <span>{agentStatus.message}</span>
                    </div>
                  </div>
                )}

                {protocol === "sftp" && (
                  <div className="form-field full">
                    <label className="checkbox-row" style={{ fontWeight: 500, color: "var(--text)" }}>
                      <input
                        type="checkbox"
                        checked={useJump}
                        onChange={(e) => setUseJump(e.target.checked)}
                      />
                      <Network size={14} color="var(--accent)" />
                      通过 SSH 跳板机 (ProxyJump / Bastion) 连接
                    </label>

                    {useJump && (
                      <div className="jump-section">
                        <div className="form-grid">
                          <div className="form-field">
                            <label>跳板机主机</label>
                            <input
                              className="form-input"
                              value={jumpHost}
                              onChange={(e) => setJumpHost(e.target.value)}
                              placeholder="bastion.example.com"
                            />
                          </div>
                          <div className="form-field">
                            <label>跳板机端口</label>
                            <input
                              type="number"
                              className="form-input"
                              value={jumpPort}
                              onChange={(e) => setJumpPort(e.target.value)}
                            />
                          </div>
                          <div className="form-field">
                            <label>跳板机用户名（留空同目标）</label>
                            <input
                              className="form-input"
                              value={jumpUsername}
                              onChange={(e) => setJumpUsername(e.target.value)}
                              placeholder={username || "ops"}
                            />
                          </div>
                          <div className="form-field">
                            <label>跳板机认证方式</label>
                            <select
                              className="form-input"
                              value={jumpAuthType}
                              onChange={(e) => setJumpAuthType(e.target.value as AuthType)}
                            >
                              <option value="agent">本地 ssh-agent / Pageant</option>
                              <option value="private_key">SSH 私钥文件</option>
                              <option value="password">密码认证</option>
                            </select>
                          </div>

                          {jumpAuthType === "password" && (
                            <div className="form-field full">
                              <label>
                                跳板机密码 {initial?.hasSavedSecret ? "（留空保持已存）" : ""}
                              </label>
                              <input
                                type="password"
                                className="form-input"
                                value={jumpSecret}
                                onChange={(e) => setJumpSecret(e.target.value)}
                              />
                            </div>
                          )}

                          {jumpAuthType === "private_key" && (
                            <>
                              <div className="form-field full">
                                <label>跳板机私钥文件</label>
                                <div style={{ display: "flex", gap: 6 }}>
                                  <input
                                    className="form-input"
                                    style={{ flex: 1 }}
                                    value={jumpPrivateKeyPath}
                                    onChange={(e) => setJumpPrivateKeyPath(e.target.value)}
                                    placeholder="C:\Users\you\.ssh\id_ed25519"
                                  />
                                  <button
                                    type="button"
                                    className="btn btn-outline"
                                    onClick={() => void pickKeyFile("jump")}
                                  >
                                    <FolderOpen size={14} />
                                    浏览
                                  </button>
                                </div>
                              </div>
                              <div className="form-field full">
                                <label>跳板机私钥口令（可选）</label>
                                <input
                                  type="password"
                                  className="form-input"
                                  value={jumpPassphrase}
                                  onChange={(e) => setJumpPassphrase(e.target.value)}
                                />
                              </div>
                            </>
                          )}
                        </div>
                      </div>
                    )}
                  </div>
                )}
              </>
            )}

            <div className="form-field">
              <label>默认远程目录</label>
              <input
                className="form-input"
                value={remotePath}
                onChange={(e) => setRemotePath(e.target.value)}
                placeholder="/"
              />
            </div>

            <div className="form-field">
              <label>分组</label>
              <input
                className="form-input"
                value={group}
                onChange={(e) => setGroup(e.target.value)}
                placeholder="常用站点"
              />
            </div>

            {(protocol === "ftps_explicit" ||
              protocol === "ftps_implicit" ||
              protocol === "s3" ||
              protocol === "webdav") && (
              <div className="form-field full">
                <label className="checkbox-row">
                  <input
                    type="checkbox"
                    checked={tlsSkipVerify}
                    onChange={(e) => setTlsSkipVerify(e.target.checked)}
                  />
                  忽略自签名 TLS 证书校验（仅限内网测试环境）
                </label>
              </div>
            )}

            <div className="form-field full">
              <label className="checkbox-row">
                <input
                  type="checkbox"
                  checked={saveSecret}
                  onChange={(e) => setSaveSecret(e.target.checked)}
                />
                将密钥安全托管到操作系统凭据库（Windows 凭据管理器 / macOS Keychain）
              </label>
            </div>
          </div>
        </div>

        <div className="modal-footer">
          <button className="btn btn-ghost" onClick={onClose} disabled={submitting}>
            取消
          </button>
          <button
            className="btn btn-outline"
            onClick={() => submit(false)}
            disabled={submitting}
          >
            仅保存
          </button>
          <button
            className="btn btn-primary"
            onClick={() => submit(true)}
            disabled={submitting}
          >
            {submitting ? "处理中..." : "保存并连接"}
          </button>
        </div>
      </div>
    </div>
  );
};
