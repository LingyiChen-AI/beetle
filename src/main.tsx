import React from "react";
import ReactDOM from "react-dom/client";
import App from "./App";
// 字体随前端打包（CSP 只允许本地字体，不能用 Google Fonts）：Inter / JetBrains Mono 可变字体
import "@fontsource-variable/inter";
import "@fontsource-variable/jetbrains-mono";
import "./index.css";

ReactDOM.createRoot(document.getElementById("root") as HTMLElement).render(
  <React.StrictMode>
    <App />
  </React.StrictMode>,
);
