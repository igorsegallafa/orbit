// One language server session: JSON-RPC over the backend's stdio bridge
// (lsp.rs: `lsp_send` out, `lsp-message` events in).
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import * as monaco from "monaco-editor";

/* eslint-disable @typescript-eslint/no-explicit-any */

interface StartInfo {
  id: number;
  root: string;
  command: string;
}

interface Pending {
  resolve: (v: any) => void;
  reject: (e: Error) => void;
}

type Listener = () => void;

const clients = new Map<number, LspClient>();
let listening: Promise<unknown> | null = null;

function ensureListening() {
  listening ??= Promise.all([
    listen<{ id: number; message: string }>("lsp-message", (e) => clients.get(e.payload.id)?.receive(e.payload.message)),
    listen<{ id: number; code: number | null; log: string[] }>("lsp-exit", (e) =>
      clients.get(e.payload.id)?.exited(e.payload.code, e.payload.log),
    ),
  ]);
  return listening;
}

/** What Orbit's editor supports, told to every server at initialize. */
const CLIENT_CAPABILITIES = {
  general: { positionEncodings: ["utf-16"] },
  window: { workDoneProgress: true, showMessage: {} },
  workspace: {
    workspaceFolders: true,
    configuration: true,
    symbol: { symbolKind: { valueSet: range(1, 26) } },
    semanticTokens: { refreshSupport: true },
  },
  textDocument: {
    synchronization: { didSave: true, dynamicRegistration: false },
    completion: {
      completionItem: {
        snippetSupport: true,
        documentationFormat: ["markdown", "plaintext"],
        insertReplaceSupport: true,
        labelDetailsSupport: true,
      },
      completionItemKind: { valueSet: range(1, 25) },
      contextSupport: true,
    },
    hover: { contentFormat: ["markdown", "plaintext"] },
    signatureHelp: {
      signatureInformation: {
        documentationFormat: ["markdown", "plaintext"],
        parameterInformation: { labelOffsetSupport: true },
        activeParameterSupport: true,
      },
    },
    definition: { linkSupport: true },
    declaration: { linkSupport: true },
    typeDefinition: { linkSupport: true },
    implementation: { linkSupport: true },
    references: {},
    documentHighlight: {},
    documentSymbol: { hierarchicalDocumentSymbolSupport: true, symbolKind: { valueSet: range(1, 26) } },
    formatting: {},
    publishDiagnostics: { relatedInformation: true, tagSupport: { valueSet: [1, 2] } },
    semanticTokens: {
      requests: { full: true },
      tokenTypes: [
        "namespace", "type", "class", "enum", "interface", "struct", "typeParameter", "parameter", "variable",
        "property", "enumMember", "event", "function", "method", "macro", "keyword", "modifier", "comment",
        "string", "number", "regexp", "operator", "decorator", "concept", "label",
      ],
      tokenModifiers: [
        "declaration", "definition", "readonly", "static", "deprecated", "abstract", "async", "modification",
        "documentation", "defaultLibrary",
      ],
      formats: ["relative"],
      overlappingTokenSupport: false,
      multilineTokenSupport: false,
    },
  },
};

function range(from: number, to: number) {
  return Array.from({ length: to - from + 1 }, (_, i) => from + i);
}

export type ClientState = "starting" | "ready" | "exited";

export class LspClient {
  readonly id: number;
  readonly root: string;
  readonly rootUri: string;
  readonly command: string;
  readonly language: string;
  capabilities: any = {};
  state: ClientState = "starting";
  /** Work-done progress in flight (clangd indexing, rust-analyzer loading…). */
  progress = new Map<string | number, string>();
  exitInfo: { code: number | null; log: string[] } | null = null;

  private seq = 1;
  private pending = new Map<number, Pending>();
  private handlers = new Map<string, (params: any) => void>();
  private requestHandlers = new Map<string, (params: any) => any>();
  private listeners = new Set<Listener>();

  private constructor(info: StartInfo, language: string) {
    this.id = info.id;
    this.root = info.root;
    this.rootUri = monaco.Uri.file(info.root).toString();
    this.command = info.command;
    this.language = language;
  }

  /** Starts the server for a repo folder and completes the LSP handshake. */
  static async start(workspace: string, repo: string, language: string): Promise<LspClient> {
    await ensureListening();
    const info = await invoke<StartInfo>("lsp_start", { workspace, repo, language });
    const client = new LspClient(info, language);
    clients.set(info.id, client);
    try {
      await client.initialize();
    } catch (e) {
      // Died during the handshake: its stderr says why (bad install, bad flags).
      const log = client.exitInfo?.log.filter((l) => l.trim()) ?? [];
      client.stop();
      throw new Error(log.length ? `${client.serverName} failed to start: ${log.slice(-4).join("\n")}` : String(e));
    }
    return client;
  }

  /** "clangd", or for launcher commands ("npx --yes typescript-language-server@4
   *  --stdio") the server they launch. */
  get serverName(): string {
    const words = this.command.split(" ").filter(Boolean);
    const base = (w: string) => w.split(/[\\/]/).pop()!.replace(/\.(exe|cmd|bat)$/i, "");
    const launchers = ["npx", "node", "bunx", "pnpm", "yarn", "uvx", "pipx", "python", "python3", "py", "dotnet", "cmd"];
    if (launchers.includes(base(words[0] ?? "").toLowerCase())) {
      const target = words.slice(1).find((w) => !w.startsWith("-") && w.toLowerCase() !== "exec" && w.toLowerCase() !== "run");
      if (target) return base(target).replace(/(.)@[^@]*$/, "$1");
    }
    return base(words[0] ?? "");
  }

  /** Changes to state or progress (status chips re-render). */
  subscribe(l: Listener): () => void {
    this.listeners.add(l);
    return () => this.listeners.delete(l);
  }

  private changed() {
    this.listeners.forEach((l) => l());
  }

  on(method: string, handler: (params: any) => void) {
    this.handlers.set(method, handler);
  }

  /** Answers a request the server sends to the client. */
  onRequest(method: string, handler: (params: any) => any) {
    this.requestHandlers.set(method, handler);
  }

  private send(msg: object): Promise<unknown> {
    return invoke("lsp_send", { id: this.id, message: JSON.stringify(msg) });
  }

  request<T = any>(method: string, params: unknown, token?: monaco.CancellationToken): Promise<T> {
    if (this.state === "exited") return Promise.reject(new Error(`${this.serverName} is not running`));
    const id = this.seq++;
    return new Promise<T>((resolve, reject) => {
      this.pending.set(id, { resolve, reject });
      token?.onCancellationRequested(() => {
        if (!this.pending.has(id)) return;
        this.pending.delete(id);
        this.notify("$/cancelRequest", { id });
        reject(new Error("cancelled"));
      });
      this.send({ jsonrpc: "2.0", id, method, params }).catch((e) => {
        this.pending.delete(id);
        reject(new Error(String(e)));
      });
    });
  }

  notify(method: string, params: unknown) {
    if (this.state === "exited") return;
    this.send({ jsonrpc: "2.0", method, params }).catch(() => null);
  }

  receive(raw: string) {
    let msg: any;
    try {
      msg = JSON.parse(raw);
    } catch {
      return;
    }
    if (msg.method === undefined) {
      const p = this.pending.get(msg.id);
      if (!p) return;
      this.pending.delete(msg.id);
      if (msg.error) p.reject(new Error(msg.error.message ?? "request failed"));
      else p.resolve(msg.result ?? null);
    } else if (msg.id !== undefined) {
      this.answer(msg);
    } else {
      if (msg.method === "$/progress") this.trackProgress(msg.params);
      this.handlers.get(msg.method)?.(msg.params);
    }
  }

  /** Server → client requests: what Orbit supports, and polite no-ops. */
  private answer(msg: any) {
    const reply = (result: unknown) => this.send({ jsonrpc: "2.0", id: msg.id, result }).catch(() => null);
    const custom = this.requestHandlers.get(msg.method);
    if (custom) return reply(custom(msg.params) ?? null);
    switch (msg.method) {
      case "workspace/configuration":
        return reply((msg.params?.items ?? []).map(() => null));
      case "workspace/workspaceFolders":
        return reply([{ uri: this.rootUri, name: this.root.split(/[\\/]/).pop() }]);
      case "workspace/applyEdit":
        return reply({ applied: false });
      case "window/workDoneProgress/create":
      case "client/registerCapability":
      case "client/unregisterCapability":
      case "window/showMessageRequest":
      case "workspace/semanticTokens/refresh":
      case "workspace/inlayHint/refresh":
      case "workspace/diagnostic/refresh":
      case "workspace/codeLens/refresh":
        return reply(null);
      default:
        return this.send({ jsonrpc: "2.0", id: msg.id, error: { code: -32601, message: `unsupported: ${msg.method}` } }).catch(
          () => null,
        );
    }
  }

  private trackProgress(params: any) {
    const v = params?.value;
    if (!v) return;
    if (v.kind === "end") this.progress.delete(params.token);
    else {
      const pct = typeof v.percentage === "number" ? ` ${Math.round(v.percentage)}%` : "";
      const title = v.title ?? this.progress.get(params.token)?.split(" ·")[0] ?? "Working";
      this.progress.set(params.token, `${title}${v.message ? ` · ${v.message}` : ""}${pct}`);
    }
    this.changed();
  }

  exited(code: number | null, log: string[]) {
    this.state = "exited";
    this.exitInfo = { code, log };
    this.progress.clear();
    for (const p of this.pending.values()) p.reject(new Error(`${this.serverName} exited`));
    this.pending.clear();
    clients.delete(this.id);
    this.changed();
  }

  private async initialize() {
    const result = await this.request("initialize", {
      processId: null,
      clientInfo: { name: "Orbit" },
      rootUri: this.rootUri,
      rootPath: this.root,
      workspaceFolders: [{ uri: this.rootUri, name: this.root.split(/[\\/]/).pop() }],
      capabilities: CLIENT_CAPABILITIES,
      initializationOptions: {},
    });
    this.capabilities = result?.capabilities ?? {};
    this.notify("initialized", {});
    this.state = "ready";
    this.changed();
  }

  /** Graceful shutdown (the backend kills it if it lingers). */
  stop() {
    if (this.state !== "exited") invoke("lsp_stop", { id: this.id }).catch(() => null);
    this.exited(null, []);
  }
}
