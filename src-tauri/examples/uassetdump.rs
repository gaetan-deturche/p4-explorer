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
    if mode == "rows" {
        // uassetdump <left> rows <right>: the asset_diff command's answer, for a
        // browser test of the window without the app.
        let right = args.next().expect("rows needs a right file");
        let open = |p: &str| Package::open(std::path::Path::new(p)).unwrap();
        let (l, r) = (open(&path), open(&right));
        let rows = p4gui_lib::uasset::diff::diff(&p4gui_lib::uasset::doc::document(&l), &p4gui_lib::uasset::doc::document(&r));
        let changes = rows.iter().filter(|r| !r.kids && r.status != p4gui_lib::uasset::diff::Status::Same).count();
        let out = serde_json::json!({
            "rows": rows, "changes": changes, "leftError": "", "rightError": "",
            "leftThumb": p4gui_lib::uasset::doc::thumbnail_url(&l),
            "rightThumb": p4gui_lib::uasset::doc::thumbnail_url(&r),
            "class": p4gui_lib::uasset::doc::main_class(&r),
        });
        println!("{out}");
        return;
    }
    if mode == "diff" {
        // uassetdump <left> diff <right>: the rows that differ, indented.
        let right = args.next().expect("diff needs a right file");
        let open = |p: &str| p4gui_lib::uasset::doc::document(&Package::open(std::path::Path::new(p)).unwrap());
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
    let out = if mode == "doc" { p4gui_lib::uasset::doc::document(&pkg) } else { python_shape(&pkg) };
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
