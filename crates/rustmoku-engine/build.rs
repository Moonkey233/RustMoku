#![forbid(unsafe_code)]

#[path = "src/line_classifier.rs"]
mod line_classifier;

fn build_identity() -> std::io::Result<()> {
    use sha2::{Digest, Sha256};
    use std::path::Path;
    fn sources(path: &Path, files: &mut Vec<std::path::PathBuf>) -> std::io::Result<()> {
        if path.is_dir() {
            for entry in std::fs::read_dir(path)? {
                sources(&entry?.path(), files)?;
            }
        } else {
            files.push(path.to_owned());
        }
        Ok(())
    }
    let root = Path::new("../..");
    let mut files = vec![root.join("Cargo.toml"), root.join("Cargo.lock")];
    for name in ["rustmoku-core", "rustmoku-engine", "rustmoku-simd"] {
        let base = root.join("crates").join(name);
        files.push(base.join("Cargo.toml"));
        if base.join("build.rs").is_file() {
            files.push(base.join("build.rs"));
        }
        println!("cargo:rerun-if-changed={}", base.join("src").display());
        sources(&base.join("src"), &mut files)?;
    }
    files.sort();
    let mut hash = Sha256::new();
    hash.update(b"rustmoku-engine-build-v1\0");
    for file in files {
        println!("cargo:rerun-if-changed={}", file.display());
        hash.update(
            file.strip_prefix(root)
                .unwrap()
                .to_string_lossy()
                .replace('\\', "/"),
        );
        hash.update([0]);
        // Git checkout line endings must not change the source identity.
        hash.update(std::fs::read_to_string(file)?.replace("\r\n", "\n"));
        hash.update([0]);
    }
    for key in [
        "TARGET",
        "PROFILE",
        "CARGO_CFG_TARGET_FEATURE",
        "CARGO_ENCODED_RUSTFLAGS",
    ] {
        hash.update(key);
        hash.update(std::env::var(key).unwrap_or_default());
        hash.update([0]);
    }
    let mut features: Vec<_> = std::env::vars()
        .filter(|(key, _)| key.starts_with("CARGO_FEATURE_"))
        .collect();
    features.sort();
    for (key, value) in features {
        hash.update(key);
        hash.update(value);
    }
    let rustc = std::process::Command::new(std::env::var_os("RUSTC").expect("Cargo RUSTC"))
        .arg("--version")
        .output()?;
    if !rustc.status.success() {
        return Err(std::io::Error::other("cannot identify Rust compiler"));
    }
    hash.update(rustc.stdout);
    println!(
        "cargo:rustc-env=RUSTMOKU_ENGINE_BUILD_ID={:x}",
        hash.finalize()
    );
    Ok(())
}

fn main() -> std::io::Result<()> {
    build_identity()?;
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
