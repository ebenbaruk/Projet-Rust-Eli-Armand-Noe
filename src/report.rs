//! Reports built from the outcomes of the stress test.

use std::time::Duration;

use serde::Serialize;

use crate::types::UsdcAmount;

/// Outcomes of many runs made with the same rules.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Tally {
    /// Number of runs.
    pub runs: u64,
    /// Runs that ended without bad debt.
    pub survived: u64,
    /// Sum of the bad debt left at the end of every run.
    pub bad_debt: UsdcAmount,
}

impl Tally {
    /// Adds the bad debt left at the end of one run.
    pub fn record(&mut self, bad_debt: UsdcAmount) {
        self.runs += 1;
        if bad_debt.0 == 0 {
            self.survived += 1;
        }
        self.bad_debt.0 = self.bad_debt.0.saturating_add(bad_debt.0);
    }

    /// Combines the tallies of two groups of runs.
    pub fn merge(self, other: Tally) -> Tally {
        Tally {
            runs: self.runs + other.runs,
            survived: self.survived + other.survived,
            bad_debt: UsdcAmount(self.bad_debt.0.saturating_add(other.bad_debt.0)),
        }
    }

    /// Share of the runs that ended without bad debt, between 0 and 1.
    pub fn survival_rate(&self) -> f64 {
        if self.runs == 0 {
            return 0.0;
        }
        self.survived as f64 / self.runs as f64
    }

    /// Bad debt per run, survived runs included.
    pub fn average_bad_debt(&self) -> UsdcAmount {
        UsdcAmount(self.bad_debt.0.checked_div(self.runs).unwrap_or(0))
    }
}

/// Result of `POST /stress-test` (section 6.2 of the specification).
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct StressReport {
    /// Number of random crashes, each replayed with and without the guardian.
    pub scenarios: u64,
    /// Share of the crashes survived without the guardian.
    pub survival_rate_without_guardian: f64,
    /// Share of the crashes survived with the guardian.
    pub survival_rate_with_guardian: f64,
    /// Bad debt per crash without the guardian.
    pub average_bad_debt_without_guardian: UsdcAmount,
    /// Bad debt per crash with the guardian.
    pub average_bad_debt_with_guardian: UsdcAmount,
    /// Number of threads the crashes were spread over.
    pub threads: usize,
    /// Wall-clock time of the whole stress test.
    pub duration_ms: u64,
}

impl StressReport {
    /// Builds the report from the tallies of both sets of runs.
    pub fn new(without: Tally, with: Tally, threads: usize, duration: Duration) -> StressReport {
        StressReport {
            scenarios: without.runs,
            survival_rate_without_guardian: without.survival_rate(),
            survival_rate_with_guardian: with.survival_rate(),
            average_bad_debt_without_guardian: without.average_bad_debt(),
            average_bad_debt_with_guardian: with.average_bad_debt(),
            threads,
            duration_ms: u64::try_from(duration.as_millis()).unwrap_or(u64::MAX),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tally_counts_survivals_and_bad_debt() {
        let mut tally = Tally::default();
        tally.record(UsdcAmount(0));
        tally.record(UsdcAmount(300));
        tally.record(UsdcAmount(0));
        tally.record(UsdcAmount(100));
        assert_eq!(tally.survival_rate(), 0.5);
        assert_eq!(tally.average_bad_debt(), UsdcAmount(100));
    }

    #[test]
    fn empty_tally_has_no_rate() {
        let tally = Tally::default();
        assert_eq!(tally.survival_rate(), 0.0);
        assert_eq!(tally.average_bad_debt(), UsdcAmount(0));
    }

    #[test]
    fn merged_tallies_add_up() {
        let mut first = Tally::default();
        first.record(UsdcAmount(0));
        let mut second = Tally::default();
        second.record(UsdcAmount(50));
        let total = first.merge(second);
        assert_eq!(total.runs, 2);
        assert_eq!(total.survived, 1);
        assert_eq!(total.bad_debt, UsdcAmount(50));
    }
}
