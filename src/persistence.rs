//! Saving and loading the state of the protocol in a JSON file.
//!
//! The whole `Protocol` is written with serde_json, so a saved pool can be
//! reloaded after a restart of the server.

use std::fs;
use std::path::Path;

use crate::error::LendingError;
use crate::protocol::Protocol;

/// Writes the protocol to a JSON file, replacing any previous content.
///
/// Returns `Storage` if the state cannot be serialized or the file cannot be written.
pub fn save(protocol: &Protocol, path: &Path) -> Result<(), LendingError> {
    let json = serde_json::to_string_pretty(protocol)
        .map_err(|error| LendingError::Storage(error.to_string()))?;
    fs::write(path, json).map_err(|error| LendingError::Storage(error.to_string()))?;
    Ok(())
}

/// Reads a protocol from a JSON file.
///
/// Returns `Storage` if the file is missing or does not contain a valid state.
pub fn load(path: &Path) -> Result<Protocol, LendingError> {
    let json =
        fs::read_to_string(path).map_err(|error| LendingError::Storage(error.to_string()))?;
    let protocol =
        serde_json::from_str(&json).map_err(|error| LendingError::Storage(error.to_string()))?;
    Ok(protocol)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::{EthAmount, Params, Price, UserId};
    use std::path::PathBuf;

    fn temp_file(name: &str) -> PathBuf {
        std::env::temp_dir().join(format!("crashlend-{name}.json"))
    }

    #[test]
    fn save_then_load_gives_the_same_state() {
        let mut protocol = Protocol::new(Params::default(), Price::from_usdc(2_000));
        protocol
            .deposit(&UserId::new("bob"), EthAmount::from_eth(3))
            .unwrap();

        let path = temp_file("save-load");
        save(&protocol, &path).unwrap();
        let loaded = load(&path).unwrap();
        fs::remove_file(&path).unwrap();

        assert_eq!(loaded, protocol);
    }

    #[test]
    fn missing_file_is_a_storage_error() {
        let result = load(&temp_file("does-not-exist"));
        assert!(matches!(result, Err(LendingError::Storage(_))));
    }

    #[test]
    fn invalid_json_is_a_storage_error() {
        let path = temp_file("invalid");
        fs::write(&path, "not json").unwrap();
        let result = load(&path);
        fs::remove_file(&path).unwrap();
        assert!(matches!(result, Err(LendingError::Storage(_))));
    }
}
