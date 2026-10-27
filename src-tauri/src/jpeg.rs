//! JPEG on all cores, for the pictures the viewer is sent (a frame every slider move): the picture
//! cut into strips of whole MCU rows, each encoded by `jpeg-encoder` with the same tables, then
//! joined into one ordinary baseline JPEG with restart markers between the strips (each strip starts
//! its DC prediction afresh, as a restart does), so any decoder reads it.
//!
//! On the Mac Studio (M1 Max, 25 Oct): a 1600 px frame 20.7 -> 2.3 ms, 2400 px 45 -> 5.3 ms, 5120 px
//! 205 -> 25 ms against the image crate's encoder on one core, and about a quarter smaller
//! (`examples/frame_cost.rs`).

use rayon::prelude::*;

/// RGB (3 bytes a pixel, `w` x `h`) as a JPEG at `quality` (1..100; below 90 the colour is stored
/// at half size each way, 4:2:0, as `jpeg-encoder` does by default).
pub fn encode(px: &[u8], w: u32, h: u32, quality: u8) -> Result<Vec<u8>, String> {
    if w == 0 || h == 0 || w > 65535 || h > 65535 {
        return Err(format!("can't encode a {w}x{h} JPEG"));
    }
    let mcu = if quality < 90 { 16 } else { 8 };
    let across = w.div_ceil(mcu);
    let mcu_rows = h.div_ceil(mcu);
    // Twice as many strips as cores (they don't all take the same time), each a whole number of MCU
    // rows, and no more MCUs in a strip than a restart interval can count.
    let threads = rayon::current_num_threads().max(1) as u32;
    let rows = mcu_rows.div_ceil(threads * 2).max(1).min((65535 / across).max(1));
    let strip_h = rows * mcu;
    let strips = h.div_ceil(strip_h);
    // Small pictures (thumbnails) in one piece: the strips' own headers would cost more than they save.
    if strips <= 1 || (w as u64) * (h as u64) < 1 << 18 {
        return strip(px, w, 0, h, quality);
    }
    let parts: Vec<Vec<u8>> = (0..strips).into_par_iter().map(|k| strip(px, w, k * strip_h, (h - k * strip_h).min(strip_h), quality)).collect::<Result<_, _>>()?;
    let (sof, sos, data) = layout(&parts[0])?;
    let first = &parts[0];
    let mut out = Vec::with_capacity(parts.iter().map(|p| p.len()).sum::<usize>() + 64);
    out.extend_from_slice(&first[..sos]);
    // The whole picture's height in the frame header.
    out[sof + 5..sof + 7].copy_from_slice(&(h as u16).to_be_bytes());
    // A restart every strip's worth of MCUs.
    let interval = (across * rows) as u16;
    out.extend_from_slice(&[0xFF, 0xDD, 0x00, 0x04]);
    out.extend_from_slice(&interval.to_be_bytes());
    out.extend_from_slice(&first[sos..data]);
    for (k, p) in parts.iter().enumerate() {
        let (_, _, at) = layout(p)?;
        if p.len() < at + 2 || p[p.len() - 2..] != [0xFF, 0xD9] {
            return Err("a JPEG strip didn't end as expected".into());
        }
        out.extend_from_slice(&p[at..p.len() - 2]);
        if k + 1 < parts.len() {
            out.extend_from_slice(&[0xFF, 0xD0 + (k % 8) as u8]);
        }
    }
    out.extend_from_slice(&[0xFF, 0xD9]);
    Ok(out)
}

/// Rows `y0 .. y0 + sh` as a JPEG of their own.
fn strip(px: &[u8], w: u32, y0: u32, sh: u32, quality: u8) -> Result<Vec<u8>, String> {
    let row = w as usize * 3;
    let part = &px[y0 as usize * row..(y0 + sh) as usize * row];
    let mut out = Vec::with_capacity(part.len() / 8);
    jpeg_encoder::Encoder::new(&mut out, quality).encode(part, w as u16, sh as u16, jpeg_encoder::ColorType::Rgb).map_err(|e| e.to_string())?;
    Ok(out)
}

/// Where a JPEG's frame header (SOF0) and scan header (SOS) start, and where its coded data starts.
fn layout(j: &[u8]) -> Result<(usize, usize, usize), String> {
    let mut at = 2;
    let mut sof = None;
    while at + 4 <= j.len() {
        if j[at] != 0xFF {
            return Err("not a JPEG segment".into());
        }
        let marker = j[at + 1];
        let len = u16::from_be_bytes([j[at + 2], j[at + 3]]) as usize;
        if marker == 0xC0 {
            sof = Some(at);
        }
        if marker == 0xDA {
            return sof.map(|s| (s, at, at + 2 + len)).ok_or_else(|| "a JPEG without a baseline frame header".to_string());
        }
        at += 2 + len;
    }
    Err("a JPEG without a scan".into())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The strips joined decode to the same picture as one JPEG of the whole (4:2:0 and 4:4:4).
    #[test]
    fn strips_decode_as_the_whole_does() {
        let (w, h) = (1437u32, 981u32);
        let px: Vec<u8> = (0..w * h).flat_map(|i| {
            let (x, y) = (i % w, i / w);
            [(x * 7 + y) as u8, (y * 5 ^ x) as u8, ((x + y) * 3) as u8]
        }).collect();
        for q in [75u8, 92] {
            let joined = encode(&px, w, h, q).unwrap();
            let whole = strip(&px, w, 0, h, q).unwrap();
            let a = image::load_from_memory(&joined).unwrap().to_rgb8();
            let b = image::load_from_memory(&whole).unwrap().to_rgb8();
            assert_eq!(a.dimensions(), (w, h));
            let worst = a.as_raw().iter().zip(b.as_raw()).map(|(x, y)| (*x as i32 - *y as i32).abs()).max().unwrap();
            assert!(worst <= 1, "quality {q}: strips differ from the whole by {worst}");
        }
    }
}
