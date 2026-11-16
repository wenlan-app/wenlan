// SPDX-License-Identifier: Apache-2.0

fn main() {
    println!("cargo:rerun-if-changed=src/macos_host_activity.m");
    generate_web_assets();

    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("macos") {
        return;
    }

    let mut build = cc::Build::new();
    build.file("src/macos_host_activity.m").flag("-fobjc-arc");
    let objects = build.compile_intermediates();

    // Xcode's BSD ar rejects the GNU `D` flag that cc probes before falling
    // back, which turns every otherwise-clean build into a Cargo warning. Use
    // cc's target-aware archiver/ranlib commands with Apple's deterministic
    // ZERO_AR_DATE contract, then publish normal native-library metadata so
    // downstream crates linking wenlan-server also receive this bridge.
    let out_dir = std::path::PathBuf::from(
        std::env::var_os("OUT_DIR").expect("Cargo must provide OUT_DIR to build scripts"),
    );
    let archive = out_dir.join("libwenlan_macos_host_activity.a");
    if archive.exists() {
        std::fs::remove_file(&archive).expect("failed to replace macOS host-activity archive");
    }
    let archive_status = build
        .get_archiver()
        .env("ZERO_AR_DATE", "1")
        .arg("cq")
        .arg(&archive)
        .args(&objects)
        .status()
        .expect("failed to run the macOS host-activity archiver");
    assert!(
        archive_status.success(),
        "macOS host-activity archiver failed"
    );
    let ranlib_status = build
        .get_ranlib()
        .env("ZERO_AR_DATE", "1")
        .arg(&archive)
        .status()
        .expect("failed to index the macOS host-activity archive");
    assert!(ranlib_status.success(), "macOS host-activity ranlib failed");

    println!("cargo:rustc-link-search=native={}", out_dir.display());
    println!("cargo:rustc-link-lib=static=wenlan_macos_host_activity");
    println!("cargo:rustc-link-lib=framework=Foundation");
    println!("cargo:rustc-link-lib=framework=CoreGraphics");
    println!("cargo:rustc-link-lib=framework=IOKit");
}

fn generate_web_assets() {
    use std::fmt::Write as _;
    use std::path::{Path, PathBuf};

    let root = PathBuf::from("../../dist/web");
    println!("cargo:rerun-if-changed={}", root.display());
    let mut files = Vec::new();
    fn visit(dir: &Path, files: &mut Vec<PathBuf>) {
        let Ok(entries) = std::fs::read_dir(dir) else {
            return;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                visit(&path, files);
            } else if path.is_file() {
                files.push(path);
            }
        }
    }
    visit(&root, &mut files);
    files.sort();
    let mut generated = String::from("pub(crate) static WEB_ASSETS: &[EmbeddedAsset] = &[\n");
    for path in files {
        let relative = path
            .strip_prefix(&root)
            .expect("web asset must be below dist/web");
        let relative = relative.to_string_lossy().replace('\\', "/");
        let absolute = std::fs::canonicalize(&path).expect("canonicalize web asset");
        let extension = path
            .extension()
            .and_then(|value| value.to_str())
            .unwrap_or("");
        let mime = match extension {
            "html" => "text/html; charset=utf-8",
            "js" => "text/javascript; charset=utf-8",
            "css" => "text/css; charset=utf-8",
            "json" => "application/json",
            "svg" => "image/svg+xml",
            "png" => "image/png",
            "jpg" | "jpeg" => "image/jpeg",
            "webp" => "image/webp",
            "woff2" => "font/woff2",
            "pdf" => "application/pdf",
            _ => "application/octet-stream",
        };
        writeln!(
            &mut generated,
            "EmbeddedAsset {{ path: {:?}, content_type: {:?}, bytes: include_bytes!({:?}) }},",
            relative, mime, absolute
        )
        .unwrap();
    }
    generated.push_str("];\n");
    let out = PathBuf::from(std::env::var_os("OUT_DIR").expect("Cargo must provide OUT_DIR"));
    std::fs::write(out.join("web_assets.rs"), generated).expect("write embedded web asset table");
}
