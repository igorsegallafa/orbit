// Compiler and linker messages in build output: GCC/Clang
// ("file:line:col: error: msg"), MSVC ("file(line,col): error C2065: msg")
// and GNU ld ("undefined reference to …").

export type Severity = "error" | "warning";

export interface Problem {
  /** Absolute path, or as printed when it can't be made absolute; null for
   *  messages without a location (linker errors). */
  file: string | null;
  line: number;
  column: number;
  severity: Severity;
  message: string;
  /** e.g. "C2065", "-Wunused-variable". */
  code?: string;
}

const GCC = /^(.*?):(\d+)(?::(\d+))?:\s+(fatal error|error|warning):\s+(.*)$/;
const MSVC = /^\s*(.*?)\((\d+)(?:,(\d+))?\)\s*:\s+(fatal error|error|warning)\s+([A-Z]+\d+)\s*:\s+(.*)$/;
const LD = /(?:^|\s)(?:\/\S*\/)?ld(?:\.\w+)?: (.*(?:undefined reference|multiple definition|cannot find).*)$/;
const LINK = /^\s*(?:LINK|\S+\.(?:obj|lib|exe|dll))\s*:\s+(fatal error|error|warning)\s+(LNK\d+)\s*:\s+(.*)$/;

function absolute(file: string, baseDir: string | null): string {
  if (!baseDir || /^([A-Za-z]:[\\/]|\/)/.test(file)) return file;
  const sep = baseDir.includes("\\") ? "\\" : "/";
  const parts = `${baseDir}${sep}${file}`.split(/[\\/]/);
  const out: string[] = [];
  for (const p of parts) {
    if (p === "..") out.pop();
    else if (p !== "." && (p !== "" || out.length === 0)) out.push(p);
  }
  return out.join(sep);
}

/** The problem a line reports, if any. Relative paths resolve against
 *  `baseDir` (the build folder: Ninja prints paths relative to it). */
export function parseProblem(raw: string, baseDir: string | null = null): Problem | null {
  // Strip colours (CMake passes them through from the compiler).
  const line = raw.replace(/\x1b\[[0-9;]*m/g, "");
  let m = MSVC.exec(line);
  if (m) {
    return {
      file: absolute(m[1].trim(), baseDir),
      line: Number(m[2]),
      column: Number(m[3] ?? 1),
      severity: m[4] === "warning" ? "warning" : "error",
      code: m[5],
      message: m[6].trim(),
    };
  }
  m = GCC.exec(line);
  if (m && !/^\s*(In file included from|from)\b/.test(m[1])) {
    const flag = /\s\[(-W[\w=+-]+)\]$/.exec(m[5]);
    return {
      file: absolute(m[1], baseDir),
      line: Number(m[2]),
      column: Number(m[3] ?? 1),
      severity: m[4] === "warning" ? "warning" : "error",
      code: flag?.[1],
      message: flag ? m[5].slice(0, flag.index) : m[5],
    };
  }
  m = LINK.exec(line);
  if (m) return { file: null, line: 0, column: 0, severity: m[1] === "warning" ? "warning" : "error", code: m[2], message: m[3].trim() };
  m = LD.exec(line);
  if (m) return { file: null, line: 0, column: 0, severity: "error", message: m[1].trim() };
  return null;
}

/** Identity for de-duplication: a header's warning repeats for every file
 *  that includes it. */
export const problemKey = (p: Problem) => `${p.file}:${p.line}:${p.column}:${p.severity}:${p.message}`;
