import { Component, ReactNode } from "react";

interface State {
  error: Error | null;
}

/** Catches render crashes: without it a throw in any component unmounts
 *  the whole React tree ("everything disappears"). Now the app shows the
 *  error and a reload button, and we get the stack in one place. */
export class ErrorBoundary extends Component<{ children: ReactNode }, State> {
  state: State = { error: null };

  static getDerivedStateFromError(error: Error): State {
    return { error };
  }

  componentDidCatch(error: Error, info: unknown) {
    console.error("[orbit] render crash:", error, info);
    // Land the full stack (with app frames) in /tmp/orbit-render-crash.log
    // — the on-screen card shows only the first frames.
    const stack = [
      String(error.stack ?? error.message),
      "",
      "componentStack:",
      String((info as { componentStack?: string } | null)?.componentStack ?? ""),
    ].join("\n");
    import("@tauri-apps/api/core")
      .then(({ invoke }) => invoke("log_render_crash", { message: stack }))
      .catch(() => null);
  }

  render() {
    if (this.state.error) {
      return (
        <div className="error-boundary">
          <h2>Something broke</h2>
          <pre>{String(this.state.error.stack ?? this.state.error.message)}</pre>
          <button type="button" onClick={() => window.location.reload()}>
            Reload
          </button>
        </div>
      );
    }
    return this.props.children;
  }
}
