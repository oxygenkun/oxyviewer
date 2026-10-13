//! Typed cache clearing derived from the feature declarations.
use crate::storage::table;
use rusqlite::Connection;
pub static DEFS: std::sync::LazyLock<Vec<table::TableDef>> = std::sync::LazyLock::new(|| {
    super::definitions()
        .iter()
        .copied()
        .filter(|def| def.class != table::DataClass::UserOwned)
        .collect()
});
pub(super) fn clear(connection: &mut Connection) -> Result<(), rusqlite::Error> {
    let transaction = connection.transaction()?;
    table::clear_all(&transaction, &DEFS)?;
    transaction.commit()
}
