//! UE5 package files (.uasset / .umap): the summary header and the tables.

use super::archive::{archive_err, Reader, Res};

pub const PACKAGE_FILE_TAG: u32 = 0x9E2A83C1;
pub const PACKAGE_FILE_TAG_SWAPPED: u32 = 0xC1832A9E;

// EUnrealEngineObjectUE4Version
pub const VER_UE4_WORLD_LEVEL_INFO: i32 = 224;
pub const VER_UE4_ADDED_CHUNKID_TO_ASSETDATA_AND_UPACKAGE: i32 = 278;
pub const VAR_UE4_ARRAY_PROPERTY_INNER_TAGS: i32 = 282;
pub const VER_UE4_CHANGED_CHUNKID_TO_BE_AN_ARRAY_OF_CHUNKIDS: i32 = 326;
pub const VER_UE4_ENGINE_VERSION_OBJECT: i32 = 336;
pub const VER_UE4_LOAD_FOR_EDITOR_GAME: i32 = 365;
pub const VER_UE4_ADD_STRING_ASSET_REFERENCES_MAP: i32 = 384;
pub const VER_UE4_STRUCT_GUID_IN_PROPERTY_TAG: i32 = 441;
pub const VER_UE4_PACKAGE_SUMMARY_HAS_COMPATIBLE_ENGINE_VERSION: i32 = 444;
pub const VER_UE4_SERIALIZE_TEXT_IN_PACKAGES: i32 = 459;
pub const VER_UE4_COOKED_ASSETS_IN_EDITOR_SUPPORT: i32 = 485;
pub const VER_UE4_PROPERTY_GUID_IN_PROPERTY_TAG: i32 = 503;
pub const VER_UE4_NAME_HASHES_SERIALIZED: i32 = 504;
pub const VER_UE4_PRELOAD_DEPENDENCIES_IN_COOKED_EXPORTS: i32 = 507;
pub const VER_UE4_TEMPLATE_INDEX_IN_COOKED_EXPORTS: i32 = 508;
pub const VER_UE4_ADDED_SEARCHABLE_NAMES: i32 = 510;
pub const VER_UE4_64BIT_EXPORTMAP_SERIALSIZES: i32 = 511;
pub const VER_UE4_ADDED_SOFT_OBJECT_PATH: i32 = 514;
pub const VER_UE4_ADDED_PACKAGE_SUMMARY_LOCALIZATION_ID: i32 = 516;
pub const VER_UE4_ADDED_PACKAGE_OWNER: i32 = 518;
pub const VER_UE4_NON_OUTER_PACKAGE_IMPORT: i32 = 520;
pub const VER_UE4_ASSETREGISTRY_DEPENDENCYFLAGS: i32 = 521;

// EUnrealEngineObjectUE5Version
pub const UE5_NAMES_REFERENCED_FROM_EXPORT_DATA: i32 = 1001;
pub const UE5_PAYLOAD_TOC: i32 = 1002;
pub const UE5_OPTIONAL_RESOURCES: i32 = 1003;
pub const UE5_LARGE_WORLD_COORDINATES: i32 = 1004;
pub const UE5_REMOVE_OBJECT_EXPORT_PACKAGE_GUID: i32 = 1005;
pub const UE5_TRACK_OBJECT_EXPORT_IS_INHERITED: i32 = 1006;
pub const UE5_FSOFTOBJECTPATH_REMOVE_ASSET_PATH_FNAMES: i32 = 1007;
pub const UE5_ADD_SOFTOBJECTPATH_LIST: i32 = 1008;
pub const UE5_DATA_RESOURCES: i32 = 1009;
pub const UE5_SCRIPT_SERIALIZATION_OFFSET: i32 = 1010;
pub const UE5_PROPERTY_TAG_EXTENSION_AND_OVERRIDABLE_SERIALIZATION: i32 = 1011;
pub const UE5_PROPERTY_TAG_COMPLETE_TYPE_NAME: i32 = 1012;
pub const UE5_METADATA_SERIALIZATION_OFFSET: i32 = 1014;
pub const UE5_VERSE_CELLS: i32 = 1015;
pub const UE5_PACKAGE_SAVED_HASH: i32 = 1016;

pub const PKG_COOKED: u32 = 0x00000200;
pub const PKG_UNVERSIONED_PROPERTIES: u32 = 0x00002000;
pub const PKG_FILTER_EDITOR_ONLY: u32 = 0x80000000;

/// EPackageFlags
pub const PACKAGE_FLAGS: &[(u32, &str)] = &[
    (0x00000001, "NewlyCreated"),
    (0x00000002, "ClientOptional"),
    (0x00000004, "ServerSideOnly"),
    (0x00000010, "CompiledIn"),
    (0x00000020, "ForDiffing"),
    (0x00000040, "EditorOnly"),
    (0x00000080, "Developer"),
    (0x00000100, "UncookedOnly"),
    (0x00000200, "Cooked"),
    (0x00000400, "ContainsNoAsset"),
    (0x00000800, "NotExternallyReferenceable"),
    (0x00001000, "AccessSpecifierEpicInternal"),
    (0x00002000, "UnversionedProperties"),
    (0x00004000, "ContainsMapData"),
    (0x00008000, "IsSaving"),
    (0x00010000, "Compiling"),
    (0x00020000, "ContainsMap"),
    (0x00040000, "RequiresLocalizationGather"),
    (0x00080000, "LoadUncooked"),
    (0x00100000, "PlayInEditor"),
    (0x00200000, "ContainsScript"),
    (0x00400000, "DisallowExport"),
    (0x08000000, "CookGenerated"),
    (0x10000000, "DynamicImports"),
    (0x20000000, "RuntimeGenerated"),
    (0x40000000, "ReloadingForCooker"),
    (0x80000000, "FilterEditorOnly"),
];

/// EObjectFlags worth surfacing (RF_Load subset)
pub const OBJECT_FLAGS: &[(u32, &str)] = &[
    (0x00000001, "Public"),
    (0x00000002, "Standalone"),
    (0x00000004, "MarkAsNative"),
    (0x00000008, "Transactional"),
    (0x00000010, "ClassDefaultObject"),
    (0x00000020, "ArchetypeObject"),
    (0x00000040, "Transient"),
    (0x00000080, "MarkAsRootSet"),
    (0x00000200, "LoadCompleted"),
    (0x00000400, "InheritableComponentTemplate"),
    (0x00000800, "DuplicateTransient"),
    (0x00008000, "DefaultSubObject"),
    (0x00010000, "WasLoaded"),
    (0x00080000, "HasExternalPackage"),
];

pub fn flag_names(value: u32, table: &[(u32, &str)]) -> Vec<String> {
    table.iter().filter(|(bit, _)| value & bit != 0).map(|(_, n)| n.to_string()).collect()
}

#[derive(Debug, Default, Clone)]
pub struct Summary {
    pub legacy_file_version: i32,
    pub file_version_ue4: i32,
    pub file_version_ue5: i32,
    pub file_version_licensee: i32,
    pub total_header_size: i32,
    /// (guid, version)
    pub custom_versions: Vec<(String, i32)>,
    pub package_name: String,
    pub package_flags: u32,
    pub filter_editor_only: bool,
    pub name_count: i32,
    pub name_offset: i32,
    pub soft_object_paths_count: i32,
    pub soft_object_paths_offset: i32,
    pub export_count: i32,
    pub export_offset: i32,
    pub import_count: i32,
    pub import_offset: i32,
    pub soft_package_references_count: i32,
    pub soft_package_references_offset: i32,
    pub thumbnail_table_offset: i32,
    pub saved_by_engine_version: String,
    pub asset_registry_data_offset: i32,
}

impl Summary {
    pub fn custom_version(&self, guid: &str) -> i32 {
        self.custom_versions.iter().find(|(g, _)| g == guid).map(|(_, v)| *v).unwrap_or(-1)
    }
}

#[derive(Debug, Clone)]
pub struct ObjectImport {
    pub class_package: String,
    pub class_name: String,
    pub outer_index: i32,
    pub object_name: String,
}

#[derive(Debug, Clone)]
pub struct ObjectExport {
    pub class_index: i32,
    pub super_index: i32,
    pub template_index: i32,
    pub outer_index: i32,
    pub object_name: String,
    pub object_flags: u32,
    pub serial_size: i64,
    pub serial_offset: i64,
    pub is_asset: bool,
    pub script_start: i64,
    pub script_end: i64,
}

#[derive(Debug, Clone)]
pub struct AssetData {
    pub object_path: String,
    pub class: String,
    pub tags: Vec<(String, String)>,
}

#[derive(Debug, Clone)]
pub struct Thumbnail {
    pub class: String,
    pub object_path: String,
    pub width: i32,
    pub height: i32,
    /// Byte range of the image inside the package.
    pub data: std::ops::Range<usize>,
}

pub struct Package {
    pub data: Vec<u8>,
    /// Cooked packages keep export data in a sibling .uexp; uncooked ones inline it.
    pub uexp: Option<Vec<u8>>,
    pub names: Vec<String>,
    pub summary: Summary,
    pub imports: Vec<ObjectImport>,
    pub exports: Vec<ObjectExport>,
    pub soft_object_paths: Vec<String>,
    pub soft_package_references: Vec<String>,
    pub asset_data: Vec<AssetData>,
    pub thumbnails: Vec<Thumbnail>,
}

impl Package {
    pub fn open(path: &std::path::Path) -> Result<Package, String> {
        let data = std::fs::read(path).map_err(|e| format!("{}: {e}", path.display()))?;
        let uexp = std::fs::read(path.with_extension("uexp")).ok();
        Package::parse(data, uexp)
    }

    /// Header and tables are required; the sections after them are read
    /// best-effort, so one odd block never hides the properties.
    pub fn parse(data: Vec<u8>, uexp: Option<Vec<u8>>) -> Result<Package, String> {
        let summary = read_summary(&data).map_err(|e| e.to_string())?;
        let names = read_names(&data, &summary).map_err(|e| format!("name table: {e}"))?;
        let mut pkg = Package {
            data,
            uexp,
            names,
            summary,
            imports: Vec::new(),
            exports: Vec::new(),
            soft_object_paths: Vec::new(),
            soft_package_references: Vec::new(),
            asset_data: Vec::new(),
            thumbnails: Vec::new(),
        };
        pkg.imports = pkg.read_imports().map_err(|e| format!("import table: {e}"))?;
        pkg.exports = pkg.read_exports().map_err(|e| format!("export table: {e}"))?;
        pkg.soft_object_paths = pkg.read_soft_object_paths().unwrap_or_default();
        pkg.soft_package_references = pkg.read_soft_package_refs().unwrap_or_default();
        pkg.asset_data = pkg.read_asset_data().unwrap_or_default();
        pkg.thumbnails = pkg.read_thumbnails().unwrap_or_default();
        Ok(pkg)
    }

    fn reader(&self) -> Reader<'_> {
        Reader::new(&self.data, &self.names)
    }

    fn read_imports(&self) -> Res<Vec<ObjectImport>> {
        let s = &self.summary;
        let mut out = Vec::new();
        if s.import_count <= 0 {
            return Ok(out);
        }
        let mut ar = self.reader();
        ar.seek(s.import_offset as i64)?;
        for _ in 0..s.import_count {
            let class_package = ar.name()?;
            let class_name = ar.name()?;
            let outer_index = ar.package_index()?;
            let object_name = ar.name()?;
            if s.file_version_ue4 >= VER_UE4_NON_OUTER_PACKAGE_IMPORT && !s.filter_editor_only {
                ar.fname()?; // PackageName
            }
            if s.file_version_ue5 >= UE5_OPTIONAL_RESOURCES {
                ar.bool32()?; // bImportOptional
            }
            out.push(ObjectImport { class_package, class_name, outer_index, object_name });
        }
        Ok(out)
    }

    fn read_exports(&self) -> Res<Vec<ObjectExport>> {
        let s = &self.summary;
        let mut out = Vec::new();
        if s.export_count <= 0 {
            return Ok(out);
        }
        let mut ar = self.reader();
        ar.seek(s.export_offset as i64)?;
        let uses_unversioned = s.package_flags & PKG_UNVERSIONED_PROPERTIES != 0;
        for _ in 0..s.export_count {
            let class_index = ar.package_index()?;
            let super_index = ar.package_index()?;
            let template_index =
                if s.file_version_ue4 >= VER_UE4_TEMPLATE_INDEX_IN_COOKED_EXPORTS { ar.package_index()? } else { 0 };
            let outer_index = ar.package_index()?;
            let object_name = ar.name()?;
            let object_flags = ar.u32()?;
            let (serial_size, serial_offset) = if s.file_version_ue4 < VER_UE4_64BIT_EXPORTMAP_SERIALSIZES {
                (ar.i32()? as i64, ar.i32()? as i64)
            } else {
                (ar.i64()?, ar.i64()?)
            };
            ar.bool32()?; // bForcedExport
            ar.bool32()?; // bNotForClient
            ar.bool32()?; // bNotForServer
            if s.file_version_ue5 < UE5_REMOVE_OBJECT_EXPORT_PACKAGE_GUID {
                ar.guid()?;
            }
            if s.file_version_ue5 >= UE5_TRACK_OBJECT_EXPORT_IS_INHERITED {
                ar.bool32()?; // bIsInheritedInstance
            }
            ar.u32()?; // PackageFlags
            if s.file_version_ue4 >= VER_UE4_LOAD_FOR_EDITOR_GAME {
                ar.bool32()?; // bNotAlwaysLoadedForEditorGame
            }
            let is_asset = if s.file_version_ue4 >= VER_UE4_COOKED_ASSETS_IN_EDITOR_SUPPORT { ar.bool32()? } else { false };
            if s.file_version_ue5 >= UE5_OPTIONAL_RESOURCES {
                ar.bool32()?; // bGeneratePublicHash
            }
            if s.file_version_ue4 >= VER_UE4_PRELOAD_DEPENDENCIES_IN_COOKED_EXPORTS {
                for _ in 0..5 {
                    ar.i32()?; // FirstExportDependency and the four dependency counts
                }
            }
            let (script_start, script_end) = if !uses_unversioned && s.file_version_ue5 >= UE5_SCRIPT_SERIALIZATION_OFFSET {
                (ar.i64()?, ar.i64()?)
            } else {
                (0, 0)
            };
            out.push(ObjectExport {
                class_index,
                super_index,
                template_index,
                outer_index,
                object_name,
                object_flags,
                serial_size,
                serial_offset,
                is_asset,
                script_start,
                script_end,
            });
        }
        Ok(out)
    }

    fn read_soft_object_paths(&self) -> Res<Vec<String>> {
        let s = &self.summary;
        let mut out = Vec::new();
        if s.soft_object_paths_count <= 0 || s.soft_object_paths_offset <= 0 {
            return Ok(out);
        }
        let mut ar = self.reader();
        ar.seek(s.soft_object_paths_offset as i64)?;
        for _ in 0..s.soft_object_paths_count {
            out.push(read_soft_object_path(&mut ar, s)?);
        }
        Ok(out)
    }

    fn read_soft_package_refs(&self) -> Res<Vec<String>> {
        let s = &self.summary;
        let mut out = Vec::new();
        if s.soft_package_references_count <= 0 || s.soft_package_references_offset <= 0 {
            return Ok(out);
        }
        let mut ar = self.reader();
        ar.seek(s.soft_package_references_offset as i64)?;
        for _ in 0..s.soft_package_references_count {
            // Editor packages store these as FName, cooked ones as FString.
            out.push(if s.filter_editor_only { ar.fstring()? } else { ar.name()? });
        }
        Ok(out)
    }

    fn read_asset_data(&self) -> Res<Vec<AssetData>> {
        let s = &self.summary;
        let mut out = Vec::new();
        if s.asset_registry_data_offset <= 0 || s.asset_registry_data_offset as usize >= self.data.len() {
            return Ok(out);
        }
        let mut ar = self.reader();
        ar.seek(s.asset_registry_data_offset as i64)?;
        let pre_dependency_format =
            s.file_version_ue4 < VER_UE4_ASSETREGISTRY_DEPENDENCYFLAGS || s.filter_editor_only;
        if !pre_dependency_format {
            ar.i64()?; // DependencyDataOffset
        }
        let object_count = ar.count(1 << 20)?;
        for _ in 0..object_count {
            let object_path = ar.fstring()?;
            let class = ar.fstring()?;
            let tag_count = ar.count(1 << 20)?;
            let mut tags = Vec::new();
            for _ in 0..tag_count {
                let key = ar.fstring()?;
                let value = ar.fstring()?;
                if !key.is_empty() {
                    tags.push((key, value));
                }
            }
            out.push(AssetData { object_path, class, tags });
        }
        Ok(out)
    }

    fn read_thumbnails(&self) -> Res<Vec<Thumbnail>> {
        let s = &self.summary;
        let mut out = Vec::new();
        if s.thumbnail_table_offset <= 0 || s.thumbnail_table_offset as usize >= self.data.len() {
            return Ok(out);
        }
        let mut ar = self.reader();
        ar.seek(s.thumbnail_table_offset as i64)?;
        let count = ar.i32()?;
        if !(0..=1 << 16).contains(&count) {
            return Ok(out);
        }
        let mut entries = Vec::new();
        for _ in 0..count {
            entries.push((ar.fstring()?, ar.fstring()?, ar.i32()?));
        }
        for (class, object_path, offset) in entries {
            let mut read = || -> Res<Thumbnail> {
                ar.seek(offset as i64)?;
                let width = ar.i32()?;
                let height = ar.i32()?;
                let size = ar.i32()?;
                let start = ar.tell();
                ar.bytes(size.max(0) as usize)?;
                Ok(Thumbnail {
                    class: class.clone(),
                    object_path: object_path.clone(),
                    width,
                    height: height.abs(), // negative height marks a compressed image
                    data: start..start + size.max(0) as usize,
                })
            };
            if let Ok(t) = read() {
                out.push(t);
            }
        }
        Ok(out)
    }

    pub fn thumbnail_bytes(&self, t: &Thumbnail) -> &[u8] {
        &self.data[t.data.clone()]
    }

    fn walk_outer_chain(&self, start_index: i32) -> Vec<String> {
        let mut parts = Vec::new();
        let mut seen = std::collections::HashSet::new();
        let mut current = start_index;
        while current != 0 {
            if !seen.insert(current) {
                parts.push("<cycle>".to_string());
                break;
            }
            let (name, outer) = if current > 0 {
                match self.exports.get((current - 1) as usize) {
                    Some(e) => (&e.object_name, e.outer_index),
                    None => {
                        parts.push(format!("<badexport#{}>", current - 1));
                        break;
                    }
                }
            } else {
                match self.imports.get((-current - 1) as usize) {
                    Some(i) => (&i.object_name, i.outer_index),
                    None => {
                        parts.push(format!("<badimport#{}>", -current - 1));
                        break;
                    }
                }
            };
            parts.push(name.clone());
            current = outer;
        }
        parts.reverse();
        parts
    }

    pub fn import_path(&self, index: usize) -> String {
        join_object_path(&self.walk_outer_chain(-(index as i32 + 1)))
    }

    pub fn export_path(&self, index: usize) -> String {
        join_object_path(&self.walk_outer_chain(index as i32 + 1))
    }

    /// Human-readable path for an FPackageIndex.
    pub fn resolve_index(&self, package_index: i32) -> String {
        match package_index {
            0 => "None".to_string(),
            i if i < 0 => self.import_path((-i - 1) as usize),
            i => self.export_path((i - 1) as usize),
        }
    }

    pub fn export_class(&self, exp: &ObjectExport) -> String {
        self.resolve_index(exp.class_index)
    }

    /// Raw serialized bytes of one export (clamped to what the file holds).
    pub fn export_data(&self, exp: &ObjectExport) -> &[u8] {
        let (buf, base) = match &self.uexp {
            // Cooked layout: export offsets are relative to the end of the .uasset header.
            Some(u) => (u.as_slice(), exp.serial_offset - self.summary.total_header_size as i64),
            None => (self.data.as_slice(), exp.serial_offset),
        };
        let start = base.clamp(0, buf.len() as i64) as usize;
        let end = (base + exp.serial_size).clamp(start as i64, buf.len() as i64) as usize;
        &buf[start..end]
    }

    pub fn is_cooked(&self) -> bool {
        self.summary.package_flags & PKG_COOKED != 0 || self.summary.filter_editor_only
    }

    pub fn uses_unversioned_properties(&self) -> bool {
        self.summary.package_flags & PKG_UNVERSIONED_PROPERTIES != 0
    }
}

/// First element is the package, the rest are object / subobject names.
fn join_object_path(parts: &[String]) -> String {
    match parts.len() {
        0 => String::new(),
        1 => parts[0].clone(),
        2 => format!("{}.{}", parts[0], parts[1]),
        _ => format!("{}.{}:{}", parts[0], parts[1], parts[2..].join(":")),
    }
}

fn read_summary(data: &[u8]) -> Res<Summary> {
    let mut s = Summary::default();
    if data.len() < 32 {
        return archive_err(format!("file too small to be a package ({} bytes)", data.len()));
    }
    let names: Vec<String> = Vec::new();
    let mut ar = Reader::new(data, &names);
    let tag = ar.u32()?;
    if tag == PACKAGE_FILE_TAG_SWAPPED {
        return archive_err("byte-swapped (big endian) package is not supported".into());
    }
    if tag != PACKAGE_FILE_TAG {
        return archive_err(format!("not a UE package: magic 0x{tag:08X}"));
    }
    s.legacy_file_version = ar.i32()?;
    if s.legacy_file_version >= 0 || s.legacy_file_version < -9 {
        return archive_err(format!("unsupported legacy file version {}", s.legacy_file_version));
    }
    if s.legacy_file_version != -4 {
        ar.i32()?; // LegacyUE3Version
    }
    s.file_version_ue4 = ar.i32()?;
    s.file_version_ue5 = if s.legacy_file_version <= -8 { ar.i32()? } else { 0 };
    s.file_version_licensee = ar.i32()?;

    if s.file_version_ue5 >= UE5_PACKAGE_SAVED_HASH {
        ar.bytes(20)?; // SavedHash
        s.total_header_size = ar.i32()?;
    }

    if s.legacy_file_version <= -2 {
        let count = ar.i32()?;
        if !(0..=4096).contains(&count) {
            return archive_err(format!("implausible custom version count {count}"));
        }
        for _ in 0..count {
            if s.legacy_file_version == -2 {
                // Enums format: an invented GUID of three zeroes plus the tag.
                let tag = ar.u32()?;
                s.custom_versions.push((format!("00000000000000000000000{tag:08X}"), ar.i32()?));
            } else if s.legacy_file_version >= -5 {
                // Guids format: key, version and a friendly name.
                let guid = ar.guid()?;
                let version = ar.i32()?;
                ar.fstring()?;
                s.custom_versions.push((guid, version));
            } else {
                let guid = ar.guid()?;
                s.custom_versions.push((guid, ar.i32()?));
            }
        }
    }

    if s.file_version_ue5 < UE5_PACKAGE_SAVED_HASH {
        s.total_header_size = ar.i32()?;
    }

    s.package_name = ar.fstring()?;
    s.package_flags = ar.u32()?;
    s.filter_editor_only = s.package_flags & PKG_FILTER_EDITOR_ONLY != 0;

    s.name_count = ar.i32()?;
    s.name_offset = ar.i32()?;

    if s.file_version_ue5 >= UE5_ADD_SOFTOBJECTPATH_LIST {
        s.soft_object_paths_count = ar.i32()?;
        s.soft_object_paths_offset = ar.i32()?;
    }

    if !s.filter_editor_only && s.file_version_ue4 >= VER_UE4_ADDED_PACKAGE_SUMMARY_LOCALIZATION_ID {
        ar.fstring()?; // LocalizationId
    }

    if s.file_version_ue4 >= VER_UE4_SERIALIZE_TEXT_IN_PACKAGES {
        ar.i32()?; // GatherableTextDataCount
        ar.i32()?; // GatherableTextDataOffset
    }

    s.export_count = ar.i32()?;
    s.export_offset = ar.i32()?;
    s.import_count = ar.i32()?;
    s.import_offset = ar.i32()?;

    if s.file_version_ue5 >= UE5_VERSE_CELLS {
        for _ in 0..4 {
            ar.i32()?; // Cell export/import counts and offsets
        }
    }

    if s.file_version_ue5 >= UE5_METADATA_SERIALIZATION_OFFSET {
        ar.i32()?; // MetaDataOffset
    }

    ar.i32()?; // DependsOffset

    if s.file_version_ue4 >= VER_UE4_ADD_STRING_ASSET_REFERENCES_MAP {
        s.soft_package_references_count = ar.i32()?;
        s.soft_package_references_offset = ar.i32()?;
    }

    if s.file_version_ue4 >= VER_UE4_ADDED_SEARCHABLE_NAMES {
        ar.i32()?; // SearchableNamesOffset
    }

    s.thumbnail_table_offset = ar.i32()?;

    if s.file_version_ue5 < UE5_PACKAGE_SAVED_HASH {
        ar.guid()?; // Guid
    }

    if !s.filter_editor_only && s.file_version_ue4 >= VER_UE4_ADDED_PACKAGE_OWNER {
        ar.guid()?; // PersistentGuid
        if s.file_version_ue4 < VER_UE4_NON_OUTER_PACKAGE_IMPORT {
            ar.guid()?; // OwnerPersistentGuid, present for one engine version only
        }
    }

    let generations = ar.count(1 << 22)?;
    for _ in 0..generations {
        ar.i32()?; // ExportCount
        ar.i32()?; // NameCount
    }

    s.saved_by_engine_version = if s.file_version_ue4 >= VER_UE4_ENGINE_VERSION_OBJECT {
        ar.engine_version()?
    } else {
        format!("4.0.0 CL {}", ar.i32()?)
    };

    if s.file_version_ue4 >= VER_UE4_PACKAGE_SUMMARY_HAS_COMPATIBLE_ENGINE_VERSION {
        ar.engine_version()?;
    }

    ar.u32()?; // CompressionFlags
    let compressed_chunks = ar.i32()?;
    if compressed_chunks != 0 {
        return archive_err(format!("package-level compression is not supported ({compressed_chunks} chunks)"));
    }
    ar.u32()?; // PackageSource
    let additional = ar.count(1 << 22)?; // AdditionalPackagesToCook, no longer used
    for _ in 0..additional {
        ar.fstring()?;
    }

    if s.legacy_file_version > -7 {
        ar.i32()?; // NumTextureAllocations, always 0
    }

    s.asset_registry_data_offset = ar.i32()?;
    ar.i64()?; // BulkDataStartOffset

    if s.file_version_ue4 >= VER_UE4_WORLD_LEVEL_INFO {
        ar.i32()?; // WorldTileInfoDataOffset
    }

    if s.file_version_ue4 >= VER_UE4_CHANGED_CHUNKID_TO_BE_AN_ARRAY_OF_CHUNKIDS {
        let chunks = ar.count(1 << 22)?;
        for _ in 0..chunks {
            ar.i32()?;
        }
    } else if s.file_version_ue4 >= VER_UE4_ADDED_CHUNKID_TO_ASSETDATA_AND_UPACKAGE {
        ar.i32()?;
    }

    if s.file_version_ue4 >= VER_UE4_PRELOAD_DEPENDENCIES_IN_COOKED_EXPORTS {
        ar.i32()?; // PreloadDependencyCount
        ar.i32()?; // PreloadDependencyOffset
    }

    if s.file_version_ue5 >= UE5_NAMES_REFERENCED_FROM_EXPORT_DATA {
        ar.i32()?;
    }
    if s.file_version_ue5 >= UE5_PAYLOAD_TOC {
        ar.i64()?;
    }
    if s.file_version_ue5 >= UE5_DATA_RESOURCES {
        ar.i32()?;
    }
    Ok(s)
}

fn read_names(data: &[u8], s: &Summary) -> Res<Vec<String>> {
    let mut names = Vec::new();
    if s.name_count <= 0 {
        return Ok(names);
    }
    let none: Vec<String> = Vec::new();
    let mut ar = Reader::new(data, &none);
    ar.seek(s.name_offset as i64)?;
    for _ in 0..s.name_count {
        let text = ar.fstring()?;
        if s.file_version_ue4 >= VER_UE4_NAME_HASHES_SERIALIZED {
            ar.skip(4)?; // two uint16 hashes, unused on load
        }
        names.push(text);
    }
    Ok(names)
}

/// FSoftObjectPath::SerializePath - layout depends on the package version.
pub fn read_soft_object_path(ar: &mut Reader, s: &Summary) -> Res<String> {
    if s.file_version_ue4 < VER_UE4_ADDED_SOFT_OBJECT_PATH {
        return ar.fstring();
    }
    let (asset_path, sub_path) = if s.file_version_ue5 < UE5_FSOFTOBJECTPATH_REMOVE_ASSET_PATH_FNAMES {
        (ar.name()?, ar.fstring()?)
    } else {
        let package_name = ar.name()?;
        let asset_name = ar.name()?;
        let sub_path = ar.fstring()?;
        let asset_path = if package_name == "None" || package_name.is_empty() {
            String::new()
        } else if asset_name == "None" || asset_name.is_empty() {
            package_name
        } else {
            format!("{package_name}.{asset_name}")
        };
        (asset_path, sub_path)
    };
    if asset_path.is_empty() {
        return Ok(sub_path);
    }
    Ok(if sub_path.is_empty() { asset_path } else { format!("{asset_path}:{sub_path}") })
}
