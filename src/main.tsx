import React from "react";
import ReactDOM from "react-dom/client";
import { configureBuiltinTypeScript } from "./lib/monacoSetup";
import App from "./App";
import { ErrorBoundary } from "./components/ErrorBoundary";

// Suppress the native webview context menu everywhere; we provide our own.
document.addEventListener("contextmenu", (e) => e.preventDefault());

// Before any editor opens (restored tabs included): Monaco's TypeScript
// features are fixed when the first TS file loads.
void configureBuiltinTypeScript().finally(() =>
  ReactDOM.createRoot(document.getElementById("root") as HTMLElement).render(
    <React.StrictMode>
      <ErrorBoundary>
        <App />
      </ErrorBoundary>
    </React.StrictMode>,
  ),
);
