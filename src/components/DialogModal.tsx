/**
 * 跨平台统一输入与确认弹窗组件
 *
 * 替代浏览器原生的 `window.prompt` 与 `window.confirm`（Linux WebKitGTK 默认不实现这两个弹窗），
 * 保证 Windows / macOS / Linux 三平台下新建目录、重命名、删除确认、CHMOD 权限修改等交互完全一致。
 */

import React, { useEffect, useRef, useState } from "react";
import { X } from "lucide-react";

export type DialogRequest =
  | {
      kind: "confirm";
      title: string;
      message: string;
      confirmLabel?: string;
      danger?: boolean;
      resolve: (confirmed: boolean) => void;
    }
  | {
      kind: "prompt";
      title: string;
      message?: string;
      defaultValue?: string;
      placeholder?: string;
      confirmLabel?: string;
      resolve: (value: string | null) => void;
    };

interface DialogModalProps {
  dialog: DialogRequest;
  onClose: () => void;
}

export const DialogModal: React.FC<DialogModalProps> = ({ dialog, onClose }) => {
  const [value, setValue] = useState(
    dialog.kind === "prompt" ? (dialog.defaultValue ?? "") : ""
  );
  const inputRef = useRef<HTMLInputElement>(null);

  useEffect(() => {
    if (dialog.kind === "prompt") {
      inputRef.current?.focus();
      inputRef.current?.select();
    }
  }, [dialog]);

  const handleCancel = () => {
    if (dialog.kind === "confirm") {
      dialog.resolve(false);
    } else {
      dialog.resolve(null);
    }
    onClose();
  };

  const handleSubmit = (e: React.FormEvent) => {
    e.preventDefault();
    if (dialog.kind === "confirm") {
      dialog.resolve(true);
    } else {
      dialog.resolve(value);
    }
    onClose();
  };

  useEffect(() => {
    const onKeyDown = (e: KeyboardEvent) => {
      if (e.key === "Escape") {
        e.preventDefault();
        handleCancel();
      }
    };
    window.addEventListener("keydown", onKeyDown);
    return () => window.removeEventListener("keydown", onKeyDown);
  });

  return (
    <div className="modal-backdrop" onClick={handleCancel}>
      <form
        className="modal"
        style={{ maxWidth: 420 }}
        onClick={(e) => e.stopPropagation()}
        onSubmit={handleSubmit}
      >
        <div className="modal-header">
          <span>{dialog.title}</span>
          <button type="button" className="icon-btn" onClick={handleCancel}>
            <X size={16} />
          </button>
        </div>

        <div className="modal-body">
          {dialog.message && (
            <p style={{ margin: 0, color: "var(--text-secondary)", whiteSpace: "pre-line" }}>
              {dialog.message}
            </p>
          )}
          {dialog.kind === "prompt" && (
            <input
              ref={inputRef}
              className="form-input"
              value={value}
              onChange={(e) => setValue(e.target.value)}
              placeholder={dialog.placeholder}
            />
          )}
        </div>

        <div className="modal-footer">
          <button type="button" className="btn btn-ghost" onClick={handleCancel}>
            取消
          </button>
          <button
            type="submit"
            className={`btn ${dialog.kind === "confirm" && dialog.danger ? "btn-danger-solid" : "btn-primary"}`}
            autoFocus={dialog.kind === "confirm"}
          >
            {dialog.confirmLabel ?? "确定"}
          </button>
        </div>
      </form>
    </div>
  );
};
