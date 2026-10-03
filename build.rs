//! Embeds the bundled `modules/` directory into the binary so a single downloaded
//! executable can bootstrap its modules on first launch.

use std::fmt::Write as _;
use std::path::Path;

fn main() {
    let root = std::env::var("CARGO_MANIFEST_DIR").expect("cargo sets CARGO_MANIFEST_DIR");
    let modules_dir = Path::new(&root).join("modules");
    println!("cargo:rerun-if-changed=modules");
    let target = std::env::var("TARGET").unwrap_or_else(|_| "unknown".to_string());
    println!("cargo:rustc-env=FLY_TARGET={target}");

    let mut names = std::fs::read_dir(&modules_dir)
        .map(|entries| {
            entries
                .filter_map(|entry| entry.ok())
                .map(|entry| entry.file_name().to_string_lossy().to_string())
                .filter(|name| name.ends_with(".lua") || name.ends_with(".lua.manifest.json"))
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    names.sort();

    let mut out = String::from("pub const BUNDLED_FILES: &[(&str, &str)] = &[\n");
    for name in names {
        println!("cargo:rerun-if-changed=modules/{name}");
        let path = modules_dir.join(&name);
        writeln!(
            out,
            "    ({name:?}, include_str!({:?})),",
            path.to_string_lossy()
        )
        .expect("writing to a String cannot fail");
    }
    out.push_str("];\n");

    let out_dir = std::env::var("OUT_DIR").expect("cargo sets OUT_DIR");
    std::fs::write(Path::new(&out_dir).join("bundled_modules.rs"), out)
        .expect("OUT_DIR is writable");
}
