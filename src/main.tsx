import React from "react";
import ReactDOM from "react-dom/client";
import App from "./App";

// Suppress the native webview context menu everywhere; we provide our own.
document.addEventListener("contextmenu", (e) => e.preventDefault());

ReactDOM.createRoot(document.getElementById("root") as HTMLElement).render(
  <React.StrictMode>
    <App />
  </React.StrictMode>,
);
