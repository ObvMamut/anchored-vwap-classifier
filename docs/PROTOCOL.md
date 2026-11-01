# Research protocol: the closed loop

This document is the contract for iterating on hypotheses without fooling ourselves. The harness in `crates/vwap_research` enforces it in code. Config: `config/research.toml`. Symbols: `GLOSSARY.md`.

## 1. Why a protocol

The data spans 2016-11-15 to 2025-10-01, about 8.9 years. Every look at the same data raises the chance of a false discovery. The loop therefore uses a three way split, a hard budget of looks, pre registered hypothesis cards, and an append only ledger. A negative result is a valid outcome.

## 2. Data split

By event time (UTC), applied to both the touch time and the label end time of an event. An event whose label spans a boundary is dropped from both sides.

* **Discovery** 2016-11-15 to 2021-09-30. The loop iterates here, freely.
* **Confirmation** 2021-10-01 to 2023-09-30. One shot per promoted hypothesis.
* **Holdout** 2023-10-01 to 2025-10-01. Run once, at the very end, for the single frozen strategy.

Three view types exist and only the first can be built by `vwap research run`: `DiscoveryView`, `ConfirmView`, `HoldoutView`. A `ConfirmView` needs a card with verdict PROMOTE and a recorded hash, and works once per card. A `HoldoutView` exists only in `vwap holdout run`, which writes a lock file and refuses to run twice.

## 3. Hypothesis card

A TOML file in `hypotheses/`, committed to git before it is run. Fields:

* `id` (for example `H001`), `family` (approach, volume, levels, regime), `mechanism` (why the effect should exist, in words), `parent` (the card it refines, if any).
* `outcome`: `rejection_given_resolved` (indicator of class REJECTION among events that hit a barrier; **the default for directional cards**), `rejection` (indicator of class REJECTION over all events, which also moves with the chop rate), or `net_ticks` (outcome in ticks minus round trip cost).

  Why the amendment: the unconditional indicator equals $P(\text{resolved})\cdot P(\text{REJ}\mid\text{resolved})$. Cycle 0 showed that chop is predictable from the clock (`minutes_since_anchor` and `minute_of_day` carry most of the gain) while direction is not. A feature related only to chop, time of day above all, would therefore make a card on the plain indicator look significant. The conditional outcome removes that route. Amended on the owner's decision after cycle 0; the omnibus alpha already spent is unchanged.
* `feature`: one of the registered feature names. `rule`: `top_tercile`, `bottom_tercile`, `above`, `below`, `flag`, or `groups` (a partition into named groups). `cut_points`: frozen numbers, or derived from discovery quantiles at run time and then written to the ledger. `direction`: the predicted sign of the contrast (subset minus complement).
* `timeframes` (5 and 15), `min_effect`, `alpha` (its share of the budget).

The runner computes the SHA256 of the card file and records it. It refuses to run a card whose hash differs from the committed hash recorded at registration.

## 4. Alpha budget

Total $\alpha = 0.05$. The omnibus cycle takes $\alpha_{\mathrm{omni}} = 0.01$. The remaining $0.04$ is split evenly over at most $K = 12$ targeted cards (Bonferroni), $\alpha_{\text{card}} = 0.04 / 12 \approx 0.0033$. The ledger counts cards and alpha spent. The runner returns an error when the budget is spent. The ledger length is the trial count $N_{\text{trials}}$ that feeds the Deflated Sharpe Ratio and PBO in `VALIDATION.md`.

## 5. The cycle

1. **Hypothesize.** Write a card from a mechanism, never from a look at outcomes. Commit it.
2. **Write.** Implement only what the card needs. A new feature gets its `no_lookahead_<feature>` proptest.
3. **Test.** `cargo test`, clippy with warnings as errors, rustfmt, and the harness controls: a planted effect is recovered, shuffled labels reject about $\alpha$ of the time.
4. **Run.** `vwap research run <id> config` on the `DiscoveryView`.
5. **Analyse.** Fixed report (section 7).
6. **Decide and modify** by the rules in section 6 only.

## 6. Verdict rules

Let $\Delta = \hat\pi_{\mathrm{REJ}}(\text{subset}) - \hat\pi_{\mathrm{REJ}}(\text{complement})$ (or the same contrast in mean net ticks for `net_ticks` cards). $p$ is the session clustered bootstrap p value for $H_0: \Delta = 0$, one sided in the card's predicted direction. $n_{\min}$ is the smaller arm size.

* **PROMOTE** if $p \le \alpha_{\text{card}}$, $|\Delta| \ge$ `min_effect`, $\mathrm{sign}(\Delta)$ equals the predicted direction on both timeframes, the sign agrees in at least `promote_min_years_same_sign` of the discovery years with data, and both arms have at least `min_events_per_arm` events.
* **REFINE** if $\alpha_{\text{card}} < p < 0.25$ with the predicted sign on both timeframes. One refinement per mechanism, filed as a new card with `parent` set and its own alpha share. A refinement may change cut points or the subset rule, never the outcome or the data.
* **DROP** otherwise.

Confirmation (`vwap research confirm`) tests each promoted card once on the `ConfirmView` at a one sided $\alpha = 0.05 / k$ with $k$ the number of promoted cards (a Bonferroni share, the conservative form of a Holm adjustment), with the predicted direction, enough events in each arm and **the cut points frozen on discovery**, never recomputed on the confirmation data. The cost adjusted expectancy requirement of `BACKTEST.md` is not enforced for hypothesis cards; strategy cards (section 12) carry the backtest. The single holdout run (`vwap holdout run`) needs `research/FREEZE.toml` naming a confirmed card, a clean git tree and an unused lock file, and applies the same decision rule at $\alpha = 0.05$.

## 7. The fixed analysis report

Each run writes `research/reports/<id>.md` and appends one ledger line. The report states, for each timeframe: arm sizes, $\hat\pi$ in each arm, $\Delta$, simultaneous 95 percent session clustered interval, $p$, the minimum detectable effect, and the sign by discovery year. It also gives the contrast by volatility tercile and by side, the same contrast on the control levels of `BASELINES.md` (difference in differences, as a diagnostic of VWAP specificity, not a promotion condition), the missing value rate of the feature, and a cost adjusted expectancy proxy.

Minimum detectable effect, with session clustering variance inflation $\mathrm{deff}$ taken from the bootstrap:

$$ \mathrm{MDE} \approx \big(z_{1-\alpha/2} + z_{0.8}\big)\sqrt{\mathrm{deff}\cdot\hat p(1-\hat p)\Big(\frac{1}{n_1} + \frac{1}{n_2}\Big)} $$

A card whose `min_effect` is below its MDE is flagged underpowered in the report and cannot be PROMOTED on that basis alone. Claims below the MDE are not made.

## 8. Cycle 0, the omnibus screen

Before targeted cards, one test asks whether there is any predictive structure at all. A LightGBM multiclass model is scored by expanding blocked cross validation inside discovery (5 blocks, purge and embargo at least $T$ bars). The statistic is the log loss improvement over the majority class:

$$ S = \mathrm{LL}_{\text{majority}} - \mathrm{LL}_{\text{model}} $$

The null distribution comes from `omnibus_permutations` (1000) circular shifts of the time ordered label sequence by a seeded offset, which keep the label dependence structure and break its alignment with the features. (The first draft of this document said within session shuffles. That was corrected before any real run, because a within session shuffle preserves between session associations such as volatility regime and class mix, so it is not a valid null for regime features.) $p = \frac{1 + \#\{S^{*} \ge S\}}{1 + B}$, tested at $\alpha_{\mathrm{omni}} = 0.01$. Permutation importance explains a positive result. Multiclass TreeSHAP is available for strategy cards (`MODELING.md`, section 9).

## 9. Stop rules and checkpoints

The loop stops when the budget is spent, or after two consecutive nulls following the omnibus. Then the negative result is reported as the finding. The agent runs cycles on its own and stops to ask the owner only at: a PROMOTE (before confirmation), any change to this protocol, any holdout use, and budget exhaustion.

## 10. Ledger

`research/ledger.jsonl`, append only, one JSON object per cycle: cycle number, card id, card SHA256, git commit, view (discovery or confirm), timeframes, trials added, alpha spent, $\Delta$, $p$, verdict, MDE, UTC time. The pipeline never rewrites or deletes it.

## 11. Commands that check the record

* `vwap research verify config`: the ledger follows this protocol (strictly increasing cycles, every run registered first with an unchanged card, budget respected, confirmations only after a PROMOTE) and every commit it names exists.
* `vwap research replay <id|all> config`: recomputes a card on the discovery view without touching the ledger and compares delta, p value and verdict with the last recorded run, to $10^{-12}$.
* The commands refuse to run if the ledger or the report directory would be ignored by git, because an untracked audit trail disappears on a fresh clone and silently resets the budget.

## 12. Strategy cards: a model plus the backtest

A strategy card answers a different question from a hypothesis card: not "does this feature move the outcome" but "does a calibrated model, traded with a threshold and costs, earn money out of sample". It lives in `hypotheses/S###.toml` and is registered, hashed and counted like a hypothesis card.

* **Fields** (`kind = "strategy"` marks it): `id` (S followed by digits), `mechanism` in words, `model` (`majority`, `logistic` or `lgbm`), `timeframes` and `features` (names, empty means all 34). Every number (the grid, the calibration sizes, the threshold range, the folds, the costs, the gate limits) comes from `config/strategy.toml`, `validation.toml` and `backtest.toml`. The ledger entry records the SHA256 of each file the run read, so `verify` and `replay` notice a changed configuration. A changed configuration is a new study and needs new cards.
* **Running.** `vwap research strategy S001 config` needs a committed, registered card and runs each timeframe once on the `DiscoveryView`: the walk forward of `VALIDATION.md` with the nested scheme of `MODELING.md` inside each fold, the signals above each fold's own threshold, and the backtest of the pooled out of fold signals (`BACKTEST.md`). A timeframe that was already run is refused. The result is written to `research/strategies/S001_15m.json` (tracked: per fold results, pooled metrics, the sweep, the daily net PnL, the reliability bins, importance and a thin sample of contributions) and one `strategy` entry is appended to the ledger.
* **Optional card fields.** `target = "direction"` trains, selects and calibrates on resolved touches (rejection against breakthrough) and still trades every touch; `m1`, `m2`, `horizon_5m` and `horizon_15m` set the barrier shape of the card (the samples are built for it and checked against it); `min_trades_per_week` requires an average number of trades per calendar week over the test windows, constrains the tuned threshold and is a gate. A card without them behaves as the first three strategy cards did.
* **Trials, not alpha.** Each (card, timeframe) is one counted trial for the DSR and the PBO. A strategy run spends no alpha and does not use up the budget of 12 hypothesis cards: its gates are not p values but bootstrap intervals, the DSR and the PBO, and the deflation by the trial count is what protects them.
* **Gates.** A strategy is PROMOTED only if every condition holds, and the ledger lists the failed ones otherwise. In more than half of the folds: $\mathrm{BSS} > 0$ and rejection precision above the base rate. Pooled: the $\mathrm{BSS}$ interval excludes zero and the expected calibration error of every class is at most 0.05. Economic: positive net expectancy in at least 60 percent of the folds, a pooled lower bound above zero, a positive net expectancy at 2 ticks of slippage per side. Overfitting: $\mathrm{DSR} \ge 0.95$ and $\mathrm{PBO} \le 0.2$, both undefined (and failing) until at least 4 strategy series exist and the strategy itself traded. Details in `MODELING.md`, section 9, `BACKTEST.md`, section 9 and `VALIDATION.md`.
* **No runtime self test.** The self test of section 3 belongs to the hypothesis harness. For strategies the same control is the test suite: on a synthetic market a planted signal is promoted, confirmed and held out once, and noise is dropped and cannot be confirmed.
* **Confirmation.** `vwap research confirm S001 config` confirms every timeframe of the card that has a PROMOTE verdict, once each, on the confirmation segment, with the configuration frozen and the models refit on the rolling pool of earlier events. The gates are the same without the DSR and the PBO, with the bootstrap interval of a one sided test at $\alpha = 0.05 / k$, $k$ the number of promoted cards and strategies.
* **Holdout.** `research/FREEZE.toml` names the card and, for a strategy, its `width`. The run needs a `Confirmed` entry, a clean git tree and an unused lock file; the lock is shared with the hypothesis holdout, so only one run of either kind ever happens. The interval level is that of a one sided test at $\alpha = 0.05$.
* **Checks.** `vwap research verify` also checks that each summary file matches the hash in the ledger, that the recorded configuration hashes still match the files, and that the recorded DSR and PBO follow from the series of the earlier trials. `vwap research replay all` recomputes every strategy run and compares the result with the tracked summary to a relative $10^{-9}$. `vwap research dsr config` prints the current DSR and PBO of every series with the trial count of the ledger.

