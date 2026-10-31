# Baselines and controls

Owner crate: `vwap_baselines`. Config: `config/baselines.toml`. Symbols: `GLOSSARY.md`.

Purpose: before any model is built, check whether outcomes at the anchored VWAP differ from outcomes at levels that have no special meaning. If they do not, there is nothing for a model to find at the VWAP and the project stops here. This is the Phase 3 gate.

## 1. Controls run through the identical pipeline

Controls implement `LevelSource`. They pass through the same detector (same $k$, $N_{\mathrm{ex}}$, side rule, re arm rule, gates), the same labeler (same $m_1, m_2, T, \sigma_t$) and later the same features. Only the level changes.

**Control 1, random horizontal levels.** For each session, $K_{\text{ctrl}}$ levels (config, default 20) are drawn at the anchor from a seeded generator:

$$ \ell_q = C_{t_0} + z_q\,\sigma_{t_0}, \qquad z_q \sim \mathrm{Uniform}[-Z, Z] $$

where $C_{t_0}$ and $\sigma_{t_0}$ are the close and ATR of the last bar completed at the anchor, and $Z$ is a config constant (default 3). The level is a constant for the session. Each level is processed as a separate event stream (the re arm state machine is per level). Everything used to draw a level is known at the anchor, so the control is causal. $Z$ is fixed in config and not tuned to match the VWAP.

**Control 2a, time shifted anchor VWAP.** An anchored VWAP started at a non standard clock time, config `shifted_anchors` (default 11:00 and 13:00 New York for the Globex anchor kind). The VWAP is undefined before its own anchor and resets daily. It tests whether anything is special about the session anchor itself.

**Control 2b, previous session VWAP curve.** The VWAP curve of the previous session read at the same New York clock time:

$$ \ell^{\text{prev}}_t = \mathrm{VWAP}^{(d-1)}_{\text{slot}(t)} $$

It has the same smooth, volume weighted structure as the real VWAP but is not today's, and it uses only earlier information.

**Self reference check.** Control 2b is also a fixed level for the bar, so comparing it with the real VWAP tests the effect of VWAP including bar $t$'s own volume (see `EVENTS.md`, section 4). The sensitivity run with `vwap_reference = "prior_bar_close"` is reported next to it.

## 2. Matching the event mix

Controls would otherwise differ from VWAP events simply in when events occur. Both sets are post stratified on strata $g$ defined by time of day bucket (config, default 30 minute buckets) and volatility tercile (of $\mathrm{rvpct}$ from `FEATURES.md`, tercile edges taken from the development data). The adjusted control class frequency is

$$ \hat\pi^{\text{ctrl,adj}}_c = \sum_g \frac{n^{\mathrm{VWAP}}_g}{n^{\mathrm{VWAP}}}\;\hat\pi^{\text{ctrl}}_{c \mid g} $$

with the unadjusted version reported alongside. Strata with fewer than `min_stratum_events` (default 30) events in either set are merged with the neighbouring bucket.

## 3. Reported statistics

For each of VWAP and every control, for each timeframe, on the primary label cell (and descriptively on the full grid):

* Class counts $n_c$ and frequencies $\hat\pi_c = \frac{n_c}{n}$ for $c \in \{\text{REJ}, \text{BRK}, \text{CHOP}\}$.
* Simultaneous 95 percent confidence intervals for the three frequencies, from a **session clustered bootstrap**: resample whole sessions with replacement (2000 draws, seeded), recompute frequencies, take the Bonferroni adjusted percentiles. A Wilson interval on the raw counts is shown as a reference only, because events in one session are dependent and the Wilson interval is too narrow.
* Mean and median time to barrier, mean MAE.
* Event counts per year, to show the sample is not driven by a few years.
* Effective sample size $n_{\text{eff}} = \frac{n}{1 + (\bar m - 1)\hat\rho}$ with $\bar m$ the mean events per session and $\hat\rho$ the intra session correlation of class indicators.

## 4. Test

Statistic: total variation distance between VWAP and a control,

$$ \mathrm{TV} = \frac{1}{2}\sum_{c}\big|\hat\pi^{\mathrm{VWAP}}_c - \hat\pi^{\text{ctrl,adj}}_c\big| $$

Null distribution: a **session level permutation test**. Sessions are the exchangeable units. In each permutation, whole sessions' event sets are randomly reassigned between the VWAP group and the control group within strata, and $\mathrm{TV}$ is recomputed (10000 permutations, seeded). The two sided $p$ value is the share of permutations with $\mathrm{TV}$ at least as large as observed, with the usual plus one correction $p = \frac{1 + \#\{\ge\}}{1 + B}$.

Multiple testing: Holm correction across all (control, timeframe, label cell) combinations in the family that is declared in config.

## 5. Gate (pre registered)

Continue to Phase 4 only if, on the pre registered primary label cell:

1. for **every** control family (1, 2a, 2b), at least one timeframe has Holm adjusted $p < 0.05$, and
2. in that timeframe the absolute difference in rejection frequency exceeds $\delta_{\min}$ (config, default 2 percentage points), and
3. the sign of the rejection difference agrees across the two timeframes.

If the gate fails, the pipeline stops. Write `reports/gate_baselines.md` stating the numbers, the intervals, and the conclusion "no detectable difference between anchored VWAP outcomes and controls". Do not search other cells or parameters to rescue the result. Exploring other cells after a failure is allowed only as a clearly labeled new study with its trial count added.

Note recorded for transparency: the gate tests unconditional class frequencies. A failure does not prove that features cannot predict outcomes conditionally. The rule is kept anyway, as specified by the owner, because a feature conditioned edge on a level whose outcomes look like noise is more likely an artifact.

## 6. Tests

* Controls are causal: replacing all bars after the anchor leaves the drawn level unchanged (random control) and replacing bars after $t$ leaves control level values at $t$ unchanged (time shifted controls).
* Property: with a control level identical to the real VWAP, the baseline pipeline returns exactly the VWAP events (pipeline equivalence).
* Property: the permutation test gives uniformly distributed $p$ values on data where both groups are drawn from the same distribution (checked with a Kolmogorov Smirnov bound over many seeded runs).
* Unit: Bonferroni and Holm on small worked examples; clustered bootstrap coverage on synthetic data with known cluster structure.
