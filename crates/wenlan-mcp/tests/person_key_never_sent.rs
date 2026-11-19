// SPDX-License-Identifier: Apache-2.0
//! UX-SKILL-13: an AI tool's connection never speaks as the person. Nothing
//! in `wenlan-mcp` names the person marker, the person key header, or the
//! key file, so no tool call can carry them, whatever a tool asks for.

use std::path::Path;

fn rust_files(dir: &Path, out: &mut Vec<std::path::PathBuf>) {
    for entry in std::fs::read_dir(dir).unwrap() {
        let path = entry.unwrap().path();
        if path.is_dir() {
            rust_files(&path, out);
        } else if path.extension().is_some_and(|ext| ext == "rs") {
            out.push(path);
        }
    }
}

#[test]
fn ux_skill_13_mcp_never_sends_the_person_key_or_marker() {
    let src = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let mut files = Vec::new();
    rust_files(&src, &mut files);
    assert!(files.len() > 5, "found the sources: {files:?}");
    let forbidden = [
        wenlan_types::requests::PERSON_KEY_HEADER,
        "PERSON_KEY_HEADER",
        wenlan_types::requests::CALLER_HEADER,
        "CALLER_HEADER",
        "PERSON_CALLER_VALUE",
        wenlan_types::person_key::FILE_NAME,
        "person_key::",
    ];
    for file in files {
        let text = std::fs::read_to_string(&file).unwrap();
        for word in forbidden {
            assert!(
                !text.contains(word),
                "{} names {word}: an AI tool must never speak as the person",
                file.display()
            );
        }
    }
}
