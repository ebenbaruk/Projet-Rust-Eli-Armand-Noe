# CrashLend

Project built for the Rust course at ESILV (A4 - S7).

CrashLend is a simplified DeFi lending protocol in the style of Aave, written in Rust and run as a simulation. Lenders supply USDC, borrowers borrow that USDC by depositing ETH as collateral, and positions that become too risky can be liquidated. The project then replays real ETH crashes to measure whether the protocol would have survived them.

The full specification (in French) is in [`docs/SPECIFICATION.md`](docs/SPECIFICATION.md).

## Team

- Eli Benbaruk: lending protocol
- Armand: crashes and liquidations
- Noé: risk guardian and stress test

## Running the project

Requirement: stable Rust (edition 2024).

```bash
cargo run          # starts the server on http://127.0.0.1:3000
cargo test         # unit and integration tests
cargo bench        # criterion benchmarks
```

Example session with `curl`:

```bash
H='Content-Type: application/json'
curl -X POST localhost:3000/supply  -H "$H" -d '{"user":"alice","amount":1000000}'    # 10,000 USDC
curl -X POST localhost:3000/deposit -H "$H" -d '{"user":"bob","amount":1000000000}'    # 1 ETH
curl -X POST localhost:3000/borrow  -H "$H" -d '{"user":"bob","amount":150000}'        # 1,500 USDC
curl -X POST localhost:3000/price   -H "$H" -d '{"price":150000}'                      # ETH at 1,500 USDC
curl localhost:3000/positions                                                          # bob can be liquidated
```

Amounts are integers: USDC in cents, ETH in gwei, and the price in USDC cents per ETH.

## Code layout

| Module | Role |
|---|---|
| `types.rs` | domain types: amounts, prices, percentages, position, parameters |
| `error.rs` | `LendingError`, the single error type of the library |
| `protocol.rs` | state of the pool and lending operations |
| `interest.rs` | interest rate model and interest accrual |
| `persistence.rs` | saving and loading the state as JSON |
| `guardian.rs` | risk guardian: 60-minute drop and borrowing restrictions |
| `liquidation.rs` | liquidation rule for one position |
| `scenario.rs` | loading the historical crashes from `data/` |
| `replay.rs` | replay of a crash, minute by minute |
| `bots.rs` | competing liquidation bots |
| `stress_test.rs` | random crashes replayed on several threads |
| `report.rs` | reports of the stress test and of the replay |
| `api/` | axum router, shared state and routes |

## Lending protocol

### Routes

| Method | Route | Effect |
|---|---|---|
| POST | `/supply` | adds USDC to the pool |
| POST | `/deposit` | adds ETH as collateral |
| POST | `/borrow` | borrows USDC |
| POST | `/repay` | repays debt (an amount larger than the debt is capped at the debt) |
| POST | `/withdraw` | withdraws ETH |
| GET | `/pool` | liquidity, total debt, utilization, rate |
| GET | `/positions` | positions with their health factor |
| GET / POST | `/price` | reads or sets the ETH price |
| POST | `/save`, `/load` | saves or reloads the state in `state.json` |

On error, the response has a suitable HTTP status (400, 404, 422, 500) and the body `{ "error": "<message>" }`.

`state.json` is written at the root of the project, known at compile time (`env!("CARGO_MANIFEST_DIR")` in `api::default_state_file`), not in the folder the server is started from. A save is therefore found again after a restart, whatever that folder. The file is ignored by Git.

### Design choices

- **A newtype for each unit.** `EthAmount`, `UsdcAmount`, `Price` and `Bps` wrap a `u64`. The compiler therefore refuses to add ETH to USDC, or to pass a price where an amount is expected. Thanks to serde, these types stay plain numbers in JSON.
- **No floating-point numbers.** Every computation uses integers. Intermediate products go through a `u128` to avoid overflows.
- **A single error type.** Every function that can fail returns `Result<_, LendingError>`. The API turns each variant into an HTTP status through `IntoResponse`.
- **A refused operation changes nothing.** Each operation works on a copy of the position and only writes it back once every check has passed.
- **Interest without rounding loss.** One minute of interest on a small debt is worth less than a cent. That remainder is kept in the position and carried over to the next minute. Over a year, the exact annual rate is recovered.

### Shared state

The `Protocol` is held by an `Arc<Mutex<Protocol>>` in `AppState`. axum passes it to every route through `State`, and the replay and the bots receive a copy of the pointer. Each access takes the lock for a short operation and releases it right away. `/save` copies the state and releases the lock before writing to disk. `/load` reads the file before taking the lock.

A `std::sync::Mutex` is enough: the lock is never held across an `.await`, so it does not block the tokio runtime.

### Benchmark

`cargo bench` measures two loops on a pool of 10,000 positions:

| Benchmark | Measures | Typical result |
|---|---|---|
| `health_factor_10000_positions` | finding which positions can be liquidated, as a bot does at each step | about 80 µs |
| `accrue_interest_10000_positions` | adding one minute of interest to every position | about 85 µs |

Both loops run at every simulated minute: they set the speed of a replay. To make them faster, the positions could be stored in a `Vec` rather than a `BTreeMap`, which makes the traversal more cache-friendly.

## Crashes and liquidations

### Data

`data/` holds the ETH price minute by minute during three crashes, in the format of section 5.2 of the specification:

| Scenario | Period | Minutes | Real price (start → lowest) | Max drop |
|---|---|---|---|---|
| `black-thursday` | March 12, 2020 | 1,440 | 195.02 → 101.37 USDC | −48% |
| `luna` | May 9 to 12, 2022 | 5,760 | 2,524.06 → 1,800.01 USDC | −29% |
| `ftx` | November 7 to 10, 2022 | 5,760 | 1,569.26 → 1,076.24 USDC | −31% |

These prices are the close prices of the one-minute ETH/USDT candles from the public Binance API. `scripts/fetch_scenarios.sh` downloads them once with `curl` and `jq`, and stops if a minute is missing. The server therefore depends on no external service. `scenario.rs` only accepts known ids: an id received by the API cannot point to another file on disk.

The server finds `data/` from the root of the project, known at compile time (`env!("CARGO_MANIFEST_DIR")` in `scenario::data_dir`), not from the folder it is started from. `cargo run` and `target/release/crashlend` therefore work from any folder. The only condition is that the project folder still exists: a binary copied to another machine would not find the scenarios.

### Routes

| Method | Route | Effect |
|---|---|---|
| GET | `/scenarios` | available scenarios, with their length and real starting price |
| POST | `/replay/{id}` | starts the replay in the background and answers 202 right away (409 if a replay is already running) |
| GET | `/liquidations` | liquidations of the current or last replay: minute, bot, user, repaid debt, seized collateral |
| GET | `/report` | report of the last finished replay, or `null` |

### Demo

```bash
H='Content-Type: application/json'
curl -X POST localhost:3000/supply  -H "$H" -d '{"user":"alice","amount":10000000}'   # 100,000 USDC
curl -X POST localhost:3000/deposit -H "$H" -d '{"user":"bob","amount":1000000000}'   # 1 ETH
curl -X POST localhost:3000/deposit -H "$H" -d '{"user":"carol","amount":1000000000}'
curl -X POST localhost:3000/deposit -H "$H" -d '{"user":"dave","amount":1000000000}'
curl -X POST localhost:3000/borrow  -H "$H" -d '{"user":"bob","amount":140000}'       # 1,400 USDC
curl -X POST localhost:3000/borrow  -H "$H" -d '{"user":"carol","amount":120000}'     # 1,200 USDC
curl -X POST localhost:3000/borrow  -H "$H" -d '{"user":"dave","amount":100000}'      # 1,000 USDC
curl -X POST localhost:3000/replay/black-thursday                                     # about 15 s
curl localhost:3000/liquidations                                                      # during or after
curl localhost:3000/report                                                            # once finished
```

The report gives the replayed prices, the maximum drop, the number of liquidations, the repaid debt, the final bad debt and the details per bot. With three borrowers at 1,400, 1,200 and 1,000 USDC, our runs gave between 8 and 11 liquidations. Their number and how they are split between the bots change from one replay to the next, since the bots compete. Only the outcome is stable: no bad debt.

### Liquidation rule

`liquidation::liquidate` applies section 4.5 to one position. It is a synchronous function, used both by the bots and by the stress test.

- It refuses a healthy position (`PositionHealthy`), a zero amount and an unknown user.
- It repays at most 50% of the debt and gives the liquidator `repaid × 1.05 / price` in ETH.
- **When the collateral is no longer enough, we follow Aave V3** (`_calculateAvailableCollateralToLiquidate` in `LiquidationLogic.sol`): the liquidator takes all the remaining collateral but only repays its value, bonus deducted. The rest of the debt stays visible as bad debt. If the liquidator still repaid 50% of the debt, it would pay for collateral that does not exist, and the bad debt would disappear artificially.
- A position without collateral can no longer be liquidated (`NothingToLiquidate`, the equivalent of `CollateralCannotBeLiquidated` in Aave), and its debt no longer earns interest.

What we do not take from Aave: the 100% close factor below a health factor of 0.95, the dust rule and the `deficit` counter. The two measures of the loss are not the same. Aave only records a `deficit` when a liquidation leaves a position without collateral. Our bad debt (`Protocol::total_bad_debt`, section 4.6) also counts underwater positions that still hold ETH: it is what the pool would lose if every position were closed at the current price. It is therefore at least equal to Aave's `deficit`, and can be positive when the latter is zero. That is the case of a replay without liquidators: no liquidation happens, the borrowers keep all their ETH, and yet their debt is larger than the value of that collateral.

### Replay and bots

At each minute, the replay updates the price, adds one minute of interest and informs the guardian, then waits 10 ms. It sends the current minute to the bots through a `tokio::sync::watch` channel. At the end, it sends them `Finished`, waits for them to stop and stores the report.

Three bots run alongside the replay:

| Bot | Reaction time | That is |
|---|---|---|
| `fast` | 20 ms | 2 simulated minutes |
| `average` | 50 ms | 5 simulated minutes |
| `slow` | 150 ms | 15 simulated minutes |

Each bot spots the positions that can be liquidated, waits for its reaction time, then tries to liquidate them by asking for the whole debt (the rule caps it at 50%). While it waits, a faster bot may have taken the same position, or the price may have moved back up: the attempt is then counted as missed (`missed`). This is how a real blockchain works, where the fastest transaction wins.

**Price rescaling.** The scenarios hold the real prices (195 USDC at the start of Black Thursday), while the pool starts at 2,000 USDC. Replaying those prices as they are would make ETH drop by 90% at the first minute. The replay therefore multiplies each price by `current price / first price of the scenario`: the crash keeps exactly its shape (−48% at the lowest for Black Thursday), whatever the starting price. Liquidations and the guardian only depend on these moves, not on the price level.

### Async or threads

- **The replay and the bots are tokio tasks.** They spend almost all their time waiting (`tokio::time::sleep`). A waiting task costs almost nothing, whereas a system thread would keep its stack and its place in the scheduler to do nothing, which means 4 blocked threads per replay.
- **The `/replay` route answers right away.** It starts the replay with `tokio::spawn` and does not wait for it: the report is stored in the shared state at the end.

### Shared state

The replay, the bots and the routes share the same `AppState`. Its `replay` state (replay in progress, history, last report) has its own `Mutex`. The locks are always taken in the same order: protocol, then guardian, then replay. Two tasks can therefore never wait on each other. `replay::start` holds the protocol and the replay at the same time: two simultaneous requests cannot start two replays.

No lock is held while waiting. The compiler actually guarantees it: a `std::sync::MutexGuard` is not `Send`, so a task started with `tokio::spawn` that held one across an `.await` would not compile.

### Benchmark

`cargo bench --bench replay` replays the whole of Black Thursday (1,440 minutes) without any delay between steps, on a pool of 1,000 borrowers. It measures the work done at each minute: setting the price, adding interest to every position and informing the guardian. The bots are left out: they wait for a fixed time, so with them the benchmark would measure the clock rather than the code.

| Benchmark | Time | Per simulated minute |
|---|---|---|
| `replay_black_thursday_1000_positions` | 7.7 ms | about 5.4 µs |

**What the benchmark revealed.** The first measurement gave 1.69 s, that is 1.2 ms per minute when the computation takes a few microseconds. The tokio timer has a 1 ms resolution: even `sleep(Duration::ZERO)` waits for the next tick. Without a delay, the replay therefore simply yields to the other tasks (`tokio::task::yield_now`), and the replay is 219 times faster. The integration test benefits from it too.

During the demo, at 10 ms per minute, the computation takes less than 0.1% of the time: the replay spends its time waiting, which confirms the choice of a tokio task. To speed up the computation itself, the leads are those of the stress test flamegraph: `accrue_interest` recomputes the total debt at every minute, and the positions are stored in a `BTreeMap` rather than a `Vec`.

## Risk guardian and stress test

### Risk guardian

At each new price, the guardian computes the drop between the highest price of the last 60 minutes and the current price. It derives a mode from it, which only applies to new borrows:

| Mode | Drop over 60 min | New borrows |
|---|---|---|
| `Normal` | < 5% | 75% LTV |
| `Alert` | 5% to 15% | LTV lowered to 50% |
| `Emergency` | ≥ 15% | paused (`BorrowingPaused`) |

Existing positions are never modified. The guardian only acts through two fields of `Protocol`: `borrowing_paused` and `max_ltv_override`. `Protocol` therefore does not depend on the guardian.

These two fields are saved with the rest of `Protocol`. After a `/load`, the guardian therefore observes the reloaded price, as after `POST /price`. Otherwise, the file would bring back the restrictions of the moment of the save, even with a guardian disabled since then, and the guardian would not see the gap between the reloaded price and those of the last hour.

### Stress test

The stress test replays N random crashes on the same portfolio, once without the guardian and once with it.

- **Crash:** a straight-line drop of 10 to 70% over 30 minutes to 24 hours, with ±0.5% of noise per minute.
- **Portfolio:** 80 borrowers between 30 and 70% LTV, and 20 "dip buyers". The latter borrow the maximum allowed once the price has dropped by 2.5%, 5%, … or 50%.
- **Synchronous liquidation:** each minute, the liquidators liquidate at most 2 positions with the rule of section 4.5 (`liquidation.rs`), which repays up to 50% of the debt against collateral with a 5% bonus. A position with nothing left to seize does not take a slot. Without this capacity limit, liquidation always keeps up with a progressive crash and no bad debt appears. That is precisely what went wrong during Black Thursday.
- **Survival:** the protocol survives if the bad debt is zero at the end of the crash.

Result with 10,000 scenarios and seed 42:

| | Without guardian | With guardian |
|---|---|---|
| Survival rate | 91.2% | 92.3% |
| Average bad debt | 1,536.59 USDC | 939.41 USDC |

The guardian reduces the bad debt by 39%, but barely improves the survival rate. In the most violent crashes, the loss comes from positions opened before the crash, which the guardian does not touch.

None of the 2,888,901 liquidations of the 20,000 replays targets a position that is already insolvent, and none exhausts its collateral. The bad debt comes from positions the liquidators do not reach in time: they become insolvent while waiting for their turn. The remaining-collateral cap of section 4.5 therefore does not change these figures.

### Routes

| Method | Route | Effect |
|---|---|---|
| GET | `/risk` | mode, observed drop (in bps) and state of the guardian |
| POST | `/risk/toggle` | turns the guardian on or off |
| POST | `/stress-test` | `{ "scenarios": 10000 }`, returns the stress test report (100,000 scenarios at most) |

`POST /price` counts as one minute for the guardian. It can therefore be tested by hand:

```bash
curl -X POST localhost:3000/price -H "$H" -d '{"price":200000}'
curl -X POST localhost:3000/price -H "$H" -d '{"price":160000}'   # -20%: Emergency
curl localhost:3000/risk
curl -X POST localhost:3000/stress-test -H "$H" -d '{"scenarios":10000}'
```

### Async or threads

- **The stress test runs on system threads** (`std::thread::scope`). It is pure computation, without waiting: each thread gets a slice of the scenarios and runs it on its own core. `scope` lets the threads borrow the portfolio without an `Arc`. Each scenario works on a clone of `Protocol`, so the threads have no lock to take.
- **The route runs this computation in `tokio::task::spawn_blocking`.** Run directly in an async handler, it would keep a tokio worker busy for several seconds and slow down the other requests.
- **Each scenario has its own seed** (`seed + index`). The result is therefore the same whatever the number of threads, which `tests/stress.rs` checks.

The guardian of the API has its own `Mutex`, next to the one of the protocol. When both are needed, the protocol is always locked first, which rules out any deadlock.

### Benchmark and flamegraph

`cargo bench --bench stress_test` measures 10,000 scenarios on 1 thread, then on every core. One measurement takes several seconds: criterion takes 10 flat samples (`SamplingMode::Flat`), and the benchmark takes about 8 minutes.

| Benchmark | Time | Throughput |
|---|---|---|
| `stress_test/threads/1` | 12.9 s | 776 scenarios/s |
| `stress_test/threads/8` | 2.80 s | 3,576 scenarios/s |

On 8 cores, the speedup is ×4.6 and not ×8. Two likely reasons: the processor (Apple Silicon) mixes performance and efficiency cores, and the slices do not all take the same time, while the slowest thread sets the total time.

#### Flamegraph

[`docs/flamegraph-stress-test.svg`](docs/flamegraph-stress-test.svg) profiles the `stress_test/threads/1` benchmark. It is produced with standard tools: `sample`, the sampler built into macOS, and [inferno](https://github.com/jonhoo/inferno), Brendan Gregg's flamegraph scripts ported to Rust.

On macOS, `cargo flamegraph` goes through `xctrace`, which requires the full Xcode. `sample` needs neither Xcode nor root rights.

```bash
cargo install inferno rustfilt
BIN=$(CARGO_PROFILE_BENCH_DEBUG=true cargo bench --bench stress_test --no-run 2>&1 \
  | grep -o 'target/release/deps/stress_test-[0-9a-f]*')
$BIN --bench --profile-time 40 stress_test/threads/1 > /dev/null & sleep 5
sample $! 30 1 -file /tmp/stress.sample
rustfilt < /tmp/stress.sample | inferno-collapse-sample \
  | sed -E 's/stress_test-[0-9a-f]+`//g' \
  | inferno-flamegraph --title "CrashLend: stress test on 1 thread (cargo bench)" \
  > docs/flamegraph-stress-test.svg
```

On Linux, `cargo flamegraph` goes through `perf`: `cargo flamegraph --bench stress_test -- --bench --profile-time 30 stress_test/threads/1`.

`sample` does not see inlined functions: `liquidate`, `health_factor` and `buy_the_dip` appear under `simulate`.

| Function | Share of time | Cause |
|---|---|---|
| `simulate` (inlined code) | 51.5% | mostly the health factor of every position, at every minute (`liquidate`). Its `u128` division alone takes 25.8% of the time. |
| `Protocol::accrue_interest` | 43.8% | the `u128` computation of the interest of every position, at every minute. Within these 43.8%, `current_rate` takes 12.6%: `total_debt` walks through every position again at every minute to compute the utilization. |
| `Guardian::observe` | 2.9% | maximum over the last 60 prices |
| `memcmp` | 1.7% | `String` comparisons in `liquidate` and the other accesses to the `BTreeMap` |

Leads for improvement:

- **Done: no longer look up dip buyers that have already borrowed.** A buyer only borrows once, and its debt never goes back to zero, since a liquidation repays at most half of it. A flag per buyer therefore avoids looking up its position in the `BTreeMap` at every minute. `memcmp` goes from 17.2% to 1.7% of the time, and the benchmark from 16.4 s to 12.9 s on 1 thread (−21.5%) and from 3.48 s to 2.80 s on 8 threads (−19.6%), with identical results.
- **`is_liquidatable` without a division.** Comparing `collateral value × threshold < debt × 10,000` gives the same result with a single multiplication. It is now the biggest lead: this division takes 25.8% of the time.
- **Keep the total debt up to date in `Protocol`** at every borrow, repayment and interest accrual, instead of recomputing it. This removes the second pass over the positions at every minute (12.6% of the time).
- **The guardian is a low priority.** A monotonic queue would give the maximum in O(1), but it cannot gain more than its 3%.
