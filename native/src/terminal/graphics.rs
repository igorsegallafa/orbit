//! kitty graphics protocol, as the React port (`src/lib/kittyGraphics.ts`) does it.
//! APC `G` commands are cut out of the PTY output before alacritty parses it.
//! Images show over Unicode placeholder cells (U+10EEEE, row/column diacritics,
//! image id in the foreground colour). Direct placements are written into the
//! grid as such cells, so they scroll, clear and leave the scrollback with the text.
use super::diacritics::DIACRITICS;
use alacritty_terminal::{
    event::EventListener,
    grid::Dimensions,
    index::{Column, Line},
    sync::FairMutex,
    term::{cell::Cell, Term, TermMode},
    vte::ansi::{Color, Processor, Rgb},
};
use base64::Engine;
use gpui::RenderImage;
use std::{
    collections::{HashMap, HashSet},
    io::{Read, Seek, SeekFrom},
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
};

pub const PLACEHOLDER: char = '\u{10EEEE}';
/// Decoded bytes kept per terminal; the oldest pictures are dropped past it (as in React).
const STORAGE_LIMIT: usize = 320 << 20;
/// Above this, placements whose cells left the grid are collected.
const GC_THRESHOLD: usize = 64 << 20;
/// One transmission's payload, chunks included.
const MAX_PAYLOAD: usize = 128 << 20;
const MAX_FILE: u64 = 128 << 20;
/// Larger pictures are scaled down to fit GPU textures.
const MAX_SIDE: u32 = 4096;
/// Ids for direct placements (high byte 0xFE), kept apart from program ids.
const ALIAS_BASE: u32 = 0xFE00_0000;

// ---- APC filter ----

pub enum Piece<'a> {
    Text(&'a [u8]),
    /// The body of `ESC _ G … ESC \`, without the `G`.
    Command(Vec<u8>),
}

#[derive(Default, Clone, Copy, PartialEq, Debug)]
enum State {
    #[default]
    Ground,
    Esc,
    Apc,
    Body,
    BodyEsc,
}

/// Streaming splitter: sequences may be cut anywhere between reads.
#[derive(Default)]
pub struct ApcFilter {
    state: State,
    buf: Vec<u8>,
    overflow: bool,
}

impl ApcFilter {
    pub fn feed<'a>(&mut self, input: &'a [u8], mut out: impl FnMut(Piece<'a>)) {
        let len = input.len();
        // Start of text not yet emitted, and where a possible sequence began
        // (None: in an earlier read, its bytes held back as `held`).
        let mut text = 0;
        let mut prefix: Option<usize> = None;
        let mut held: &'static [u8] = match self.state {
            State::Esc | State::BodyEsc => b"\x1b",
            State::Apc => b"\x1b_",
            _ => b"",
        };
        let mut i = 0;
        while i < len {
            match self.state {
                State::Ground => match memchr::memchr(0x1b, &input[i..]) {
                    None => i = len,
                    Some(p) => {
                        prefix = Some(i + p);
                        i += p + 1;
                        self.state = State::Esc;
                    }
                },
                State::Esc | State::Apc => {
                    let want = if self.state == State::Esc { b'_' } else { b'G' };
                    if input[i] != want {
                        // Not ours: the prefix goes to the parser as it was.
                        if prefix.is_none() && !held.is_empty() {
                            out(Piece::Text(held));
                        }
                        held = b"";
                        self.state = State::Ground;
                        continue;
                    }
                    i += 1;
                    if want == b'_' {
                        self.state = State::Apc;
                        continue;
                    }
                    let start = prefix.unwrap_or(text);
                    if start > text {
                        out(Piece::Text(&input[text..start]));
                    }
                    held = b"";
                    self.state = State::Body;
                    self.buf.clear();
                    self.overflow = false;
                    text = i;
                }
                State::Body => {
                    let end = memchr::memchr(0x1b, &input[i..]).map_or(len, |p| i + p);
                    self.push(&input[i..end]);
                    if end < len {
                        self.state = State::BodyEsc;
                        i = end + 1;
                    } else {
                        i = len;
                    }
                    text = i;
                }
                State::BodyEsc => {
                    if input[i] == b'\\' {
                        i += 1;
                        text = i;
                        self.state = State::Ground;
                        if !self.overflow {
                            out(Piece::Command(std::mem::take(&mut self.buf)));
                        }
                        self.buf = Vec::new();
                    } else {
                        // An ESC that isn't ST cancels the APC and starts a new sequence.
                        self.buf = Vec::new();
                        if i > 0 {
                            prefix = Some(i - 1);
                            text = i - 1;
                        } else {
                            prefix = None;
                            held = b"\x1b";
                        }
                        self.state = State::Esc;
                    }
                }
            }
        }
        let end = match self.state {
            State::Ground => len,
            State::Esc | State::Apc => prefix.unwrap_or(text),
            State::Body | State::BodyEsc => text,
        };
        if end > text {
            out(Piece::Text(&input[text..end]));
        }
    }

    fn push(&mut self, bytes: &[u8]) {
        if self.buf.len() + bytes.len() > MAX_PAYLOAD {
            self.overflow = true;
        }
        if !self.overflow {
            self.buf.extend_from_slice(bytes);
        }
    }
}

// ---- commands ----

#[derive(Default, Clone, Debug)]
pub struct Keys(Vec<(u8, String)>);

impl Keys {
    fn get(&self, k: u8) -> Option<&str> {
        self.0.iter().find(|(key, _)| *key == k).map(|(_, v)| v.as_str())
    }

    fn num(&self, k: u8) -> i64 {
        self.get(k).and_then(|v| v.parse().ok()).unwrap_or(0)
    }

    fn set(&mut self, k: u8, v: String) {
        match self.0.iter_mut().find(|(key, _)| *key == k) {
            Some(e) => e.1 = v,
            None => self.0.push((k, v)),
        }
    }
}

pub fn parse_command(data: &[u8]) -> (Keys, Vec<u8>) {
    let semi = data.iter().position(|&b| b == b';');
    let control = &data[..semi.unwrap_or(data.len())];
    let mut keys = Keys::default();
    for part in control.split(|&b| b == b',') {
        if let [k, b'=', v @ ..] = part {
            keys.0.push((*k, String::from_utf8_lossy(v).into_owned()));
        }
    }
    (keys, semi.map(|s| data[s + 1..].to_vec()).unwrap_or_default())
}

#[derive(Debug, PartialEq)]
struct Error(&'static str, String);

fn err(code: &'static str, msg: impl Into<String>) -> Error {
    Error(code, msg.into())
}

fn base64(data: &[u8]) -> Result<Vec<u8>, Error> {
    use base64::engine::{general_purpose::GeneralPurposeConfig, DecodePaddingMode, GeneralPurpose};
    const B64: GeneralPurpose = GeneralPurpose::new(
        &base64::alphabet::STANDARD,
        GeneralPurposeConfig::new().with_decode_padding_mode(DecodePaddingMode::Indifferent),
    );
    let clean: Vec<u8> = data.iter().copied().filter(|b| !b.is_ascii_whitespace()).collect();
    B64.decode(clean).map_err(|e| err("EINVAL", e.to_string()))
}

/// A decoded picture, BGRA as GPUI wants it.
pub struct Decoded {
    image: Arc<RenderImage>,
    width: u32,
    height: u32,
}

fn decode(keys: &Keys, payload: &[u8]) -> Result<Decoded, Error> {
    let mut bytes = match keys.get(b't').unwrap_or("d") {
        "d" => base64(payload)?,
        m @ ("f" | "t") => {
            let path = String::from_utf8(base64(payload)?).map_err(|_| err("EINVAL", "bad path"))?;
            let (o, s) = (keys.num(b'O').max(0) as u64, keys.num(b'S').max(0) as u64);
            read_image(Path::new(&path), o, s, m == "t").map_err(|e| err("EBADF", e))?
        }
        m => return Err(err("EINVAL", format!("unsupported transmission medium {m}"))),
    };
    if keys.get(b'o') == Some("z") {
        let mut out = Vec::new();
        flate2::read::ZlibDecoder::new(&bytes[..])
            .take(STORAGE_LIMIT as u64)
            .read_to_end(&mut out)
            .map_err(|e| err("EINVAL", e.to_string()))?;
        bytes = out;
    }
    let rgba = match keys.num(b'f') {
        100 => image::load_from_memory(&bytes).map_err(|e| err("EINVAL", e.to_string()))?.to_rgba8(),
        f @ (0 | 24 | 32) => {
            let channels = if f == 24 { 3 } else { 4 };
            let (w, h) = (keys.num(b's').max(0) as u32, keys.num(b'v').max(0) as u32);
            let need = w as usize * h as usize * channels;
            if w == 0 || h == 0 || bytes.len() < need || w as usize * h as usize * 4 > STORAGE_LIMIT {
                return Err(err("ENODATA", "size does not match the data"));
            }
            let data = if channels == 4 {
                bytes.truncate(need);
                bytes
            } else {
                bytes[..need].as_chunks::<3>().0.iter().flat_map(|p| [p[0], p[1], p[2], 255]).collect()
            };
            image::RgbaImage::from_raw(w, h, data).ok_or_else(|| err("ENODATA", "size does not match the data"))?
        }
        f => return Err(err("EINVAL", format!("unsupported format {f}"))),
    };
    let (width, height) = rgba.dimensions();
    let mut rgba = if width.max(height) > MAX_SIDE {
        let k = MAX_SIDE as f32 / width.max(height) as f32;
        let (w, h) = (((width as f32 * k) as u32).max(1), ((height as f32 * k) as u32).max(1));
        image::imageops::resize(&rgba, w, h, image::imageops::FilterType::Triangle)
    } else {
        rgba
    };
    for px in rgba.as_chunks_mut::<4>().0 {
        px.swap(0, 2);
    }
    let image = Arc::new(RenderImage::new(vec![image::Frame::new(rgba)]));
    Ok(Decoded { image, width, height })
}

/// A file named by `t=f` / `t=t`.
fn read_image(path: &Path, offset: u64, size: u64, temp: bool) -> Result<Vec<u8>, String> {
    if !path.is_absolute() {
        return Err("path must be absolute".into());
    }
    let real = path.canonicalize().map_err(|e| e.to_string())?;
    if ["/proc", "/sys", "/dev"].iter().any(|p| real.starts_with(p)) && !real.starts_with("/dev/shm") {
        return Err("not a regular file".into());
    }
    let meta = std::fs::metadata(&real).map_err(|e| e.to_string())?;
    if !meta.is_file() {
        return Err("not a regular file".into());
    }
    let len = if size > 0 { size } else { meta.len().saturating_sub(offset) };
    if len > MAX_FILE {
        return Err("file too big".into());
    }
    let mut file = std::fs::File::open(&real).map_err(|e| e.to_string())?;
    file.seek(SeekFrom::Start(offset)).map_err(|e| e.to_string())?;
    let mut bytes = Vec::with_capacity(len as usize);
    file.take(len).read_to_end(&mut bytes).map_err(|e| e.to_string())?;
    // Only files the protocol names as temporary, so a stray `t=t` can't delete anything else.
    if temp && is_graphics_temp_file(&real) {
        let _ = std::fs::remove_file(&real);
    }
    Ok(bytes)
}

fn is_graphics_temp_file(path: &Path) -> bool {
    let named = path.file_name().is_some_and(|n| n.to_string_lossy().contains("tty-graphics-protocol"));
    let dirs: Vec<PathBuf> = [std::env::temp_dir(), "/tmp".into(), "/dev/shm".into()]
        .into_iter()
        .filter_map(|d| d.canonicalize().ok())
        .collect();
    named && dirs.iter().any(|d| path.starts_with(d))
}

// ---- store ----

/// A picture, its size in pixels and its box in cells.
pub type Shown = (Arc<RenderImage>, (f32, f32), (u32, u32));
/// Writes a reply to the program.
pub type Reply<'a> = &'a dyn Fn(&[u8]);

struct Stored {
    image: Arc<RenderImage>,
    width: u32,
    height: u32,
    bytes: usize,
    seq: u64,
    /// Transmitted without an id for one direct placement.
    anonymous: bool,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Placement {
    pub image: u32,
    /// Cells; 0 means the picture's own size.
    pub cols: u32,
    pub rows: u32,
}

/// Pictures and placements of one terminal, shared with the renderer.
#[derive(Default)]
pub struct Store {
    images: HashMap<u32, Stored>,
    /// By placeholder id: an image id (virtual placements) or an alias (direct ones).
    placements: HashMap<u32, Placement>,
    bytes: usize,
    seq: u64,
    next_id: u32,
    next_alias: u32,
    /// Freed pictures the renderer still has to drop from the GPU atlas.
    pub retired: Vec<Arc<RenderImage>>,
}

impl Store {
    pub fn summary(&self) -> String {
        format!("{} images, {} placements, {} KB", self.images.len(), self.placements.len(), self.bytes >> 10)
    }

    /// The picture for a placeholder id, its pixel size, and its box in cells.
    pub fn lookup(&self, id: u32, cell: (f32, f32)) -> Option<Shown> {
        let p = self.placements.get(&id)?;
        let img = self.images.get(&p.image)?;
        let (w, h) = (img.width as f32, img.height as f32);
        let cols = if p.cols > 0 { p.cols } else { (w / cell.0).ceil() as u32 };
        let rows = if p.rows > 0 { p.rows } else { (h / cell.1).ceil() as u32 };
        Some((img.image.clone(), (w, h), (cols.max(1), rows.max(1))))
    }

    pub fn take_all(&mut self) -> Vec<Arc<RenderImage>> {
        let mut out = std::mem::take(&mut self.retired);
        out.extend(self.images.drain().map(|(_, s)| s.image));
        self.placements.clear();
        self.bytes = 0;
        out
    }

    fn store(&mut self, id: u32, d: &Decoded, anonymous: bool) {
        self.drop_image(id);
        let size = d.image.size(0);
        let bytes = size.width.0 as usize * size.height.0 as usize * 4;
        self.seq += 1;
        let stored = Stored { image: d.image.clone(), width: d.width, height: d.height, bytes, seq: self.seq, anonymous };
        self.images.insert(id, stored);
        self.bytes += bytes;
        while self.bytes > STORAGE_LIMIT {
            let Some(old) = self.images.iter().filter(|(k, _)| **k != id).min_by_key(|(_, s)| s.seq).map(|(k, _)| *k) else {
                break;
            };
            self.drop_image(old);
        }
    }

    fn drop_image(&mut self, id: u32) {
        if let Some(s) = self.images.remove(&id) {
            self.bytes -= s.bytes;
            self.retired.push(s.image);
            self.placements.retain(|_, p| p.image != id);
        }
    }

    fn alias(&mut self, p: Placement) -> u32 {
        // Direct placements reuse alias ids after 2^24 of them.
        let alias = ALIAS_BASE | (self.next_alias & 0xff_ffff);
        self.next_alias = self.next_alias.wrapping_add(1);
        self.placements.insert(alias, p);
        alias
    }

    fn remove(&mut self, keys: &Keys) {
        let id = keys.num(b'i') as u32;
        match keys.get(b'd').unwrap_or("a") {
            w @ ("a" | "A") => {
                self.placements.clear();
                let ids: Vec<u32> = self.images.iter().filter(|(_, s)| w == "A" || s.anonymous).map(|(k, _)| *k).collect();
                for k in ids {
                    self.drop_image(k);
                }
            }
            w @ ("i" | "I") if id != 0 => {
                self.placements.retain(|_, p| p.image != id);
                if w == "I" || self.images.get(&id).is_some_and(|s| s.anonymous) {
                    self.drop_image(id);
                }
            }
            _ => {}
        }
    }

    /// Drops direct placements whose cells are gone, and their anonymous pictures.
    fn collect(&mut self, live: &HashSet<u32>) {
        self.placements.retain(|k, _| *k < ALIAS_BASE || live.contains(k));
        let used: HashSet<u32> = self.placements.values().map(|p| p.image).collect();
        let dead: Vec<u32> = self.images.iter().filter(|(k, s)| s.anonymous && !used.contains(k)).map(|(k, _)| *k).collect();
        for k in dead {
            self.drop_image(k);
        }
    }
}

// ---- placeholders ----

fn diacritic_index(c: char) -> Option<u32> {
    DIACRITICS.binary_search(&c).ok().map(|i| i as u32)
}

/// The 24 low bits of a placeholder's id, from its foreground colour.
pub fn colour_id(fg: Color) -> u32 {
    match fg {
        Color::Spec(Rgb { r, g, b }) => ((r as u32) << 16) | ((g as u32) << 8) | b as u32,
        Color::Indexed(ix) => ix as u32,
        Color::Named(_) => 0,
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PlaceholderCell {
    pub id: u32,
    pub row: u32,
    pub col: u32,
}

/// Diacritics left out are taken from the cell to the left, as the protocol allows.
pub fn read_placeholder(zerowidth: &[char], colour: u32, prev: Option<PlaceholderCell>) -> PlaceholderCell {
    let marks: Vec<u32> = zerowidth.iter().map_while(|&c| diacritic_index(c)).take(3).collect();
    let same = prev.filter(|p| p.id & 0xff_ffff == colour);
    let id = match (marks.get(2), same) {
        (Some(hi), _) => colour | (hi << 24),
        (None, Some(p)) => p.id,
        (None, None) => colour,
    };
    let row = marks.first().copied().or(same.map(|p| p.row)).unwrap_or(0);
    let col = match (marks.get(1), same) {
        (Some(c), _) => *c,
        (None, Some(p)) if p.row == row => p.col + 1,
        _ => 0,
    };
    PlaceholderCell { id, row, col }
}

// ---- the output side ----

/// Per terminal, on the PTY output thread: cuts commands out of the output,
/// decodes pictures there (never on the UI thread) and places them.
#[derive(Default)]
pub struct Kitty {
    filter: ApcFilter,
    pending: Option<(Keys, Vec<u8>)>,
}

pub struct Target<'a, L: EventListener> {
    pub term: &'a FairMutex<Term<L>>,
    pub store: &'a Mutex<Store>,
    /// Cell size in pixels.
    pub cell: (f32, f32),
    /// None while replaying saved output.
    pub reply: Option<Reply<'a>>,
}

impl Kitty {
    pub fn feed<L: EventListener>(&mut self, bytes: &[u8], parser: &mut Processor, t: &Target<L>) {
        let pending = &mut self.pending;
        self.filter.feed(bytes, |piece| match piece {
            Piece::Text(text) => parser.advance(&mut *t.term.lock(), text),
            Piece::Command(cmd) => Self::command(pending, &cmd, parser, t),
        });
    }

    fn command<L: EventListener>(pending: &mut Option<(Keys, Vec<u8>)>, cmd: &[u8], parser: &mut Processor, t: &Target<L>) {
        let (keys, payload) = parse_command(cmd);
        let more = keys.get(b'm') == Some("1");
        // Chunked transfer: only the first chunk carries the keys.
        if let Some((_, data)) = pending.as_mut() {
            if data.len() + payload.len() <= MAX_PAYLOAD {
                data.extend_from_slice(&payload);
            }
            if more {
                return;
            }
            let (mut whole, data) = pending.take().unwrap();
            if let Some(q) = keys.get(b'q') {
                whole.set(b'q', q.to_string());
            }
            return run(whole, &data, parser, t);
        }
        if more {
            *pending = Some((keys, payload));
            return;
        }
        run(keys, &payload, parser, t);
    }
}

fn run<L: EventListener>(mut keys: Keys, payload: &[u8], parser: &mut Processor, t: &Target<L>) {
    let action = keys.get(b'a').unwrap_or("t").to_string();
    let virtual_ = keys.get(b'U') == Some("1");
    let result = match action.as_str() {
        "q" => decode(&keys, payload).map(|_| true),
        "t" | "T" => decode(&keys, payload).map(|d| {
            let mut store = t.store.lock().unwrap();
            let mut id = keys.num(b'i') as u32;
            if id == 0 && keys.get(b'I').is_some() {
                store.next_id = store.next_id.max(0x7f00_0000) + 1;
                id = store.next_id;
            }
            let anonymous = id == 0;
            if anonymous {
                if action != "T" || virtual_ {
                    return false;
                }
                store.next_id = store.next_id.max(0x7f00_0000) + 1;
                id = store.next_id;
            }
            if store.bytes > GC_THRESHOLD {
                drop(store);
                collect(t);
                store = t.store.lock().unwrap();
            }
            store.store(id, &d, anonymous);
            drop(store);
            if action == "T" {
                place(id, &keys, parser, t);
            }
            keys.set(b'i', id.to_string());
            !anonymous
        }),
        "p" => {
            let id = keys.num(b'i') as u32;
            if t.store.lock().unwrap().images.contains_key(&id) {
                place(id, &keys, parser, t);
                Ok(true)
            } else {
                Err(err("ENOENT", "no such image"))
            }
        }
        "d" => {
            t.store.lock().unwrap().remove(&keys);
            Ok(false)
        }
        _ => Ok(false),
    };
    match result {
        Ok(true) => respond(&keys, None, t),
        Ok(false) => {}
        Err(e) => respond(&keys, Some(e), t),
    }
}

fn respond<L: EventListener>(keys: &Keys, error: Option<Error>, t: &Target<L>) {
    let Some(reply) = t.reply else { return };
    let quiet = keys.num(b'q');
    if (error.is_none() && quiet >= 1) || (error.is_some() && quiet >= 2) {
        return;
    }
    if keys.get(b'i').is_none() && keys.get(b'I').is_none() {
        return;
    }
    let ids: Vec<String> = b"iIp"
        .iter()
        .filter_map(|&k| keys.get(k).map(|v| format!("{}={v}", k as char)))
        .collect();
    let msg = match error {
        None => "OK".to_string(),
        Some(Error(code, m)) => format!("{code}:{}", m.replace(|c: char| c.is_control() || c == ';' || c == '\\', " ")),
    };
    reply(format!("\x1b_G{};{msg}\x1b\\", ids.join(",")).as_bytes());
}

fn place<L: EventListener>(id: u32, keys: &Keys, parser: &mut Processor, t: &Target<L>) {
    let (c, r) = (keys.num(b'c').max(0) as u32, keys.num(b'r').max(0) as u32);
    if keys.get(b'U') == Some("1") {
        t.store.lock().unwrap().placements.insert(id, Placement { image: id, cols: c, rows: r });
        return;
    }
    // Direct placement: the picture's box in cells, at the cursor.
    let (cw, ch) = t.cell;
    let alias = {
        let mut store = t.store.lock().unwrap();
        let Some(img) = store.images.get(&id) else { return };
        let (w, h) = (img.width.max(1) as f32, img.height.max(1) as f32);
        let (cols, rows) = match (c, r) {
            (0, 0) => ((w / cw).ceil() as u32, (h / ch).ceil() as u32),
            (c, 0) => (c, (c as f32 * cw * h / w / ch).ceil() as u32),
            (0, r) => ((r as f32 * ch * w / h / cw).ceil() as u32, r),
            (c, r) => (c, r),
        };
        let cols = cols.clamp(1, DIACRITICS.len() as u32);
        let rows = rows.clamp(1, DIACRITICS.len() as u32);
        let alias = store.alias(Placement { image: id, cols, rows });
        (alias, cols, rows)
    };
    let (alias, cols, rows) = alias;
    let keep_cursor = keys.get(b'C') == Some("1");
    let mut term = t.term.lock();
    let saved = term.grid().cursor.clone();
    let start = saved.point.column.0;
    let shown_cols = (cols as usize).min(term.columns().saturating_sub(start)).max(1);
    let mut shown_rows = rows as usize;
    if keep_cursor {
        // Nothing scrolls: rows below the screen are cut off.
        shown_rows = shown_rows.min((term.screen_lines() as i32 - saved.point.line.0).max(1) as usize);
    }
    let hi = DIACRITICS[(alias >> 24) as usize];
    let mut out = String::with_capacity(shown_rows * (shown_cols * 10 + 8));
    for (row, &row_mark) in DIACRITICS.iter().take(shown_rows).enumerate() {
        if row > 0 {
            out.push_str("\r\n");
            if start > 0 {
                out.push_str(&format!("\x1b[{start}C"));
            }
        }
        for &col_mark in DIACRITICS.iter().take(shown_cols) {
            out.extend([PLACEHOLDER, row_mark, col_mark, hi]);
        }
    }
    let rgb = Rgb { r: (alias >> 16) as u8, g: (alias >> 8) as u8, b: alias as u8 };
    term.grid_mut().cursor.template = Cell { fg: Color::Spec(rgb), ..Cell::default() };
    parser.advance(&mut *term, out.as_bytes());
    if keep_cursor {
        term.grid_mut().cursor = saved;
    } else {
        term.grid_mut().cursor.template = saved.template;
    }
}

/// Placeholder ids still in the grid (primary screen only: the other grid is out of reach).
fn collect<L: EventListener>(t: &Target<L>) {
    let term = t.term.lock();
    if term.mode().contains(TermMode::ALT_SCREEN) {
        return;
    }
    let grid = term.grid();
    let mut live = HashSet::new();
    let top = -(grid.history_size() as i32);
    for line in top..grid.screen_lines() as i32 {
        let row = &grid[Line(line)];
        for col in 0..grid.columns() {
            let cell = &row[Column(col)];
            if cell.c == PLACEHOLDER {
                let hi = cell.zerowidth().and_then(|z| z.get(2)).and_then(|&c| diacritic_index(c)).unwrap_or(0);
                live.insert(colour_id(cell.fg) | (hi << 24));
            }
        }
    }
    t.store.lock().unwrap().collect(&live);
}

#[cfg(test)]
mod tests {
    use super::*;
    use alacritty_terminal::{event::VoidListener, term::Config};

    fn split(filter: &mut ApcFilter, reads: &[&[u8]]) -> (Vec<u8>, Vec<Vec<u8>>) {
        let (mut text, mut cmds) = (Vec::new(), Vec::new());
        for r in reads {
            filter.feed(r, |p| match p {
                Piece::Text(t) => text.extend_from_slice(t),
                Piece::Command(c) => cmds.push(c),
            });
        }
        (text, cmds)
    }

    #[test]
    fn filter_cuts_commands_at_any_split() {
        let input: &[u8] = b"ab\x1b[1mc\x1b_Ga=T,f=100;QUJD\x1b\\de\x1b_Xother\x1b\\f\x1b\x1b_Gi=2;\x1b\\g";
        let want_text: &[u8] = b"ab\x1b[1mcde\x1b_Xother\x1b\\f\x1bg";
        let want_cmds = vec![b"a=T,f=100;QUJD".to_vec(), b"i=2;".to_vec()];
        let (text, cmds) = split(&mut ApcFilter::default(), &[input]);
        assert_eq!((text.as_slice(), &cmds), (want_text, &want_cmds));
        // Every split into two and byte by byte.
        for at in 0..=input.len() {
            let (text, cmds) = split(&mut ApcFilter::default(), &[&input[..at], &input[at..]]);
            assert_eq!(text, want_text, "split at {at}");
            assert_eq!(cmds, want_cmds, "split at {at}");
        }
        let bytes: Vec<&[u8]> = input.chunks(1).collect();
        assert_eq!(split(&mut ApcFilter::default(), &bytes), (want_text.to_vec(), want_cmds));
    }

    #[test]
    fn esc_inside_apc_cancels_it() {
        let (text, cmds) = split(&mut ApcFilter::default(), &[b"\x1b_Gabc", b"\x1b[0mx"]);
        assert_eq!(text, b"\x1b[0mx");
        assert!(cmds.is_empty());
    }

    fn png(w: u32, h: u32) -> Vec<u8> {
        let img = image::RgbaImage::from_pixel(w, h, image::Rgba([255, 0, 0, 255]));
        let mut out = std::io::Cursor::new(Vec::new());
        img.write_to(&mut out, image::ImageFormat::Png).unwrap();
        out.into_inner()
    }

    struct Rig {
        term: FairMutex<Term<VoidListener>>,
        store: Mutex<Store>,
        replies: Mutex<Vec<String>>,
        kitty: Kitty,
        parser: Processor,
    }

    impl Rig {
        fn new() -> Self {
            let size = super::super::GridSize { cols: 40, rows: 10 };
            Rig {
                term: FairMutex::new(Term::new(Config::default(), &size, VoidListener)),
                store: Mutex::default(),
                replies: Mutex::default(),
                kitty: Kitty::default(),
                parser: Processor::new(),
            }
        }

        fn feed(&mut self, reads: &[&[u8]]) {
            let replies = &self.replies;
            let reply = |b: &[u8]| replies.lock().unwrap().push(String::from_utf8_lossy(b).into_owned());
            let t = Target { term: &self.term, store: &self.store, cell: (10., 20.), reply: Some(&reply) };
            for r in reads {
                self.kitty.feed(r, &mut self.parser, &t);
            }
        }
    }

    fn b64(bytes: &[u8]) -> String {
        base64::engine::general_purpose::STANDARD.encode(bytes)
    }

    #[test]
    fn chunked_transfer_split_across_reads_with_query_reply() {
        let data = b64(&png(30, 40));
        let (a, b) = data.split_at(data.len() / 2 / 4 * 4);
        let stream = format!("hi\x1b_Ga=q,i=7,f=100;{data}\x1b\\\x1b_Ga=T,i=3,f=100,m=1;{a}\x1b\\\x1b_Gm=0;{b}\x1b\\");
        let bytes = stream.as_bytes();
        let mut rig = Rig::new();
        let cut = bytes.len() - b.len() - 3;
        rig.feed(&[&bytes[..5], &bytes[5..cut], &bytes[cut..]]);
        assert_eq!(*rig.replies.lock().unwrap(), vec!["\x1b_Gi=7;OK\x1b\\", "\x1b_Gi=3;OK\x1b\\"]);
        let store = rig.store.lock().unwrap();
        assert_eq!(store.images.len(), 1, "a query stores nothing");
        // 30x40 px at 10x20 px cells: 3 columns, 2 rows after "hi", cursor past it.
        let term = rig.term.lock();
        let grid = term.grid();
        assert_eq!(grid[Line(0)][Column(0)].c, 'h');
        let mut prev = None;
        for (line, col) in [(0, 2), (0, 3), (0, 4), (1, 2), (1, 4)] {
            let cell = &grid[Line(line)][Column(col)];
            assert_eq!(cell.c, PLACEHOLDER);
            let p = read_placeholder(cell.zerowidth().unwrap(), colour_id(cell.fg), prev);
            assert_eq!((p.row, p.col), (line as u32, col as u32 - 2));
            assert_eq!(store.lookup(p.id, (10., 20.)).map(|l| l.2), Some((3, 2)));
            prev = Some(p);
        }
        assert_eq!(grid[Line(1)][Column(5)].c, ' ');
        assert_eq!(grid.cursor.point, alacritty_terminal::index::Point::new(Line(1), Column(5)));
        assert_eq!(grid.cursor.template, Cell::default());
    }

    #[test]
    fn errors_formats_placement_and_delete() {
        let mut rig = Rig::new();
        // Raw RGB 2x1, then a virtual placement with a size in cells.
        let rgb = b64(&[1, 2, 3, 4, 5, 6]);
        let seq = format!(
            "\x1b_Ga=t,i=5,f=24,s=2,v=1;{rgb}\x1b\\\x1b_Ga=p,i=5,U=1,c=4,r=2\x1b\\\x1b_Ga=p,i=9\x1b\\\x1b_Ga=t,i=6,f=24,s=9,v=9;{rgb}\x1b\\"
        );
        rig.feed(&[seq.as_bytes()]);
        let replies = rig.replies.lock().unwrap().clone();
        assert_eq!(replies[0], "\x1b_Gi=5;OK\x1b\\");
        assert_eq!(replies[1], "\x1b_Gi=5;OK\x1b\\");
        assert!(replies[2].starts_with("\x1b_Gi=9;ENOENT:"));
        assert!(replies[3].starts_with("\x1b_Gi=6;ENODATA:"));
        assert_eq!(rig.store.lock().unwrap().lookup(5, (10., 20.)).map(|l| (l.1, l.2)), Some(((2., 1.), (4, 2))));
        // The program's own placeholder cells (id 5 in a 256-colour fg).
        rig.feed(&["\x1b[38;5;5m\u{10EEEE}\u{305}\u{305}\u{10EEEE}\x1b[0m".as_bytes()]);
        {
            let term = rig.term.lock();
            let cell = &term.grid()[Line(0)][Column(0)];
            assert_eq!((colour_id(cell.fg), cell.zerowidth().map(|z| z.len())), (5, Some(2)));
            let first = read_placeholder(cell.zerowidth().unwrap(), 5, None);
            let next = &term.grid()[Line(0)][Column(1)];
            assert_eq!(read_placeholder(next.zerowidth().unwrap_or(&[]), colour_id(next.fg), Some(first)).col, 1);
        }
        rig.feed(&[b"\x1b_Ga=d,d=i,i=5\x1b\\"]);
        assert!(rig.store.lock().unwrap().lookup(5, (10., 20.)).is_none());
        assert_eq!(rig.store.lock().unwrap().images.len(), 1, "d=i keeps the data");
        rig.feed(&[b"\x1b_Ga=d,d=A\x1b\\"]);
        assert!(rig.store.lock().unwrap().images.is_empty());
    }

    #[test]
    fn keep_cursor_and_collect() {
        let mut rig = Rig::new();
        let data = b64(&png(10, 20));
        rig.feed(&[format!("\x1b_Ga=T,f=100,C=1,c=2,r=1;{data}\x1b\\").as_bytes()]);
        assert!(rig.replies.lock().unwrap().is_empty(), "no id, no reply");
        assert_eq!(rig.term.lock().grid().cursor.point.column, Column(0));
        assert_eq!(rig.store.lock().unwrap().placements.len(), 1);
        rig.feed(&[b"\x1b[H\x1b[2J\x1b[3J"]);
        collect(&Target { term: &rig.term, store: &rig.store, cell: (10., 20.), reply: None });
        let store = rig.store.lock().unwrap();
        assert!(store.placements.is_empty() && store.images.is_empty());
        assert_eq!(store.retired.len(), 1);
    }

    #[test]
    fn temp_files_are_read_and_removed() {
        let dir = std::env::temp_dir();
        let marked = dir.join(format!("orbit-native-tty-graphics-protocol-{}", std::process::id()));
        std::fs::write(&marked, png(4, 4)).unwrap();
        let keys = parse_command(format!("t=t,f=100;{}", b64(marked.to_str().unwrap().as_bytes())).as_bytes());
        let d = decode(&keys.0, &keys.1).ok().unwrap();
        assert_eq!((d.width, d.height), (4, 4));
        assert!(!marked.exists());
        assert!(read_image(Path::new("relative.png"), 0, 0, false).is_err());
    }
}
