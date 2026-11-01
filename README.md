# Anchored VWAP touch study

A Rust research pipeline that asks one question about Nasdaq 100 futures (NQ): when price touches the daily anchored VWAP on 5 or 15 minute bars, does anything known at the touch predict whether price rejects the VWAP, breaks through it, or goes nowhere, well enough to trade after costs?

The pipeline cleans 1 minute bars, builds the anchored VWAP, detects touches, labels each touch with a volatility scaled triple barrier, compares VWAP touches with control levels, and then tests hypotheses and trading strategies under a protocol that limits false discoveries.

**In short.** On the data in this repository no usable directional edge was found. Everything specified is built: the pipeline, the research protocol, calibrated models, purged walk forward validation, a cost aware backtest, the Deflated Sharpe Ratio and the Probability of Backtest Overfitting, a report and a calendar snapshot. The whole record is in git and can be reproduced with four commands (see the quick start).

## Contents

* [Result at a glance](#result-at-a-glance)
* [How it fits together](#how-it-fits-together)
* [Quick start](#quick-start)
* [The data](#the-data)
* [The pipeline](#the-pipeline)
* [The research protocol](#the-research-protocol)
* [Models, backtest, report and calendar](#models-backtest-report-and-calendar)
* [Configuration](#configuration)
* [Repository layout](#repository-layout)
* [Where to read next](#where-to-read-next)
* [Development](#development)
* [Limitations and what is not built](#limitations-and-what-is-not-built)

## Result at a glance

| Stage | Question | Outcome | Record |
|---|---|---|---|
| Gate 3a | Do VWAP touches behave like touches of arbitrary levels? | Yes, to within about 2 percentage points on the rejection rate. Reported, never decisive. | `docs/DECISIONS.md`, section 11 |
| Omnibus screen | Is there any predictive structure? | Yes, but it is the clock driven chop rate, which has nothing to do with VWAP. Direction is not predictable. | section 13 |
| Gate 3b | Does any pre registered hypothesis survive? | Not passed. Seven cards on the discovery segment (2016 to 2021): five dropped, two left at refine, none promoted. Confirmation and holdout were never used. | section 14 |
| Strategies S001 to S003 | Does a calibrated model, traded with costs, earn money out of sample? | No. Logistic: Brier skill 0.0011 (5 minute) and 0.0298 (15 minute). LightGBM: 0.0015 and 0.0274. Every fold abstained, so no trades. | section 16 |

The 15 minute models have genuine probability skill, but it comes from range and clock features, the same chop structure as before. No threshold on the rejection probability earns more than the round trip cost of 2.8 ticks on out of fold data. The ledger holds 14 counted trials (the omnibus, 7 hypothesis cards and 6 strategy runs) and 0.0333 of the 0.05 alpha is spent.

The only thread above noise is level confluence (a touch that coincides with prior day or overnight levels): +2.3 points for one coinciding level and +6.6 points for two or more. It is not significant and the data is too small to settle it.

The full record is in `docs/DECISIONS.md` (sections 13, 14 and 16) and in `research/`.

## How it fits together

```
data.parquet (1 minute bars)
  => clean         cleaned bars, true UTC, quality and roll reports
  => events        anchored VWAP, ATR, 5 and 15 minute bars, touches
  => labels        rejection, breakthrough or chop (triple barrier)
  => features      34 columns known at the touch
  => baselines     control levels, gate 3a
  => research      hypothesis cards: omnibus, discovery, confirmation, holdout
  => strategies    walk forward, calibrated model, threshold, backtest, DSR and PBO
  => report        summary and plots
every research step appends to research/ledger.jsonl, which verify and replay check
```

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
* `pipeline` runs data cleaning, event detection, labels, the feature export, the baseline report and the spot check charts, then verifies the research ledger. About 10 seconds on the included data.
* `research verify` checks that the research ledger follows the protocol and that every commit it names exists.
* `research replay all` recomputes every recorded hypothesis card and strategy run from the raw data and compares it with the ledger. All 13 reproduce, because the whole chain is deterministic.

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
vwap features build config    # the joined event, label and feature tables as parquet
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

## Models, backtest, report and calendar

A strategy card (`hypotheses/S001.toml`) names a model (`majority`, `logistic` or `lgbm`), the timeframes and optionally a feature list, and is registered and counted in the same ledger. `vwap research strategy S001 config` runs it on the discovery segment:

* a purged rolling walk forward (24 months of training, 6 months of testing, five folds) with a nested, cross fitted scheme inside each fold that picks the hyperparameters, calibrates the probabilities (isotonic or Platt) and tunes a trade threshold on out of fold data only;
* a cost aware backtest of the pooled out of fold signals: entry at the next bar open, exits at the label barriers, 1 tick of slippage per side and 2 USD of commission per side, one position at a time;
* gates on the calibrated probabilities and on the net expectancy, and the Deflated Sharpe Ratio and the Probability of Backtest Overfitting over every counted trial. Failed gates are listed in the ledger entry. A summary with the per fold results, the daily PnL series and the data for the plots is written to `research/strategies/` (tracked).

Only a promoted strategy can be confirmed (once, on the confirmation segment) and then run on the holdout (once). `vwap research dsr config` prints the DSR and PBO of every trial. `vwap report build config` writes `reports/summary.md` and the plots: reliability diagrams, equity and drawdown, feature importance with its spread over folds, and a beeswarm of the contributions to the rejection score, computed by multiclass TreeSHAP written in safe Rust (checked against LightGBM's own values on binary models).

```bash
vwap research register S004 config
vwap research strategy S004 config
vwap research dsr config
vwap report build config
vwap features build config         # parquet export of the sample table
vwap calendar fetch config         # optional: CPI, NFP, FOMC schedule snapshot, needs internet
```

The calendar features (release day, minutes until and since the next CPI, NFP and FOMC release) are off. They need a frozen snapshot from the official pages, which could not be fetched from the development environment. Run `vwap calendar fetch config` once on a machine with internet access, commit `research/calendar/`, and set `calendar_enabled = true` in `config/features.toml`. If a page changed the command fails with the reason and `vwap calendar import` accepts dates you supply.

## Configuration

All parameters live in `config/*.toml` and are read into typed, validated structs (`crates/vwap_core/src/config.rs`).

* `data.toml`: raw file and its checksum, timestamp conversion, session anchor, quality thresholds.
* `events.toml`: timeframes, ATR length, re arm excursion, exclusion window, chart settings.
* `labels.toml`: barrier multiples, horizons and the descriptive sensitivity grid.
* `features.toml`: feature windows, confluence band, time blocks. Equal to the values behind every recorded result.
* `baselines.toml`: control levels, strata, bootstrap and the equivalence margin.
* `research.toml`: the three way split, the alpha budget, verdict thresholds, paths of the ledger and reports.
* `model.toml`: the fixed LightGBM settings of the omnibus screen.
* `validation.toml`: walk forward geometry, embargo, inner blocks and the gate limits (no dates).
* `strategy.toml`: model grids, calibration, the threshold rule.
* `backtest.toml`: slippage, commission, position policy, the cost sweep.
* `report.toml`, `calendar.toml`: the report and the calendar snapshot.

## Repository layout

```
config/        all parameters
crates/        vwap_core (config, types, manifests)  vwap_data (cleaning)
               vwap_events (VWAP, ATR, touches)  vwap_labels (triple barrier)
               vwap_features (34 features)  vwap_dataset (joined table, export)
               vwap_baselines (controls, gate 3a)  vwap_validation (walk forward, DSR, PBO)
               vwap_model (models, calibration, TreeSHAP)  vwap_backtest (fills, costs)
               vwap_calendar (release schedule snapshot)  vwap_research (the protocol)
               vwap_report (charts)  vwap_cli (the vwap binary)  vwap_testkit (test data)
docs/          specifications, the protocol, decisions and results, operations
hypotheses/    one TOML card per hypothesis or strategy
research/      the ledger, the card reports and the strategy summaries (tracked)
data.parquet   the raw input
```

## Where to read next

| Document | What it owns |
|---|---|
| `docs/OPERATIONS.md` | Building, running, reproducing and troubleshooting |
| `docs/PROTOCOL.md` | The research protocol: splits, cards, alpha budget, strategy cards |
| `docs/DECISIONS.md` | Every decision and every result, in order |
| `docs/ARCHITECTURE.md` | The crates and the data flow |
| `docs/DATA.md`, `docs/EVENTS.md`, `docs/LABELING.md` | Cleaning, touches and the triple barrier labels |
| `docs/FEATURES.md` | The 34 feature columns and their causal proofs |
| `docs/BASELINES.md` | Control levels and gate 3a |
| `docs/MODELING.md`, `docs/VALIDATION.md`, `docs/BACKTEST.md` | Models and calibration, walk forward, DSR and PBO, fills and costs |
| `docs/ROADMAP.md`, `docs/GLOSSARY.md` | The phase plan with its gates, and the symbols |
| `CLAUDE.md` | Conventions for anyone changing the code |

## Development

```bash
cargo test --workspace
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo fmt --all --check
```

There are about 215 tests, including property tests that replace every bar after a cut and require labels, features, VWAP and events to stay bit for bit identical (no lookahead), golden fingerprints of the features, a check of TreeSHAP against LightGBM, and end to end tests that drive the real binary and the whole strategy protocol (promotion, one shot confirmation, one holdout run) on synthetic data. Conventions are in `CLAUDE.md`, the runbook is `docs/OPERATIONS.md`.

## Limitations and what is not built

* The weight is a tick count and the series is not exchange NQ. Conclusions may not transfer to real NQ volume.
* The discovery segment is only about 4.9 years, so effects smaller than a few percentage points cannot be established.
* Not built: the per trade log and a standalone backtest command, breakdowns of the strategy metrics by year, regime and side, reliability bands, and the designed features for volume trend, cumulative relative volume, opening range levels, realised volatility percentile and the overnight gap. The calendar fetch has not been run against the live sites. See `docs/DECISIONS.md`, section 16.
* The strategy tests use a synthetic market with a planted signal. They prove the plumbing and the gates, not anything about NQ.

## License

MIT, see `LICENSE`.
