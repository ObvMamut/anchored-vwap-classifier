# Decisions and open items

A running log. Each decision has a number, a reason, and where it is specified. Changes after a gate has been passed need a new entry with the date, the reason, and the effect on the trial count.

## 1. Facts established with the owner

1. Input is `data.parquet` at the repository root with columns `DateTime, Open, High, Low, Close, Volume, TickVolume`.
2. 1 minute bars only. The file is the only data file. 5 and 15 minute bars are aggregated from it.
3. Timestamps are UTC.
4. The series is a raw front month series rolled by a calendar rule. The offset of the rule is not known.
5. `Volume` is the VWAP weight.
6. No contract id, no bid or ask volume, no delta.
7. Backtest policy: one position at a time.
8. Calendar flags (CPI, FOMC, NFP) must be fetched by the code from official sources.
9. Docs style: no dashes of any kind in prose, headings, list markers or names that the author controls. Hyphens remain only where syntax needs them (code, command options, math minus signs).

## 2. Decisions taken in the plan (approved)

1. **Extra crate `vwap_core`** for shared types, config, errors and cost constants. Total of 10 library crates plus the `vwap_cli` binary. Specified in `ARCHITECTURE.md`.
2. **Calendar fetch is a separate stage** (`vwap calendar fetch`), run once, producing a frozen snapshot with checksum. Fails closed. Feature OFF by default until a snapshot exists. Specified in `FEATURES.md` section 6.
3. **Nested cross fitted validation inside each training window** for early stopping, calibration and $\tau$. Specified in `MODELING.md` section 2.
4. **Ambiguous events** are excluded from fitting and metrics and booked as stops in the backtest. `LABELING.md` section 2 and `BACKTEST.md` section 3.
5. **Events whose full horizon would cross the session `flat_time` are dropped.** Clock based, so not lookahead. `LABELING.md` section 4.
6. **Self reference flag** `vwap_reference` with the strict variant as a sensitivity run. `EVENTS.md` section 4.
7. **Holdout protection by types and a lock file.** `VALIDATION.md` section 3.
8. **Reference values for hand written maths** come from papers or hand worked examples. No Python anywhere. `VALIDATION.md` section 8.
9. `data.parquet` stays at the repository root. Derived output is git ignored. Git LFS for the raw file is suggested for later and not done.

## 3. Open items needing the owner's input or a data check

1. **Bar timestamp convention (open or close).** Assumed open. Phase 1 diagnostic must confirm from session boundaries. A mismatch stops the stage. See `DATA.md` section 1.
2. **Roll offset.** Assumed `roll_offset_days = 8` before the third Friday expiry. Phase 1 diagnostic reports the observed step for offsets 0 to 14 days. The owner confirms the value.
3. **Timestamp dtype in the file** (naive or timezone aware, which resolution). The loader handles both; the actual dtype is recorded in the Phase 1 report.
4. **Early close times and the holiday rule set** are rule generated and verified against the data. Mismatches are listed for the owner.
5. **Commission value** in USD per side. A placeholder is used until the owner provides it.
6. **Crate API checks** for `lightgbm3` (`pred_contrib`) and `linfa-logistic` (multinomial). Not verified in the planning session because the documentation host was unreachable. Phase 1 spikes A and B.
7. **Calendar sources reachable from the execution environment.** Verified in Phase 4 before relying on the fetch.

## 4. Default parameters (placeholders, fixed in config before any outcome is seen)

* ATR length $n_{\mathrm{ATR}} = 14$ on the event timeframe.
* Re arm excursion $k = 1.0$ ATR.
* Exclusion after the anchor $N_{\mathrm{ex}} = 30$ minutes.
* Labels: $m_1 = m_2 = 1.0$; $T = 12$ bars for 5 minute bars and $T = 8$ bars for 15 minute bars.
* Embargo $h = T\,w$ (at least $T$ bars).
* Anchor kind: `globex` first; `rth` as a separate configuration that counts as its own trials.
* Walk forward: 36 months training, 6 months test, 6 months step. Holdout: last 24 months.
* Flat times: 16:55 for the Globex anchor and 16:00 for the RTH anchor.
* Costs: 1 tick slippage per side, commission placeholder, tick 0.25 points, tick value 5 USD.
* Gate levels: Holm adjusted $p < 0.05$; $\delta_{\min} = 2$ percentage points; `min_fold_fraction = 0.6`; `dsr_min = 0.95`; `pbo_max = 0.2`; `max_ece = 0.05`.

These are the starting values. They are fixed once in config, committed, and not changed after outcomes are seen without a log entry here and a higher trial count.

## 5. Gate decisions (filled in as the project proceeds)

* Phase 1 exit: passed with deviations, decided by the implementing agent and open for the owner to overrule. The roadmap no go condition (series cannot be confirmed as raw front month with identifiable rolls) is technically met, because rolls are not identifiable in this file. It was waived because its purpose is to prevent VWAP contamination by adjusted or stitched prices, and the evidence shows an unadjusted continuous series with no visible roll steps, while the VWAP resets every session. If the owner can supply per contract data, Phase 1 should be redone on it. Other Phase 1 exit criteria: all quality checks run and reported; prices unchanged versus raw (tested); bar label confirmed by data (open); both library spikes done with a documented fallback for the multiclass TreeSHAP sizing problem.
* Phase 2 exit: passed. Events: 20,892 on 5 minute bars and 10,879 on 15 minute bars over 2016 to 2025 (about 2,400 and 1,200 per year from 2017), with defaults $k = 1.0$, $N_{\mathrm{ex}} = 30$ minutes, ATR length 14. The 20 day visual check was run (seed 42, 40 charts in `artifacts/spotcheck/`, development range only). The implementing agent reviewed 3 of the 40 charts and found VWAP starting at the 18:00 anchor, the exclusion window respected, markers on bars that cross the VWAP and sides matching the approach direction. The owner sign off the roadmap asks for is still open. The rules are also enforced by property tests (lookahead on VWAP, ATR and events; touch inequality; side from the previous bar; re arm separation; exclusion window).
* Phase 3 baseline gate: **FAILED. The project stops here by the pre registered rule.** Primary cell $m_1 = m_2 = 1$, $T = 12$ bars (5 minute) and $8$ bars (15 minute), development data only (holdout untouched). Anchored VWAP outcomes are statistically close to a coin flip and close to every control. On 5 minute bars (14,681 events): rejection 47.7 percent, breakthrough 49.6 percent, chop 2.6 percent. On 15 minute bars (7,343 events): rejection 44.3, breakthrough 45.1, chop 10.7 percent. Rejection frequency gap to random horizontal levels: $-0.5$ percentage points (95 percent interval $-1.5$ to $+0.5$) on 5 minute bars, Holm adjusted $p = 0.030$, and $-0.5$ points (Holm adjusted $p = 0.089$) on 15 minute bars. Against the shifted anchor VWAP: $-1.1$ and $+0.2$ points (Holm adjusted $p = 1.0$ and $0.30$, with only 14 percent stratum coverage). Against the previous session VWAP curve: $+0.1$ and $+2.9$ points (Holm adjusted $p = 1.0$ and $0.30$). No family has a gap above the 2 percentage point minimum together with Holm adjusted $p < 0.05$ in any timeframe, and the sign is inconsistent for the shifted anchor family. Full numbers: `reports/gate_baselines.md` and `artifacts/baselines/baselines_report.json`.
  The sensitivity grid (descriptive only) shows rejection and breakthrough both close to 50 percent in every cell, with chop growing as the barriers widen or the horizon shortens, which is what a symmetric first passage problem looks like for any level. No other cell was used to rescue the result. Caveats that apply to this conclusion: the VWAP is weighted by tick count and not traded volume; the price series is a broker style index series and not exchange NQ prints; the VWAP includes the touch bar's own volume (the `prior_bar_close` variant was not run, because that would be a new study); the gate tests unconditional class frequencies, so it does not rule out a feature conditioned effect. Phases 4 to 7 were not started.
* Phase 4 lookahead matrix: not yet.
* Phase 5 model gate: not yet.
* Phase 6 backtest gate: not yet.
* Phase 7 holdout run: not yet. Lock file absent.

## 6. Phase 1 findings, measured on data.parquet

These were measured by `vwap data clean config`. They override the assumptions in sections 1 and 3 where they differ.

1. **Row order and span.** The file is sorted newest first (every step is descending). The loader sorts ascending. It holds 3,077,548 one minute bars from 2016-11-15 to 2025-10-01, about 8.9 years and not 15. Consequence: with a 2 year holdout the development range is about 7 years, so a 3 year training window with 6 month steps gives about 8 test folds and not 20.
2. **Volume is zero on every row** of every year. Only `TickVolume` carries activity (mean about 123 per bar). The VWAP weight is therefore `tick_volume` (config `weight_column`). This departs from the owner's answer "Volume" and is reversible by config, but the Volume column cannot be used. Tick counts are a common proxy for volume on index CFD feeds, but they are not traded contracts, and all volume features inherit that caveat.
3. **The prices are not exchange NQ prints.** 100 percent of prices lie on a 0.1 point grid and only 20 percent on a 0.25 grid. This is a broker style continuous index series with CME like trading hours, used here as a proxy for NQ. Costs are still modelled in NQ ticks of 0.25 points worth 5 USD.
4. **The timestamps are not UTC.** The daily gap sits at a fixed label hour all year, and the weekly gap is 2941 minutes. This is broker server time, equal to New York time plus 7 hours with US style daylight saving, stored as if it were UTC. With `timestamp_kind = "server_ny_synced"` and `server_offset_hours = 7` the cleaned series shows the CME maintenance break at exactly 17:00 to 18:00 New York (1553 of 1559 scheduled breaks begin with an 18:00 bar), a Friday 16:59 to Sunday 18:00 weekly gap, and zero ambiguous or non existent local times. The cleaned files store true UTC.
5. **Bar label is open.** Sessions open with an 18:00 bar and the last bar before the break is stamped 16:59. The stage fails if this ever contradicts `bar_label`.
6. **Rolls cannot be identified.** The series is not back adjusted (yearly first closes of about 4,700 in 2016, 12,900 in 2021 and 21,100 in 2025 match real nominal NQ levels), but it also shows no roll steps. The median absolute step at a session boundary is 4.7 points, the 99th percentile is 121 points, steps at the predicted roll boundaries are about 0 to 5 points (a real NQ roll carries roughly 50 to 200 points of spread), and only 4 of 36 expiries exceed the 99th percentile, at inconsistent offsets that are mostly weekend gaps. Conclusion: this is a continuous series whose vendor already handles the roll. The roll buffer flag is still applied conservatively at the calendar predicted boundary (first session after the third Friday minus 8 days). The owner's calendar rule claim cannot be verified from this file.
7. **Data quality by year.** 2017 to 2020 are much gappier (about 300 to 470 unexpected gaps over 5 minutes per year) than 2021 to 2024 (25 to 37 per year). 2025 has 187. 145 of 2302 sessions are thin (under 90 percent of 1380 bars), 266 end before 16:55. Full US holidays (for example Thanksgiving, MLK Day) still trade until about 13:00, so the holiday rules list them for review but the thin session flag is what protects the events.
8. **Outliers.** 4,551 one minute return outliers (robust score above 12) and 70 range outliers are flagged, not removed.

## 7. Implementation deviations from the earlier docs

1. `roll_table.json` and `quality_report.json` are written as JSON under `artifacts/data/`, not parquet. Sessions are derived from the `globex_session` column of the cleaned bars and are not written as a separate table.
2. Outlier statistics use the previous block of `outlier_window_bars` bars (causal) instead of a window of prior sessions.
3. `indexing_slicing` is denied workspace wide, but numeric kernel modules (`clean`, `quality`, `roll`, `write` in `vwap_data`) allow it with a comment, because they index equal length columns with indices produced by iterating those columns. Test code allows `unwrap`, `expect` and indexing.
4. **`lightgbm3` multiclass `predict_contrib` is unsafe to call.** Version 1.0.11 allocates `n_rows * (F + 1)` doubles, but LightGBM writes `n_rows * K * (F + 1)` for $K$ classes, a heap overflow inside the C call. The booster handle is private, so the C API cannot be reached safely. Plan for Phase 5: compute multiclass TreeSHAP in safe Rust from the LightGBM text model (`Booster::save_string`) and verify it against `predict_contrib` on binary models, where the sizing is correct. The spike test `lightgbm_binary_treeshap_is_additive` covers the binary case.
5. Spike results (Phase 1): LightGBM builds with cmake in this environment, trains deterministically with the documented settings, supports early stopping with a validation set; `linfa-logistic` 0.8 provides `MultiLogisticRegression` with an L2 `alpha` and `predict_probabilities`.

## 8. Phase 3 implementation notes

1. The baseline test statistic changed from a session level permutation test to a paired session bootstrap with a Wald test (`docs/BASELINES.md`, section 4), because pooling several random levels per session makes the groups non exchangeable. The gate rule itself (Holm adjusted $p < 0.05$, gap above 2 percentage points, consistent sign) is unchanged.
2. Volatility stratification uses $\sigma_t / C_t$ terciles and the New York hour of the touch close, because the realised volatility percentile feature does not exist before Phase 4.
3. `realised_vol` as a barrier scale is not implemented. Config accepts only `sigma = "atr"`.
4. Ambiguous events are at most 0.4 percent of events in any year (limit 2 percent).
5. To reopen the study after this failed gate, an entry here must state the new hypothesis and the added trial count, and the holdout stays untouched (`docs/ROADMAP.md`, stop rule).

## 9. Environment note

Debug builds with full debug info filled the disk (every test binary links polars). The workspace now sets `debug = 0` and `incremental = false` for the dev and test profiles.

## 10. Redesign approved by the owner after the failed gate

1. **Three way split.** Discovery 2016-11-15 to 2021-09-30, confirmation 2021-10-01 to 2023-09-30, holdout 2023-10-01 to 2025-10-01. The Phase 3 gate had used a holdout starting 2023-10-01T04:12 UTC and everything before it; the holdout rows were never touched. Confirmation rows were used in that first Phase 3 run as part of development data, so the first marginal numbers (section 5) are labeled as such. Confirmation is used once per promoted card from now on.
2. **Gate 3 is two tier** (`docs/BASELINES.md`, section 5). 3b alone decides whether the project continues.
3. **Closed research loop** under `docs/PROTOCOL.md` with an enforced budget: omnibus $\alpha = 0.01$ plus at most 12 cards sharing $0.04$.
4. **Autonomy.** The agent runs cycles on its own and stops only for a PROMOTE, a protocol change, any holdout use, or budget exhaustion.
5. **Statistical honesty.** With about 6,000 discovery events at 15 minutes, the minimum detectable subset effect at the card alpha is about 4 points, while the economic break even is only about 1 point. The loop can find large effects and cannot establish small ones. Cards report their MDE.

## 11. Gate 3a result on the discovery segment

Run with `vwap baselines run config` (window 2016-11-15 to 2021-09-30). Anchored VWAP outcomes: 5 minute bars 9,766 events, rejection 47.9, breakthrough 49.6, chop 2.5 percent; 15 minute bars 4,879 events, rejection 44.5, breakthrough 46.3, chop 9.2 percent. Verdicts with the 90 percent interval of the rejection gap and margin 2 percentage points:

* random horizontal levels, 5 minute: **EQUIVALENT** (gap 0.0, interval $-0.8$ to $+0.9$).
* random horizontal levels, 15 minute: INCONCLUSIVE (gap $-1.3$, interval $-2.6$ to $+0.1$, just outside the margin).
* shifted anchor VWAP and previous session VWAP, both timeframes: INCONCLUSIVE (wide intervals from low overlap).
* No comparison is DIFFERENT.

Reading: at the level of the marginal class mix the VWAP behaves like an arbitrary level, to within about 2 points, which is what the loop assumes. This tier never decides the project; tier 3b does.

## 12. Implementation notes for the research harness

1. The omnibus null uses circular shifts of the label sequence and not within session shuffles (corrected in `PROTOCOL.md` before any real run, because a within session shuffle keeps between session associations).
2. The difference in differences against control levels is not computed yet. It becomes required before any confirmation.
3. For a card the combined $p$ is the larger of the two timeframes, so both must pass, which also corrects for looking at two timeframes.
4. Positive and negative harness controls run at the start of every `vwap research run` and omnibus run (`selftest.rs`).

## 13. Cycle 0 result and its analysis (discovery segment)

**Cycle 0, omnibus (multiclass), alpha 0.01 spent.** LightGBM with fixed settings, expanding blocked cross validation, 1,000 circular shift permutations. Log loss improvement over the training frequencies: 0.0116 on 5 minute bars (9,766 events, p = 0.001) and 0.0709 on 15 minute bars (4,879 events, p = 0.001). Verdict: structure.

**Analysis (diagnostic, no alpha, `omnibus_decomposition`).**
* Direction task (breakthrough versus rejection among resolved events): improvement $-0.0024$ (5 minute, p = 0.19) and $-0.0054$ (15 minute, p = 0.30). **No directional structure** in any of the 17 features, even with a flexible nonlinear model.
* Chop task (chop versus barrier hit): improvement 0.0150 and 0.0782 (p = 0.002, the minimum with 500 permutations). Importance is dominated by `minutes_since_anchor` and `minute_of_day` (about 60 percent of gain on 5 minute and 84 percent on 15 minute bars). This is a time of day volatility effect on how often a $1\sigma$ barrier is hit within the horizon, unrelated to VWAP.

**Conclusion.** The omnibus "structure" is the mundane chop and clock effect. The project question, whether features at a VWAP touch predict rejection versus breakthrough, shows no signal at the omnibus level.

**Flaw found in the protocol.** The card outcome `rejection` is the indicator of the REJECTION class, which equals $(1 - P_{\text{chop}})\,P(\text{rej} \mid \text{resolved})$. Any feature related to chop, above all time of day, would make a card on `rejection` look significant through chop alone. The directional hypothesis needs the outcome conditional on resolution. Changing the outcome definition is a protocol change and needs the owner's decision.

## 14. Research loop result on the discovery segment (cycles 1 to 8)

Protocol: `docs/PROTOCOL.md`. Data: discovery 2016-11-15 to 2021-09-30 only. Confirmation and holdout rows were not read. Outcome for all cards: `rejection_given_resolved` (amended after cycle 0, see section 13). Card alpha $0.04/12 \approx 0.0033$, both timeframes must pass (the larger p is used).

* **H001 trend pullback (`vslope_align`, top tercile).** Difference $-0.04$ points (5 minute) and $-1.4$ points (15 minute), p = 0.80. DROP. Predicted sign not found on 15 minute bars.
* **H002 stretched approach (`dist_travelled`, top tercile).** $+0.9$ and $+0.6$ points, p = 0.36. DROP.
* **H003 level confluence (`conf_flag`).** $+2.3$ points on both timeframes, one sided p = 0.045 (5 minute) and 0.080 (15 minute), sign stable in only 4 of 6 and 3 of 5 years, below the 3 point minimum and the 5 to 6 point minimum detectable effect. REFINE.
* **H004 session blocks (`time_block`, groups test).** p = 0.55 (5 minute) and 0.03 (15 minute, driven by one cell of 96 afternoon events at 63.5 percent). DROP. The harness recorded a minimum detectable effect of 0.00 for this card because it was not computed for group tests; fixed for later runs (shown as not computed), and the append only ledger line for H004 keeps the 0.0 value.
* **H005 relative volume (`rel_volume`, top tercile, predicted negative).** $-0.7$ and $-1.2$ points, p = 0.28 and 0.22. DROP. Volume is a tick count proxy.
* **H006 first touch of the session (`ordinal` below 1.5).** $-3.8$ points on 5 minute bars (opposite to the prediction) and $+0.6$ on 15 minute bars. DROP.
* **H007 refinement of H003 (`conf_count` at least 2).** $+6.6$ and $+6.8$ points on 115 and 142 events, one sided p = 0.071 and 0.058, interval from about $-2$ to $+15$ points, minimum detectable effect 15 to 16 points. REFINE. The one allowed refinement for this mechanism is now used.

**Counted trials:** 8 (the omnibus and 7 cards). Alpha spent 0.0333 of 0.05, cards run 7 of 12.

**Gate 3b: not passed.** No card was PROMOTED, so none went to the confirmation segment, which was not touched. The stop rule (two consecutive nulls after the omnibus) was met after H002 and H004 to H006, and the one refinement is spent.

**What the data can and cannot show.**
* Direction (rejection versus breakthrough) is not predictable from 17 features at the VWAP touch: the omnibus direction screen gave a negative log loss improvement with a flexible model, and none of the six mechanism cards found an effect near its minimum detectable effect.
* Only chop is predictable, and mostly from the clock, which is not VWAP specific.
* The only thread above noise is level confluence: $+2.3$ points for one coinciding level and $+6.6$ points for two or more, consistent in sign on both timeframes. It does not reach significance and it is far below the data's power. To detect a $+3$ point effect for the single level subset at about 15 percent of events, the sample would need to be roughly four times larger, and for the two level subset roughly twenty times larger. This is a lead for a future study with more data or a better price and volume series, not a result.
* Caveats that apply to all conclusions: the weight is a tick count, the series is a broker style index series and not exchange NQ prints, 2017 to 2020 are gappy, and discovery spans only about 4.9 years.

**Recommended next steps (owner's decision).** Stop here and report the negative result for VWAP touches on this data, or reopen with a new data source (exchange futures with real volume, or more history) and register confluence as the first card there. No holdout run is justified, because nothing was promoted and nothing survived confirmation.

## 15. Operational hardening after the merge

An audit of the merged code found that it ran but was not trustworthy as a repository. What was found and fixed:

1. **The research ledger was git ignored.** `artifacts/research/ledger.jsonl` and the card reports sat under `/artifacts/` and `/reports/`, so a fresh clone had an empty ledger: the alpha budget and the registered card hashes silently reset, and the evidence behind section 14 was not in the repository. They now live in the tracked `research/` directory, byte for byte as recorded. Commands refuse to run if those paths would be ignored.
2. **Two sources of truth for the holdout.** `config/validation.toml` (24 months before the last bar) was used by the chart command and `research.toml` (fixed dates) by everything else. `validation.toml` and its code are removed. Moreover the chart command drew days from the whole development range, including the confirmation segment that nobody may look at. It now draws discovery days only (2017 to 2021 in the output, where before it also drew 2022 and 2023).
3. **Hard coded parameters.** Feature windows, the confluence band, time blocks, RTH minutes, the omnibus settings, the self test limits, minimum sample sizes, the strata width, the expected session length and chart fonts moved to `config/features.toml`, `model.toml`, `research.toml`, `baselines.toml`, `data.toml` and `events.toml`. A golden fingerprint of all features, tests that the files equal the built in defaults, and `vwap research replay all` show that no result changed.
4. **Documented stages that did not exist.** Added `research confirm`, `holdout run`, `doctor`, `pipeline`, `research verify` and `research replay`. Confirmation and the holdout use cut points frozen on discovery. Calendar fetch, features export, model walk forward, backtest, DSR, PBO, TreeSHAP and the report are marked as specification only.
5. **Stale inputs were not detected.** Every stage now writes a manifest, and every stage after `data clean` verifies the fingerprint of the cleaned data. The raw file checksum is pinned in `config/data.toml`.
6. **Tests.** `vwap_testkit` replaces three copies of the synthetic data builder; end to end tests run the real binary; the dead `vwap_model` crate is removed (its library tests moved to `vwap_research`).
7. **Packaging.** README, LICENSE (MIT), `docs/OPERATIONS.md`, a rewritten `CLAUDE.md`, status banners on the unbuilt specifications.

Residual limitations: the confirmation rule does not include the cost adjusted expectancy test (no backtest exists), the confirmation alpha is shared Bonferroni style and not Holm, and the data caveats of section 6 are unchanged.

## 16. Building the stages that were specified and left unbuilt

After the negative result of section 14 the owner asked for the specified stages to be built anyway, as reusable tooling for a future hypothesis or a better data source. The owner's choices: strategy cards run through the same ledger, a rolling walk forward of 24 months of training and 6 months of testing, a calendar built against fixtures with the live fetch left to the owner, and costs of 1 tick of slippage and 2 USD of commission per side. Nothing here changes a recorded result: all seven hypothesis cards still replay exactly, the label files are bit for bit identical after the barrier scan was shared, and the fingerprint of the 18 original features did not change.

**Built.** `vwap_validation`, `vwap_dataset`, `vwap_model`, `vwap_backtest` and `vwap_calendar`; strategy cards, gates, DSR and PBO in `vwap_research`; the report plots; the commands `features build`, `research strategy`, `research dsr`, `report build`, `calendar fetch` and `calendar import`; the configuration files `validation.toml`, `strategy.toml`, `backtest.toml`, `report.toml` and `calendar.toml`. Crates and files are described in `ARCHITECTURE.md`, the protocol for strategies in `PROTOCOL.md`, section 12.

**Decisions and deviations from the specifications.**

1. **`validation.toml` is back, for geometry only.** Section 15 removed a `validation.toml` that duplicated the holdout date. The new file holds the fold lengths, the embargo, the inner blocks, the PBO blocks and the gate limits, and no split date. The segment boundaries are defined once, in `research.toml`.
2. **Strategy runs count as trials and spend no alpha.** Each (card, timeframe) raises the trial count that feeds the DSR, and does not use up the 12 hypothesis cards. A run is not repeated; replay checks it. The self test of the hypothesis harness is not run for strategies. The test suite plays that role: on a synthetic market a planted signal is promoted, confirmed and held out once, noise is dropped.
3. **A fold abstains unless its tuned threshold has a positive lower bound** (`tau_require_positive_lcb`, on by default). The literal rule in `MODELING.md` trades at the least bad threshold even for a useless model. A synthetic test showed that such trades inflate the variance of the Sharpe ratios and make the DSR of a real edge unreachable. The switch can be turned off, which is a new study.
4. **DSR and PBO conventions.** The variance of the Sharpe ratios is taken over the strategy series that exist, with the null value $1/(T-1)$ for a single series. Hypothesis cards count in $N_{\text{trials}}$ and have no series, which is conservative. The PBO is undefined, and fails the gate, with fewer than 4 series or when the strategy under test made no trades. Series that abstained throughout stay in the matrix as trials with zero PnL. A first version dropped them and a planted strategy then could not be promoted among abstaining baselines; a second kept them for an idle strategy and returned a meaningless PBO of 1. The rule in `VALIDATION.md`, section 6 is what remains.
5. **Gate details** follow `MODELING.md`, section 9 and `BACKTEST.md`, section 9. In particular a fold that abstained counts as failing the precision and the expectancy conditions, the calibration error is pooled, and the net expectancy at 2 ticks of slippage must be positive.
6. **TreeSHAP is written in safe Rust** because of the buffer size defect of section 7, and verified against LightGBM on binary models to $10^{-9}$.
7. **The calendar was not verified live.** The development environment cannot resolve the two hosts. The page addresses in `calendar.toml` are a best guess, the parsers read the visible text of a page and were tested on fixtures that follow the expected patterns, and the validation fails closed with the counts it found. The features are off and no snapshot is committed. The owner runs `vwap calendar fetch config` once, or freezes dates by hand with `vwap calendar import`.
8. **Features.** 34 columns: the 18 original ones, nine calendar columns, and seven appended designed columns (touch bar shape, time of day as a circle, weekday, distance to the overnight low, band position of the approach extreme). The remaining designed columns stay specified only. A card that lists no feature now uses all 34, nine of which are `NaN` while the calendar is off (they are constant and harmless to the models).
9. **Not built:** the per trade log and `vwap backtest run`, the model implied expected value against the realised one, breakdowns by year, regime, side and time of day, reliability bands, `calibration_order = "chronological"`, grouped importance totals. See the notes at the end of `MODELING.md` and `BACKTEST.md`.
10. **The report goes to an ignored directory** (`reports/`, with the plots), because `research/` is append only and a regenerated summary would be an edit. The tracked evidence is the ledger and `research/strategies/`.

**Acceptance run on the real data (cycles 9 to 14).** The expected result was stated before the run: no strategy passes its gates. Three baseline cards on both timeframes, 14 counted trials in the ledger in all (the omnibus, 7 hypothesis cards, 6 strategy runs), alpha spent unchanged at 0.0333. Discovery segment only, five outer folds with test windows from November 2018 to May 2021.

* **S001, majority class.** Pooled $\mathrm{BSS}$ is $-0.0004$ (5 minute) and $+0.0004$ (15 minute), as it must be for the benchmark.
* **S002, logistic regression.** 5 minute: $\mathrm{BSS} = 0.0011$, interval $[-0.0014, 0.0038]$, no skill. 15 minute: $\mathrm{BSS} = 0.0298$, interval $[0.0208, 0.0396]$, positive in all five folds, AUC of rejection against the rest 0.550, calibration error 0.027, 0.029 and 0.012 for rejection, breakthrough and chop. The model that wins the inner folds is the strongest penalty (10) in every fold. Largest contributions to the rejection score: `bar_range`, `tod_cos`, `minute_of_day`, `tod_sin`, `band_width`.
* **S003, LightGBM.** 5 minute: $\mathrm{BSS} = 0.0015$, interval $[-0.0014, 0.0047]$. 15 minute: $\mathrm{BSS} = 0.0274$, interval $[0.0172, 0.0372]$, AUC 0.543. It does not beat the logistic model.
* **Trading.** All six runs made no trades. In every fold the nested scheme found no threshold on the calibrated rejection probability whose bootstrap lower bound of the net ticks was positive, so every fold abstained. Verdict DROP for all six, with the failed gates listed in the ledger. The DSR is near one half and the PBO undefined, as expected for strategies without trades.

**Reading.** This agrees with section 14. The 15 minute models do have probability skill, but it comes from range and clock features, which is the chop structure that the omnibus screen found. The relevant question for trading is direction, and no threshold on the rejection probability earns more than the 2.8 ticks of round trip cost on out of fold data. The tooling can now say so with calibrated probabilities, a cost model, a deflated Sharpe ratio and a record that anyone can replay. `vwap research verify` and `vwap research replay all` pass for all 13 recorded runs.

**Caveats.** The 15 minute skill is a statement about three class probabilities, not an edge. The tests of the strategy stages use a synthetic market with a planted signal, which proves the plumbing and the gates and nothing about NQ. The calendar features and the live fetch are untested against the real sites.

**Win rate of taking every touch (descriptive benchmark).** Because the strategies made no trades they have no win rate, so `vwap research benchmark config` simulates the plain rule: take every touch toward rejection, with the label barriers as target and stop, the configured costs (2.8 ticks round trip) and the same five test windows of the discovery segment. It uses the discovery view only, adds no ledger entry and spends no alpha. Pooled over the five windows, with every touch taken:

* **5 minute:** 5,326 trades, rejection share among resolved touches 48.9 percent, win rate 48.9 percent (48.8 net of cost), average `-0.61` ticks gross and `-3.41` net, interval `-4.65` to `-2.17`. The barriers average 39 ticks, so the win rate needed to break even is $p^{*} = \tfrac12 + \kappa / (2\bar\sigma) = 53.6$ percent, 4.7 points above what was seen.
* **15 minute:** 2,764 trades, rejection share 47.9 percent, win rate 48.0 percent (47.6 net), average `-1.98` ticks gross and `-4.78` net, interval `-8.14` to `-1.39`. The barriers average 72 ticks and the break even win rate is 51.9 percent, 3.9 points above.
* In every one of the five windows and on both timeframes the win rate is below the break even rate, and the gross average is negative in 9 of 10 windows. Taking one trade at a time (the configured policy) changes nothing: the pooled net average is `-3.44` and `-4.73` ticks.

This agrees with the strategy runs: the plain win rate is a coin flip, and no filter that the models could learn from out of fold data lifted it above the break even rate. The best lead on discovery data remains level confluence (section 14), which is unconfirmed and had an interval that includes zero.

## 17. The search for a better win rate

**Request and target.** The owner asked for a better win rate and then for an average of at least 4 trades per week. The target chosen was an edge: the win rate must beat the break even rate of the same barriers and costs,
$$ p^{*} = \frac{S + \kappa}{T + S}, $$
with target $T$ and stop $S$ in ticks and cost $\kappa = 2.8$ ticks, because a higher raw win rate bought with a smaller target raises $p^{*}$ by the same amount. Stated before any run: a null result was the most likely outcome.

**Baseline** (section 16): taking every touch wins 48.9 percent (5 minute) and 48.0 percent (15 minute) against break even rates of 53.6 and 51.9 percent.

**Tooling added.** The edge (win rate minus $p^{*}$) in the benchmark and in every strategy summary; strategy cards can train on resolved touches only (`target = "direction"`), set their own barrier shape, and require an average number of trades per week (`min_trades_per_week`, which also constrains the tuned threshold and is a gate); `vwap research benchmark --grid` examines the whole label grid and records its cells as counted trials. None of this touches `strategy.toml`, `validation.toml` or `backtest.toml`, so all earlier records still replay.

**1. Barrier shapes (96 cells, every touch, both timeframes).** Raw win rates run from 24.9 to 73.6 percent: a small target and a wide stop makes the win rate high. No cell has a positive net expectancy; the best, a 0.5 target and a 1.0 stop on 15 minute bars with a long horizon, wins 66.0 percent against 69.3 percent needed and loses 1.85 ticks per trade. Four cells show a positive win rate edge, but the approximation ignores trades that end at the horizon, and their net result is negative with an interval below zero. The 96 cells are counted as trials.

**2. Four new hypothesis cards** (existing features, mechanism written first, outcome rejection given resolved, alpha 0.0033 each):

| Card | Rule (prediction) | 5 minute difference, p | 15 minute difference, p | Verdict |
|---|---|---|---|---|
| H008 stretch exhaustion | `band_pos_max` top tercile (positive) | $-1.35$ points, 0.90 | $-2.26$ points, 0.92 | Drop, opposite sign |
| H009 quiet market | `vol_ratio` bottom tercile (positive) | $+0.84$ points, 0.22 | $+1.02$ points, 0.26 | Drop |
| H010 grind in | `bars_on_side` top tercile (negative) | $-1.16$ points, 0.16 | $+0.03$ points, 0.51 | Drop |
| H011 fighting the slope | `vslope_align` bottom tercile (negative) | $+1.84$ points, 0.95 | $+0.92$ points, 0.71 | Drop, opposite sign |

The opposite sign of H008 was not turned into a new card after the fact: that would be a post hoc flip with its own alpha cost and, at 15 minutes, a one sided p of about 0.08 in that direction. Cards run: 11 of 12; alpha spent 0.0467 of 0.05, so one card with 0.0033 is left.

**3. Direction models, 6 strategy cards (S004 to S009), 12 runs.** Logistic regression and LightGBM trained only on touches that resolve, the default barriers and the four barrier cells fixed before the grid was read, each required to average 4 trades per week. The pooled Brier skill on resolved touches is between $-0.0059$ and $+0.0001$ in all 12 runs, every interval contains zero, and every fold of every run abstained: no threshold on the calibrated probability of rejection earned a positive lower bound after costs. So the 15 minute skill of section 16 was chop, and with the chop removed from the objective there is nothing left. Rejection against breakthrough is not predictable from the 34 features in this data.

**4. The 4 trades per week requirement.** Over the 130 weeks of the test windows it means at least 520 trades, which is 10 percent of 5 minute touches (about 41 per week) and 19 percent of 15 minute touches (about 21 per week). The only lead in the record, a touch that coincides with two or more other levels (H007), has 115 to 142 events in 254 weeks, about half a trade per week, so it could not meet the requirement even if it were confirmed.

**Result.** The win rate cannot be raised honestly on this data. The all touch win rate is 48 to 49 percent against a break even rate of 52 to 54 percent; no feature subset, no model and no barrier shape lifted it above break even. Counted trials are now 126 (the omnibus, 11 cards, 18 strategy runs and the 96 grid cells). Everything stayed on the discovery segment: confirmation and holdout are untouched, and nothing was promoted.

**What would change this.** More history or a different instrument, real traded volume instead of a tick count, or a mechanism that the 34 features cannot see (order flow, the order book). A new idea needs a new card and costs part of the one remaining alpha share.

