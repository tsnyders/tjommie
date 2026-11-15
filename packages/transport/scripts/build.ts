import { createHash } from "node:crypto";
import { execFileSync } from "node:child_process";
import { existsSync, mkdirSync, writeFileSync } from "node:fs";
import { homedir } from "node:os";
import { join, resolve } from "node:path";

const root = resolve(import.meta.dirname, "..");
const version = "0.2.129";
const archiveName = `wasm-bindgen-${version}-x86_64-pc-windows-msvc`;
const archiveHash = "79e348c169d0c10c0b287647f1e753d0985353ca96800bc29be8f4261baf13e6";
const tools = join(root, ".tools");
let bindgen = process.env.TJOMMIE_WASM_BINDGEN;
if (!bindgen) {
  if (process.platform !== "win32" || process.arch !== "x64") throw new Error(`Set TJOMMIE_WASM_BINDGEN to wasm-bindgen ${version} on this platform.`);
  bindgen = join(tools, archiveName, "wasm-bindgen.exe");
  if (!existsSync(bindgen)) {
    mkdirSync(tools, { recursive: true });
    const response = await fetch(`https://github.com/wasm-bindgen/wasm-bindgen/releases/download/${version}/${archiveName}.tar.gz`, { signal: AbortSignal.timeout(120_000) });
    if (!response.ok) throw new Error("Could not download the pinned wasm-bindgen build tool.");
    const archive = Buffer.from(await response.arrayBuffer());
    if (createHash("sha256").update(archive).digest("hex") !== archiveHash) throw new Error("wasm-bindgen archive checksum mismatch.");
    const path = join(tools, `${archiveName}.tar.gz`); writeFileSync(path, archive);
    execFileSync("tar", ["-xzf", path, "-C", tools], { windowsHide: true, stdio: "inherit" });
  }
}
const toolVersion = execFileSync(bindgen, ["--version"], { windowsHide: true, encoding: "utf8" }).trim();
if (toolVersion !== `wasm-bindgen ${version}`) throw new Error(`Expected wasm-bindgen ${version}.`);
// A Rust panic message carries the path of the file it came from. For a dependency or the standard library that is the builder's own folder
// (its .cargo\registry and .rustup\toolchains folders, under the person's profile), and it ended up inside the committed wasm. These flags rewrite those
// prefixes while compiling, so a build anywhere carries the same paths and none of them is a person's. The flags go in
// CARGO_ENCODED_RUSTFLAGS (separated by 0x1F, so a folder with a space in its name survives), together with any RUSTFLAGS already set,
// which cargo would otherwise ignore.
const remap = [
  [process.env.CARGO_HOME ?? join(homedir(), ".cargo"), "/cargo"],
  [process.env.RUSTUP_HOME ?? join(homedir(), ".rustup"), "/rustup"],
  [resolve(root, "..", ".."), "/src"],
].map(([from, to]) => `--remap-path-prefix=${from}=${to}`);
const separator = String.fromCharCode(31);
const inherited = process.env.CARGO_ENCODED_RUSTFLAGS ? process.env.CARGO_ENCODED_RUSTFLAGS.split(separator) : (process.env.RUSTFLAGS ?? "").split(/\s+/).filter(Boolean);
execFileSync("cargo", ["build", "--locked", "--release", "--target", "wasm32-unknown-unknown", "--manifest-path", join(root, "native", "Cargo.toml")], {
  windowsHide: true,
  stdio: "inherit",
  env: { ...process.env, CARGO_ENCODED_RUSTFLAGS: [...inherited, ...remap].join(separator) },
});
execFileSync(bindgen, [join(root, "native", "target", "wasm32-unknown-unknown", "release", "tjommie_noise.wasm"), "--target", "web", "--out-dir", join(root, "wasm"), "--out-name", "tjommie_noise", "--no-typescript"], { windowsHide: true, stdio: "inherit" });
console.log("Built the pinned Snow Noise module for Node and browser use.");
