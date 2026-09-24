import React from "react";
import ReactDOM from "react-dom/client";
import "./lib/monacoSetup";
import App from "./App";
import { ErrorBoundary } from "./components/ErrorBoundary";

// Suppress the native webview context menu everywhere; we provide our own.
document.addEventListener("contextmenu", (e) => e.preventDefault());

ReactDOM.createRoot(document.getElementById("root") as HTMLElement).render(
  <React.StrictMode>
    <ErrorBoundary>
      <App />
    </ErrorBoundary>
  </React.StrictMode>,
);
