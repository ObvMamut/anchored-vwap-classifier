# Architecture

This document maps the crates, the data flow, the public types and traits between crates, and the places where Rust is a weaker choice than Python for this work, with the cover for each. Symbols are defined in `GLOSSARY.md`.

## 1. Crate map

Dependency direction runs from the bottom of the list upward. A crate may depend only on crates above it in this list.

1. `vwap_core`: shared types (`Timeframe`, `Side`, `Class`, `Bars`, `SessionId`, `Event`, `LabeledEvent`), typed `Config` structs for every `config/*.toml`, error base types, tick and cost constants, seeds.
2. `vwap_data`: reads `data.parquet`, validates, builds the roll table, runs quality checks, writes cleaned parquet partitioned by year.
3. `vwap_events`: anchored VWAP, ATR, 5 and 15 minute bar sampling, touch detection and the re arm state machine. Exposes `LevelSource` and `EventDetector`.
4. `vwap_labels`: triple barrier labels, ambiguity resolution on 1 minute bars, time to barrier, MAE. Exposes `Labeler`.
5. `vwap_features`: feature functions behind the `Feature` trait, `CausalView`, the feature registry, the lookahead proptests.
6. `vwap_baselines`: control `LevelSource` implementations (random horizontal levels, time shifted VWAP), class frequency tables, confidence intervals, the gate tests. Reuses events and labels through traits, so controls run the same pipeline.
7. `vwap_validation`: `Splitter` (purged walk forward), holdout lock and `DevData` view, experiment log, Deflated Sharpe Ratio, Probability of Backtest Overfitting.
8. `vwap_model`: majority class, multinomial logistic regression, LightGBM multiclass, isotonic and Platt calibration, metrics, threshold tuning, explainability.
9. `vwap_backtest`: trade simulation with next bar open entry, barrier exits, costs, metrics, cost sweep.
10. `vwap_report`: plotters charts, spot checks, the Markdown report.
11. `vwap_research`: the closed loop harness of `PROTOCOL.md`: split views, hypothesis cards, the append only ledger with budget enforcement, subset contrast runners and reports. Depends on `vwap_baselines` for the statistics and controls.
12. `vwap_cli`: the binary `vwap`. It wires stages and holds the only `anyhow` usage.

`vwap_model` depends on `vwap_validation` for splits and on `vwap_core` for the cost constants. The threshold tuner needs a per event net outcome in ticks. That function (`net_ticks_proxy`) lives in `vwap_core` next to the cost model, so `vwap_model` does not depend on `vwap_backtest`.

## 2. Data flow from raw data to final report

```
data.parquet (raw 1 minute, UTC, raw front month)
  => vwap_data     cleaned 1 minute bars, data/clean/year=YYYY/, roll_table, quality_report
  => vwap_events   anchored VWAP series, ATR, 5 and 15 minute bars, touch events
  => vwap_labels   labeled events (class, time to barrier, MAE, ambiguity flag)
  => vwap_baselines  control events and labels, class frequencies, GATE report
  => vwap_features feature matrix aligned to labeled events
  => vwap_validation  purged walk forward splits, DevData view, experiment log
  => vwap_model    per fold: fit, calibrate, tune tau, predict, contributions
  => vwap_backtest trades, equity, cost sweep
  => vwap_report   charts and report.  Holdout path runs once at the end.
```

Every arrow is a CLI stage. Each stage writes parquet or JSON under `artifacts/<stage>/` plus a manifest holding input hashes, config hash and git SHA. A stage refuses to run when a manifest hash does not match its inputs.

## 3. Public types

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
   * Cover: LightGBM `pred_contrib` returns exact TreeSHAP values, shape $n \times K(F+1)$ for $K$ classes and $F$ features, last entry per class being the bias. A property test checks additivity: $\phi_0 + \sum_f \phi_f$ equals the raw score for every row and class. Reports show mean $|\phi|$ per class, per fold stability, and a hand drawn beeswarm. Logistic regression is explained exactly by coefficient times standardised value. If `lightgbm3` does not expose `pred_contrib`, the call goes through the C API with `C_API_PREDICT_CONTRIB`. This is verified in the Phase 1 spike.
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
