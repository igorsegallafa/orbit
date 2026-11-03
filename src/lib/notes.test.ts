import { describe, expect, it } from "vitest";
import { Note, anchorNote } from "./notes";

const note = (start: number, end: number, snippet: string): Note => ({
  id: "x",
  done: false,
  repo: "api",
  path: "a.rs",
  start,
  end,
  body: "fix",
  snippet,
});

describe("anchorNote", () => {
  const text = ["fn a() {}", "", "fn b() {", "  todo!()", "}"].join("\n");

  it("stays put when the code is still there", () => {
    expect(anchorNote(note(3, 4, "fn b() {\n  todo!()"), text)).toEqual({ start: 3, end: 4, moved: false, lost: false });
  });

  it("follows the code when lines above were added", () => {
    const shifted = "// new\n// lines\n" + text;
    expect(anchorNote(note(3, 4, "fn b() {\n  todo!()"), shifted)).toEqual({ start: 5, end: 6, moved: true, lost: false });
  });

  it("picks the occurrence nearest the old line", () => {
    const twice = ["x", "dup", "y", "y", "y", "y", "dup"].join("\n");
    expect(anchorNote(note(6, 6, "dup"), twice).start).toBe(7);
    expect(anchorNote(note(3, 3, "dup"), twice).start).toBe(2);
  });

  it("flags notes whose code is gone, clamped to the file", () => {
    expect(anchorNote(note(9, 12, "gone()"), text)).toEqual({ start: 5, end: 5, moved: false, lost: true });
  });

  it("keeps notes without a snippet where they were", () => {
    expect(anchorNote(note(2, 2, ""), text)).toEqual({ start: 2, end: 2, moved: false, lost: false });
  });
});
