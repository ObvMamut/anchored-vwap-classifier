# Data

Owner crate: `vwap_data`. Config: `config/data.toml`. Symbols: `GLOSSARY.md`.

## 1. Input contract

File: `data.parquet` at the repository root (path from config).

Columns found in the file footer: `DateTime`, `Open`, `High`, `Low`, `Close`, `Volume`, `TickVolume`. There is no symbol, contract, bid volume or ask volume column.

Facts given by the owner and treated as the contract:

1. 1 minute bars only. 5 and 15 minute bars are built by aggregation in `vwap_events`.
2. Timestamps are UTC.
3. The series is a raw front month series, rolled by a calendar rule. Prices are real traded prices of the front month at each time, not adjusted.
4. VWAP weight is `Volume`. `TickVolume` is kept for cross checks and one optional feature.

Loader requirements, each a hard error if violated:

* Timestamps are strictly increasing after de duplication, normalised to `Datetime(Microseconds, "UTC")`. A naive datetime column is interpreted as UTC. A timezone aware column is converted to UTC.
* Prices are finite `f64`, positive, and lie on the tick grid: $\frac{P}{0.25}$ is an integer within $10^{-6}$. Violations are counted and reported, not repaired.
* `Volume` is finite and non negative.

### Open item: bar timestamp convention

The file does not say whether `DateTime` marks the bar open or the bar close. The pipeline assumes **open time** (config `bar_label = "open"`). A diagnostic in the Phase 1 data report tests this on the data: the first bar of a Globex session should be stamped 18:00 New York if the label is the open, and 18:01 if it is the close; the last bar before the daily maintenance break should be stamped 16:59 or 17:00 respectively. The diagnostic fails the stage if the evidence contradicts the config value. A wrong convention shifts every event by one bar and would itself be a lookahead error, so this check is mandatory.

## 2. Contract rolls

The data is one continuous raw series. Rolls are therefore visible only as a price discontinuity between two consecutive sessions, never as a second contract.

### 2.1 Roll table from the calendar rule

NQ expires on the third Friday of March, June, September and December. For each expiry $E$, the predicted roll date is

$$ r = E - \texttt{roll\_offset\_days} $$

with default `roll_offset_days = 8` (the Thursday one week before expiry week). The owner confirmed a calendar rule but not the offset, so the offset is a config value that the diagnostic must confirm.

For each predicted roll the diagnostic measures the discontinuity at the first session boundary on or after $r$:

$$ \Delta_r = O_{\text{first bar after}} - C_{\text{last bar before}} $$

and compares $|\Delta_r|$ with the distribution of ordinary overnight gaps over the whole sample (median and 99th percentile). The report lists, for each expiry, $\Delta_r$ at offsets from 0 to 14 days so the owner can see which offset produces a systematic step. Output: `roll_table.parquet` with columns expiry, predicted roll date, observed roll boundary, $\Delta_r$, and a `confirmed` flag. A roll is `confirmed` when $|\Delta_r|$ exceeds the 99th percentile of ordinary gaps. Unconfirmed rolls are listed for manual review and stop the stage until the owner accepts them in config.

### 2.2 Rules that follow from a raw series

1. **Prices are never adjusted.** A test asserts that every cleaned price equals the raw price bit for bit.
2. **VWAP is computed on the traded contract.** The anchored VWAP resets at the session anchor, and rolls are placed at a session boundary, so a VWAP never spans two contracts.
3. **Contamination.** Anything that looks back across a roll mixes two contracts and is wrong by the carry spread. An event is flagged `roll_contaminated` and excluded when any of the following spans a roll boundary:
   * its ATR window, its relative volume window, its realised volatility percentile window, its prior day or overnight levels, or its overnight gap reference;
   * its label horizon.

   The exclusion buffer is `roll_buffer_sessions` (default 1 session after the roll for levels and gaps; the longer lookbacks are handled by the rule above). Counts are reported per roll.
4. True range on the first bar after a roll uses only $H - L$, since the previous close belongs to a different contract.

## 3. Session anchors

Config `session_anchor` is one of:

* `globex`: the session begins at 18:00 America/New_York and ends at the next day 17:00.
* `rth`: the session begins at 09:30 America/New_York and ends at `rth_flat_time` (default 16:00).

Common definitions:

* The session id is the New York date of the anchor instant together with the anchor kind.
* The **trading date** $d$ of a session is the date of the RTH session that follows its anchor. A Globex session beginning Sunday 18:00 has Monday as trading date.
* `flat_time` per anchor (defaults 16:55 for Globex and 16:00 for RTH, both configurable) is the clock time after which no label horizon may extend. See `LABELING.md`.
* Early exclusion window of $N_{\mathrm{ex}}$ minutes after the anchor is defined in `EVENTS.md`.

Why 18:00 and 09:30 are safe: daylight saving changes happen at 02:00 on Sundays, so these two clock times always exist and are unique. The repeated hour of the autumn change and the skipped hour of the spring change still occur inside Globex sessions, so conversions between UTC and New York go through `chrono_tz` only and never use fixed offsets.

## 4. Timestamps and daylight saving

* Storage: UTC only. The cleaned parquet has no local time column.
* Derived: New York local time is computed on demand with `chrono_tz::America::New_York`.
* Ambiguous or non existent local times are never constructed from local strings. Anchors are constructed from a local date and time with an explicit rule: a non existent local time is an error; an ambiguous one is resolved to the earlier instant and logged. Neither case can occur for 18:00 or 09:30.
* Test: a table of every daylight saving transition date in the sample range with the expected UTC offset of the anchor, for both anchors. A second test checks that the number of 1 minute bars in the Globex session on transition days matches the expected 23 or 25 hour sessions less the maintenance break.

## 5. Quality checks

Each check has a threshold in config, an action, and a line in `quality_report.json`.

1. **Gaps.** Expected bars are generated from the session calendar (trading hours, maintenance break 17:00 to 18:00 New York, weekends, full holidays, early closes). Missing expected bars are grouped into runs. Runs of up to `max_gap_minutes_ok` (default 2) are logged only. Longer runs are listed as true gaps. Events whose lookback or horizon contains a true gap longer than `max_gap_minutes_event` (default 5) are flagged `gap_contaminated` and excluded.
2. **Zero volume bars.** Kept. A bar with $v_i = 0$ adds nothing to the VWAP sums. VWAP is undefined until the first positive volume bar of the session, and no event can occur before it. Counts per year are reported, and a year with an unusual share is flagged.
3. **Duplicate timestamps.** Rows identical in all columns are dropped and counted. Rows with the same timestamp but different values are an error that stops the stage.
4. **Outliers.** Errors: non positive price, $H < L$, open or close outside $[L, H]$, negative volume. Flags: a robust return score $z_i = \frac{r_i - \mathrm{median}(r)}{1.4826 \cdot \mathrm{MAD}(r)}$ on 1 minute log returns computed within a rolling window of prior sessions, flagged when $|z_i| > $ `outlier_z` (default 12), and a bar range above `max_range_atr_multiple` times the prior ATR. Flagged bars are written to a review file. They are not deleted automatically, because genuine news spikes are large.
5. **Early close holidays and full holidays.** A rule based CME equity holiday calendar is generated in code: New Year's Day, Martin Luther King Day, Presidents Day, Good Friday (computed from Easter), Memorial Day, Juneteenth (from its first observance), Independence Day, Labor Day, Thanksgiving, Christmas. Early closes: the day after Thanksgiving, Christmas Eve, and the shortened days around Independence Day when applicable, with early close time 13:15 New York by default (`early_close_time`). The rule is verified against the data: for each rule based early close day, the observed last bar time must equal the rule; mismatches are listed. Early close days are flagged in the session table and use their own `flat_time`.
6. **Session shape.** Each session's first and last bar times and bar count are tabulated. Sessions with fewer than `min_bars_fraction` (default 0.9) of expected bars are flagged `thin_session` and excluded from event generation.

## 6. Output

* `data/clean/year=YYYY/part0.parquet`: cleaned 1 minute bars, same columns as input plus `session_id`, `trading_date`, `is_early_close`, `flags` (bitmask for gap, outlier, thin session, roll buffer). Partitioned by calendar year of the UTC timestamp.
* `data/clean/roll_table.parquet`.
* `data/clean/sessions.parquet`: one row per session with anchor time, end time, flat time, bar counts, flags.
* `artifacts/data/quality_report.json` and `artifacts/data/manifest.json` (input file hash, output hashes, row counts per year, config hash, git SHA).
* Config keys (all in `config/data.toml`): `input_path`, `bar_label`, `session_anchor`, `roll_offset_days`, `roll_buffer_sessions`, `max_gap_minutes_ok`, `max_gap_minutes_event`, `outlier_z`, `max_range_atr_multiple`, `min_bars_fraction`, `early_close_time`, `flat_time_globex`, `flat_time_rth`.

## 7. Tests

* Unit tests for the roll date calculation (third Friday, offset arithmetic) over every quarter in the sample.
* Unit test: cleaning twice gives the same output (idempotent).
* Unit test: no price changes between raw and cleaned.
* Property test: random timestamp sequences with injected duplicates and gaps produce reports whose counts match the injected counts.
* Fixture: one hand built week containing a roll, a holiday, an early close and a daylight saving change.
