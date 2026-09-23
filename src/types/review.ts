export type Side = "LEFT" | "RIGHT";

export interface ReviewComment {
  id: number;
  nodeId: string;
  author: string;
  avatarUrl: string;
  body: string;
  createdAt: string;
  url: string;
  state: string;
  isMine: boolean;
}

export interface ReviewThread {
  id: string;
  path: string;
  side: Side;
  line: number | null;
  startLine: number | null;
  originalLine: number | null;
  isResolved: boolean;
  isOutdated: boolean;
  comments: ReviewComment[];
}

export interface ReviewSummary {
  author: string;
  avatarUrl: string;
  state: "APPROVED" | "CHANGES_REQUESTED" | "COMMENTED" | "DISMISSED" | "PENDING" | string;
  body: string;
  submittedAt: string | null;
}

export interface Commentable {
  right: [number, number][];
  left: [number, number][];
}

export interface ReviewData {
  viewer: string;
  prAuthor: string;
  reviewDecision: string | null;
  threads: ReviewThread[];
  reviews: ReviewSummary[];
  commentable: Record<string, Commentable>;
}

/** A pending comment of the review being written (local until submitted). */
export interface Draft {
  id: string;
  path: string;
  side: Side;
  line: number;
  startLine?: number;
  body: string;
}

export type ReviewEvent = "COMMENT" | "APPROVE" | "REQUEST_CHANGES";

/** Hunk containing `line` on `side`, or null when the line can't take comments. */
export function hunkOf(c: Commentable | undefined, side: Side, line: number): [number, number] | null {
  const ranges = side === "RIGHT" ? c?.right : c?.left;
  return ranges?.find(([a, b]) => line >= a && line <= b) ?? null;
}

/** "3h ago" style relative time. */
export function timeAgo(iso: string): string {
  const t = Date.parse(iso);
  if (Number.isNaN(t)) return "";
  const s = Math.max(0, (Date.now() - t) / 1000);
  if (s < 60) return "just now";
  if (s < 3600) return `${Math.floor(s / 60)}m ago`;
  if (s < 86400) return `${Math.floor(s / 3600)}h ago`;
  if (s < 86400 * 30) return `${Math.floor(s / 86400)}d ago`;
  return new Date(t).toLocaleDateString();
}
