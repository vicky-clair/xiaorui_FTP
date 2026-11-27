/**
 * 前端 React 入口文件
 *
 * 根据 URL 参数自动区分主文件工作台窗口与独立弹出的原生 SSH 终端窗口（`?mode=terminal`）。
 */

import React from "react";
import ReactDOM from "react-dom/client";
import App from "./App";
import { ErrorBoundary } from "./components/ErrorBoundary";
import { StandaloneTerminalApp } from "./components/StandaloneTerminalApp";
import "./styles/app.css";

const isTerminalWindow =
  new URLSearchParams(window.location.search).get("mode") === "terminal";

ReactDOM.createRoot(document.getElementById("root") as HTMLElement).render(
  <React.StrictMode>
    <ErrorBoundary>
      {isTerminalWindow ? <StandaloneTerminalApp /> : <App />}
    </ErrorBoundary>
  </React.StrictMode>
);
