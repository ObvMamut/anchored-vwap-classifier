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

**As implemented** (this replaces the permutation test first specified, see `DECISIONS.md`, section 8). The control events and the VWAP events of one session travel together, so the design is paired by session. Inference uses a **paired session bootstrap**: whole sessions are resampled with replacement (config `bootstrap_draws`, default 2000, seeded), and in each draw the adjusted class frequencies of both groups are recomputed. Let $\hat\pi^{\mathrm{V}}_c$ and $\hat\pi^{\mathrm{C}}_c$ be the VWAP and stratum adjusted control frequencies and $\Delta_c = \hat\pi^{\mathrm{V}}_c - \hat\pi^{\mathrm{C}}_c$.

* Intervals: simultaneous 95 percent percentile intervals for the three classes (Bonferroni, so $\alpha/3$ per class) for each group frequency and for each $\Delta_c$.
* Test: a Wald statistic on the class frequency difference vector. With $\hat\Sigma$ the bootstrap covariance of $(\Delta_{\mathrm{REJ}}, \Delta_{\mathrm{BRK}})$,

$$ W = \boldsymbol\Delta^{\top}\hat\Sigma^{-1}\boldsymbol\Delta, \qquad p = e^{-W/2} $$

  where the closed form of the chi square tail with two degrees of freedom is used. If $\hat\Sigma$ is singular (a class that never occurs) the test falls back to one degree of freedom on the remaining component, $W = \Delta^2/\hat\sigma^2$ and $p = \mathrm{erfc}\big(\sqrt{W/2}\big)$.
* The total variation distance $\mathrm{TV} = \frac12\sum_c|\Delta_c|$ is reported as a descriptive effect size only.
* Why not the permutation test: pooling $K_{\text{ctrl}}$ random levels per session makes the control group about $K_{\text{ctrl}}$ times the size of the VWAP group, so the two groups are not exchangeable and a label swap would not reproduce the null distribution.
* Multiple testing: Holm correction across all (control family, timeframe) tests, six in total.

Stratification as implemented: 24 clock hour buckets of the touch bar close in New York time, crossed with terciles of $\sigma_t / C_t$ (the ATR relative to price, known at the touch) with tercile edges taken from the development VWAP events. This stands in for the realised volatility percentile, which belongs to Phase 4. A stratum is used only if both groups have at least `min_stratum_events` events in it, and the coverage of the VWAP events is reported.

Other implemented details: the random control level is $\ell = O_{\text{first bar}} + z\,\mathrm{ATR}_{\text{previous bar}}$ with $z$ drawn from $\mathrm{Uniform}[-Z, Z]$, a level count $K_{\text{ctrl}} = 10$ per session, and a draw that depends only on the seed, the level index and the session key. The shifted anchors are 11:00 and 13:00 New York time. All statistics use development data only: events whose label ends at or after the holdout start are removed.

## 5. Gate 3, two tiers

The first version of this gate (a single pass or fail on the marginal class mix) failed on development data, see `DECISIONS.md`. Its weakness: with symmetric barriers every level gives about 50 percent rejection, so it cannot see a conditional edge, and "not significantly different" is not evidence of equivalence. The gate is now two tiers.

**3a, marginal equivalence (reported, never decisive).** For each control family and timeframe, with $\Delta = \Delta_{\mathrm{REJ}}$ and margin $\delta$ = `delta_min` (2 percentage points):

* **EQUIVALENT** if the 90 percent percentile interval of $\Delta$ lies inside $(-\delta, +\delta)$ (two one sided tests at 5 percent).
* **DIFFERENT** if the Holm adjusted $p < 0.05$ and $|\Delta| > \delta$.
* **INCONCLUSIVE** otherwise, which usually means too little overlap (low stratum coverage) or too wide an interval.

**3b, conditional screen (decisive).** Whether the project continues is decided by the research loop of `PROTOCOL.md`: the omnibus screen and the targeted hypothesis cards, on the discovery segment only. Gate 3b passes if at least one card is PROMOTED. If the budget is spent without one, the project stops and the negative result is reported.

Remark kept from the first version: tier 3a tests unconditional frequencies, so it never ruled out a feature conditioned effect.

## 6. Tests

* Controls are causal: replacing all bars after the anchor leaves the drawn level unchanged (random control) and replacing bars after $t$ leaves control level values at $t$ unchanged (time shifted controls).
* Property: with a control level identical to the real VWAP, the baseline pipeline returns exactly the VWAP events (pipeline equivalence).
* The Wald test on identical groups rarely rejects (at most 8 false rejections in 40 seeded trials at the 5 percent level in the unit test) and detects a planted rejection frequency difference.
* Unit: Holm on a small worked example; the stratum reweighting on a hand built two stratum case; the closed form of the Wald p value; `erfc` against known values.
