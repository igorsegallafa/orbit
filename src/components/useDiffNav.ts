import { useCallback, useRef, useState } from "react";
import type { editor as MonacoEditor } from "monaco-editor";

type DiffEditor = MonacoEditor.IStandaloneDiffEditor;

/** Previous/next-change navigation + scrollbar change markers for a Monaco
 *  DiffEditor. Render the returned {count, index} in the filebar and pass
 *  onMount to <DiffEditor>. F7/Shift+F7 are wired on the modified editor. */
export function useDiffNav() {
  const editorRef = useRef<DiffEditor | null>(null);
  const [count, setCount] = useState(0);
  const [index, setIndex] = useState(-1); // -1 until the first jump

  const jump = useCallback((dir: 1 | -1) => {
    const ed = editorRef.current;
    if (!ed) return;
    const changes = ed.getLineChanges() ?? [];
    if (!changes.length) return;
    const modified = ed.getModifiedEditor();
    setIndex((i) => {
      const next = Math.min(changes.length - 1, Math.max(0, i + dir));
      const ch = changes[next];
      // Pure deletions have modifiedStartLineNumber 0 — land on the line
      // after the deletion point in the modified side.
      const line = ch.modifiedStartLineNumber || ch.originalStartLineNumber + 1 || 1;
      const end = ch.modifiedEndLineNumber || line;
      modified.revealLinesNearTop(line, Math.max(line, end), 0);
      modified.setPosition({ lineNumber: line, column: 1 });
      return next;
    });
  }, []);

  const onMount = useCallback((editor: DiffEditor, monaco: typeof import("monaco-editor")) => {
    editorRef.current = editor;
    const modified = editor.getModifiedEditor();

    // F7 next change / Shift+F7 previous
    modified.addCommand(monaco.KeyMod.Shift | monaco.KeyCode.F7, () => jump(-1));
    modified.addCommand(monaco.KeyCode.F7, () => jump(1));

    const update = () => {
      const changes = editor.getLineChanges() ?? [];
      setCount(changes.length);
      // Scrollbar change markers are native: the DiffEditor overview ruler
      // (renderOverviewRuler: true) colors removed/added lines by itself.
    };

    editor.onDidUpdateDiff(update);
    if (editor.getLineChanges() !== null) update();
  }, [jump]);

  return { count, index, jump, onMount };
}
