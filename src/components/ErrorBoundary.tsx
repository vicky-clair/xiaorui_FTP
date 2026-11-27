/**
 * 全局 React 错误边界组件
 *
 * 捕获前端渲染期间未预期的运行时异常，展示友好的恢复页面与一键刷新按钮，
 * 避免整个桌面窗口白屏卡死，且不影响用户本地保存的站点与凭据数据。
 */

import React from "react";

interface ErrorBoundaryState {
  hasError: boolean;
  message: string;
}

export class ErrorBoundary extends React.Component<
  { children: React.ReactNode },
  ErrorBoundaryState
> {
  constructor(props: { children: React.ReactNode }) {
    super(props);
    this.state = { hasError: false, message: "" };
  }

  static getDerivedStateFromError(error: unknown): ErrorBoundaryState {
    return {
      hasError: true,
      message: error instanceof Error ? error.message : String(error),
    };
  }

  componentDidCatch(_error: unknown) {
    // 已通过 state 渲染友好降级界面
  }

  render() {
    if (this.state.hasError) {
      return (
        <div
          style={{
            height: "100vh",
            display: "flex",
            flexDirection: "column",
            alignItems: "center",
            justifyContent: "center",
            padding: 28,
            gap: 12,
            textAlign: "center",
            background: "var(--bg)",
            color: "var(--text)",
          }}
        >
          <div style={{ fontSize: 18, fontWeight: 650 }}>界面遇到了意外异常</div>
          <div
            style={{
              maxWidth: 480,
              fontSize: 12.5,
              color: "var(--text-secondary)",
              fontFamily: "var(--font-mono)",
              padding: "10px 14px",
              borderRadius: 8,
              border: "1px solid var(--border)",
              background: "var(--surface)",
              wordBreak: "break-word",
            }}
          >
            {this.state.message || "未知渲染错误"}
          </div>
          <div style={{ fontSize: 12, color: "var(--text-muted)" }}>
            你的站点配置、系统凭据与已知主机指纹均未受影响，可安全重新加载界面。
          </div>
          <button
            type="button"
            className="btn btn-primary"
            onClick={() => window.location.reload()}
          >
            重新加载界面
          </button>
        </div>
      );
    }
    return this.props.children;
  }
}
