import { describe, expect, it } from "vitest";
import { placeFileTab, TabOps } from "./previewTabs";

type T = { id: string; preview?: boolean };
const ops: TabOps<T> = { id: (t) => t.id, isPreview: (t) => !!t.preview, pin: (t) => ({ id: t.id }) };

describe("placeFileTab", () => {
  it("replaces the preview tab in place", () => {
    const tabs: T[] = [{ id: "agent" }, { id: "a.cpp", preview: true }, { id: "b.cpp" }];
    expect(placeFileTab(tabs, { id: "c.cpp", preview: true }, true, ops)).toEqual([{ id: "agent" }, { id: "c.cpp", preview: true }, { id: "b.cpp" }]);
  });

  it("appends when there is no preview tab or the open is regular", () => {
    const tabs: T[] = [{ id: "a.cpp", preview: true }];
    expect(placeFileTab(tabs, { id: "b.cpp" }, false, ops)).toEqual([{ id: "a.cpp", preview: true }, { id: "b.cpp" }]);
    expect(placeFileTab([{ id: "a.cpp" }], { id: "b.cpp", preview: true }, true, ops)).toHaveLength(2);
  });

  it("keeps an open preview tab when opened for real, and leaves it otherwise", () => {
    const tabs: T[] = [{ id: "a.cpp", preview: true }];
    expect(placeFileTab(tabs, { id: "a.cpp" }, false, ops)).toEqual([{ id: "a.cpp" }]);
    expect(placeFileTab(tabs, { id: "a.cpp", preview: true }, true, ops)).toBe(tabs);
  });
});
