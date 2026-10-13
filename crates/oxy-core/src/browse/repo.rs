//! Directory snapshot statements. Scan and invalidation policy stay with browse.
use rusqlite::{Connection, OptionalExtension, params};
use std::{
    collections::HashSet,
    path::{Path, PathBuf},
};

pub(crate) fn store_snapshot(
    connection: &Connection,
    root: &Path,
    directory: &Path,
    json: &str,
) -> Result<usize, rusqlite::Error> {
    connection.execute(
        "INSERT INTO directory_snapshots(root_path,directory_path,assets_json) VALUES (?1,?2,?3)
        ON CONFLICT(root_path,directory_path) DO UPDATE SET assets_json=excluded.assets_json",
        params![root.to_string_lossy(), directory.to_string_lossy(), json],
    )
}

pub(crate) fn load_snapshot(
    connection: &Connection,
    root: &Path,
    directory: &Path,
) -> Result<Option<String>, rusqlite::Error> {
    connection
        .query_row(
            "SELECT assets_json FROM directory_snapshots WHERE root_path=?1 AND directory_path=?2",
            params![root.to_string_lossy(), directory.to_string_lossy()],
            |row| row.get("assets_json"),
        )
        .optional()
}

pub(crate) fn has_snapshot(
    connection: &Connection,
    root: &Path,
    directory: &Path,
) -> Result<bool, rusqlite::Error> {
    connection.query_row("SELECT EXISTS(SELECT 1 FROM directory_snapshots WHERE root_path=?1 AND directory_path=?2 AND assets_json!='') AS present",
        params![root.to_string_lossy(), directory.to_string_lossy()], |row| row.get("present"))
}

pub(crate) fn forget_root(connection: &Connection, root: &Path) -> Result<(), rusqlite::Error> {
    connection.execute(
        "DELETE FROM directory_snapshots WHERE root_path=?1",
        [root.to_string_lossy()],
    )?;
    Ok(())
}

pub(crate) fn tombstones(
    connection: &Connection,
    root: &Path,
) -> Result<Vec<PathBuf>, rusqlite::Error> {
    connection
        .prepare(
            "SELECT directory_path FROM directory_snapshots WHERE root_path=?1 AND assets_json=''",
        )?
        .query_map([root.to_string_lossy()], |row| {
            row.get::<_, String>("directory_path").map(PathBuf::from)
        })?
        .collect()
}

pub(crate) fn paths(
    connection: &Connection,
) -> Result<HashSet<(PathBuf, PathBuf)>, rusqlite::Error> {
    connection
        .prepare("SELECT root_path,directory_path FROM directory_snapshots")?
        .query_map([], |row| {
            Ok((
                PathBuf::from(row.get::<_, String>("root_path")?),
                PathBuf::from(row.get::<_, String>("directory_path")?),
            ))
        })?
        .collect()
}
