# Labeling: volatility scaled triple barrier

Owner crate: `vwap_labels`. Config: `config/labels.toml`. Symbols: `GLOSSARY.md`.

## 1. Definition

For a touch event at bar $t$ with approach side $s_t \in \{+1, -1\}$:

* Reference price: $P_0 = C_t$, the close of the touch bar.
* Volatility scale: $\sigma_t = \mathrm{ATR}_t$ by default. Config `sigma = "realised_vol"` selects the alternative

$$ \mathrm{rv}_t = \sqrt{\sum_{i=1}^{W} r_i^2}, \qquad \sigma_t = C_t \cdot \mathrm{rv}_t \cdot \sqrt{\frac{w}{W}} $$

  where $r_i$ are the 1 minute log returns of the last $W$ minutes up to the close of bar $t$ (config `rv_window_minutes`), and the factor $\sqrt{w/W}$ rescales the $W$ minute volatility to one coarse bar of $w$ minutes. Both choices give $\sigma_t$ in index points per coarse bar. ATR is the default and the primary cell uses it.

Barriers, with $s_t = +1$ meaning price approached from above, so a rejection is a move back up:

$$ B^{\text{rej}} = P_0 + s_t\, m_1 \sigma_t, \qquad B^{\text{brk}} = P_0 - s_t\, m_2 \sigma_t $$

The vertical barrier is the close of bar $t + T$.

Scan bars $u = t+1, \dots, t+T$ in order. In bar $u$:

* The rejection barrier is hit if $s_t = +1$ and $H_u \ge B^{\text{rej}}$, or $s_t = -1$ and $L_u \le B^{\text{rej}}$.
* The breakthrough barrier is hit if $s_t = +1$ and $L_u \le B^{\text{brk}}$, or $s_t = -1$ and $H_u \ge B^{\text{brk}}$.

Class:

$$ y = \begin{cases} \text{REJECTION} & \text{rejection barrier hit first} \\ \text{BREAKTHROUGH} & \text{breakthrough barrier hit first} \\ \text{CHOP} & \text{neither hit within } T \text{ bars} \end{cases} $$

The touch bar itself is never scanned. Its close is the reference price.

Note on geometry: both barriers are measured from $P_0$, not from the VWAP. At the touch, $P_0$ may sit above or below the VWAP. The sign convention uses the approach side, so "rejection" always means a move back toward the side price came from. This keeps the two classes symmetric for $s_t = \pm 1$, and a property test checks it by mirroring prices around $P_0$.

## 2. Same bar double hits

If in one coarse bar $u$ both barriers are hit:

1. Replay the 1 minute bars inside bar $u$ in time order. The first 1 minute bar whose range reaches a barrier decides the class.
2. If a single 1 minute bar reaches both barriers, the order is unknowable. Mark the event `ambiguous`.

Ambiguous events are excluded from model fitting and from all classification metrics. They are counted per year in the label report. In the backtest they are treated as the worse outcome (see `BACKTEST.md`). If the share of ambiguous events in any year exceeds `max_ambiguous_share` (default 0.02), the stage warns, and the owner decides in `DECISIONS.md`.

A bar where only one barrier is hit needs no 1 minute data.

## 3. Recorded per event

* `class`, `ambiguous`.
* `t_b`: bar index of the first barrier hit, and **time to barrier** $\tau_b = (t_b - t)\,w$ in minutes. For CHOP, $\tau_b = T w$.
* `t_end`: the close time of bar $t_b$ for barrier hits, or of bar $t+T$ for CHOP. This is the end of the label span used by purging in `VALIDATION.md`.
* **Maximum adverse excursion**, in units of $\sigma_t$, the largest move against the rejection direction before the label resolves:

$$ \mathrm{MAE} = \frac{1}{\sigma_t}\max_{u \in (t, t_b]} \max\!\big(0,\ s_t (P_0 - X_u)\big), \qquad X_u = \begin{cases} L_u & s_t = +1 \\ H_u & s_t = -1 \end{cases} $$

* **Horizon return** in ticks, signed in the rejection direction, used by the threshold proxy and the CHOP outcome:

$$ R_T = \frac{s_t\,(C_{t+T} - P_0)}{\delta} $$

* Outcome in ticks under the barrier exit rule (before costs): $+\frac{m_1\sigma_t}{\delta}$ for REJECTION, $-\frac{m_2\sigma_t}{\delta}$ for BREAKTHROUGH, $R_T$ for CHOP.

## 4. Horizon rule and contamination

* If the full horizon of $T$ bars does not fit before the session `flat_time`, the event is dropped and counted (`horizon_fits = false`). This depends on the clock only, so it is not lookahead. Truncating the horizon would bias CHOP upward near the close, which is why events are dropped instead.
* Events whose horizon spans a contract roll or a long gap are dropped (`DATA.md`).
* The maintenance break and weekends are never inside a horizon, because of the first rule.

## 5. Parameters and sensitivity grid

All parameters are in `config/labels.toml`: `m1`, `m2`, `horizon_bars` per timeframe, `sigma`, `max_ambiguous_share`.

Placeholder defaults, to be fixed in config before any model result is looked at: $m_1 = 1.0$, $m_2 = 1.0$, $T = 12$ bars for 5 minute bars (1 hour) and $T = 8$ bars for 15 minute bars (2 hours).

**Sensitivity grid** (config `[grid]`): $m_1 \in \{0.5, 0.75, 1.0, 1.5\}$, $m_2 \in \{0.5, 0.75, 1.0, 1.5\}$, $T$ per timeframe in $\{6, 12, 24\}$ bars for 5 minute and $\{4, 8, 16\}$ bars for 15 minute.

Rules:

1. One **primary cell** is written into config and committed before any model or backtest run. It is chosen from the placeholder defaults above on grounds of symmetry and economic meaning, not on outcomes.
2. The grid is run for **descriptive statistics only**: class frequencies, time to barrier, MAE, ambiguity share per cell. It is never used to pick the cell with the best model result or the best backtest.
3. Grid statistics are computed on development data only. They are never computed on a test fold in a way that feeds a decision, and never on the holdout.
4. Every cell that is run through a model or backtest counts as a trial in the experiment log.
5. Switching the primary cell after seeing any model result is a protocol violation and must be logged in `DECISIONS.md` with the trial count increased.

## 6. Output

`artifacts/labels/labeled_events_{5m,15m}.parquet`: all `Event` fields plus the recorded fields in section 3. `label_report.json`: counts by class, year, side, ambiguity, dropped reasons.

## 7. Tests

* Hand built bar sequences, one per outcome: REJECTION, BREAKTHROUGH, CHOP, a same bar double hit resolved by 1 minute data, and an ambiguous case.
* Property, lookahead: replacing all bars after $t + T$ (and all 1 minute bars after the end of bar $t+T$) leaves the label, $t_b$, MAE and $R_T$ unchanged. Replacing bars after $t_b$ leaves the class unchanged.
* Property, causality of the reference: replacing bars before $t$ cannot change the class when $P_0$ and $\sigma_t$ are held fixed.
* Property, symmetry: reflecting all prices around $P_0$ and flipping $s_t$ gives the same class.
* Property: $\mathrm{MAE} \ge 0$; $\tau_b \le T w$; class CHOP implies neither barrier price was reached in any scanned bar.
