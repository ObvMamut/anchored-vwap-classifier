# Backtest

> **Status.** Implemented in `crates/vwap_backtest`, run inside `vwap research strategy`. Section 9 lists what differs from this design. Configuration: `config/backtest.toml`. Symbols: `GLOSSARY.md`.

Owner crate: `vwap_backtest`.

The backtest takes the walk forward outer test predictions (never training data), applies the trade rule, simulates fills with costs, and reports. It is the authority on economic value. The label proxy used for tuning $\tau$ in `MODELING.md` is not.

## 1. Signal and entry

* Signal: at the close of touch bar $t$, trade if $\tilde P(\text{rejection}) > \tau^*_f$, where $\tau^*_f$ is the threshold chosen by fold $f$ from its own training window.
* Direction: the rejection direction, $\mathrm{dir} = s_t$. If price came from above ($s_t = +1$), go long. If it came from below, go short.
* **Entry at the open of bar $t+1$**, never at the touch price or the touch close. Fill price $E = O_{t+1} + \mathrm{dir}\cdot n_{\text{slip}}\,\delta$ (the slippage always works against the trade).
* One contract of NQ per trade. No scaling and no compounding. Sizing is fixed so the equity curve is in USD per contract.

## 2. Position policy

One position at a time, flat to flat. A signal that arrives while a trade is open is ignored and counted as `skipped_busy`. Config `position_policy` also allows `one_per_session` and `overlap`, with the results of all three reported side by side. The default and the headline numbers use `one_at_a_time`. For `overlap`, statistics use session clustered intervals because overlapping trades are not independent.

## 3. Exits

Exits use the labeling barriers, as fixed absolute price levels computed at the touch from $P_0 = C_t$ and $\sigma_t$:

* Target (rejection barrier): $B^{\text{rej}} = P_0 + s_t\,m_1\sigma_t$.
* Stop (breakthrough barrier): $B^{\text{brk}} = P_0 - s_t\,m_2\sigma_t$.
* Time exit: the close of bar $t+T$.

The bars after entry are scanned from $t+1$ in order.

* **Gap at the open**: because entry is at the open of bar $t+1$, the open may already be beyond a barrier. If $O_{t+1}$ is beyond the stop, the trade exits at $O_{t+1}$ with slippage (a stop fills at the worse of the barrier and the open). If it is beyond the target, the trade exits at the open (a limit order fills at the barrier or better).
* **Same bar double hit**: resolved with 1 minute bars exactly as in `LABELING.md`. If a single 1 minute bar reaches both, the event is `ambiguous` and the trade is booked as a **stop** (the pessimistic assumption).
* **Fills**: stop exit fills at $B^{\text{brk}}$ with adverse slippage $n_{\text{slip}}$ ticks, or at the open with slippage if the open is beyond. Target exit fills at $B^{\text{rej}}$ with adverse slippage $n_{\text{slip}}$ ticks (a conservative choice: slippage is applied on every fill, entry and exit). Time exit fills at the close of bar $t+T$ minus adverse slippage.
* No trade is open at the session `flat_time`: the label stage already drops events whose horizon would cross it.

## 4. Costs

Constants (config, with these defaults and validation floors):

* Tick size $\delta = 0.25$ index points. Tick value $5$ USD. One full index point is worth $20$ USD.
* Slippage $n_{\text{slip}}$: at least 1 tick per side (validation error below 1 for headline runs), configurable.
* Commission $c_{\text{usd}}$ per side, configurable, and fees included in it.

For a trade with entry fill $E$ and exit fill $X$ (slippage already inside both prices), net result in ticks and USD:

$$ \text{PnL}_{\text{ticks}} = \frac{\mathrm{dir}\,(X - E)}{\delta} - \frac{2\,c_{\text{usd}}}{5}, \qquad \text{PnL}_{\text{usd}} = 5\cdot\text{PnL}_{\text{ticks}} $$

Equivalent form with the frictionless prices $E_0 = O_{t+1}$ and $X_0$:

$$ \text{PnL}_{\text{ticks}} = \frac{\mathrm{dir}\,(X_0 - E_0)}{\delta} - 2\,n_{\text{slip}} - \frac{2\,c_{\text{usd}}}{5} = \text{gross ticks} - \kappa $$

**Cost sensitivity sweep**: re run the identical trades with $n_{\text{slip}} \in \{0, 1, 2, 3\}$ ticks per side (the 0 case is shown for reference only and never as a headline) and commission levels from 0 to 6 USD per side in 1 USD steps. Output a grid of net expectancy and a break even slippage: the largest $n_{\text{slip}}$ at which pooled net expectancy stays positive. The thresholds $\tau^*$ are not re tuned inside the sweep.

## 5. Metrics

Per fold, pooled, and by the breakdowns in `VALIDATION.md`:

* **Trade count** $n$, plus counts of skipped (busy), ambiguous, gap exits.
* **Expectancy in ticks**: $\overline{\text{PnL}}_{\text{ticks}} = \dfrac{1}{n}\sum_{i} \text{PnL}_{\text{ticks},i}$, with a session clustered bootstrap 95 percent interval.
* **Hit rate**: $\dfrac{\#\{\text{PnL}_i > 0\}}{n}$.
* **Profit factor**: $\mathrm{PF} = \dfrac{\sum_{i:\,\text{PnL}_i > 0} \text{PnL}_i}{\sum_{i:\,\text{PnL}_i < 0} |\text{PnL}_i|}$.
* **Sharpe ratio**: build the daily net USD PnL series over all days in the evaluation range (zero on days without trades), then

$$ \widehat{SR}_{\text{ann}} = \sqrt{252}\;\frac{\bar r}{s_r} $$

  with $\bar r$ and $s_r$ the mean and sample standard deviation of daily PnL. The unannualised daily value $\widehat{SR}$ feeds the Deflated Sharpe Ratio.
* **Max drawdown** on the cumulative USD equity $E_u$:

$$ \mathrm{MDD} = \max_{u}\ \Big(\max_{v \le u} E_v - E_u\Big) $$

* Average time in trade, average MAE of trades, exit reason mix (target, stop, time, gap).
* Expected value per trade implied by the model, $\sum_c \tilde P_c\,\mu_c - \kappa$, compared with realised expectancy, as a calibration of economic expectations.

The equity curve plot shows the cumulative USD per contract, the drawdown underneath, and fold boundaries.

## 6. Gate (Phase 6)

Pass only if all of the following hold on the primary configuration with at least 1 tick slippage per side and the configured commission:

1. net expectancy is positive in at least a pre registered fraction of outer test folds (config `min_fold_fraction`, default 0.6, which satisfies "most folds");
2. pooled net expectancy has a session clustered bootstrap lower bound above zero;
3. $\mathrm{DSR} \ge$ `dsr_min` and $\mathrm{PBO} \le$ `pbo_max` from `VALIDATION.md`, computed with the true trial count;
4. the result is positive at 2 ticks of slippage per side in the sweep, or the report states the break even slippage and the owner accepts it in `DECISIONS.md`.

If the gate fails, the pipeline stops: no holdout run. The report states the failure plainly.

## 7. Trade log

`artifacts/backtest/trades.parquet`: event id, fold, direction, signal time, entry time and price, exit time and price, exit reason, ambiguous flag, gross ticks, costs in ticks, net ticks, net USD, $\tilde P$ values, $\tau^*$ used.

## 8. Tests

* Hand built scenarios, one per exit reason, plus gap at the open beyond the stop and beyond the target, a same bar double hit resolved by 1 minute data, and an ambiguous double hit booked as a stop.
* Cost arithmetic: a round trip with zero price change and 1 tick slippage per side loses exactly $2 + \frac{2c_{\text{usd}}}{5}$ ticks.
* No lookahead: the entry price uses $O_{t+1}$ only; altering bars after the exit does not change a trade; altering the touch close $C_t$ without changing the barriers does not change the entry.
* One position at a time: no two trades overlap in time; a signal during a trade is skipped and counted.
* Metric formulas against hand computed series (profit factor, drawdown, Sharpe).
* Property: with slippage and commission set to zero and every signal taken, the sum of net ticks equals the sum of label outcomes up to the entry difference (open versus close), within a tolerance.

## 9. Implementation notes and deviations

1. **Barrier scan.** The label stage and the backtest call one function, `vwap_labels::scan_barriers`, so the two cannot drift apart. The label files are bit for bit identical to those written before the function was extracted. A property test shows that, with no costs and an entry at the touch close, the gross result of a trade equals the label outcome of the same event.
2. **Gap rule, generalised.** A stop fills at the worse of its barrier and the open of the bar in which it is hit, a target at the better of its barrier and that open. The design only named the open of bar $t + 1$. A trade that gaps through its stop at the entry open enters and exits at the same price and loses only the costs. The exit time of a barrier exit is the close of the resolving coarse bar.
3. **Position policies.** `one_at_a_time` (a signal at exactly the close of the exit bar is taken), `one_per_session` and `overlap`, as an enum in the configuration. The headline numbers use the configured one, `one_at_a_time` by default. The three are not reported side by side.
4. **Costs and the sweep.** Net ticks are $\text{gross} - 2 n_{\text{slip}} - 2 c_{\text{usd}} / 5$. Barrier hits do not depend on the costs, so the sweep recomputes the net expectancy over the grid of `sweep_slippage` and `sweep_commission` from the same trades (the threshold is not re tuned). The break even slippage is the exact value $(\bar g - 2 c_{\text{usd}}/5)/2$ in ticks per side, with $\bar g$ the mean gross ticks.
5. **Metrics.** Trade count, expectancy with a session clustered bootstrap interval, hit rate, profit factor, daily Sharpe ratio (zero on days without trades, annualised with $\sqrt{252}$ for display only), maximum drawdown on the cumulative USD equity, mean bars held, mean MAE, and the exit mix with the gap and ambiguous counts. The daily net USD series is stored in the strategy summary and feeds the DSR and the PBO.
6. **Gate (section 6) as applied.** Net expectancy positive in at least `min_fold_fraction` (0.6) of the outer folds (a fold without trades counts as not positive), a pooled lower bound above zero (session clustered bootstrap, two sided level `interval_level`), the net expectancy at 2 ticks of slippage per side positive (required, where the design allowed the owner to accept the break even slippage instead), and DSR and PBO within their limits (`VALIDATION.md`).
7. **Not built.** The per trade log `artifacts/backtest/trades.parquet`, the command `vwap backtest run`, the comparison of the model implied expected value with the realised expectancy, and the breakdowns by year, volatility tercile, side and time of day. The strategy summary in `research/strategies/` keeps the per fold results, the pooled metrics, the sweep, the break even slippage and the daily series, which is what the gates and the report need.

