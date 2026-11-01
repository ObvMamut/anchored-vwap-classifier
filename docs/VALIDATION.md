# Validation

> **Status.** Implemented: the three way split, the holdout lock and the experiment ledger (see `PROTOCOL.md`). Specification only, not implemented: purged walk forward splits, the Deflated Sharpe Ratio and the Probability of Backtest Overfitting. They need a strategy to evaluate, and none was promoted.

The research loop of `PROTOCOL.md` implements the split and ledger inside `vwap_research`. The crate `vwap_validation` named here was never built. Symbols: `GLOSSARY.md`.

## 1. Event spans

Each labeled event $i$ has a span $[t_i, t_i^{\text{end}}]$: $t_i$ is the close time of the touch bar and $t_i^{\text{end}}$ is the close time of the bar where the label resolved (the barrier bar, or bar $t+T$ for CHOP), from `LABELING.md`. Features use only data at or before $t_i$, but the label uses data up to $t_i^{\text{end}}$. Overlap between a training label span and a test period is the leakage this document prevents.

## 2. Purged walk forward

Defaults (config): training window 36 months, test window 6 months, step 6 months, rolling (a fixed length training window that moves forward). Config `train_mode = "expanding"` is an option. Let the first test start be $a_0$ and the folds be

$$ a_f = a_0 + 6f\ \text{months},\qquad b_f = a_f + 6\ \text{months},\qquad \text{train range } [a_f - 36\ \text{months},\ a_f) $$

Training set for fold $f$ before purging: events with $t_i \in [a_f - 36\ \text{months},\ a_f)$. Test set: events with $t_i \in [a_f, b_f)$.

**Purge.** Remove from the training set every event whose label span plus embargo reaches the test start:

$$ \mathcal P_f = \{\, i : t_i < a_f,\ t_i^{\text{end}} + h \ge a_f \,\} $$

**Embargo.** $h$ is the embargo duration, at least $T$ bars of the event's timeframe:

$$ h \ge T \cdot w \ \text{minutes} $$

Config validation rejects a smaller value. The default is $h = T\,w$ plus one full session length when the embargo would otherwise end inside a session, so that no training event shares a session with a test event at the boundary.

The purge also applies to test events: a test event whose features look back into a period used for training is fine, because features use past data only. Test labels may extend past $b_f$; that is allowed, because the next fold's training set purges against its own test start, and the final fold's test labels are bounded by the end of the development range.

The data spans about 8.9 years. Under the three way split of `PROTOCOL.md` the research loop works inside a discovery segment of about 4.9 years and uses blocked cross validation. The rolling 3 year train and 6 month test scheme above applies to the final model, trained on discovery plus confirmation (about 6.9 years) and tested on the holdout. Exact counts are computed from the data and written to the validation report.

The inner splits in `MODELING.md` use the same routine with blocks in place of fold windows.

## 3. Final untouched holdout

* The holdout is the **last 2 years** of the cleaned data. Its start is $H = \max(\text{timestamp}) - 24\ \text{months}$, rounded down to a session boundary.
* Development events must satisfy $t_i^{\text{end}} + h < H$. Events whose span reaches into the holdout are dropped from development.
* **Type level protection.** All development stages obtain data through a `DevData` view that physically excludes holdout rows. There is no function in a development crate that can load holdout rows. The holdout loader (`HoldoutData`) is available only to `vwap holdout run`.
* **Run once.** Before the first holdout run, the owner freezes: label cell, feature set, model family, hyperparameter grid and selection rule, calibration method, $\tau$ rule, cost model. `vwap holdout run` computes the config hash, requires a clean git tree, writes `artifacts/holdout/holdout.lock` containing the hash, the commit, the time and the trial count, then runs. It refuses to run if the lock exists, even with a different config.
* **What the run does.** Train on the last 36 months of development data (with purge and embargo against $H$) using the same nested procedure, apply to events in $[H, \text{end}]$, simulate trades with the frozen cost model, and report. There is no tuning, no second run, and no peeking at partial results.
* If the holdout result is poor, it is reported as is. A change to the method afterwards is a new study with a new holdout, which does not exist for this data. This is stated in the report.

## 4. Experiment log and trial counting

Every run that fits a model or evaluates a strategy appends one JSON line to `artifacts/experiments/experiments.jsonl` (append only; the file is never edited by the pipeline):

* `trial_id`, UTC time, git commit, config hash, master seed
* timeframe, label cell $(m_1, m_2, T, \sigma\text{ type})$, `vwap_reference`, anchor kind
* feature set (names and lookbacks), model family and hyperparameter point, calibration method, $\tau$ rule
* summary metrics: pooled log loss, $\mathrm{BSS}$, trade count, net expectancy, Sharpe
* a pointer to the per fold daily PnL series used for DSR and PBO

**What counts as a trial.** Each distinct tuple of the fields above for which an out of sample result was computed. In particular: each label grid cell run through a model, each timeframe, each feature set, each hyperparameter grid point evaluated with an outer test result, each calibration choice, each $\tau$ rule variant, each anchor kind, each value of `vwap_reference`. The trial count $N_{\text{trials}}$ is the number of distinct tuples in the log. Runs that only repeat a tuple (for example a determinism check) are marked `repeat` and not counted. Hyperparameter points evaluated only inside the nested inner validation (never on an outer test window) are not counted individually; the whole nested search is one trial per outer configuration. Descriptive label grid statistics without a model are not trials, but see `LABELING.md` for the rule when a cell is run through a model.

The log is the single source for the counts shown in the report.

## 5. Deflated Sharpe Ratio

Reference: Bailey and Lopez de Prado, "The Deflated Sharpe Ratio". Let $\widehat{SR}$ be the Sharpe ratio estimate of the selected strategy on its $T_{\text{obs}}$ daily net returns, with sample skewness $\hat\gamma_3$ and kurtosis (non excess) $\hat\gamma_4$. Let $\{\widehat{SR}_n\}$ be the Sharpe ratios of all $N_{\text{trials}}$ trials, on the same time base.

Expected maximum Sharpe ratio under the null that all trials have zero true Sharpe:

$$ \widehat{SR}^{*} = \sqrt{\mathrm{Var}\big[\{\widehat{SR}_n\}\big]}\ \Big( (1 - \gamma)\,\Phi^{-1}\!\Big(1 - \frac{1}{N_{\text{trials}}}\Big) + \gamma\,\Phi^{-1}\!\Big(1 - \frac{1}{N_{\text{trials}}\,e}\Big)\Big) $$

with $\gamma \approx 0.5772$ the Euler Mascheroni constant. The deflated statistic is

$$ \mathrm{DSR} = \Phi\!\left(\frac{\big(\widehat{SR} - \widehat{SR}^{*}\big)\sqrt{T_{\text{obs}} - 1}}{\sqrt{1 - \hat\gamma_3\,\widehat{SR} + \dfrac{\hat\gamma_4 - 1}{4}\,\widehat{SR}^{2}}}\right) $$

Sharpe ratios are per period (daily), not annualised, inside the formula. $\mathrm{DSR}$ is the probability that the true Sharpe exceeds zero after accounting for the number of trials. Reporting rule: $\mathrm{DSR}$ is reported with $N_{\text{trials}}$, $\mathrm{Var}[\{\widehat{SR}_n\}]$, $T_{\text{obs}}$, $\hat\gamma_3$, $\hat\gamma_4$. A threshold of 0.95 (config `dsr_min`) is the pass level. If the variance of trial Sharpes cannot be estimated (fewer than 2 trials), the report says so and does not print a DSR.

## 6. Probability of Backtest Overfitting

Reference: Bailey, Borwein, Lopez de Prado and Zhu, combinatorially symmetric cross validation (CSCV).

1. Build the matrix $M$ of size $T_{\text{obs}} \times N_{\text{trials}}$ of daily net returns of all trials on a common calendar of out of sample days.
2. Split the rows into $S$ contiguous blocks of equal size (config `pbo_blocks`, default 16, even).
3. For each of the $\binom{S}{S/2}$ ways $c$ to choose $S/2$ blocks as in sample (IS) with the rest out of sample (OOS): compute each trial's Sharpe ratio on the IS rows, find the best trial $n^{*}_c$, and compute the OOS rank of $n^{*}_c$ among all trials, $\mathrm{rank}_c \in \{1, \dots, N_{\text{trials}}\}$ with 1 the worst. Relative rank and logit:

$$ \omega_c = \frac{\mathrm{rank}_c}{N_{\text{trials}} + 1}, \qquad \lambda_c = \ln\!\frac{\omega_c}{1 - \omega_c} $$

4. The probability of backtest overfitting is the share of combinations where the IS winner falls at or below the OOS median:

$$ \mathrm{PBO} = \frac{1}{\binom{S}{S/2}}\sum_{c}\mathbf 1\big[\lambda_c \le 0\big] $$

Pass level: $\mathrm{PBO} < 0.5$, and preferably below 0.2 (config `pbo_max`, default 0.2). With fewer than 4 trials PBO is not meaningful and the report says so. Ties in rank use average ranks.

## 7. Breakdowns

Every metric table is reported for:

* each calendar year of the development range (and the holdout once, separately);
* each volatility regime tercile of $\mathrm{rvpct}$ (tercile edges from training folds only);
* 5 minute versus 15 minute bars;
* side ($s_t = +1$ and $-1$) and time of day bucket;
* each outer fold, so fold to fold variation is visible.

Small cells (under 30 events) are shown with their count and no interval.

## 8. Unit tests with reference values

All reference values are derived by hand in the test docstrings or taken from the cited papers. No Python.

* **PAV**: input targets $y = (1, 3, 2, 4, 3, 5)$ with unit weights give the fit $(1,\ 2.5,\ 2.5,\ 3.5,\ 3.5,\ 5)$. A second case $y = (5, 4, 3, 2, 1)$ gives the constant fit $(3, 3, 3, 3, 3)$. Weighted case $y = (2, 1)$, $w = (1, 3)$ gives $(1.25, 1.25)$.
* **Brier**: forecasts $(0.9, 0.1, 0.0)$ for a true class 0 give $0.01 + 0.01 + 0 = 0.02$. A uniform forecast over 3 classes gives $\frac{2}{3}$ per event.
* **DSR**: the worked example of the paper, and degenerate cases: with $N_{\text{trials}} = 1$ the correction term is zero and $\mathrm{DSR}$ equals the probabilistic Sharpe ratio; $\mathrm{DSR}$ falls as $N_{\text{trials}}$ grows with all else fixed.
* **PBO**: a matrix of pure noise columns gives $\mathrm{PBO}$ near 0.5 (tolerance from the number of combinations); a matrix where one column dominates every block gives $\mathrm{PBO} = 0$.
* **Purged splits** (property tests): no training span overlaps a test window after embargo; fold windows increase strictly; no event appears in both train and test of a fold; every purged event satisfies the purge rule; the holdout start is never inside a development fold.
* **Holdout lock**: a second `holdout run` returns an error; `DevData` cannot yield a row with timestamp at or after $H$.
