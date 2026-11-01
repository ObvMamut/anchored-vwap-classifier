# Events: anchored VWAP and touch detection

Owner crate: `vwap_events`. Config: `config/events.toml`. Symbols: `GLOSSARY.md`.

## 1. Anchored VWAP

For a session with anchor $t_0$, on 1 minute bars:

$$ p_i = \frac{H_i + L_i + C_i}{3}, \qquad \mathrm{VWAP}_t = \frac{\sum_{i=t_0}^{t} p_i v_i}{\sum_{i=t_0}^{t} v_i} $$

The sums run over 1 minute bars of one session and one contract. They reset at every anchor. The VWAP is undefined until $\sum v_i > 0$.

Incremental form, used by the implementation. With cumulative volume $V_t = \sum_{i=t_0}^{t} v_i$ and cumulative price volume $S_t = \sum_{i=t_0}^{t} p_i v_i$:

$$ V_t = V_{t-1} + v_t, \qquad S_t = S_{t-1} + p_t v_t, \qquad \mathrm{VWAP}_t = \frac{S_t}{V_t} $$

Volume weighted variance of the typical price around the VWAP, needed for the bands in `FEATURES.md`. It is computed with a weighted Welford update to avoid cancellation:

$$ \mu_t = \mu_{t-1} + \frac{v_t}{V_t}(p_t - \mu_{t-1}), \qquad M_t = M_{t-1} + v_t (p_t - \mu_{t-1})(p_t - \mu_t), \qquad \sigma^2_{\mathrm{VWAP},t} = \frac{M_t}{V_t} $$

where $\mu_t = \mathrm{VWAP}_t$.

## 2. Sampling onto 5 and 15 minute bars

1. Coarse bars of width $w \in \{5, 15\}$ minutes are aligned to New York clock multiples of $w$ counted from midnight local time. Because the anchors 18:00 and 09:30 are multiples of 5 and 15 minutes, every session starts on a bar boundary. Bars never span the anchor.
2. Open $=$ open of the first 1 minute bar, high $=$ maximum of highs, low $=$ minimum of lows, close $=$ close of the last 1 minute bar, volume $=$ sum of volumes. Bars are labeled by open time and are known at close.
3. The VWAP value of a coarse bar $t$ is the 1 minute VWAP at the last 1 minute bar of the coarse bar:

$$ \mathrm{VWAP}^{(w)}_t = \mathrm{VWAP}_{\text{last minute of bar } t} $$

   so it includes all volume up to the close of bar $t$ and nothing later.
4. A coarse bar missing some 1 minute bars is built from the bars present and flagged `partial`. Partial bars cannot be touch bars. A coarse bar with no 1 minute bars does not exist.
5. Bars never cross a session boundary or a maintenance break.

## 3. ATR

Wilder ATR of length $n_{\mathrm{ATR}}$ (config, default 14) on the coarse bars of the same timeframe, carried across sessions within a contract:

$$ \mathrm{TR}_t = \max\big(H_t - L_t,\ |H_t - C_{t-1}|,\ |L_t - C_{t-1}|\big), \qquad \mathrm{ATR}_t = \frac{(n_{\mathrm{ATR}} - 1)\,\mathrm{ATR}_{t-1} + \mathrm{TR}_t}{n_{\mathrm{ATR}}} $$

Initialisation: the first value is the simple mean of the first $n_{\mathrm{ATR}}$ true ranges. No event can occur before ATR is initialised. On the first bar after a roll, $\mathrm{TR}_t = H_t - L_t$ (see `DATA.md`). $\mathrm{ATR}_t$ uses bars up to and including $t$, which is known at the close of $t$.

## 4. Touch and approach side

A **touch** is a bar with

$$ L_t \le \mathrm{VWAP}_t \le H_t $$

The **approach side** comes from the previous bar:

$$ s_t = \mathrm{sign}\big(C_{t-1} - \mathrm{VWAP}_{t-1}\big) \in \{+1, 0, -1\} $$

$s_t = +1$ means price approached from above. $s_t = 0$ (previous close exactly on the VWAP) is skipped and counted in the stage report. The previous bar must be in the same session and must not be partial.

### Self reference flag

Because $\mathrm{VWAP}_t$ includes bar $t$'s own volume at its typical price, the VWAP is pulled toward bar $t$'s range, which makes a touch more likely than for a level that existed before the bar. The default follows the specification above. A config flag selects a stricter reference as a sensitivity run:

* `vwap_reference = "bar_close"` (default): use $\mathrm{VWAP}_t$ as above.
* `vwap_reference = "prior_bar_close"`: use $\mathrm{VWAP}_{t-1}$ in the touch test, so the level is fully known before the bar starts.

Both are covered by the time shifted VWAP control in `BASELINES.md`. Using the second flag counts as a separate trial in the experiment log.

## 5. Re arm rule

After an event, a new touch only counts after price has moved away from the VWAP by at least $k$ ATRs. Define the excursion at bar $j$:

$$ e_j = \frac{|C_j - \mathrm{VWAP}_j|}{\mathrm{ATR}_j} $$

State machine per session:

1. The state starts **armed** at the first eligible bar of the session (after the exclusion window below).
2. If armed and bar $t$ is a touch with $s_t \ne 0$, emit an event and set the state to **disarmed**.
3. If disarmed, the state returns to armed at the first bar $j$ after the last event with $e_j \ge k$.
4. The state resets at every session anchor.

$k$ is configurable (`rearm_k`, default 1.0). Touches while disarmed are not events but are counted in the raw touch count used as a feature.

## 6. Exclusion after the anchor

Touches whose bar close is less than $N_{\mathrm{ex}}$ minutes after $t_0$ are ignored (config `exclude_minutes`, default 30). The VWAP and ATR keep accumulating during the window. Ignored touches do not change the state machine. The state starts armed at the first bar after the window.

## 7. Other event gates (all clock or data quality based)

An event is kept only if all of the following hold. None depends on future prices.

* The session is not `thin_session` and the bar is not `partial` (see `DATA.md`).
* No `roll_contaminated` or `gap_contaminated` flag.
* Close time of the touch bar is at or before the session's `last_entry_time` (config), chosen so that a full horizon of $T$ bars fits before the session `flat_time`. The label stage re checks this and records `horizon_fits`.
* ATR is initialised and positive.

Dropped events are counted by reason per year in the stage report.

## 8. Event record

`Event`: id, timeframe, session id, trading date, touch bar index, bar open time, bar close time, side $s_t$, $\mathrm{VWAP}_t$, $\mathrm{VWAP}_{t-1}$, $O_t, H_t, L_t, C_t$, $\mathrm{ATR}_t$, $\sigma_t$, accepted touch ordinal within the session, raw touch count so far, flags.

## 9. Visual check procedure

1. Draw 20 trading dates at random from the development range with `ChaCha8Rng` seeded from config. The seed and dates are written to the report. Holdout dates are never drawn.
2. For each date and timeframe, `vwap spotcheck` renders a PNG: candlesticks, the anchored VWAP line, the exclusion window shaded, each event marked with its side, and the re arm excursions annotated.
3. Review checklist per chart: VWAP starts at the right anchor; touches mark bars whose range contains the VWAP; the side matches the previous close; no event inside the exclusion window; no second event without a $k$ ATR excursion; no event after `last_entry_time`.
4. The owner signs off the 20 charts before Phase 3 starts. Findings are written to `reports/visual_check_events.md`.

## 10. Tests

* Unit: VWAP on a hand computed 5 bar example; Welford variance equals the two pass variance within $10^{-9}$; ATR on a textbook sequence.
* Property, lookahead: replacing all 1 minute bars after the close of bar $t$ leaves $\mathrm{VWAP}^{(w)}_t$, $\mathrm{ATR}_t$, the touch decision and $s_t$ unchanged.
* Property: every event satisfies $L_t \le \mathrm{VWAP}_t \le H_t$; the previous close sign equals $s_t$; consecutive events in one session are separated by an excursion $e_j \ge k$; no event closes within $N_{\mathrm{ex}}$ minutes of the anchor.
* Property: coarse bars aggregated from 1 minute bars satisfy $L \le O, C \le H$ and volume sums match.
* Integration: a synthetic session with a known approach, touch and re arm sequence.
