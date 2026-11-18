//! Historical crash scenarios (section 5 of the specification).
//!
//! Each scenario is a JSON file in `data/`, with one ETH price per minute.
//! The files are downloaded once by `scripts/fetch_scenarios.sh`, so the
//! project never needs the network at run time.

use std::fs;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::error::LendingError;
use crate::types::Price;

/// Folder that holds the scenario files, inside the root of the project.
pub const DATA_DIR: &str = "data";

/// Returns the `data/` folder of the project.
///
/// The path starts from the root of the project, known when the code is
/// compiled, and not from the folder the server is launched from. The server
/// therefore finds the scenarios wherever it is started.
pub fn data_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join(DATA_DIR)
}

/// Ids of the scenarios shipped in `data/`.
///
/// Only these ids are accepted, so an id received from the API can never
/// point to another file of the disk.
pub const SCENARIO_IDS: [&str; 3] = ["black-thursday", "luna", "ftx"];

/// A crash replayed minute by minute.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Scenario {
    /// Short id used in the API, such as "black-thursday".
    pub id: String,
    /// Readable name, such as "Black Thursday".
    pub name: String,
    /// Date and time of the first price (ISO 8601, UTC).
    pub start: String,
    /// Price of 1 ETH at each minute, in USDC cents.
    pub prices: Vec<Price>,
}

/// Loads the scenario `id` from the folder `dir`.
///
/// Returns `ScenarioNotFound` if the id is unknown, or `Storage` if the file
/// cannot be read or is not a valid scenario.
pub fn load(dir: &Path, id: &str) -> Result<Scenario, LendingError> {
    if !SCENARIO_IDS.contains(&id) {
        return Err(LendingError::ScenarioNotFound(id.to_string()));
    }
    let path = dir.join(format!("{id}.json"));
    let json =
        fs::read_to_string(&path).map_err(|error| LendingError::Storage(error.to_string()))?;
    let scenario: Scenario =
        serde_json::from_str(&json).map_err(|error| LendingError::Storage(error.to_string()))?;
    check(&scenario)?;
    Ok(scenario)
}

/// Loads every scenario of `SCENARIO_IDS` from the folder `dir`.
pub fn load_all(dir: &Path) -> Result<Vec<Scenario>, LendingError> {
    SCENARIO_IDS.iter().map(|id| load(dir, id)).collect()
}

/// Refuses a scenario that the replay could not use.
fn check(scenario: &Scenario) -> Result<(), LendingError> {
    if scenario.prices.is_empty() {
        return Err(LendingError::Storage(format!(
            "scenario {} has no price",
            scenario.id
        )));
    }
    // A zero price would make ETH free and every position worthless.
    if scenario.prices.contains(&Price(0)) {
        return Err(LendingError::Storage(format!(
            "scenario {} has a zero price",
            scenario.id
        )));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// An empty temporary folder, to write broken scenario files in.
    fn temp_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("crashlend-scenario-{name}"));
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn black_thursday_has_one_price_per_minute_of_the_day() {
        let scenario = load(&data_dir(), "black-thursday").unwrap();
        assert_eq!(scenario.id, "black-thursday");
        assert_eq!(scenario.start, "2020-03-12T00:00:00Z");
        assert_eq!(scenario.prices.len(), 24 * 60);
    }

    #[test]
    fn black_thursday_is_a_crash() {
        let scenario = load(&data_dir(), "black-thursday").unwrap();
        let first = scenario.prices[0];
        let lowest = scenario.prices.iter().min().unwrap();
        // ETH lost more than 40 % during the day.
        assert!(lowest.0 * 10 < first.0 * 6);
    }

    #[test]
    fn data_dir_does_not_depend_on_the_launch_folder() {
        // An absolute path points to the same folder wherever the server starts.
        assert!(data_dir().is_absolute());
        assert!(data_dir().join("black-thursday.json").is_file());
    }

    #[test]
    fn every_scenario_can_be_loaded() {
        let scenarios = load_all(&data_dir()).unwrap();
        let ids: Vec<&str> = scenarios.iter().map(|s| s.id.as_str()).collect();
        assert_eq!(ids, SCENARIO_IDS);
    }

    #[test]
    fn unknown_id_is_refused() {
        let result = load(&data_dir(), "covid");
        assert_eq!(result, Err(LendingError::ScenarioNotFound("covid".into())));
    }

    #[test]
    fn an_id_cannot_point_outside_the_data_folder() {
        let result = load(&data_dir(), "../Cargo");
        assert_eq!(
            result,
            Err(LendingError::ScenarioNotFound("../Cargo".into()))
        );
    }

    #[test]
    fn missing_file_is_a_storage_error() {
        let result = load(&temp_dir("missing"), "luna");
        assert!(matches!(result, Err(LendingError::Storage(_))));
    }

    #[test]
    fn invalid_json_is_a_storage_error() {
        let dir = temp_dir("invalid");
        fs::write(dir.join("ftx.json"), "not json").unwrap();
        let result = load(&dir, "ftx");
        fs::remove_dir_all(&dir).unwrap();
        assert!(matches!(result, Err(LendingError::Storage(_))));
    }

    #[test]
    fn scenario_without_prices_is_refused() {
        let dir = temp_dir("empty");
        let json = r#"{"id":"ftx","name":"FTX","start":"2022-11-07T00:00:00Z","prices":[]}"#;
        fs::write(dir.join("ftx.json"), json).unwrap();
        let result = load(&dir, "ftx");
        fs::remove_dir_all(&dir).unwrap();
        assert!(matches!(result, Err(LendingError::Storage(_))));
    }

    #[test]
    fn scenario_with_a_zero_price_is_refused() {
        let dir = temp_dir("zero");
        let json =
            r#"{"id":"ftx","name":"FTX","start":"2022-11-07T00:00:00Z","prices":[150000,0]}"#;
        fs::write(dir.join("ftx.json"), json).unwrap();
        let result = load(&dir, "ftx");
        fs::remove_dir_all(&dir).unwrap();
        assert!(matches!(result, Err(LendingError::Storage(_))));
    }
}
