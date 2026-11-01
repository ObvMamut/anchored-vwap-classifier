# Architecture

This document maps the crates, the data flow, the public types and traits between crates, and the places where Rust is a weaker choice than Python for this work, with the cover for each. Symbols are defined in `GLOSSARY.md`.

## 1. Crate map

Dependency direction runs from the bottom of the list upward. A crate depends only on crates above it in this list.

1. `vwap_core`: typed, validated configuration for every `config/*.toml`, the small shared enums (`Timeframe`, `Side`, `Class`), tick and cost constants, error types and stage manifests (`manifest.rs`: SHA256 helpers and the `Manifest` written by every stage).
2. `vwap_data`: reads the raw parquet, converts timestamps, sorts, de duplicates, runs the quality checks and the roll diagnostic, writes cleaned parquet partitioned by year, and computes the `clean_fingerprint` that later stages verify.
3. `vwap_events`: anchored VWAP, ATR, 5 and 15 minute bar sampling, touch detection and the re arm state machine (`detect_level` works on any level series, which is how controls reuse it). Exposes the `LevelSource` trait. A small `fixture` module builds synthetic bars for the tests of later stages.
4. `vwap_labels`: triple barrier labels, ambiguity resolution on 1 minute bars, time to barrier, MAE, parquet output. `scan_barriers` is the one barrier scan shared with the backtest.
5. `vwap_features`: 34 feature columns behind the `CausalView`, which cannot read any bar after the touch, plus the prior day and overnight level table and the calendar table.
6. `vwap_dataset`: the joined sample table (event, label, features, barrier prices) and the parquet export written by `vwap features build`.
7. `vwap_baselines`: control `LevelSource` implementations (random horizontal levels, shifted anchor VWAP, previous session VWAP), class frequencies, the paired session bootstrap, the Wald test, Holm, and the gate 3a equivalence verdicts.
8. `vwap_validation`: purged walk forward splits, blocked inner splits, the cluster bootstrap, Sharpe moments, the Deflated Sharpe Ratio, the Probability of Backtest Overfitting, and the normal and chi square maths. It knows nothing about NQ or the data layout.
9. `vwap_model`: metrics, isotonic and Platt calibration, the majority, logistic and LightGBM models, the nested cross fitted scheme, the threshold rule, multiclass TreeSHAP and the importance summaries.
10. `vwap_backtest`: trade simulation with next open entry, barrier exits and costs, position policies, metrics, the cost sweep and the break even slippage.
11. `vwap_calendar`: the frozen snapshot of scheduled CPI, NFP and FOMC releases: a `Fetcher` trait with an HTTP and a fixture implementation, parsers, validation, `events.csv` with a `lock.json` hash. The only crate that may use the network.
12. `vwap_research`: the closed research loop of `PROTOCOL.md`: split views with permits, hypothesis cards, the append only ledger with budget enforcement, the subset contrast runner and verdict rules, the omnibus screen, confirmation, the holdout run, `verify` and `replay`; and the strategy cards: walk forward evaluation, gates, DSR and PBO bookkeeping, tracked summaries, confirmation and holdout for strategies.
13. `vwap_report`: plotters charts (the candlestick spot check, the reliability diagram, equity and drawdown, importance bars, the beeswarm). Depends on `vwap_events`.
14. `vwap_cli`: the `vwap` binary, split into `ctx` (git, the clean data check, manifests, the calendar table), `stages`, `research`, `report`, `calendar` and `ops` (doctor, pipeline). It holds the only `anyhow` usage.
15. `vwap_testkit`: synthetic sessions and test configurations shared by the test suites. Used only as a dev dependency.

## 2. Data flow from raw data to final report

```
data.parquet (raw 1 minute bars, broker server time, tick volume)
  => vwap_data       cleaned 1 minute bars, data/clean/year=YYYY/, roll_table, quality_report
  => vwap_events     anchored VWAP series, ATR, 5 and 15 minute bars, touch events
  => vwap_labels     labeled events (class, time to barrier, MAE, ambiguity flag)
  => vwap_baselines  control events and labels, class frequencies, GATE report
  => vwap_features   feature matrix aligned to labeled events (calendar columns from the snapshot)
  => vwap_dataset    the joined sample table, artifacts/features/features_{5,15}m.parquet
  => vwap_research   split views, ledger, hypothesis cards, omnibus, confirm, holdout
  => vwap_validation purged walk forward folds, inner blocks
  => vwap_model      per fold: nested fit, calibrate, tune tau, predict, explain
  => vwap_backtest   trades, daily PnL, cost sweep
  => vwap_research   gates, DSR and PBO, ledger entry, research/strategies/<id>_<w>m.json
  => vwap_report     reports/summary.md and the plots.  Holdout path runs once at the end.
```

Every arrow is a CLI stage. Each stage writes parquet or JSON under `artifacts/<stage>/` plus a manifest holding input hashes, config hash and git SHA, except the research stages, whose record is the tracked ledger and the tracked strategy summaries. A stage refuses to run when a manifest hash does not match its inputs.

## 3. Public types

The names below are the design vocabulary. The code differs in places: `Sample` (in `vwap_dataset`) is the joined event, label and feature row; `FeatureMatrix` is `Matrix` in `vwap_model`; `ProbMatrix` is `Vec<[f64; 3]>`; `TradePolicy` is the enum `PositionPolicy` in `vwap_core`; splitting is done by functions of `vwap_validation` rather than a `Splitter` trait.

Shown as signatures only (documentation, not code to compile).

* `Timeframe`: enum `{M1, M5, M15}` with `minutes() -> u32`.
* `Side`: enum `{Above, Below}` with `sign() -> i8`, so $s_t = +1$ for `Above` (price came from above).
* `Class`: enum `{Rejection, Breakthrough, Chop}` with a fixed index order 0, 1, 2 used by every model.
* `Bars`: columnar struct of `Vec<i64>` open times in UTC microseconds, `Vec<f64>` for open, high, low, close, volume, tick volume, plus the `Timeframe`. Constructed only by `vwap_data` and `vwap_events`, never mutated after.
* `SessionId`: the New York date of the anchor instant, plus the anchor kind.
* `Event`: id, session, timeframe, touch bar index, close time, `Side`, VWAP, $\sigma_t$, flags (`roll_contaminated`, `horizon_fits`).
* `LabeledEvent`: an `Event` plus class, barrier time, MAE, return at horizon in ticks, `ambiguous`.
* `FeatureMatrix`: row major `f64` matrix, column names, per column NaN counts.
* `ProbMatrix`: $n \times 3$ probabilities whose rows sum to 1 within $10^{-12}$.
* `Trade`: event id, direction, entry and exit times and prices, exit reason, net ticks, net USD.
* `CostModel`: tick size, tick value, slippage ticks per side, commission USD per side.
* `Config`: aggregate of the typed sections, validated at load.

## 4. Public traits

* `LevelSource`: `fn level_at(&self, bars: &Bars, i: usize) -> Option<f64>`. Implemented by `AnchoredVwap`, `RandomHorizontal`, `ShiftedAnchorVwap`, `PriorSessionVwapCurve`. This is how controls reuse the exact same detector and labeler.
* `EventDetector`: `fn detect(&self, bars: &Bars, level: &dyn LevelSource) -> Result<Vec<Event>, EventsError>`.
* `Labeler`: `fn label(&self, events: &[Event], bars: &Bars, base: &Bars) -> Result<Vec<LabeledEvent>, LabelError>` where `base` is the 1 minute data used to resolve double hits.
* `Feature`: `const NAME: &'static str; fn compute(&self, view: &CausalView<'_>) -> f64`. `CausalView` exposes only bars up to and including the touch bar and the session state built from them.
* `Classifier`: `fn fit(&mut self, x: &FeatureMatrix, y: &[Class]) -> Result<(), ModelError>` and `fn predict_proba(&self, x: &FeatureMatrix) -> Result<ProbMatrix, ModelError>`.
* `Calibrator`: `fn fit(&mut self, p: &ProbMatrix, y: &[Class])` and `fn transform(&self, p: &ProbMatrix) -> ProbMatrix`.
* `Explainer`: `fn contributions(&self, x: &FeatureMatrix) -> Result<Contributions, ModelError>`, shaped $n \times K(F+1)$ for LightGBM.
* `Splitter`: `fn splits(&self, spans: &[EventSpan]) -> Vec<Split>` where each `Split` lists train indices, test indices, purged indices and the test window $[a_f, b_f)$.
* `TradePolicy`: decides whether a signal opens a trade given current position state. The default implementation is one position at a time.

## 5. Determinism design

* `rand_chacha::ChaCha8Rng` seeded from config; child seeds derived by hashing a stage name with the master seed so adding a stage does not shift others.
* LightGBM: `deterministic=true`, `force_row_wise=true`, fixed `num_threads`, and `seed`, `bagging_seed`, `feature_fraction_seed`, `data_random_seed` all set.
* rayon only across folds or across sessions. Results are collected with an indexed parallel iterator so output order equals input order. LightGBM runs single threaded inside each fold task. No parallel floating point reductions, because float addition is not associative.
* Polars `group_by` with `maintain_order`. Numeric kernels work on plain `Vec<f64>` columns.
* A determinism test runs the walk forward on a small fixture twice and compares outputs bit for bit.

## 6. Where Rust is weaker than Python, and the cover for each

1. **Model ecosystem.**
   * Gap: there is no sklearn equivalent. `linfa` offers multinomial logistic regression but with fewer solvers and penalties and less maturity. There is no off the shelf calibration, isotonic regression, scoring metrics, bootstrap, permutation test or multiple testing correction. There is no Optuna.
   * Cover: the following are hand written with unit tests against reference values: pool adjacent violators isotonic calibration, Platt scaling, log loss, Brier score, expected calibration error, session clustered bootstrap, permutation tests, Wilson intervals, Holm correction, Deflated Sharpe Ratio, Probability of Backtest Overfitting, purged walk forward splits. The normal CDF and quantile come from `statrs`. Hyperparameter search is a small pre registered grid or a seeded random search, and every candidate is logged as a trial.
   * LightGBM is a C library reached through the `lightgbm3` bindings. Building needs cmake, a C++ toolchain and OpenMP. Versions are pinned and the build steps are in the ROADMAP Phase 1 spike. If the binding lacks a needed call, `lightgbm3-sys` exposes the raw C API.
2. **Plotting.**
   * Gap: `plotters` is low level. There is no seaborn style layer, text rendering needs the font feature or the SVG backend, and there is no interactive exploration.
   * Cover: `vwap_report` has four plot functions (reliability diagram, equity and drawdown, candlestick spot check with VWAP and touches, importance bars with fold spread). Each plot is produced from a plain data struct built by a pure function, and those functions are unit tested. Pixels are never tested. Spot checks are generated PNG files, which also replace notebook exploration.
3. **SHAP.**
   * Gap: there is no `shap` package, no beeswarm, no dependence plots, no interaction values.
   * Cover: `crates/vwap_model/src/shap.rs` implements exact path dependent TreeSHAP in safe Rust from the LightGBM text model. `lightgbm3` 1.0.11 sizes the multiclass `predict_contrib` buffer as $n(F+1)$ instead of $nK(F+1)$, so its own call cannot be used for three classes. The implementation is checked against `predict_contrib` on binary models (agreement to $10^{-9}$, missing values included), and on multiclass models $\phi_0 + \sum_f \phi_f$ equals the raw score of every class. Reports show mean $|\phi|$ per class, the stability across folds, and a hand drawn beeswarm. Logistic regression is explained exactly by coefficient times standardised value.
4. **Data wrangling.**
   * Gap: the polars Rust API is more verbose than pandas and the lazy API changes between versions.
   * Cover: pin the polars version, keep polars at the input and output edges, and run numeric kernels on `Vec<f64>`.
5. **No notebooks.** Cover: spot check charts, stage reports in JSON and the Markdown report.
6. **Slower iteration.** Compile times are longer than a Python edit and run cycle. Cover: a small fixture dataset checked into `crates/*/tests/fixtures` and release mode integration tests.

## 7. Testing strategy

* Unit tests with hand computed reference values for all hand written maths.
* Proptests for lookahead on every feature and label function, for VWAP, ATR, touch detection, purged splits and the isotonic fit (monotone output, mean preserved).
* Integration test on a synthetic day with a known touch, a known label and a known trade.
* Determinism test (section 5).
* `cargo clippy --workspace --all-targets --all-features -- -D warnings` and `cargo fmt --all --check` gate every commit.
