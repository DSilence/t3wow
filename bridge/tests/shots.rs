//! The screenshot channel end to end: Shot.lua encodes, JPEG compresses,
//! the bridge decodes.

mod support;

use t3wow::shot::{Chunk, decode_jpeg, read_chunk};

use support::shots::{ShotEncoder, jpeg};

fn decode(grid: &[Vec<u8>], cell: f64, quality: u8) -> Option<Chunk> {
    read_chunk(&decode_jpeg(&jpeg(grid, cell, quality))?)
}

#[test]
fn survives_jpeg_compression() {
    let encoder = ShotEncoder::new();
    let payload = encoder.json(
        r#"{ v = 1, instanceId = "6ad2-b09", focus = { threadId = "t1" },
             outbox = { { seq = 12, kind = "send", threadId = "t1", text = "Fix the \"login\" bug\n– please ✓", mode = "auto" } } }"#,
    );
    let value: serde_json::Value = serde_json::from_slice(&payload).unwrap();
    assert_eq!(value["outbox"][0]["text"], "Fix the \"login\" bug\n– please ✓");
    let grids = encoder.grids(&payload, 99);
    assert_eq!(grids.len(), 1);
    // WoW's top quality is 10/10; even much lower settings decode.
    for quality in [95, 75, 50] {
        for cell in [8.0, 8.5] {
            let chunk = decode(&grids[0], cell, quality).unwrap_or_else(|| panic!("quality {quality}, cell {cell}"));
            assert_eq!((chunk.id, chunk.index, chunk.count), (99, 0, 1));
            assert_eq!(chunk.payload, payload);
        }
    }
}

#[test]
fn splits_long_messages() {
    let encoder = ShotEncoder::new();
    let long = "word ".repeat(1000);
    let payload = encoder
        .json(&format!(r#"{{ instanceId = "i", outbox = {{ {{ seq = 1, kind = "send", text = "{long}" }} }} }}"#));
    let grids = encoder.grids(&payload, 5);
    assert_eq!(grids.len(), 3, "about 2.3 KB per screenshot");
    let mut joined = Vec::new();
    for (index, grid) in grids.iter().enumerate() {
        let chunk = decode(grid, 8.0, 90).unwrap();
        assert_eq!((chunk.index as usize, chunk.count), (index, 3));
        joined.extend(chunk.payload);
    }
    assert_eq!(joined, payload);
}
