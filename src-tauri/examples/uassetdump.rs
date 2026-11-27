//! Dump a .uasset as JSON, shaped like the Python reader's
//! `uasset_dump.py --json --tags --imports --deps --graph --props all --raw-bytes 32`
//! output (mode `py`, the default), or as the document the asset diff compares
//! (mode `doc`).
//!
//!   cargo run --example uassetdump -- <file.uasset> [py|doc]

use p4gui_lib::uasset::archive::Val;
use p4gui_lib::uasset::graph::{connections, graph_val, read_graph};
use p4gui_lib::uasset::package::Package;
use p4gui_lib::uasset::properties::read_export_properties;

fn main() {
    let mut args = std::env::args().skip(1);
    let Some(path) = args.next() else {
        eprintln!("usage: uassetdump <file.uasset> [py|doc]");
        std::process::exit(2);
    };
    let mode = args.next().unwrap_or_else(|| "py".into());
    if mode == "anims" {
        // uassetdump <old> anims <new>: both revisions' animations paired for the time line.
        let other = args.next().expect("anims needs a second file");
        let read = |p: &str| p4gui_lib::uasset::moviescene::read(&Package::open(std::path::Path::new(p)).unwrap());
        let pairs = p4gui_lib::uasset::moviescene::pair(&read(&path), &read(&other));
        println!("{}", serde_json::to_string_pretty(&pairs).unwrap());
        return;
    }
    if mode == "layout" {
        // uassetdump <texture> layout: the source layout as one JSON line.
        let pkg = Package::open(std::path::Path::new(&path)).unwrap();
        println!("{}", serde_json::to_string(&p4gui_lib::uasset::texture::source_layout(&pkg)).unwrap());
        return;
    }
    if mode == "tex" {
        // uassetdump <texture> tex <out.png> <engine root> [block layer mip slice max]: one decoded view.
        let out = args.next().expect("tex needs an output file");
        let root = std::path::PathBuf::from(args.next().unwrap_or_else(|| ".".into()));
        let mut n = || args.next().and_then(|a| a.parse::<usize>().ok()).unwrap_or(0);
        let view = (n(), n(), n(), n());
        let max = n();
        let pkg = Package::open(std::path::Path::new(&path)).unwrap();
        let start = std::time::Instant::now();
        match p4gui_lib::uasset::texture::decode_view(&pkg, &path, &[root], view, max) {
            Ok(p4gui_lib::uasset::texture::Pixels::Rgba { width, height, data }) => {
                std::fs::write(&out, png(width, height, &data)).unwrap();
                println!("{view:?} -> {width}x{height} in {:?}", start.elapsed());
            }
            Ok(p4gui_lib::uasset::texture::Pixels::Encoded { mime, data }) => {
                std::fs::write(&out, data).unwrap();
                println!("{view:?} -> {mime} file");
            }
            Err(e) => println!("error: {e}"),
        }
        return;
    }
    if mode == "mesh" {
        // uassetdump <mesh> mesh <engine root> [other revision]: each source LOD's
        // geometry, and LOD 0 compared with the other revision.
        use p4gui_lib::uasset::mesh;
        let root = vec![std::path::PathBuf::from(args.next().unwrap_or_else(|| ".".into()))];
        let other = args.next();
        let pkg = Package::open(std::path::Path::new(&path)).unwrap();
        if std::env::var("MESH_VERSIONS").is_ok() {
            let v = |g: &str| pkg.summary.custom_version(g);
            println!(
                "ue4 {} ue5 {} editor {} release {} ue5main {}",
                pkg.summary.file_version_ue4,
                pkg.summary.file_version_ue5,
                v("E4B068EDF49442E9A231DA0B2E46BB41"),
                v("9C54D522A8264FBE9421074661B482D0"),
                v("697DD581E64F41ABAA4A51ECBEB7B628")
            );
        }
        let start = std::time::Instant::now();
        let mut first = None;
        for lod in mesh::source_lods(&pkg) {
            match mesh::geometry(&pkg, &lod, &root) {
                Ok(Some(g)) => {
                    let stats = Val::Map(mesh::stats(&g, &|_| None));
                    println!("{}: {}", lod.name, serde_json::to_string(&stats).unwrap());
                    first.get_or_insert(g);
                }
                Ok(None) => println!("{}: {}", lod.name, mesh::missing_source(&lod)),
                Err(e) => println!("{}: error: {e}", lod.name),
            }
        }
        println!("decoded in {:?}", start.elapsed());
        if let (Some(other), Some(a)) = (other, first) {
            let opkg = Package::open(std::path::Path::new(&other)).unwrap();
            let lods = mesh::source_lods(&opkg);
            if let Some(Ok(Some(b))) = lods.first().map(|l| mesh::geometry(&opkg, l, &root)) {
                let c = mesh::compare(&a, &b);
                println!(
                    "LOD 0: same topology {} moved {} (max {}) removed {} added {}",
                    c.same_topology, c.moved_vertices, c.max_move, c.removed_triangles, c.added_triangles
                );
            }
        }
        return;
    }
    if mode == "meshview" {
        // uassetdump <left> meshview <right> <out.bin> [lod]: the mesh_view command's answer.
        let right = args.next().expect("meshview needs a right file");
        let out = args.next().expect("meshview needs an output file");
        let lod = args.next().unwrap_or_default();
        let bytes = p4gui_lib::commands::mesh_view_bytes(&path, &right, &lod, &[std::path::PathBuf::from(".")]);
        std::fs::write(&out, &bytes).unwrap();
        println!("{} bytes", bytes.len());
        return;
    }
    if mode == "rows" {
        // uassetdump <left> rows <right>: the asset_diff command's answer, for a
        // browser test of the window without the app.
        let right = args.next().expect("rows needs a right file");
        let open = |p: &str| Package::open(std::path::Path::new(p)).unwrap();
        let (l, r) = (open(&path), open(&right));
        use p4gui_lib::uasset::{canvas, doc, graph::read_graph};
        let roots = [std::path::PathBuf::from(".")];
        let (ldoc, rdoc) = (doc::document_in(&l, &roots), doc::document_in(&r, &roots));
        let rows = p4gui_lib::uasset::diff::diff(&ldoc, &rdoc);
        let changes = rows.iter().filter(|r| !r.kids && r.status != p4gui_lib::uasset::diff::Status::Same).count();
        let graphs = canvas::pair(
            canvas::sides(&l, &read_graph(&l)),
            canvas::sides(&r, &read_graph(&r)),
            &|g: &str, n: &str| doc::graph_node(&ldoc, g, n).cloned(),
            &|g: &str, n: &str| doc::graph_node(&rdoc, g, n).cloned(),
        );
        let out = serde_json::json!({
            "rows": rows, "changes": changes, "leftError": "", "rightError": "",
            "leftThumb": p4gui_lib::uasset::doc::thumbnail_url(&l),
            "rightThumb": p4gui_lib::uasset::doc::thumbnail_url(&r),
            "class": p4gui_lib::uasset::doc::main_class(&r),
            "rawChanges": 0,
            "graphs": graphs,
        });
        println!("{out}");
        return;
    }
    if mode == "diff" {
        // uassetdump <left> diff <right>: the rows that differ, indented.
        let right = args.next().expect("diff needs a right file");
        let roots = [std::path::PathBuf::from(".")];
        let open = |p: &str| p4gui_lib::uasset::doc::document_in(&Package::open(std::path::Path::new(p)).unwrap(), &roots);
        let rows = p4gui_lib::uasset::diff::diff(&open(&path), &open(&right));
        for r in rows.iter().filter(|r| r.status != p4gui_lib::uasset::diff::Status::Same) {
            let text = |s: &Option<String>| s.as_deref().unwrap_or("").chars().take(70).collect::<String>();
            println!("{}{} [{:?}] {} | {}", "  ".repeat(r.depth as usize), r.key, r.status, text(&r.left), text(&r.right));
        }
        return;
    }
    let pkg = match Package::open(std::path::Path::new(&path)) {
        Ok(p) => p,
        Err(e) => {
            eprintln!("error: {e}");
            std::process::exit(2);
        }
    };
    let roots = [std::path::PathBuf::from(".")];
    let out = if mode == "doc" { p4gui_lib::uasset::doc::document_in(&pkg, &roots) } else { python_shape(&pkg) };
    println!("{}", serde_json::to_string(&out).unwrap());
}

fn python_shape(pkg: &Package) -> Val {
    let registry = pkg
        .asset_data
        .iter()
        .map(|a| {
            Val::map(vec![
                ("object_path", Val::str(&a.object_path)),
                ("class", Val::str(&a.class)),
                ("tags", Val::Map(a.tags.iter().map(|(k, v)| (k.clone(), Val::str(v))).collect())),
            ])
        })
        .collect();
    let imports = pkg
        .imports
        .iter()
        .enumerate()
        .map(|(i, imp)| {
            Val::map(vec![
                ("index", Val::Int(i as i64)),
                ("class", Val::str(&imp.class_name)),
                ("path", Val::Str(pkg.import_path(i))),
            ])
        })
        .collect();
    let mut hard: Vec<String> = Vec::new();
    for imp in &pkg.imports {
        if imp.outer_index == 0 && imp.class_name == "Package" && !hard.contains(&imp.object_name) {
            hard.push(imp.object_name.clone());
        }
    }
    let strings = |v: &[String]| Val::List(v.iter().map(Val::str).collect());
    let graph = read_graph(pkg);
    let links = connections(&graph)
        .into_iter()
        .map(|(a, b, c, d)| {
            Val::map(vec![
                ("from_node", Val::Str(a)),
                ("from_pin", Val::Str(b)),
                ("to_node", Val::Str(c)),
                ("to_pin", Val::Str(d)),
            ])
        })
        .collect();
    let properties = pkg
        .exports
        .iter()
        .enumerate()
        .map(|(i, e)| (pkg.export_path(i), read_export_properties(pkg, e, 32, false)))
        .collect();
    Val::map(vec![
        ("asset_registry", Val::List(registry)),
        ("imports", Val::List(imports)),
        ("hard_dependencies", strings(&hard)),
        ("soft_package_references", strings(&pkg.soft_package_references)),
        ("soft_object_paths", strings(&pkg.soft_object_paths)),
        ("graph", graph_val(&graph)),
        ("connections", Val::List(links)),
        ("properties", Val::Map(properties)),
    ])
}

/// A minimal RGBA8 PNG (stored rows, zlib), for looking at a decoded texture.
fn png(width: u32, height: u32, rgba: &[u8]) -> Vec<u8> {
    use std::io::Write;
    fn crc(data: &[u8]) -> u32 {
        let mut c = 0xffff_ffffu32;
        for &b in data {
            c ^= b as u32;
            for _ in 0..8 {
                c = if c & 1 != 0 { 0xedb8_8320 ^ (c >> 1) } else { c >> 1 };
            }
        }
        !c
    }
    fn chunk(out: &mut Vec<u8>, kind: &[u8], body: &[u8]) {
        out.extend((body.len() as u32).to_be_bytes());
        let start = out.len();
        out.extend(kind);
        out.extend(body);
        let c = crc(&out[start..]);
        out.extend(c.to_be_bytes());
    }
    let mut raw = Vec::with_capacity(rgba.len() + height as usize);
    for row in rgba.chunks(width as usize * 4) {
        raw.push(0);
        raw.extend(row);
    }
    let mut z = flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::fast());
    z.write_all(&raw).unwrap();
    let mut out = b"\x89PNG\r\n\x1a\n".to_vec();
    let mut ihdr = Vec::new();
    ihdr.extend(width.to_be_bytes());
    ihdr.extend(height.to_be_bytes());
    ihdr.extend([8, 6, 0, 0, 0]);
    chunk(&mut out, b"IHDR", &ihdr);
    chunk(&mut out, b"IDAT", &z.finish().unwrap());
    chunk(&mut out, b"IEND", &[]);
    out
}
