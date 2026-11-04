/** Subsequence fuzzy score: longer runs of consecutive matches and matches
 *  at word boundaries score higher. 0 = no match. Expects lower case. */
export function fuzzyScore(s: string, q: string): number {
  if (s === q) return 1000;
  let si = 0;
  let score = 0;
  let streak = 0;
  for (const ch of q) {
    if (si >= s.length) return 0;
    const idx = s.indexOf(ch, si);
    if (idx === -1) return 0;
    streak = idx === si ? streak + 1 : 0;
    score += 1 + streak * 2;
    if (idx === 0 || "/-._ ".includes(s[idx - 1] ?? "")) score += 3;
    si = idx + 1;
  }
  return score;
}

/** File paths ranked for a query: the file name counts more than its folders. */
export function rankPaths<T>(items: T[], path: (t: T) => string, query: string, limit: number): T[] {
  const q = query.trim().toLowerCase();
  if (!q) return items.slice(0, limit);
  return items
    .map((it) => {
      const p = path(it).toLowerCase();
      const name = p.slice(p.lastIndexOf("/") + 1);
      const s = fuzzyScore(p, q);
      return { it, score: s && s + fuzzyScore(name, q) * 2 };
    })
    .filter((r) => r.score > 0)
    .sort((a, b) => b.score - a.score)
    .slice(0, limit)
    .map((r) => r.it);
}
