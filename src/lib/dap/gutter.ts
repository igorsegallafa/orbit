// Breakpoints and the current execution line in the editors: a click in the
// gutter (left of the line numbers) or Ctrl+F8 toggles a breakpoint, and the
// line where the debugger stopped is highlighted.
import * as monaco from "monaco-editor";
import { breakpointsIn, getDebugState, subscribeDebug, toggleBreakpoint } from "./session";
import { comparablePath } from "../lsp/paths";

const IS_WINDOWS = navigator.userAgent.includes("Windows");
const collections = new Map<monaco.editor.IStandaloneCodeEditor, monaco.editor.IEditorDecorationsCollection>();

function decorate(editor: monaco.editor.IStandaloneCodeEditor) {
  const model = editor.getModel();
  const coll = collections.get(editor);
  if (!model || !coll) return;
  const file = model.uri.fsPath;
  const { frames, frameId, status } = getDebugState();
  const frame = status === "stopped" ? frames.find((f) => f.id === frameId) : undefined;
  const here = frame?.file && comparablePath(frame.file, IS_WINDOWS) === comparablePath(file, IS_WINDOWS) ? frame.line : null;
  const bps = breakpointsIn(file);
  const list: monaco.editor.IModelDeltaDecoration[] = bps.map((line) => ({
    range: new monaco.Range(line, 1, line, 1),
    options: { glyphMarginClassName: line === here ? "dbg-glyph-bp dbg-glyph-here" : "dbg-glyph-bp", glyphMarginHoverMessage: { value: "Breakpoint (click to remove)" }, stickiness: 1 },
  }));
  if (here !== null) {
    list.push({
      range: new monaco.Range(here, 1, here, 1),
      options: { isWholeLine: true, className: "dbg-line-here", ...(bps.includes(here) ? {} : { glyphMarginClassName: "dbg-glyph-here" }) },
    });
  }
  coll.set(list);
}

subscribeDebug(() => collections.forEach((_, ed) => decorate(ed)));

/** Breakpoint gutter for an editor; returns the cleanup. */
export function attachDebugGutter(editor: monaco.editor.IStandaloneCodeEditor): () => void {
  editor.updateOptions({ glyphMargin: true });
  collections.set(editor, editor.createDecorationsCollection());
  decorate(editor);
  const T = monaco.editor.MouseTargetType;
  const subs = [
    editor.onMouseDown((e) => {
      if (e.target.type !== T.GUTTER_GLYPH_MARGIN || !e.target.position) return;
      const model = editor.getModel();
      if (model) toggleBreakpoint(model.uri.fsPath, e.target.position.lineNumber);
    }),
    editor.onDidChangeModel(() => decorate(editor)),
    editor.addAction({
      id: "orbit.toggleBreakpoint",
      label: "Toggle Breakpoint",
      keybindings: [monaco.KeyMod.CtrlCmd | monaco.KeyCode.F8],
      run: (ed) => {
        const model = ed.getModel();
        const pos = ed.getPosition();
        if (model && pos) toggleBreakpoint(model.uri.fsPath, pos.lineNumber);
      },
    }),
  ];
  return () => {
    subs.forEach((s) => s.dispose());
    try {
      collections.get(editor)?.clear();
    } catch {
      // The editor is already disposed (its tab closed first).
    }
    collections.delete(editor);
  };
}
