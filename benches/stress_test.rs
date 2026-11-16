//! Benchmark of the stress test on 1 thread, then on every core.
//!
//! The crashes are independent, so the time should drop with the number of
//! threads until the cores run out.

use std::hint::black_box;
use std::thread;
use std::time::Duration;

use crashlend::stress_test::{self, StressConfig};
use crashlend::types::Params;
use criterion::{
    BenchmarkId, Criterion, SamplingMode, Throughput, criterion_group, criterion_main,
};

/// Crashes per run, as in the demo (section 10.2 of the specification).
const SCENARIOS: usize = 10_000;

fn stress_test_benchmark(c: &mut Criterion) {
    let cores = thread::available_parallelism().map_or(1, |cores| cores.get());
    let mut group = c.benchmark_group("stress_test");
    // One run takes seconds: flat sampling with the minimum of 10 samples,
    // and enough time for 10 runs on a single thread.
    group.sampling_mode(SamplingMode::Flat);
    group.sample_size(10);
    group.measurement_time(Duration::from_secs(180));
    group.throughput(Throughput::Elements(SCENARIOS as u64));

    for threads in [1, cores] {
        let config = StressConfig {
            scenarios: SCENARIOS,
            threads,
            seed: stress_test::DEFAULT_SEED,
        };
        group.bench_function(BenchmarkId::new("threads", threads), |b| {
            b.iter(|| stress_test::run(Params::default(), black_box(&config)))
        });
    }
    group.finish();
}

criterion_group!(benches, stress_test_benchmark);
criterion_main!(benches);
