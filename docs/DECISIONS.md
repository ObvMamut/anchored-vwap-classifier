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

* Phase 1 exit: not yet.
* Phase 2 visual check sign off: not yet.
* Phase 3 baseline gate: not yet.
* Phase 4 lookahead matrix: not yet.
* Phase 5 model gate: not yet.
* Phase 6 backtest gate: not yet.
* Phase 7 holdout run: not yet. Lock file absent.
