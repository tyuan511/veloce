//! Connection secrets in the operating system's keychain.
//!
//! The database only ever sees non-secret settings. Each connection has at most one
//! secret, filed under its database id so renaming a connection never orphans it.

use keyring::Entry;

const SERVICE: &str = "dev.veloce.connections";

fn account(connection_id: i64) -> String {
    format!("connection:{connection_id}")
}

fn entry(connection_id: i64) -> Result<Entry, String> {
    Entry::new(SERVICE, &account(connection_id)).map_err(|error| error.to_string())
}

pub fn save(connection_id: i64, secret: &str) -> Result<(), String> {
    entry(connection_id)?
        .set_password(secret)
        .map_err(|error| error.to_string())
}

/// `Ok(None)` when nothing was ever saved, which is normal for optional passwords.
pub fn load(connection_id: i64) -> Result<Option<String>, String> {
    match entry(connection_id)?.get_password() {
        Ok(secret) => Ok(Some(secret)),
        Err(keyring::Error::NoEntry) => Ok(None),
        Err(error) => Err(error.to_string()),
    }
}

/// Forget the secret; already gone counts as success.
pub fn delete(connection_id: i64) {
    if let Ok(entry) = entry(connection_id) {
        let _ = entry.delete_credential();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accounts_are_keyed_by_database_id() {
        assert_eq!(account(7), "connection:7");
        assert_ne!(account(1), account(11));
    }
}
