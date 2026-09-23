import { useMemo } from "react";
import { marked } from "marked";

interface Props {
  content: string;
}

/**
 * Read-only markdown renderer for the editor's Preview mode (JetBrains-style
 * Editor/Preview toggle for .md files). Sanitization isn't needed — content
 * comes from the user's own local files, and Tauri's webview has no origin
 * to protect beyond the app itself.
 */
export function MarkdownPreview({ content }: Props) {
  const html = useMemo(() => {
    return marked.parse(content, { async: false, gfm: true, breaks: false });
  }, [content]);

  return (
    <div className="md-preview">
      {/* eslint-disable-next-line react/no-danger -- local files, no sanitization needed */}
      <div className="md-body" dangerouslySetInnerHTML={{ __html: html }} />
    </div>
  );
}