//! The two claims this crate's boundary is supposed to make true.
//!
//! Neither can be checked from `oxy-store`, and neither is checkable by the
//! compiler on its own: a `Cargo.toml` entry is not a type, and a SQL string
//! inside a `&str` is not a call. So both are asserted here, from this crate's
//! own source, and they fail the build's tests rather than a review.

use std::path::Path;

/// The reason the person rules are a crate over `oxy-store` rather than a
/// module inside it.
///
/// A domain crate owns the transaction boundary, so it has to name a connection
/// and a transaction — but it must not have to name the engine to do it.
/// `oxy-store` re-exports both, and this asserts that the re-export is what is
/// actually being used: the day someone adds `rusqlite` here for a convenience,
/// the boundary has stopped being enforced.
#[test]
fn the_crate_does_not_depend_on_the_storage_engine() {
    let manifest = Path::new(env!("CARGO_MANIFEST_DIR")).join("Cargo.toml");
    let source = std::fs::read_to_string(&manifest).expect("manifest must be readable");
    let dependencies = source
        .split("[dev-dependencies]")
        .next()
        .expect("a manifest has a head");
    assert!(
        !dependencies.contains("rusqlite"),
        "oxy-people must reach the database through oxy-store, not rusqlite"
    );
}

/// The other half of the same boundary: a rule may name a repository function,
/// but the statement it ends in belongs to `oxy_store`.
///
/// This crate moved three cache modules that used to hold their SQL inline, so
/// the check matters most here: a verb creeping back is how the two layers start
/// to interleave again, and it shows up in review only if someone reads every
/// function body. Tests still stage rows directly, which is their business, so
/// the scan stops at the test module.
#[test]
fn the_crate_carries_no_sql_of_its_own() {
    let directory = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let mut offenders = Vec::new();
    let mut directories = vec![directory.clone()];
    let mut files = Vec::new();
    while let Some(directory) = directories.pop() {
        for entry in std::fs::read_dir(directory).expect("source directory must be readable") {
            let path = entry.expect("directory entry").path();
            if path.is_dir() {
                directories.push(path);
            } else {
                files.push(path);
            }
        }
    }
    for path in files {
        if path.extension().and_then(|ext| ext.to_str()) != Some("rs") {
            continue;
        }
        // This module is the checker, and it has to spell the verbs it looks
        // for. Nothing else in the crate may.
        if path.file_name().and_then(|name| name.to_str()) == Some("audit.rs") {
            continue;
        }
        let source = std::fs::read_to_string(&path).expect("source must be readable");
        let name = path.strip_prefix(&directory).unwrap().to_string_lossy();
        let production = source.split("#[cfg(test)]").next().unwrap_or(&source);
        for (index, line) in production.lines().enumerate() {
            if looks_like_sql(line.trim()) {
                offenders.push(format!("{name}:{}", index + 1));
            }
        }
    }
    assert!(
        offenders.is_empty(),
        "a person-domain statement belongs in oxy_store::repo, not here:\n{}",
        offenders.join("\n")
    );
}

/// True for lines that carry SQL, whether the statement is inline in an
/// `execute(...)` call or a standalone string literal. Uppercase verbs only
/// appear in SQL, so Rust code and prose drop out here.
fn looks_like_sql(line: &str) -> bool {
    ["DELETE FROM ", "UPDATE ", "INSERT ", "REPLACE ", "SELECT "]
        .iter()
        .any(|verb| line.contains(verb))
}
