//! The sqlite-vec extension: registering it and proving every connection has it.
//!
//! Registration is process-global and must happen before the first connection
//! is opened, so [`register`] is the first thing [`crate::Store`] does. The
//! probe exists because a mismatch between the connection that writes a vector
//! and the one that reads it would surface much later, as a wrong distance
//! rather than as an error.

use parking_lot::Mutex;
use rusqlite::Connection;
use std::sync::OnceLock;

static EXTENSION_REGISTRATION: OnceLock<Result<(), String>> = OnceLock::new();

/// Registers the extension once per process.
pub(super) fn register() -> Result<(), String> {
    EXTENSION_REGISTRATION
        .get_or_init(|| {
            // The sqlite-vec crate exposes its C init symbol as an opaque function.
            // SQLite calls it with the standard extension-init ABI after registration.
            let init = unsafe {
                std::mem::transmute::<
                    *const (),
                    unsafe extern "C" fn(
                        *mut rusqlite::ffi::sqlite3,
                        *mut *mut std::ffi::c_char,
                        *const rusqlite::ffi::sqlite3_api_routines,
                    ) -> std::ffi::c_int,
                >(sqlite_vec::sqlite3_vec_init as *const ())
            };
            let code = unsafe { rusqlite::ffi::sqlite3_auto_extension(Some(init)) };
            if code == rusqlite::ffi::SQLITE_OK {
                Ok(())
            } else {
                Err(format!("SQLite extension registration failed: {code}"))
            }
        })
        .clone()
}

/// Returns the extension version, after proving every connection agrees on it.
pub(super) fn probe(
    writer: &Connection,
    reader: Option<&Mutex<Connection>>,
    projection_reader: Option<&Mutex<Connection>>,
) -> Result<String, String> {
    let version = |connection: &Connection| {
        connection
            .query_row("SELECT vec_version() AS version", [], |row| {
                row.get::<_, String>("version")
            })
            .map_err(|error| error.to_string())
    };
    let expected = version(writer)?;
    for connection in [reader, projection_reader].into_iter().flatten() {
        let actual = version(&connection.lock())?;
        if actual != expected {
            return Err(format!(
                "sqlite-vec version mismatch: {expected} vs {actual}"
            ));
        }
    }
    Ok(expected)
}
