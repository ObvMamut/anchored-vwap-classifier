# Anchored VWAP touch study

A Rust research pipeline that asks one question about Nasdaq 100 futures (NQ): when price touches the daily anchored VWAP on 5 or 15 minute bars, does anything known at the touch predict whether price rejects the VWAP, breaks through it, or goes nowhere, well enough to trade after costs?

The pipeline cleans 1 minute bars, builds the anchored VWAP, detects touches, labels each touch with a volatility scaled triple barrier, compares VWAP touches with control levels, and then tests hypotheses under a protocol that limits false discoveries.

## Result

On the data in this repository the answer is **no usable directional edge was found**.

* Gate 3a (do VWAP touches behave like touches of arbitrary levels?): yes, to within about 2 percentage points on the rejection rate. Reported, never decisive.
* Gate 3b (does any pre registered hypothesis survive?): not passed. An omnibus screen found structure, but it is the clock driven chop rate, which has nothing to do with VWAP. Direction (rejection versus breakthrough) is not predictable from 18 features. Seven hypothesis cards were run on the discovery segment (2016 to 2021): five were dropped, two were left at "refine", none was promoted, so the confirmation and holdout segments were never used.
* The only thread above noise is level confluence (a touch that coincides with prior day or overnight levels): +2.3 points for one coinciding level and +6.6 points for two or more. It is not significant and the data is too small to settle it.

The full record is in `docs/DECISIONS.md` (sections 13 and 14) and in `research/`.

## Quick start

Requirements: a Rust toolchain (the exact version is pinned in `rust-toolchain.toml`, `rustup` installs it), `cmake` and a C++ compiler with OpenMP (LightGBM is compiled from source), and about 5 GB of free disk space.

```bash
git clone https://github.com/ObvMamut/anchored-vwap-classifier
cd anchored-vwap-classifier
cargo build --release
```

The first build takes about 5 to 10 minutes because polars and LightGBM are compiled. Then:

```bash
./target/release/vwap doctor config
./target/release/vwap pipeline config
./target/release/vwap research verify config
./target/release/vwap research replay all config
```

* `doctor` checks configuration, the raw data checksum, toolchain, fonts, disk space and git.
* `pipeline` runs data cleaning, event detection, labels, the baseline report and the spot check charts, then verifies the research ledger. About 10 seconds on the included data.
* `research verify` checks that the research ledger follows the protocol and that every commit it names exists.
* `research replay all` recomputes every recorded hypothesis card from the raw data and compares it with the ledger. All seven reproduce exactly, because the whole chain is deterministic.

## The data

`data.parquet` (57 MB, SHA256 pinned in `config/data.toml`) holds 3,077,548 one minute bars from 15 November 2016 to 1 October 2025. Columns: `DateTime`, `Open`, `High`, `Low`, `Close`, `Volume`, `TickVolume`. Things the cleaning stage discovered and handles, and that you should know:

* The file is sorted newest first.
* The timestamps are broker server time (New York time plus 7 hours with US daylight saving), stored as if they were UTC. `config/data.toml` converts them. The cleaned files hold true UTC.
* `Volume` is zero on every row, so the VWAP is weighted by `TickVolume` (a tick count, not traded contracts).
* Prices lie on a 0.1 point grid, so this is a broker style index series with CME like hours, used as a proxy for NQ. Costs are modelled in NQ ticks (0.25 points, 5 USD).
* No contract roll steps are visible, so the series behaves like a continuous vendor series.
* 2017 to 2020 are much gappier than 2021 to 2024.

To use another file, change `input_path` and `input_sha256` in `config/data.toml` and set `timestamp_kind`, `server_offset_hours` and `weight_column` to match it. `vwap data inspect config` prints a first look at a raw file.

## The pipeline

Every stage takes the config directory as its last argument and writes to `artifacts/<stage>/` with a `manifest.json` (configuration hashes, the fingerprint of the cleaned data it read, row counts, git commit). A stage refuses to run if the cleaned data changed after `data clean`.

```bash
vwap data clean config        # raw file to cleaned yearly parquet, quality and roll reports
vwap events detect config     # coarse bars, anchored VWAP, ATR, touch events
vwap labels build config      # triple barrier labels
vwap baselines run config     # control levels and the gate 3a equivalence report
vwap spotcheck config         # candlestick charts of 20 random discovery days
```

Other commands: `vwap data inspect config`, `vwap data levels config`, `vwap doctor config`, `vwap pipeline config`.

## The research protocol

Hypotheses are tested under `docs/PROTOCOL.md`, enforced in code (`crates/vwap_research`):

* The data is split in time into discovery (November 2016 to September 2021), confirmation (October 2021 to September 2023) and holdout (October 2023 onward). Runs can only read the discovery view. Confirmation is one shot per promoted card. The holdout is run once.
* Every hypothesis is a TOML card in `hypotheses/`, committed before it is run. The runner records the card hash and refuses to run a card that changed.
* A budget of 12 cards and a total alpha of 0.05 is enforced by an append only ledger, `research/ledger.jsonl`, which is tracked in git.
* The harness proves itself first: a planted effect must be recovered and a null must not be promoted, otherwise nothing runs.

```bash
vwap research status config        # ledger and remaining budget
vwap research register H008 config # register a committed card
vwap research run H008 config      # run it on the discovery segment
vwap research confirm H008 config  # only after a PROMOTE verdict, once
vwap holdout run config            # needs research/FREEZE.toml and a confirmed card, once
```

To add a hypothesis, copy a card from `hypotheses/`, write the mechanism in words, commit it, then register and run it. Commit the ledger and the new report in `research/` afterwards, because the audit trail only counts if it is in the repository.

## Configuration

All parameters live in `config/*.toml` and are read into typed, validated structs (`crates/vwap_core/src/config.rs`).

* `data.toml`: raw file and its checksum, timestamp conversion, session anchor, quality thresholds.
* `events.toml`: timeframes, ATR length, re arm excursion, exclusion window, chart settings.
* `labels.toml`: barrier multiples, horizons and the descriptive sensitivity grid.
* `features.toml`: feature windows, confluence band, time blocks. Equal to the values behind every recorded result.
* `baselines.toml`: control levels, strata, bootstrap and the equivalence margin.
* `research.toml`: the three way split, the alpha budget, verdict thresholds, paths of the ledger and reports.
* `model.toml`: the fixed LightGBM settings of the omnibus screen.

## Repository layout

```
config/        all parameters
crates/        vwap_core (config, types, manifests)  vwap_data (cleaning)
               vwap_events (VWAP, ATR, touches)  vwap_labels (triple barrier)
               vwap_features (18 features)  vwap_baselines (controls, gate 3a)
               vwap_research (the protocol)  vwap_report (charts)
               vwap_cli (the vwap binary)  vwap_testkit (test data)
docs/          specifications, the protocol, decisions and results, operations
hypotheses/    one TOML card per hypothesis
research/      the ledger and the card reports (tracked)
data.parquet   the raw input
```

## Development

```bash
cargo test --workspace
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo fmt --all --check
```

There are about 80 tests, including property tests that replace every bar after a cut and require labels, features, VWAP and events to stay bit for bit identical (no lookahead), a golden fingerprint of all features, and end to end tests that drive the real binary on synthetic data. Conventions are in `CLAUDE.md`, the runbook is `docs/OPERATIONS.md`.

## Limitations and what is not built

* The weight is a tick count and the series is not exchange NQ. Conclusions may not transfer to real NQ volume.
* The discovery segment is only about 4.9 years, so effects smaller than a few percentage points cannot be established.
* Specified but not implemented, because the research found no signal to model or trade: calendar fetch, a features export, model walk forward with calibration, a cost aware backtest, the Deflated Sharpe Ratio and the Probability of Backtest Overfitting, multiclass TreeSHAP and the full report. See the banners in `docs/MODELING.md`, `docs/BACKTEST.md` and `docs/ROADMAP.md`.

## License

MIT, see `LICENSE`.
