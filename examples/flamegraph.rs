//! Profiles the stress test on one thread and writes a flamegraph.
//!
//! pprof samples the program from the inside, so unlike dtrace or perf it
//! needs no root rights:
//!
//! `CARGO_PROFILE_RELEASE_DEBUG=true cargo run --release --example flamegraph`

use std::error::Error;
use std::fs::File;

use crashlend::stress_test::{self, StressConfig};
use crashlend::types::Params;

/// Where the flamegraph is written.
const OUTPUT: &str = "docs/flamegraph-stress-test.svg";

fn main() -> Result<(), Box<dyn Error>> {
    let config = StressConfig {
        scenarios: 3_000,
        threads: 1,
        seed: stress_test::DEFAULT_SEED,
    };
    let profiler = pprof::ProfilerGuardBuilder::default()
        .frequency(1_000)
        .build()?;
    stress_test::run(Params::default(), &config)?;
    profiler
        .report()
        .build()?
        .flamegraph(File::create(OUTPUT)?)?;
    println!("flamegraph written to {OUTPUT}");
    Ok(())
}
