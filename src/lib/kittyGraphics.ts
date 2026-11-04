// kitty graphics with Unicode placeholders, painted on a canvas over the placeholder cells.
// https://sw.kovidgoyal.net/kitty/graphics-protocol/#unicode-placeholders
import type { IBufferCell, IDisposable, Terminal } from "@xterm/xterm";
import { KITTY_DIACRITICS } from "./kittyDiacritics";

export const PLACEHOLDER = 0x10eeee;
const PLACEHOLDER_TEXT = String.fromCodePoint(PLACEHOLDER);
const DIACRITIC_INDEX = new Map(KITTY_DIACRITICS.map((cp, i) => [cp, i]));
/** Decoded RGBA bytes kept; the oldest pictures are dropped past it. */
const STORAGE_LIMIT = 320 * 1024 * 1024;

export interface KittyCommand {
  keys: Record<string, string>;
  payload: string;
}

export function parseCommand(data: string): KittyCommand {
  const semi = data.indexOf(";");
  const control = semi < 0 ? data : data.slice(0, semi);
  const keys: Record<string, string> = {};
  for (const part of control.split(",")) {
    const eq = part.indexOf("=");
    if (eq > 0) keys[part.slice(0, eq)] = part.slice(eq + 1);
  }
  return { keys, payload: semi < 0 ? "" : data.slice(semi + 1) };
}

export interface PlaceholderCell {
  id: number;
  row: number;
  col: number;
}

/** Diacritics left out are taken from the cell to the left, as the protocol allows. */
export function readPlaceholder(chars: string, colourId: number, prev: PlaceholderCell | null): PlaceholderCell | null {
  if (chars.codePointAt(0) !== PLACEHOLDER) return null;
  const marks: number[] = [];
  for (const ch of chars.slice(2)) {
    const n = DIACRITIC_INDEX.get(ch.codePointAt(0)!);
    if (n === undefined) break;
    marks.push(n);
  }
  const sameLow = prev !== null && (prev.id & 0xffffff) === colourId;
  const id = marks.length > 2 ? colourId | (marks[2] << 24) : sameLow ? prev!.id : colourId;
  const row = marks.length > 0 ? marks[0] : sameLow ? prev!.row : 0;
  const col = marks.length > 1 ? marks[1] : sameLow && prev!.row === row ? prev!.col + 1 : 0;
  return { id, row, col };
}

export function fitInBox(width: number, height: number, boxWidth: number, boxHeight: number) {
  const scale = Math.min(boxWidth / width, boxHeight / height);
  const w = width * scale;
  const h = height * scale;
  return { x: (boxWidth - w) / 2, y: (boxHeight - h) / 2, w, h };
}

interface StoredImage {
  bitmap: ImageBitmap;
  bytes: number;
}

interface Placement {
  columns: number;
  rows: number;
}

export interface KittyIO {
  reply(data: string): void;
  readFile(path: string, offset: number, size: number, temp: boolean): Promise<ArrayBuffer>;
}

function base64Bytes(b64: string): Uint8Array {
  const bin = atob(b64);
  const out = new Uint8Array(bin.length);
  for (let i = 0; i < bin.length; i++) out[i] = bin.charCodeAt(i);
  return out;
}

async function inflate(bytes: Uint8Array): Promise<Uint8Array> {
  const stream = new Blob([bytes as BlobPart]).stream().pipeThrough(new DecompressionStream("deflate"));
  return new Uint8Array(await new Response(stream).arrayBuffer());
}

class ProtocolError extends Error {
  constructor(readonly code: string, message: string) {
    super(message);
  }
}

const num = (keys: Record<string, string>, k: string) => {
  const v = keys[k];
  return v === undefined ? 0 : Number.parseInt(v, 10) || 0;
};

export class KittyGraphics implements IDisposable {
  private images = new Map<number, StoredImage>();
  private placements = new Map<number, Placement>();
  private storedBytes = 0;
  private pending: KittyCommand | null = null;
  private nextId = 0x7f000000;
  private canvas: HTMLCanvasElement;
  private frame = 0;
  private painted = false;
  private disposables: IDisposable[] = [];
  private cell: IBufferCell | undefined;

  constructor(private term: Terminal, private io: KittyIO) {
    this.disposables.push(
      term.parser.registerApcHandler({ final: "G" }, (data) => this.onCommand(data)),
      term.onRender(() => this.schedule()),
      term.onScroll(() => this.schedule()),
      term.onResize(() => this.schedule()),
    );
    this.canvas = document.createElement("canvas");
    this.canvas.className = "kitty-graphics-layer";
    Object.assign(this.canvas.style, { position: "absolute", left: "0", top: "0", pointerEvents: "none", zIndex: "10" });
    term.element?.querySelector(".xterm-screen")?.appendChild(this.canvas);
  }

  dispose(): void {
    cancelAnimationFrame(this.frame);
    for (const d of this.disposables) d.dispose();
    for (const img of this.images.values()) img.bitmap.close();
    this.images.clear();
    this.canvas.remove();
  }

  schedule(): void {
    if (this.frame) return;
    this.frame = requestAnimationFrame(() => {
      this.frame = 0;
      this.paint();
    });
  }

  private async onCommand(data: string): Promise<boolean> {
    const cmd = parseCommand(data);
    // Chunked transfer: only the first chunk carries the keys.
    if (this.pending) {
      this.pending.payload += cmd.payload;
      if (cmd.keys.m === "1") return true;
      const whole = this.pending;
      this.pending = null;
      if (cmd.keys.q) whole.keys.q = cmd.keys.q;
      await this.run(whole);
      return true;
    }
    if (cmd.keys.m === "1") {
      this.pending = cmd;
      return true;
    }
    await this.run(cmd);
    return true;
  }

  private async run({ keys, payload }: KittyCommand): Promise<void> {
    const action = keys.a ?? "t";
    try {
      if (action === "q") {
        (await this.decode(keys, payload)).close();
        this.respond(keys, null);
      } else if (action === "t" || action === "T") {
        const bitmap = await this.decode(keys, payload);
        let id = num(keys, "i");
        if (!id && keys.I) id = this.nextId++;
        if (!id) {
          bitmap.close();
          return;
        }
        this.store(id, bitmap);
        if (action === "T" && keys.U === "1") this.place(id, keys);
        this.respond({ ...keys, i: String(id) }, null);
      } else if (action === "p") {
        const id = num(keys, "i");
        if (!this.images.has(id)) throw new ProtocolError("ENOENT", "no such image");
        if (keys.U === "1") this.place(id, keys);
        this.respond(keys, null);
      } else if (action === "d") {
        this.remove(keys);
      }
    } catch (e) {
      const err = e instanceof ProtocolError ? e : new ProtocolError("EINVAL", String((e as Error)?.message ?? e));
      this.respond(keys, err);
    }
  }

  private respond(keys: Record<string, string>, err: ProtocolError | null): void {
    const quiet = num(keys, "q");
    if ((!err && quiet >= 1) || (err && quiet >= 2)) return;
    if (!keys.i && !keys.I) return;
    const ids = [keys.i && `i=${keys.i}`, keys.I && `I=${keys.I}`, keys.p && `p=${keys.p}`].filter(Boolean).join(",");
    const msg = err ? `${err.code}:${err.message.replace(/[\x00-\x1f;\\]/g, " ")}` : "OK";
    this.io.reply(`\x1b_G${ids};${msg}\x1b\\`);
  }

  private async decode(keys: Record<string, string>, payload: string): Promise<ImageBitmap> {
    const medium = keys.t ?? "d";
    let bytes: Uint8Array;
    if (medium === "d") {
      bytes = base64Bytes(payload);
    } else if (medium === "f" || medium === "t") {
      const path = new TextDecoder().decode(base64Bytes(payload));
      try {
        bytes = new Uint8Array(await this.io.readFile(path, num(keys, "O"), num(keys, "S"), medium === "t"));
      } catch (e) {
        throw new ProtocolError("EBADF", String(e));
      }
    } else {
      throw new ProtocolError("EINVAL", `unsupported transmission medium ${medium}`);
    }
    if (keys.o === "z") bytes = await inflate(bytes);
    const format = num(keys, "f") || 32;
    if (format === 100) return createImageBitmap(new Blob([bytes as BlobPart], { type: "image/png" }));
    if (format !== 24 && format !== 32) throw new ProtocolError("EINVAL", `unsupported format ${format}`);
    const width = num(keys, "s");
    const height = num(keys, "v");
    const channels = format / 8;
    if (!width || !height || bytes.length < width * height * channels) throw new ProtocolError("ENODATA", "size does not match the data");
    const rgba = new Uint8ClampedArray(width * height * 4);
    if (channels === 4) rgba.set(bytes.subarray(0, rgba.length));
    else
      for (let s = 0, d = 0; d < rgba.length; s += 3, d += 4) {
        rgba[d] = bytes[s];
        rgba[d + 1] = bytes[s + 1];
        rgba[d + 2] = bytes[s + 2];
        rgba[d + 3] = 255;
      }
    return createImageBitmap(new ImageData(rgba, width, height));
  }

  private store(id: number, bitmap: ImageBitmap): void {
    this.drop(id);
    const bytes = bitmap.width * bitmap.height * 4;
    this.images.set(id, { bitmap, bytes });
    this.storedBytes += bytes;
    for (const old of this.images.keys()) {
      if (this.storedBytes <= STORAGE_LIMIT || old === id) break;
      this.drop(old);
      this.placements.delete(old);
    }
    this.schedule();
  }

  private drop(id: number): void {
    const img = this.images.get(id);
    if (!img) return;
    img.bitmap.close();
    this.storedBytes -= img.bytes;
    this.images.delete(id);
  }

  private place(id: number, keys: Record<string, string>): void {
    this.placements.set(id, { columns: num(keys, "c"), rows: num(keys, "r") });
    this.schedule();
  }

  private remove(keys: Record<string, string>): void {
    const what = keys.d ?? "a";
    const id = num(keys, "i");
    if (what === "a" || what === "A") {
      this.placements.clear();
      if (what === "A") for (const k of [...this.images.keys()]) this.drop(k);
    } else if ((what === "i" || what === "I") && id) {
      this.placements.delete(id);
      if (what === "I") this.drop(id);
    }
    this.schedule();
  }

  private paint(): void {
    const term = this.term;
    const screen = this.canvas.parentElement;
    if (!screen) return;
    const ctx = this.canvas.getContext("2d");
    if (!ctx) return;
    if (this.images.size === 0) {
      if (this.painted) ctx.clearRect(0, 0, this.canvas.width, this.canvas.height);
      this.painted = false;
      return;
    }
    const dpr = window.devicePixelRatio || 1;
    const cssW = screen.clientWidth;
    const cssH = screen.clientHeight;
    const w = Math.round(cssW * dpr);
    const h = Math.round(cssH * dpr);
    if (this.canvas.width !== w || this.canvas.height !== h) {
      this.canvas.width = w;
      this.canvas.height = h;
      this.canvas.style.width = `${cssW}px`;
      this.canvas.style.height = `${cssH}px`;
    } else {
      ctx.clearRect(0, 0, w, h);
    }
    this.painted = true;
    const cw = w / term.cols;
    const ch = h / term.rows;
    ctx.imageSmoothingQuality = "high";
    const background = term.options.theme?.background ?? "#000";
    const buffer = term.buffer.active;

    for (let y = 0; y < term.rows; y++) {
      const line = buffer.getLine(buffer.viewportY + y);
      if (!line || !line.translateToString(false).includes(PLACEHOLDER_TEXT)) continue;
      let prev: PlaceholderCell | null = null;
      let run: { x: number; count: number; at: PlaceholderCell } | null = null;
      const flush = () => {
        if (run) this.paintRun(ctx, run.at, run.x, y, run.count, cw, ch, background);
        run = null;
      };
      for (let x = 0; x < term.cols; x++) {
        const cell = (this.cell = line.getCell(x, this.cell));
        const chars = cell?.getChars() ?? "";
        const here: PlaceholderCell | null = cell ? readPlaceholder(chars, colourId(cell), prev) : null;
        if (!here) {
          // The placeholder's missing-glyph box spills into the next cell.
          if (run && (chars === "" || chars === " ") && cell?.isBgDefault()) this.coverCell(ctx, x, y, cw, ch, background);
          prev = null;
          flush();
          continue;
        }
        prev = here;
        const r = run as { x: number; count: number; at: PlaceholderCell } | null;
        if (r && r.at.id === here.id && r.at.row === here.row && r.at.col + r.count === here.col) r.count++;
        else {
          flush();
          run = { x, count: 1, at: here };
        }
      }
      flush();
    }
  }

  private coverCell(ctx: CanvasRenderingContext2D, x: number, y: number, cw: number, ch: number, background: string) {
    const x0 = Math.round(x * cw);
    const y0 = Math.round(y * ch);
    ctx.fillStyle = background;
    ctx.fillRect(x0, y0, Math.round((x + 1) * cw) - x0, Math.round((y + 1) * ch) - y0);
  }

  private paintRun(ctx: CanvasRenderingContext2D, at: PlaceholderCell, x: number, y: number, count: number, cw: number, ch: number, background: string) {
    const img = this.images.get(at.id);
    const placement = this.placements.get(at.id);
    if (!img || !placement) return;
    const { bitmap } = img;
    const columns = placement.columns || Math.ceil(bitmap.width / cw);
    const rows = placement.rows || Math.ceil(bitmap.height / ch);
    const fit = fitInBox(bitmap.width, bitmap.height, columns * cw, rows * ch);
    const x0 = Math.round(x * cw);
    const y0 = Math.round(y * ch);
    const x1 = Math.round((x + count) * cw);
    const y1 = Math.round((y + 1) * ch);
    ctx.save();
    ctx.beginPath();
    ctx.rect(x0, y0, x1 - x0, y1 - y0);
    ctx.clip();
    ctx.fillStyle = background;
    ctx.fillRect(x0, y0, x1 - x0, y1 - y0);
    ctx.drawImage(bitmap, x * cw - at.col * cw + fit.x, y * ch - at.row * ch + fit.y, fit.w, fit.h);
    ctx.restore();
  }
}

function colourId(cell: IBufferCell): number {
  return cell.isFgRGB() || cell.isFgPalette() ? cell.getFgColor() : 0;
}
