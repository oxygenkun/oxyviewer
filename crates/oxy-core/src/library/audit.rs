//! Source audit of library root policy and its rebuildable index.
//! Schema audits preserve all user facts during cache clearing. These checks
//! additionally prevent index statements from writing user-owned tables and
//! require root policy to delegate its statements to the library repository.
//! Feature modules share one crate, so these source checks remain necessary.

use crate::storage::schema::{rebuildable_tables, tables, user_owned_tables};
use std::path::Path;

#[test]
fn index_policy_delegates_statements_to_its_repository() {
    let source = include_str!("index.rs")
        .split("#[cfg(test)]")
        .next()
        .unwrap();
    for verb in ["SELECT ", "INSERT INTO ", "UPDATE ", "DELETE FROM "] {
        assert!(
            !source.contains(verb),
            "index SQL belongs in library::repo::index: {verb}"
        );
    }
}

#[test]
fn no_module_writes_tables_owned_by_the_other_namespace() {
    let source_root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("src")
        .join("library");
    let offenders = namespace_offenders(&source_root.join("user"), user_owned_tables())
        .into_iter()
        .chain(namespace_offenders(
            &source_root.join("repo/index.rs"),
            rebuildable_tables(),
        ))
        .collect::<Vec<_>>();
    assert!(
        offenders.is_empty(),
        "modules must not write tables owned by the other namespace:\n{}",
        offenders.join("\n")
    );
}

/// Every statement against a user-owned table now has a home in
/// `the owning feature repository`.
///
/// The audit above catches a write to the *other* namespace. This one catches a
/// statement that should not be in the policy module: naming a repository
/// function keeps the SQL of one domain in exactly one place to read and
/// change. Tests still set rows up directly, which is their business, so the
/// check stops at the test module.
#[test]
fn the_user_namespace_carries_no_sql_of_its_own() {
    let directory = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("src")
        .join("library")
        .join("user");
    let mut offenders = Vec::new();
    for entry in std::fs::read_dir(&directory).expect("namespace directory must be readable") {
        let path = entry.expect("directory entry").path();
        if path.extension().and_then(|ext| ext.to_str()) != Some("rs") {
            continue;
        }
        let source = std::fs::read_to_string(&path).expect("source must be readable");
        let name = path.file_name().unwrap().to_string_lossy().into_owned();
        let production = source.split("#[cfg(test)]").next().unwrap_or(&source);
        for (index, line) in production.lines().enumerate() {
            if looks_like_sql(line.trim()) {
                offenders.push(format!("{name}:{}", index + 1));
            }
        }
    }
    assert!(
        offenders.is_empty(),
        "a user-domain statement belongs in the owning feature repository, not here:\n{}",
        offenders.join("\n")
    );
}

/// Collects `file:line: table` for every SQL literal in `directory` that
/// writes to a table it does not own.
fn namespace_offenders(directory: &Path, owned: impl Iterator<Item = &'static str>) -> Vec<String> {
    let owned: Vec<&str> = owned.collect();
    let foreign: Vec<&str> = tables()
        .filter(|(name, _)| !owned.contains(name))
        .map(|(name, _)| name)
        .collect();

    let mut offenders = Vec::new();
    let paths = if directory.is_file() {
        vec![directory.to_owned()]
    } else {
        std::fs::read_dir(directory)
            .expect("namespace must be readable")
            .map(|entry| entry.expect("directory entry").path())
            .collect()
    };
    for path in paths {
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
