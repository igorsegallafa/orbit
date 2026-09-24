// Language servers ⇄ Monaco: which server serves a file, document sync,
// the Monaco providers (definition, references, hover, completion, symbols,
// semantic tokens…), diagnostics, and opening definitions that live in
// other files as Orbit tabs.
import { invoke } from "@tauri-apps/api/core";
import * as monaco from "monaco-editor";
import { LspClient } from "./client";

/* eslint-disable @typescript-eslint/no-explicit-any */

/** Monaco language → Orbit language (lsp.rs registry id). */
const LANGUAGE_OF: Record<string, string> = {
  c: "cpp",
  cpp: "cpp",
  rust: "rust",
  typescript: "typescript",
  javascript: "typescript",
  python: "python",
  go: "go",
  csharp: "csharp",
  lua: "lua",
};

/** LSP languageId for a file (TSX/JSX differ from their Monaco language). */
function lspLanguageId(monacoLang: string, path: string): string {
  if (/\.tsx$/i.test(path)) return "typescriptreact";
  if (/\.jsx$/i.test(path)) return "javascriptreact";
  return monacoLang;
}

export function languageFor(monacoLang: string): string | null {
  return LANGUAGE_OF[monacoLang] ?? null;
}

export interface DocContext {
  workspace: string;
  repo: string;
  /** Repo-relative path. */
  path: string;
}

export type LspState = "off" | "starting" | "ready" | "missing" | "exited";

export interface LspStatus {
  state: LspState;
  /** Orbit language id ("cpp"). */
  language: string | null;
  server: string | null;
  /** Indexing / loading progress text. */
  progress: string | null;
  error: string | null;
}

interface ClientEntry {
  key: string;
  workspace: string;
  repo: string;
  language: string;
  promise: Promise<LspClient>;
  client: LspClient | null;
  error: string | null;
  docs: Set<string>;
  idleTimer: number | null;
}

interface Doc {
  model: monaco.editor.ITextModel;
  ctx: DocContext;
  entry: ClientEntry;
  version: number;
  languageId: string;
  flushTimer: number | null;
  dirty: boolean;
  detach: () => void;
}

const entries = new Map<string, ClientEntry>();
const docs = new Map<string, Doc>();
const statusListeners = new Map<string, Set<() => void>>();
const registeredLanguages = new Set<string>();
const semanticRefresh = new Map<string, monaco.Emitter<void>>();
/** Latest diagnostics per file, applied when its model appears. */
const diagnostics = new Map<string, any[]>();

/** How Orbit opens a repo file in a tab (set by App). */
type Opener = (ctx: DocContext, line: number, column: number) => void;
let openFile: Opener | null = null;
export function setFileOpener(fn: Opener) {
  openFile = fn;
}

/** Stop a server this long after its last file closed. */
const IDLE_STOP_MS = 3 * 60 * 1000;

const entryKey = (workspace: string, repo: string, language: string) => `${workspace}|${repo}|${language}`;

// ---------- paths & positions ----------

const isWindows = navigator.userAgent.includes("Windows");

/** Comparable form of a file path (separators, and case on Windows). */
function normPath(p: string): string {
  const s = p.replace(/\\/g, "/").replace(/\/+$/, "");
  return isWindows ? s.toLowerCase() : s;
}

export function fileUri(absPath: string): string {
  return monaco.Uri.file(absPath).toString();
}

function toRange(r: any): monaco.IRange {
  return {
    startLineNumber: r.start.line + 1,
    startColumn: r.start.character + 1,
    endLineNumber: r.end.line + 1,
    endColumn: r.end.character + 1,
  };
}

function toPosition(p: monaco.Position) {
  return { line: p.lineNumber - 1, character: p.column - 1 };
}

function markdown(content: any): monaco.IMarkdownString[] {
  if (!content) return [];
  if (Array.isArray(content)) return content.flatMap(markdown);
  if (typeof content === "string") return [{ value: content }];
  if (content.kind === "plaintext") return [{ value: content.value.replace(/[\\`*_{}[\]()#+\-.!]/g, "\\$&") }];
  if (content.language) return [{ value: "```" + content.language + "\n" + content.value + "\n```" }];
  return [{ value: content.value ?? "" }];
}

/** Where a server-reported file belongs: a repo folder with a running server. */
function locate(uri: monaco.Uri): DocContext | null {
  const file = normPath(uri.fsPath);
  for (const e of entries.values()) {
    const root = e.client ? normPath(e.client.root) : null;
    if (root && file.startsWith(root + "/")) {
      const rel = uri.fsPath.replace(/\\/g, "/").slice(e.client!.root.replace(/\\/g, "/").replace(/\/+$/, "").length + 1);
      return { workspace: e.workspace, repo: e.repo, path: rel };
    }
  }
  return null;
}

/** Makes sure Monaco has a model for a file a result points at, so peek
 *  views and "go to definition" can show it. */
async function ensureModel(uriStr: string): Promise<monaco.Uri> {
  const uri = monaco.Uri.parse(uriStr);
  if (!monaco.editor.getModel(uri)) {
    try {
      const text = await invoke<string>("lsp_read_file", { path: uri.fsPath });
      if (!monaco.editor.getModel(uri)) monaco.editor.createModel(text, undefined, uri);
    } catch {
      // Unreadable (binary, gone): the location still shows, without preview.
    }
  }
  return uri;
}

async function toLocations(result: any): Promise<monaco.languages.Location[]> {
  if (!result) return [];
  const list: any[] = Array.isArray(result) ? result : [result];
  const locs = list.slice(0, 300).map((l) =>
    l.targetUri ? { uri: l.targetUri, range: l.targetSelectionRange ?? l.targetRange } : { uri: l.uri, range: l.range },
  );
  return Promise.all(locs.map(async (l) => ({ uri: await ensureModel(l.uri), range: toRange(l.range) })));
}

// ---------- clients ----------

function notifyStatus(entry: ClientEntry) {
  for (const uri of entry.docs) statusListeners.get(uri)?.forEach((l) => l());
}

function getEntry(workspace: string, repo: string, language: string): ClientEntry {
  const key = entryKey(workspace, repo, language);
  let entry = entries.get(key);
  if (entry && entry.client?.state === "exited") {
    // Crashed or stopped: start fresh on next use.
    entries.delete(key);
    entry = undefined;
  }
  if (!entry) {
    const e: ClientEntry = { key, workspace, repo, language, client: null, error: null, docs: new Set(), idleTimer: null, promise: null! };
    e.promise = LspClient.start(workspace, repo, language).then(
      (client) => {
        e.client = client;
        wireClient(client);
        client.subscribe(() => notifyStatus(e));
        notifyStatus(e);
        return client;
      },
      (err) => {
        e.error = String(err);
        notifyStatus(e);
        throw err;
      },
    );
    e.promise.catch(() => null);
    entries.set(key, e);
    entry = e;
  }
  return entry;
}

function wireClient(client: LspClient) {
  client.on("textDocument/publishDiagnostics", (p) => {
    diagnostics.set(monaco.Uri.parse(p.uri).toString(), p.diagnostics ?? []);
    applyDiagnostics(monaco.Uri.parse(p.uri));
  });
  client.onRequest("workspace/semanticTokens/refresh", () => {
    semanticRefresh.forEach((em) => em.fire());
    return null;
  });
  // Monaco's own TS features are set aside at startup when a TS server is
  // available (configureBuiltinTypeScript). A server that turns up later
  // (installed mid-session) still clears its syntax markers, which the
  // diagnostics options (unlike the rest) apply live.
  if (client.language === "typescript") {
    for (const defaults of [monaco.typescript.typescriptDefaults, monaco.typescript.javascriptDefaults]) {
      defaults.setDiagnosticsOptions({ noSemanticValidation: true, noSyntaxValidation: true, noSuggestionDiagnostics: true });
    }
  }
}

const SEVERITY: Record<number, monaco.MarkerSeverity> = {
  1: monaco.MarkerSeverity.Error,
  2: monaco.MarkerSeverity.Warning,
  3: monaco.MarkerSeverity.Info,
  4: monaco.MarkerSeverity.Hint,
};

function applyDiagnostics(uri: monaco.Uri) {
  const model = monaco.editor.getModel(uri);
  if (!model) return;
  const list = diagnostics.get(uri.toString()) ?? [];
  monaco.editor.setModelMarkers(
    model,
    "lsp",
    list.map((d) => ({
      ...toRange(d.range),
      severity: SEVERITY[d.severity ?? 1] ?? monaco.MarkerSeverity.Error,
      message: d.message,
      source: d.source,
      code: typeof d.code === "object" ? String(d.code?.value ?? "") : d.code !== undefined ? String(d.code) : undefined,
      tags: (d.tags ?? []).map((t: number) => (t === 1 ? monaco.MarkerTag.Unnecessary : monaco.MarkerTag.Deprecated)),
    })),
  );
}

// ---------- documents ----------

/** Sends pending edits now (requests must see the latest text). */
function flush(doc: Doc) {
  if (doc.flushTimer !== null) {
    window.clearTimeout(doc.flushTimer);
    doc.flushTimer = null;
  }
  if (!doc.dirty || !doc.entry.client) return;
  doc.dirty = false;
  doc.version++;
  doc.entry.client.notify("textDocument/didChange", {
    textDocument: { uri: doc.model.uri.toString(), version: doc.version },
    // Whole text: valid whatever sync kind the server asked for.
    contentChanges: [{ text: doc.model.getValue() }],
  });
}

/** The ready client serving a model, with its edits flushed. */
async function clientFor(model: monaco.editor.ITextModel): Promise<LspClient | null> {
  const doc = docs.get(model.uri.toString());
  if (!doc) return null;
  const client = await doc.entry.promise.catch(() => null);
  if (!client || client.state !== "ready") return null;
  flush(doc);
  return client;
}

/**
 * Connects an editor model to its language server (started on demand for
 * the repo folder). Returns the detach function; the status of the server
 * is available through `status(model)` / `onStatus(model, …)`.
 */
export function attach(model: monaco.editor.ITextModel, ctx: DocContext): () => void {
  const lang = languageFor(model.getLanguageId());
  const uri = model.uri.toString();
  if (!lang || docs.has(uri)) return () => undefined;
  const entry = getEntry(ctx.workspace, ctx.repo, lang);
  if (entry.idleTimer !== null) {
    window.clearTimeout(entry.idleTimer);
    entry.idleTimer = null;
  }
  const doc: Doc = {
    model,
    ctx,
    entry,
    version: 1,
    languageId: lspLanguageId(model.getLanguageId(), ctx.path),
    flushTimer: null,
    dirty: false,
    detach: () => undefined,
  };
  docs.set(uri, doc);
  entry.docs.add(uri);
  notifyStatus(entry);

  entry.promise
    .then((client) => {
      if (docs.get(uri) !== doc) return;
      registerProviders(model.getLanguageId(), client);
      client.notify("textDocument/didOpen", {
        textDocument: { uri, languageId: doc.languageId, version: doc.version, text: model.getValue() },
      });
      applyDiagnostics(model.uri);
    })
    .catch(() => null);

  const change = model.onDidChangeContent(() => {
    doc.dirty = true;
    if (doc.flushTimer !== null) window.clearTimeout(doc.flushTimer);
    doc.flushTimer = window.setTimeout(() => flush(doc), 250);
  });
  let detached = false;
  const detach = () => {
    if (detached) return;
    detached = true;
    change.dispose();
    dispose.dispose();
    if (doc.flushTimer !== null) window.clearTimeout(doc.flushTimer);
    // A restart may already have attached the model again: leave that one be.
    if (docs.get(uri) === doc) docs.delete(uri);
    entry.docs.delete(uri);
    entry.client?.notify("textDocument/didClose", { textDocument: { uri } });
    if (entry.docs.size === 0) {
      entry.idleTimer = window.setTimeout(() => {
        if (entry.docs.size > 0) return;
        entry.client?.stop();
        if (entries.get(entry.key) === entry) entries.delete(entry.key);
      }, IDLE_STOP_MS);
    }
  };
  const dispose = model.onWillDispose(detach);
  doc.detach = detach;
  return detach;
}

/** Tells the server a file was written (some re-index on save). */
export function saved(model: monaco.editor.ITextModel) {
  const doc = docs.get(model.uri.toString());
  if (!doc?.entry.client) return;
  flush(doc);
  doc.entry.client.notify("textDocument/didSave", { textDocument: { uri: doc.model.uri.toString() }, text: doc.model.getValue() });
}

export function status(model: monaco.editor.ITextModel | null): LspStatus {
  const lang = model ? languageFor(model.getLanguageId()) : null;
  const doc = model ? docs.get(model.uri.toString()) : undefined;
  if (!lang || !doc) return { state: "off", language: lang, server: null, progress: null, error: null };
  const e = doc.entry;
  const client = e.client;
  if (e.error) return { state: "missing", language: lang, server: null, progress: null, error: e.error };
  if (!client) return { state: "starting", language: lang, server: null, progress: null, error: null };
  if (client.state === "exited") {
    const why = client.exitInfo?.log.slice(-3).join("\n") || (client.exitInfo?.code != null ? `exit code ${client.exitInfo.code}` : "stopped");
    return { state: "exited", language: lang, server: client.serverName, progress: null, error: why };
  }
  const progress = [...client.progress.values()].pop() ?? null;
  return { state: client.state === "ready" ? "ready" : "starting", language: lang, server: client.serverName, progress, error: null };
}

export function onStatus(model: monaco.editor.ITextModel, cb: () => void): () => void {
  const uri = model.uri.toString();
  let set = statusListeners.get(uri);
  if (!set) statusListeners.set(uri, (set = new Set()));
  set.add(cb);
  return () => set!.delete(cb);
}

/** Restarts the server serving a model: every file it served reconnects. */
export async function restart(model: monaco.editor.ITextModel) {
  const doc = docs.get(model.uri.toString());
  if (!doc) return;
  const old = doc.entry;
  const served = [...old.docs].map((u) => docs.get(u)).filter((d): d is Doc => !!d);
  for (const d of served) d.detach();
  if (old.idleTimer !== null) window.clearTimeout(old.idleTimer);
  old.client?.stop();
  if (entries.get(old.key) === old) entries.delete(old.key);
  for (const d of served) attach(d.model, d.ctx);
}

/** Stops a server by its backend id (Settings → Languages). */
export function stopServer(id: number) {
  for (const e of entries.values()) {
    if (e.client?.id === id) {
      e.client.stop();
      entries.delete(e.key);
      notifyStatus(e);
    }
  }
}

/** The server session behind a model, for workspace-wide queries. */
export async function serverFor(model: monaco.editor.ITextModel): Promise<LspClient | null> {
  return clientFor(model);
}

export interface SymbolHit {
  name: string;
  kind: number;
  container: string;
  ctx: DocContext | null;
  file: string;
  line: number;
  column: number;
}

/** Project-wide symbol search (workspace/symbol) from a file's server. */
export async function searchSymbols(model: monaco.editor.ITextModel, query: string): Promise<SymbolHit[]> {
  const client = await clientFor(model);
  if (!client) return [];
  const result: any[] = (await client.request("workspace/symbol", { query }).catch(() => [])) ?? [];
  return result.slice(0, 200).map((s) => {
    const uri = monaco.Uri.parse(s.location.uri);
    const start = s.location.range?.start ?? { line: 0, character: 0 };
    return {
      name: s.name,
      kind: s.kind,
      container: s.containerName ?? "",
      ctx: locate(uri),
      file: uri.fsPath,
      line: start.line + 1,
      column: start.character + 1,
    };
  });
}

/** Opens a symbol hit (or any repo file position) as an Orbit tab. */
export function openLocation(ctx: DocContext, line: number, column: number) {
  openFile?.(ctx, line, column);
}

// ---------- Monaco providers ----------

const COMPLETION_KIND: Record<number, monaco.languages.CompletionItemKind> = (() => {
  const K = monaco.languages.CompletionItemKind;
  return {
    1: K.Text, 2: K.Method, 3: K.Function, 4: K.Constructor, 5: K.Field, 6: K.Variable, 7: K.Class, 8: K.Interface,
    9: K.Module, 10: K.Property, 11: K.Unit, 12: K.Value, 13: K.Enum, 14: K.Keyword, 15: K.Snippet, 16: K.Color,
    17: K.File, 18: K.Reference, 19: K.Folder, 20: K.EnumMember, 21: K.Constant, 22: K.Struct, 23: K.Event,
    24: K.Operator, 25: K.TypeParameter,
  };
})();

function toSymbol(s: any): monaco.languages.DocumentSymbol {
  // Hierarchical DocumentSymbol, or flat SymbolInformation.
  const range = toRange(s.range ?? s.location.range);
  return {
    name: s.name,
    detail: s.detail ?? s.containerName ?? "",
    kind: ((s.kind ?? 13) - 1) as monaco.languages.SymbolKind,
    tags: [],
    range,
    selectionRange: s.selectionRange ? toRange(s.selectionRange) : range,
    children: (s.children ?? []).map(toSymbol),
  };
}

function registerProviders(monacoLang: string, client: LspClient) {
  if (registeredLanguages.has(monacoLang)) return;
  registeredLanguages.add(monacoLang);
  const caps = client.capabilities;
  const L = monaco.languages;
  const uriOf = (model: monaco.editor.ITextModel) => ({ uri: model.uri.toString() });
  const at = (model: monaco.editor.ITextModel, pos: monaco.Position) => ({ textDocument: uriOf(model), position: toPosition(pos) });

  L.registerHoverProvider(monacoLang, {
    async provideHover(model, pos, token) {
      const c = await clientFor(model);
      const r = await c?.request("textDocument/hover", at(model, pos), token).catch(() => null);
      if (!r?.contents) return null;
      return { contents: markdown(r.contents), range: r.range ? toRange(r.range) : undefined };
    },
  });

  const locationProvider = (method: string) => ({
    async provide(model: monaco.editor.ITextModel, pos: monaco.Position, token: monaco.CancellationToken) {
      const c = await clientFor(model);
      const r = await c?.request(method, at(model, pos), token).catch(() => null);
      return toLocations(r);
    },
  });
  const def = locationProvider("textDocument/definition");
  L.registerDefinitionProvider(monacoLang, { provideDefinition: def.provide });
  const decl = locationProvider("textDocument/declaration");
  L.registerDeclarationProvider(monacoLang, { provideDeclaration: decl.provide });
  const typeDef = locationProvider("textDocument/typeDefinition");
  L.registerTypeDefinitionProvider(monacoLang, { provideTypeDefinition: typeDef.provide });
  const impl = locationProvider("textDocument/implementation");
  L.registerImplementationProvider(monacoLang, { provideImplementation: impl.provide });

  L.registerReferenceProvider(monacoLang, {
    async provideReferences(model, pos, context, token) {
      const c = await clientFor(model);
      const r = await c
        ?.request("textDocument/references", { ...at(model, pos), context: { includeDeclaration: context.includeDeclaration } }, token)
        .catch(() => null);
      return toLocations(r);
    },
  });

  L.registerDocumentHighlightProvider(monacoLang, {
    async provideDocumentHighlights(model, pos, token) {
      const c = await clientFor(model);
      const r: any[] | null = await c?.request("textDocument/documentHighlight", at(model, pos), token).catch(() => null);
      return (r ?? []).map((h) => ({ range: toRange(h.range), kind: ((h.kind ?? 1) - 1) as monaco.languages.DocumentHighlightKind }));
    },
  });

  L.registerDocumentSymbolProvider(monacoLang, {
    async provideDocumentSymbols(model, token) {
      const c = await clientFor(model);
      const r: any[] | null = await c?.request("textDocument/documentSymbol", { textDocument: uriOf(model) }, token).catch(() => null);
      return (r ?? []).map(toSymbol);
    },
  });

  L.registerCompletionItemProvider(monacoLang, {
    triggerCharacters: caps.completionProvider?.triggerCharacters ?? [".", ":", ">"],
    async provideCompletionItems(model, pos, context, token) {
      const c = await clientFor(model);
      const r = await c
        ?.request(
          "textDocument/completion",
          {
            ...at(model, pos),
            context: context.triggerCharacter ? { triggerKind: 2, triggerCharacter: context.triggerCharacter } : { triggerKind: 1 },
          },
          token,
        )
        .catch(() => null);
      if (!r) return { suggestions: [] };
      const items: any[] = Array.isArray(r) ? r : (r.items ?? []);
      const word = model.getWordUntilPosition(pos);
      const fallback = new monaco.Range(pos.lineNumber, word.startColumn, pos.lineNumber, word.endColumn);
      return {
        incomplete: !Array.isArray(r) && !!r.isIncomplete,
        suggestions: items.map((item) => {
          const te = item.textEdit;
          const range = te
            ? te.range
              ? toRange(te.range)
              : { insert: toRange(te.insert), replace: toRange(te.replace) }
            : fallback;
          const label = String(item.label).trim();
          return {
            label: item.labelDetails ? { label, detail: item.labelDetails.detail, description: item.labelDetails.description } : label,
            kind: COMPLETION_KIND[item.kind] ?? monaco.languages.CompletionItemKind.Text,
            detail: item.detail,
            documentation: item.documentation ? markdown(item.documentation)[0] : undefined,
            insertText: te?.newText ?? item.insertText ?? label,
            insertTextRules: item.insertTextFormat === 2 ? monaco.languages.CompletionItemInsertTextRule.InsertAsSnippet : undefined,
            range,
            sortText: item.sortText,
            filterText: item.filterText,
            preselect: item.preselect,
            additionalTextEdits: item.additionalTextEdits?.map((e: any) => ({ range: toRange(e.range), text: e.newText })),
          } as monaco.languages.CompletionItem;
        }),
      };
    },
  });

  L.registerSignatureHelpProvider(monacoLang, {
    signatureHelpTriggerCharacters: caps.signatureHelpProvider?.triggerCharacters ?? ["(", ","],
    signatureHelpRetriggerCharacters: caps.signatureHelpProvider?.retriggerCharacters ?? [","],
    async provideSignatureHelp(model, pos, token) {
      const c = await clientFor(model);
      const r = await c?.request("textDocument/signatureHelp", at(model, pos), token).catch(() => null);
      if (!r?.signatures?.length) return null;
      return {
        value: {
          activeSignature: r.activeSignature ?? 0,
          activeParameter: r.activeParameter ?? 0,
          signatures: r.signatures.map((s: any) => ({
            label: s.label,
            documentation: s.documentation ? markdown(s.documentation)[0] : undefined,
            parameters: (s.parameters ?? []).map((p: any) => ({
              label: p.label,
              documentation: p.documentation ? markdown(p.documentation)[0] : undefined,
            })),
            activeParameter: s.activeParameter,
          })),
        },
        dispose: () => undefined,
      };
    },
  });

  L.registerDocumentFormattingEditProvider(monacoLang, {
    async provideDocumentFormattingEdits(model, options, token) {
      const c = await clientFor(model);
      const r: any[] | null = await c
        ?.request("textDocument/formatting", { textDocument: uriOf(model), options: { tabSize: options.tabSize, insertSpaces: options.insertSpaces } }, token)
        .catch(() => null);
      return (r ?? []).map((e) => ({ range: toRange(e.range), text: e.newText }));
    },
  });

  const legend = caps.semanticTokensProvider?.legend;
  if (legend && caps.semanticTokensProvider?.full) {
    const refresh = new monaco.Emitter<void>();
    semanticRefresh.set(monacoLang, refresh);
    L.registerDocumentSemanticTokensProvider(monacoLang, {
      onDidChange: refresh.event,
      getLegend: () => legend,
      async provideDocumentSemanticTokens(model, _lastResultId, token) {
        const c = await clientFor(model);
        const r = await c?.request("textDocument/semanticTokens/full", { textDocument: uriOf(model) }, token).catch(() => null);
        return r?.data ? { data: new Uint32Array(r.data), resultId: r.resultId } : null;
      },
      releaseDocumentSemanticTokens: () => undefined,
    });
  }
}

// Definitions in other files open as Orbit tabs (Monaco alone would do
// nothing: the standalone editor shows one file).
monaco.editor.registerEditorOpener({
  openCodeEditor(source, resource, selectionOrPosition) {
    if (source.getModel()?.uri.toString() === resource.toString()) return false;
    const ctx = locate(resource);
    if (!ctx || !openFile) return false;
    const pos = selectionOrPosition
      ? "startLineNumber" in selectionOrPosition
        ? { line: selectionOrPosition.startLineNumber, column: selectionOrPosition.startColumn }
        : { line: selectionOrPosition.lineNumber, column: selectionOrPosition.column }
      : { line: 1, column: 1 };
    openFile(ctx, pos.line, pos.column);
    return true;
  },
});
