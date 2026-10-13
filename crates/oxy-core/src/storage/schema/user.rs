//! View of registered user declarations; no compatibility or domain policy.
use crate::storage::table;
pub static DEFS: std::sync::LazyLock<Vec<table::TableDef>> = std::sync::LazyLock::new(|| {
    super::definitions()
        .iter()
        .copied()
        .filter(|def| def.class == table::DataClass::UserOwned)
        .collect()
});
