import { describe, expect, it } from "vitest";
import { NavHistory } from "./navHistory";

const at = (key: string) => ({ key, target: key });
const always = () => true;

describe("NavHistory", () => {
  it("goes back and forward through visits", () => {
    const h = new NavHistory<string>();
    h.visit(at("a"));
    h.visit(at("b"));
    h.visit(at("c"));
    expect(h.step("back", always)?.key).toBe("b");
    expect(h.step("back", always)?.key).toBe("a");
    expect(h.canGoBack).toBe(false);
    expect(h.step("forward", always)?.key).toBe("b");
    // A new visit drops the forward branch.
    h.visit(at("d"));
    expect(h.canGoForward).toBe(false);
    expect(h.step("back", always)?.key).toBe("b");
  });

  it("ignores revisiting the place already shown", () => {
    const h = new NavHistory<string>();
    h.visit(at("a"));
    h.visit(at("a"));
    expect(h.canGoBack).toBe(false);
  });

  it("records jumps inside a file with the position left behind", () => {
    const h = new NavHistory<string>();
    h.visit(at("file"));
    h.setPosition("file", 5, 1);
    h.jump("file", "file", { line: 5, column: 1 }, { line: 120, column: 4 });
    const back = h.step("back", always);
    expect([back?.key, back?.line]).toEqual(["file", 5]);
    const fwd = h.step("forward", always);
    expect([fwd?.line, fwd?.column]).toEqual([120, 4]);
  });

  it("stays quiet while applying its own moves, and skips gone places", () => {
    const h = new NavHistory<string>();
    h.visit(at("a"));
    h.visit(at("closed"));
    h.visit(at("b"));
    h.quiet();
    h.jump("b", "b", { line: 1, column: 1 }, { line: 90, column: 1 });
    expect(h.step("back", (e) => e.key !== "closed")?.key).toBe("a");
  });
});
