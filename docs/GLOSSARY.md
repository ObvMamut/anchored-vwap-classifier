# Glossary of symbols and units

One notation for every document. If a symbol is used in a doc it means exactly this.

## Time and bars

* $w \in \{5, 15\}$: bar width in minutes. 1 minute bars are the base data.
* $i, j, u$: bar indices within one timeframe. $t$ is the touch bar.
* $s_i$: open time of bar $i$. The bar is known at its close $s_i + w$.
* $t_0$: anchor time of a session (Globex 18:00 or RTH 09:30, America/New_York).
* $d$: trading date of a session. $d-1$ is the previous trading date.
* $\text{slot}(t)$: the New York clock time of bar $t$ at resolution $w$.

## Prices and volume

* $O_i, H_i, L_i, C_i$: open, high, low, close. $v_i$: volume (the `Volume` column). $\mathrm{TickVolume}_i$: the tick count column.
* $p_i = \frac{H_i + L_i + C_i}{3}$: typical price.
* $\mathrm{VWAP}_t$: anchored VWAP from $t_0$ to $t$.
* $\sigma_{\mathrm{VWAP},t}$: volume weighted standard deviation of $p_i$ around $\mathrm{VWAP}_t$.
* $\mathrm{TR}_t, \mathrm{ATR}_t$: true range and Wilder average true range of length $n_{\mathrm{ATR}}$.
* $\sigma_t$: volatility scale used by labels and features. Default $\sigma_t = \mathrm{ATR}_t$.
* $\delta = 0.25$ index points: tick size. A tick is worth 5 USD. A full point is worth 20 USD.

## Events and labels

* $s_t \in \{+1, -1\}$: approach side. $+1$ means price came from above the VWAP.
* $k$: re arm excursion in ATR units. $N_{\mathrm{ex}}$: minutes excluded after the anchor.
* $P_0 = C_t$: label reference price.
* $m_1, m_2$: rejection and breakthrough barrier multiples. $T$: vertical barrier in bars.
* $B^{\text{rej}}, B^{\text{brk}}$: barrier price levels. $t_b$: bar at which the first barrier is hit.
* $c \in \{\text{REJ}, \text{BRK}, \text{CHOP}\}$: class. $K = 3$ classes.
* $\mathrm{MAE}$: maximum adverse excursion in units of $\sigma_t$.
* $t_i^{\text{end}}$: end time of the label span of event $i$.

## Features

* $L$: lookback in bars. $d_j = s_t (C_j - \mathrm{VWAP}_j)$: signed distance, positive on the approach side.
* $\mathrm{RV}_t$: relative volume. $\mathrm{rv}_t$: realised volatility. $G_t$: overnight gap.
* $d^{\ell}_t$: distance to a level $\ell$ in units of $\sigma_t$, positive when the level lies in the rejection direction.

## Models and validation

* $\hat P_c$: calibrated probability of class $c$. $\tau$: trade threshold on $\hat P_{\text{REJ}}$.
* $\mathrm{BS}$: multiclass Brier score. $\mathrm{BSS}$: Brier skill score.
* $a_f, b_f$: start and end of test window of fold $f$. $h$: embargo length, at least $T$ bars.
* $N_{\text{trials}}$: number of counted trials. $T_{\text{obs}}$: number of return observations. These differ from $N_{\mathrm{ex}}$ and from the vertical barrier $T$ on purpose.
* $\widehat{SR}$: Sharpe ratio estimate. $\mathrm{DSR}$: Deflated Sharpe Ratio. $\mathrm{PBO}$: Probability of Backtest Overfitting.
* $\Phi$, $\Phi^{-1}$: standard normal CDF and quantile function. $\gamma \approx 0.5772$: Euler Mascheroni constant.

## Costs

* $n_{\text{slip}}$: slippage in ticks per side. $c_{\text{usd}}$: commission in USD per side. $\kappa = 2 n_{\text{slip}} + \frac{2 c_{\text{usd}}}{5}$: round trip cost in ticks.
