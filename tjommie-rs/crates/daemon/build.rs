//! Embeds the app version as `TJOMMIE_VERSION` (releases bump the package.json files, AGENTS.md, not Cargo.toml) and the
//! build identity (spec §4): `TJOMMIE_IDENTITY` at build time picks which app this binary is, `dev` when unset. The
//! contract is `desktop-host`'s build.rs. The daemon reads the identity as `cfg(tjommie_identity = "...")`; today only
//! the `provider-fixtures` guard in lib.rs does.

use serde::Deserialize;

const IDENTITIES: [&str; 4] = ["dev", "production", "updatetest", "e2e"];

#[derive(Deserialize)]
struct Package {
    version: String,
}

fn main() -> Result<(), String> {
    let file = "../../../package.json"; // The repo root, from this crate's folder.
    println!("cargo::rerun-if-changed={file}");
    let text = std::fs::read_to_string(file).expect("the repo-root package.json is readable");
    let package: Package =
        serde_json::from_str(&text).expect("the repo-root package.json has a version");
    println!("cargo::rustc-env=TJOMMIE_VERSION={}", package.version);

    println!("cargo::rerun-if-env-changed=TJOMMIE_IDENTITY");
    println!(
        "cargo::rustc-check-cfg=cfg(tjommie_identity, values(\"dev\", \"production\", \"updatetest\", \"e2e\"))"
    );
    let identity = std::env::var("TJOMMIE_IDENTITY").unwrap_or_else(|_| "dev".to_owned());
    if !IDENTITIES.contains(&identity.as_str()) {
        return Err(format!(
            "TJOMMIE_IDENTITY={identity}: expected one of {IDENTITIES:?}"
        ));
    }
    println!("cargo::rustc-cfg=tjommie_identity=\"{identity}\"");
    Ok(())
}
