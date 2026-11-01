# Features

> **Status.** 34 feature columns are implemented and listed in section 9: the 18 of the research loop, nine calendar columns (K1 to K3 for three release types) and seven appended designed columns. The rest of this document is the original design. Not built: A1 and A2 for several lookbacks, V2, V3, V4, C3 as a one hot, C7 and C8 as separate columns, opening range levels, R1 and R2.

Owner crate: `vwap_features`. Config: `config/features.toml`. Symbols: `GLOSSARY.md`.

Every feature is a function of the `CausalView` at the touch bar $t$. The view exposes 1 minute bars and coarse bars with close time at or before the close of bar $t$, the session state built from them, and nothing else. A feature is computed once per event and stored as `f64`; undefined values are `NaN`.

## 0. Notation and the causal proof template

* $t$: touch bar. $s_t$: approach side. $\sigma_t = \mathrm{ATR}_t$. $\delta = 0.25$.
* Signed distance to VWAP, positive on the approach side: $d_j = s_t\,(C_j - \mathrm{VWAP}_j)$.
* The **current run** is the maximal block of consecutive bars $j = r, \dots, t-1$ such that $d_j > 0$ (closes on the approach side of their own VWAP). By definition of $s_t$, bar $t-1$ belongs to the run. $n^{\text{run}}_t = t - r$.
* $L$ is a lookback in bars, taken from a config list (defaults $\{3, 6\}$).
* All lookbacks are in whole bars of the event's timeframe. A feature needing $L$ bars of history inside the session is `NaN` when fewer exist since the anchor.

Each feature below carries a **causal proof**: it lists the inputs and states the maximal bar index used. The proof is backed by a named proptest of the form `no_lookahead_<feature>`: generate random bars, pick a cut at $t$, replace all later bars, recompute, assert bit equality.

## 1. Approach features

**A1 speed into VWAP** (for each $L$):

$$ \mathrm{speed}^{(L)}_t = \frac{d_{t-L} - d_{t-1}}{L\,\sigma_t} $$

Positive when price is closing in on the VWAP. Units: $\sigma_t$ per bar. Causal proof: uses $C_{t-L}, C_{t-1}$, $\mathrm{VWAP}_{t-L}, \mathrm{VWAP}_{t-1}$ and $\sigma_t$; maximal index $t$ (through $\sigma_t$). `NaN` if $t - L$ precedes the first eligible bar of the session.

**A2 slope into VWAP** (for each $L$): the ordinary least squares slope $\beta$ of $d_j$ on $j$ over $j = t-L, \dots, t-1$, scaled:

$$ \mathrm{slope}^{(L)}_t = \frac{1}{\sigma_t}\cdot\frac{\sum_{j}(j - \bar j)(d_j - \bar d)}{\sum_{j}(j - \bar j)^2} $$

Negative when approaching. Causal proof: indices $t-L$ to $t-1$ and $\sigma_t$.

**A3 distance travelled** over the current run, in units of $\sigma_t$:

$$ D_t = \frac{\max_{j \in [r,\, t-1]} d_j}{\sigma_t} $$

Causal proof: closes and VWAPs for $j \le t-1$, plus $\sigma_t$.

**A4 bars on one side**: $n^{\text{side}}_t = n^{\text{run}}_t = t - r$. Causal proof: closes and VWAPs for $j \le t-1$. Capped at the number of bars since the anchor.

**A5 touch bar shape** (optional, config): close location $\frac{C_t - L_t}{H_t - L_t}$ in $[0,1]$ and bar range $\frac{H_t - L_t}{\sigma_t}$. Causal proof: bar $t$ only, known at its close. Not used to define the label, which starts at $t+1$.

## 2. Volume features

Let $\text{slot}(t)$ be the New York clock time of bar $t$ at resolution $w$. For a prior session $d-m$, let $v^{(d-m)}_{\text{slot}}$ be the volume of its bar at that slot, and $V^{(d-m)}_{\text{slot}}$ the cumulative volume from its anchor to the close of that slot. The sessions $d-1, \dots, d-20$ are the 20 sessions immediately preceding the current one, of the same anchor kind, with slot present.

**V1 relative volume**:

$$ \mathrm{RV}_t = \frac{v_t}{\frac{1}{M}\sum_{m=1}^{M} v^{(d-m)}_{\text{slot}(t)}} $$

where $M$ is the number of those 20 sessions that have the slot, required to be at least 10, else `NaN`. Causal proof: $v_t$ and volumes of earlier sessions only. Sessions are strictly earlier than the anchor of the current session.

**V2 cumulative relative volume**:

$$ \mathrm{CRV}_t = \frac{\sum_{i=t_0}^{t} v_i}{\frac{1}{M}\sum_{m=1}^{M} V^{(d-m)}_{\text{slot}(t)}} $$

Causal proof: current session volume up to $t$, and earlier sessions.

**V3 volume trend on the approach**: the OLS slope of $v_j$ on $j$ over the run (at least 3 bars, else `NaN`), divided by the run mean volume:

$$ \mathrm{vtrend}_t = \frac{\mathrm{slope}\big(v_j\big)_{j=r}^{t-1}}{\frac{1}{n^{\text{run}}_t}\sum_{j=r}^{t-1} v_j} $$

Causal proof: volumes for $j \le t-1$.

**V4 optional, off by default**: tick count per contract $\frac{\mathrm{TickVolume}_t}{v_t}$ as a trade size proxy, with the caveat that the source tick count may be price change ticks and not trades. Causal proof: bar $t$ only.

**Delta is omitted.** The data has no bid and ask volume, so signed volume cannot be formed. The docs do not synthesise a delta from candle direction, because that is a different, weaker quantity.

## 3. Context features

**C1 minutes since anchor**: $(s_t + w - t_0)$ in minutes. Causal proof: clock only.

**C2 time of day**: with $\theta_t$ the New York clock time of the bar close in minutes since midnight,

$$ \sin\!\Big(\frac{2\pi\,\theta_t}{1440}\Big), \qquad \cos\!\Big(\frac{2\pi\,\theta_t}{1440}\Big) $$

Trees use C1 directly. The sine and cosine pair is for the logistic model. Causal proof: clock only.

**C3 weekday** of the trading date, integer 0 to 4 (one hot for the logistic model). Causal proof: calendar only.

**C4 VWAP slope** (for each $L$):

$$ \mathrm{vslope}^{(L)}_t = \frac{\mathrm{VWAP}_t - \mathrm{VWAP}_{t-L}}{L\,\sigma_t} $$

Causal proof: indices $t-L$ and $t$ of the VWAP series, which at index $t$ uses 1 minute bars up to the close of $t$.

**C5 VWAP band width** in units of $\sigma_t$, using the volume weighted standard deviation from `EVENTS.md`:

$$ \mathrm{bw}_t = \frac{\sigma_{\mathrm{VWAP},t}}{\sigma_t} $$

**C6 band position of the approach extreme**: the distance of the run's furthest close from the VWAP, in band units,

$$ z^{\max}_t = \max_{j \in [r,\, t-1]} \frac{d_j}{\sigma_{\mathrm{VWAP},j}} $$

Causal proof for C5 and C6: use $\sigma_{\mathrm{VWAP},j}$ for $j \le t$, each built from bars up to $j$. The standard deviation is `NaN` until the session volume is positive and at least 5 bars exist.

**C7 accepted touches so far**: the number of events emitted earlier in the same session, before $t$.

**C8 raw touches so far**: the number of earlier bars $j < t$ in the session, after the exclusion window, with $L_j \le \mathrm{VWAP}_j \le H_j$.

Causal proof for C7 and C8: counts of events and touch tests at indices strictly below $t$, a function of bars up to $t-1$.

## 4. Level features

For a level $\ell$ (prior day high, low, close; overnight high, low; opening range high, low), the distance in units of $\sigma_t$ in the **rejection direction** is

$$ d^{\ell}_t = \frac{s_t\,(\ell - C_t)}{\sigma_t} $$

It is positive when the level lies in the direction price would go if it rejected the VWAP, and negative when it lies behind price. A companion flag is $\mathbf 1\big[\,0 < d^{\ell}_t \le m_1\,\big]$: the level sits inside the rejection barrier distance.

Definitions, with $d$ the trading date of the current session:

* **Prior day high, low, close** ($\mathrm{PDH}, \mathrm{PDL}, \mathrm{PDC}$): the high, low and close of the RTH session (09:30 to 16:00 New York) of the trading date before $d$, computed from 1 minute bars of that finished session. Config `prior_day = "rth"` is the default; `"globex"` uses the whole previous Globex session instead.
* **Overnight high and low** ($\mathrm{ONH}, \mathrm{ONL}$): the extremes of 1 minute bars from the previous RTH close (16:00) through $\min(\text{close of } t,\ 09{:}30)$ on the trading date. For events after 09:30 the overnight range is complete. For events before 09:30 it is the running range so far, with a companion indicator `on_complete = 0`.
* **Opening range high and low** ($\mathrm{ORH}, \mathrm{ORL}$): the extremes of the first `or_minutes` (default 30) of RTH. `NaN` until the close of the opening range window, with `or_complete = 0`.

Causal proof: prior day values come from sessions finished before the current anchor; overnight values come from bars up to $\min(\text{close of } t, 09{:}30)$; opening range values are used only after the range window has closed, which is at or before the close of $t$.

Levels that span a contract roll are `NaN` (see `DATA.md`).

## 5. Regime features

**R1 realised volatility percentile**. Let $r_i$ be 1 minute log returns and $W$ the window (config `rv_window_minutes`, default 60):

$$ \mathrm{rv}_t = \sqrt{\sum_{i=1}^{W} r_i^2} $$

with the sum over the last $W$ minutes up to the close of $t$. Let $\mathrm{rv}^{(d-m)}_{\text{slot}(t)}$ be the same quantity at the same clock slot in prior session $d-m$ for $m = 1, \dots, 252$ (at least 60 present, else `NaN`):

$$ \mathrm{rvpct}_t = \frac{1}{M}\sum_{m=1}^{M}\mathbf 1\big[\mathrm{rv}^{(d-m)}_{\text{slot}(t)} \le \mathrm{rv}_t\big] $$

Causal proof: current $\mathrm{rv}_t$ uses returns up to the close of $t$; the reference distribution uses strictly earlier sessions only.

**R2 overnight gap**, in units of the prior daily scale $\sigma^D$ (the average of the RTH high minus low of the 14 most recent finished sessions):

$$ G_t = \frac{X_t - C^{\mathrm{RTH}}_{d-1}}{\sigma^{D}}, \qquad X_t = \begin{cases} O_{09:30} & \text{if the close of } t > 09{:}30 \text{ on trading date } d \\ C_t & \text{otherwise} \end{cases} $$

Causal proof: $O_{09:30}$ is the open of the first RTH 1 minute bar, which has printed by any time after 09:30; before that, the current close is used and the feature is a running overnight move. $C^{\mathrm{RTH}}_{d-1}$ and $\sigma^D$ come from finished sessions.

## 6. Optional calendar flags (CPI, FOMC, NFP)

`config/features.toml` has `calendar_enabled = false` by default, and while it is false the nine calendar columns are `NaN`. The source is a frozen snapshot in `research/calendar/` (tracked by git): `events.csv` (columns `kind`, `date`, `utc_us`, `source`) and `lock.json` (retrieval time, the SHA256 of every page, the SHA256 of the CSV, the counts per year). It is produced by `vwap calendar fetch config`, which downloads the official release schedules once (the BLS schedule pages for CPI and the Employment Situation, the Federal Reserve meeting calendars for FOMC), reads the visible text of each page, validates the result against `config/calendar.toml` and writes the snapshot, or fails closed and writes nothing. `vwap calendar import file.csv config` freezes a hand supplied CSV of `kind,date` rows with the same validation, for a machine that cannot reach the sites. A snapshot whose CSV no longer matches its hash is refused by every reader, and `vwap doctor` checks it when the features are on.

Validation (`calendar.toml`): each year of the range holds 11 to 12 CPI and Employment Situation releases (2025 lost a CPI release and delayed a jobs report in the federal shutdown) and 6 to 9 FOMC meetings, no kind and date appears twice, and the release instant is the configured New York time, 08:30 for CPI and NFP and 14:00 for the FOMC statement, converted to UTC with the daylight saving rules of that day. No other stage touches the network.

**Limits of what was verified.** The page addresses and the page layouts could not be checked from the development environment, where the two hosts do not resolve. The parsers were written against the text patterns of these pages and tested on fixtures that follow them. If a host moved or a page changed, the fetch fails with the reason (the counts per year that were found) and the addresses in `calendar.toml` are the place to fix. Nothing was recorded from a live fetch, so `calendar_enabled` stays false in this repository until the owner commits a snapshot.

For each type $e \in \{\text{CPI}, \text{FOMC}, \text{NFP}\}$:

* **K1 release day flag**, column `cal_<e>_day`: $\mathbf 1[\text{a release of type } e \text{ falls inside the Globex session of the touch}]$.
* **K2 minutes until the next scheduled release** of type $e$ after the touch close, column `cal_<e>_until`, capped at `calendar_cap_minutes` (1440).
* **K3 minutes since the last release** of type $e$ at or before the touch close, column `cal_<e>_since`, capped at 1440. A close exactly at the release gives 0.

Causal proof: the scheduled release time of an event is published in advance by the source. The features use schedule times, the close time and the session of the touch bar, never a price and never the release outcome. The snapshot stores the date as originally scheduled.

## 7. Missing values and scaling

* `NaN` marks undefined values. Per feature `NaN` rates are reported per year and per fold.
* LightGBM treats `NaN` as missing natively. The logistic model imputes with training fold medians and adds a missing indicator column for any feature with a `NaN` share above 1 percent in the training fold.
* Scaling constants for the logistic model (mean and standard deviation) are fit on the training fold only.
* Heavy tails: for the logistic model, features in $\sigma_t$ units are winsorised at training fold 1st and 99th percentiles.

## 8. Lookahead test matrix

Every row needs a passing proptest before Phase 4 closes.

* A1 speed: `no_lookahead_speed`
* A2 slope: `no_lookahead_approach_slope`
* A3 distance travelled: `no_lookahead_distance_travelled`
* A4 bars on one side: `no_lookahead_bars_on_side`
* A5 touch bar shape: `no_lookahead_bar_shape`
* V1 relative volume: `no_lookahead_rel_volume`
* V2 cumulative relative volume: `no_lookahead_cum_rel_volume`
* V3 volume trend: `no_lookahead_volume_trend`
* V4 tick per contract: `no_lookahead_tick_ratio`
* C1 to C3 clock features: `no_lookahead_clock`
* C4 VWAP slope: `no_lookahead_vwap_slope`
* C5 and C6 bands: `no_lookahead_vwap_bands`
* C7 and C8 touch counts: `no_lookahead_touch_counts`
* Levels (PDH, PDL, PDC, ONH, ONL, ORH, ORL): `no_lookahead_levels`
* R1 volatility percentile: `no_lookahead_rv_percentile`
* R2 overnight gap: `no_lookahead_gap`
* K1 to K3 calendar: `no_lookahead_calendar`

A global test also computes the full feature matrix on a series and on the same series with every bar after each event replaced, and asserts equality for all features at all events.

Further engine level tests: the `CausalView` has no method that can return a bar after $t$ (checked by a compile fail test); statistics over a window never include the current session's future; rolling percentile references exclude the current session.

## 9. Implemented feature set

The 34 columns below are built in `crates/vwap_features/src/features.rs` behind the `CausalView` of `crates/vwap_features/src/view.rs`, in this order. The names are the card field `feature`. The column order is part of the golden fingerprints: new columns are appended, never inserted. Notation as in section 0.

**The 18 original columns** (the research loop of `PROTOCOL.md` used these):

* `vslope_align` (regime): $s_t\,\frac{\mathrm{VWAP}_t - \mathrm{VWAP}_{t-L}}{L\,\sigma_t}$ with $L = \lceil 60 / w \rceil$ bars, `NaN` if $t-L$ is in an earlier session. Positive when the VWAP slopes in the approach direction. Inputs: VWAP at $t$ and $t-L$, $\sigma_t$.
* `dist_travelled` (approach): $\max_{j \in \text{run}} d_j / \sigma_t$ over the current same side run, inputs at indices $\le t-1$ plus $\sigma_t$. `NaN` if the run is empty.
* `bars_on_side` (approach): length of that run.
* `speed` (approach): $\frac{d_{t-4} - d_{t-1}}{3\,\sigma_t}$, `NaN` if $t-4$ is in an earlier session.
* `rel_volume` (volume): $v_t$ divided by the mean volume of the same New York clock slot over up to 20 earlier sessions, `NaN` with fewer than 10. Volume is the tick count.
* `ordinal`, `raw_touches_before` (context): accepted events and raw touches earlier in the session.
* `minute_of_day`, `minutes_since_anchor` (context): New York clock minute of the touch close, and minutes since the 18:00 anchor.
* `vol_ratio` (regime): $10^4\,\sigma_t / C_t$.
* `band_width` (context): $\sigma_{\mathrm{VWAP},t} / \sigma_t$.
* `conf_flag`, `dist_pdh`, `dist_pdl`, `dist_pdc`, `dist_onh` (levels): $s_t(\ell - C_t)/\sigma_t$ for prior day high, low, close and overnight high, and `conf_flag` $= 1$ when any available level is within $0.5\,\sigma_t$. Prior day levels come from the finished regular session of the previous trading date; the overnight range is complete only after 09:30 New York and is `NaN` before.
* `time_block` (timing): clock block of the touch close in New York time, 0 for 09:30 to 10:30, 1 for 10:30 to 14:00, 2 for 14:00 to 17:00, 3 otherwise (overnight).
* `conf_count` (levels): how many of the five levels lie within $0.5\,\sigma_t$ of the touch close.

**Nine calendar columns** (section 6): `cal_cpi_day`, `cal_cpi_until`, `cal_cpi_since`, `cal_fomc_day`, `cal_fomc_until`, `cal_fomc_since`, `cal_nfp_day`, `cal_nfp_until`, `cal_nfp_since`. `NaN` while the calendar is off.

**Seven appended columns:**

* `close_location` (A5): $\frac{C_t - L_t}{H_t - L_t}$ in $[0, 1]$, `NaN` for a bar without range. Bar $t$ only, known at its close; the label starts at $t+1$.
* `bar_range` (A5): $\frac{H_t - L_t}{\sigma_t}$.
* `tod_sin`, `tod_cos` (C2): $\sin(2\pi\theta_t / 1440)$ and $\cos(2\pi\theta_t / 1440)$ with $\theta_t$ the New York clock minute of the touch close. Clock only.
* `weekday` (C3): $(\text{trading date in days since the epoch} + 3) \bmod 7$, Monday is 0. Calendar only. A Globex session that opens on Sunday evening belongs to Monday.
* `dist_onl` (levels): the overnight low in the same form as `dist_onh`.
* `band_pos_max` (C6): $\max_{j \in \text{run}} d_j / \sigma_{\mathrm{VWAP},j}$ over the current run, using only $\sigma_{\mathrm{VWAP},j} > 0$. Inputs at indices $\le t-1$.

**Causal proof for the whole set.** Every feature is computed from a `CausalView`, whose accessors return `None` for any bar index above the touch bar $t$, from the touch event (known at the close of its bar), from finished earlier sessions and finished prior day levels, and from the published schedule of the calendar table. The property test `features_do_not_look_ahead` (`crates/vwap_features/tests/lookahead.rs`) replaces every 1 minute bar after a random cut and asserts that all 34 features of all events that closed before the cut are bit for bit unchanged, on both timeframes, with a calendar table that has releases inside the series. Three golden fingerprints pin the values on a fixed synthetic series: the 18 original columns (unchanged since the research loop, so nothing in the recorded results moved), the nine calendar columns, and the seven appended columns. A fingerprint is changed only deliberately, with the reason in the commit message.

**Flaw found in the protocol.** The card outcome `rejection` is the indicator of the REJECTION class, which equals $(1 - P_{\text{chop}})\,P(\text{rej} \mid \text{resolved})$. Any feature related to chop, above all time of day, would make a card on `rejection` look significant through chop alone. The directional hypothesis needs the outcome conditional on resolution. Changing the outcome definition is a protocol change and needs the owner's decision.
