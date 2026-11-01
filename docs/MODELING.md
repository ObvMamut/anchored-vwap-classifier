# Modeling

> **Status.** Implemented in `crates/vwap_model`, driven by `vwap research strategy` (see `PROTOCOL.md`, section 12). Where the implementation differs from the design below, section 9 says so. Configuration: `config/strategy.toml` (models, calibration, threshold) and `config/validation.toml` (folds, inner blocks, gate limits). Symbols: `GLOSSARY.md`. Splits and trial counting are in `VALIDATION.md`.

## 1. Models, in this order

Each model is evaluated on the same walk forward folds and the same events. A later model is only interesting if it beats the earlier one on calibrated metrics.

1. **Majority class.** Predicts the training fold class frequencies $\hat\pi_c$ for every event. This is the base rate benchmark for all metrics.
2. **Multinomial logistic regression** (`linfa-logistic`, L2 penalty, strength from a small grid). Inputs standardised with training fold statistics, missing values imputed with training fold medians plus missing indicators, heavy tails winsorised (`FEATURES.md`, section 7). The exact `linfa-logistic` multinomial API is verified in the Phase 1 spike.
3. **LightGBM multiclass** (`lightgbm3`): `objective = multiclass`, `num_class = 3`, `metric = multi_logloss`, small pre registered grid over `num_leaves`, `learning_rate`, `min_data_in_leaf`, `lambda_l2`, `feature_fraction`, `bagging_fraction`. Deterministic settings from `ARCHITECTURE.md`. Early stopping uses validation folds only.

Neural networks are out of scope unless LightGBM shows a robust edge. "Robust edge" means: it passes the Phase 5 gate (section 6) in a majority of folds, and the DSR and PBO limits in `VALIDATION.md`. Until then no neural network code is written.

## 2. Nested scheme inside each outer training window

Events are scarce. A 3 year window holds a few thousand events at best, so a fixed split into fit, early stopping, calibration and threshold sets would leave a few hundred events each. Instead, each outer training window runs a **nested, cross fitted** procedure. Nothing in it touches the outer test window.

Let the outer training events (after purge and embargo, `VALIDATION.md`) be ordered by time and cut into $K_{\text{in}} + 1$ blocks $B_0, B_1, \dots, B_{K_{\text{in}}}$ of about equal size (config `inner_blocks`, default 5).

1. **Inner expanding folds.** For $k = 1, \dots, K_{\text{in}}$: fit on $B_0 \cup \dots \cup B_{k-1}$ after purging events whose label span plus embargo reaches the first event of $B_k$, and predict $B_k$. This yields out of fold probabilities $Q_k$ for $B_1, \dots, B_{K_{\text{in}}}$.
2. **Early stopping and hyperparameters.** For each grid point, record the multiclass log loss on each $B_k$ after each boosting round. Average the curves across $k$ (weighted by block size). The round count $r^*$ minimises the averaged curve, and the grid point with the lowest averaged minimum is selected. The test fold never enters. All predictions $Q_k$ are re generated with the selected grid point truncated at $r^*$. $r^*$ is one scalar chosen from the same data as the predictions, a small and documented optimism.
3. **Cross fitted calibration.** For each block $k$, the calibrator is fit on the out of fold predictions of the other blocks, after removing events within the embargo of block $k$'s boundaries, then applied to $Q_k$. This gives honest calibrated probabilities $\tilde Q_k$ for tuning $\tau$. Config `calibration_order = "chronological"` restricts the calibrator for block $k$ to blocks before $k$ (block 1 is then left out of tuning).
4. **Threshold.** $\tau$ is tuned on $\tilde Q_1, \dots, \tilde Q_{K_{\text{in}}}$ (section 5).
5. **Final fit.** Refit the selected model on the entire outer training window with $r^*$ rounds. Fit the final calibrator on all out of fold predictions $Q$. Predict the outer test window, calibrate, and apply $\tau$.

The first block $B_0$ never receives an out of fold prediction, so it is used only for fitting. For the logistic model the same scheme tunes the penalty. The majority model needs no nesting.

Known bias: out of fold models are fit on less data than the final model, so calibration is slightly conservative. This is documented in the report and checked by the reliability diagrams on the outer test windows.

## 3. Calibration

Applied to each class score one versus rest, then rows are renormalised to sum to 1:

$$ \tilde p_{nk} = \frac{g_k(\hat p_{nk})}{\sum_{k'} g_{k'}(\hat p_{nk'})} $$

* **Isotonic** (pool adjacent violators, hand written). For scores sorted ascending with binary targets $y_i$ and weights $w_i$ the fit solves

$$ \min_{f_1 \le f_2 \le \dots \le f_n}\ \sum_{i=1}^{n} w_i\,(y_i - f_i)^2 $$

  Ties in the score are merged first. Prediction is linear interpolation between block means and clamps outside the training score range. Output is clipped to $[\epsilon, 1 - \epsilon]$ with $\epsilon = 10^{-6}$.
* **Platt**: $g(q) = \dfrac{1}{1 + e^{a\,q + b}}$ with $q$ the logit of the raw probability, fit by Newton iterations with the Platt target smoothing. Platt is the default when the calibration sample has fewer than `isotonic_min_events` (default 1000) events, because isotonic overfits small samples.

Checks:

* **Reliability diagram** per class: 10 quantile bins of predicted probability against observed frequency, with session clustered bootstrap bands, on the outer test windows pooled.
* **Brier score** (multiclass): $\mathrm{BS} = \dfrac{1}{N}\sum_{n=1}^{N}\sum_{k=1}^{3}(\hat p_{nk} - y_{nk})^2$ with $y_{nk}$ the one hot label.
* **Expected calibration error** per class: $\mathrm{ECE}_k = \sum_{b}\dfrac{|I_b|}{N}\,\big|\bar y_{b} - \bar p_{b}\big|$ over quantile bins $I_b$.
* **Log loss**: $-\dfrac{1}{N}\sum_{n}\sum_{k} y_{nk}\ln \hat p_{nk}$, probabilities clipped at $10^{-12}$.
* **Brier skill score** against the majority model on the same events: $\mathrm{BSS} = 1 - \dfrac{\mathrm{BS}}{\mathrm{BS}_{\text{base}}}$.

## 4. Metrics on the outer test windows

Computed per fold and pooled, never on training or inner data: log loss, $\mathrm{BS}$, $\mathrm{BSS}$, $\mathrm{ECE}_k$, class wise AUC, rejection precision at $\tau$ and its lift over the base rate $\dfrac{\text{precision}}{\hat\pi_{\text{REJ}}}$, trade count at $\tau$, coverage $=\dfrac{\#\{\hat P_{\text{REJ}} > \tau\}}{N}$. Ambiguous events are excluded.

Breakdowns by year, vol regime tercile, timeframe, side and time of day bucket (`VALIDATION.md`).

## 5. Threshold $\tau$

Rule: trade only when $\tilde P(\text{rejection}) > \tau$, otherwise abstain.

The per event net outcome in ticks for taking the trade, from the label record (`LABELING.md`), is

$$ g_i = \begin{cases} \dfrac{m_1\sigma_i}{\delta} & y_i = \text{REJ} \\[4pt] -\dfrac{m_2\sigma_i}{\delta} & y_i = \text{BRK} \\[4pt] R_{T,i} & y_i = \text{CHOP} \end{cases} \qquad \text{net}_i = g_i - \kappa $$

with round trip cost $\kappa = 2 n_{\text{slip}} + \dfrac{2 c_{\text{usd}}}{5}$ ticks. This is a proxy: it enters at the touch close rather than the next open. The backtest is the authority.

Reference identity for a calibrated model: the expected net value of trading an event is

$$ \mathbb E[\text{net}] = \sum_{c} \tilde P_c\,\mu_c - \kappa $$

with $\mu_c$ the training mean of $g$ within class $c$. This shows why a threshold on $\tilde P_{\text{REJ}}$ alone is a simplification: the other classes carry different payoffs. The rule stays as specified. The identity is reported for diagnostics.

Tuning: over a grid of $\tau$ values (config, default 0.35 to 0.80 in steps of 0.01), compute on the honest calibrated out of fold probabilities $\tilde Q$ the mean of $\text{net}_i$ over events with $\tilde P_{\text{REJ},i} > \tau$. Choose

$$ \tau^* = \arg\max_{\tau}\ \mathrm{LCB}_{80}\Big(\overline{\text{net}}(\tau)\Big) \quad \text{subject to}\quad n_{\text{trades}}(\tau) \ge n_{\min} $$

where $\mathrm{LCB}_{80}$ is the 20th percentile of a session clustered bootstrap of the mean, and $n_{\min}$ is `min_trades_tau` (default 100). If no $\tau$ satisfies the constraint, the fold abstains entirely ($\tau = 1$) and this is reported. $\tau$ is never tuned on the outer test window or the holdout. Each fold chooses its own $\tau^*$ from its own training window. The grid is one trial family in the log.

## 6. Calibrated lift gate (Phase 5)

Pass only if, for the best model on the primary label cell and for at least one timeframe, in a **majority of outer test folds** (strictly more than half):

1. $\mathrm{BSS} > 0$, with the session clustered bootstrap 95 percent interval of the pooled $\mathrm{BSS}$ excluding zero, and
2. rejection precision at $\tau^*$ exceeds the base rate $\hat\pi_{\text{REJ}}$ of the same fold, and
3. the reliability diagram shows no class with $\mathrm{ECE}_k$ above `max_ece` (default 0.05).

If the gate fails, the pipeline stops and `reports/gate_model.md` records the numbers. No new features or labels are tried afterwards to rescue a result within the same study.

## 7. Explainability

* **LightGBM TreeSHAP** through `pred_contrib` (exact, built in). Output per row has $K(F+1)$ entries: for each class, $F$ feature contributions and a bias term $\phi_0$. Additivity test: for every row and class,

$$ \phi_0 + \sum_{f=1}^{F}\phi_f = \text{raw score} $$

  within $10^{-9}$. If the binding does not expose it, the call goes through the C API with the contribution flag.
* **Summaries**: mean absolute contribution per class and feature on each outer test window, $\overline{|\phi|}_{kf} = \dfrac{1}{N}\sum_n |\phi_{nkf}|$; a hand drawn beeswarm; grouped totals (approach, volume, context, levels, regime).
* **Logistic regression**: contribution of feature $f$ to class $k$ is $\beta_{kf}\,x_f$ on standardised inputs, which is exact in logit space.
* **Feature stability across folds**: for folds $a$ and $b$ with importance vectors $I^{(a)}, I^{(b)}$, report the Spearman rank correlation $\rho_{ab}$ averaged over fold pairs, the overlap of top $m$ features $\dfrac{|\text{top}_m(a) \cap \text{top}_m(b)|}{m}$, and for each feature the share of folds in which it is in the top $m$. Features with high importance in few folds are flagged as unstable.
* Importances and contributions are computed on test windows for reporting only. They never feed back into feature selection within a study.

## 8. Tests

* Isotonic: the fit is non decreasing; the weighted mean of fitted values equals the weighted mean of targets; reference example in `VALIDATION.md`.
* Brier, log loss and ECE against hand computed small examples.
* Platt: recovers known parameters on synthetic data generated from a Platt model.
* Majority model: probabilities equal training frequencies and sum to 1.
* Probabilities from any model sum to 1 within $10^{-12}$ and lie in $[0,1]$.
* SHAP additivity (section 7) on a fixed small model.
* Determinism: two fits with the same seed give identical probabilities bit for bit.
* Nested scheme: no inner training event has a label span reaching its validation block's first event plus embargo, and no outer test event is ever visible to any inner fold.

## 9. Implementation notes and deviations

What was built, and where it differs from sections 1 to 8.

1. **Majority and logistic.** The logistic model is `linfa-logistic` (multinomial, L2, penalty from `logistic_l2_grid`, chosen by the inner folds). Inputs are prepared on the training window only: every column with at least one missing value gets a missing indicator column and a median imputation (the design said a share above 1 percent; any missing value is simpler and costs nothing), values are winsorised at the 1st and 99th percentile and standardised.
2. **LightGBM.** The pre registered grid of 8 points is in `strategy.toml`. The number of rounds is not found by early stopping on a validation set. One booster per grid point and inner fold is trained for `lgbm_max_rounds` (300) and scored every `lgbm_round_step` (10) rounds, and the pair (grid point, rounds) with the lowest block size weighted out of fold log loss is selected. Settings are deterministic: one thread, row wise histograms, every seed derived from the master seed.
3. **Nested scheme.** As in section 2, except that `calibration_order = "chronological"` was not built. The calibrator for block $k$ is always fit on the out of fold predictions of all the other blocks, after removing the events whose span plus the embargo overlaps block $k$.
4. **Calibration.** Isotonic regression (pool adjacent violators, linear interpolation, clamping and clipping to $[10^{-6}, 1 - 10^{-6}]$) when the calibration sample has at least `isotonic_min_events` (1000) events, Platt scaling by Newton iterations with target smoothing otherwise. One versus rest, then rows are renormalised.
5. **Threshold.** As in section 5, with $\kappa$ taken from `backtest.toml` (1 tick of slippage and 2 USD of commission per side, 2.8 ticks). One deviation, on by default (`tau_require_positive_lcb`): a fold abstains unless its best threshold has a bootstrap lower bound above zero. Without it a useless model still trades at its least bad threshold, which only adds noise to the Sharpe ratios that feed the DSR. A synthetic test shows the effect.
6. **Gate.** Section 6 is applied to each (strategy, timeframe) as follows, and the verdict lists every failed condition. In more than half of the outer folds: $\mathrm{BSS} > 0$ against the training frequencies of the same fold, and rejection precision at that fold's $\tau$ above the base rate of the fold (a fold that abstained fails). Pooled over the folds: the session clustered bootstrap interval of the $\mathrm{BSS}$ excludes zero, and the expected calibration error of every class is at most `max_ece` (the pooled reliability diagram, as the design says, not each fold on its own). The economic conditions and the DSR and PBO are in `BACKTEST.md`, section 6.
7. **Reliability diagrams.** Pooled quantile bins per class, without bootstrap bands. The bands were not built.
8. **TreeSHAP.** `crates/vwap_model/src/shap.rs`, exact path dependent TreeSHAP in safe Rust from the text model, because `lightgbm3` 1.0.11 sizes the multiclass `predict_contrib` buffer wrongly (`DECISIONS.md`, section 7). It agrees with LightGBM's own `predict_contrib` on a binary model to $10^{-9}$ (missing values and exact zeros included), and on a multiclass model the contributions and the bias add up to the raw score of every class to $10^{-9}$. The logistic model is explained exactly by coefficient times standardised value. Summaries keep the mean absolute contribution per class and feature, the stability across folds (mean Spearman rank correlation and the mean overlap of the top 5), and a thin sample of events (`contrib_rows_per_fold`) for the beeswarm.
9. **Not built.** The grouped importance totals (approach, volume, context, levels, regime) and the flag for features that are important in few folds.
10. **Tests.** Isotonic and Platt (reference examples, monotone fit, weighted mean preserved, planted Platt parameters recovered), Brier, log loss and ECE against hand values, the majority forecast, probabilities that sum to one, determinism (two fits give identical bits), the nested scheme end to end on synthetic data (a planted signal beats the majority and noise does not, a too small window is an error, no threshold on noise), TreeSHAP as in item 8.
11. **Direction target and trade rate.** With `target = "direction"` the models are fit, scored on the inner folds and calibrated on touches that resolve only, and then predict every touch; the threshold is tuned on net ticks over all touches, chop included, and the Brier skill and the calibration error are computed on resolved touches with the probabilities renormalised over rejection and breakthrough. `min_trades_per_week` raises the minimum number of selected touches of the threshold search to $\lceil r W \rceil$, with $r$ the rate and $W$ the weeks the out of fold events span.

