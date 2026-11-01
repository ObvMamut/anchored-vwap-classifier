# CLAUDE.md

Conventions for anyone (human or agent) working in this repository. Read this first, then the doc that owns the stage you are touching.

## Project in five lines

1. Instrument: NQ (Nasdaq 100 futures), about 8.9 years (2016-11 to 2025-10) of 1 minute OHLCV bars in `data.parquet`.
2. Event: a 5 or 15 minute bar whose range contains the daily anchored VWAP (a "touch").
3. Label: REJECTION, BREAKTHROUGH or CHOP from a volatility scaled triple barrier.
4. Question: do features known at the touch predict the class better than base rates, with an edge that survives costs?
5. Output: calibrated class probabilities. Trade only when $\hat P(\text{rejection}) > \tau$, otherwise abstain.

Status: Phases 1 to 3 are built (data, events, labels, controls). The research loop harness and a 17 feature library are built. Cycle 0 is done. Hypothesis cards run next. See `docs/DECISIONS.md` for results and `docs/PROTOCOL.md` for the loop rules.

## Hard rules

1. **No lookahead anywhere.** See the operational definition below. Every feature function and every label function has a proptest that alters future bars and asserts the output is unchanged.
2. **No tuning on test folds or on the holdout.** The last 2 years of data are the holdout and are run once only.
3. **All parameters live in `config/*.toml`**, deserialised with serde into typed structs. No magic numbers in code.
4. **Never compute VWAP on back adjusted prices.** Prices are never adjusted anywhere in the pipeline.
5. **No Python anywhere in the pipeline or the repository.**
6. Every experiment is logged and counted (see `docs/VALIDATION.md`).
7. **The research loop follows `docs/PROTOCOL.md`.** Hypotheses are written as cards before they are run, runs touch the discovery segment only, the budget of looks is enforced by the ledger, confirmation data is used once per promoted card, and the holdout once at the end. Never peek at confirmation or holdout rows.

## The no lookahead rule, operationally

1. A bar with open time $s_i$ and width $w$ becomes known at its close $s_i + w$. A decision made "at bar $t$" may use only information with timestamp at or before $s_t + w$.
2. A feature function receives a `CausalView` that the engine builds from the touch index. The view exposes bars at indices up to and including $t$ and nothing after. A label function receives a forward window built the same way from $t+1$ to $t+T$. Neither function can index outside its legal range.
3. Every feature and label function has a proptest of this form: generate random bars, pick a cut index, replace every bar after the cut with different random bars, recompute, assert the output is bit for bit identical.
4. A training event $j$ may be used for a test window starting at $a$ only if its label horizon, plus the embargo, ended before $a$ (purge plus embargo).
5. Clock based rules (for example "drop events whose horizon would cross the session end") are allowed because they depend on the calendar, not on future prices. Rules that depend on future prices are not.
6. Statistics computed over a window (percentiles, means, scaling constants) use only data strictly before the event, or only the training fold.

## Workspace layout

```
Cargo.toml  rust-toolchain.toml  rustfmt.toml  CLAUDE.md
config/   default.toml data.toml events.toml labels.toml features.toml
          baselines.toml model.toml validation.toml backtest.toml report.toml
crates/   vwap_core        shared types, typed config, errors, cost constants
          vwap_data        load, clean, roll table, quality report, yearly parquet
          vwap_events      anchored VWAP, ATR, bar sampling, touch detection
          vwap_labels      triple barrier labels, MAE, time to barrier
          vwap_features    feature functions, CausalView, lookahead tests
          vwap_baselines   control levels, class frequencies, gate tests
          vwap_model       majority, logistic, LightGBM, calibration, metrics, SHAP
          vwap_validation  purged walk forward, holdout, experiment log, DSR, PBO
          vwap_backtest    trade simulation, costs, metrics
          vwap_report      plotters charts and the Markdown report
          vwap_research    closed loop harness: split views, cards, ledger, runners
          vwap_cli         binary named vwap, one subcommand per stage
docs/     ARCHITECTURE DATA EVENTS LABELING FEATURES BASELINES MODELING
          VALIDATION BACKTEST ROADMAP DECISIONS GLOSSARY PROTOCOL
hypotheses/   one TOML card per hypothesis
data.parquet   raw input, stays at the repository root
data/ artifacts/ reports/   generated output, git ignored
```

Crate and directory names use underscores. `vwap_core` is the one crate beyond the nine requested modules; it exists to avoid cyclic or duplicated types.

## Commands

Build, test, lint (all must pass before any commit):

```bash
cargo build --workspace --release
```

```bash
cargo test --workspace
```

```bash
cargo clippy --workspace --all-targets --all-features -- -D warnings
```

```bash
cargo fmt --all --check
```

Heavier property test run (nightly or before a release of results):

```bash
PROPTEST_CASES=1000 cargo test --workspace
```

Pipeline stages, in order. Each takes the config directory as its argument and writes to `artifacts/<stage>/` with a manifest (input hashes, config hash, git SHA). A stage refuses to run on stale or missing inputs.

```bash
vwap data clean config
vwap events detect config
vwap labels build config
vwap baselines run config
vwap calendar fetch config
vwap features build config
vwap model walkforward config
vwap backtest run config
vwap spotcheck config
vwap report build config
vwap holdout run config
vwap research run H001 config
vwap research status config
vwap research confirm H001 config
```

`vwap calendar fetch` is the only stage allowed to use the network. `vwap holdout run` works once only; it writes a lock file and refuses to run again.

## Error handling

* One `thiserror` enum per library crate. `anyhow` is used only in `vwap_cli`, at the top.
* Workspace lints deny `unwrap_used`, `expect_used`, `panic` and `indexing_slicing` outside test code. `forbid(unsafe_code)` in every crate.
* Data problems are errors, never silent repairs. Anything repaired (duplicate rows dropped, bars flagged) is counted and written to the stage report.
* Missing feature values are `f64::NAN`, counted per feature, and handled explicitly by each model (LightGBM natively, logistic regression by train fold imputation plus a missing indicator).
* Config validation runs at load. Examples of rejected configs: embargo smaller than $T$ bars, slippage below 1 tick per side, holdout overlapping the development range.
* Numeric code uses `f64` and the tick grid constant from `vwap_core` (tick $=0.25$ points, tick value $=5$ USD).

## Determinism

* Seeds come from config. Randomness uses `rand_chacha::ChaCha8Rng` only.
* LightGBM: `deterministic=true`, `force_row_wise=true`, fixed `num_threads`, and all seed parameters set from the config seed.
* rayon is used only across folds or across independent sessions, with indexed collection that preserves order. No parallel floating point reductions.
* Polars `group_by` always sets `maintain_order`.
* `Cargo.lock` is committed. Toolchain is pinned in `rust-toolchain.toml`.

## Style

* Code reads like the surrounding code. Match comment density, naming and idiom.
* rustfmt defaults plus the repo `rustfmt.toml`. Clippy with warnings as errors.
* Docs: every formula in LaTeX. No em dashes, en dashes or hyphens in prose. Use numbered lists or asterisk bullets.

## How to add a feature

1. Write the exact formula and units in `docs/FEATURES.md`.
2. Write the causal proof paragraph: list the inputs and show every index is at most $t$.
3. Implement it behind the `Feature` trait using only `CausalView`.
4. Add the proptest that alters future bars and asserts equality.
5. Add its config entry and default lookbacks in `config/features.toml`.
6. Add it to the lookahead matrix at the end of `docs/FEATURES.md`.

## Where to read next

Start at `docs/ARCHITECTURE.md`, then the stage doc. Decisions and open items are in `docs/DECISIONS.md`. Symbols are in `docs/GLOSSARY.md`. The phase order and go or no go gates are in `docs/ROADMAP.md`.
