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

Confirmation tests each promoted card once on the `ConfirmView` at one sided $\alpha = 0.05$, Holm adjusted over the promoted cards, with the same direction and cut points. A confirmed card must also have a positive cost adjusted expectancy, from the backtest of `BACKTEST.md`, to count.

## 7. The fixed analysis report

Each run writes `reports/research/<id>.md` and appends one ledger line. The report states, for each timeframe: arm sizes, $\hat\pi$ in each arm, $\Delta$, simultaneous 95 percent session clustered interval, $p$, the minimum detectable effect, and the sign by discovery year. It also gives the contrast by volatility tercile and by side, the same contrast on the control levels of `BASELINES.md` (difference in differences, as a diagnostic of VWAP specificity, not a promotion condition), the missing value rate of the feature, and a cost adjusted expectancy proxy.

Minimum detectable effect, with session clustering variance inflation $\mathrm{deff}$ taken from the bootstrap:

$$ \mathrm{MDE} \approx \big(z_{1-\alpha/2} + z_{0.8}\big)\sqrt{\mathrm{deff}\cdot\hat p(1-\hat p)\Big(\frac{1}{n_1} + \frac{1}{n_2}\Big)} $$

A card whose `min_effect` is below its MDE is flagged underpowered in the report and cannot be PROMOTED on that basis alone. Claims below the MDE are not made.

## 8. Cycle 0, the omnibus screen

Before targeted cards, one test asks whether there is any predictive structure at all. A LightGBM multiclass model is scored by expanding blocked cross validation inside discovery (5 blocks, purge and embargo at least $T$ bars). The statistic is the log loss improvement over the majority class:

$$ S = \mathrm{LL}_{\text{majority}} - \mathrm{LL}_{\text{model}} $$

The null distribution comes from `omnibus_permutations` (1000) circular shifts of the time ordered label sequence by a seeded offset, which keep the label dependence structure and break its alignment with the features. (The first draft of this document said within session shuffles. That was corrected before any real run, because a within session shuffle preserves between session associations such as volatility regime and class mix, so it is not a valid null for regime features.) $p = \frac{1 + \#\{S^{*} \ge S\}}{1 + B}$, tested at $\alpha_{\mathrm{omni}} = 0.01$. Permutation importance explains a positive result. Multiclass TreeSHAP is deferred (see `DECISIONS.md`, section 7).

## 9. Stop rules and checkpoints

The loop stops when the budget is spent, or after two consecutive nulls following the omnibus. Then the negative result is reported as the finding. The agent runs cycles on its own and stops to ask the owner only at: a PROMOTE (before confirmation), any change to this protocol, any holdout use, and budget exhaustion.

## 10. Ledger

`artifacts/research/ledger.jsonl`, append only, one JSON object per cycle: cycle number, card id, card SHA256, git commit, view (discovery or confirm), timeframes, trials added, alpha spent, $\Delta$, $p$, verdict, MDE, UTC time. The pipeline never rewrites or deletes it.
