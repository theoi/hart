//! WebAssembly bindings for hart. Owned by the web builder; this is a placeholder export.

use wasm_bindgen::prelude::*;

/// The hart-core version string.
#[wasm_bindgen]
pub fn version() -> String {
    hart_core::VERSION.to_string()
}
