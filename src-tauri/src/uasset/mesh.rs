//! A mesh's source geometry, read from its package: each LOD keeps its
//! FMeshDescription as an editor bulk payload (FMeshDescriptionBulkData), in the
//! package trailer since 5.0; a mesh saved by UE4 keeps it in its own data, as
//! an FMeshDescription of the old format or, before 4.24, an FRawMesh. Decoded
//! here into vertices, triangles and material sections, for the summary's counts
//! and the 3D view, and two revisions of one LOD compared triangle by triangle.
//!
//! Layouts follow MeshDescription.cpp, MeshElementContainer.h, MeshAttributeArray.h,
//! AttributeArrayContainer.h, StaticMesh.cpp, StaticMeshSourceData.cpp, RawMesh.cpp
//! and SkeletalMesh.cpp (materials, reference skeleton) in UE 5.6.

use std::collections::HashMap;
use std::hash::{BuildHasherDefault, Hasher};
use std::path::PathBuf;
use std::sync::{Arc, Mutex, OnceLock};

use super::archive::{archive_err, Reader, Res, Val};
use super::doc::properties;
use super::package::Package;
use super::properties::read_export_properties_end;
use super::texture::{byte_bulk, decompress, local_payloads, oodle, ByteBulk};

/// One source model of a mesh: its label and where its geometry is.
pub struct SourceLod {
    pub name: String,
    /// None when there is no geometry (see `missing_source`).
    pub source: Option<Source>,
    /// The source model's saved settings.
    pub settings: Val,
}

pub enum Source {
    /// A mesh description bulk-data export (UE5).
    Export(usize),
    /// In the mesh's own data (UE4): an FRawMesh, or an FMeshDescription of the old format.
    Inline { raw_mesh: bool, bulk: ByteBulk },
    /// The mesh's own data could not be walked to it.
    Unreadable(String),
}

const UE5_MAIN_VERSION: &str = "697DD581E64F41ABAA4A51ECBEB7B628";
const EDITOR_VERSION: &str = "E4B068EDF49442E9A231DA0B2E46BB41";
const RELEASE_VERSION: &str = "9C54D522A8264FBE9421074661B482D0";
const RENDERING_VERSION: &str = "12F88B9F88754AFCA67CD90C383ABD29";
const ENTERPRISE_VERSION: &str = "9DFFBCD6494F0158E22112823C92A888";
const FORTNITE_MAIN_VERSION: &str = "601D1886AC644F84AA16D3DE0DEAC7D6";

// FUE5MainStreamObjectVersion
const MESH_DESCRIPTION_NEW_FORMAT: i32 = 4;
const SERIALIZE_MESH_DESCRIPTION_BASE: i32 = 25;
// FEditorObjectVersion
const STATIC_MESH_DEPRECATED_RAW_MESH: i32 = 28;
const MESH_DESCRIPTION_BULK_DATA_GUID: i32 = 29;
const MESH_DESCRIPTION_REMOVED_HOLES: i32 = 30;
const MESH_DESCRIPTION_TRIANGLES: i32 = 35;
// FReleaseObjectVersion
const MESH_DESCRIPTION_NEW_SERIALIZATION: i32 = 17;
/// FReleaseObjectVersion::MeshDescriptionNewFormat, which only UE5 early access saved.
const RELEASE_MESH_DESCRIPTION_NEW_FORMAT: i32 = 31;
// FRenderingObjectVersion, FEnterpriseObjectVersion, FFortniteMainBranchObjectVersion
const DEPRECATED_HIGH_RES_SOURCE_MESH: i32 = 49;
const MESH_DESCRIPTION_BULK_DATA_GUID_IS_HASH: i32 = 8;
const MESH_DESCRIPTION_FOR_SKELETAL_MESH: i32 = 133;
// EUnrealEngineObjectUE4Version
const VER_UE4_STATIC_MESH_STORE_NAV_COLLISION: i32 = 216;
const VER_UE4_DEPRECATED_STATIC_MESH_THUMBNAIL_PROPERTIES_REMOVED: i32 = 242;

/// The mesh's source models in LOD order, then the Nanite hi-res one when it has geometry.
pub fn source_lods(pkg: &Package) -> Vec<SourceLod> {
    let Some(main) = pkg.exports.iter().position(|e| e.is_asset) else { return Vec::new() };
    let props = properties(pkg, main);
    let export_of = |v: Option<&Val>| -> Option<usize> {
        let path = v?.as_str()?;
        (0..pkg.exports.len()).find(|&i| pkg.export_path(i) == path)
    };
    let mut out = Vec::new();
    if let Some(Val::List(models)) = props.get("SourceModels") {
        let static_mesh = super::doc::leaf(&pkg.export_class(&pkg.exports[main])) == "StaticMesh";
        let mut inline = (static_mesh && pkg.summary.custom_version(UE5_MAIN_VERSION) < SERIALIZE_MESH_DESCRIPTION_BASE)
            .then(|| inline_sources(pkg, main, models.len()));
        for (i, m) in models.iter().enumerate() {
            let source = match &mut inline {
                Some(Ok(found)) => found.get_mut(i).and_then(Option::take),
                Some(Err(e)) => Some(Source::Unreadable(e.clone())),
                None => {
                    let bulk = m.get("StaticMeshDescriptionBulkData").or(m.get("MeshDescriptionBulkData"));
                    export_of(bulk).filter(|&e| bulk_header(pkg, e).is_ok_and(|h| h.size > 0)).map(Source::Export)
                }
            };
            out.push(SourceLod { name: format!("LOD {i}"), source, settings: m.clone() });
        }
    }
    if let Some(hi) = props.get("HiResSourceModel") {
        if let Some(e) = export_of(hi.get("StaticMeshDescriptionBulkData")) {
            if bulk_header(pkg, e).is_ok_and(|h| h.size > 0) {
                out.push(SourceLod { name: HI_RES.into(), source: Some(Source::Export(e)), settings: hi.clone() });
            }
        }
    }
    out
}

pub const HI_RES: &str = "Nanite hi-res";

/// Why a source LOD has no geometry.
pub fn missing_source(lod: &SourceLod) -> &'static str {
    if lod.name == "LOD 0" {
        "no source geometry"
    } else {
        "generated by reduction"
    }
}

/// Each source model's bulk data in a UE4 mesh's own data, which UStaticMesh::Serialize
/// writes after its properties: strip flags, cooked flag, body setup, nav collision,
/// lighting guid, sockets, then per source model FStaticMeshSourceModel::SerializeBulkData.
fn inline_sources(pkg: &Package, main: usize, models: usize) -> Result<Vec<Option<Source>>, String> {
    let s = &pkg.summary;
    let editor = s.custom_version(EDITOR_VERSION);
    let raw_mesh = editor < STATIC_MESH_DEPRECATED_RAW_MESH;
    let exp = &pkg.exports[main];
    let (_, end) = read_export_properties_end(pkg, exp, 0, false);
    let mut ar = Reader::new(pkg.export_data(exp), &pkg.names);
    (|| -> Res<Vec<Option<Source>>> {
        ar.seek(end as i64)?;
        if ar.bool32()? {
            ar.guid()?;
        }
        let global_strip = ar.u8()?;
        ar.u8()?; // class strip flags
        if global_strip & 1 != 0 {
            return Ok(Vec::new()); // editor data stripped
        }
        ar.bool32()?; // bCooked
        ar.package_index()?; // BodySetup
        if s.file_version_ue4 >= VER_UE4_STATIC_MESH_STORE_NAV_COLLISION {
            ar.package_index()?;
        }
        if s.file_version_ue4 < VER_UE4_DEPRECATED_STATIC_MESH_THUMBNAIL_PROPERTIES_REMOVED {
            ar.skip(16)?; // thumbnail angle and distance
        }
        if s.custom_version(RENDERING_VERSION) < DEPRECATED_HIGH_RES_SOURCE_MESH {
            ar.fstring()?;
            ar.u32()?;
        }
        ar.guid()?; // LightingGuid
        let sockets = ar.count(1 << 16)?;
        ar.skip(sockets * 4)?;
        let mut out = Vec::with_capacity(models);
        for _ in 0..models {
            // An FRawMeshBulkData; or a valid flag and an FMeshDescriptionBulkData.
            if !raw_mesh && !ar.bool32()? {
                out.push(None);
                continue;
            }
            let flags = ar.u32()?;
            let bulk = byte_bulk(pkg, &mut ar, exp.serial_offset, flags)?;
            if bulk.is_inline() {
                ar.skip(bulk.stored.max(0) as usize)?;
            }
            if raw_mesh || editor >= MESH_DESCRIPTION_BULK_DATA_GUID {
                ar.guid()?;
            }
            if raw_mesh || s.custom_version(ENTERPRISE_VERSION) >= MESH_DESCRIPTION_BULK_DATA_GUID_IS_HASH {
                ar.bool32()?; // bGuidIsHash
            }
            out.push((bulk.count > 0).then_some(Source::Inline { raw_mesh, bulk }));
        }
        Ok(out)
    })()
    .map_err(|e| format!("unreadable mesh data: {e}"))
}

/// The mesh's material slots: (slot name, imported slot name, material). A
/// static mesh saves them as a property (`Materials` before 4.14), a skeletal
/// mesh natively.
pub fn material_slots(pkg: &Package, main: usize) -> Vec<(String, String, String)> {
    let props = properties(pkg, main);
    if let Some(Val::List(l)) = props.get("StaticMaterials") {
        return l
            .iter()
            .map(|m| {
                let s = |k: &str| m.get(k).and_then(|v| v.as_str()).unwrap_or("None").to_string();
                (s("MaterialSlotName"), s("ImportedMaterialSlotName"), s("MaterialInterface"))
            })
            .collect();
    }
    if let Some(Val::List(l)) = props.get("Materials") {
        return l
            .iter()
            .enumerate()
            .map(|(i, m)| (format!("Material {i}"), format!("Material {i}"), m.as_str().unwrap_or("None").to_string()))
            .collect();
    }
    if super::doc::leaf(&pkg.export_class(&pkg.exports[main])) == "SkeletalMesh" {
        return skeletal_native(pkg, main).map(|n| n.materials).unwrap_or_default();
    }
    Vec::new()
}

/// A section's material: the slot whose imported name (or name) is the polygon group's.
pub fn slot_material(slots: &[(String, String, String)], group: &str) -> Option<String> {
    slots.iter().find(|s| s.1 == group || s.0 == group).map(|s| s.2.clone())
}

// ---------------------------------------------------------------------------
// the payload

const EDITOR_BULK_IS_VIRTUALIZED: u32 = 1 << 0;
const EDITOR_BULK_STORED_IN_PACKAGE_TRAILER: u32 = 1 << 9;
const EDITOR_BULK_IS_COOKED: u32 = 1 << 10;

struct BulkHeader {
    id: [u8; 20],
    size: i64,
    flags: u32,
    offset: i64,
}

/// The FEditorBulkData an FMeshDescriptionBulkData opens with, after the
/// object's properties and guid (EditorBulkData.cpp).
fn bulk_header(pkg: &Package, export: usize) -> Result<BulkHeader, String> {
    let exp = &pkg.exports[export];
    let (_, end) = read_export_properties_end(pkg, exp, 0, false);
    let mut ar = Reader::new(pkg.export_data(exp), &pkg.names);
    (|| -> Res<BulkHeader> {
        ar.seek(end as i64)?;
        if ar.bool32()? {
            ar.guid()?;
        }
        let flags = ar.u32()?;
        ar.guid()?; // BulkDataId
        let id: [u8; 20] = ar.bytes(20)?.try_into().unwrap();
        let size = ar.i64()?;
        let offset = if flags & (EDITOR_BULK_IS_VIRTUALIZED | EDITOR_BULK_STORED_IN_PACKAGE_TRAILER | EDITOR_BULK_IS_COOKED) == 0 {
            ar.i64()?
        } else {
            -1
        };
        Ok(BulkHeader { id, size, flags, offset })
    })()
    .map_err(|e| format!("unreadable mesh description header: {e}"))
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// The unpacked mesh description, or None when the LOD has none.
fn payload(pkg: &Package, h: &BulkHeader, roots: &[PathBuf]) -> Result<Option<Vec<u8>>, String> {
    if h.size == 0 {
        return Ok(None);
    }
    if h.flags & EDITOR_BULK_IS_VIRTUALIZED != 0 {
        return Err("the mesh description is virtualized (not stored in the package)".into());
    }
    let buf = if h.flags & EDITOR_BULK_STORED_IN_PACKAGE_TRAILER != 0 {
        let payloads = local_payloads(&pkg.data, pkg.summary.payload_toc_offset)?;
        let p = payloads.iter().find(|p| p.hash == h.id).ok_or("the mesh description is not in the package trailer")?;
        pkg.data.get(p.offset..p.offset + p.compressed).ok_or("mesh description outside the file")?
    } else {
        usize::try_from(h.offset).ok().and_then(|at| pkg.data.get(at..)).ok_or("mesh description outside the file")?
    };
    decompress(buf, &|| oodle(roots)).map(Some)
}

// ---------------------------------------------------------------------------
// fingerprints

/// A fast 64-bit fingerprint for telling contents apart (not cryptographic):
/// hundreds of MB of attributes go through it.
struct Fingerprint {
    lanes: [u64; 4],
    len: u64,
}

const PRIME: u64 = 0x9E37_79B9_7F4A_7C15;

/// splitmix64's finalizer.
fn mix(mut x: u64) -> u64 {
    x ^= x >> 30;
    x = x.wrapping_mul(0xBF58_476D_1CE4_E5B9);
    x ^= x >> 27;
    x = x.wrapping_mul(0x94D0_49BB_1331_11EB);
    x ^ (x >> 31)
}

impl Fingerprint {
    fn new() -> Self {
        Fingerprint { lanes: [1u64, 2, 3, 4].map(|k| k.wrapping_mul(PRIME)), len: 0 }
    }

    fn update(&mut self, b: &[u8]) {
        let mut chunks = b.chunks_exact(32);
        for c in &mut chunks {
            for (i, lane) in self.lanes.iter_mut().enumerate() {
                let w = u64::from_le_bytes(c[i * 8..i * 8 + 8].try_into().unwrap());
                *lane = (*lane ^ w).wrapping_mul(PRIME).rotate_left(31);
            }
        }
        for (i, &byte) in chunks.remainder().iter().enumerate() {
            let lane = &mut self.lanes[i % 4];
            *lane = (*lane ^ byte as u64).wrapping_mul(PRIME).rotate_left(31);
        }
        self.len += b.len() as u64;
    }

    fn finish(&self) -> u64 {
        self.lanes.iter().fold(mix(self.len), |h, &l| mix(h ^ l))
    }

    fn text(&self) -> String {
        format!("{:012x}", self.finish() >> 16)
    }
}

/// Numbers, or arrays of numbers, of 4 bytes: no padding, so their bytes can be read.
pub trait Plain: Copy {}
impl Plain for u32 {}
impl Plain for f32 {}
impl<T: Plain, const N: usize> Plain for [T; N] {}

/// A slice's bytes, in this (little-endian) machine's order.
pub fn bytes_of<T: Plain>(v: &[T]) -> &[u8] {
    // SAFETY: Plain types have no padding, and u8 has no alignment.
    unsafe { std::slice::from_raw_parts(v.as_ptr().cast(), std::mem::size_of_val(v)) }
}

// ---------------------------------------------------------------------------
// FMeshDescription

/// AttributeTypes (MeshAttributeArray.h): FVector4f, FVector3f, FVector2f, float,
/// int32, bool, FName, FTransform. The first six serialize in bulk.
const BULK_SIZES: [usize; 6] = [16, 12, 8, 4, 4, 1];
const TYPE_FNAME: u32 = 6;
const TYPE_TRANSFORM: u32 = 7;
/// FQuat4d, FVector3d translation, FVector3d scale.
const TRANSFORM_BYTES: usize = 80;

/// One channel of an attribute: little-endian elements, or names.
enum Values<'a> {
    Raw(&'a [u8]),
    /// Built from an old-format description's element arrays.
    Owned(Vec<u8>),
    Names(Vec<String>),
    /// Transforms, and attributes with a variable count per element: kept out.
    Skipped,
}

impl Values<'_> {
    fn bytes(&self) -> Option<&[u8]> {
        match self {
            Values::Raw(b) => Some(b),
            Values::Owned(b) => Some(b),
            _ => None,
        }
    }
}

struct Attribute<'a> {
    name: String,
    channels: Vec<Values<'a>>,
}

/// An FMeshElementContainer: which ids are in use, and their attributes.
struct Container<'a> {
    valid: Vec<bool>,
    attributes: Vec<Attribute<'a>>,
}

impl<'a> Container<'a> {
    fn attr(&self, name: &str) -> Option<&Attribute<'a>> {
        self.attributes.iter().find(|a| a.name == name)
    }
    fn count(&self) -> usize {
        self.valid.iter().filter(|v| **v).count()
    }
}

type Elements<'a> = Vec<(String, Vec<Container<'a>>)>;

/// Versions an old-format (UE4) description was saved with.
#[derive(Clone, Copy)]
struct Legacy {
    release: i32,
    editor: i32,
}

struct Parser<'a> {
    ar: Reader<'a>,
    /// FTransform arrays saved before MeshDescriptionForSkeletalMesh went through BulkSerialize.
    bulk_transforms: bool,
    legacy: Option<Legacy>,
}

fn too_many<T>(what: &str, n: i32) -> Res<T> {
    archive_err(format!("implausible {what} count {n}"))
}

impl<'a> Parser<'a> {
    fn count(&mut self, what: &str) -> Res<usize> {
        let n = self.ar.i32()?;
        if n < 0 || n as usize > self.ar.data.len() {
            return too_many(what, n);
        }
        Ok(n as usize)
    }

    /// TBitArray: bit count, then whole 32-bit words.
    fn bits(&mut self) -> Res<Vec<bool>> {
        let n = self.count("bit")?;
        let raw = self.ar.bytes(n.div_ceil(32) * 4)?;
        Ok((0..n).map(|i| raw[i / 8] >> (i % 8) & 1 != 0).collect())
    }

    fn names(&mut self, n: usize) -> Res<Vec<String>> {
        (0..n).map(|_| self.ar.fstring()).collect()
    }

    /// TArray::BulkSerialize: element size, count, the elements.
    fn bulk(&mut self, size: usize) -> Res<&'a [u8]> {
        let stored = self.ar.i32()?;
        if stored as usize != size {
            return archive_err(format!("attribute element size {stored}, expected {size}"));
        }
        let n = self.count("attribute element")?;
        self.ar.bytes(n * size)
    }

    /// A TMeshAttributeArrayBase (after its extent). Old descriptions saved
    /// before MeshDescriptionNewSerialization wrote plain arrays.
    fn array(&mut self, ty: u32) -> Res<Values<'a>> {
        let plain = self.legacy.is_some_and(|l| l.release < MESH_DESCRIPTION_NEW_SERIALIZATION);
        Ok(match ty {
            0..=5 if plain => {
                let n = self.count("attribute element")?;
                Values::Raw(self.ar.bytes(n * BULK_SIZES[ty as usize])?)
            }
            0..=5 => Values::Raw(self.bulk(BULK_SIZES[ty as usize])?),
            TYPE_FNAME => {
                let n = self.count("name")?;
                Values::Names(self.names(n)?)
            }
            TYPE_TRANSFORM if self.bulk_transforms => {
                let size = self.ar.i32()?.max(0) as usize;
                let n = self.count("transform")?;
                self.ar.skip(n * size)?;
                Values::Skipped
            }
            TYPE_TRANSFORM => {
                let n = self.count("transform")?;
                self.ar.skip(n * TRANSFORM_BYTES)?;
                Values::Skipped
            }
            _ => return archive_err(format!("unknown mesh attribute type {ty}")),
        })
    }

    /// A plain TArray of the type, as a chunk of a variable-count attribute holds.
    fn plain_array(&mut self, ty: u32) -> Res<()> {
        let n = self.count("attribute element")?;
        match ty {
            0..=5 => self.ar.skip(n * BULK_SIZES[ty as usize]),
            TYPE_FNAME => self.names(n).map(|_| ()),
            _ => self.ar.skip(n * TRANSFORM_BYTES),
        }
    }

    /// One value of the type, as `Ar << Value` writes it (a bool as 32 bits).
    fn value(&mut self, ty: u32) -> Res<()> {
        match ty {
            0..=4 => self.ar.skip(BULK_SIZES[ty as usize]),
            5 => self.ar.skip(4),
            TYPE_FNAME => self.ar.fstring().map(|_| ()),
            _ => self.ar.skip(TRANSFORM_BYTES),
        }
    }

    /// FAttributesSetEntry: type, extent (not in old descriptions), then the
    /// array set of that shape.
    fn attribute(&mut self, name: String) -> Res<Attribute<'a>> {
        let ty = self.ar.u32()?;
        let extent = if self.legacy.is_some() { 1 } else { self.ar.u32()? };
        self.ar.i32()?; // NumElements
        let n = self.count("attribute channel")?;
        let mut channels = Vec::with_capacity(n);
        for _ in 0..n {
            if extent > 0 {
                if self.legacy.is_none() {
                    self.ar.u32()?; // the array's own extent
                }
                channels.push(self.array(ty)?);
            } else {
                // TAttributeArrayContainer: chunks of data, start/count/max per element.
                for _ in 0..self.count("chunk")? {
                    self.plain_array(ty)?;
                    let elements = self.count("chunk element")?;
                    self.ar.skip(elements * 12)?;
                }
                self.ar.i32()?; // NumElements
                self.value(ty)?; // DefaultValue
                channels.push(Values::Skipped);
            }
        }
        self.value(ty)?; // DefaultValue
        self.ar.u32()?; // Flags
        Ok(Attribute { name, channels })
    }

    /// FAttributesSetBase: element count, then attribute name → array set.
    fn attribute_set(&mut self) -> Res<Vec<Attribute<'a>>> {
        self.ar.i32()?; // NumElements
        let n = self.count("attribute")?;
        let mut attributes = Vec::with_capacity(n);
        for _ in 0..n {
            let name = self.ar.fstring()?;
            attributes.push(self.attribute(name)?);
        }
        Ok(attributes)
    }

    fn container(&mut self) -> Res<Container<'a>> {
        let valid = self.bits()?;
        self.ar.i32()?; // NumHoles
        Ok(Container { valid, attributes: self.attribute_set()? })
    }

    /// FMeshDescription::Elements: element type name → its channels.
    fn elements(&mut self) -> Res<Elements<'a>> {
        let n = self.count("element type")?;
        let mut out = Vec::with_capacity(n);
        for _ in 0..n {
            let name = self.ar.fstring()?;
            let channels = self.count("element channel")?;
            let mut c = Vec::with_capacity(channels);
            for _ in 0..channels {
                c.push(self.container()?);
            }
            out.push((name, c));
        }
        Ok(out)
    }

    fn skip_ints(&mut self) -> Res<()> {
        let n = self.count("id")?;
        self.ar.skip(n * 4)
    }

    fn ints(&mut self) -> Res<Vec<i32>> {
        let n = self.count("id")?;
        (0..n).map(|_| self.ar.i32()).collect()
    }

    /// A TMeshElementArray: the ids in use, then each one's element.
    fn element_array<T>(&mut self, mut one: impl FnMut(&mut Self) -> Res<T>) -> Res<(Vec<bool>, Vec<Option<T>>)> {
        let valid = self.bits()?;
        let mut items = Vec::with_capacity(valid.len());
        for &v in &valid {
            items.push(if v { Some(one(self)?) } else { None });
        }
        Ok((valid, items))
    }

    /// FMeshDescription::SerializeLegacy: element arrays, then their attribute
    /// sets, then (since MeshDescriptionTriangles) the triangles. Rebuilt into
    /// the containers the current format has.
    fn legacy_elements(&mut self, l: Legacy) -> Res<Elements<'a>> {
        let old = l.release < MESH_DESCRIPTION_NEW_SERIALIZATION;
        let (vertices, _) = self.element_array(|p| {
            if old {
                p.skip_ints()?;
                p.skip_ints()?;
            }
            Ok(())
        })?;
        let (instances, instance_vertex) = self.element_array(|p| {
            let v = p.ar.i32()?;
            if old {
                p.skip_ints()?;
            }
            Ok(v)
        })?;
        let (edges, _) = self.element_array(|p| {
            p.ar.skip(8)?;
            if old {
                p.skip_ints()?;
            }
            Ok(())
        })?;
        let (polygons, polygon_data) = self.element_array(|p| {
            let corners = p.ints()?;
            if l.editor < MESH_DESCRIPTION_REMOVED_HOLES {
                for _ in 0..p.count("hole")? {
                    p.skip_ints()?;
                }
            }
            if old {
                let n = p.count("triangle")?;
                p.ar.skip(n * if l.editor >= MESH_DESCRIPTION_TRIANGLES { 16 } else { 12 })?;
            }
            Ok((corners, p.ar.i32()?))
        })?;
        let (groups, _) = self.element_array(|p| if old { p.skip_ints() } else { Ok(()) })?;
        let vertex_attrs = self.attribute_set()?;
        let mut instance_attrs = self.attribute_set()?;
        let edge_attrs = self.attribute_set()?;
        let polygon_attrs = self.attribute_set()?;
        let group_attrs = self.attribute_set()?;
        let group_of = |poly: i32| polygon_data.get(poly.max(0) as usize).and_then(|p| p.as_ref()).map(|p| p.1).unwrap_or(0);
        let mut corner_bytes = Vec::new();
        let mut group_bytes = Vec::new();
        let mut triangles = Vec::new();
        if l.editor >= MESH_DESCRIPTION_TRIANGLES {
            let (valid, tris) = self.element_array(|p| Ok(([p.ar.i32()?, p.ar.i32()?, p.ar.i32()?], p.ar.i32()?)))?;
            self.attribute_set()?;
            for t in &tris {
                let (corners, poly) = t.unwrap_or(([0; 3], 0));
                corner_bytes.extend(corners.iter().flat_map(|c| c.to_le_bytes()));
                group_bytes.extend(group_of(poly).to_le_bytes());
            }
            triangles = valid;
        } else {
            // Before triangles were stored: a fan over each polygon.
            for p in polygon_data.iter().flatten() {
                for k in 1..p.0.len().saturating_sub(1) {
                    for c in [p.0[0], p.0[k], p.0[k + 1]] {
                        corner_bytes.extend(c.to_le_bytes());
                    }
                    group_bytes.extend(p.1.to_le_bytes());
                    triangles.push(true);
                }
            }
        }
        let instance_vertex: Vec<u8> = instance_vertex.iter().flat_map(|v| v.unwrap_or(-1).to_le_bytes()).collect();
        instance_attrs.push(Attribute { name: "VertexIndex".into(), channels: vec![Values::Owned(instance_vertex)] });
        let triangle_attrs = vec![
            Attribute { name: "VertexInstanceIndex".into(), channels: vec![Values::Owned(corner_bytes)] },
            Attribute { name: "PolygonGroupIndex".into(), channels: vec![Values::Owned(group_bytes)] },
        ];
        let one = |valid, attributes| vec![Container { valid, attributes }];
        Ok(vec![
            ("Vertices".into(), one(vertices, vertex_attrs)),
            ("VertexInstances".into(), one(instances, instance_attrs)),
            ("Edges".into(), one(edges, edge_attrs)),
            ("Triangles".into(), one(triangles, triangle_attrs)),
            ("Polygons".into(), one(polygons, polygon_attrs)),
            ("PolygonGroups".into(), one(groups, group_attrs)),
        ])
    }
}

/// A LOD's source geometry, compacted (no unused ids).
pub struct Geometry {
    pub positions: Vec<[f32; 3]>,
    pub triangles: Vec<[u32; 3]>,
    /// Per triangle, an index into `sections`.
    pub triangle_sections: Vec<u32>,
    /// Polygon groups by their imported material slot name.
    pub sections: Vec<String>,
    pub vertex_instances: usize,
    pub polygons: usize,
    pub uv_channels: usize,
    pub vertex_colors: bool,
    pub bones: usize,
    /// A fingerprint of the positions and triangles.
    pub shape: String,
    /// Fingerprints of what else the description holds per element (UVs, normals,
    /// colors, hard edges), so an edit that moves no vertex still shows.
    pub attributes: Vec<(String, String)>,
}

fn raw_i32(v: &Values, i: usize) -> Option<i32> {
    v.bytes()?.get(i * 4..i * 4 + 4).map(|s| i32::from_le_bytes(s.try_into().unwrap()))
}

fn first<'b, 'a>(c: Option<&'b Attribute<'a>>) -> Option<&'b Values<'a>> {
    c.and_then(|a| a.channels.first())
}

fn shape_fingerprint(positions: &[[f32; 3]], triangles: &[[u32; 3]]) -> String {
    let mut f = Fingerprint::new();
    f.update(bytes_of(positions));
    f.update(bytes_of(triangles));
    f.text()
}

fn geometry_of(pkg: &Package, raw: &[u8]) -> Result<Geometry, String> {
    let s = &pkg.summary;
    let mut p = Parser {
        ar: Reader::new(raw, &[]),
        bulk_transforms: s.custom_version(FORTNITE_MAIN_VERSION) < MESH_DESCRIPTION_FOR_SKELETAL_MESH,
        legacy: None,
    };
    let release = s.custom_version(RELEASE_VERSION);
    let elements = if release != RELEASE_MESH_DESCRIPTION_NEW_FORMAT && s.custom_version(UE5_MAIN_VERSION) < MESH_DESCRIPTION_NEW_FORMAT {
        let l = Legacy { release, editor: s.custom_version(EDITOR_VERSION) };
        p.legacy = Some(l);
        p.legacy_elements(l)
    } else {
        p.elements()
    };
    extract(&elements.map_err(|e| format!("unreadable mesh description: {e}"))?)
}

fn extract(elements: &Elements) -> Result<Geometry, String> {
    let find = |name: &str| elements.iter().find(|(n, _)| n == name).and_then(|(_, c)| c.first());
    let (Some(verts), Some(instances), Some(tris)) = (find("Vertices"), find("VertexInstances"), find("Triangles")) else {
        return Err("the mesh description has no vertices or triangles".into());
    };
    // MeshAttribute::Vertex::Position really is "Position " (MeshAttributes.cpp).
    let Some(pos) = first(verts.attr("Position ").or(verts.attr("Position"))).and_then(Values::bytes) else {
        return Err("the mesh description has no vertex positions".into());
    };
    let mut remap = vec![u32::MAX; verts.valid.len()];
    let mut positions = Vec::new();
    for (i, _) in verts.valid.iter().enumerate().filter(|(_, v)| **v) {
        let at = i * 12;
        let Some(b) = pos.get(at..at + 12) else { break };
        let f = |k: usize| f32::from_le_bytes(b[k * 4..k * 4 + 4].try_into().unwrap());
        remap[i] = positions.len() as u32;
        positions.push([f(0), f(1), f(2)]);
    }
    let vertex_of = first(instances.attr("VertexIndex")).ok_or("vertex instances have no vertex")?;
    let corners = first(tris.attr("VertexInstanceIndex")).ok_or("triangles have no corners")?;
    let groups = first(tris.attr("PolygonGroupIndex"));
    let mut group_remap: HashMap<i32, u32> = HashMap::new();
    let mut sections = Vec::new();
    if let Some(g) = find("PolygonGroups") {
        let names = match first(g.attr("ImportedMaterialSlotName")) {
            Some(Values::Names(n)) => n.as_slice(),
            _ => &[],
        };
        for (i, _) in g.valid.iter().enumerate().filter(|(_, v)| **v) {
            group_remap.insert(i as i32, sections.len() as u32);
            sections.push(names.get(i).cloned().unwrap_or_else(|| format!("Group {i}")));
        }
    }
    let mut triangles = Vec::new();
    let mut triangle_sections = Vec::new();
    for (t, _) in tris.valid.iter().enumerate().filter(|(_, v)| **v) {
        let mut tri = [0u32; 3];
        let ok = (0..3).all(|k| {
            let v = raw_i32(corners, t * 3 + k).and_then(|vi| raw_i32(vertex_of, vi.max(0) as usize));
            match v.and_then(|v| remap.get(v as usize)) {
                Some(&m) if m != u32::MAX => {
                    tri[k] = m;
                    true
                }
                _ => false,
            }
        });
        if !ok {
            continue;
        }
        triangles.push(tri);
        let group = groups.and_then(|g| raw_i32(g, t)).and_then(|g| group_remap.get(&g).copied());
        triangle_sections.push(group.unwrap_or(0));
    }
    if sections.is_empty() {
        sections.push("Group 0".into());
    }
    // Only the elements in use: unused ids keep stale values.
    let fingerprint = |c: &Container, names: &[&str]| -> Option<String> {
        let mut f = Fingerprint::new();
        let mut any = false;
        let all = c.valid.iter().all(|v| *v);
        for name in names {
            let Some(a) = c.attr(name) else { continue };
            for b in a.channels.iter().filter_map(Values::bytes) {
                let size = b.len() / c.valid.len().max(1);
                if all {
                    f.update(&b[..size * c.valid.len()]);
                } else {
                    for (i, _) in c.valid.iter().enumerate().filter(|(_, v)| **v) {
                        f.update(&b[i * size..(i + 1) * size]);
                    }
                }
                any = true;
            }
        }
        any.then(|| f.text())
    };
    let mut attributes = Vec::new();
    for (label, c, names) in [
        ("UVs", Some(instances), &["TextureCoordinate"][..]),
        ("Normals", Some(instances), &["Normal", "Tangent", "BinormalSign"][..]),
        ("Colors", Some(instances), &["Color"][..]),
        ("Hard edges", find("Edges"), &["IsHard"][..]),
    ] {
        if let Some(f) = c.and_then(|c| fingerprint(c, names)) {
            attributes.push((label.to_string(), f));
        }
    }
    let vertex_colors = match first(instances.attr("Color")).and_then(Values::bytes) {
        Some(c) => c.chunks_exact(16).enumerate().any(|(i, px)| {
            instances.valid.get(i) == Some(&true)
                && px.chunks_exact(4).any(|f| f32::from_le_bytes(f.try_into().unwrap()) != 1.0)
        }),
        None => false,
    };
    Ok(Geometry {
        shape: shape_fingerprint(&positions, &triangles),
        positions,
        triangles,
        triangle_sections,
        sections,
        vertex_instances: instances.count(),
        polygons: find("Polygons").map(|c| c.count()).unwrap_or(0),
        uv_channels: instances.attr("TextureCoordinate").map(|a| a.channels.len()).unwrap_or(0),
        vertex_colors,
        bones: find("Bones").map(|c| c.count()).unwrap_or(0),
        attributes,
    })
}

/// FRawMesh (RawMesh.cpp): per face a material index and smoothing mask, the
/// positions, then per wedge (face corner) its vertex, tangent basis, eight UV
/// channels and color.
fn raw_mesh_geometry(raw: &[u8], slots: &[(String, String, String)]) -> Result<Geometry, String> {
    fn array<'a>(ar: &mut Reader<'a>, size: usize) -> Res<&'a [u8]> {
        let n = ar.i32()?;
        if n < 0 || n as usize * size > ar.data.len() {
            return too_many("raw mesh element", n);
        }
        ar.bytes(n as usize * size)
    }
    let mut ar = Reader::new(raw, &[]);
    let read = (|| -> Res<_> {
        ar.i32()?; // version
        ar.i32()?; // licensee version
        let face_materials = array(&mut ar, 4)?;
        let smoothing = array(&mut ar, 4)?;
        let positions = array(&mut ar, 12)?;
        let wedges = array(&mut ar, 4)?;
        let tangents = [array(&mut ar, 12)?, array(&mut ar, 12)?, array(&mut ar, 12)?];
        let mut uvs = Vec::new();
        for _ in 0..8 {
            uvs.push(array(&mut ar, 8)?);
        }
        let colors = array(&mut ar, 4)?;
        Ok((face_materials, smoothing, positions, wedges, tangents, uvs, colors))
    })();
    let (face_materials, smoothing, positions, wedges, tangents, uvs, colors) =
        read.map_err(|e| format!("unreadable raw mesh: {e}"))?;
    let f32_at = |b: &[u8], i: usize| f32::from_le_bytes(b[i * 4..i * 4 + 4].try_into().unwrap());
    let u32_at = |b: &[u8], i: usize| u32::from_le_bytes(b[i * 4..i * 4 + 4].try_into().unwrap());
    let positions: Vec<[f32; 3]> = (0..positions.len() / 12).map(|i| [0, 1, 2].map(|k| f32_at(positions, i * 3 + k))).collect();
    let mut materials: Vec<u32> = (0..face_materials.len() / 4).map(|i| u32_at(face_materials, i)).collect();
    let mut order = materials.clone();
    order.sort_unstable();
    order.dedup();
    let sections = order
        .iter()
        .map(|&m| {
            let slot = slots.get(m as usize);
            match slot {
                Some((_, imported, _)) if imported != "None" && !imported.is_empty() => imported.clone(),
                Some((name, _, _)) if name != "None" && !name.is_empty() => name.clone(),
                _ => format!("Material {m}"),
            }
        })
        .collect::<Vec<_>>();
    for m in &mut materials {
        *m = order.binary_search(m).unwrap_or(0) as u32;
    }
    let mut triangles = Vec::new();
    let mut triangle_sections = Vec::new();
    for face in 0..wedges.len() / 12 {
        let tri = [0, 1, 2].map(|k| u32_at(wedges, face * 3 + k));
        if tri.iter().all(|&v| (v as usize) < positions.len()) {
            triangles.push(tri);
            triangle_sections.push(materials.get(face).copied().unwrap_or(0));
        }
    }
    let fingerprint = |parts: &[&[u8]]| {
        let mut f = Fingerprint::new();
        parts.iter().for_each(|p| f.update(p));
        f.text()
    };
    let used_uvs: Vec<&[u8]> = uvs.iter().copied().filter(|u| !u.is_empty()).collect();
    let mut attributes = vec![("Normals".to_string(), fingerprint(&tangents))];
    if !used_uvs.is_empty() {
        attributes.insert(0, ("UVs".into(), fingerprint(&used_uvs)));
    }
    if !colors.is_empty() {
        attributes.push(("Colors".into(), fingerprint(&[colors])));
    }
    attributes.push(("Smoothing groups".into(), fingerprint(&[smoothing])));
    Ok(Geometry {
        shape: shape_fingerprint(&positions, &triangles),
        polygons: wedges.len() / 12,
        vertex_instances: wedges.len() / 4,
        uv_channels: used_uvs.len(),
        vertex_colors: colors.chunks_exact(4).any(|c| c != [255; 4]),
        bones: 0,
        positions,
        triangles,
        triangle_sections,
        sections,
        attributes,
    })
}

/// Decoded LODs, by a fingerprint of what they were decoded from.
static GEOMETRY: OnceLock<Mutex<Vec<(String, Arc<Geometry>)>>> = OnceLock::new();

/// A source LOD's geometry, or None for a LOD without one.
pub fn geometry(pkg: &Package, lod: &SourceLod, roots: &[PathBuf]) -> Result<Option<Arc<Geometry>>, String> {
    let (key, decode): (String, Box<dyn Fn() -> Result<Option<Geometry>, String>>) = match &lod.source {
        None => return Ok(None),
        Some(Source::Unreadable(e)) => return Err(e.clone()),
        Some(Source::Export(e)) => {
            let h = bulk_header(pkg, *e)?;
            if h.size == 0 {
                return Ok(None);
            }
            let key = hex(&h.id);
            (key, Box::new(move || payload(pkg, &h, roots)?.map(|raw| geometry_of(pkg, &raw)).transpose()))
        }
        Some(Source::Inline { raw_mesh, bulk }) => {
            let stored = usize::try_from(bulk.at)
                .ok()
                .and_then(|at| pkg.data.get(at..at + bulk.stored.max(0) as usize))
                .ok_or("mesh data outside the file")?;
            let mut f = Fingerprint::new();
            f.update(stored);
            let raw_mesh = *raw_mesh;
            let decode = move || {
                let raw = bulk.bytes(pkg)?;
                if raw_mesh {
                    let main = pkg.exports.iter().position(|e| e.is_asset).unwrap_or(0);
                    raw_mesh_geometry(&raw, &material_slots(pkg, main)).map(Some)
                } else {
                    geometry_of(pkg, &raw).map(Some)
                }
            };
            (format!("inline {}", f.text()), Box::new(decode))
        }
    };
    let cache = GEOMETRY.get_or_init(|| Mutex::new(Vec::new()));
    if let Some((_, g)) = cache.lock().map_err(|_| "cache poisoned")?.iter().find(|(k, _)| *k == key) {
        return Ok(Some(g.clone()));
    }
    let Some(g) = decode()? else { return Ok(None) };
    let g = Arc::new(g);
    let mut c = cache.lock().map_err(|_| "cache poisoned")?;
    c.push((key, g.clone()));
    // The oldest go once over budget; the two newest (one LOD of each side) stay.
    while c.len() > 2 && c.iter().map(|(_, g)| g.size()).sum::<usize>() > CACHE_BYTES {
        c.remove(0);
    }
    Ok(Some(g))
}

/// What decoded LODs may hold: both sides of a few multi-million-triangle LODs.
const CACHE_BYTES: usize = 768 << 20;

/// Every LOD's geometry, decoded side by side.
pub fn geometries(pkg: &Package, lods: &[SourceLod], roots: &[PathBuf]) -> Vec<Result<Option<Arc<Geometry>>, String>> {
    std::thread::scope(|s| {
        let jobs: Vec<_> = lods.iter().map(|l| s.spawn(move || geometry(pkg, l, roots))).collect();
        jobs.into_iter().map(|j| j.join().unwrap_or_else(|_| Err("decoding the mesh panicked".into()))).collect()
    })
}

impl Geometry {
    pub fn bounds(&self) -> ([f32; 3], [f32; 3]) {
        let mut lo = [f32::MAX; 3];
        let mut hi = [f32::MIN; 3];
        for p in &self.positions {
            for k in 0..3 {
                lo[k] = lo[k].min(p[k]);
                hi[k] = hi[k].max(p[k]);
            }
        }
        if self.positions.is_empty() {
            return ([0.0; 3], [0.0; 3]);
        }
        (lo, hi)
    }

    fn size(&self) -> usize {
        self.positions.len() * 12 + self.triangles.len() * 16
    }
}

fn cm(v: f32) -> String {
    let r = (v as f64 * 100.0).round() / 100.0;
    format!("{}", if r == 0.0 { 0.0 } else { r })
}

/// What the summary shows of a LOD's geometry; sections go through `material`
/// (slot name → material) when the mesh assigns them.
pub fn stats(g: &Geometry, material: &dyn Fn(&str) -> Option<String>) -> Vec<(String, Val)> {
    let mut per_section = vec![0usize; g.sections.len()];
    for s in &g.triangle_sections {
        if let Some(n) = per_section.get_mut(*s as usize) {
            *n += 1;
        }
    }
    let sections = g
        .sections
        .iter()
        .zip(per_section)
        .map(|(name, n)| {
            let mut m = vec![("Triangles".to_string(), Val::Int(n as i64))];
            if let Some(mat) = material(name) {
                m.push(("Material".into(), Val::Str(mat)));
            }
            (name.clone(), Val::Map(m))
        })
        .collect();
    let (lo, hi) = g.bounds();
    let mut out = vec![
        ("Triangles".to_string(), Val::Int(g.triangles.len() as i64)),
        ("Vertices".into(), Val::Int(g.positions.len() as i64)),
        ("Vertex instances".into(), Val::Int(g.vertex_instances as i64)),
    ];
    if g.polygons != g.triangles.len() {
        out.push(("Polygons".into(), Val::Int(g.polygons as i64)));
    }
    out.push(("UV channels".into(), Val::Int(g.uv_channels as i64)));
    out.push(("Vertex colors".into(), Val::Bool(g.vertex_colors)));
    if g.bones > 0 {
        out.push(("Bones".into(), Val::Int(g.bones as i64)));
    }
    out.push(("Size".into(), Val::Str(format!("{} × {} × {}", cm(hi[0] - lo[0]), cm(hi[1] - lo[1]), cm(hi[2] - lo[2])))));
    out.push((
        "Center".into(),
        Val::Str(format!("{}, {}, {}", cm((hi[0] + lo[0]) / 2.0), cm((hi[1] + lo[1]) / 2.0), cm((hi[2] + lo[2]) / 2.0))),
    ));
    out.push(("Sections".into(), Val::Map(sections)));
    out.push(("Geometry".into(), Val::Str(g.shape.clone())));
    out.extend(g.attributes.iter().map(|(k, v)| (k.clone(), Val::Str(v.clone()))));
    out
}

// ---------------------------------------------------------------------------
// two revisions of a LOD

pub const SAME: u8 = 0;
/// Only on this side: a removed triangle on the left, an added one on the right.
pub const ONE_SIDE: u8 = 1;
pub const MOVED: u8 = 2;

/// Closer than this (cm) counts as the same position.
const EPSILON: f32 = 1e-3;

pub struct Comparison {
    /// Same vertices and triangles, only positions may differ.
    pub same_topology: bool,
    pub moved_vertices: usize,
    pub max_move: f32,
    pub removed_triangles: usize,
    pub added_triangles: usize,
    /// Per triangle of each side: SAME, ONE_SIDE or MOVED.
    pub left: Vec<u8>,
    pub right: Vec<u8>,
}

fn quantize(p: [f32; 3]) -> [i64; 3] {
    p.map(|c| (c as f64 / EPSILON as f64).round() as i64)
}

/// A triangle by its corner positions, from its smallest corner on (winding
/// kept), mixed down to 64 bits.
fn triangle_key(g: &Geometry, t: [u32; 3]) -> u64 {
    let c = t.map(|i| quantize(g.positions[i as usize]));
    let start = (0..3).min_by_key(|&k| c[k]).unwrap();
    (0..3).flat_map(|k| c[(start + k) % 3]).fold(PRIME, |h, v| mix(h ^ v as u64))
}

/// Keys that are already mixed hash to themselves.
#[derive(Default)]
struct Premixed(u64);

impl Hasher for Premixed {
    fn finish(&self) -> u64 {
        self.0
    }
    fn write(&mut self, bytes: &[u8]) {
        for &b in bytes {
            self.0 = mix(self.0 ^ b as u64);
        }
    }
    fn write_u64(&mut self, v: u64) {
        self.0 = v;
    }
}

pub fn compare(a: &Geometry, b: &Geometry) -> Comparison {
    if a.positions.len() == b.positions.len() && a.triangles == b.triangles {
        let moves: Vec<f32> = a
            .positions
            .iter()
            .zip(&b.positions)
            .map(|(p, q)| ((p[0] - q[0]).powi(2) + (p[1] - q[1]).powi(2) + (p[2] - q[2]).powi(2)).sqrt())
            .collect();
        let status: Vec<u8> =
            a.triangles.iter().map(|t| if t.iter().any(|&i| moves[i as usize] > EPSILON) { MOVED } else { SAME }).collect();
        return Comparison {
            same_topology: true,
            moved_vertices: moves.iter().filter(|&&m| m > EPSILON).count(),
            max_move: moves.iter().copied().fold(0.0, f32::max),
            removed_triangles: 0,
            added_triangles: 0,
            left: status.clone(),
            right: status,
        };
    }
    // Different topology: match triangles by where their corners are. A key
    // found n times on one side and m on the other matches min(n, m) of each.
    let (ka, kb) = std::thread::scope(|s| {
        let ka = s.spawn(|| a.triangles.iter().map(|&t| triangle_key(a, t)).collect::<Vec<_>>());
        let kb: Vec<u64> = b.triangles.iter().map(|&t| triangle_key(b, t)).collect();
        (ka.join().unwrap_or_default(), kb)
    });
    let mut counts: HashMap<u64, [u32; 2], BuildHasherDefault<Premixed>> =
        HashMap::with_capacity_and_hasher(ka.len().max(kb.len()), Default::default());
    for k in &ka {
        counts.entry(*k).or_default()[0] += 1;
    }
    for k in &kb {
        counts.entry(*k).or_default()[1] += 1;
    }
    for c in counts.values_mut() {
        let matched = c[0].min(c[1]);
        *c = [matched, matched];
    }
    let mut status = |keys: &[u64], side: usize| -> Vec<u8> {
        keys.iter()
            .map(|k| match counts.get_mut(k) {
                Some(c) if c[side] > 0 => {
                    c[side] -= 1;
                    SAME
                }
                _ => ONE_SIDE,
            })
            .collect()
    };
    let left = status(&ka, 0);
    let right = status(&kb, 1);
    Comparison {
        same_topology: false,
        moved_vertices: 0,
        max_move: 0.0,
        removed_triangles: left.iter().filter(|&&s| s == ONE_SIDE).count(),
        added_triangles: right.iter().filter(|&&s| s == ONE_SIDE).count(),
        left,
        right,
    }
}

// ---------------------------------------------------------------------------
// USkeletalMesh native data

/// A skeletal mesh's material slots (slot name, imported slot name, material)
/// and bones (name, parent index), which USkeletalMesh::Serialize writes after
/// its properties: strip flags, imported bounds, materials, reference skeleton.
pub struct SkeletalNative {
    pub materials: Vec<(String, String, String)>,
    pub bones: Vec<(String, i32)>,
}

pub fn skeletal_native(pkg: &Package, main: usize) -> Result<SkeletalNative, String> {
    let exp = &pkg.exports[main];
    let (_, end) = read_export_properties_end(pkg, exp, 0, false);
    let mut ar = Reader::new(pkg.export_data(exp), &pkg.names);
    let lwc = pkg.summary.file_version_ue5 >= super::package::UE5_LARGE_WORLD_COORDINATES;
    let overlay = pkg.summary.custom_version(FORTNITE_MAIN_VERSION) >= 196; // MeshMaterialSlotOverlayMaterialAdded
    (|| -> Res<SkeletalNative> {
        ar.seek(end as i64)?;
        if ar.bool32()? {
            ar.guid()?;
        }
        ar.u16()?; // FStripDataFlags
        ar.skip(if lwc { 7 * 8 } else { 7 * 4 })?; // FBoxSphereBounds
        let n = ar.count(1 << 12)?;
        let mut materials = Vec::with_capacity(n);
        for _ in 0..n {
            let material = pkg.resolve_index(ar.package_index()?);
            let slot = ar.name()?;
            let imported = if ar.bool32()? { ar.name()? } else { String::new() };
            ar.skip(4 + 4 + 4 * 4)?; // FMeshUVChannelInfo
            if overlay {
                ar.package_index()?;
            }
            materials.push((slot, imported, material));
        }
        let n = ar.count(1 << 16)?;
        let mut bones = Vec::with_capacity(n);
        for _ in 0..n {
            let name = ar.name()?;
            let parent = ar.i32()?;
            ar.fstring()?; // ExportName
            bones.push((name, parent));
        }
        Ok(SkeletalNative { materials, bones })
    })()
    .map_err(|e| format!("unreadable skeletal mesh data: {e}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn quad(offset: f32) -> Geometry {
        Geometry {
            positions: vec![[0.0, 0.0, 0.0], [1.0, 0.0, 0.0], [1.0, 1.0, offset], [0.0, 1.0, 0.0]],
            triangles: vec![[0, 1, 2], [0, 2, 3]],
            triangle_sections: vec![0, 0],
            sections: vec!["Slot".into()],
            vertex_instances: 6,
            polygons: 2,
            uv_channels: 1,
            vertex_colors: false,
            bones: 0,
            shape: String::new(),
            attributes: Vec::new(),
        }
    }

    #[test]
    fn a_moved_corner_marks_the_triangles_using_it() {
        let c = compare(&quad(0.0), &quad(0.5));
        assert!(c.same_topology);
        assert_eq!(c.moved_vertices, 1);
        assert!((c.max_move - 0.5).abs() < 1e-6);
        assert_eq!(c.left, vec![MOVED, MOVED]);
    }

    #[test]
    fn a_new_triangle_is_found_whatever_the_vertex_order() {
        let a = quad(0.0);
        let mut b = quad(0.0);
        b.positions.reverse(); // same shape, other ids
        b.triangles = vec![[3, 2, 1], [3, 1, 0], [3, 0, 2]];
        b.triangle_sections = vec![0, 0, 0];
        let c = compare(&a, &b);
        assert!(!c.same_topology);
        assert_eq!((c.removed_triangles, c.added_triangles), (0, 1));
        assert_eq!(c.right, vec![SAME, SAME, ONE_SIDE]);
    }

    #[test]
    fn a_duplicated_triangle_matches_once() {
        let a = quad(0.0);
        let mut b = quad(0.0);
        b.triangles.push([0, 1, 2]);
        b.triangle_sections.push(0);
        let c = compare(&a, &b);
        assert_eq!(c.right, vec![SAME, SAME, ONE_SIDE]);
        assert_eq!(c.left, vec![SAME, SAME]);
    }

    #[test]
    fn fingerprints_tell_contents_apart() {
        let text = |b: &[u8]| {
            let mut f = Fingerprint::new();
            f.update(b);
            f.text()
        };
        let data: Vec<u8> = (0..100u8).collect();
        let mut other = data.clone();
        other[70] ^= 1;
        assert_eq!(text(&data), text(&data.clone()));
        assert_ne!(text(&data), text(&other));
        assert_ne!(text(&data[..99]), text(&data));
    }
}
