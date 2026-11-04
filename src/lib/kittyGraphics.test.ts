import { describe, expect, it } from "vitest";
import { KITTY_DIACRITICS } from "./kittyDiacritics";
import { fitInBox, parseCommand, PLACEHOLDER, PlaceholderMask, readPlaceholder } from "./kittyGraphics";

const placeholder = (...marks: number[]) => String.fromCodePoint(PLACEHOLDER, ...marks.map((n) => KITTY_DIACRITICS[n]));
/** A placeholder as xterm.js stores it, after the mask. */
const masked = (s: string, mask = new PlaceholderMask()) => new TextDecoder().decode(mask.apply(new TextEncoder().encode(s)));
const cell = (...marks: number[]) => masked(placeholder(...marks) + "x").slice(0, -1);

describe("PlaceholderMask", () => {
  it("leaves no U+10EEEE or diacritic for the font to draw, and other text alone", () => {
    const out = masked(`a${placeholder(2, 296)}b 🙂`);
    expect(out.startsWith("a\u2800")).toBe(true);
    expect(out.endsWith("b 🙂")).toBe(true);
    expect([...out].every((ch) => ch.codePointAt(0)! < 0x300 || ch.codePointAt(0)! >= 0x2800)).toBe(true);
  });

  it("joins a placeholder and its diacritics split across chunks", () => {
    const bytes = new TextEncoder().encode(`x${placeholder(5, 7)}y`);
    const mask = new PlaceholderMask();
    const whole = masked(`x${placeholder(5, 7)}y`);
    const chunks = [1, 3, 5, 7, 8, 9, bytes.length].map((end, i, ends) => mask.apply(bytes.subarray(i ? ends[i - 1] : 0, end)));
    const joined = new Uint8Array(chunks.reduce((n, c) => n + c.length, 0));
    chunks.reduce((o, c) => (joined.set(c, o), o + c.length), 0);
    expect(new TextDecoder().decode(joined)).toBe(whole);
  });
});

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
    expect(readPlaceholder(cell(0), 10, first)).toEqual({ id: 10, row: 0, col: 0 });
  });

  it("reads diacritics outside the basic plane", () => {
    const last = KITTY_DIACRITICS.length - 1;
    expect(readPlaceholder(cell(last, 1), 1, null)).toEqual({ id: 1, row: last, col: 1 });
  });

  it("ignores other text, and a bare U+2800 that continues no picture", () => {
    expect(readPlaceholder("a", 1, null)).toBeNull();
    expect(readPlaceholder("", 1, null)).toBeNull();
    expect(readPlaceholder(cell(), 1, null)).toBeNull();
  });
});

describe("fitInBox", () => {
  it("fits keeping the shape and centres the rest", () => {
    expect(fitInBox(200, 100, 100, 100)).toEqual({ x: 0, y: 25, w: 100, h: 50 });
    expect(fitInBox(100, 100, 400, 200)).toEqual({ x: 100, y: 0, w: 200, h: 200 });
  });
});
