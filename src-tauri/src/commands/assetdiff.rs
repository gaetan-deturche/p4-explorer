//! The in-app asset diff: two revisions of a .uasset / .umap compared object by
//! object and property by property, read straight from the files (no editor).

use serde::Serialize;
use tauri::{AppHandle, WebviewUrl, WebviewWindowBuilder};

use super::diffwin::{enc, DiffPair};
use crate::p4::P4Conn;
use crate::uasset::archive::Val;
use crate::uasset::canvas::{self, GraphPair};
use crate::uasset::diff::{diff, Row, Status};
use crate::uasset::doc::{document_in, graph_node, main_class, thumbnail_url, ALL_DATA};
use crate::uasset::graph::read_graph;
use crate::uasset::package::Package;

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AssetDiff {
    rows: Vec<Row>,
    /// Rows that differ, leaves only, outside "All data"...
    changes: usize,
    /// ...and inside it, when a summary leads.
    raw_changes: usize,
    /// Why a side could not be read; empty when it was (or is absent).
    left_error: String,
    right_error: String,
    left_thumb: Option<String>,
    right_thumb: Option<String>,
    class: String,
    /// Each graph, both revisions drawn side by side.
    graphs: Vec<GraphPair>,
}

struct Side {
    doc: Val,
    error: String,
    thumb: Option<String>,
    class: String,
    graphs: Vec<(String, canvas::Side)>,
}

/// An absent side (an add, a delete) is an empty document, not an error.
fn read_side(path: &str, roots: &[std::path::PathBuf]) -> Side {
    let empty =
        |error: String| Side { doc: Val::Map(Vec::new()), error, thumb: None, class: String::new(), graphs: Vec::new() };
    if path.is_empty() || std::fs::metadata(path).map(|m| m.len() == 0).unwrap_or(true) {
        return empty(String::new());
    }
    match Package::open(std::path::Path::new(path)) {
        Ok(pkg) => Side {
            doc: document_in(&pkg, roots),
            error: String::new(),
            thumb: thumbnail_url(&pkg),
            class: main_class(&pkg),
            graphs: canvas::sides(&pkg, &read_graph(&pkg)),
        },
        Err(e) => empty(e),
    }
}

/// Where to look for the engine (its Oodle unpacks texture and mesh sources):
/// the window's workspace, then the files' own folders.
fn engine_roots(job: &str, paths: &[&str]) -> Vec<std::path::PathBuf> {
    let root = asset_jobs().lock().ok().and_then(|j| j.get(job).map(|j| j.root.clone())).unwrap_or_default();
    let mut roots: Vec<std::path::PathBuf> = paths.iter().filter(|p| !p.is_empty()).map(std::path::PathBuf::from).collect();
    if !root.is_empty() {
        roots.insert(0, std::path::PathBuf::from(root));
    }
    roots
}

/// Compare two package files on disk.
#[tauri::command]
pub async fn asset_diff(left: String, right: String, job: Option<String>) -> Result<AssetDiff, String> {
    let roots = engine_roots(job.as_deref().unwrap_or(""), &[&left, &right]);
    tauri::async_runtime::spawn_blocking(move || {
        let (l, r) = std::thread::scope(|s| {
            let l = s.spawn(|| read_side(&left, &roots));
            let r = read_side(&right, &roots);
            (l.join().unwrap_or_else(|_| read_side("", &[])), r)
        });
        let rows = diff(&l.doc, &r.doc);
        let graphs = canvas::pair(
            l.graphs,
            r.graphs,
            &|g: &str, n: &str| graph_node(&l.doc, g, n).cloned(),
            &|g: &str, n: &str| graph_node(&r.doc, g, n).cloned(),
        );
        let (mut changes, mut raw_changes, mut in_raw) = (0, 0, false);
        for row in &rows {
            if row.depth == 0 {
                in_raw = row.key == ALL_DATA;
            }
            if !row.kids && row.status != Status::Same {
                if in_raw {
                    raw_changes += 1;
                } else {
                    changes += 1;
                }
            }
        }
        AssetDiff {
            rows,
            changes,
            raw_changes,
            left_error: l.error,
            right_error: r.error,
            left_thumb: l.thumb,
            right_thumb: r.thumb,
            class: if r.class.is_empty() { l.class } else { r.class },
            graphs,
        }
    })
    .await
    .map_err(|e| e.to_string())
}

/// What an asset diff window needs to hand the same pair to Unreal, and the
/// workspace root whose engine holds the Oodle library textures need.
#[derive(Serialize, Clone)]
pub struct AssetJob {
    pub conn: P4Conn,
    pub name: String,
    pub root: String,
}

fn asset_jobs() -> &'static std::sync::Mutex<std::collections::HashMap<String, AssetJob>> {
    static R: std::sync::OnceLock<std::sync::Mutex<std::collections::HashMap<String, AssetJob>>> =
        std::sync::OnceLock::new();
    R.get_or_init(|| std::sync::Mutex::new(std::collections::HashMap::new()))
}

#[tauri::command]
pub fn asset_diff_job(job: String) -> Result<AssetJob, String> {
    asset_jobs()
        .lock()
        .map_err(|_| "window registry poisoned")?
        .get(&job)
        .cloned()
        .ok_or_else(|| format!("no asset diff job named {job}"))
}

/// Open the asset diff window on the `/assetdiff` route, one window per call.
#[tauri::command]
pub async fn open_asset_diff_window(app: AppHandle, pair: DiffPair, conn: P4Conn, name: String) -> Result<(), String> {
    static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);
    let label = format!("assetdiff-{}", NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed));
    let info_conn = conn.clone();
    let root = tauri::async_runtime::spawn_blocking(move || crate::p4::run(&info_conn, &["info"]))
        .await
        .ok()
        .and_then(|r| r.ok())
        .and_then(|recs| recs.first().and_then(|r| r.get("clientRoot")).and_then(|v| v.as_str()).map(String::from))
        .unwrap_or_default();
    asset_jobs()
        .lock()
        .map_err(|_| "window registry poisoned")?
        .insert(label.clone(), AssetJob { conn, name, root });
    let url = format!(
        "assetdiff?left={}&right={}&ll={}&rl={}&title={}&job={}",
        enc(&pair.left),
        enc(&pair.right),
        enc(&pair.left_label),
        enc(&pair.right_label),
        enc(&pair.title),
        enc(&label),
    );
    let win = WebviewWindowBuilder::new(&app, &label, WebviewUrl::App(url.into()))
        .title(format!("Asset diff — {}", pair.title))
        .inner_size(1280.0, 800.0)
        .min_inner_size(700.0, 400.0)
        .visible(false) // shown by wingeom::apply, already at its remembered spot
        .build()
        .map_err(|e| format!("failed to open asset diff window: {e}"))?;
    crate::wingeom::apply(&win, "assetdiff");
    Ok(())
}

/// A texture's source layout (UDIM blocks, layers, mips, slices), or None for
/// a side that does not exist or holds no texture source.
#[tauri::command]
pub async fn texture_layout(path: String) -> Result<Option<crate::uasset::texture::Layout>, String> {
    tauri::async_runtime::spawn_blocking(move || {
        if path.is_empty() || std::fs::metadata(&path).map(|m| m.len() == 0).unwrap_or(true) {
            return Ok(None);
        }
        let pkg = Package::open(std::path::Path::new(&path))?;
        Ok(crate::uasset::texture::source_layout(&pkg))
    })
    .await
    .map_err(|e| e.to_string())?
}

/// One view of a texture's source for the viewer — UDIM block, layer, source
/// mip, slice, at most `max` texels a side (0 = full) — as one binary message:
/// kind (0 RGBA8, 1 PNG file, 2 JPEG file), width and height (u32 LE), then the
/// pixels or the file.
#[tauri::command]
#[allow(clippy::too_many_arguments)]
pub async fn texture_pixels(
    path: String,
    job: String,
    block: usize,
    layer: usize,
    mip: usize,
    slice: usize,
    max: usize,
) -> Result<tauri::ipc::Response, String> {
    let roots = engine_roots(&job, &[&path]);
    tauri::async_runtime::spawn_blocking(move || {
        use crate::uasset::texture::{decode_view, Pixels};
        let meta = std::fs::metadata(&path).map_err(|e| format!("{path}: {e}"))?;
        let key = format!("{path}|{}|{:?}", meta.len(), meta.modified().ok());
        let pkg = Package::open(std::path::Path::new(&path))?;
        let (kind, w, h, data) = match decode_view(&pkg, &key, &roots, (block, layer, mip, slice), max)? {
            Pixels::Rgba { width, height, data } => (0u8, width, height, data),
            Pixels::Encoded { mime, data } => (if mime == "image/png" { 1 } else { 2 }, 0, 0, data),
        };
        let mut out = Vec::with_capacity(data.len() + 9);
        out.push(kind);
        out.extend(w.to_le_bytes());
        out.extend(h.to_le_bytes());
        out.extend(data);
        Ok(tauri::ipc::Response::new(out))
    })
    .await
    .map_err(|e| e.to_string())?
}

/// One revision's LOD for the 3D view, and why there is none when there is not.
struct MeshSide {
    geometry: Option<std::sync::Arc<crate::uasset::mesh::Geometry>>,
    error: String,
    lods: Vec<String>,
    materials: Vec<(String, String, String)>,
}

fn mesh_side(path: &str, lod: &str, roots: &[std::path::PathBuf]) -> Option<MeshSide> {
    use crate::uasset::mesh;
    if path.is_empty() || std::fs::metadata(path).map(|m| m.len() == 0).unwrap_or(true) {
        return None;
    }
    let pkg = match Package::open(std::path::Path::new(path)) {
        Ok(p) => p,
        Err(error) => return Some(MeshSide { geometry: None, error, lods: Vec::new(), materials: Vec::new() }),
    };
    let lods = mesh::source_lods(&pkg);
    let names = lods.iter().filter(|l| l.source.is_some()).map(|l| l.name.clone()).collect();
    let materials = pkg.exports.iter().position(|e| e.is_asset).map(|m| mesh::material_slots(&pkg, m)).unwrap_or_default();
    let (geometry, error) = match lods.iter().find(|l| l.name == lod) {
        Some(l) => match mesh::geometry(&pkg, l, roots) {
            Ok(Some(g)) => (Some(g), String::new()),
            Ok(None) => (None, mesh::missing_source(l).into()),
            Err(e) => (None, e),
        },
        None => (None, format!("no {lod} in this revision")),
    };
    Some(MeshSide { geometry, error, lods: names, materials })
}

/// Both revisions of one mesh LOD for the 3D view, as one binary message: the
/// JSON header's length (u32 LE) and the header, then for each side that has
/// geometry its positions (f32 x3), triangles (u32 x3), each triangle's section
/// (u32) and status (u8: same, only on this side, moved), every block starting
/// on a 4-byte boundary.
#[tauri::command]
pub async fn mesh_view(left: String, right: String, job: String, lod: String) -> Result<tauri::ipc::Response, String> {
    let roots = engine_roots(&job, &[&left, &right]);
    tauri::async_runtime::spawn_blocking(move || tauri::ipc::Response::new(mesh_view_bytes(&left, &right, &lod, &roots)))
        .await
        .map_err(|e| e.to_string())
}

/// `mesh_view`'s answer (pub for the uassetdump example).
pub fn mesh_view_bytes(left: &str, right: &str, lod: &str, roots: &[std::path::PathBuf]) -> Vec<u8> {
    use crate::uasset::mesh;
    let lod = if lod.is_empty() { "LOD 0" } else { lod }.to_string();
    let (l, r) = std::thread::scope(|s| {
        let l = s.spawn(|| mesh_side(left, &lod, roots));
        let r = mesh_side(right, &lod, roots);
        (l.join().unwrap_or(None), r)
    });
    let geometry = |s: &Option<MeshSide>| s.as_ref().and_then(|s| s.geometry.clone());
    let (gl, gr) = (geometry(&l), geometry(&r));
    let comparison = match (&gl, &gr) {
        (Some(a), Some(b)) => Some(mesh::compare(a, b)),
        _ => None,
    };
    // A side alone is all added (or removed); next to an unreadable one it is just shown.
    let alone = |g: &mesh::Geometry, other: &Option<MeshSide>| {
        vec![if other.is_none() { mesh::ONE_SIDE } else { mesh::SAME }; g.triangles.len()]
    };
    let status_l = comparison.as_ref().map(|c| c.left.clone()).or_else(|| gl.as_ref().map(|g| alone(g, &r)));
    let status_r = comparison.as_ref().map(|c| c.right.clone()).or_else(|| gr.as_ref().map(|g| alone(g, &l)));
    let mut lods: Vec<String> = Vec::new();
    for s in [&l, &r].into_iter().flatten() {
        for n in &s.lods {
            if !lods.contains(n) {
                lods.push(n.clone());
            }
        }
    }
    let header_side = |s: &Option<MeshSide>| -> serde_json::Value {
        let Some(s) = s else { return serde_json::Value::Null };
        let Some(g) = &s.geometry else { return serde_json::json!({ "error": s.error }) };
        let sections: Vec<serde_json::Value> = g
            .sections
            .iter()
            .map(|n| serde_json::json!({ "name": n, "material": mesh::slot_material(&s.materials, n) }))
            .collect();
        let (lo, hi) = g.bounds();
        serde_json::json!({
            "vertices": g.positions.len(),
            "triangles": g.triangles.len(),
            "sections": sections,
            "min": lo,
            "max": hi,
        })
    };
    let header = serde_json::json!({
        "lod": lod,
        "lods": lods,
        "left": header_side(&l),
        "right": header_side(&r),
        "compare": comparison.as_ref().map(|c| serde_json::json!({
            "sameTopology": c.same_topology,
            "movedVertices": c.moved_vertices,
            "maxMove": c.max_move,
            "removedTriangles": c.removed_triangles,
            "addedTriangles": c.added_triangles,
        })),
    })
    .to_string();
    let pad = |out: &mut Vec<u8>| out.resize(out.len().div_ceil(4) * 4, 0);
    let mut out = Vec::new();
    out.extend((header.len() as u32).to_le_bytes());
    out.extend(header.as_bytes());
    pad(&mut out);
    for (g, status) in [(gl, status_l), (gr, status_r)] {
        let (Some(g), Some(status)) = (g, status) else { continue };
        out.reserve(g.positions.len() * 12 + g.triangles.len() * 17 + 4);
        out.extend_from_slice(mesh::bytes_of(&g.positions));
        out.extend_from_slice(mesh::bytes_of(&g.triangles));
        out.extend_from_slice(mesh::bytes_of(&g.triangle_sections));
        out.extend(&status);
        pad(&mut out);
    }
    out
}
