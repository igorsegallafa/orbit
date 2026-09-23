// Per-filetype icons for the file tree (VS Code style). Small filled/stroke
// glyphs tinted per language — enough to scan the tree visually without a
// full icon font.

function base(size: number, children: React.ReactNode) {
  return (
    <svg
      width={size}
      height={size}
      viewBox="0 0 24 24"
      fill="none"
      stroke="currentColor"
      strokeWidth="1.8"
      strokeLinecap="round"
      strokeLinejoin="round"
      className="tree-file-icon"
    >
      {children}
    </svg>
  );
}

const FILE_ICONS: Record<string, (size: number) => React.ReactNode> = {
  ts: (s) => base(s, <><path d="M4 4h16v14H4z" /><path d="m9 9-2 3 2 3" /><path d="m14 9 2 3-2 3" /></>),
  tsx: (s) => base(s, <><path d="M4 4h16v14H4z" /><path d="m9 9-2 3 2 3" /><path d="m14 9 2 3-2 3" /></>),
  js: (s) => base(s, <><path d="M4 4h16v14H4z" /><path d="M9 15c-.6.6-1.4.8-2.2.6" /><path d="M15 9.5c-1.8-.8-3.6.5-3.6 2.2 0 1.9 1.9 3 3.6 2.2" /></>),
  json: (s) => base(s, <><path d="M8 4H6a2 2 0 0 0-2 2v3a2 2 0 0 1-2 2 2 2 0 0 1 2 2v3a2 2 0 0 0 2 2h2" /><path d="M16 4h2a2 2 0 0 1 2 2v3a2 2 0 0 0 2 2 2 2 0 0 0-2 2v3a2 2 0 0 1-2 2h-2" /></>),
  md: (s) => base(s, <><path d="M4 4h16v14H4z" /><path d="M8 10l2 3 2-3" /><path d="M15 10v6" /></>),
  rs: (s) => base(s, <circle cx="12" cy="12" r="7" />),
  go: (s) => base(s, <><circle cx="12" cy="12" r="7" /><path d="M9 10h2.5l1 3 1-3H16" /></>),
  py: (s) => base(s, <><path d="M9 4h8v4H9z" /><path d="M7 8h10v12H7z" /><circle cx="13" cy="14" r="1.2" /></>),
  css: (s) => base(s, <><path d="M5 4h14l-1.5 14L12 20l-5.5-2z" /><path d="M9 8l.7 7" /><path d="m12.4 8-.4 7 3.4-1" /></>),
  html: (s) => base(s, <><path d="M5 4h14l-1.5 14L12 20l-5.5-2z" /><path d="M9 8l.5 7L12 16l2.5-1" /></>),
  yaml: (s) => base(s, <><path d="M4 4h16v14H4z" /><path d="M8 9v6" /><path d="M12 9v6" /><path d="M16 9v6" /></>),
  yml: (s) => base(s, <><path d="M4 4h16v14H4z" /><path d="M8 9v6" /><path d="M12 9v6" /><path d="M16 9v6" /></>),
  sql: (s) => base(s, <><ellipse cx="12" cy="6" rx="7" ry="2.5" /><path d="M5 6v12c0 1.4 3.1 2.5 7 2.5s7-1.1 7-2.5V6" /></>),
  lock: (s) => base(s, <><rect x="5" y="11" width="14" height="9" rx="2" /><path d="M8 11V7a4 4 0 0 1 8 0v4" /></>),
  sh: (s) => base(s, <><path d="m8 9-3 3 3 3" /><path d="M13 15h6" /></>),
  env: (s) => base(s, <><path d="M5 5h14v14H5z" /><path d="M9 10h.01M15 10h.01M9 14h.01M15 14h.01" /></>),
  toml: (s) => base(s, <><path d="M4 5h16v14H4z" /><path d="M4 9h16M9 5v14" /></>),
};

const ICON_COLORS: Record<string, string> = {
  ts: "#519aba",
  tsx: "#519aba",
  js: "#f1e05a",
  json: "#cbcb41",
  md: "#519aba",
  rs: "#dea584",
  go: "#00add8",
  py: "#3572a5",
  css: "#563d7c",
  html: "#e34c26",
  yaml: "#a074c4",
  yml: "#a074c4",
  sql: "#e38c00",
  lock: "#8b8b8b",
  sh: "#89e051",
  env: "#e0d050",
  toml: "#9c4221",
};

export function FileTypeIcon({ name, size = 14 }: { name: string; size?: number }) {
  const ext = name.split(".").pop()?.toLowerCase() ?? "";
  const icon = FILE_ICONS[ext];
  if (icon) {
    return (
      <span className="tree-file-icon-wrap" style={{ color: ICON_COLORS[ext] }}>
        {icon(size)}
      </span>
    );
  }
  return (
    <span className="tree-file-icon-wrap" style={{ color: "#8a919e" }}>
      {base(size, <><path d="M6 3h8l4 4v14H6z" /><path d="M14 3v4h4" /></>)}
    </span>
  );
}

export function FolderTreeIcon({ open, size = 14 }: { open: boolean; size?: number }) {
  return (
    <span className="tree-file-icon-wrap" style={{ color: "#dcb67a" }}>
      {open
        ? base(size, <path d="M3 6a1 1 0 0 1 1-1h4l2 2h10a1 1 0 0 1 1 1v2H3z" />)
        : base(size, <path d="M3 6a2 2 0 0 1 2-2h4l2 2h8a2 2 0 0 1 2 2v8a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2z" />)}
    </span>
  );
}