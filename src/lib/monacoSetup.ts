// Monaco from the bundled package instead of @monaco-editor/react's default
// CDN download: the editor works offline and every editor instance (files,
// diffs, reviews, find previews) shares the one the language servers extend.
import * as monaco from "monaco-editor";
import { loader } from "@monaco-editor/react";
import { invoke } from "@tauri-apps/api/core";
// The package's `exports` map serves esm/vs/* at the root ("./*" → "./esm/vs/*.js").
import EditorWorker from "monaco-editor/editor/editor.worker?worker";
import JsonWorker from "monaco-editor/language/json/json.worker?worker";
import CssWorker from "monaco-editor/language/css/css.worker?worker";
import HtmlWorker from "monaco-editor/language/html/html.worker?worker";
import TsWorker from "monaco-editor/language/typescript/ts.worker?worker";

self.MonacoEnvironment = {
  getWorker(_id: string, label: string) {
    switch (label) {
      case "json":
        return new JsonWorker();
      case "css":
      case "scss":
      case "less":
        return new CssWorker();
      case "html":
      case "handlebars":
      case "razor":
        return new HtmlWorker();
      case "typescript":
      case "javascript":
        return new TsWorker();
      default:
        return new EditorWorker();
    }
  },
};

loader.config({ monaco });

// Monaco's built-in TypeScript service sees one file at a time, without the
// project's node_modules or tsconfig: its semantic errors are all false
// ("Cannot find module 'react'"). Keep its syntax checks only; a real
// language server (Settings → Languages) brings the rest.
for (const defaults of [monaco.typescript.typescriptDefaults, monaco.typescript.javascriptDefaults]) {
  defaults.setDiagnosticsOptions({ noSemanticValidation: true, noSuggestionDiagnostics: true, noSyntaxValidation: false });
}

/**
 * With a TypeScript language server available, Monaco's own TS features
 * step aside entirely: both would answer, doubling every definition (a
 * peek list instead of a jump), hover and completion. Must run before the
 * first TS/JS model exists: Monaco registers those providers once, then.
 */
export async function configureBuiltinTypeScript() {
  let available: string[] = [];
  try {
    available = await invoke<string[]>("lsp_available");
  } catch {
    return;
  }
  if (!available.includes("typescript")) return;
  for (const defaults of [monaco.typescript.typescriptDefaults, monaco.typescript.javascriptDefaults]) {
    defaults.setModeConfiguration({
      ...defaults.modeConfiguration,
      completionItems: false,
      hovers: false,
      documentSymbols: false,
      definitions: false,
      references: false,
      documentHighlights: false,
      rename: false,
      diagnostics: false,
      documentRangeFormattingEdits: false,
      signatureHelp: false,
      onTypeFormattingEdits: false,
      codeActions: false,
      inlayHints: false,
    });
  }
}
