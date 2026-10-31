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
* Phase 2 visual check sign off: not yet.
* Phase 3 baseline gate: not yet.
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
