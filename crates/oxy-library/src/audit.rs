//! The source-level audit of the cache/user namespace boundary.
//!
//! The *data*-level audit — every table has a class, and a cache clear leaves
//! every user-owned table with its rows — lives in `oxy_store::schema`, next to
//! the declarations it checks. What cannot live there is this one: a check that
//! the modules of one namespace do not write the other namespace's tables.
//! That is a property of this crate's layout, so it is asserted here.
//!
//! A module may read across the boundary — the index looks up which roots
//! exist — but never write: every cross-namespace write goes through a named
//! function on the owning module, so it shows up in review as a call instead of
//! hiding inside a SQL string.
//!
//! Once the domains become separate crates this test is expected to disappear:
//! a crate that cannot name another domain's tables cannot write them either,
//! and the guarantee moves from a grep to the compiler.

use oxy_store::schema::{rebuildable_tables, tables, user_owned_tables};
use std::path::Path;

#[test]
fn no_module_writes_tables_owned_by_the_other_namespace() {
    let source_root = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let offenders = namespace_offenders(&source_root.join("user"), user_owned_tables())
        .into_iter()
        .chain(namespace_offenders(
            &source_root.join("cache"),
            rebuildable_tables(),
        ))
        .collect::<Vec<_>>();
    assert!(
        offenders.is_empty(),
        "modules must not write tables owned by the other namespace:\n{}",
        offenders.join("\n")
    );
}

/// Collects `file:line: table` for every SQL literal in `directory` that
/// writes to a table it does not own.
fn namespace_offenders(
    directory: &Path,
    owned: impl Iterator<Item = &'static str>,
) -> Vec<String> {
    let owned: Vec<&str> = owned.collect();
    let foreign: Vec<&str> = tables()
        .filter(|(name, _)| !owned.contains(name))
        .map(|(name, _)| name)
        .collect();

    let mut offenders = Vec::new();
    for entry in std::fs::read_dir(directory).expect("namespace directory must be readable") {
        let path = entry.expect("directory entry").path();
        if path.extension().and_then(|ext| ext.to_str()) != Some("rs") {
            continue;
        }
        let source = std::fs::read_to_string(&path).expect("source must be readable");
        let name = path.file_name().unwrap().to_string_lossy().into_owned();
        for (index, line) in source.lines().enumerate() {
            let sql = line.trim();
            if !looks_like_sql(sql) {
                continue;
            }
            for table in &foreign {
                if writes_table(sql, table) {
                    offenders.push(format!("{name}:{}: {table}", index + 1));
                }
            }
        }
    }
    offenders
}

/// True for lines that carry SQL, whether the statement is inline in an
/// `execute(...)` call or a standalone string literal. Uppercase verbs only
/// appear in SQL, so Rust code and prose drop out here.
fn looks_like_sql(line: &str) -> bool {
    ["DELETE FROM ", "UPDATE ", "INSERT ", "REPLACE "]
        .iter()
        .any(|verb| line.contains(verb))
}

fn writes_table(sql: &str, table: &str) -> bool {
    sql.contains(&format!("DELETE FROM {table}"))
        || sql.contains(&format!("INTO {table}"))
        || sql.contains(&format!("UPDATE {table}"))
        || sql.contains(&format!("UPDATE OR REPLACE {table}"))
        || sql.contains(&format!("UPDATE OR IGNORE {table}"))
}
