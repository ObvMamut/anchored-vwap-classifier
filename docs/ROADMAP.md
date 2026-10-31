# Roadmap

Status: Phases 1 and 2 are done. Phase 3 as first specified ran and its marginal gate failed (see `DECISIONS.md`, section 5). The owner approved a redesign: gate 3 is now two tiers (3a equivalence verdict, 3b conditional screen) and the later phases run as a closed research loop under `PROTOCOL.md`, with a three way data split (discovery, confirmation, holdout).

Seven phases. Each has deliverables, tests, an exit criterion and a go or no go decision. A gate failure stops the project at that phase and produces a written report. Phases do not overlap: a phase starts only after the previous gate decision is recorded in `DECISIONS.md`.

No Rust code exists yet. Code starts only when the owner says so.

## Phase 1: Data pipeline plus tests

Crates: `vwap_core`, `vwap_data`, the first part of `vwap_cli`.

Deliverables:

1. Cargo workspace skeleton, typed config for `data.toml`, error types, CI style commands (`cargo test`, clippy with `-D warnings`, `cargo fmt --check`).
2. Loader and cleaner for `data.parquet` as specified in `DATA.md`; cleaned yearly parquet; roll table; session table; quality report.
3. **Spike A, LightGBM**: build `lightgbm3` in this environment (cmake and clang are present), train a tiny multiclass model, confirm determinism, and confirm whether `pred_contrib` is exposed. If not, prove the C API fallback through `lightgbm3-sys`.
4. **Spike B, linfa**: confirm the multinomial logistic regression API and its penalty options.
5. **Bar timestamp convention check** (open versus close) and **roll offset diagnostic** reported to the owner.

Tests: everything under "Tests" in `DATA.md`.

Exit criteria: all quality checks run; no unexplained gap or outlier remains unreviewed; prices unchanged versus raw; the roll table is confirmed by the owner; both spikes pass or have a documented fallback; bar label convention confirmed by data.

**Go or no go**: go when the above hold. No go if the series cannot be confirmed as raw front month with identifiable rolls, or the bar timestamp convention cannot be determined.

## Phase 2: VWAP and touch detection

Crate: `vwap_events`.

Deliverables: anchored VWAP, ATR, 5 and 15 minute bars, touch detection with re arm and exclusion, events parquet, `vwap spotcheck` charts.

Tests: all of `EVENTS.md` section 10, including the lookahead property tests.

Exit criteria: visual check of **20 random days** by the owner, using the checklist in `EVENTS.md` section 9; event counts per year and timeframe reported and plausible; drop reasons counted.

**Go or no go**: go after the owner signs off the visual check. No go while any chart shows a rule violation.

## Phase 3: Labels plus base rates versus controls. GATE (two tiers: 3a equivalence verdict, 3b conditional screen)

Crates: `vwap_labels`, `vwap_baselines`.

Deliverables: labeled events, label report, controls, class frequencies with clustered confidence intervals, permutation tests, `reports/gate_baselines.md`.

Tests: all of `LABELING.md` section 7 and `BASELINES.md` section 6.

Exit criteria: the primary label cell was committed to config before this phase produced any outcome numbers; ambiguity share within the limit.

**GATE**: the pre registered rule in `BASELINES.md` section 5 (Holm adjusted $p < 0.05$ against every control family in at least one timeframe, rejection frequency gap above $\delta_{\min}$, consistent sign). **Fail means stop** and document. No search over other cells to rescue it.

## Phase 4: Features plus lookahead tests

Crate: `vwap_features`. Also the `vwap calendar fetch` stage.

Deliverables: all features in `FEATURES.md`, feature matrix, NaN report, frozen calendar snapshot (if reachable) or calendar features OFF.

Tests: a named lookahead proptest for every feature plus the global replacement test; unit tests against hand computed values; the compile fail test for `CausalView`.

Exit criteria: lookahead matrix fully green; NaN rates understood; calendar snapshot validated or the feature switched off with a note; verify that the network policy of the environment allows the official hosts before relying on the fetch.

**Go or no go**: go when every feature has a passing proof and test. No go for any feature without one: it is removed, not waived.

## Phase 5: Logistic regression and LightGBM under walk forward validation. GATE

Crates: `vwap_validation`, `vwap_model`.

Deliverables: purged walk forward splits, nested cross fitted scheme, majority, logistic and LightGBM predictions, calibrators, $\tau$ per fold, reliability diagrams, Brier and log loss tables, TreeSHAP summaries, fold stability, experiment log.

Tests: `VALIDATION.md` section 8 for splits, PAV, Brier; `MODELING.md` section 8.

Exit criteria: the experiment log exists and counts trials; no result uses holdout rows (`DevData` only).

**GATE**: calibrated lift over base rates as defined in `MODELING.md` section 6 ($\mathrm{BSS} > 0$ with interval excluding zero, precision at $\tau^*$ above base rate, $\mathrm{ECE}$ within limit, in a majority of folds). **Fail means stop** and document.

Neural networks start only if LightGBM passes this gate robustly.

## Phase 6: Cost aware backtest. GATE

Crate: `vwap_backtest`.

Deliverables: trade log, equity curves, metrics, cost sweep, DSR and PBO with the true trial count.

Tests: `BACKTEST.md` section 8 and the DSR and PBO tests.

**GATE**: `BACKTEST.md` section 6: positive net expectancy after costs in most folds, lower bound above zero, DSR and PBO within limits, and a stated break even slippage. **Fail means stop**; the holdout is not run.

## Phase 7: Single holdout run and written report

Crates: `vwap_validation` (holdout), `vwap_backtest`, `vwap_report`.

Deliverables: freeze document (all choices), holdout lock, one holdout run, `reports/final_report.md` with all plots and tables, including failures and the trial count.

Exit criteria: the lock file exists and the run happened once. The report states every deviation from the protocol.

**Go or no go**: the result is reported as it comes out. There is no second run.

## Phase R: the closed research loop (replaces Phases 4 to 6 as a sequence)

Phases 4 (features), 5 (models) and 6 (backtest) now run inside the loop of `PROTOCOL.md`. Features are written one at a time because a card needs them, each with its lookahead proptest. The omnibus screen (cycle 0) comes first. Each targeted card ends in PROMOTE, REFINE or DROP. A PROMOTE is a checkpoint: the owner is asked before the card is run once on the confirmation segment. The cost aware backtest and the holdout run (Phase 7) apply only to a card that survives confirmation. If the budget is spent with nothing promoted, the project stops with a negative result.

## Risks and mitigations

1. **Few events.** A 3 year training window may hold only a few thousand events. Mitigation: nested cross fitting, Platt default for small samples, small model grids, session clustered intervals, and honest reporting of wide intervals.
2. **Roll contamination.** Looking back across a roll mixes two contracts. Mitigation: roll table, `roll_contaminated` exclusion, true range reset.
3. **Self reference of VWAP.** Bar $t$ contributes to its own VWAP. Mitigation: the `prior_bar_close` sensitivity run and the time shifted controls.
4. **Bar timestamp convention.** A wrong open versus close assumption would be an invisible lookahead. Mitigation: mandatory diagnostic in Phase 1.
5. **Trial inflation.** Many label cells and models raise the odds of a lucky winner. Mitigation: pre registered primary cell, full trial log, DSR and PBO.
6. **Toolchain.** LightGBM needs cmake, a C++ compiler and OpenMP; its binding may not expose everything. Mitigation: Spike A in Phase 1 and the C API fallback.
7. **Calendar fetch.** Official sites change formats or may be blocked. Mitigation: separate stage, frozen snapshot with checksum, fail closed, feature OFF by default.
8. **Dependence of events.** Touches cluster inside sessions. Mitigation: session level resampling for every interval and test.

## Stop rule

If any gate fails, stop at that phase. Write the gate report with the numbers, say plainly that the hypothesis is not supported at that stage, and do not continue to later phases. Re opening a study after a failed gate needs an explicit entry in `DECISIONS.md` with the added trial count, and never reuses the holdout.
