//! Confirmed file actions awaiting application-state reconciliation.
crate::storage::table::tables! {
    user create file_operation_recovery =
        "id INTEGER PRIMARY KEY AUTOINCREMENT, operation_json TEXT NOT NULL,
         phase TEXT NOT NULL CHECK(phase IN ('prepared','filesApplied')),
         proofs_json TEXT, detail TEXT";
}
