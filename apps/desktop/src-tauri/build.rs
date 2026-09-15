use serde_json::Value;
use std::{env, fs, path::PathBuf};

fn main() {
    tauri_build::build();

    // Tauri generates the runtime `BundleConfig` without `publisher`, and a
    // packaged app ships no `tauri.conf.json`, so the About panel's author is
    // resolved here from the config the bundler reads and baked into the binary.
    println!("cargo:rerun-if-env-changed=TAURI_CONFIG");
    println!("cargo:rustc-env=OXYVIEWER_ABOUT_AUTHOR={}", about_author());
}

/// Resolves `bundle.publisher` the way `tauri-build` resolves the whole config:
/// the base file, overridden by the JSON patch the CLI passes in `TAURI_CONFIG`.
fn about_author() -> String {
    let manifest_dir = PathBuf::from(env::var("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR"));
    let path = manifest_dir.join("tauri.conf.json");
    println!("cargo:rerun-if-changed={}", path.display());
    let base: Value = fs::read_to_string(&path)
        .ok()
        .and_then(|raw| serde_json::from_str(&raw).ok())
        .unwrap_or(Value::Null);
    // A patch that sets the key to `null` removes it, exactly as it would for
    // the bundler, so the key is taken from the patch whenever it appears.
    let patch = env::var("TAURI_CONFIG")
        .ok()
        .and_then(|raw| serde_json::from_str::<Value>(&raw).ok());
    let author = patch
        .as_ref()
        .and_then(|patch| patch.pointer("/bundle/publisher"))
        .unwrap_or_else(|| base.pointer("/bundle/publisher").unwrap_or(&Value::Null));
    author.as_str().unwrap_or_default().trim().to_owned()
}
