//! Per-ecosystem hacks. Each submodule owns the special-casing for one
//! language family — knowledge that doesn't generalize but earns its keep
//! on real repos in that ecosystem.
//!
//! Kept under one roof so a future maintainer can grep "what does tingle
//! special-case for X?" and find a single file. Core modules
//! (`enumerate`, `resolve`, `render`) call into here rather than embedding
//! per-language branches.

pub mod jvm;
pub mod vue;
