/**
 * 远程文本/配置文件在线轻编辑器弹窗组件
 *
 * 支持查看与修改 2 MB 以内的远程文本文件，支持 Ctrl+S / Cmd+S 快捷键直接保存并回传服务器，
 * 以及 Esc 快捷关闭。
 */

import React, { useCallback, useEffect, useState } from "react";
import { Save, X } from "lucide-react";

interface EditorModalProps {
  filePath: string;
  initialContent: string;
  onClose: () => void;
  onSave: (content: string) => Promise<void>;
}

export const EditorModal: React.FC<EditorModalProps> = ({
  filePath,
  initialContent,
  onClose,
  onSave,
}) => {
  const [content, setContent] = useState(initialContent);
  const [saving, setSaving] = useState(false);
  const dirty = content !== initialContent;

  const handleSave = useCallback(async () => {
    if (saving) return;
    setSaving(true);
    try {
      await onSave(content);
      onClose();
    } catch {
      // 错误提示由上层 App 展示，保留编辑框内容避免用户修改丢失
    } finally {
      setSaving(false);
    }
  }, [content, onClose, onSave, saving]);

  // 监听 Ctrl+S / Cmd+S 保存与 Esc 关闭
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if ((e.ctrlKey || e.metaKey) && e.key.toLowerCase() === "s") {
        e.preventDefault();
        void handleSave();
      } else if (e.key === "Escape" && !saving) {
        e.preventDefault();
        onClose();
      }
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [handleSave, onClose, saving]);

  return (
    <div className="modal-backdrop" onClick={() => !saving && onClose()}>
      <div className="modal modal-wide" onClick={(e) => e.stopPropagation()}>
        <div className="modal-header">
          <span style={{ fontFamily: "var(--font-mono)", fontSize: 13 }}>
            {filePath} {dirty ? "• 未保存" : ""}
          </span>
          <button type="button" className="icon-btn" onClick={onClose} disabled={saving}>
            <X size={16} />
          </button>
        </div>

        <div className="modal-body" style={{ flex: 1, minHeight: 0 }}>
          <textarea
            className="editor-textarea"
            value={content}
            onChange={(e) => setContent(e.target.value)}
            spellCheck={false}
          />
        </div>

        <div className="modal-footer">
          <span style={{ marginRight: "auto", fontSize: 12, color: "var(--text-muted)" }}>
            快捷键 Ctrl+S 直接保存并回传至远程服务器 · Esc 关闭
          </span>
          <button type="button" className="btn btn-ghost" onClick={onClose} disabled={saving}>
            关闭
          </button>
          <button
            type="button"
            className="btn btn-primary"
            onClick={() => void handleSave()}
            disabled={saving}
          >
            <Save size={14} />
            {saving ? "保存中..." : "保存到远程"}
          </button>
        </div>
      </div>
    </div>
  );
};
