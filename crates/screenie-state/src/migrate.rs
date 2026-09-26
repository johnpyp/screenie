//! Bringing old state files up to date.
//!
//! The file is migrated as an untyped document, before it's read as [`crate::State`],
//! so each step only needs to know the layout it starts from.
//!
//! To change the layout:
//!
//! 1. Bump [`VERSION`].
//! 2. Add a step at the end of [`migrate`]: `if from < N { ... }`, turning a version N-1
//!    document into a version N one. Never edit a released step; later steps build on it.
//! 3. Add `tests/fixtures/vN.yaml`, a file as version N writes it. The fixture test loads
//!    every version's fixture through the chain, so a missing step shows up there.

use serde_json::Value;

/// The layout this build reads and writes.
pub const VERSION: u32 = 1;

/// Turn a version `from` document into a [`VERSION`] one.
#[allow(unused_mut, unused_variables)] // until the first step exists
pub(crate) fn migrate(mut doc: Value, from: u32) -> Value {
    // Version 1 is the first layout, so there's nothing to do yet. Steps go here, in
    // order, e.g.:
    //
    // if from < 2 {
    //     // v2 renamed `editor.size` to `editor.width`.
    //     if let Some(editor) = doc.get_mut("editor").and_then(Value::as_object_mut)
    //         && let Some(size) = editor.remove("size")
    //     {
    //         editor.insert("width".into(), size);
    //     }
    // }
    doc
}
