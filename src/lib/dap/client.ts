// One debug adapter session: DAP requests, responses and events over the
// backend's stdio bridge (dap.rs: `dap_send` out, `dap-message` events in).
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";

/* eslint-disable @typescript-eslint/no-explicit-any */

export interface Adapter {
  kind: "gdb" | "lldb-dap" | string;
  command: string;
  args: string[];
}

type EventHandler = (body: any) => void;

const clients = new Map<number, DapClient>();
let listening: Promise<unknown> | null = null;

function ensureListening() {
  listening ??= Promise.all([
    listen<{ id: number; message: string }>("dap-message", (e) => clients.get(e.payload.id)?.receive(e.payload.message)),
    listen<{ id: number; code: number | null }>("dap-exit", (e) => clients.get(e.payload.id)?.exited()),
  ]);
  return listening;
}

export class DapClient {
  private seq = 1;
  private pending = new Map<number, { resolve: (v: any) => void; reject: (e: Error) => void }>();
  private handlers = new Map<string, Set<EventHandler>>();
  alive = true;

  private constructor(readonly id: number, readonly adapter: Adapter) {}

  static async start(requested: string | null, cwd: string): Promise<DapClient> {
    await ensureListening();
    const [id, adapter] = await invoke<[number, Adapter]>("dap_start", { requested, cwd });
    const client = new DapClient(id, adapter);
    clients.set(id, client);
    return client;
  }

  on(event: string, handler: EventHandler): () => void {
    if (!this.handlers.has(event)) this.handlers.set(event, new Set());
    this.handlers.get(event)!.add(handler);
    return () => this.handlers.get(event)?.delete(handler);
  }

  request<T = any>(command: string, args: unknown = {}): Promise<T> {
    if (!this.alive) return Promise.reject(new Error("the debugger is not running"));
    const seq = this.seq++;
    return new Promise<T>((resolve, reject) => {
      this.pending.set(seq, { resolve, reject });
      invoke("dap_send", { id: this.id, message: JSON.stringify({ seq, type: "request", command, arguments: args }) }).catch((e) => {
        this.pending.delete(seq);
        reject(new Error(String(e)));
      });
    });
  }

  receive(raw: string) {
    let msg: any;
    try {
      msg = JSON.parse(raw);
    } catch {
      return;
    }
    if (msg.type === "response") {
      const p = this.pending.get(msg.request_seq);
      if (!p) return;
      this.pending.delete(msg.request_seq);
      if (msg.success) p.resolve(msg.body ?? {});
      else p.reject(new Error(msg.body?.error?.format ?? msg.message ?? `${msg.command} failed`));
    } else if (msg.type === "event") {
      this.handlers.get(msg.event)?.forEach((h) => h(msg.body ?? {}));
    } else if (msg.type === "request") {
      // Reverse requests (runInTerminal, startDebugging): not supported.
      invoke("dap_send", {
        id: this.id,
        message: JSON.stringify({ seq: this.seq++, type: "response", request_seq: msg.seq, command: msg.command, success: false, message: "unsupported" }),
      }).catch(() => null);
    }
  }

  exited() {
    if (!this.alive) return;
    this.alive = false;
    clients.delete(this.id);
    for (const p of this.pending.values()) p.reject(new Error("the debugger exited"));
    this.pending.clear();
    this.handlers.get("$exit")?.forEach((h) => h({}));
  }

  kill() {
    invoke("dap_stop", { id: this.id }).catch(() => null);
    this.exited();
  }
}
