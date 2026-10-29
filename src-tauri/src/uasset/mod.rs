//! Reading Unreal .uasset / .umap packages without the editor - header, tables,
//! tagged properties and Blueprint pins - and diffing two revisions of one.
//!
//! A translation of the Python reader behind the `uasset-read` skill
//! (~/Claude/ue-scripts/uasset), kept to its layout so the two can be compared
//! output for output (the `uassetdump` dev bin prints the same JSON).

pub mod archive;
pub mod canvas;
pub mod diff;
pub mod doc;
pub mod graph;
pub mod graphs;
pub mod package;
pub mod properties;
pub mod summary;
