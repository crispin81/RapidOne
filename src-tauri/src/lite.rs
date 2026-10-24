//! Remove Lite: object removal for any computer, no graphics card or Python
//! needed. ONNX Runtime on the CPU (as the subject mask and noise reduction).
//!
//! - Fill: LaMa (Suvorov et al., big-lama, Apache-2.0; `lama_fp32.onnx` from
//!   Carve/LaMa-ONNX, a fixed 512 x 512 input). It continues the textures and
//!   lines round the hole and never invents objects. Excellent for small and
//!   medium objects; softer on very large ones (Klein's job).
//! - Outline: MobileSAM (Zhang et al., MIT; `mobile_sam_image_encoder.onnx`
//!   and `sam_mask_decoder_multi.onnx` from Acly/MobileSAM): the object in a
//!   drawn box, or in each box Grounding DINO finds; of its four outlines the
//!   one that fits the box and keeps off people (BiRefNet's subject mask,
//!   `birefnet-lite.onnx`, which also keeps people out of the hole).
//! - Find by name: Grounding DINO tiny (IDEA Research, Apache-2.0; the int8
//!   ONNX from onnx-community/grounding-dino-tiny-ONNX as
//!   `grounding-dino-tiny-int8.onnx`, its tokenizer as
//!   `grounding-dino-tokenizer.json`): "sign" -> a box round every sign.
//!   1.2 s on this PC's CPU at its 800 x 800 input; int8 matched fp32.
//!
//! 17 Oct, this PC's CPU: outline 0.3 s (encoder 0.2 s once a photo, box
//! 0.07 s), fill 0.9 s; MI-GAN (MIT, 30 MB) was tried too and smeared badly.
//!
//! The model files sit in the models folder (`mask::find_model`), never in git.

use image::{GrayImage, Luma, Rgb, RgbImage};
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};

const LAMA_FILE: &str = "lama_fp32.onnx";
const DINO_FILE: &str = "grounding-dino-tiny-int8.onnx";
const DINO_VOCAB: &str = "grounding-dino-tokenizer.json";
/// Grounding DINO's input (the picture stretched to it).
const DINO: usize = 800;
/// A match this sure or more counts; at most this many, overlaps merged.
const DINO_SCORE: f32 = 0.35;
const DINO_MAX: usize = 8;
const ENCODER_FILE: &str = "mobile_sam_image_encoder.onnx";
const DECODER_FILE: &str = "sam_mask_decoder_multi.onnx";
/// The subject (people) mask, so a box's outline keeps them out of the hole.
const SUBJECT_FILE: &str = "birefnet-lite.onnx";
/// In an outline file: 255 = remove, PROTECT = a person next to it, which
/// the fill must not grow into; 0 = anything else.
pub const PROTECT: u8 = 128;
/// LaMa's fixed input size.
const LAMA: usize = 512;
/// MobileSAM's input: long edge.
const SAM_SIDE: f32 = 1024.0;

fn session(file: &str) -> Option<ort::session::Session> {
    let path = crate::mask::find_model(file)?;
    let env = crate::mask::ort_env()?;
    ort::session::Session::builder(env).ok()?.commit_from_file(path).ok()
}

fn lama() -> Option<&'static Mutex<ort::session::Session>> {
    static S: OnceLock<Option<Mutex<ort::session::Session>>> = OnceLock::new();
    S.get_or_init(|| session(LAMA_FILE).map(Mutex::new)).as_ref()
}

fn sam() -> Option<&'static Mutex<(ort::session::Session, ort::session::Session)>> {
    static S: OnceLock<Option<Mutex<(ort::session::Session, ort::session::Session)>>> = OnceLock::new();
    S.get_or_init(|| Some(Mutex::new((session(ENCODER_FILE)?, session(DECODER_FILE)?)))).as_ref()
}

fn dino() -> Option<&'static Mutex<(ort::session::Session, std::collections::HashMap<String, i64>)>> {
    static S: OnceLock<Option<Mutex<(ort::session::Session, std::collections::HashMap<String, i64>)>>> = OnceLock::new();
    S.get_or_init(|| {
        let text = std::fs::read_to_string(crate::mask::find_model(DINO_VOCAB)?).ok()?;
        let json: serde_json::Value = serde_json::from_str(&text).ok()?;
        let vocab = json["model"]["vocab"].as_object()?.iter().filter_map(|(k, v)| Some((k.clone(), v.as_i64()?))).collect();
        Some(Mutex::new((session(DINO_FILE)?, vocab)))
    })
    .as_ref()
}

/// Whether Lite can find things by name (Grounding DINO installed).
pub fn can_find_words() -> bool {
    crate::mask::find_model(DINO_FILE).is_some() && crate::mask::find_model(DINO_VOCAB).is_some()
}

/// BERT's uncased word pieces for `text` (Grounding DINO's tokenizer): lower
/// case, accents off, split at spaces and punctuation, each word into the
/// longest pieces the vocabulary has ("##" continuing).
fn word_pieces(text: &str, vocab: &std::collections::HashMap<String, i64>) -> Vec<(String, i64)> {
    let unk = vocab.get("[UNK]").copied().unwrap_or(100);
    let mut words: Vec<String> = Vec::new();
    let mut cur = String::new();
    for c in text.to_lowercase().chars() {
        let c = match c {
            'à'..='å' => 'a',
            'è'..='ë' => 'e',
            'ì'..='ï' => 'i',
            'ò'..='ö' => 'o',
            'ù'..='ü' => 'u',
            'ç' => 'c',
            'ñ' => 'n',
            c => c,
        };
        if c.is_whitespace() || c.is_control() {
            if !cur.is_empty() {
                words.push(std::mem::take(&mut cur));
            }
        } else if c.is_ascii_punctuation() {
            if !cur.is_empty() {
                words.push(std::mem::take(&mut cur));
            }
            words.push(c.to_string());
        } else {
            cur.push(c);
        }
    }
    if !cur.is_empty() {
        words.push(cur);
    }
    let mut out = Vec::new();
    for w in words {
        let chars: Vec<char> = w.chars().collect();
        let mut start = 0;
        let mut pieces = Vec::new();
        while start < chars.len() {
            let mut end = chars.len();
            let mut found = None;
            while end > start {
                let piece: String = chars[start..end].iter().collect();
                let key = if start > 0 { format!("##{piece}") } else { piece };
                if let Some(&id) = vocab.get(&key) {
                    found = Some((key, id));
                    break;
                }
                end -= 1;
            }
            match found {
                Some(p) => {
                    pieces.push(p);
                    start = end;
                }
                None => {
                    pieces = vec![("[UNK]".to_string(), unk)];
                    break;
                }
            }
        }
        out.extend(pieces);
    }
    out
}

/// Grounding DINO: boxes (pixels of `picture`) round everything matching
/// `words`, surest first, with their scores.
pub fn find_words(picture: &Path, words: &str) -> Result<Vec<([f32; 4], f32)>, String> {
    let dino = dino().ok_or("Grounding DINO isn't installed")?;
    let img = image::open(picture).map_err(|e| e.to_string())?.to_rgb8();
    let (w, h) = (img.width() as f32, img.height() as f32);
    let small = image::imageops::resize(&img, DINO as u32, DINO as u32, image::imageops::FilterType::CatmullRom);
    const MEAN: [f32; 3] = [0.485, 0.456, 0.406];
    const STD: [f32; 3] = [0.229, 0.224, 0.225];
    let mut px = vec![0.0f32; 3 * DINO * DINO];
    for (x, y, p) in small.enumerate_pixels() {
        for c in 0..3 {
            px[c * DINO * DINO + y as usize * DINO + x as usize] = (p.0[c] as f32 / 255.0 - MEAN[c]) / STD[c];
        }
    }
    let mut dino = dino.lock().map_err(|_| "busy")?;
    // "[CLS] words . [SEP]": a phrase ends with a full stop.
    let mut tokens = vec![("[CLS]".to_string(), dino.1.get("[CLS]").copied().unwrap_or(101))];
    tokens.extend(word_pieces(&format!("{} .", words.trim().trim_end_matches('.')), &dino.1));
    tokens.push(("[SEP]".to_string(), dino.1.get("[SEP]").copied().unwrap_or(102)));
    let ids: Vec<i64> = tokens.iter().map(|t| t.1).collect();
    let n = ids.len();
    let query: Vec<usize> = tokens.iter().enumerate().filter(|(_, t)| !matches!(t.0.as_str(), "[CLS]" | "[SEP]" | ".")).map(|(i, _)| i).collect();
    if query.is_empty() {
        return Ok(Vec::new());
    }
    let t = |shape: Vec<usize>, v: Vec<i64>| ort::value::Tensor::from_array((shape, v.into_boxed_slice())).map_err(|e| e.to_string());
    let out = dino
        .0
        .run(ort::inputs![
            "pixel_values" => ort::value::Tensor::from_array(([1usize, 3, DINO, DINO], px.into_boxed_slice())).map_err(|e| e.to_string())?,
            "input_ids" => t(vec![1, n], ids)?,
            "token_type_ids" => t(vec![1, n], vec![0; n])?,
            "attention_mask" => t(vec![1, n], vec![1; n])?,
            "pixel_mask" => t(vec![1, DINO, DINO], vec![1; DINO * DINO])?,
        ])
        .map_err(|e| e.to_string())?;
    let (ls, logits) = out["logits"].try_extract_tensor::<f32>().map_err(|e| e.to_string())?;
    let (_, boxes) = out["pred_boxes"].try_extract_tensor::<f32>().map_err(|e| e.to_string())?;
    let (q, k) = (ls[1] as usize, ls[2] as usize);
    let mut found: Vec<([f32; 4], f32)> = (0..q)
        .filter_map(|i| {
            let score = query.iter().map(|&j| 1.0 / (1.0 + (-logits[i * k + j]).exp())).fold(0.0f32, f32::max);
            (score >= DINO_SCORE).then(|| {
                let (cx, cy, bw, bh) = (boxes[i * 4], boxes[i * 4 + 1], boxes[i * 4 + 2], boxes[i * 4 + 3]);
                ([((cx - bw / 2.0) * w).max(0.0), ((cy - bh / 2.0) * h).max(0.0), ((cx + bw / 2.0) * w).min(w), ((cy + bh / 2.0) * h).min(h)], score)
            })
        })
        .collect();
    found.sort_by(|a, b| b.1.total_cmp(&a.1));
    // Overlapping boxes for the same thing: keep the surest.
    let iou = |a: &[f32; 4], b: &[f32; 4]| {
        let ix = (a[2].min(b[2]) - a[0].max(b[0])).max(0.0);
        let iy = (a[3].min(b[3]) - a[1].max(b[1])).max(0.0);
        let inter = ix * iy;
        inter / ((a[2] - a[0]) * (a[3] - a[1]) + (b[2] - b[0]) * (b[3] - b[1]) - inter).max(1e-6)
    };
    let mut kept: Vec<([f32; 4], f32)> = Vec::new();
    for f in found {
        if kept.len() < DINO_MAX && kept.iter().all(|k| iou(&k.0, &f.0) < 0.5) {
            kept.push(f);
        }
    }
    Ok(kept)
}

fn subject_session() -> Option<&'static Mutex<ort::session::Session>> {
    static S: OnceLock<Option<Mutex<ort::session::Session>>> = OnceLock::new();
    S.get_or_init(|| session(SUBJECT_FILE).map(Mutex::new)).as_ref()
}

/// The last picture's subject mask (BiRefNet, people and other main
/// subjects): (picture, mask at the picture's size).
static SUBJECT: Mutex<Option<(PathBuf, Vec<bool>)>> = Mutex::new(None);

/// Which pixels of `picture` (w x h) are the photo's subject; None without
/// the model.
fn subject(picture: &Path, w: u32, h: u32) -> Option<Vec<bool>> {
    let mut held = SUBJECT.lock().unwrap_or_else(|e| e.into_inner());
    if let Some((p, m)) = held.as_ref() {
        if p == picture && m.len() == (w * h) as usize {
            return Some(m.clone());
        }
    }
    const N: usize = 1024;
    let img = image::open(picture).ok()?.to_rgb8();
    let small = image::imageops::resize(&img, N as u32, N as u32, image::imageops::FilterType::Triangle);
    const MEAN: [f32; 3] = [0.485, 0.456, 0.406];
    const STD: [f32; 3] = [0.229, 0.224, 0.225];
    let mut x = vec![0.0f32; 3 * N * N];
    for (px, py, p) in small.enumerate_pixels() {
        for c in 0..3 {
            x[c * N * N + py as usize * N + px as usize] = (p.0[c] as f32 / 255.0 - MEAN[c]) / STD[c];
        }
    }
    let s = subject_session()?;
    let logits = {
        let mut s = s.lock().ok()?;
        let out = s.run(ort::inputs![ort::value::Tensor::from_array(([1usize, 3, N, N], x.into_boxed_slice())).ok()?]).ok()?;
        let last = out.len().checked_sub(1)?;
        let (_, l) = out[last].try_extract_tensor::<f32>().ok()?;
        l.to_vec()
    };
    if logits.len() != N * N {
        return None;
    }
    let m: Vec<bool> = (0..(w * h) as usize)
        .map(|i| {
            let (px, py) = ((i % w as usize) * N / w as usize, (i / w as usize) * N / h as usize);
            logits[py.min(N - 1) * N + px.min(N - 1)] > 0.0
        })
        .collect();
    *held = Some((picture.to_path_buf(), m.clone()));
    Some(m)
}

/// Whether Lite removal can run (LaMa installed).
pub fn available() -> bool {
    crate::mask::find_model(LAMA_FILE).is_some()
}

/// Whether Lite can outline what's in a box (MobileSAM installed).
pub fn can_outline() -> bool {
    crate::mask::find_model(ENCODER_FILE).is_some() && crate::mask::find_model(DECODER_FILE).is_some()
}

/// MobileSAM's analysis of the last picture: (picture file, its size, the
/// scale to the encoder's input, the embedding).
type Embedded = (PathBuf, (u32, u32), f32, Vec<f32>);
static EMBEDDED: Mutex<Option<Embedded>> = Mutex::new(None);

/// MobileSAM's analysis of `picture` (an image file), kept for the next boxes.
fn embedding(picture: &Path) -> Result<((u32, u32), f32, Vec<f32>), String> {
    let mut held = EMBEDDED.lock().unwrap_or_else(|e| e.into_inner());
    if let Some((p, size, s, e)) = held.as_ref() {
        if p == picture {
            return Ok((*size, *s, e.clone()));
        }
    }
    let img = image::open(picture).map_err(|e| e.to_string())?.to_rgb8();
    let (w, h) = img.dimensions();
    let s = SAM_SIDE / w.max(h) as f32;
    let (sw, sh) = (((w as f32 * s).round() as u32).max(1), ((h as f32 * s).round() as u32).max(1));
    let small = image::imageops::resize(&img, sw, sh, image::imageops::FilterType::Triangle);
    // The encoder takes the picture as it is (H x W x 3, 0..255); it
    // normalises and pads to 1024 itself.
    let input: Vec<f32> = small.as_raw().iter().map(|&v| v as f32).collect();
    let tensor = ort::value::Tensor::from_array(([sh as usize, sw as usize, 3usize], input.into_boxed_slice())).map_err(|e| e.to_string())?;
    let sam = sam().ok_or("MobileSAM isn't installed")?;
    let mut sam = sam.lock().map_err(|_| "busy")?;
    let out = sam.0.run(ort::inputs!["input_image" => tensor]).map_err(|e| e.to_string())?;
    let (_, emb) = out[0].try_extract_tensor::<f32>().map_err(|e| e.to_string())?;
    let emb = emb.to_vec();
    *held = Some((picture.to_path_buf(), (w, h), s, emb.clone()));
    Ok(((w, h), s, emb))
}

/// Loads Lite's models (LaMa, MobileSAM, Grounding DINO) ahead of the Remove
/// tool, so its first use doesn't wait for them. They stay loaded until quit.
pub fn warm() {
    let _ = lama();
    if can_outline() {
        let _ = sam();
    }
    let _ = dino();
}

/// Gets MobileSAM ready for `picture` (Remove opening on the photo).
pub fn prepare(picture: &Path) -> Result<(), String> {
    // LaMa loads now too (about 5 s the first time), not on the first removal.
    let _ = lama();
    let _ = dino();
    if !can_outline() {
        return Ok(());
    }
    embedding(picture).map(|_| ())
}

/// The object in `bbox` (pixels of `picture`): a white-on-black mask the
/// picture's size, and the mask's own bounding box; None if nothing's there.
pub fn outline(picture: &Path, bbox: [f32; 4]) -> Result<Option<(GrayImage, [f32; 4])>, String> {
    let ((w, h), s, emb) = embedding(picture)?;
    let sam = sam().ok_or("MobileSAM isn't installed")?;
    let mut sam = sam.lock().map_err(|_| "busy")?;
    let t = |shape: Vec<usize>, v: Vec<f32>| ort::value::Tensor::from_array((shape, v.into_boxed_slice())).map_err(|e| e.to_string());
    let out = sam
        .1
        .run(ort::inputs![
            "image_embeddings" => t(vec![1, 256, 64, 64], emb)?,
            // A box is two points: top-left (label 2) and bottom-right (3).
            "point_coords" => t(vec![1, 2, 2], vec![bbox[0] * s, bbox[1] * s, bbox[2] * s, bbox[3] * s])?,
            "point_labels" => t(vec![1, 2], vec![2.0, 3.0])?,
            "mask_input" => t(vec![1, 1, 256, 256], vec![0.0; 256 * 256])?,
            "has_mask_input" => t(vec![1], vec![0.0])?,
            "orig_im_size" => t(vec![2], vec![h as f32, w as f32])?,
        ])
        .map_err(|e| e.to_string())?;
    let m = out["masks"].try_extract_tensor::<f32>().map_err(|e| e.to_string())?.1.to_vec();
    let iou = out["iou_predictions"].try_extract_tensor::<f32>().map_err(|e| e.to_string())?.1.to_vec();
    drop(out);
    let n = (w * h) as usize;
    let count = m.len() / n.max(1);
    if count == 0 {
        return Err("MobileSAM gave an odd mask".into());
    }
    drop(sam);
    // MobileSAM offers several outlines (17 Oct, the sign behind the bride's
    // head: one was the sign and its post, others took her head too). Pick
    // the one that best fits the box and keeps off the photo's subject, and
    // keep the subject out of the hole (Lite is for things, not people; when
    // most outlines are mostly subject, the box is round a person and the
    // fullest outline is taken instead).
    let subj = subject(picture, w, h);
    let (bx0, by0, bx1, by1) = (bbox[0].max(0.0) as u32, bbox[1].max(0.0) as u32, (bbox[2] as u32).min(w), (bbox[3] as u32).min(h));
    let box_area = ((bx1.saturating_sub(bx0)) * (by1.saturating_sub(by0))).max(1) as f32;
    let stats: Vec<(f32, f32, f32, f32)> = (0..count)
        .map(|k| {
            let mk = &m[k * n..(k + 1) * n];
            let (mut total, mut inside, mut on_subject) = (0usize, 0usize, 0usize);
            for (i, &v) in mk.iter().enumerate() {
                if v > 0.0 {
                    total += 1;
                    let (x, y) = (i as u32 % w, i as u32 / w);
                    if x >= bx0 && x < bx1 && y >= by0 && y < by1 {
                        inside += 1;
                    }
                    if subj.as_ref().is_some_and(|s| s[i]) {
                        on_subject += 1;
                    }
                }
            }
            let t = total.max(1) as f32;
            (iou.get(k).copied().unwrap_or(0.0), inside as f32 / t, on_subject as f32 / t, inside as f32 / box_area)
        })
        .collect();
    let mut on: Vec<f32> = stats.iter().map(|s| s.2).collect();
    on.sort_by(f32::total_cmp);
    let person = on[on.len() / 2] > 0.5;
    let fits: Vec<usize> = (0..count).filter(|&k| stats[k].1 >= 0.85).collect();
    let pool = if fits.is_empty() { (0..count).collect() } else { fits };
    let score = |k: usize| {
        let (q, _, s, cover) = stats[k];
        if person { q + cover } else { q - 2.0 * s + 0.2 * cover }
    };
    let pick = pool.into_iter().max_by(|&a, &b| score(a).total_cmp(&score(b))).unwrap_or(0);
    let mk = &m[pick * n..(pick + 1) * n];
    // People within reach of the box (grown 25%) are protected.
    let (gx, gy) = ((bx1 - bx0) as f32 * 0.25, (by1 - by0) as f32 * 0.25);
    let near = |x: u32, y: u32| (x as f32) >= bx0 as f32 - gx && (x as f32) <= bx1 as f32 + gx && (y as f32) >= by0 as f32 - gy && (y as f32) <= by1 as f32 + gy;
    let mut mask = GrayImage::new(w, h);
    let (mut x0, mut y0, mut x1, mut y1) = (u32::MAX, u32::MAX, 0u32, 0u32);
    for (i, &v) in mk.iter().enumerate() {
        let (x, y) = (i as u32 % w, i as u32 / w);
        let is_subject = !person && subj.as_ref().is_some_and(|s| s[i]) && near(x, y);
        if is_subject {
            mask.put_pixel(x, y, Luma([PROTECT]));
        } else if v > 0.0 {
            mask.put_pixel(x, y, Luma([255]));
            x0 = x0.min(x);
            y0 = y0.min(y);
            x1 = x1.max(x);
            y1 = y1.max(y);
        }
    }
    if x0 == u32::MAX {
        return Ok(None);
    }
    Ok(Some((mask, [x0 as f32, y0 as f32, (x1 + 1) as f32, (y1 + 1) as f32])))
}

/// LaMa fills `hole` (true = remove) in `picture`; everything outside the
/// hole is kept exactly, the fill blended in over a few pixels at its edge.
/// `seed` varies the result (Try again): the hole grown a little more each time.
/// `protect` (if given): pixels the grown hole must not reach (a person
/// beside what's removed).
pub fn fill(picture: &RgbImage, hole: &[bool], protect: Option<&[bool]>, seed: u32) -> Result<RgbImage, String> {
    let (w, h) = picture.dimensions();
    let (wu, hu) = (w as usize, h as usize);
    // Grown a little (edges and shadows of the object), more on each retry.
    let grow = ((w.max(h) as f32) * (0.006 + 0.004 * (seed % 4) as f32)).ceil() as usize + 2;
    let mut hole = crate::removal::dilate(hole, wu, hu, grow);
    if let Some(p) = protect {
        hole.iter_mut().zip(p).for_each(|(h, &p)| *h &= !p);
    }
    // Into LaMa's 512 x 512 (stretched; undone on the way back).
    let small = image::imageops::resize(picture, LAMA as u32, LAMA as u32, image::imageops::FilterType::CatmullRom);
    let mut img = vec![0.0f32; 3 * LAMA * LAMA];
    let mut mask = vec![0.0f32; LAMA * LAMA];
    for y in 0..LAMA {
        for x in 0..LAMA {
            let p = small.get_pixel(x as u32, y as u32).0;
            for c in 0..3 {
                img[c * LAMA * LAMA + y * LAMA + x] = p[c] as f32 / 255.0;
            }
            // A pixel of LaMa's picture is in the hole if any of the photo's
            // pixels it covers are.
            let (sx0, sx1) = (x * wu / LAMA, ((x + 1) * wu).div_ceil(LAMA).min(wu));
            let (sy0, sy1) = (y * hu / LAMA, ((y + 1) * hu).div_ceil(LAMA).min(hu));
            let any = (sy0..sy1.max(sy0 + 1)).any(|sy| (sx0..sx1.max(sx0 + 1)).any(|sx| hole[sy.min(hu - 1) * wu + sx.min(wu - 1)]));
            mask[y * LAMA + x] = if any { 1.0 } else { 0.0 };
        }
    }
    let lama = lama().ok_or("LaMa isn't installed")?;
    let out = {
        let mut lama = lama.lock().map_err(|_| "busy")?;
        let out = lama
            .run(ort::inputs![
                "image" => ort::value::Tensor::from_array(([1usize, 3, LAMA, LAMA], img.into_boxed_slice())).map_err(|e| e.to_string())?,
                "mask" => ort::value::Tensor::from_array(([1usize, 1, LAMA, LAMA], mask.into_boxed_slice())).map_err(|e| e.to_string())?,
            ])
            .map_err(|e| e.to_string())?;
        let (_, o) = out[0].try_extract_tensor::<f32>().map_err(|e| e.to_string())?;
        o.to_vec()
    };
    // Its output is 0..255, channels first.
    let filled = RgbImage::from_fn(LAMA as u32, LAMA as u32, |x, y| {
        let i = y as usize * LAMA + x as usize;
        Rgb(std::array::from_fn(|c| out[c * LAMA * LAMA + i].clamp(0.0, 255.0) as u8))
    });
    let filled = image::imageops::resize(&filled, w, h, image::imageops::FilterType::CatmullRom);
    Ok(composite(picture, &filled, &hole, (grow / 2).max(2)))
}

/// `filled` (the same size as `picture`) inside `hole`, the picture exactly
/// everywhere else, blended over `soft` pixels at the hole's edge.
pub fn composite(picture: &RgbImage, filled: &RgbImage, hole: &[bool], soft: usize) -> RgbImage {
    let (w, h) = picture.dimensions();
    let (wu, hu) = (w as usize, h as usize);
    let weight: Vec<f32> = blur(&hole.iter().map(|&b| if b { 1.0 } else { 0.0 }).collect::<Vec<f32>>(), wu, hu, soft);
    RgbImage::from_fn(w, h, |x, y| {
        let i = y as usize * wu + x as usize;
        let k = if hole[i] { 1.0 } else { weight[i] };
        let (a, b) = (picture.get_pixel(x, y).0, filled.get_pixel(x, y).0);
        Rgb(std::array::from_fn(|c| (a[c] as f32 * (1.0 - k) + b[c] as f32 * k).round() as u8))
    })
}


/// A box blur of radius r, twice (close to a Gaussian).
fn blur(v: &[f32], w: usize, h: usize, r: usize) -> Vec<f32> {
    let pass = |src: &[f32], horizontal: bool| -> Vec<f32> {
        let mut out = vec![0.0; src.len()];
        let (n, m) = if horizontal { (h, w) } else { (w, h) };
        for line in 0..n {
            let at = |k: usize| if horizontal { line * w + k } else { k * w + line };
            let mut sum = 0.0;
            let mut count = 0.0;
            for k in 0..=r.min(m - 1) {
                sum += src[at(k)];
                count += 1.0;
            }
            for k in 0..m {
                out[at(k)] = sum / count;
                if k + r + 1 < m {
                    sum += src[at(k + r + 1)];
                    count += 1.0;
                }
                if k >= r {
                    sum -= src[at(k - r)];
                    count -= 1.0;
                }
            }
        }
        out
    };
    let mut x = v.to_vec();
    for _ in 0..2 {
        x = pass(&x, true);
        x = pass(&x, false);
    }
    x
}
