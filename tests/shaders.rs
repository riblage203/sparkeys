//! Parses and validates every WGSL shader so mistakes fail `cargo test`
//! instead of panicking when the app creates its pipelines.

use std::path::Path;

#[test]
fn all_shaders_validate() {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("src/render/shaders");
    let mut count = 0;
    for entry in std::fs::read_dir(&dir).expect("shader directory exists") {
        let path = entry.expect("readable entry").path();
        if path.extension().is_none_or(|e| e != "wgsl") {
            continue;
        }
        let source = std::fs::read_to_string(&path).expect("readable shader");
        let module = naga::front::wgsl::parse_str(&source)
            .unwrap_or_else(|e| panic!("{}: {}", path.display(), e.emit_to_string(&source)));
        naga::valid::Validator::new(naga::valid::ValidationFlags::all(), naga::valid::Capabilities::default())
            .validate(&module)
            .unwrap_or_else(|e| panic!("{}: {}", path.display(), e.emit_to_string(&source)));
        count += 1;
    }
    assert!(count >= 6, "expected all shaders, found {count}");
}
