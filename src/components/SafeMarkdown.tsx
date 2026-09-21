import { useMemo } from "react";
import { marked } from "marked";
import DOMPurify from "dompurify";
import { openUrl } from "@tauri-apps/plugin-opener";

/** Markdown from other people (PR descriptions, review comments): sanitized,
 *  since the webview can reach Tauri commands, and links open in the
 *  browser instead of navigating the app. */
export function SafeMarkdown({ content, className }: { content: string; className?: string }) {
  const html = useMemo(() => {
    const raw = marked.parse(content, { async: false, gfm: true, breaks: true }) as string;
    return DOMPurify.sanitize(raw, { USE_PROFILES: { html: true } });
  }, [content]);

  return (
    <div
      className={`safe-md ${className ?? ""}`}
      onClick={(e) => {
        const a = (e.target as HTMLElement).closest("a");
        if (!a) return;
        e.preventDefault();
        const href = a.getAttribute("href") ?? "";
        if (/^https?:\/\//i.test(href)) openUrl(href).catch(() => null);
      }}
      // eslint-disable-next-line react/no-danger -- sanitized by DOMPurify above
      dangerouslySetInnerHTML={{ __html: html }}
    />
  );
}
