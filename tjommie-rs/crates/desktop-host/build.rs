//! The build identity (spec §4): TJOMMIE_IDENTITY at build time picks which app this binary is, `dev` when unset.

const IDENTITIES: [&str; 4] = ["dev", "production", "updatetest", "e2e"];

fn main() -> Result<(), String> {
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
