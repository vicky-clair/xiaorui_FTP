/**
 * 左侧可折叠站点收藏夹侧边栏
 *
 * 支持按关键字即时搜索站点、按分组展示、点击一键连接、编辑/删除站点，
 * 以及站点列表的本地 JSON 备份导出与导入。
 */

import React, { useMemo, useState } from "react";
import { Download, Pencil, Search, ShieldCheck, Trash2, Upload } from "lucide-react";
import { PROTOCOL_META, type SiteConfig } from "../types";

interface SiteSidebarProps {
  sites: SiteConfig[];
  connectingSiteId: string | null;
  onConnect: (site: SiteConfig) => void;
  onEdit: (site: SiteConfig) => void;
  onDelete: (site: SiteConfig) => void;
  onExport: () => void;
  onImport: () => void;
  onOpenKnownHosts: () => void;
}

export const SiteSidebar: React.FC<SiteSidebarProps> = ({
  sites,
  connectingSiteId,
  onConnect,
  onEdit,
  onDelete,
  onExport,
  onImport,
  onOpenKnownHosts,
}) => {
  const [query, setQuery] = useState("");

  /** 按搜索词过滤并按分组名称归类站点 */
  const grouped = useMemo(() => {
    const q = query.trim().toLowerCase();
    const filtered = q
      ? sites.filter(
          (s) =>
            s.name.toLowerCase().includes(q) ||
            s.host.toLowerCase().includes(q) ||
            s.group.toLowerCase().includes(q) ||
            s.protocol.toLowerCase().includes(q)
        )
      : sites;

    const map = new Map<string, SiteConfig[]>();
    for (const s of filtered) {
      const g = s.group || "默认分组";
      const list = map.get(g) ?? [];
      list.push(s);
      map.set(g, list);
    }
    return Array.from(map.entries());
  }, [sites, query]);

  return (
    <aside className="sidebar">
      <div className="sidebar-header">
        <div className="search-box">
          <Search size={14} color="var(--text-muted)" />
          <input
            value={query}
            onChange={(e) => setQuery(e.target.value)}
            placeholder="搜索站点..."
          />
        </div>
      </div>

      <div className="sidebar-list">
        {grouped.length === 0 ? (
          <div
            style={{
              padding: "28px 12px",
              textAlign: "center",
              color: "var(--text-muted)",
              fontSize: 12,
            }}
          >
            {sites.length === 0
              ? "暂无已保存站点，点击右上角「新建站点」添加。"
              : "没有匹配的站点"}
          </div>
        ) : (
          grouped.map(([group, list]) => (
            <div key={group}>
              <div className="site-group-label">{group}</div>
              {list.map((site) => {
                const meta = PROTOCOL_META[site.protocol];
                const isBusy = connectingSiteId === site.id;
                return (
                  <div
                    key={site.id}
                    className="site-item"
                    onClick={() => !isBusy && onConnect(site)}
                    title={`点击连接 ${site.host}:${site.port}${site.jumpHost ? ` (跳板机: ${site.jumpHost})` : ""}`}
                  >
                    <span className={`proto-badge ${meta.badgeClass}`}>
                      {meta.short}
                    </span>
                    <div className="site-item-info">
                      <div className="site-item-name">
                        {isBusy ? `连接中... ${site.name}` : site.name}
                      </div>
                      <div className="site-item-sub">
                        {site.protocol === "s3"
                          ? `${site.bucket || ""} · ${site.endpoint || site.host}`
                          : `${site.username ? site.username + "@" : ""}${site.host}:${site.port}`}
                      </div>
                    </div>
                    <div
                      className="site-item-actions"
                      onClick={(e) => e.stopPropagation()}
                    >
                      <button
                        className="icon-btn"
                        style={{ width: 24, height: 24 }}
                        title="编辑站点"
                        onClick={() => onEdit(site)}
                      >
                        <Pencil size={13} />
                      </button>
                      <button
                        className="icon-btn btn-danger"
                        style={{ width: 24, height: 24 }}
                        title="删除站点"
                        onClick={() => onDelete(site)}
                      >
                        <Trash2 size={13} />
                      </button>
                    </div>
                  </div>
                );
              })}
            </div>
          ))
        )}
      </div>

      <div className="sidebar-footer">
        <button className="btn btn-sm btn-ghost" onClick={onImport} title="从 JSON 文件导入站点配置">
          <Upload size={13} />
          导入
        </button>
        <button className="btn btn-sm btn-ghost" onClick={onExport} title="导出站点备份为 JSON（不含敏感密钥）">
          <Download size={13} />
          备份
        </button>
        <button className="btn btn-sm btn-ghost" onClick={onOpenKnownHosts} title="管理 SSH 已知主机公钥指纹">
          <ShieldCheck size={13} />
          主机
        </button>
      </div>
    </aside>
  );
};
