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
pub const VERSION: u32 = 2;

/// Turn a version `from` document into a [`VERSION`] one.
pub(crate) fn migrate(mut doc: Value, from: u32) -> Value {
    if from < 2 {
        // v2 remembers the last captures, under `last`. A v1 file has none yet.
        if let Some(map) = doc.as_object_mut() {
            map.entry("last")
                .or_insert_with(|| Value::Object(Default::default()));
        }
    }
    doc
}
