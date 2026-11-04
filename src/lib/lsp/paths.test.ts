import { describe, expect, it } from "vitest";
import { mapUris, rebase, uriPath } from "./paths";

const toUri = (p: string) => "file://" + (p.startsWith("/") ? "" : "/") + p.replace(/\\/g, "/");

describe("uriPath", () => {
  it("decodes unix and windows file URIs", () => {
    expect(uriPath("file:///home/me/a%20b.cpp")).toBe("/home/me/a b.cpp");
    expect(uriPath("file:///c%3A/src/x.h")).toBe("c:/src/x.h");
    expect(uriPath("https://x")).toBeNull();
  });
});

describe("rebase", () => {
  it("moves paths under the root and leaves others alone", () => {
    expect(rebase("/real/engine/src/a.h", "/real/engine", "/ws/engine", false)).toBe("/ws/engine/src/a.h");
    expect(rebase("/real/engine2/a.h", "/real/engine", "/ws/engine", false)).toBeNull();
    expect(rebase("/usr/include/x.h", "/real/engine", "/ws/engine", false)).toBeNull();
  });

  it("ignores case and separators on windows", () => {
    expect(rebase("c:/Repos/Engine/src/a.h", "C:\\repos\\engine", "C:\\ws\\engine", true)).toBe("C:\\ws\\engine\\src\\a.h");
  });
});

describe("mapUris", () => {
  it("rewrites URIs anywhere in a payload and keeps untouched parts", () => {
    const tokens = [1, 2, 3];
    const msg = {
      result: [
        { targetUri: "file:///real/engine/src/Game.cpp", targetRange: {} },
        { uri: "file:///usr/include/stdio.h" },
      ],
      data: tokens,
    };
    const out = mapUris(msg, "/real/engine", "/ws/engine", false, toUri);
    expect(out.result[0].targetUri).toBe("file:///ws/engine/src/Game.cpp");
    expect(out.result[1]).toBe(msg.result[1]);
    expect(out.data).toBe(tokens);
  });

  it("returns the same object when nothing matches", () => {
    const msg = { uri: "file:///elsewhere/a.cpp", n: 1 };
    expect(mapUris(msg, "/real/engine", "/ws/engine", false, toUri)).toBe(msg);
  });
});
