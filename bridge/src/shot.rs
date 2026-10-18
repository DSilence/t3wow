//! Commands the addon sends without a reload, as screenshots of a cell grid
//! (format in addon/T3Wow/Shot.lua). The bridge watches each flavor's
//! Screenshots folder, decodes new images, and deletes the ones that carried
//! data. The player's own screenshots are left alone.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use serde_json::Value;

use crate::log;

pub const MAGIC: &[u8; 4] = b"T3W1";
pub const GRID_WIDTH: usize = 96;
const HEADER: usize = 16;
/// Files younger than this may still be being written.
const SETTLE: Duration = Duration::from_millis(300);

pub struct Image {
    pub width: usize,
    pub height: usize,
    /// RGB, row by row from the top.
    pub rgb: Vec<u8>,
}

impl Image {
    fn pixel(&self, x: usize, y: usize) -> [u8; 3] {
        let i = (y * self.width + x) * 3;
        [self.rgb[i], self.rgb[i + 1], self.rgb[i + 2]]
    }

    fn luma(&self, x: usize, y: usize) -> u32 {
        let [r, g, b] = self.pixel(x, y);
        (r as u32 + g as u32 + b as u32) / 3
    }
}

pub fn decode_jpeg(bytes: &[u8]) -> Option<Image> {
    let mut decoder = jpeg_decoder::Decoder::new(bytes);
    let pixels = decoder.decode().ok()?;
    let info = decoder.info()?;
    if info.pixel_format != jpeg_decoder::PixelFormat::RGB24 {
        return None;
    }
    Some(Image { width: info.width as usize, height: info.height as usize, rgb: pixels })
}

/// Uncompressed or run-length encoded true-color TGA (what WoW writes).
pub fn decode_tga(bytes: &[u8]) -> Option<Image> {
    let header = bytes.get(..18)?;
    let id_length = header[0] as usize;
    let (color_map, kind) = (header[1], header[2]);
    let width = u16::from_le_bytes([header[12], header[13]]) as usize;
    let height = u16::from_le_bytes([header[14], header[15]]) as usize;
    let depth = header[16] as usize / 8;
    let top_down = header[17] & 0x20 != 0;
    if color_map != 0 || !(kind == 2 || kind == 10) || !(depth == 3 || depth == 4) || width == 0 || height == 0 {
        return None;
    }
    let mut data = &bytes[18 + id_length..];
    let mut pixels: Vec<u8> = Vec::with_capacity(width * height * 3);
    let push = |pixels: &mut Vec<u8>, p: &[u8]| pixels.extend_from_slice(&[p[2], p[1], p[0]]);
    while pixels.len() < width * height * 3 {
        if kind == 2 {
            push(&mut pixels, data.get(..depth)?);
            data = &data[depth..];
            continue;
        }
        let packet = *data.first()?;
        let count = (packet & 0x7f) as usize + 1;
        data = &data[1..];
        if packet & 0x80 != 0 {
            let p = data.get(..depth)?;
            for _ in 0..count {
                push(&mut pixels, p);
            }
            data = &data[depth..];
        } else {
            for _ in 0..count {
                push(&mut pixels, data.get(..depth)?);
                data = &data[depth..];
            }
        }
    }
    pixels.truncate(width * height * 3);
    if !top_down {
        let row = width * 3;
        let flipped: Vec<u8> = pixels.chunks(row).rev().flatten().copied().collect();
        pixels = flipped;
    }
    Some(Image { width, height, rgb: pixels })
}

#[derive(Debug, Clone, PartialEq)]
pub struct Chunk {
    pub id: u32,
    pub index: u8,
    pub count: u8,
    pub payload: Vec<u8>,
}

pub fn adler32(data: &[u8]) -> u32 {
    let (mut a, mut b) = (1u32, 0u32);
    for &byte in data {
        a = (a + byte as u32) % 65521;
        b = (b + a) % 65521;
    }
    (b << 16) | a
}

/// Read the grid in the top-left corner, if there is one.
pub fn read_chunk(image: &Image) -> Option<Chunk> {
    if image.height < 8 || image.width < GRID_WIDTH * 4 {
        return None;
    }
    // The clock row starts dark and flips every cell; its period is the cell size.
    let y = 2;
    if image.luma(0, y) > 96 {
        return None;
    }
    let mut flips = Vec::new();
    let mut bright = false;
    for x in 1..image.width {
        let now = image.luma(x, y) > 128;
        if now != bright {
            flips.push(x);
            bright = now;
            if flips.len() == 32 {
                break;
            }
        }
    }
    if flips.len() < 32 {
        return None;
    }
    let cell = flips[31] as f64 / 32.0;
    // Every flip has to be where the period says.
    if !(4.0..=64.0).contains(&cell)
        || flips.iter().enumerate().any(|(i, &x)| (x as f64 - (i + 1) as f64 * cell).abs() > cell / 3.0)
    {
        return None;
    }

    let mut reader = CellReader { image, cell, next: 0, bits: 0, count: 0 };
    let header = reader.bytes(HEADER)?;
    if &header[..4] != MAGIC {
        return None;
    }
    let id = u32::from_be_bytes(header[4..8].try_into().ok()?);
    let (index, count) = (header[8], header[9]);
    let length = u16::from_be_bytes([header[10], header[11]]) as usize;
    let checksum = u32::from_be_bytes(header[12..16].try_into().ok()?);
    let payload = reader.bytes(length)?;
    (index < count && adler32(&payload) == checksum).then_some(Chunk { id, index, count, payload })
}

struct CellReader<'a> {
    image: &'a Image,
    cell: f64,
    next: usize,
    bits: u32,
    count: u32,
}

impl CellReader<'_> {
    /// The 3 bits of the next data cell (rows after the clock row).
    fn cell_bits(&mut self) -> Option<u32> {
        let (col, row) = (self.next % GRID_WIDTH, 1 + self.next / GRID_WIDTH);
        self.next += 1;
        let cx = ((col as f64 + 0.5) * self.cell) as usize;
        let cy = ((row as f64 + 0.5) * self.cell) as usize;
        if cx + 1 >= self.image.width || cy + 1 >= self.image.height {
            return None;
        }
        let mut sum = [0u32; 3];
        for dy in 0..3 {
            for dx in 0..3 {
                let p = self.image.pixel(cx + dx - 1, cy + dy - 1);
                for c in 0..3 {
                    sum[c] += p[c] as u32;
                }
            }
        }
        let on = |c: usize| (sum[c] / 9 > 127) as u32;
        Some(on(0) << 2 | on(1) << 1 | on(2))
    }

    fn bytes(&mut self, n: usize) -> Option<Vec<u8>> {
        let mut out = Vec::with_capacity(n);
        while out.len() < n {
            while self.count < 8 {
                self.bits = (self.bits << 3) | self.cell_bits()?;
                self.count += 3;
            }
            self.count -= 8;
            out.push((self.bits >> self.count) as u8);
            self.bits &= (1 << self.count) - 1;
        }
        Some(out)
    }
}

pub fn read_file(path: &Path) -> Option<Chunk> {
    let bytes = fs::read(path).ok()?;
    let ext = path.extension()?.to_str()?.to_ascii_lowercase();
    let image = match ext.as_str() {
        "jpg" | "jpeg" => decode_jpeg(&bytes)?,
        "tga" => decode_tga(&bytes)?,
        _ => return None,
    };
    read_chunk(&image)
}

struct Partial {
    count: u8,
    parts: BTreeMap<u8, Vec<u8>>,
    files: Vec<PathBuf>,
}

/// Watches Screenshots folders for messages from the addon.
pub struct ShotReader {
    dirs: Vec<PathBuf>,
    since: SystemTime,
    seen: HashSet<PathBuf>,
    partial: HashMap<u32, Partial>,
}

impl ShotReader {
    pub fn new(dirs: Vec<PathBuf>) -> Self {
        // A little slack for a screenshot taken just before the bridge started.
        let since = SystemTime::now() - Duration::from_secs(30);
        Self { dirs, since, seen: HashSet::new(), partial: HashMap::new() }
    }

    pub fn dirs(&self) -> &[PathBuf] {
        &self.dirs
    }

    /// Messages completed since the last call, parsed from JSON.
    pub fn poll(&mut self) -> Vec<Value> {
        let mut messages = Vec::new();
        let now = SystemTime::now();
        let dirs = self.dirs.clone();
        for dir in &dirs {
            let Ok(entries) = fs::read_dir(dir) else { continue };
            for entry in entries.flatten() {
                let path = entry.path();
                if self.seen.contains(&path) {
                    continue;
                }
                let Ok(modified) = entry.metadata().and_then(|m| m.modified()) else { continue };
                if modified < self.since {
                    self.seen.insert(path);
                    continue;
                }
                if now.duration_since(modified).unwrap_or_default() < SETTLE {
                    continue;
                }
                self.seen.insert(path.clone());
                let Some(chunk) = read_file(&path) else { continue };
                if let Some(message) = self.add(chunk, path) {
                    messages.push(message);
                }
            }
        }
        messages
    }

    fn add(&mut self, chunk: Chunk, file: PathBuf) -> Option<Value> {
        let partial = self.partial.entry(chunk.id).or_insert_with(|| Partial {
            count: chunk.count,
            parts: BTreeMap::new(),
            files: Vec::new(),
        });
        partial.parts.insert(chunk.index, chunk.payload);
        partial.files.push(file);
        if partial.parts.len() < partial.count as usize {
            return None;
        }
        let partial = self.partial.remove(&chunk.id)?;
        for file in &partial.files {
            let _ = fs::remove_file(file);
        }
        let payload: Vec<u8> = partial.parts.into_values().flatten().collect();
        match serde_json::from_slice(&payload) {
            Ok(value) => Some(value),
            Err(error) => {
                log!("screenshot message is not valid JSON: {error}");
                None
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Draw cell values like FastSend.lua does, `cell` pixels each, from the top-left.
    pub fn render(grid: &[Vec<u8>], cell: f64, width: usize, height: usize) -> Image {
        let mut rgb = vec![90u8; width * height * 3];
        for (row, values) in grid.iter().enumerate() {
            for (col, &value) in values.iter().enumerate() {
                let (x0, x1) = ((col as f64 * cell) as usize, ((col + 1) as f64 * cell) as usize);
                let (y0, y1) = ((row as f64 * cell) as usize, ((row + 1) as f64 * cell) as usize);
                for y in y0..y1.min(height) {
                    for x in x0..x1.min(width) {
                        let i = (y * width + x) * 3;
                        rgb[i] = if value & 4 != 0 { 255 } else { 0 };
                        rgb[i + 1] = if value & 2 != 0 { 255 } else { 0 };
                        rgb[i + 2] = if value & 1 != 0 { 255 } else { 0 };
                    }
                }
            }
        }
        Image { width, height, rgb }
    }

    /// The same layout as Shot.lua, in Rust, for unit tests.
    fn grid(bytes: &[u8]) -> Vec<Vec<u8>> {
        let mut cells = Vec::new();
        let (mut value, mut used) = (0u8, 0);
        for &b in bytes {
            for shift in (0..8).rev() {
                value = value << 1 | (b >> shift) & 1;
                used += 1;
                if used == 3 {
                    cells.push(value);
                    (value, used) = (0, 0);
                }
            }
        }
        if used > 0 {
            cells.push(value << (3 - used));
        }
        let mut rows = vec![(0..GRID_WIDTH).map(|x| if x % 2 == 0 { 0 } else { 7 }).collect::<Vec<u8>>()];
        for chunk in cells.chunks(GRID_WIDTH) {
            let mut row = chunk.to_vec();
            row.resize(GRID_WIDTH, 0);
            rows.push(row);
        }
        rows
    }

    fn chunk_bytes(payload: &[u8]) -> Vec<u8> {
        let mut bytes = MAGIC.to_vec();
        bytes.extend_from_slice(&7u32.to_be_bytes());
        bytes.extend_from_slice(&[0, 1]);
        bytes.extend_from_slice(&(payload.len() as u16).to_be_bytes());
        bytes.extend_from_slice(&adler32(payload).to_be_bytes());
        bytes.extend_from_slice(payload);
        bytes
    }

    #[test]
    fn reads_a_grid_at_any_cell_size() {
        let payload = r#"{"instanceId":"i","outbox":[{"seq":3,"kind":"send","text":"héllo ✓"}]}"#.as_bytes();
        for cell in [8.0, 9.5, 12.0] {
            let image = render(&grid(&chunk_bytes(payload)), cell, 1280, 720);
            let chunk = read_chunk(&image).unwrap_or_else(|| panic!("cell {cell}"));
            assert_eq!((chunk.id, chunk.index, chunk.count), (7, 0, 1));
            assert_eq!(chunk.payload, payload);
        }
    }

    #[test]
    fn ignores_ordinary_screenshots() {
        let plain = Image { width: 1280, height: 720, rgb: vec![30; 1280 * 720 * 3] };
        assert!(read_chunk(&plain).is_none());
        // A damaged payload fails the checksum.
        let mut bytes = chunk_bytes(b"{\"a\":1}");
        let last = bytes.len() - 1;
        bytes[last] ^= 0x10;
        assert!(read_chunk(&render(&grid(&bytes), 8.0, 1280, 720)).is_none());
    }

    #[test]
    fn decodes_tga_both_ways_up() {
        // 2x2, bottom-up, uncompressed BGR.
        let mut tga = vec![0, 0, 2, 0, 0, 0, 0, 0, 0, 0, 0, 0, 2, 0, 2, 0, 24, 0];
        tga.extend_from_slice(&[1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12]);
        let image = decode_tga(&tga).unwrap();
        assert_eq!(image.pixel(0, 0), [9, 8, 7], "the last row in the file is the top");
        tga[17] = 0x20;
        assert_eq!(decode_tga(&tga).unwrap().pixel(0, 0), [3, 2, 1]);
    }
}
