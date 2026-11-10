//! Embeds the app version as `TJOMMIE_VERSION`. Releases bump the package.json files (AGENTS.md), not Cargo.toml.

use serde::Deserialize;

#[derive(Deserialize)]
struct Package {
    version: String,
}

fn main() {
    let file = "../../../package.json"; // The repo root, from this crate's folder.
    println!("cargo::rerun-if-changed={file}");
    let text = std::fs::read_to_string(file).expect("the repo-root package.json is readable");
    let package: Package =
        serde_json::from_str(&text).expect("the repo-root package.json has a version");
    println!("cargo::rustc-env=TJOMMIE_VERSION={}", package.version);
}
