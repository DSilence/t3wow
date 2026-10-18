//! Screenshots like the addon takes: the real Shot.lua encodes a Lua table,
//! the grid is drawn into a picture of the game, and the picture is saved as
//! a JPEG the way WoW would.

use std::fs;
use std::path::Path;

use mlua::{Lua, Table};

pub struct ShotEncoder {
    lua: Lua,
    shot: Table,
}

impl ShotEncoder {
    pub fn new() -> Self {
        let lua = Lua::new();
        let source = fs::read_to_string(super::addon_vm::addon_dir().join("Shot.lua")).unwrap();
        let ns = lua.create_table().unwrap();
        lua.load(source).set_name("@Shot.lua").call::<()>(("T3Wow", ns.clone())).unwrap();
        let shot: Table = ns.get("Shot").unwrap();
        Self { lua, shot }
    }

    /// JSON for a Lua table expression, as the addon builds it.
    pub fn json(&self, lua_table: &str) -> Vec<u8> {
        let value: mlua::Value = self.lua.load(format!("return {lua_table}")).eval().unwrap();
        let json: mlua::Function = self.shot.get("JSON").unwrap();
        json.call::<mlua::LuaString>(value).unwrap().as_bytes().to_vec()
    }

    /// Cell grids (rows of values 0..7) for each chunk of a payload.
    pub fn grids(&self, payload: &[u8], message_id: u32) -> Vec<Vec<Vec<u8>>> {
        let chunks: mlua::Function = self.shot.get("Chunks").unwrap();
        let layout: mlua::Function = self.shot.get("Layout").unwrap();
        let chunks: Table = chunks.call((self.lua.create_string(payload).unwrap(), message_id)).unwrap();
        chunks
            .sequence_values::<mlua::LuaString>()
            .map(|chunk| {
                let grid: Table = layout.call(chunk.unwrap()).unwrap();
                grid.sequence_values::<Table>()
                    .map(|row| row.unwrap().sequence_values::<u8>().map(|v| v.unwrap()).collect())
                    .collect()
            })
            .collect()
    }
}

/// Draw a grid over a busy game-like background, `cell` pixels per cell, as JPEG.
pub fn jpeg(grid: &[Vec<u8>], cell: f64, quality: u8) -> Vec<u8> {
    let (width, height) = (1280usize, 720usize);
    let mut rgb = vec![0u8; width * height * 3];
    for y in 0..height {
        for x in 0..width {
            let i = (y * width + x) * 3;
            rgb[i] = (x * 7 % 256) as u8;
            rgb[i + 1] = (y * 3 % 256) as u8;
            rgb[i + 2] = ((x + y) % 200) as u8;
        }
    }
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
    let mut out = Vec::new();
    let encoder = jpeg_encoder::Encoder::new(&mut out, quality);
    encoder.encode(&rgb, width as u16, height as u16, jpeg_encoder::ColorType::Rgb).unwrap();
    out
}

/// Save the screenshots for a payload into `dir`, named like WoW names them.
pub fn write_screenshots(dir: &Path, payload: &[u8], message_id: u32) {
    fs::create_dir_all(dir).unwrap();
    let encoder = ShotEncoder::new();
    for (index, grid) in encoder.grids(payload, message_id).iter().enumerate() {
        let file = dir.join(format!("WoWScrnShot_101726_1200{index:02}.jpg"));
        fs::write(file, jpeg(grid, 8.0, 90)).unwrap();
    }
}
