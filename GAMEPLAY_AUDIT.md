# Roady Car — Gameplay Audit

**Scope:** Read-only synthesis of four parallel audits (numerical model, game-theory/pacing, UX/accessibility, leaderboard/security) against `src/`, `tools/`, `index.html`, `Cargo.toml`, `LEADERBOARD_ARCHITECTURE.md`, and unit tests. No game source was edited. Date: 2026-07-12.

**Method & evidence hierarchy:** Every claim is sourced to `file::function` or `file::CONSTANT` and marked **PROVEN** (derivable from code + tests) or **HYPOTHESIS** (plausible from code but needs telemetry/playtest). Where reports conflict, source/test evidence is preferred over docs and over inference. No telemetry was invented; gaps are stated explicitly.

**Headline conflict resolved:** The README and `LEADERBOARD_ARCHITECTURE.md` §8 state the round is "capped at 90 seconds." The code has **two distinct caps**: coins cap `TimeLeft` at **90 s** (`world.rs::MAX_ROUND_TIME`), but Time power-ups cap at **99 s** (`pickups.rs::TIME_CAP`). The true maximum round length is **99 s**. Any leaderboard cap derived from a 90 s assumption is miscalibrated (see §7, §8).

## Post-audit implementation status — 2026-07-13

The findings below intentionally preserve the audited source snapshot. The following high-confidence recommendations have since been implemented and validated:

- Browser/OS `prefers-reduced-motion` is inherited when no persisted choice exists; an explicit saved preference takes precedence.
- Reduced Motion now suppresses smoke and hit particles, pickup bob/spin/flash, creature bob/waddle, water ripples, speed zoom, camera trauma, and stale damage flashes while preserving gameplay information.
- Timer and combo contrast panels were added; objective, combo, event, minimap, and level-label bands were separated.
- The event banner now includes a non-color signature, explicit seconds remaining, and segmented duration progress.
- The Menu now includes a five-condition medal gallery, and maximal Game Over content has a compact mobile layout.
- The touch HUD now uses a coordinated compact cockpit, health, power-up, event, minimap, and driving-control layout at the 844×390 target viewport. A later mobile wave replaced the legacy STEER/BRAKE/GO split with a first-touch-owned `DRIVE` drag pad and one `BRAKE / REVERSE` action that brakes through a stop before reversing; the mobile Menu and terminal leaderboard/modal layouts are also collision-free.
- Static-obstacle damage now requires impact strictly above 5 u/s, uses a 0.5 s cooldown, and deterministically applies only the strongest qualifying contact per frame. Sub-threshold closing contacts still stop inward motion.
- Deterministic Field and Orchard tile variants and conservative rotation-independent farm-prop colliders were added without changing road socket compatibility.

Current validation after the mobile-control follow-up: **290 Rust tests**, zero native/WASM warnings, strict desktop/touch/Settings browser scenarios passed, and release WASM **22.859 MiB** (25 MiB limit). Telemetry-dependent balance proposals remain intentionally unimplemented.

---

## Executive Summary

Roady Car is a deterministic, single-life arcade driving score-attack with a fixed 60 s round (extendable to 99 s), five rotating road conditions, two fixed-time mid-run events per round, a 2.5 s combo window capped at 5×, and per-condition medal progression (15 total). A cloud leaderboard architecture is documented and its implementation is now in progress; the committed gameplay baseline audited here did not yet expose it in the UI. The core loop is well-engineered and tightly bounded:

- **Pacing is proven sound.** A frame-rate-independent difficulty ramp (level 0→6 over ~60 s), traffic always slower than the player (11.5 vs 12.0 u/s), and a combo window that forces aggression produce a classic rising-tension arcade curve.
- **Risk/reward is intentional and bounded.** Damage scales with own speed (self-balancing); GlassCannon doubles both damage and combo bonus; the combo multiplier never scales the base +1, capping per-hit at 17 (chicken) in the most stacked *reachable* case.
- **The dominant-strategy surface is narrow but real.** Coins are weakly dominant at low combo (score + time extension); chickens dominate at 3×+ combo; coins also refresh combo, making farming and chaining synergistic rather than competing. The 90 s coin cap is the correct anti-degeneracy guard; the open question is whether the *path to the cap* is itself a degenerate opener — answerable only with telemetry.
- **Accessibility has one large gap and several medium ones.** The largest: no `prefers-reduced-motion` auto-detection, and several presentation effects (particles, water shader, orb bob, pickup flash) are not gated by the Reduced Motion setting that *does* gate camera shake/timer pulse/combo punch. Timer and combo HUD elements lack background panels → likely WCAG contrast failures.
- **Leaderboard anti-cheat is honestly threat-modeled but cap-calibration-dependent.** The server enforces invariant, range, cap, session, and HMAC checks but cannot verify per-hit rate, combo legitimacy, time–score consistency, objective completion, or condition/event co-occurrence. The 90 s vs 99 s cap error must be fixed before caps ship. Generous caps (needed to avoid rejecting legitimate exceptional play) create a wide fabrication window; the flag-near-cap mechanism has no specified threshold or review SLA.
- **Replayability/meta-progression is thin.** 15 medals is a finite ceiling; no per-condition medal gallery, no "retry this condition" path (every restart advances the cycle), no stats/streaks/daily challenges. For completionists, the inability to retry a missed medal condition is the central motivation drain.

**Score plausibility:** Medal Gold thresholds are 45–100. Without play telemetry, they cannot yet be labeled by player-skill percentile. Source-derived scenario estimates place exceptional rounds in the hundreds, but no strict theoretical score ceiling is currently derivable because spawn/recycle throughput and several self-reported submission fields require either simulation or observed-rate bounds. The large gap between medal targets and any generous anti-cheat cap is the fundamental calibration tension.

---

## Canonical Source-Cited Numerical Table

All values verified against source; citations are `file::CONSTANT` or `file::function`.

### Timer & time extension

| Parameter | Value | Citation |
|---|---|---|
| Round start time | 60.0 s | `game/resources.rs::TimeLeft::default` |
| Countdown duration | 3.0 s (input frozen, timer paused) | `countdown.rs::start_countdown` (`t = 3.0`) |
| Coin time bonus | +1.5 s per coin | `world.rs::COIN_TIME_BONUS` |
| **Coin time cap** | **90.0 s** (`MAX_ROUND_TIME`) | `world.rs::coin_time_after_collect` |
| Time power-up bonus | +5.0 s | `pickups.rs::TIME_BONUS` |
| **Time power-up cap** | **99.0 s** (`TIME_CAP`) | `pickups.rs::collect_pickup` |
| **Effective max round time** | **99 s** (coins→90, then Time PU→99) | `world.rs` + `pickups.rs` |
| Timer tick | `t.0 -= delta_secs()`; ≤0 → `GameOver(TimeUp)` | `game/mod.rs::tick_timeleft` |

### Scoring

| Component | Value | Citation |
|---|---|---|
| Terminal total | `Score.chickens + Score.coins` (u32, saturating) | `game/resources.rs::Score`; confirmed `ui.rs::spawn_gameover`, `persist.rs::persist_best_on_round_end` |
| Chicken base/hit | +1 to `chickens` | `chickens.rs::hit_chickens` (`CHICKEN_BASE_SCORE = 1`) |
| ChickenFrenzy direct bonus | +1/hit | `modifiers.rs::chicken_score_bonus` |
| ChickenBurst event direct bonus | +1/hit | `run_events.rs::chicken_score_bonus` |
| Coin base | +1 to `coins` | `world.rs::collect_coins` |
| MegaCoin | +5 to `coins` (one `CoinCollected` msg) | `pickups.rs::collect_pickup` (`MEGA_COIN_AMOUNT`) |
| Objective bonus | +10 to `chickens` (once) | `objectives.rs::award_objective_bonus` (`OBJECTIVE_BONUS`) |
| Critter penalty | −2 `chickens` + −25 health (0.4 s cooldown) | `critters.rs` (`HIT_SCORE_PENALTY`, `HIT_HEALTH_PENALTY`, `HIT_PENALTY_COOLDOWN`) |

### Combo

| Parameter | Value | Citation |
|---|---|---|
| Combo window | 2.5 s (refresh per hit; expiry → 1×) | `combos.rs::COMBO_WINDOW` |
| Tiers | 0–4→1×, 5–9→2×, 10–14→3×, 15–19→4×, 20+→5× (cap) | `combos.rs::multiplier_from_count` |
| Bonus formula | `(mult − 1) × mod_combo × event_combo` (base +1 unscaled) | `combos.rs::combo_bonus_for_hit` |
| GlassCannon combo mult | ×2 | `modifiers.rs::combo_bonus_multiplier` |
| ComboFrenzy event combo mult | ×2 | `run_events.rs::combo_bonus_multiplier` |
| Combo refresh sources | `ChickenHit`, `CoinCollected` (NOT `CritterHit`) | `combos.rs::register_hit` |

### Per-hit totals at 5× combo (reachable combos)

| Condition | Event | Chicken/hit | Coin/hit |
|---|---|---|---|
| Standard | none | 1 + 4 = **5** | 1 + 4 = **5** |
| ChickenFrenzy | none | 2 + 4 = **6** | 1 + 4 = **5** |
| GlassCannon | none | 1 + 8 = **9** | 1 + 8 = **9** |
| RushHour/Stampede | ComboFrenzy | 1 + 8 = **9** | 1 + 8 = **9** |
| ChickenFrenzy + ChickenBurst | — | 3 + 4 = **7** | **unreachable** in normal play¹ |
| GlassCannon + ComboFrenzy | — | 1 + 16 = **17** | **unreachable** in normal play¹ |

¹ Both same-flavor combinations are excluded by deterministic event plans (`run_events.rs::EventPlan::for_kind`, tested `plans_exclude_same_flavor_and_reach_every_event`). ChickenFrenzy receives CritterBurst/TrafficSurge; GlassCannon receives CritterBurst/TrafficSurge. The realistic maximum combo-scaled hit is **9**. The unreachable rows are retained only as anti-cheat cross-field examples.

### Health & damage

| Parameter | Value | Citation |
|---|---|---|
| Max health | 100.0 | `health.rs::HEALTH_MAX` |
| Obstacle damage | `impact_speed × DAMAGE_K × damage_mult` | `health.rs::obstacle_damage` |
| `DAMAGE_K` | 4.0 | `health.rs` |
| Min impact for damage | 3.0 u/s | `car.rs::MIN_IMPACT_SPEED` |
| GlassCannon damage mult | ×2.0 | `modifiers.rs::damage_multiplier` |
| Health power-up | +35.0 (cap 100) | `pickups.rs::HEALTH_RESTORE` |
| Critter damage | 25 × damage_mult per 0.4 s | `critters.rs` |
| Wreck threshold | health ≤ 0 → `GameOverReason::Wrecked` | `health.rs::apply_damage`, `critters.rs::hit_critters` |

**Damage examples:** Standard 12 u/s hit = 48 dmg → ~2 hits wreck. GlassCannon 12 u/s = 96 → ~1 hit wreck. GlassCannon critter = 50/hit → 2 critter hits wreck.

### Car physics

| Parameter | Value | Citation |
|---|---|---|
| Max speed | 12.0 u/s | `game/resources.rs::GameConfig` |
| Turn rate | 2.5 rad/s (scaled by speed/max_speed — no turn at standstill) | `car.rs::move_car` |
| Accel rate | 3.0 (`ACCEL_RESPONSE_RATE`) | `car.rs::next_speed` |
| Coast rate | 2.0 (`COAST_RESPONSE_RATE`) | `car.rs::next_speed` |
| Brake rate | 4.0 (`BRAKE_RESPONSE_RATE`); brake dominates throttle | `car.rs::next_speed` |
| Brake time 12→0 | ~1.5–2.0 s | test `braking_is_progressive_but_stops_in_a_reasonable_time` |
| Car solid footprint | 1.12 x 2.00 u, oriented with heading | `car.rs::car_footprint_half_extents` |
| SpeedBoost | 4 s; +20 u/s² accel; cap 19.2 u/s (1.6× max); non-stackable (refreshes) | `pickups.rs` |

### Populations & spawns

| System | Value | Citation |
|---|---|---|
| Chickens base / Frenzy / Burst | 14 / 35 / +14 temporary | `chickens.rs` (`CHICKEN_COUNT`, `effective_chicken_target`, `CHICKEN_BURST_SPAWN_LIMIT`) |
| Chicken speed | 2.4 u/s (`max × 0.2`) | `chickens.rs::CHICKEN_SPEED_RATIO` |
| Chicken hit radius | 1.0 u | `chickens.rs::HIT_RADIUS` |
| Chicken respawn ahead | 34–56 u; lateral ±22 u | `chickens.rs` (`RESPAWN_AHEAD_MIN=34`, `RESPAWN_AHEAD_RANGE=22`, `LATERAL_SPREAD`) |
| Chicken keep radius | 65.0 u; behind threshold 15.0 u | `chickens.rs` |
| Cross-road probability | 0.65 | `chickens.rs::CROSS_ROAD_PROBABILITY` |
| Critters base / Stampede / Burst | 5 / 10 / +5 | `critters.rs` (`CRITTER_COUNT`) |
| Critter hit radius | 1.2 u | `critters.rs::HIT_RADIUS` |
| Critter speeds | Pedestrian 1.8, Cow 0.96, Moose 1.44 u/s | `critters.rs::critter_speed` |
| Traffic hard cap | 8 (`MAX_TRAFFIC`) | `difficulty.rs` |
| Traffic baseline | `1 + level/2` capped 8 | `difficulty.rs::target_traffic_count` |
| Traffic level | `(elapsed/10) as u32` capped 6 (`MAX_LEVEL`) | `difficulty.rs` |
| Traffic base speed | 5.0 + level × 0.7; jitter ×0.85–1.15; cap 11.5 | `difficulty.rs` |
| Traffic keep radius | 90.0 u | `difficulty.rs::TRAFFIC_KEEP_RADIUS` |
| Coins per block | 4 if any road edge, else 0 | `world.rs::collect_coins` |
| Coin grid | 5×5 blocks, 40 u each (25 blocks) | `game/resources.rs::GridConfig` |
| Coin pickup radius | 1.2 u | `world.rs` |
| Power-up spawn interval | 8–12 s; max 4 active | `pickups.rs` (`SPAWN_INTERVAL_MIN/MAX`, `MAX_ACTIVE_PICKUPS`) |
| Power-up kind weights | SpeedBoost 30%, Time 25%, Health 15%, MegaCoin 15%, CoinMagnet 15% | `pickups.rs::POWER_KIND_WEIGHTS` |
| CoinMagnet | 4 s; 10 u radius; strength 3.0; max 24 coins/frame | `pickups.rs` |

### Events (mid-run)

| Parameter | Value | Citation |
|---|---|---|
| First event window | elapsed ∈ [15.0, 23.0) | `run_events.rs::FIRST_EVENT_START/END` |
| Second event window | elapsed ∈ [40.0, 48.0) | `run_events.rs::SECOND_EVENT_START/END` |
| Elapsed clock | ticks only while `InputFrozen` false | `difficulty.rs::tick_difficulty` |
| Event plans | per-modifier, exclude same-flavor | `run_events.rs::EventPlan::for_kind` |

| Event | Traffic | Chicken bonus | Combo bonus | Extra spawns |
|---|---|---|---|---|
| TrafficSurge | ×2 count, ×1.25 speed | +0 | ×1 | 0 |
| ChickenBurst | ×1 | +1 | ×1 | +14 chickens |
| ComboFrenzy | ×1 | +0 | ×2 | 0 |
| CritterBurst | ×1 | +0 | ×1 | +5 critters |

| Modifier | Event 1 (15–23 s) | Event 2 (40–48 s) |
|---|---|---|
| Standard | TrafficSurge | CritterBurst |
| RushHour | ChickenBurst | ComboFrenzy |
| ChickenFrenzy | CritterBurst | TrafficSurge |
| Stampede | ComboFrenzy | ChickenBurst |
| GlassCannon | CritterBurst | TrafficSurge |

### Medals (`persist.rs::medal_for`)

| Condition | Bronze | Silver | Gold |
|---|---|---|---|
| Standard | 20 | 40 | 70 |
| RushHour | 15 | 30 | 55 |
| ChickenFrenzy | 35 | 65 | 100 |
| Stampede | 15 | 25 | 45 |
| GlassCannon | 25 | 50 | 80 |

- 15 total medals; menu shows aggregate "MEDALS: N / 15" (`ui.rs::spawn_menu`).
- Medals computed from **terminal** condition bests only (`persist.rs::with_terminal_total`; test `peak_during_play_is_irrelevant`).
- Persisted `v1:global:s0:s1:s2:s3:s4` (`persist.rs::encode_bests`); versioned schema with legacy migration + corruption-defaulting.

---

## Annotated Loop/Pacing Map

The round is a single-life, fixed-clock arcade loop with deterministic structure. Elapsed time is the active-play clock (frozen during the 3 s countdown).

```
t=0.0   Countdown (3 s, InputFrozen, timer paused)
        ↓ shows condition + current medal best
t=3.0   GO — timer starts at 60.0 s; Difficulty.elapsed begins ticking
        ↓ Level 0: 1 traffic car @ ~5 u/s; 14 chickens; 5 critters
t=3–15  Opening: low traffic, player builds combo, collects coins on road edges
        ↓ Coins: +1 score + 1.5 s each (cap 90 s) — time-farming window
t=15.0  EVENT 1 begins (8 s window, [15, 23))
        ↓ Kind is deterministic per modifier (see table above)
        ↓ Banner shows display_name() + color(); generic click.wav sting only
t=23.0  Event 1 ends
        ↓ Level ramps: level = floor(elapsed/10), capped 6
        ↓ Traffic: +1 car per 2 levels; speed +0.7/level; cap 11.5 u/s
t=40.0  EVENT 2 begins (8 s window, [40, 48))
        ↓ Best stacking windows: RushHour/Stampede get ComboFrenzy here (×2 combo)
t=48.0  Event 2 ends
        ↓ Level 4–6: 3–4 traffic cars @ ~8.5–9.2 u/s (Standard); "frenetic finish"
t=60.0  Base timer expires → GameOver(TimeUp) UNLESS extended
        ↺ Coins can extend to 90 s; Time power-ups to 99 s
        ↓ At 90 s, coins stop adding time; Time PU still adds to 99
t=99.0  Absolute ceiling — no further extension possible
        ↓ GameOver(TimeUp) or GameOver(Wrecked) if health ≤ 0

COMBO LAYER (overlaid throughout):
  hit every ≤2.5 s → maintain multiplier
  5 hits→2×, 10→3×, 15→4×, 20→5× (cap)
  coins AND chickens refresh; critters do NOT break combo directly
  bonus = (mult−1) × mod × event; base +1 never scaled
  urgency pulse at timer < 35% (URGENCY_THRESHOLD)
```

**Pacing verdict (PROVEN):** Rising tension via linear population/speed ramp; traffic always slower than player preserves agency; combo window forces aggression; timer urgency pulse provides tension feedback. **HYPOTHESIS:** whether late-round L5–6 creates a satisfying frenetic finish or a frustrating spike where score farming becomes impossible — needs per-10 s scoring telemetry.

---

## Expected-Value & Dominant-Strategy Analysis

### Per-second economics (PROVEN from constants; rates are HYPOTHESIS)

| Action | Score yield | Time yield | Notes |
|---|---|---|---|
| 1 coin | +1 + combo bonus | +1.5 s (cap 90) | Also refreshes combo |
| 1 chicken | +1 + bonuses + combo bonus | 0 | Also refreshes combo |
| 1 MegaCoin | +5 + combo bonus | 0 | Single `CoinCollected` msg |
| 1 critter hit | −2 chicken + −25 HP | 0 | Does NOT break combo directly |
| 1 objective | +10 (once) | 0 | ~14% of a Standard Gold run |

**At 1× combo:** coin ≈ chicken in score, but coin *also* extends time → **coins are weakly dominant per-second at low combo.**

**At 3×+ combo:** chicken bonus = (mult−1) = +2/hit vs coin's flat +1 bonus → **ch chickens pull ahead in score, but coins still extend time and refresh combo.**

### Strategy candidates and verdicts

| Strategy | Mechanism | Verdict |
|---|---|---|
| **Time Farmer** | Coin-for-time extension to 90 s cap | **PROVEN viable** as an opener; net +1.2 s/block early (3 coins/block @ 12 u/s = +4.5 s while spending 3.3 s). Cap is correct anti-degeneracy guard. **HYPOTHESIS:** if farming-to-cap-then-chaining is the dominant opening, first 30 s of every round look identical (degenerate opener). |
| **Chicken Aggression** | Hunt chickens through traffic at 12 u/s | **PROVEN** primary score engine; combo forces aggression. Well-risked (chickens live on roads with traffic + critters). **HYPOTHESIS:** may dominate so strongly that coins/critters become irrelevant mid-late — needs score-source mix telemetry. |
| **Combo Maintainer** | Hit every ≤2.5 s to hold 5× | **PROVEN** skill ceiling (20 consecutive hits ≈ 50 s of flawless chaining). Coins refresh combo = safety valve making Farmer + Maintainer synergistic. **HYPOTHESIS:** 2.5 s window may be too forgiving or too punishing — needs combo-length distribution telemetry. |
| **Objective Optimizer** | Target the +10 objective | **PROVEN non-dominant** by magnitude (10 vs ~70 Gold). Objectives steer, not dictate. Modifier-adjusted targets prevent trivialization. |
| **Event Waiter** | Play passively until 15 s, exploit 8 s boost | **PROVEN net-negative in theory**: waiting wastes 15 s (25% of round) for an 8 s boost (13%). Only viable if the boost multiplies existing scoring rather than replacing zero scoring. **HYPOTHESIS:** may still be rational for risk-averse players if early traffic is punishing — needs playtest. |
| **GlassCannon risk/reward** | 2× damage for 2× combo bonus | **PROVEN genuine trade** (not a trap): +8/hit vs +4/hit Standard at 5×. But medal Gold = 80 vs Standard 70 (+14% reward for +100% damage). **HYPOTHESIS:** likely **under-rewarded** — mortality may wipe the upside. Needs median-score comparison telemetry. |

### Dominant-strategy synthesis

**No pure dominant strategy is PROVEN.** The design is healthy: coins and chickens are synergistic (both refresh combo), the time cap prevents infinite farming, same-flavor event exclusion prevents double-stacking, and objectives are non-dominant by magnitude. The most likely *emergent* dominant strategy is a **"farm-to-cap then chain chickens at 5×"** opener, which would make early rounds repetitive — but this is HYPOTHESIS pending coin-collection-rate-over-time telemetry (a bimodal "farm then fight" distribution would confirm).

---

## Findings

### Risk/Reward

**PROVEN:**
- Asymmetric reward/penalty is well-bounded. Reward scales with combo; penalty (damage) scales with own speed → self-balancing ("the faster you go to chase chickens, the harder you crash").
- GlassCannon is a genuine trade: 2× damage AND 2× combo bonus; critter damage also scales (50/hit → 2 hits wreck).
- Combo bonus never multiplies base +1 → prevents pure exponential; per-hit ceiling bounded by u32 saturation (tested `glass_cannon_and_combo_frenzy_compose_and_saturate`).
- SpeedBoost is non-stackable (refreshes timer, cap 1.6× max) → no degenerate speed stacking.
- Critter penalty cooldown (0.4 s) prevents clustered critters from one-shotting the car (tested `cooldown_preserves_health_and_blocks_scaled_damage`).

**HYPOTHESIS:**
- The −2 chicken-score critter penalty may be under-tuned as a deterrent (25 HP is the real deterrent; −2 ≈ 1 chicken). If players routinely eat critters to maintain combo pacing, the score penalty is cosmetic.
- GlassCannon is likely under-rewarded: +14% Gold threshold for +100% damage risk. Needs median-score-by-condition telemetry.

### Health

**PROVEN:**
- Damage = `impact_speed × 4.0 × damage_mult`; traffic uses relative velocity (head-on sums, same-direction differences); static obstacles use absolute player speed. Tested.
- Full-speed Standard hit (48 dmg) → ~2 hits wreck; GlassCannon full-speed (96) → ~1 hit wreck.
- Health power-up +35 (cap 100); weight 15% in spawn table.
- `GameOverReason::Wrecked` at health ≤ 0; `GameOverReason::TimeUp` at timer ≤ 0.

**HYPOTHESIS:**
- Health power-up at 15% spawn weight may be too rare to recover from a bad mid-round hit, making early mistakes unrecoverable. Needs health-at-game-over distribution telemetry.

### Five Conditions/Events

**PROVEN:**
- Deterministic 5-round modifier cycle (`MODIFIER_CYCLE`): Standard → RushHour → ChickenFrenzy → Stampede → GlassCannon. First round always Standard.
- Event plans exclude same-flavor events (tested `plans_exclude_same_flavor_and_reach_every_event`) → no degenerate double-stacks (ChickenFrenzy never gets ChickenBurst; GlassCannon never gets ComboFrenzy).
- Modifier-adjusted objective targets prevent trivialization (tested `modifier_targets_are_adjusted_only_for_matching_flavors`).
- RushHour + TrafficSurge composition respects hard caps (tested `rush_hour_plus_traffic_surge_respects_hard_caps`); traffic always ≤ 11.5 u/s.

**PROVEN gaps:**
- **Zero player agency over condition selection.** A player who struggles with one condition must play it every 5 rounds with no skip/re-roll/retry. Determinism is fair but motivation-risky for completionists.
- **Event kind is conveyed only visually** (banner `display_name()` + `color()`) with a single generic `click.wav` sting — no per-event audio differentiation. Low-vision/glance-away/screen-reader users get no event-type information.
- **Event banner shows no remaining-duration indicator** — players don't know the event lasts 8 s, reducing strategic play ("push hard during ComboFrenzy").

**HYPOTHESIS:**
- Fixed event timing every round reduces surprise/replayability variety for repeat players ("surge at 15 s" is knowable).

### Medals/Progression

**PROVEN:**
- Per-condition Bronze/Silver/Gold with distinct thresholds; versioned `v1:` storage with legacy migration, corruption-defaulting, atomic native writes, idempotent retry.
- Game-over uses `ConditionBestsAtRoundStart` snapshot → ordering-safe (`terminal_condition_result` independent of persistence order).
- Medals computed from terminal bests only (in-progress peaks never record; tested `peak_during_play_is_irrelevant`).

**PROVEN gaps:**
- **No per-condition medal gallery.** Menu shows only aggregate "MEDALS: N / 15." A condition's current medal is visible only at countdown and game-over. You cannot view "I still need Gold on Stampede" anywhere → directly limits the medal-chasing loop the system was built to support.
- **No in-run "progress to next medal" indicator.** Score and best are shown, but nothing says "12 points to Silver."
- **15 medals is a finite, completable ceiling.** Once all Gold, only raw best-score chasing remains. No unlockable cars/tracks, no stats page, no streak/daily/weekly challenges.

**HYPOTHESIS:**
- A "NEW BEST" / "MEDAL UPGRADE" callout at game-over is motivating but text-only (no celebration animation in default motion).

### Touch/Accessibility

**PROVEN:**
- Touch zones: STEER (x<0.45), BRAKE (0.55–0.75), GO (≥0.75), all y≥0.55; PAUSE top-center (y<0.14, x 0.44–0.56). Multi-touch merges with brake-priority. Thorough state-transition and boundary tests.
- Steering is speed-gated (`heading += steer × turn_rate × dt × (speed/max_speed)`) → no turn at standstill. Realistic but potentially confusing for new touch players.
- Reduced Motion IS consumed by: camera shake, timer alpha pulse, combo punch/reveal-fade/urgency-pulse, countdown punch, damage-flash vignette, HTML loading spinner. All tested.

**PROVEN gaps (touch):**
- **No touch equivalent for reverse.** `S`/`↓` is keyboard-only. Touch players cannot back up (matters when wedged against a building).
- **Mute IS reachable on touch** via Settings overlay — README's "no on-screen touch equivalent" claim is stale. But it's not a dedicated one-tap mute.
- **Settings overlay (incl. Reduced Motion) only openable from Menu/Paused** (`settings_context` returns true only for `Menu | Paused`). Cannot toggle mid-run without pausing. Opener is a text label "O SETTINGS" (top-right), not a clear gear icon → low discoverability.
- **10% horizontal dead gap (x 0.45–0.55)** between STEER and BRAKE → thumb drifting lands in no-op strip → dropped input at critical moments.
- **0.12 steering deadzone** (`STEER_DEADZONE`) → fine control may feel numb.
- **PAUSE target ~12%×14% of viewport** (~96×52 px on 844×390) — height borderline vs 44pt/48dp minimums; top-center where notches live. `viewport-fit=cover` set but **no safe-area-inset handling** for in-game HUD/touch zones → can extend under notch/home indicator.
- **No orientation lock or rotate prompt.** README says "landscape recommended" but nothing enforces it.
- **No haptic feedback** (Bevy has no haptics API).

**PROVEN gaps (HUD readability):**
- **Timer (top-right) has NO background panel.** White label + gold/red value on skydome. Estimated contrast below WCAG 4.5:1 for normal text. Red urgent value **pulses α 0.725→1.0**, further dropping minimum contrast mid-pulse. **Single clearest readability defect.**
- **Combo badge (top-center) also has no panel** — same contrast problem.
- **Combo badge overlaps objective pill.** Combo at top:48px, objective at top:54px, both centered → overlap in y≈54–79 band when multiplier ≥2 (exactly during intense chaining).
- **"Lv {level}" label overlaps minimap.** `UI_TOP = 182` assumes `MAP_SIZE = 120` but actual `MAP_SIZE = 132` → label sits 8 px inside minimap bottom edge. Confirmed layout bug (arithmetic error in comment).
- **Stampede condition color is brown** `srgb(0.72,0.42,0.18)` on 0.35-alpha dark panel → likely lower contrast than other four conditions.

**PROVEN gaps (reduced motion — the largest accessibility gap):**
- **No auto-detection of OS/browser `prefers-reduced-motion`.** `index.html` respects it only for the loading spinner; no `matchMedia` bridge; `Cargo.toml` `web-sys` features are `["Window","Storage"]` (no `MediaQuery`). A player with OS-level reduced motion starts every session with `reduced_motion=false` and must manually find it in Settings (which requires reaching Menu/Paused first).
- **Ungated effects** (NOT consumed by Reduced Motion): particle bursts (chicken feathers/puffs, critter gibs/blood, tire smoke), power-up orb bob+spin, instant-pickup full-screen flash (Health/Time/MegaCoin — note the *damage* flash IS gated but the *pickup* flash is NOT → inconsistent), water ripple shader, chicken/critter waddle bob.
- **`settings.rs` module doc is stale:** says presentation systems "do not consume it yet" — several now do. Signals intentionally incomplete implementation.
- **No unit tests exist for ungated effects** (nothing to test — no branch) → gap is overlooked, not deliberately accepted.

### Replayability

**PROVEN:**
- Deterministic cycle + events + objectives provide per-round variety within a fixed structure.
- Best score + condition bests persist correctly (localStorage + native `best_score.txt`).
- Restart is one action from GameOver (R/Enter/Space or left-2/3 tap) or Paused (R or middle-1/3 tap) — low input friction.

**PROVEN gaps:**
- **Every restart advances the condition cycle.** `end_round` sets `RoundActive=false` on GameOver/Menu → `select_modifier` increments `RoundIndex`. **No "retry this condition" path.** Wreck at 50 on RushHour (Silver) chasing Gold → next round is Stampede → must play ~4 rounds to cycle back. Central motivation drain for completionists; compounds the no-per-condition-gallery gap.
- **Every fresh round has a 3 s countdown** — restart-to-driving is ≥3 s plus game-over read time. No "quick restart" that skips it (though countdown shows condition + medal, so not pure friction).
- **At the audited gameplay baseline, no active leaderboard UI, stats, streaks, dailies, or unlocks existed.** Long-term replayability therefore rested entirely on self-improvement once 15 Gold medals were earned. Leaderboard implementation began after this source snapshot.

### Leaderboard Incentives/Anti-Cheat

**PROVEN (server enforces):**
- `terminal_total == chickens + coins` (hard reject) — matches code invariant.
- `terminal_total ≤ SCORE_CAPS[condition]` (hard reject).
- `max_combo` in 1–5; `condition` 0–4; `platform` enum; `game_over_reason` enum (range/enum checks).
- Name `[A-Z0-9]{3,5}` (server-normalized) → ~62.2 M possible names.
- Session unused/unexpired/condition-bound (D1 one-time write).
- Client HMAC valid (but bypassable after key extraction — correctly treated as nuisance friction).
- Turnstile + rate limits (5 submissions/min/IP) raise automation cost.

**PROVEN (server CANNOT verify — the gap):**
- **Per-hit rate plausibility** — server receives only aggregate `chickens`, `coins`, `round_duration_ms`; no per-hit timestamps or density. `chickens=80, round_duration_ms=60000` indistinguishable from fabricated.
- **Combo legitimacy** — `max_combo` is a single self-reported int. `max_combo=5` with `chickens=5` is impossible in honest play (5× requires ≥20 hits) but **undetectable** by documented checks. No cross-field consistency check proposed.
- **Time–score consistency** — `round_duration_ms` not checked against coins (×1.5 s) + TimeBonuses (×5 s). `coins=50, round_duration_ms=60000` implies 75 s of extension unaccounted for — detectable in principle, not implemented.
- **Objective completion** — `objective_completed` is self-reported; +10 baked into `chickens` → server cannot isolate it.
- **Condition/event co-occurrence** — deterministic plans mean some combos never co-occur (GlassCannon never gets ComboFrenzy), but server doesn't validate. Fabricated submission can claim any combo to maximize cap headroom.
- **Platform/build authenticity** — self-reported; no attestation. Attacker can claim `platform=native` to evade web-specific checks.

**PROVEN (incentive design):**
- Ranking key: `(status, terminal_total DESC, submitted_at ASC)` — **only terminal score matters; ties break by earliest submission.**
- Single-metric optimization: no secondary metrics (duration, efficiency, hits, combos) ranked.
- First-mover pressure: tied scores favor earlier `submitted_at` → incentivizes rushing a reliable high score.
- Anonymous, Sybil-friendly namespace: no accounts; ~62.2 M names; IP-hashed + rate-limited but IPs are cheap to rotate. Classic Sybil surface.
- Local best-score tampering (localStorage/`best_score.txt` trivially editable) does NOT currently affect cloud board (submission is explicit player action at GameOver) — risk only if future "auto-submit local best" is added without re-validation.
- Deterministic conditions enable offline TAS/optimal-route computation → no server-side nonce or per-round entropy invalidates an offline-computed optimal.
- Flag-near-cap moderation mechanism exists but **no specified threshold or review SLA.**

**PROVEN (critical cap error):**
- `LEADERBOARD_ARCHITECTURE.md` §8 derives caps from "up to 90 seconds" — **wrong**; actual max is 99 s via `TIME_CAP`. If `SCORE_CAPS_JSON` uses a 90 s basis, legitimate 91–99 s rounds may be miscalibrated, and fabricated scores assuming 99 s of opportunity could exceed a 90 s-derived cap.

---

## Score Plausibility (Conservative & Theoretical)

### Assumptions (stated explicitly)

1. **Hit-rate estimates are HYPOTHESIS**, not telemetry. They are derived from geometry (respawn distances, populations, radii) and max speed, not observed play.
2. **Combo ramp cost is PROVEN:** 20 consecutive hits within 2.5 s windows to reach 5×; at 1 hit/1.5 s → 30 s ramp; at 1 hit/2.5 s (edge) → 50 s ramp. A realistic round has ~30–60 s at sustained 5× after ramp-up.
3. **Chicken hit rate:** Standard (14 chickens, ~35 u avg spacing in 65 u keep radius) → ~1 hit/3–5 s at 12 u/s. ChickenFrenzy (35 chickens, ~22 u spacing) → ~1 hit/1.5–2.5 s.
4. **Coin rate:** 4 coins/block, 40 u blocks at 12 u/s → ~1 block/3.3 s. Realistically 1–2 coins/block → ~0.3–0.6 coins/s sustained. CoinMagnet burst: up to 24 coins/frame for 4 s, but supply rate-limited by block recycling (~1.3 coins/s fresh).
5. **Base +1 never multiplied by combo** (PROVEN); only `(mult−1)×mod×event` bonus scales.

### Conservative estimates (competent player)

**Casual/competent (Standard, 60–75 s):**
- Combo averages 2–3× (not sustaining 5×)
- ~1 chicken/3 s → 20–25 hits × 2–3 pts = 40–75
- ~0.3 coins/s → 18–22 coins × 1–2 pts = 18–44
- Objective: +10
- **Estimated total: ~70–130** (brackets Standard Gold = 70)

**Expert (ChickenFrenzy, 90 s with coin extensions):**
- Combo 3–5× sustained after ramp
- ~1 chicken/1.5–2 s → 45–60 hits × 3–6 pts = 135–360
- ~0.5 coins/s → 45 coins × 3–5 pts = 135–225
- Objective: +10
- **Estimated total: ~280–595** (well above ChickenFrenzy Gold = 100)

### Aspirational scenario estimate (GlassCannon, 99 s, sustained 5×, no mistakes)

- 9 pts/chicken × (99 s / 2.5 s per hit) ≈ 9 × 40 = 360
- 9 pts/coin × (99 s × 0.3 coins/s) ≈ 9 × 30 = 270
- Objective: +10
- MegaCoin power-ups (~3 × 5 coins × 9 combo) ≈ 135
- **Illustrative scenario total: ~775 (not a proven upper bound)**

**This requires:** never losing combo (hit every <2.5 s for 99 s), never hitting traffic/critters at 12 u/s (96 dmg = instant wreck in GlassCannon), perfect chicken/coin routing, and favorable power-up rolls. **Aspirational, not realistic.** Note: GlassCannon+ComboFrenzy (17/hit) is unreachable in normal play; the realistic max combo-scaled hit is 9.

### Cap-calibration implication

The gap between medal Gold targets and this illustrative ~775 scenario is large. This does **not** prove 775 is the maximum: exact upper bounds require deterministic simulation or measured maximum hit/spawn throughput. `SCORE_CAPS` must either:
- Be generous enough to cover simulated/observed exceptional play (creating a fabrication window), or
- Be set near observed distributions (risking rejection of legitimate outliers).

The architecture correctly chooses generous caps + flag-near-cap, but the flag threshold and review SLA are unspecified, and the 90 s vs 99 s error must be corrected first.

---

## Linked References & Relevance

| Reference | Relevance |
|---|---|
| [WCAG 2.2 §1.4.3 Contrast (Minimum)](https://www.w3.org/TR/WCAG22/#contrast-minimum) | Timer/combo HUD lack background panels; estimated contrast below 4.5:1 on skydome. **High relevance** — clearest readability defect. |
| [WCAG 2.2 §1.4.1 Use of Color](https://www.w3.org/TR/WCAG22/#use-of-color) | Minimap double-encodes with shapes + colors (excellent practice); event banner uses color + text (OK); condition color is Stampede brown (contrast risk). |
| [MDN — prefers-reduced-motion](https://developer.mozilla.org/en-US/docs/Web/CSS/@media/prefers-reduced-motion) | No `matchMedia` bridge; OS-level reduced motion not auto-detected. **Largest accessibility gap.** |
| [Game Accessibility Guidelines](https://gameaccessibilityguidelines.com/) | Single toggle for all non-essential motion; audio/visual redundancy for events; motor feedback (haptics). Several gaps confirmed. |
| [Apple HIG — Buttons/Touch Targets](https://developer.apple.com/design/human-interface-guidelines/buttons) | PAUSE target height borderline vs 44pt minimum; no safe-area-inset handling. |
| [Material 3 — Touch Targets](https://m3.material.io/components/touch-targets/overview) | Same PAUSE target concern; 48dp minimum. |
| [NN/g — Touch Target Size (Fitts's Law)](https://www.nngroup.com/articles/touch-target-size/) | 0.12 steering deadzone; 10% dead gap between steer/brake; fine-control numbness. |
| Steve Swink, *Game Feel* (2009) | Player motion must remain expressive — traffic-always-slower preserves agency. **Confirmed in design.** |
| Jesse Schell, *The Art of Game Design* | Interest curves; dominance (ch. 5); timer urgency pulse as tension feedback. **Pacing aligns.** |
| Hunicke, LeBlanc, Zubek, *MDA: A Formal Approach to Game Design* (2004) ([PDF](https://www.cs.northwestern.edu/~hunicke/MDA.pdf)) | Achievements as aesthetics amplifier, not mechanics driver — objectives are correctly non-dominant. |
| David Sirlin, *Playing to Win* ([sirlin.net/ptw](https://www.sirlin.net/ptw)) | Dominant-strategy avoidance — no pure dominant strategy PROVEN; farm-to-cap opener is the open question. |
| Douceur, *The Sybil Attack* (IPTPS 2002) ([PDF](https://www.cs.cornell.edu/people/egs/sybil.pdf)) | Anonymous namespace + no accounts → Sybil surface for leaderboard flooding. |
| [RFC 2104 — HMAC](https://www.rfc-editor.org/rfc/rfc2104) | HMAC-SHA256 soundness; security here limited by key extraction from public WASM, not algorithm. |
| [Cloudflare Turnstile](https://developers.cloudflare.com/turnstile/) | Raises automation cost; does not authenticate gameplay. Correctly threat-modeled. |
| [OWASP API Security Cheat Sheet](https://cheatsheetseries.owasp.org/cheatsheets/API_Security_Cheat_Sheet.html) | Server-side validation guidance; documented server checks align. |
| [TASVideos](https://tasvideos.org/) | Deterministic conditions enable offline optimal-route computation; no server nonce/entropy invalidates it. |

**Note:** External URLs could not be live-verified in this audit context (web tools unavailable). Links are to well-known, stable publications; the developer should verify any link before relying on it.

---

## Priority/Confidence Matrix

| # | Finding | Severity | Confidence | Type |
|---|---|---|---|---|
| 1 | 90 s vs 99 s cap error in `LEADERBOARD_ARCHITECTURE.md` §8 / README | **Critical** | PROVEN | Fix |
| 2 | No `prefers-reduced-motion` auto-detection (no `matchMedia` bridge) | **High** | PROVEN | Fix |
| 3 | Timer/combo HUD lack background panels → WCAG contrast failure | **High** | PROVEN (estimate) | Fix |
| 4 | Pickup full-screen flash not gated by Reduced Motion (damage flash IS) | **High** | PROVEN | Fix |
| 5 | Particle bursts, water shader, orb bob, waddle not gated by Reduced Motion | **High** | PROVEN | Fix |
| 6 | No per-condition medal gallery | **High** | PROVEN | Fix/Feature |
| 7 | No "retry this condition" path — every restart advances cycle | **High** | PROVEN | Fix/Feature |
| 8 | Combo badge overlaps objective pill at top-center | **Medium** | PROVEN | Fix |
| 9 | "Lv" label overlaps minimap (arithmetic error: MAP_SIZE=132 not 120) | **Low** | PROVEN | Fix |
| 10 | No touch reverse; no safe-area-inset; PAUSE target borderline | **Medium** | PROVEN | Fix |
| 11 | No per-event audio differentiation; no event-duration indicator | **Medium** | PROVEN gap | Fix/Feature |
| 12 | `settings.rs` module doc stale ("do not consume it yet") | **Low** | PROVEN | Fix |
| 13 | README stale: "no on-screen touch equivalent" for mute | **Low** | PROVEN | Fix |
| 14 | Stampede brown condition color — contrast risk | **Medium** | PROVEN (estimate) | Investigate |
| 15 | Server cannot verify combo/time/objective/event-co-occurrence consistency | **High** | PROVEN | Architectural |
| 16 | Flag-near-cap has no threshold or review SLA | **Medium** | PROVEN | Policy |
| 17 | GlassCannon likely under-rewarded (+14% Gold for +100% damage) | **Medium** | HYPOTHESIS | Telemetry |
| 18 | Farm-to-cap-then-chain may be degenerate opener | **Medium** | HYPOTHESIS | Telemetry |
| 19 | −2 critter score penalty may be under-tuned deterrent | **Low** | HYPOTHESIS | Telemetry |
| 20 | Late-round L5–6 may be frustrating spike vs frenetic finish | **Medium** | HYPOTHESIS | Telemetry |
| 21 | Event banner no duration indicator; fixed timing reduces variety | **Low** | PROVEN/HYPOTHESIS | Telemetry |
| 22 | Health power-up rarity (15%) may make early mistakes unrecoverable | **Low** | HYPOTHESIS | Telemetry |
| 23 | Single-metric leaderboard (no secondary metrics) | **Low** | PROVEN (by design) | Design |
| 24 | First-mover tie-break pressure | **Low** | PROVEN (by design) | Design |
| 25 | No cross-field consistency: `max_combo=5` with `chickens=5` undetectable | **Medium** | PROVEN | Fix (server) |

---

## High-Confidence Fixes

These are PROVEN from source/tests, do not require telemetry, and do not change gameplay balance:

1. **Correct the 90 s → 99 s cap in `LEADERBOARD_ARCHITECTURE.md` §8 and README.** Recompute `SCORE_CAPS_JSON` on a 99 s basis. This is the single most urgent fix — miscalibrated caps either reject legitimate play or leave a fabrication gap. *(#1)*
2. **Add `prefers-reduced-motion` auto-detection.** Bridge `window.matchMedia('(prefers-reduced-motion: reduce)')` into `Settings::load_settings`; add `MediaQuery` to `Cargo.toml` `web-sys` features. Default `reduced_motion=true` when OS signals it. *(#2)*
3. **Gate remaining presentation effects on Reduced Motion:** particle bursts (chicken/critter/smoke), power-up orb bob+spin, instant-pickup full-screen flash (make consistent with damage flash), water ripple shader, waddle bob. A single toggle covering all non-essential motion is the GAG recommendation. *(#4, #5)*
4. **Add background panels to timer and combo HUD elements** to meet WCAG 1.4.3 contrast. Stop the urgent-timer alpha pulse from dropping below the contrast floor (or gate the pulse on reduced motion, which it partially is). *(#3)*
5. **Fix combo/objective overlap** — move combo badge or objective pill so they don't collide in the y≈54–79 band during chaining. *(#8)*
6. **Fix "Lv" label minimap overlap** — correct `UI_TOP` arithmetic (MAP_SIZE=132, not 120) or reposition. *(#9)*
7. **Update stale `settings.rs` module doc** to reflect that presentation systems now consume Reduced Motion. *(#12)*
8. **Update stale README** mute claim (touch mute IS available via Settings overlay). *(#13)*
9. **Add per-condition medal gallery to the menu** so players can see which medals they still need. *(#6)*
10. **Add a cross-field consistency check on the server:** reject `max_combo=5` when `chickens + coins < 20` (5× requires ≥20 hits). This is a cheap, deterministic invariant. *(#25)*

---

## Telemetry / Playtest Experiments

These are HYPOTHESIS-driven and **must not** be acted on until data is collected. Do not implement balance changes from this section without evidence.

| Experiment | Hypothesis tested | Metric needed | Decision threshold |
|---|---|---|---|
| **Coin-rate-over-time** | Farm-to-cap-then-chain is a degenerate opener (#18) | Coin collection rate vs elapsed time, per condition | Bimodal "farm then fight" distribution in >40% of expert runs → consider tightening coin cap or making early chickens more attractive |
| **Median score by condition** | GlassCannon under-rewarded (#17) | Median terminal total per condition, n≥100 each | If GlassCannon median < Standard median → raise GlassCannon medal thresholds or lower damage mult |
| **Combo-length distribution** | 2.5 s window too forgiving/punishing | Avg combo length, 50th/95th pct multiplier, combo-break reasons | If <5% of runs ever reach 5× → consider widening window or lowering tier thresholds; if >50% sustain 5× → tighten |
| **Per-10 s scoring rate** | Late-round L5–6 is a frustrating spike (#20) | Score rate in 0–10/10–20/…/90–99 s bins | If last 20 s scoring rate < 50% of peak → difficulty ramp too steep at end |
| **Critter-hit frequency** | −2 score penalty is cosmetic deterrent (#19) | Critter hits per round, score at game-over | If players routinely eat >3 critters/run and still podium → raise score penalty or add combo-break on critter |
| **Health at game-over** | Health power-up too rare (#22) | Health distribution at game-over, wreck rate | If >60% of wrecks occur at <35 HP recovered → raise Health spawn weight |
| **Score-source mix** | Chicken aggression dominates completely | Chicken % vs coin % of terminal total | If coin % < 10% across all conditions → coins may need a non-time yield boost |
| **Event-banner comprehension** | Players can't identify events without audio cue | Playtest: name the active event after 2 s, n≥10 | If <50% accuracy → add per-event audio sting |
| **Touch dead-gap drops** | 10% gap causes critical-moment input drops | Touchplay: input-drop rate in steer↔brake transitions | If >5% drops → narrow the gap or add interpolation |
| **Leaderboard cap calibration** | Cap threshold is correct | Distribution of submitted terminal totals vs cap | If >5% of legitimate submissions are flagged → cap too tight; if fabricated submissions cluster near cap → flag threshold needs tuning |

**Telemetry integrity note:** The architecture honestly states WASM telemetry is "advisory and forgeable." Telemetry should be used for *design decisions*, not *cheat detection*. For cheat detection, rely on server-side invariants (§10 above) and future server-side simulation (deferred per `MULTIPLAYER_PLAN.md`).

---

## Do Not Implement Without Evidence

These are explicitly flagged as HYPOTHESIS or design-judgment calls. Implementing them now would risk unbalancing a tightly-bounded system on speculation:

1. **Do not change the combo window (2.5 s), tier thresholds (5/10/15/20), or 5× cap** without combo-length distribution telemetry. The current values are PROVEN to produce a genuine skill ceiling; tightening or loosening speculatively could collapse the difficulty curve.
2. **Do not change GlassCannon's damage multiplier or combo bonus multiplier** without median-score-by-condition data. The trade is PROVEN genuine; whether it's under-rewarded is HYPOTHESIS. Adjusting one side without data breaks the trade.
3. **Do not lower the 90 s coin cap or the 99 s Time cap** without coin-rate-over-time telemetry. The cap is the correct anti-degeneracy guard; tightening it speculatively could kill the time-farming loop that synergizes with combo maintenance.
4. **Do not change the critter score penalty (−2) or add critter-induced combo breaks** without critter-hit-frequency data. The current design (critters don't break combo directly, only health+score) is deliberate; making critters combo-breakers could make bad luck catastrophic.
5. **Do not change the event schedule (fixed 15–23/40–48 s) or add randomness** without playtest on replay fatigue. Determinism enables the Event Waiter strategy but is PROVEN net-negative; adding entropy has correctness implications for the deterministic event plans and tests.
6. **Do not change health power-up spawn weight (15%)** without health-at-game-over data. The rarity may be intentional to make damage consequential.
7. **Do not add accounts/OAuth to the leaderboard** without weighing the anonymous-play value against the Sybil surface. The architecture explicitly lists accounts as a non-goal; the Sybil risk is acknowledged and mitigated by rate limits + caps. This is a product decision, not a proven defect.
8. **Do not add secondary leaderboard metrics (duration, efficiency, combos)** without considering first-mover pressure and single-metric clarity. The current single-metric board is simple and legible; adding metrics changes the optimization landscape fundamentally.
9. **Do not add a "retry this condition" path without resolving the medal-gallery gap first.** Retry-without-gallery could worsen the motivation structure (players retry one condition, never see the full medal map). Fix the gallery first, then evaluate retry.
10. **Do not implement server-side simulation** before it's specified in `MULTIPLAYER_PLAN.md`. The architecture defers it for good reason (cost, determinism guarantees); premature implementation risks a brittle validator.

---

## Actionable Next Wave

A small, ordered set of next actions, each independently shippable, highest-leverage first:

1. **Fix the 99 s cap error** in `LEADERBOARD_ARCHITECTURE.md` §8 + README; recompute `SCORE_CAPS_JSON` on a 99 s basis. *(#1, blocks correct cap shipping)*
2. **Add `prefers-reduced-motion` `matchMedia` bridge** + `MediaQuery` web-sys feature; default `reduced_motion` from OS signal on load. *(#2)*
3. **Gate ungated effects on Reduced Motion** (particles, orb bob, pickup flash, water shader, waddle) — make pickup flash consistent with damage flash. *(#4, #5)*
4. **Add background panels to timer + combo HUD**; ensure urgent-timer pulse doesn't breach contrast floor. *(#3)*
5. **Fix the two layout bugs:** combo/objective overlap (#8) and Lv/minimap overlap (#9).
6. **Add per-condition medal gallery to the menu.** *(#6)*
7. **Add server-side `max_combo ↔ hit count` consistency check** (`max_combo=5` requires `chickens + coins ≥ 20`). *(#25)*
8. **Specify the flag-near-cap threshold and review SLA** in the leaderboard architecture doc. *(#16)*
9. **Instrument telemetry** for the experiments in §Telemetry (coin-rate-over-time, median-by-condition, combo-length, per-10 s scoring, critter-hit frequency, health-at-game-over, score-source mix). Collect before any balance change. *(#17–22)*
10. **Evaluate "retry this condition"** after the medal gallery ships and telemetry shows whether the cycle friction is actually causing player drop-off. *(#7)*

---

*End of audit. All claims sourced to `src/` files, tests, or explicitly marked HYPOTHESIS. No game source was edited. No telemetry was invented. Conflicts between reports were resolved by preferring source/test evidence over documentation and inference.*
