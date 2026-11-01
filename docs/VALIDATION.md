# Validation

> **Status.** Implemented in `crates/vwap_validation` (purged walk forward splits, blocked inner splits, the cluster bootstrap, Sharpe moments, the Deflated Sharpe Ratio, the Probability of Backtest Overfitting) and in `crates/vwap_research` (the three way split, the holdout lock, the ledger). Configuration: `config/validation.toml` for the geometry and the gate limits. It defines no split dates: the segment boundaries are in `config/research.toml` only. Symbols: `GLOSSARY.md`.

## 1. Event spans

Each labeled event $i$ has a span $[t_i, t_i^{\text{end}}]$: $t_i$ is the close time of the touch bar and $t_i^{\text{end}}$ is the close time of the bar where the label resolved (the barrier bar, or bar $t+T$ for CHOP), from `LABELING.md`. Features use only data at or before $t_i$, but the label uses data up to $t_i^{\text{end}}$. Overlap between a training label span and a test period is the leakage this document prevents.

## 2. Purged walk forward

Defaults (`config/validation.toml`): training window 24 months, test window 6 months, step 6 months, rolling (a fixed length training window that moves forward). `mode = "expanding"` is the alternative. For a range $[R_0, R_1)$ the first test window starts at $a_0 = R_0 + 24$ months and the folds are

$$ a_f = a_0 + 6f\ \text{months},\qquad b_f = a_f + 6\ \text{months},\qquad \text{train range } [a_f - 24\ \text{months},\ a_f) $$

Only full test windows are produced: a window that would end after $R_1$ is dropped. On the discovery segment (15 November 2016 to 30 September 2021) this gives exactly five folds, with test windows from November 2018 to May 2021.

**Purge.** Training events whose label span plus the embargo reaches the test start are removed:

$$ \mathcal P_f = \{\, i : t_i < a_f,\ t_i^{\text{end}} + h \ge a_f \,\} $$

**Embargo.** $h = \texttt{embargo\_horizons} \cdot T \cdot w$ minutes, with `embargo_horizons` at least 1 (the configuration rejects less). With the default $h = T\,w$ the embargo equals the longest label span.

Test labels may extend past $b_f$. That is allowed because the next fold purges against its own test start, and the last fold is bounded by the segment.

**Inner blocked splits.** The nested scheme of `MODELING.md` cuts the training events of an outer fold, in time order, into $K_{\text{in}} + 1$ blocks of about equal size (`inner_blocks`, default 5). Inner fold $k$ predicts block $k$ from the events of the earlier blocks whose span plus the embargo ended before block $k$ starts.

**Confirmation and holdout geometry.** For a strategy confirmed on the confirmation segment, the walk forward range starts $24$ months before the segment, so the first test window starts exactly at the segment start. The training pool is the discovery events plus the confirmation events before each test window, purged by the same rule. The holdout run does the same with the holdout segment and everything before it.

## 3. The three segments and the holdout

The segments, the permits and the lock file are described in `PROTOCOL.md`. In short: runs read the discovery view only, confirmation is one shot per promoted card or strategy, the holdout needs a freeze file naming a confirmed card, a clean git tree and an unused lock file, and runs once. A result on the holdout is reported as it comes out. A change of method afterwards is a new study, which would need new data.

## 4. Trial counting

The append only ledger `research/ledger.jsonl` is the experiment log. A counted trial is

* the omnibus cycle (counted once),
* every distinct hypothesis card run on the discovery view,
* every distinct strategy card and timeframe run on the discovery view (`vwap research strategy`).

A repeated run of the same card is refused for strategies and does not add a trial for hypothesis cards. The grid points and the number of boosting rounds searched inside the nested scheme are not counted one by one: the whole nested search is one trial per strategy and timeframe, and the number of candidates it scored is recorded in the summary (`candidates_scored`). The ledger length is the $N_{\text{trials}}$ of the Deflated Sharpe Ratio.

## 5. Deflated Sharpe Ratio

Reference: Bailey and Lopez de Prado, "The Deflated Sharpe Ratio". Let $\widehat{SR}$ be the daily Sharpe ratio of the net USD series of a strategy over its $T_{\text{obs}}$ evaluated days (zero on days without trades), with sample skewness $\hat\gamma_3$ and kurtosis (non excess) $\hat\gamma_4$. Let $\{\widehat{SR}_n\}$ be the daily Sharpe ratios of all strategy trials on the same calendar.

The expected maximum Sharpe ratio under the null that all trials have zero true Sharpe is

$$ \widehat{SR}^{*} = \sqrt{\mathrm{Var}\big[\{\widehat{SR}_n\}\big]}\ \Big( (1 - \gamma)\,\Phi^{-1}\!\Big(1 - \frac{1}{N_{\text{trials}}}\Big) + \gamma\,\Phi^{-1}\!\Big(1 - \frac{1}{N_{\text{trials}}\,e}\Big)\Big) $$

with $\gamma \approx 0.5772$. The deflated statistic is

$$ \mathrm{DSR} = \Phi\!\left(\frac{\big(\widehat{SR} - \widehat{SR}^{*}\big)\sqrt{T_{\text{obs}} - 1}}{\sqrt{1 - \hat\gamma_3\,\widehat{SR} + \dfrac{\hat\gamma_4 - 1}{4}\,\widehat{SR}^{2}}}\right) $$

Sharpe ratios are per day inside the formula. Reference values used by the unit tests, computed with an independent implementation of the normal quantile: the bracket of $\widehat{SR}^{*}$ is $0.5198$ for $N = 2$, $1.5746$ for $N = 10$, $2.5306$ for $N = 100$ and $3.2551$ for $N = 1000$; $\mathrm{PSR}(0.2, 0, 101, 0, 3) = 0.97616$; $\mathrm{DSR}(0.5, 0.04, 10, 250, -0.5, 5) = 0.99145$. With $N_{\text{trials}} = 1$ the correction is zero and the DSR equals the probabilistic Sharpe ratio.

Two choices that the paper leaves open and this project fixes:

* $N_{\text{trials}}$ is the ledger count. Hypothesis cards count in it although they have no PnL series, which is conservative.
* The variance of the Sharpe ratios is the sample variance over the strategy series that exist. With a single series it falls back to the null value $1/(T_{\text{obs}} - 1)$, and the summary says so. A strategy that abstained everywhere has a daily Sharpe ratio of 0 and a DSR near one half.

The pass level is `dsr_min` (0.95).

## 6. Probability of Backtest Overfitting

Reference: Bailey, Borwein, Lopez de Prado and Zhu, combinatorially symmetric cross validation (CSCV).

1. Build the matrix $M$ of size $T_{\text{obs}} \times N$ of daily net USD results of the strategy series on a common calendar. Series on a different calendar are left out and the summary says so. Series that abstained throughout stay in as trials with zero PnL.
2. Cut the rows into $S$ contiguous blocks of equal size (`pbo_blocks`, default 16, even).
3. For each of the $\binom{S}{S/2}$ ways $c$ to choose $S/2$ blocks as in sample (IS) with the rest out of sample (OOS), find the trial with the best IS Sharpe ratio and its OOS rank among all trials, $\mathrm{rank}_c \in \{1, \dots, N\}$ with 1 the worst, ties by average rank:

$$ \omega_c = \frac{\mathrm{rank}_c}{N + 1}, \qquad \lambda_c = \ln\frac{\omega_c}{1 - \omega_c} $$

4. The probability of backtest overfitting is the share of combinations where the IS winner falls at or below the OOS median:

$$ \mathrm{PBO} = \frac{1}{\binom{S}{S/2}}\sum_{c}\mathbf 1\big[\lambda_c \le 0\big] $$

The PBO is undefined, and reported as such, when there are fewer than `pbo_min_trials` (4) series, when there are too few days for the blocks, or when the strategy under test made no trades at all (its zeros would tie with every other idle series and the ranks would be arbitrary). An undefined PBO fails the gate. The pass level is `pbo_max` (0.2).

## 7. Breakdowns

Implemented: every metric per outer fold, so the variation from fold to fold is visible, and the pooled values. Specified but not built: breakdowns by calendar year, by volatility tercile, by side and by time of day bucket. The per fold table covers the question that matters most for a drifting relationship.

## 8. Tests

All reference values are derived by hand in the test docstrings or computed independently. No Python is used in the repository.

* Moments of a series against hand computations, constant and short series.
* DSR: the constants of section 5; $\mathrm{DSR}$ falls as $N$ grows; the PSR identity at $N = 1$.
* PBO: pure noise gives a value near one half, one dominant column gives zero, fewer than four trials or an odd block count is undefined.
* Splits (property tests): no training span overlaps a test window after the embargo, folds increase strictly, every purged event satisfies the purge rule, inner folds are chronological and purged, and the discovery range gives exactly five folds.
* Cluster bootstrap: reproducible for one seed, centred on the statistic, whole clusters are drawn together.
* The holdout lock: a second run returns an error (`PROTOCOL.md`).
