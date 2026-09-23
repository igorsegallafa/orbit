/**
 * Skeleton placeholders (shimmer) for loading states — visible feedback
 * while data loads instead of empty gaps.
 */
export function Skeleton({ w, h, rounded = 6 }: { w: number | string; h: number; rounded?: number }) {
  return <div className="skeleton" style={{ width: w, height: h, borderRadius: rounded }} />;
}

/** Table skeleton: header + N rows of shimmering bars. */
export function SkeletonTable({ rows = 6, cols = 4 }: { rows?: number; cols?: number }) {
  return (
    <div className="table-wrap skeleton-table">
      <div className="skeleton-row skeleton-head">
        {Array.from({ length: cols }).map((_, i) => (
          <Skeleton key={i} w="60%" h={10} />
        ))}
      </div>
      {Array.from({ length: rows }).map((_, r) => (
        <div className="skeleton-row" key={r}>
          {Array.from({ length: cols }).map((_, c) => (
            <Skeleton key={c} w={c === 0 ? "80%" : "55%"} h={10} />
          ))}
        </div>
      ))}
    </div>
  );
}

/** Cards skeleton (dashboard stats / workspace grid). */
export function SkeletonCards({ n = 3 }: { n?: number }) {
  return (
    <div className="workspace-grid">
      {Array.from({ length: n }).map((_, i) => (
        <div className="skeleton-card" key={i}>
          <Skeleton w="55%" h={14} />
          <Skeleton w="75%" h={10} />
        </div>
      ))}
    </div>
  );
}