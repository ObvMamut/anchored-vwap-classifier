# CLAUDE.md

Conventions for anyone (human or agent) working in this repository. Read this first, then the doc that owns the stage you are touching. The user level overview is `README.md`, the runbook is `docs/OPERATIONS.md`.

## Project in five lines

1. Instrument: NQ (Nasdaq 100 futures) proxy, about 8.9 years (November 2016 to October 2025) of 1 minute bars in `data.parquet`.
2. Event: a 5 or 15 minute bar whose range contains the daily anchored VWAP (a "touch").
3. Label: REJECTION, BREAKTHROUGH or CHOP from a volatility scaled triple barrier.
4. Question: do features known at the touch predict the class, with an edge that survives costs?
5. Status: the pipeline and the research protocol are built and the research is finished with a negative result (gate 3b not passed, nothing promoted). See `docs/DECISIONS.md`, sections 13 and 14.

## Hard rules

1. **No lookahead anywhere.** See the operational definition below. Every feature function and every label function has a proptest that alters future bars and asserts the output is unchanged.
2. **No tuning on confirmation or holdout data.** The research protocol (`docs/PROTOCOL.md`) is enforced in code by `vwap_research`: runs read the discovery view only, confirmation is one shot per promoted card, the holdout runs once.
3. **All parameters live in `config/*.toml`**, deserialised with serde into typed structs in `crates/vwap_core/src/config.rs`. No magic numbers in code.
4. **Never compute VWAP on back adjusted prices.** Prices are never adjusted anywhere in the pipeline.
5. **No Python anywhere in the pipeline or the repository.**
6. **The research audit trail is tracked.** `research/ledger.jsonl` and `research/reports/` are committed, append only, and never edited by hand. A change to a result needs a new card, not a rewrite.
7. **Results must stay reproducible.** `vwap research replay all config` must keep reproducing every recorded card. A change that breaks it is a behaviour change and needs a new study, not a silent edit.

## The no lookahead rule, operationally

1. A bar with open time $s_i$ and width $w$ becomes known at its close $s_i + w$. A decision made "at bar $t$" may use only information with timestamp at or before $s_t + w$.
2. A feature function receives a `CausalView` that the engine builds from the touch index. The view exposes bars at indices up to and including $t$ and nothing after. A label function receives a forward window built the same way from $t+1$ to $t+T$.
3. Every feature and label function has a proptest of this form: generate random bars, pick a cut index, replace every bar after the cut with different random bars, recompute, assert the output is bit for bit identical.
4. A training event $j$ may be used for a test window starting at $a$ only if its label horizon, plus the embargo, ended before $a$ (purge plus embargo).
5. Clock based rules (for example "drop events whose horizon would cross the session end") are allowed because they depend on the calendar, not on future prices. Rules that depend on future prices are not.
6. Statistics computed over a window (percentiles, means, scaling constants) use only data strictly before the event, or only the training fold.

## Workspace layout

```
Cargo.toml  rust-toolchain.toml  rustfmt.toml  README.md  LICENSE  CLAUDE.md
config/   data events labels features baselines research model (.toml)
crates/   vwap_core        typed config, small enums, constants, errors, manifests
          vwap_data        load, clean, roll table, quality report, yearly parquet
          vwap_events      anchored VWAP, ATR, bar sampling, touch detection
          vwap_labels      triple barrier labels, MAE, time to barrier
          vwap_features    18 features, CausalView, level table
          vwap_validation  purged walk forward, DSR, PBO, normal and chi square maths
          vwap_dataset     the joined sample table, parquet export of features
          vwap_model       metrics, isotonic and Platt calibration, majority and logistic models
          vwap_backtest    next bar open fills, barrier exits, costs, metrics, cost sweep
          vwap_baselines   control levels, bootstrap, gate 3a
          vwap_research    the research protocol: views, cards, ledger, omnibus
          vwap_report      plotters spot check charts
          vwap_cli         the vwap binary (ctx, stages, research, ops)
          vwap_testkit     synthetic data for tests (dev dependency only)
docs/     ARCHITECTURE DATA EVENTS LABELING FEATURES BASELINES MODELING VALIDATION
          BACKTEST ROADMAP DECISIONS GLOSSARY PROTOCOL OPERATIONS
hypotheses/   one TOML card per hypothesis
research/     ledger.jsonl and reports/ (tracked audit trail)
data.parquet  raw input, stays at the repository root
data/ artifacts/ reports/ target/   generated, ignored by git
```

Crate and directory names use underscores. Everything specified is built (see the banners in `docs/MODELING.md`, `docs/BACKTEST.md`, `docs/VALIDATION.md`).

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

Heavier property test run (nightly or before relying on a change):

```bash
PROPTEST_CASES=1000 cargo test --workspace
```

The binary is `target/release/vwap`. Every stage takes the config directory as its last argument.

```bash
vwap doctor config
vwap pipeline config            # doctor, data clean, events detect, labels build, features build, baselines run, spotcheck, research verify
vwap data clean config
vwap events detect config
vwap labels build config
vwap features build config      # artifacts/features/features_<w>m.parquet
vwap baselines run config
vwap spotcheck config
vwap research status config
vwap research register H008 config
vwap research run H008 config
vwap research confirm H008 config
vwap research verify config
vwap research replay all config
vwap holdout run config
```

Each stage writes `artifacts/<stage>/manifest.json` and refuses to run if the cleaned data changed after `data clean`. `vwap holdout run` works once only. Pipeline stages that were specified but not built: calendar fetch, features export, model walk forward, backtest, report build.

## Error handling

* One `thiserror` enum per library crate. `anyhow` is used only in `vwap_cli`, at the top. The binary prints `error: ...` and exits 1, with no backtrace.
* Workspace lints deny `unwrap_used`, `expect_used`, `panic` and `indexing_slicing` outside test code. Numeric kernel modules that index equal length columns allow `indexing_slicing` with a comment saying why. `forbid(unsafe_code)` in every crate.
* Data problems are errors, never silent repairs. Anything repaired (duplicate rows dropped, bars flagged) is counted and written to the stage report.
* Missing feature values are `f64::NAN`, counted per feature.
* Config validation runs at load and rejects inconsistent values.

## Determinism

* Seeds come from config. Randomness uses `rand_chacha::ChaCha8Rng` only.
* LightGBM: `deterministic=true`, `force_row_wise=true`, one thread per model, all seed parameters set from the config seed.
* rayon is used only across independent permutations, with indexed collection that preserves order. No parallel floating point reductions.
* `Cargo.lock` is committed. The toolchain is pinned in `rust-toolchain.toml`. The raw data checksum is pinned in `config/data.toml`.

## Style

* Code reads like the surrounding code. Match comment density, naming and idiom.
* rustfmt defaults plus the repo `rustfmt.toml`. Clippy with warnings as errors.
* Docs: every formula in LaTeX. No em dashes, en dashes or hyphens in prose. Use numbered lists or asterisk bullets.
* Test builders live in `vwap_testkit`. Do not copy synthetic data builders into test files.

## How to add a feature

1. Write the exact formula and units in `docs/FEATURES.md` and the causal proof paragraph.
2. Implement it in `crates/vwap_features/src/features.rs` using only `CausalView`, with any parameter in `config/features.toml` and `FeaturesConfig`.
3. Append it to the end of `NAMES` (never insert, the column order is part of the golden fingerprint) and update `golden_feature_fingerprint` deliberately, with the reason in the commit message.
4. The `features_do_not_look_ahead` proptest covers it automatically because it iterates `feature_names()`.

## Where to read next

`README.md`, then `docs/ARCHITECTURE.md` and the stage doc. Decisions and results are in `docs/DECISIONS.md`. Symbols are in `docs/GLOSSARY.md`. The research protocol is `docs/PROTOCOL.md`. Operations and troubleshooting are in `docs/OPERATIONS.md`.
