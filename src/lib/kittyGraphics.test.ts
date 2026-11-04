import { describe, expect, it } from "vitest";
import { KITTY_DIACRITICS } from "./kittyDiacritics";
import { fitInBox, parseCommand, PLACEHOLDER, readPlaceholder } from "./kittyGraphics";

const cell = (...marks: number[]) => String.fromCodePoint(PLACEHOLDER, ...marks.map((n) => KITTY_DIACRITICS[n]));

describe("parseCommand", () => {
  it("splits keys from the payload", () => {
    expect(parseCommand("a=T,U=1,i=31,c=99,r=29,t=f,f=100,q=2;L3RtcC94LnBuZw==")).toEqual({
      keys: { a: "T", U: "1", i: "31", c: "99", r: "29", t: "f", f: "100", q: "2" },
      payload: "L3RtcC94LnBuZw==",
    });
    expect(parseCommand("a=d,d=I,i=4")).toEqual({ keys: { a: "d", d: "I", i: "4" }, payload: "" });
  });
});

describe("readPlaceholder", () => {
  it("reads row and column from the diacritics and the id from the colour", () => {
    expect(readPlaceholder(cell(3, 7), 31, null)).toEqual({ id: 31, row: 3, col: 7 });
  });

  it("takes the id's high byte from a third diacritic", () => {
    expect(readPlaceholder(cell(0, 0, 2), 5, null)).toEqual({ id: (2 << 24) | 5, row: 0, col: 0 });
  });

  it("fills in what the diacritics leave out from the cell to the left", () => {
    const first = readPlaceholder(cell(2, 4), 9, null)!;
    const bare = readPlaceholder(cell(), 9, first)!;
    expect(bare).toEqual({ id: 9, row: 2, col: 5 });
    expect(readPlaceholder(cell(2), 9, bare)).toEqual({ id: 9, row: 2, col: 6 });
    // A different picture to the left starts again at its first cell.
    expect(readPlaceholder(cell(), 10, first)).toEqual({ id: 10, row: 0, col: 0 });
  });

  it("reads diacritics outside the basic plane", () => {
    const last = KITTY_DIACRITICS.length - 1;
    expect(readPlaceholder(cell(last, 1), 1, null)).toEqual({ id: 1, row: last, col: 1 });
  });

  it("ignores other text", () => {
    expect(readPlaceholder("a", 1, null)).toBeNull();
    expect(readPlaceholder("", 1, null)).toBeNull();
  });
});

describe("fitInBox", () => {
  it("fits keeping the shape and centres the rest", () => {
    expect(fitInBox(200, 100, 100, 100)).toEqual({ x: 0, y: 25, w: 100, h: 50 });
    expect(fitInBox(100, 100, 400, 200)).toEqual({ x: 100, y: 0, w: 200, h: 200 });
  });
});
