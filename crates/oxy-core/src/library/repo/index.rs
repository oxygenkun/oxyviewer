//! Statements and prepared batches for the rebuildable library index.
use crate::browse::types::{AssetSort, AssetSummary, DirectorySummary, SortDirection};
use oxy_media::types::AssetKind;
use rusqlite::{CachedStatement, Connection, OptionalExtension, params};
use std::path::{Path, PathBuf};

pub(crate) fn forget_root(transaction: &Connection, root: &str) -> Result<(), rusqlite::Error> {
    transaction.execute(
        "DELETE FROM indexed_roots WHERE root_path = ?1",
        params![root],
    )?;
    transaction.execute(
        "DELETE FROM indexed_directory_roots WHERE root_path = ?1",
        params![root],
    )?;
    transaction.execute(
        "DELETE FROM indexed_assets WHERE root_path = ?1",
        params![root],
    )?;
    transaction.execute(
        "DELETE FROM indexed_directories WHERE root_path = ?1",
        params![root],
    )?;
    transaction.execute(
        "DELETE FROM indexed_asset_search WHERE rowid IN (
           SELECT search_rowid FROM indexed_asset_search_keys WHERE root_path = ?1
         )",
        params![root],
    )?;
    transaction.execute(
        "DELETE FROM indexed_asset_search_keys WHERE root_path = ?1",
        params![root],
    )?;
    Ok(())
}

pub(crate) fn has_completed_directory_index(
    connection: &Connection,
    root: &str,
) -> Result<bool, rusqlite::Error> {
    connection
        .query_row(
            "SELECT 1 FROM indexed_directory_roots WHERE root_path = ?1",
            params![root],
            |_| Ok(true),
        )
        .optional()
        .map(|value| value.unwrap_or(false))
}

fn kind_name(kind: AssetKind) -> &'static str {
    match kind {
        AssetKind::Raw => "raw",
        AssetKind::Jpeg => "jpeg",
        AssetKind::Heif => "heif",
        AssetKind::Png => "png",
        AssetKind::Tiff => "tiff",
        AssetKind::Webp => "webp",
    }
}

fn parse_kind(value: &str) -> Result<AssetKind, rusqlite::Error> {
    match value {
        "raw" => Ok(AssetKind::Raw),
        "jpeg" => Ok(AssetKind::Jpeg),
        "heif" => Ok(AssetKind::Heif),
        "png" => Ok(AssetKind::Png),
        "tiff" => Ok(AssetKind::Tiff),
        "webp" => Ok(AssetKind::Webp),
        _ => Err(rusqlite::Error::InvalidQuery),
    }
}

pub(crate) fn asset_from_row(row: &rusqlite::Row<'_>) -> Result<AssetSummary, rusqlite::Error> {
    Ok(AssetSummary {
        id: row.get("id")?,
        path: PathBuf::from(row.get::<_, String>("path")?),
        name: row.get("name")?,
        extension: row.get("extension")?,
        kind: parse_kind(&row.get::<_, String>("kind")?)?,
        size_bytes: row.get("size_bytes")?,
        modified_at_ms: row.get("modified_at_ms")?,
        has_sidecar: row.get("has_sidecar")?,
        rating: None,
        color_label: None,
        pick_label: None,
    })
}

pub(crate) fn directory_assets(
    connection: &Connection,
    root: &std::path::Path,
    directory: &std::path::Path,
) -> Result<Vec<crate::browse::types::AssetSummary>, rusqlite::Error> {
    let mut statement = connection.prepare("SELECT id,path,name,extension,kind,size_bytes,modified_at_ms,has_sidecar FROM indexed_assets WHERE root_path=?1 AND parent_path=?2")?;
    statement
        .query_map(
            params![root.to_string_lossy(), directory.to_string_lossy()],
            asset_from_row,
        )?
        .collect::<Result<_, _>>()
}

pub(crate) fn has_completed_index(
    connection: &Connection,
    root: &str,
) -> Result<bool, rusqlite::Error> {
    connection
        .query_row(
            "SELECT 1 FROM indexed_roots WHERE root_path = ?1",
            params![root],
            |_| Ok(true),
        )
        .optional()
        .map(|value| value.unwrap_or(false))
}

pub(crate) struct AssetPageRequest<'a> {
    pub root: &'a str,
    pub directory: &'a str,
    pub search: Option<&'a str>,
    pub kind: Option<AssetKind>,
    pub direction: SortDirection,
    pub sort: AssetSort,
    pub limit: usize,
    pub offset: usize,
}
pub(crate) fn asset_page(
    connection: &Connection,
    request: AssetPageRequest<'_>,
) -> Result<(usize, Vec<AssetSummary>), rusqlite::Error> {
    let kind = request.kind.map(kind_name);
    let direction = if matches!(request.direction, SortDirection::Descending) {
        "DESC"
    } else {
        "ASC"
    };
    let order = match request.sort {
        AssetSort::Name => "a.name COLLATE NOCASE",
        AssetSort::Modified => "a.modified_at_ms",
        AssetSort::Size => "a.size_bytes",
        AssetSort::Kind => "a.kind",
    };
    let search = request.search;
    let (total, sql, search_expression) = if let Some(search) = search {
        let expression = fts_expression(search);
        let total = connection.query_row(
            "SELECT COUNT(*) AS total
                 FROM indexed_assets a
                 JOIN indexed_asset_search s ON s.path = a.path AND s.root_path = a.root_path
                 WHERE s.root_path = ?1
                   AND indexed_asset_search MATCH ?2
                   AND (?3 IS NULL OR a.kind = ?3)",
            params![request.root, expression, kind],
            |row| row.get::<_, usize>("total"),
        )?;
        (
            total,
            format!(
                "SELECT a.id, a.path, a.name, a.extension, a.kind,
                            a.size_bytes, a.modified_at_ms, a.has_sidecar
                     FROM indexed_assets a
                     JOIN indexed_asset_search s ON s.path = a.path AND s.root_path = a.root_path
                     WHERE s.root_path = ?1
                       AND indexed_asset_search MATCH ?2
                       AND (?3 IS NULL OR a.kind = ?3)
                     ORDER BY {order} {direction}, a.name {direction}
                     LIMIT ?4 OFFSET ?5"
            ),
            Some(expression),
        )
    } else {
        let directory = request.directory;
        let total = connection.query_row(
            "SELECT COUNT(*) AS total FROM indexed_assets a
                 WHERE a.root_path = ?1 AND a.parent_path = ?2
                   AND (?3 IS NULL OR a.kind = ?3)",
            params![request.root, directory, kind],
            |row| row.get::<_, usize>("total"),
        )?;
        (
            total,
            format!(
                "SELECT a.id, a.path, a.name, a.extension, a.kind,
                            a.size_bytes, a.modified_at_ms, a.has_sidecar
                     FROM indexed_assets a
                     WHERE a.root_path = ?1 AND a.parent_path = ?2
                       AND (?3 IS NULL OR a.kind = ?3)
                     ORDER BY {order} {direction}, a.name {direction}
                     LIMIT ?4 OFFSET ?5"
            ),
            None,
        )
    };
    let mut statement = connection.prepare(&sql)?;
    let items = if let Some(expression) = search_expression {
        statement
            .query_map(
                params![
                    request.root,
                    expression,
                    kind,
                    request.limit,
                    request.offset
                ],
                asset_from_row,
            )?
            .collect::<Result<Vec<_>, _>>()?
    } else {
        statement
            .query_map(
                params![
                    request.root,
                    request.directory,
                    kind,
                    request.limit,
                    request.offset
                ],
                asset_from_row,
            )?
            .collect::<Result<Vec<_>, _>>()?
    };
    Ok((total, items))
}

pub(crate) fn list_directories(
    connection: &Connection,
    root: &str,
    parent: &Path,
) -> Result<Vec<DirectorySummary>, rusqlite::Error> {
    let mut statement = connection.prepare(
        "SELECT path, name, has_children FROM indexed_directories
             WHERE root_path = ?1 AND parent_path = ?2
             ORDER BY name COLLATE NOCASE, name",
    )?;
    let directories = statement
        .query_map(params![root, parent.to_string_lossy()], |row| {
            Ok(DirectorySummary {
                path: PathBuf::from(row.get::<_, String>("path")?),
                name: row.get("name")?,
                has_children: row.get("has_children")?,
            })
        })?
        .collect::<Result<Vec<_>, _>>()?;
    Ok(directories)
}

pub(crate) fn search_directories(
    connection: &Connection,
    root_text: &str,
    search: &str,
) -> Result<Vec<DirectorySummary>, rusqlite::Error> {
    let mut statement = connection.prepare(
        "SELECT path, name, has_children FROM indexed_directories
             WHERE root_path = ?1 AND name LIKE '%' || ?2 || '%' COLLATE NOCASE
             ORDER BY
               CASE
                 WHEN name = ?2 COLLATE NOCASE THEN 0
                 WHEN name LIKE ?2 || '%' COLLATE NOCASE THEN 1
                 ELSE 2
               END,
               name COLLATE NOCASE,
               path DESC",
    )?;
    let directories = statement
        .query_map(params![root_text, search], |row| {
            Ok(DirectorySummary {
                path: PathBuf::from(row.get::<_, String>("path")?),
                name: row.get("name")?,
                has_children: row.get("has_children")?,
            })
        })?
        .collect::<Result<Vec<_>, _>>()?;
    Ok(directories)
}

pub(crate) fn complete_asset_index(
    connection: &Connection,
    root: &Path,
    asset_count: usize,
    directory_count: usize,
) -> Result<(), rusqlite::Error> {
    connection.execute(
        "INSERT INTO indexed_roots(root_path, indexed_at, asset_count, directory_count)
             VALUES (?1, unixepoch(), ?2, ?3)
             ON CONFLICT(root_path) DO UPDATE SET
               indexed_at=excluded.indexed_at,
               asset_count=excluded.asset_count,
               directory_count=excluded.directory_count",
        params![root.to_string_lossy(), asset_count, directory_count],
    )?;
    Ok(())
}

pub(crate) fn forget_old_assets(
    connection: &Connection,
    root: &Path,
    scan_id: i64,
) -> Result<(), rusqlite::Error> {
    connection.execute(
        "DELETE FROM indexed_assets WHERE root_path = ?1 AND scan_id != ?2",
        params![root.to_string_lossy(), scan_id],
    )?;
    Ok(())
}

pub(crate) fn forget_old_search_keys(
    connection: &Connection,
    root: &Path,
    scan_id: i64,
) -> Result<(), rusqlite::Error> {
    connection.execute(
        "DELETE FROM indexed_asset_search_keys WHERE root_path = ?1 AND path IN (
               SELECT path FROM indexed_assets WHERE root_path = ?1 AND scan_id != ?2
             )",
        params![root.to_string_lossy(), scan_id],
    )?;
    Ok(())
}

pub(crate) fn forget_old_search_rows(
    connection: &Connection,
    root: &Path,
    scan_id: i64,
) -> Result<(), rusqlite::Error> {
    connection.execute(
        "DELETE FROM indexed_asset_search WHERE rowid IN (
               SELECT k.search_rowid FROM indexed_asset_search_keys k
               JOIN indexed_assets a ON a.root_path = k.root_path AND a.path = k.path
               WHERE a.root_path = ?1 AND a.scan_id != ?2
             )",
        params![root.to_string_lossy(), scan_id],
    )?;
    Ok(())
}

pub(crate) fn complete_directory_index(
    connection: &Connection,
    root: &Path,
    directory_count: usize,
) -> Result<(), rusqlite::Error> {
    connection.execute(
        "INSERT INTO indexed_directory_roots(root_path, indexed_at, directory_count)
             VALUES (?1, unixepoch(), ?2)
             ON CONFLICT(root_path) DO UPDATE SET
               indexed_at=excluded.indexed_at,
               directory_count=excluded.directory_count",
        params![root.to_string_lossy(), directory_count],
    )?;
    Ok(())
}

pub(crate) fn forget_old_directories(
    connection: &Connection,
    root: &Path,
    scan_id: i64,
) -> Result<(), rusqlite::Error> {
    connection.execute(
        "DELETE FROM indexed_directories WHERE root_path = ?1 AND scan_id != ?2",
        params![root.to_string_lossy(), scan_id],
    )?;
    Ok(())
}

pub(crate) fn advance_scan_id(connection: &Connection) -> Result<(), rusqlite::Error> {
    connection.execute(
        "UPDATE library_index_sequence SET next_scan_id = next_scan_id + 1 WHERE id = 1",
        [],
    )?;
    Ok(())
}

pub(crate) fn update_directory_status(
    connection: &Connection,
    root: &Path,
    parent: &Path,
    has_children: bool,
    scan_id: i64,
) -> Result<(), rusqlite::Error> {
    connection.execute(
        "UPDATE indexed_directories
                 SET has_children = ?3, scan_id = ?4
                 WHERE root_path = ?1 AND path = ?2",
        params![
            root.to_string_lossy(),
            parent.to_string_lossy(),
            has_children,
            scan_id,
        ],
    )?;
    Ok(())
}

pub(crate) fn forget_directory_completion(
    connection: &Connection,
    root: &Path,
) -> Result<(), rusqlite::Error> {
    connection.execute(
        "DELETE FROM indexed_directory_roots WHERE root_path = ?1",
        params![root.to_string_lossy()],
    )?;
    Ok(())
}

pub(crate) fn forget_asset_completion(
    connection: &Connection,
    root: &Path,
) -> Result<(), rusqlite::Error> {
    connection.execute(
        "DELETE FROM indexed_roots WHERE root_path = ?1",
        params![root.to_string_lossy()],
    )?;
    Ok(())
}

pub(crate) fn read_scan_id(connection: &Connection) -> Result<i64, rusqlite::Error> {
    let scan_id = connection.query_row(
        "SELECT next_scan_id FROM library_index_sequence WHERE id = 1",
        [],
        |row| row.get("next_scan_id"),
    )?;
    Ok(scan_id)
}

pub(crate) fn directory_insert(
    connection: &Connection,
) -> Result<CachedStatement<'_>, rusqlite::Error> {
    let statement = connection.prepare_cached(
        "INSERT INTO indexed_directories(
                       root_path, path, parent_path, name, has_children, scan_id
                     ) VALUES (?1, ?2, ?3, ?4, ?5, ?6)
                     ON CONFLICT(root_path, path) DO UPDATE SET
                       root_path=excluded.root_path, parent_path=excluded.parent_path,
                       name=excluded.name, has_children=excluded.has_children,
                       scan_id=excluded.scan_id",
    )?;
    Ok(statement)
}

pub(crate) fn write_directory(
    statement: &mut CachedStatement<'_>,
    root: &Path,
    parent: &Path,
    directory: &DirectorySummary,
    scan_id: i64,
) -> Result<(), rusqlite::Error> {
    statement.execute(params![
        root.to_string_lossy(),
        directory.path.to_string_lossy(),
        parent.to_string_lossy(),
        directory.name,
        directory.has_children,
        scan_id,
    ])?;
    Ok(())
}

pub(crate) struct AssetStatements<'a> {
    mark_unchanged: CachedStatement<'a>,
    upsert: CachedStatement<'a>,
    insert_search_key: CachedStatement<'a>,
    search_key: CachedStatement<'a>,
    insert_search: CachedStatement<'a>,
}
impl<'a> AssetStatements<'a> {
    pub(crate) fn new(connection: &'a Connection) -> Result<Self, rusqlite::Error> {
        let mark_unchanged = connection.prepare_cached(
            "UPDATE indexed_assets SET scan_id = ?11
                     WHERE root_path = ?1 AND path = ?2 AND parent_path = ?3
                       AND id = ?4 AND name = ?5 AND extension = ?6 AND kind = ?7
                       AND modified_at_ms = ?8 AND size_bytes = ?9
                       AND has_sidecar = ?10",
        )?;
        let upsert = connection.prepare_cached(
            "INSERT INTO indexed_assets(
                       root_path, path, parent_path, id, name, extension, kind,
                       modified_at_ms, size_bytes, has_sidecar, scan_id
                     ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)
                     ON CONFLICT(root_path, path) DO UPDATE SET
                       root_path=excluded.root_path, parent_path=excluded.parent_path,
                       id=excluded.id, name=excluded.name, extension=excluded.extension,
                       kind=excluded.kind, modified_at_ms=excluded.modified_at_ms,
                       size_bytes=excluded.size_bytes, has_sidecar=excluded.has_sidecar,
                       scan_id=excluded.scan_id",
        )?;
        let insert_search_key = connection.prepare_cached(
            "INSERT INTO indexed_asset_search_keys(root_path, path) VALUES (?1, ?2)
                     ON CONFLICT(root_path, path) DO NOTHING",
        )?;
        let search_key = connection.prepare_cached(
            "SELECT search_rowid FROM indexed_asset_search_keys
                     WHERE root_path = ?1 AND path = ?2",
        )?;
        let insert_search = connection.prepare_cached(
            "INSERT OR REPLACE INTO indexed_asset_search(rowid, path, root_path, name, directory)
                     VALUES (?1, ?2, ?3, ?4, ?5)",
        )?;
        Ok(Self {
            mark_unchanged,
            upsert,
            insert_search_key,
            search_key,
            insert_search,
        })
    }
    pub(crate) fn mark_unchanged(
        &mut self,
        root: &Path,
        parent: &Path,
        scan_id: i64,
        asset: &AssetSummary,
    ) -> Result<bool, rusqlite::Error> {
        let values = params![
            root.to_string_lossy(),
            asset.path.to_string_lossy(),
            parent.to_string_lossy(),
            asset.id,
            asset.name,
            asset.extension,
            kind_name(asset.kind),
            asset.modified_at_ms,
            asset.size_bytes,
            asset.has_sidecar,
            scan_id,
        ];
        Ok(self.mark_unchanged.execute(values)? > 0)
    }
    pub(crate) fn upsert(
        &mut self,
        root: &Path,
        parent: &Path,
        scan_id: i64,
        asset: &AssetSummary,
    ) -> Result<(), rusqlite::Error> {
        self.upsert.execute(params![
            root.to_string_lossy(),
            asset.path.to_string_lossy(),
            parent.to_string_lossy(),
            asset.id,
            asset.name,
            asset.extension,
            kind_name(asset.kind),
            asset.modified_at_ms,
            asset.size_bytes,
            asset.has_sidecar,
            scan_id,
        ])?;
        Ok(())
    }
    pub(crate) fn insert_search_key(
        &mut self,
        root: &Path,
        asset: &AssetSummary,
    ) -> Result<(), rusqlite::Error> {
        self.insert_search_key.execute(params![
            root.to_string_lossy(),
            asset.path.to_string_lossy()
        ])?;
        Ok(())
    }
    pub(crate) fn search_rowid(
        &mut self,
        root: &Path,
        asset: &AssetSummary,
    ) -> Result<i64, rusqlite::Error> {
        let search_rowid: i64 = self.search_key.query_row(
            params![root.to_string_lossy(), asset.path.to_string_lossy()],
            |row| row.get("search_rowid"),
        )?;
        Ok(search_rowid)
    }
    pub(crate) fn insert_search(
        &mut self,
        root: &Path,
        parent: &Path,
        asset: &AssetSummary,
        search_rowid: i64,
    ) -> Result<(), rusqlite::Error> {
        self.insert_search.execute(params![
            search_rowid,
            asset.path.to_string_lossy(),
            root.to_string_lossy(),
            asset.name,
            parent.to_string_lossy(),
        ])?;
        Ok(())
    }
}

fn fts_expression(search: &str) -> String {
    search
        .split_whitespace()
        .map(|token| format!("\"{}\"*", token.replace('"', "\"\"")))
        .collect::<Vec<_>>()
        .join(" AND ")
}
