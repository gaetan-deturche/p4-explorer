//! A texture's source pixels, read from its package: the payload in the package
//! trailer (or, in older packages, behind the texture's native data), unpacked from its FCompressedBuffer (Oodle, through the engine's own
//! oo2core DLL), undone from the UEDELTA row transform when that is how it was
//! stored, and turned into 8-bit RGBA for display — any UDIM block, layer, source
//! mip and slice of it.
//!
//! Layouts follow PackageTrailer.h, CompressedBuffer.cpp, ImageCoreDelta.cpp and
//! FTextureSource (Texture.cpp) in UE 5.6.

use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};

use serde::Serialize;

use super::archive::{Reader, Val};
use super::doc::properties;
use super::package::{Package, PACKAGE_FILE_TAG};
use super::properties::read_export_properties_end;

/// One UDIM block of the source (block 0 is the base one).
#[derive(Debug, Clone, Serialize)]
pub struct Block {
    pub x: i64,
    pub y: i64,
    pub width: usize,
    pub height: usize,
    pub slices: usize,
    pub mips: usize,
    #[serde(skip)]
    pub offset: usize,
}

/// What the source holds, from the texture's `Source` property.
#[derive(Debug, Clone, Serialize)]
pub struct Layout {
    pub blocks: Vec<Block>,
    /// Each layer's format (TSF_*).
    pub layers: Vec<String>,
    pub compression: String,
    /// A volume's slices halve with each mip; an array's stay.
    pub volume: bool,
}

/// Pixels ready to show.
pub enum Pixels {
    /// 8-bit RGBA, row-major.
    Rgba { width: u32, height: u32, data: Vec<u8> },
    /// A PNG or JPEG source, shown as the file it is.
    Encoded { mime: &'static str, data: Vec<u8> },
}

fn num(v: Option<&Val>) -> Option<i64> {
    match v? {
        Val::Int(i) => Some(*i),
        Val::UInt(u) => Some(*u as i64),
        _ => None,
    }
}

pub fn source_layout(pkg: &Package) -> Option<Layout> {
    let main = pkg.exports.iter().position(|e| e.is_asset)?;
    let class = pkg.export_class(&pkg.exports[main]);
    // Other classes have a `Source` too (NiagaraScript).
    if !super::doc::leaf(&class).contains("Texture") {
        return None;
    }
    let props = properties(pkg, main);
    let src = props.get("Source")?;
    let n = |v: &Val, k: &str, default: i64| num(v.get(k)).unwrap_or(default);
    let format = src.get("Format").and_then(|f| f.as_str()).unwrap_or("TSF_Invalid").to_string();
    // Unsaved means the default, TSCF_None (or a legacy PNG flag).
    let compression = match src.get("CompressionFormat").and_then(|c| c.as_str()) {
        Some(c) => c.to_string(),
        None if src.get("bPNGCompressed") == Some(&Val::Bool(true)) => "TSCF_PNG".into(),
        None => "TSCF_None".into(),
    };
    let offsets: Vec<usize> = match src.get("BlockDataOffsets") {
        Some(Val::List(l)) => l.iter().map(|v| num(Some(v)).unwrap_or(0) as usize).collect(),
        _ => Vec::new(),
    };
    let mut blocks = vec![Block {
        x: n(src, "BaseBlockX", 0),
        y: n(src, "BaseBlockY", 0),
        width: n(src, "SizeX", 0) as usize,
        height: n(src, "SizeY", 0) as usize,
        slices: n(src, "NumSlices", 1).max(1) as usize,
        mips: n(src, "NumMips", 1).max(1) as usize,
        offset: 0,
    }];
    if let Some(Val::List(more)) = src.get("Blocks") {
        for b in more {
            blocks.push(Block {
                x: n(b, "BlockX", 0),
                y: n(b, "BlockY", 0),
                width: n(b, "SizeX", 0) as usize,
                height: n(b, "SizeY", 0) as usize,
                slices: n(b, "NumSlices", 1).max(1) as usize,
                mips: n(b, "NumMips", 1).max(1) as usize,
                offset: 0,
            });
        }
    }
    for (i, b) in blocks.iter_mut().enumerate() {
        b.offset = offsets.get(i).copied().unwrap_or(0);
    }
    let layers = match src.get("LayerFormat") {
        Some(Val::List(l)) if !l.is_empty() => l.iter().map(|f| f.as_str().unwrap_or("TSF_Invalid").to_string()).collect(),
        _ => vec![format; n(src, "NumLayers", 1).max(1) as usize],
    };
    let volume = super::doc::leaf(&class) == "VolumeTexture";
    Some(Layout { blocks, layers, compression, volume })
}

/// Where one view starts in the decompressed payload, and its size —
/// FTextureSource::CalcMipOffset, plus the slice within the mip.
fn view_offset(l: &Layout, block: usize, layer: usize, mip: usize, slice: usize) -> Result<(usize, usize, usize), String> {
    let b = l.blocks.get(block).ok_or("no such UDIM block")?;
    if layer >= l.layers.len() || mip >= b.mips {
        return Err("no such layer or mip".into());
    }
    let mip_slices = |m: usize| if l.volume { (b.slices >> m).max(1) } else { b.slices };
    let mip_dims = |m: usize| ((b.width >> m).max(1), (b.height >> m).max(1));
    let bpp_of = |i: usize| bytes_per(&l.layers[i]).map(|(bpp, _)| bpp).ok_or_else(|| format!("source format {} is not supported", l.layers[i]));
    let mut offset = b.offset;
    for i in 0..layer {
        let bpp = bpp_of(i)?;
        offset += (0..b.mips).map(|m| mip_dims(m).0 * mip_dims(m).1 * mip_slices(m) * bpp).sum::<usize>();
    }
    let bpp = bpp_of(layer)?;
    for m in 0..mip {
        offset += mip_dims(m).0 * mip_dims(m).1 * mip_slices(m) * bpp;
    }
    if slice >= mip_slices(mip) {
        return Err("no such slice".into());
    }
    let (w, h) = mip_dims(mip);
    Ok((offset + slice * w * h * bpp, w, h))
}

/// Bytes per pixel and per sample (the unit the delta transform works in).
fn bytes_per(format: &str) -> Option<(usize, usize)> {
    Some(match format {
        "TSF_G8" => (1, 1),
        "TSF_BGRA8" | "TSF_BGRE8" => (4, 1),
        "TSF_G16" | "TSF_R16F" => (2, 2),
        "TSF_RGBA16" | "TSF_RGBA16F" => (8, 2),
        "TSF_R32F" => (4, 4),
        "TSF_RGBA32F" => (16, 4),
        _ => return None,
    })
}

// ---------------------------------------------------------------------------
// package trailer

const TRAILER_HEADER_TAG: u64 = 0xD1C43B2E80A5F697;
const TRAILER_FOOTER_TAG: u64 = 0x29BFCA045138DE76;
const FOOTER_SIZE: usize = 8 + 8 + 4;
pub(super) const NO_TRAILER: &str = "the package has no payload trailer";

pub(super) struct Payload {
    /// The payload's content id (FIoHash), as an FEditorBulkData names it.
    pub hash: [u8; 20],
    pub offset: usize,
    pub compressed: usize,
    pub raw: u64,
}

fn le_u32(b: &[u8], at: usize) -> u32 {
    u32::from_le_bytes(b[at..at + 4].try_into().unwrap())
}
fn le_u64(b: &[u8], at: usize) -> u64 {
    u64::from_le_bytes(b[at..at + 8].try_into().unwrap())
}

/// The payloads stored in the package file itself (FPackageTrailer::TryLoadBackwards),
/// or from the summary's `toc` offset when the footer is missing (a padded file).
pub(super) fn local_payloads(data: &[u8], toc: i64) -> Result<Vec<Payload>, String> {
    if data.len() < FOOTER_SIZE {
        return Err("package too small for a trailer".into());
    }
    let foot = data.len() - FOOTER_SIZE;
    let start = if le_u64(data, foot) == TRAILER_FOOTER_TAG {
        let trailer_len = le_u64(data, foot + 8) as usize;
        data.len().checked_sub(trailer_len).ok_or("bad trailer length")?
    } else if toc > 0 && (toc as usize) < foot && le_u64(data, toc as usize) == TRAILER_HEADER_TAG {
        toc as usize
    } else {
        return Err(NO_TRAILER.into());
    };
    if le_u64(data, start) != TRAILER_HEADER_TAG {
        return Err("bad trailer header".into());
    }
    let version = le_u32(data, start + 8);
    let header_len = le_u32(data, start + 12) as usize;
    let mut at = start + 24; // tag, version, header length, payloads length
    let mut legacy_local = true;
    if version < 1 {
        legacy_local = data[at] == 0; // EPayloadAccessMode::Local
        at += 1;
    }
    let count = le_u32(data, at) as usize;
    at += 4;
    let mut out = Vec::new();
    for _ in 0..count {
        // FLookupTableEntry: hash (20), offset, compressed size, raw size, flags (2), filter flags (2), access mode (1)
        if at + 44 > data.len() {
            return Err("truncated payload table".into());
        }
        let hash: [u8; 20] = data[at..at + 20].try_into().unwrap();
        let offset = le_u64(data, at + 20) as i64;
        let compressed = le_u64(data, at + 28) as usize;
        let raw = le_u64(data, at + 36);
        at += 44;
        let local = if version >= 1 {
            if version >= 2 {
                at += 4;
            }
            let mode = data[at];
            at += 1;
            mode == 0
        } else {
            legacy_local && offset >= 0
        };
        if local && offset >= 0 {
            out.push(Payload { hash, offset: start + header_len + offset as usize, compressed, raw });
        }
    }
    Ok(out)
}

// ---------------------------------------------------------------------------
// FCompressedBuffer

const BUFFER_MAGIC: u32 = 0xb7756362;
const BUFFER_HEADER: usize = 64;

fn be_u32(b: &[u8], at: usize) -> u32 {
    u32::from_be_bytes(b[at..at + 4].try_into().unwrap())
}
fn be_u64(b: &[u8], at: usize) -> u64 {
    u64::from_be_bytes(b[at..at + 8].try_into().unwrap())
}

pub(super) fn decompress(buf: &[u8], oodle: &dyn Fn() -> Result<&'static Oodle, String>) -> Result<Vec<u8>, String> {
    if buf.len() < BUFFER_HEADER || be_u32(buf, 0) != BUFFER_MAGIC {
        return Err("payload is not a compressed buffer".into());
    }
    let method = buf[8];
    let block_exp = buf[11];
    let blocks = be_u32(buf, 12) as usize;
    let raw_size = be_u64(buf, 16) as usize;
    if method == 0 {
        return buf.get(BUFFER_HEADER..BUFFER_HEADER + raw_size).map(|b| b.to_vec()).ok_or("truncated payload".into());
    }
    if method != 3 {
        return Err(format!("payload compression method {method} is not supported (only Oodle)"));
    }
    let block_size = 1usize << block_exp;
    let mut sources = Vec::with_capacity(blocks);
    let mut at = BUFFER_HEADER + 4 * blocks;
    for i in 0..blocks {
        let stored = be_u32(buf, BUFFER_HEADER + 4 * i) as usize;
        sources.push(buf.get(at..at + stored).ok_or("truncated payload block")?);
        at += stored;
    }
    let mut out = vec![0u8; raw_size];
    let mut dsts: Vec<&mut [u8]> = out.chunks_mut(block_size).collect();
    if dsts.len() != blocks {
        return Err("payload block count does not match its size".into());
    }
    let compressed = sources.iter().zip(&dsts).any(|(s, d)| s.len() < d.len());
    let lib = if compressed { Some(oodle()?) } else { None };
    let unpack = |src: &[u8], dst: &mut [u8]| -> Result<(), String> {
        match lib {
            Some(lib) if src.len() < dst.len() => lib.decompress(src, dst),
            _ => {
                dst.copy_from_slice(src.get(..dst.len()).ok_or("truncated payload block")?); // stored uncompressed
                Ok(())
            }
        }
    };
    // Blocks unpack on their own: big payloads (a mesh description runs to hundreds of MB) in parallel.
    let threads = if raw_size < 8 << 20 { 1 } else { std::thread::available_parallelism().map_or(1, |n| n.get()).min(8) };
    let per = blocks.div_ceil(threads).max(1);
    let unpack = &unpack;
    std::thread::scope(|s| {
        let jobs: Vec<_> = dsts
            .chunks_mut(per)
            .zip(sources.chunks(per))
            .map(|(d, src)| s.spawn(move || d.iter_mut().zip(src).try_for_each(|(d, s)| unpack(s, d))))
            .collect();
        jobs.into_iter().try_for_each(|j| j.join().unwrap_or_else(|_| Err("unpacking the payload panicked".into())))
    })?;
    Ok(out)
}

// ---------------------------------------------------------------------------
// Oodle, from the engine install

pub struct Oodle {
    decompress_fn: OodleDecompress,
}

type OodleDecompress = unsafe extern "C" fn(
    comp: *const u8,
    comp_len: isize,
    raw: *mut u8,
    raw_len: isize,
    fuzz_safe: i32,
    check_crc: i32,
    verbosity: i32,
    dec_buf_base: *mut u8,
    dec_buf_size: isize,
    callback: *const u8,
    callback_data: *const u8,
    decoder_memory: *mut u8,
    decoder_memory_size: isize,
    thread_phase: i32,
) -> isize;

impl Oodle {
    fn decompress(&self, src: &[u8], dst: &mut [u8]) -> Result<(), String> {
        // FuzzSafe_Yes, CheckCRC_Yes, Verbosity_None, Decode_Unthreaded — as FOodleDataCompression::Decompress.
        let n = unsafe {
            (self.decompress_fn)(
                src.as_ptr(),
                src.len() as isize,
                dst.as_mut_ptr(),
                dst.len() as isize,
                1,
                1,
                0,
                std::ptr::null_mut(),
                0,
                std::ptr::null(),
                std::ptr::null(),
                std::ptr::null_mut(),
                0,
                3,
            )
        };
        if n as usize != dst.len() {
            return Err("Oodle could not decompress the payload".into());
        }
        Ok(())
    }
}

#[cfg(windows)]
fn load_oodle(dll: &Path) -> Option<Oodle> {
    #[link(name = "kernel32")]
    extern "system" {
        fn LoadLibraryW(name: *const u16) -> isize;
        fn GetProcAddress(module: isize, name: *const u8) -> *const u8;
    }
    use std::os::windows::ffi::OsStrExt;
    let wide: Vec<u16> = dll.as_os_str().encode_wide().chain([0]).collect();
    unsafe {
        let module = LoadLibraryW(wide.as_ptr());
        if module == 0 {
            return None;
        }
        let f = GetProcAddress(module, b"OodleLZ_Decompress\0".as_ptr());
        if f.is_null() {
            return None;
        }
        Some(Oodle { decompress_fn: std::mem::transmute::<*const u8, OodleDecompress>(f) })
    }
}

#[cfg(not(windows))]
fn load_oodle(_dll: &Path) -> Option<Oodle> {
    None
}

/// oo2core under an engine tree: the known spots first, then a bounded walk of
/// Engine/Binaries (the DLL ships with the engine's .NET automation tools).
fn find_oodle_dll(root: &Path) -> Option<PathBuf> {
    let engine = root.join("Engine").join("Binaries");
    for rel in ["Win64/oo2core_9_win64.dll", "DotNET/AutomationTool/AutomationScripts/BuildGraph/oo2core_9_win64.dll"] {
        let p = engine.join(rel);
        if p.is_file() {
            return Some(p);
        }
    }
    fn walk(dir: &Path, depth: u32) -> Option<PathBuf> {
        let entries = std::fs::read_dir(dir).ok()?;
        let mut subdirs = Vec::new();
        for e in entries.flatten() {
            let p = e.path();
            let name = e.file_name().to_string_lossy().to_lowercase();
            if p.is_dir() {
                subdirs.push(p);
            } else if name.starts_with("oo2core_") && name.ends_with("_win64.dll") {
                return Some(p);
            }
        }
        if depth == 0 {
            return None;
        }
        subdirs.into_iter().find_map(|d| walk(&d, depth - 1))
    }
    walk(&engine, 5)
}

static OODLE: OnceLock<Mutex<Option<&'static Oodle>>> = OnceLock::new();

/// The engine's Oodle, loaded once from the first engine tree among `roots`
/// (a workspace root, or any folder inside one).
pub fn oodle(roots: &[PathBuf]) -> Result<&'static Oodle, String> {
    let slot = OODLE.get_or_init(|| Mutex::new(None));
    let mut found = slot.lock().map_err(|_| "Oodle lock poisoned")?;
    if let Some(o) = *found {
        return Ok(o);
    }
    for root in roots {
        for dir in root.ancestors() {
            if let Some(dll) = find_oodle_dll(dir) {
                if let Some(o) = load_oodle(&dll) {
                    let o: &'static Oodle = Box::leak(Box::new(o));
                    *found = Some(o);
                    return Ok(o);
                }
            }
        }
    }
    Err("no Oodle library (oo2core_*_win64.dll) found in the workspace's engine".into())
}

// ---------------------------------------------------------------------------
// UEDELTA (ImageCoreDelta.cpp)

const MIN_PIXELS_PER_CUT: i64 = 32768;
const MIN_PIXELS_FOR_ANY_CUT: i64 = 136 * 136;
const CUT_STRIDE_BYTES: i64 = 4096;
const MAX_NUM_CUTS: i64 = 512;

fn num_cuts(items: i64) -> i64 {
    if items <= MIN_PIXELS_PER_CUT {
        return 1;
    }
    let mut cuts = items / MIN_PIXELS_PER_CUT;
    while cuts > MAX_NUM_CUTS {
        cuts >>= 1;
    }
    cuts
}

/// (rows per cut, number of cuts) — ImageCoreDeltaComputeNumCutsForRows.
fn cuts_for_rows(size_x: i64, size_y: i64) -> (i64, i64) {
    let cuts1 = num_cuts(size_x * size_y);
    let rows = (size_y + cuts1 - 1) / cuts1;
    (rows, (size_y + rows - 1) / rows)
}

/// The tiles of one slice, as (x, width, y, height) in pixels —
/// AddSplitStridedViewsForDelta. Part of the file format: must match exactly.
fn tiles(size_x: i64, size_y: i64, bpp: i64) -> Vec<(i64, i64, i64, i64)> {
    let mut out = Vec::new();
    if size_x * size_y <= MIN_PIXELS_FOR_ANY_CUT {
        out.push((0, size_x, 0, size_y));
        return out;
    }
    let stride = size_x * bpp;
    if stride <= CUT_STRIDE_BYTES {
        let (rows, cuts) = cuts_for_rows(size_x, size_y);
        for c in 0..cuts {
            let y = c * rows;
            out.push((0, size_x, y, rows.min(size_y - y)));
        }
        return out;
    }
    let mut parts = (stride + CUT_STRIDE_BYTES - 1) / CUT_STRIDE_BYTES;
    let mut part_bytes = (stride + parts / 2) / parts;
    part_bytes = (part_bytes + 63) & !63;
    let part_pixels = part_bytes / bpp;
    parts = (size_x + part_pixels - 1) / part_pixels;
    for h in 0..parts {
        let x = h * part_pixels;
        let w = part_pixels.min(size_x - x);
        let (rows, cuts) = cuts_for_rows(w, size_y);
        for c in 0..cuts {
            let y = c * rows;
            out.push((x, w, y, rows.min(size_y - y)));
        }
    }
    out
}

/// Undo the row delta of one slice in place: row 0 of each tile is stored as
/// is, every later row as its difference from the row above (biased for 16- and
/// 32-bit samples).
fn undelta(slice: &mut [u8], size_x: usize, size_y: usize, bpp: usize, sample: usize) {
    let stride = size_x * bpp;
    for (x, w, y, h) in tiles(size_x as i64, size_y as i64, bpp as i64) {
        let (x, w, y, h) = (x as usize, w as usize, y as usize, h as usize);
        let row_bytes = w * bpp;
        for row in y + 1..y + h {
            let cur = row * stride + x * bpp;
            let up = cur - stride;
            match sample {
                1 => {
                    for i in 0..row_bytes {
                        slice[cur + i] = slice[cur + i].wrapping_add(slice[up + i]);
                    }
                }
                2 => {
                    for i in (0..row_bytes).step_by(2) {
                        let d = u16::from_le_bytes([slice[cur + i], slice[cur + i + 1]]);
                        let u = u16::from_le_bytes([slice[up + i], slice[up + i + 1]]);
                        let v = d.wrapping_add(u).wrapping_sub(0x8080);
                        slice[cur + i..cur + i + 2].copy_from_slice(&v.to_le_bytes());
                    }
                }
                _ => {
                    for i in (0..row_bytes).step_by(4) {
                        let d = u32::from_le_bytes(slice[cur + i..cur + i + 4].try_into().unwrap());
                        let u = u32::from_le_bytes(slice[up + i..up + i + 4].try_into().unwrap());
                        let v = d.wrapping_add(u).wrapping_sub(0x80808080);
                        slice[cur + i..cur + i + 4].copy_from_slice(&v.to_le_bytes());
                    }
                }
            }
        }
    }
}

// ---------------------------------------------------------------------------
// to 8-bit RGBA

fn half(h: u16) -> f32 {
    let sign = if h & 0x8000 != 0 { -1.0 } else { 1.0 };
    let exp = ((h >> 10) & 0x1f) as i32;
    let man = (h & 0x3ff) as f32;
    sign * match exp {
        0 => man * 2f32.powi(-24),
        31 => {
            if man == 0.0 {
                f32::INFINITY
            } else {
                f32::NAN
            }
        }
        e => (1.0 + man / 1024.0) * 2f32.powi(e - 15),
    }
}

/// Linear float to display byte (sRGB curve, clamped).
fn srgb(v: f32) -> u8 {
    let v = if v.is_finite() { v.clamp(0.0, 1.0) } else { 0.0 };
    let s = if v <= 0.0031308 { v * 12.92 } else { 1.055 * v.powf(1.0 / 2.4) - 0.055 };
    (s * 255.0 + 0.5) as u8
}

fn to_rgba(src: &[u8], pixels: usize, format: &str) -> Vec<u8> {
    let mut out = vec![255u8; pixels * 4];
    for p in 0..pixels {
        let o = &mut out[p * 4..p * 4 + 4];
        match format {
            "TSF_G8" => o[..3].fill(src[p]),
            "TSF_BGRA8" => {
                let s = &src[p * 4..p * 4 + 4];
                o.copy_from_slice(&[s[2], s[1], s[0], s[3]]);
            }
            "TSF_BGRE8" => {
                let s = &src[p * 4..p * 4 + 4];
                let scale = if s[3] == 0 { 0.0 } else { 2f32.powi(s[3] as i32 - 128 - 8) };
                for c in 0..3 {
                    o[c] = srgb(s[2 - c] as f32 * scale);
                }
            }
            "TSF_G16" => o[..3].fill(src[p * 2 + 1]),
            "TSF_RGBA16" => {
                for c in 0..4 {
                    o[c] = src[p * 8 + c * 2 + 1];
                }
            }
            "TSF_R16F" => {
                o[0] = srgb(half(u16::from_le_bytes([src[p * 2], src[p * 2 + 1]])));
                o[1] = 0;
                o[2] = 0;
            }
            "TSF_RGBA16F" => {
                for c in 0..4 {
                    let i = p * 8 + c * 2;
                    let v = half(u16::from_le_bytes([src[i], src[i + 1]]));
                    o[c] = if c == 3 { (v.clamp(0.0, 1.0) * 255.0 + 0.5) as u8 } else { srgb(v) };
                }
            }
            "TSF_R32F" => {
                o[0] = srgb(f32::from_le_bytes(src[p * 4..p * 4 + 4].try_into().unwrap()));
                o[1] = 0;
                o[2] = 0;
            }
            "TSF_RGBA32F" => {
                for c in 0..4 {
                    let i = p * 16 + c * 4;
                    let v = f32::from_le_bytes(src[i..i + 4].try_into().unwrap());
                    o[c] = if c == 3 { (v.clamp(0.0, 1.0) * 255.0 + 0.5) as u8 } else { srgb(v) };
                }
            }
            _ => {}
        }
    }
    out
}

/// Unpacked payloads of the last few packages, so stepping through mips and
/// tiles does not unpack the whole source again each time.
static PAYLOADS: OnceLock<Mutex<Vec<(String, Arc<Vec<u8>>)>>> = OnceLock::new();

/// The source payload of a package, unpacked. `key` names the file and its
/// version (path, size, mtime) for the cache.
pub fn payload(pkg: &Package, key: &str, roots: &[PathBuf]) -> Result<Arc<Vec<u8>>, String> {
    let cache = PAYLOADS.get_or_init(|| Mutex::new(Vec::new()));
    if let Some((_, p)) = cache.lock().map_err(|_| "cache poisoned")?.iter().find(|(k, _)| k == key) {
        return Ok(p.clone());
    }
    let raw = Arc::new(match local_payloads(&pkg.data, pkg.summary.payload_toc_offset) {
        Err(e) if e == NO_TRAILER => inline_payload(pkg, roots)?,
        payloads => {
            let payloads = payloads?;
            // A texture's source is its one big payload (others, if any, are small editor data).
            let largest = payloads.iter().max_by_key(|p| p.raw).ok_or("the package stores no source payload (virtualized?)")?;
            let buf = pkg.data.get(largest.offset..largest.offset + largest.compressed).ok_or("payload outside the file")?;
            decompress(buf, &|| oodle(roots))?
        }
    });
    let mut c = cache.lock().map_err(|_| "cache poisoned")?;
    c.push((key.to_string(), raw.clone()));
    if c.len() > 4 {
        c.remove(0);
    }
    Ok(raw)
}

// ---------------------------------------------------------------------------
// packages saved before the payload trailer

const UE5_MAIN_STREAM_VERSION: &str = "697DD581E64F41ABAA4A51ECBEB7B628";
const TEXTURE_SOURCE_VIRTUALIZATION: i32 = 42;

const BULKDATA_PAYLOAD_AT_END_OF_FILE: u32 = 1 << 0;
const BULKDATA_SERIALIZE_COMPRESSED_ZLIB: u32 = 1 << 1;
const BULKDATA_SIZE_64BIT: u32 = 1 << 13;
const BULKDATA_BAD_DATA_VERSION: u32 = 1 << 15;
const BULKDATA_NO_OFFSET_FIXUP: u32 = 1 << 16;

const EDITOR_BULK_IS_VIRTUALIZED: u32 = 1 << 0;
const EDITOR_BULK_REFERENCES_LEGACY_FILE: u32 = 1 << 2;
const EDITOR_BULK_LEGACY_FILE_IS_COMPRESSED: u32 = 1 << 3;
const EDITOR_BULK_STORED_IN_PACKAGE_TRAILER: u32 = 1 << 9;
const EDITOR_BULK_IS_COOKED: u32 = 1 << 10;

/// The source of a package without a trailer, which UTexture::Serialize writes after
/// the object's guid and strip flags: an FByteBulkData before TextureSourceVirtualization,
/// then an FEditorBulkData pointing at an FCompressedBuffer (BulkData.cpp, EditorBulkData.cpp).
fn inline_payload(pkg: &Package, roots: &[PathBuf]) -> Result<Vec<u8>, String> {
    let main = pkg.exports.iter().find(|e| e.is_asset).ok_or("no asset export")?;
    let (_, end) = read_export_properties_end(pkg, main, 0, false);
    let mut ar = Reader::new(pkg.export_data(main), &pkg.names);
    let header = (|| {
        ar.seek(end as i64)?;
        if ar.bool32()? {
            ar.guid()?; // FLazyObjectPtr::PossiblySerializeObjectGuid
        }
        ar.u16()?; // FStripDataFlags
        let flags = ar.u32()?;
        if pkg.summary.custom_version(UE5_MAIN_STREAM_VERSION) < TEXTURE_SOURCE_VIRTUALIZATION {
            let b = byte_bulk(pkg, &mut ar, main.serial_offset, flags)?;
            Ok((true, b.flags & BULKDATA_SERIALIZE_COMPRESSED_ZLIB != 0, b.at, b.stored, b.count))
        } else {
            ar.guid()?; // BulkDataId
            ar.bytes(20)?; // PayloadContentId
            let size = ar.i64()?;
            let offset = if flags & (EDITOR_BULK_IS_VIRTUALIZED | EDITOR_BULK_STORED_IN_PACKAGE_TRAILER | EDITOR_BULK_IS_COOKED) == 0 {
                ar.i64()?
            } else {
                -1
            };
            let legacy = flags & EDITOR_BULK_REFERENCES_LEGACY_FILE != 0;
            Ok((legacy, legacy && flags & EDITOR_BULK_LEGACY_FILE_IS_COMPRESSED != 0, offset, size, size))
        }
    })();
    let (legacy, zlib, at, stored, raw_len) = header.map_err(|e: super::archive::Error| format!("unreadable source header: {e}"))?;
    let tail = usize::try_from(at).ok().and_then(|at| pkg.data.get(at..)).ok_or("the source is not stored in the package (virtualized?)")?;
    if !legacy {
        return decompress(tail, &|| oodle(roots));
    }
    if zlib {
        return inflate_chunks(tail, raw_len as usize);
    }
    tail.get(..stored as usize).map(|b| b.to_vec()).ok_or("source payload outside the file".into())
}

/// Where an FByteBulkData (BulkData.cpp) keeps its bytes.
pub struct ByteBulk {
    pub flags: u32,
    /// Absolute offset in the package.
    pub at: i64,
    pub stored: i64,
    pub count: i64,
}

impl ByteBulk {
    /// Inline bulk data follows its header (a reader must skip `stored` bytes).
    pub fn is_inline(&self) -> bool {
        self.flags & BULKDATA_PAYLOAD_AT_END_OF_FILE == 0
    }

    pub fn bytes(&self, pkg: &Package) -> Result<Vec<u8>, String> {
        let tail = usize::try_from(self.at).ok().and_then(|at| pkg.data.get(at..)).ok_or("bulk data outside the file")?;
        if self.flags & BULKDATA_SERIALIZE_COMPRESSED_ZLIB != 0 {
            return inflate_chunks(tail, self.count.max(0) as usize);
        }
        tail.get(..self.stored.max(0) as usize).map(|b| b.to_vec()).ok_or("bulk data outside the file".into())
    }
}

/// An FByteBulkData header after its `flags`; `serial_offset` is where `ar`'s export starts.
pub(super) fn byte_bulk(pkg: &Package, ar: &mut Reader, serial_offset: i64, flags: u32) -> super::archive::Res<ByteBulk> {
    let (count, stored, offset) = if flags & BULKDATA_SIZE_64BIT != 0 {
        (ar.i64()?, ar.i64()?, ar.i64()?)
    } else {
        (ar.i32()? as i64, ar.i32()? as i64, ar.i64()?)
    };
    if flags & BULKDATA_BAD_DATA_VERSION != 0 {
        ar.u16()?;
    }
    let at = if flags & BULKDATA_PAYLOAD_AT_END_OF_FILE == 0 {
        serial_offset + ar.tell() as i64
    } else if flags & BULKDATA_NO_OFFSET_FIXUP == 0 {
        offset + pkg.summary.bulk_data_start_offset
    } else {
        offset
    };
    Ok(ByteBulk { flags, at, stored, count })
}

/// FArchive::SerializeCompressed: a header (file tag, chunk size), the total sizes,
/// each chunk's sizes, then the zlib chunks.
fn inflate_chunks(b: &[u8], raw_len: usize) -> Result<Vec<u8>, String> {
    let rd = |at: usize| b.get(at..at + 8).map(|s| i64::from_le_bytes(s.try_into().unwrap())).ok_or("truncated compressed source");
    if rd(0)? != PACKAGE_FILE_TAG as i64 {
        return Err("unsupported compressed source header".into());
    }
    let chunk = match rd(8)? {
        c if c == PACKAGE_FILE_TAG as i64 => 128 * 1024, // LOADING_COMPRESSION_CHUNK_SIZE
        c if c > 0 => c as usize,
        _ => return Err("bad compression chunk size".into()),
    };
    let total = rd(24)?.max(0) as usize;
    let chunks = total.div_ceil(chunk);
    let mut at = 32 + 16 * chunks;
    let mut out = Vec::with_capacity(total);
    for i in 0..chunks {
        let len = rd(32 + 16 * i)?.max(0) as usize;
        let src = b.get(at..at + len).ok_or("truncated compressed source")?;
        flate2::read::ZlibDecoder::new(src).read_to_end(&mut out).map_err(|e| format!("zlib: {e}"))?;
        at += len;
    }
    if out.len() != raw_len {
        return Err(format!("compressed source unpacked to {} bytes, expected {raw_len}", out.len()));
    }
    Ok(out)
}

/// Shrink to fit `max` texels a side by averaging boxes (0 = keep).
fn downscale(w: usize, h: usize, rgba: Vec<u8>, max: usize) -> (usize, usize, Vec<u8>) {
    let f = if max == 0 { 1 } else { w.max(h).div_ceil(max) };
    if f <= 1 {
        return (w, h, rgba);
    }
    let (ow, oh) = (w.div_ceil(f), h.div_ceil(f));
    let mut out = vec![0u8; ow * oh * 4];
    for oy in 0..oh {
        for ox in 0..ow {
            let mut sum = [0u32; 4];
            let mut n = 0u32;
            for y in oy * f..((oy + 1) * f).min(h) {
                for x in ox * f..((ox + 1) * f).min(w) {
                    let i = (y * w + x) * 4;
                    for c in 0..4 {
                        sum[c] += rgba[i + c] as u32;
                    }
                    n += 1;
                }
            }
            let o = (oy * ow + ox) * 4;
            for c in 0..4 {
                out[o + c] = (sum[c] / n.max(1)) as u8;
            }
        }
    }
    (ow, oh, out)
}

/// Decode one view (block, layer, source mip, slice) for display, at most `max`
/// texels a side (0 = full size). PNG/JPEG sources hold a single image and come
/// back as that file.
pub fn decode_view(
    pkg: &Package,
    key: &str,
    roots: &[PathBuf],
    view: (usize, usize, usize, usize),
    max: usize,
) -> Result<Pixels, String> {
    let l = source_layout(pkg).ok_or("not a texture with source data")?;
    let raw = payload(pkg, key, roots)?;
    match l.compression.as_str() {
        "TSCF_PNG" => return Ok(Pixels::Encoded { mime: "image/png", data: raw.to_vec() }),
        "TSCF_JPEG" => return Ok(Pixels::Encoded { mime: "image/jpeg", data: raw.to_vec() }),
        "TSCF_None" | "TSCF_UEDELTA" => {}
        other => return Err(format!("source compression {other} is not supported")),
    }
    let (block, layer, mip, slice) = view;
    let format = l.layers.get(layer).ok_or("no such layer")?.clone();
    let (bpp, sample) = bytes_per(&format).ok_or_else(|| format!("source format {format} is not supported"))?;
    let (start, w, h) = view_offset(&l, block, layer, mip, slice)?;
    let mut pixels = raw.get(start..start + w * h * bpp).ok_or("payload smaller than the source layout")?.to_vec();
    if l.compression == "TSCF_UEDELTA" {
        undelta(&mut pixels, w, h, bpp, sample);
    }
    let (w, h, rgba) = downscale(w, h, to_rgba(&pixels, w * h, &format), max);
    Ok(Pixels::Rgba { width: w as u32, height: h as u32, data: rgba })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn small_slices_are_one_tile_and_big_ones_are_cut_like_the_engine() {
        assert_eq!(tiles(128, 128, 4), vec![(0, 128, 0, 128)]);
        // 512x512 BGRA8: 2048-byte rows (no column cut), 262144 px / 32768 = 8 row cuts of 64.
        let t = tiles(512, 512, 4);
        assert_eq!(t.len(), 8);
        assert_eq!(t[1], (0, 512, 64, 64));
        // 2048 wide BGRA8: 8192-byte rows -> 2 columns of 1024 px.
        let t = tiles(2048, 256, 4);
        assert!(t.iter().all(|&(x, w, _, _)| (x == 0 || x == 1024) && w == 1024));
    }

    #[test]
    fn undelta_inverts_the_forward_transform() {
        // Forward per ImageCoreDelta: row = row - up (+ bias for 16-bit); here 16-bit RGBA16.
        let (w, h, bpp) = (3usize, 4usize, 8usize);
        let original: Vec<u8> = (0..w * h * bpp).map(|i| (i * 37 % 251) as u8).collect();
        let mut encoded = original.clone();
        for row in (1..h).rev() {
            for i in (0..w * bpp).step_by(2) {
                let cur = row * w * bpp + i;
                let up = cur - w * bpp;
                let a = u16::from_le_bytes([original[cur], original[cur + 1]]);
                let b = u16::from_le_bytes([original[up], original[up + 1]]);
                encoded[cur..cur + 2].copy_from_slice(&a.wrapping_sub(b).wrapping_add(0x8080).to_le_bytes());
            }
        }
        undelta(&mut encoded, w, h, bpp, 2);
        assert_eq!(encoded, original);
    }

    #[test]
    fn half_floats_decode() {
        assert_eq!(half(0x3c00), 1.0);
        assert_eq!(half(0xc000), -2.0);
        assert_eq!(half(0x0000), 0.0);
    }
}
