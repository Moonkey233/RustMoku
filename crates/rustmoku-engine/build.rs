#![forbid(unsafe_code)]

#[path = "src/line_classifier.rs"]
mod line_classifier;

fn main() -> std::io::Result<()> {
    println!("cargo:rerun-if-changed=src/line_classifier.rs");
    println!("cargo:rerun-if-changed=build.rs");
    // Build-time generation avoids expensive const interpretation and runtime
    // initialization/synchronization. Each entry is exactly two color bytes.
    let mut table = Vec::with_capacity(2 * 65_536);
    let mut tactical = Vec::with_capacity(8 * 65_536);
    for key in 0..=u16::MAX {
        table.push(line_classifier::classify(key, 1));
        table.push(line_classifier::classify(key, 2));
        for color in [1, 2] {
            tactical.extend_from_slice(&line_classifier::tactical_metadata(key, color));
        }
    }
    let output = std::env::var_os("OUT_DIR").expect("Cargo must provide OUT_DIR");
    std::fs::write(std::path::Path::new(&output).join("patterns.bin"), table)?;
    std::fs::write(
        std::path::Path::new(&output).join("threat_meta.bin"),
        tactical,
    )
}
