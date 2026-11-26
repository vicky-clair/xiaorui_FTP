/**
 * SFTP 内置轻量 SSH 命令终端抽屉组件
 *
 * 自动关联当前远程浏览目录，支持上下方向键翻阅历史命令、输入 `clear` 清屏以及区分展示 stdout/stderr。
 */

import React, { useRef, useState } from "react";
import { Terminal as TermIcon, Trash2, X } from "lucide-react";
import { api } from "../api/tauri";

interface TerminalDrawerProps {
  sessionId: string;
  sessionName: string;
  cwd: string;
  onClose: () => void;
}

interface LogLine {
  id: number;
  cmd: string;
  cwd: string;
  stdout: string;
  stderr: string;
  exitCode: number;
}

export const TerminalDrawer: React.FC<TerminalDrawerProps> = ({
  sessionId,
  sessionName,
  cwd,
  onClose,
}) => {
  const [command, setCommand] = useState("");
  const [running, setRunning] = useState(false);
  const [logs, setLogs] = useState<LogLine[]>([]);
  const [history, setHistory] = useState<string[]>([]);
  const [histIdx, setHistIdx] = useState(-1);
  const outRef = useRef<HTMLDivElement>(null);

  /** 向当前 SSH 会话发送命令并记录输出 */
  const runCommand = async (e: React.FormEvent) => {
    e.preventDefault();
    const cmd = command.trim();
    if (!cmd || running) return;

    if (cmd === "clear") {
      setLogs([]);
      setCommand("");
      return;
    }

    setRunning(true);
    setCommand("");
    setHistory((prev) => [cmd, ...prev.slice(0, 49)]);
    setHistIdx(-1);

    try {
      const res = await api.sshExecCommand(sessionId, cmd, cwd);
      setLogs((prev) => [
        ...prev,
        {
          id: Date.now(),
          cmd,
          cwd,
          stdout: res.stdout,
          stderr: res.stderr,
          exitCode: res.exitCode,
        },
      ]);
    } catch (err) {
      setLogs((prev) => [
        ...prev,
        {
          id: Date.now(),
          cmd,
          cwd,
          stdout: "",
          stderr: String(err),
          exitCode: -1,
        },
      ]);
    } finally {
      setRunning(false);
      requestAnimationFrame(() => {
        if (outRef.current) {
          outRef.current.scrollTop = outRef.current.scrollHeight;
        }
      });
    }
  };

  /** 上下方向键切换历史输入命令 */
  const handleKeyDown = (e: React.KeyboardEvent<HTMLInputElement>) => {
    if (e.key === "ArrowUp" && history.length > 0) {
      e.preventDefault();
      const next = Math.min(histIdx + 1, history.length - 1);
      setHistIdx(next);
      setCommand(history[next]);
    } else if (e.key === "ArrowDown") {
      e.preventDefault();
      const next = histIdx - 1;
      setHistIdx(next);
      setCommand(next >= 0 ? history[next] : "");
    }
  };

  return (
    <div className="bottom-drawer" style={{ height: 240 }}>
      <div className="drawer-header">
        <TermIcon size={14} />
        <span>
          SSH 终端 — {sessionName} ({cwd})
        </span>
        <div style={{ marginLeft: "auto", display: "flex", gap: 6 }}>
          <button className="btn btn-sm btn-ghost" onClick={() => setLogs([])}>
            <Trash2 size={12} />
            清屏
          </button>
          <button className="icon-btn" style={{ width: 24, height: 24 }} onClick={onClose}>
            <X size={14} />
          </button>
        </div>
      </div>

      <div ref={outRef} className="terminal-output">
        {logs.length === 0 ? (
          <span style={{ color: "#8b949e" }}>
            已关联当前远程目录 {cwd}。输入任意 Shell 命令（如 ls -la, df -h, docker ps）按回车执行...
          </span>
        ) : (
          logs.map((item) => (
            <div key={item.id} style={{ marginBottom: 10 }}>
              <div style={{ color: "#58a6ff" }}>
                [{item.cwd}]$ {item.cmd}
              </div>
              {item.stdout && <div>{item.stdout}</div>}
              {item.stderr && <div style={{ color: "#f85149" }}>{item.stderr}</div>}
              {item.exitCode !== 0 && (
                <div style={{ color: "#d29922", fontSize: 11 }}>
                  exit code: {item.exitCode}
                </div>
              )}
            </div>
          ))
        )}
      </div>

      <form className="terminal-input-bar" onSubmit={runCommand}>
        <span style={{ color: "#3fb950" }}>$</span>
        <input
          value={command}
          onChange={(e) => setCommand(e.target.value)}
          onKeyDown={handleKeyDown}
          placeholder={running ? "命令执行中..." : "输入命令并回车..."}
          disabled={running}
          autoFocus
        />
      </form>
    </div>
  );
};
