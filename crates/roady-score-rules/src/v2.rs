//! Pure, engine-independent gameplay rules for `roady-rules.v2`.
//!
//! Integer widths, PRNG consumption, and byte encodings in this module are
//! protocol behavior. In particular, range mapping deliberately uses modulo.

use sha2::{Digest, Sha256};
use std::collections::BTreeSet;

pub const PROTOCOL_VERSION: u8 = 2;
pub const RULES_VERSION: u8 = 2;
pub const POLICY_VERSION: u8 = 1;
pub const RULES_VERSION_ID: &str = "roady-rules.v2";
pub const MODE: &str = "rotation";
pub const CLUCK_HUNT_CATEGORY: &str = "rotation.v1.cluck_hunt";
pub const RIGHT_OF_WAY_CATEGORY: &str = "rotation.v1.right_of_way";

pub const SCHEDULE_SEGMENTS: usize = 16;
pub const INITIAL_GRACE_MS: u64 = 8_000;
pub const TELEGRAPH_MS: u64 = 3_000;
pub const ACTIVE_MS: u64 = 18_000;
pub const COOLDOWN_MS: u64 = 7_000;
pub const CADENCE_MS: u64 = TELEGRAPH_MS + ACTIVE_MS + COOLDOWN_MS;

pub const COMBO_WINDOW_MS: u64 = 2_500;
pub const OBJECTIVE_AWARD: u32 = 10;
pub const RANKED_WAVE_AWARD: u32 = 2;
pub const COIN_AWARD: u32 = 1;
pub const MEGA_COIN_AWARD: u32 = 5;
pub const CRITTER_PENALTY: u32 = 2;
pub const COIN_TIME_BONUS_MS: u64 = 1_500;
pub const PACKAGE_TIME_BONUS_MS: u64 = 3_000;
pub const TIME_PICKUP_BONUS_MS: u64 = 5_000;
pub const COIN_AND_PACKAGE_TIME_CAP_MS: u64 = 90_000;
pub const TIME_PICKUP_CAP_MS: u64 = 99_000;

pub const PACKAGE_CAPACITY: u8 = 3;
pub const PACKAGE_BASE_AWARD: u32 = 5;
pub const COURTESY_BASE_AWARD: u32 = 2;
pub const ANIMAL_HIT_DELTA: i64 = -10;
pub const INITIAL_PREMIUM_BPS: u32 = 10_000;
pub const PREMIUM_DECAY_BPS: u32 = 9_000;
pub const GUILT_MULTIPLIER_BPS: u32 = 5_000;
pub const GUILT_MS: u64 = 5_000;
pub const COURTESY_COOLDOWN_MS: u64 = 500;
pub const COURTESY_MIN_SPEED: f32 = 4.0;
pub const CHICKEN_HIT_RADIUS: f32 = 1.0;
pub const COURTESY_OUTER_RADIUS: f32 = 2.12;

pub const FRENZY_ELIGIBLE_MS: u64 = 8_000;
pub const FRENZY_INTERVAL_SPAN: u64 = 4_001;
pub const FRENZY_ROLL_RANGE: u64 = 10_000;
pub const FRENZY_SUCCESS_RANGE: u64 = 400;
pub const FRENZY_PITY_MS: u64 = 55_000;
pub const FRENZY_ORB_LIFETIME_MS: u64 = 12_000;
pub const FRENZY_RELOCATION_AGE_MS: u64 = 6_000;
pub const FRENZY_TELEGRAPH_MS: u64 = 2_000;
pub const FRENZY_ACTIVE_MS: u64 = 15_000;

const SPLITMIX_INCREMENT: u64 = 0x9e37_79b9_7f4a_7c15;
const FNV_OFFSET: u64 = 0xcbf2_9ce4_8422_2325;
const FNV_PRIME: u64 = 0x0000_0100_0000_01b3;

pub const ROTATION_DOMAIN: &str = "roady.rotation.v2.rotation";
pub const SCHEDULED_EVENTS_DOMAIN: &str = "roady.rotation.v2.scheduled_events";
pub const FRENZY_INTERVAL_DOMAIN: &str = "roady.rotation.v2.frenzy.interval";
pub const FRENZY_ROLL_DOMAIN: &str = "roady.rotation.v2.frenzy.roll";
pub const FRENZY_KIND_DOMAIN: &str = "roady.rotation.v2.frenzy.kind";
pub const FRENZY_POSITION_DOMAIN: &str = "roady.rotation.v2.frenzy.position";
pub const FRENZY_RELOCATION_DOMAIN: &str = "roady.rotation.v2.frenzy.relocation";

/// Fixed protocol ordinal (serialized as one byte).
#[repr(u8)]
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Conduct {
    CluckHunt = 0,
    RightOfWay = 1,
}

/// Fixed protocol ordinal (serialized as one byte).
#[repr(u8)]
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Effect {
    Standard = 0,
    RushHour = 1,
    ChickenFrenzy = 2,
    Stampede = 3,
    GlassCannon = 4,
}

/// Fixed protocol ordinal. These are the four scheduled-event flavors.
#[repr(u8)]
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ScheduledEvent {
    TrafficSurge = 0,
    ChickenBurst = 1,
    ComboFrenzy = 2,
    CritterBurst = 3,
}

#[repr(u8)]
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum EventKind {
    ChickenHit = 1,
    CoinCollected = 2,
    TimePickup = 3,
    ObjectiveCompleted = 4,
    CritterPenalty = 5,
    SegmentChanged = 6,
    Terminal = 7,
    PackagePickup = 8,
    PackageDelivery = 9,
    CourtesyAward = 10,
    AnimalHit = 11,
    WaveAward = 12,
    CoinAward = 13,
    FrenzyChanged = 14,
}

#[repr(u8)]
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum TerminalReason {
    TimeUp = 1,
    Wrecked = 2,
}
#[repr(u8)]
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Platform {
    Web = 1,
    Native = 2,
}
#[repr(u8)]
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Objective {
    HitChickens = 1,
    CollectCoins = 2,
    ReachCombo = 3,
    DeliverPackages = 4,
    CourtesyAwards = 5,
}
#[repr(u8)]
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum FrenzyPhase {
    Spawned = 1,
    Telegraph = 2,
    Active = 3,
    Expired = 4,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ArithmeticError;

/// SplitMix64 with the contract's explicit wrapping behavior.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SplitMix64 {
    state: u64,
}

impl SplitMix64 {
    pub const fn from_state(state: u64) -> Self {
        Self { state }
    }
    pub const fn state(&self) -> u64 {
        self.state
    }
    pub fn next_u64(&mut self) -> u64 {
        self.state = self.state.wrapping_add(SPLITMIX_INCREMENT);
        splitmix_finalizer(self.state)
    }
    pub fn range(&mut self, n: u64) -> u64 {
        assert!(n != 0, "SplitMix range must be nonzero");
        self.next_u64() % n
    }
}

pub const fn splitmix_finalizer(mut z: u64) -> u64 {
    z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
    z ^ (z >> 31)
}

/// FNV state before finalization, useful as a cross-language anchor.
pub fn stream_fnv(seed: &[u8; 32], domain: &str) -> u64 {
    let mut hash = FNV_OFFSET;
    for byte in seed
        .iter()
        .copied()
        .chain((domain.len() as u32).to_le_bytes())
        .chain(domain.bytes())
    {
        hash ^= u64::from(byte);
        hash = hash.wrapping_mul(FNV_PRIME);
    }
    hash
}

pub fn stream_state(seed: &[u8; 32], domain: &str) -> u64 {
    splitmix_finalizer(stream_fnv(seed, domain))
}

pub fn stream(seed: &[u8; 32], domain: &str) -> SplitMix64 {
    SplitMix64::from_state(stream_state(seed, domain))
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RotationWindow {
    pub effect: Effect,
    pub telegraph_start_ms: u64,
    pub active_start_ms: u64,
    pub active_end_ms: u64,
    pub cooldown_end_ms: u64,
}

pub const fn window_times(index: usize, effect: Effect) -> RotationWindow {
    let telegraph_start_ms = INITIAL_GRACE_MS + index as u64 * CADENCE_MS;
    let active_start_ms = telegraph_start_ms + TELEGRAPH_MS;
    let active_end_ms = active_start_ms + ACTIVE_MS;
    RotationWindow {
        effect,
        telegraph_start_ms,
        active_start_ms,
        active_end_ms,
        cooldown_end_ms: active_end_ms + COOLDOWN_MS,
    }
}

/// Generate all committed windows. Repeats consume one retry; a second repeat
/// resolves to the previous effect's cyclic successor without another draw.
pub fn rotation_schedule(seed: &[u8; 32]) -> [RotationWindow; SCHEDULE_SEGMENTS] {
    let mut rng = stream(seed, ROTATION_DOMAIN);
    let mut previous = None;
    core::array::from_fn(|index| {
        let mut effect = rotation_pool(rng.range(3));
        if previous == Some(effect) {
            effect = rotation_pool(rng.range(3));
            if previous == Some(effect) {
                effect = cyclic_successor(effect);
            }
        }
        previous = Some(effect);
        window_times(index, effect)
    })
}

const fn rotation_pool(index: u64) -> Effect {
    match index {
        0 => Effect::RushHour,
        1 => Effect::Stampede,
        2 => Effect::GlassCannon,
        _ => unreachable!(),
    }
}
const fn cyclic_successor(effect: Effect) -> Effect {
    match effect {
        Effect::RushHour => Effect::Stampede,
        Effect::Stampede => Effect::GlassCannon,
        Effect::GlassCannon => Effect::RushHour,
        _ => Effect::RushHour,
    }
}

pub const EVENT_WINDOWS: [(u64, u64); 2] = [(15_000, 23_000), (40_000, 48_000)];

/// Number of fully completed ranked waves at an active-play timestamp.
pub const fn completed_waves(active_ms: u64) -> u32 {
    let first_end = INITIAL_GRACE_MS + CADENCE_MS;
    if active_ms < first_end {
        0
    } else {
        let waves = 1 + (active_ms - first_end) / CADENCE_MS;
        if waves > u32::MAX as u64 {
            u32::MAX
        } else {
            waves as u32
        }
    }
}

/// Select E0 and E1 while removing the flavor matching the effect active at
/// each event's start. Both draws come from one independent event stream.
pub fn scheduled_events(
    seed: &[u8; 32],
    schedule: &[RotationWindow; SCHEDULE_SEGMENTS],
) -> [ScheduledEvent; 2] {
    let mut rng = stream(seed, SCHEDULED_EVENTS_DOMAIN);
    core::array::from_fn(|index| {
        let active = active_effect_at(schedule, EVENT_WINDOWS[index].0);
        let excluded = match active {
            Some(Effect::RushHour) => Some(ScheduledEvent::TrafficSurge),
            Some(Effect::Stampede) => Some(ScheduledEvent::CritterBurst),
            Some(Effect::GlassCannon) => Some(ScheduledEvent::ComboFrenzy),
            _ => None,
        };
        let eligible: [ScheduledEvent; 3] = match excluded {
            Some(ScheduledEvent::TrafficSurge) => [
                ScheduledEvent::ChickenBurst,
                ScheduledEvent::ComboFrenzy,
                ScheduledEvent::CritterBurst,
            ],
            Some(ScheduledEvent::ComboFrenzy) => [
                ScheduledEvent::TrafficSurge,
                ScheduledEvent::ChickenBurst,
                ScheduledEvent::CritterBurst,
            ],
            Some(ScheduledEvent::CritterBurst) => [
                ScheduledEvent::TrafficSurge,
                ScheduledEvent::ChickenBurst,
                ScheduledEvent::ComboFrenzy,
            ],
            _ => [
                ScheduledEvent::TrafficSurge,
                ScheduledEvent::ChickenBurst,
                ScheduledEvent::ComboFrenzy,
            ],
        };
        eligible[rng.range(3) as usize]
    })
}

pub fn active_effect_at(schedule: &[RotationWindow], active_ms: u64) -> Option<Effect> {
    schedule
        .iter()
        .find(|w| active_ms >= w.active_start_ms && active_ms < w.active_end_ms)
        .map(|w| w.effect)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FrenzyOpportunity {
    pub at_ms: u64,
    pub roll_residue: u16,
    pub spawn: bool,
    pub pity: bool,
}

/// Generate opportunities until spawn or `through_ms` (inclusive). The one
/// interval draw and one roll draw per emitted opportunity are byte-frozen.
pub fn frenzy_opportunities(seed: &[u8; 32], through_ms: u64) -> Vec<FrenzyOpportunity> {
    let mut intervals = stream(seed, FRENZY_INTERVAL_DOMAIN);
    let mut rolls = stream(seed, FRENZY_ROLL_DOMAIN);
    let mut at = FRENZY_ELIGIBLE_MS + intervals.range(FRENZY_INTERVAL_SPAN);
    let mut result = Vec::new();
    while at <= through_ms {
        let roll_residue = rolls.range(FRENZY_ROLL_RANGE) as u16;
        let pity = at >= FRENZY_PITY_MS;
        let spawn = roll_residue < FRENZY_SUCCESS_RANGE as u16 || pity;
        result.push(FrenzyOpportunity {
            at_ms: at,
            roll_residue,
            spawn,
            pity,
        });
        if spawn {
            break;
        }
        at = at.saturating_add(FRENZY_ELIGIBLE_MS + intervals.range(FRENZY_INTERVAL_SPAN));
    }
    result
}

/// Expiry wins at the exact lifetime boundary.
pub const fn frenzy_orb_alive(spawn_ms: u64, now_ms: u64) -> bool {
    now_ms >= spawn_ms && now_ms < spawn_ms.saturating_add(FRENZY_ORB_LIFETIME_MS)
}

/// Consume the contractually fixed sixteen relocation draws. Coordinates are
/// `(lateral, ahead)` in the car right/forward basis; road and exclusion tests
/// remain caller-owned because they depend on world geometry.
pub fn frenzy_relocation_candidates(seed: &[u8; 32]) -> [(f32, f32); 8] {
    let mut rng = stream(seed, FRENZY_RELOCATION_DOMAIN);
    core::array::from_fn(|_| {
        let lateral_units = (rng.next_u64() % 2_001) as i64 - 1_000;
        let ahead_units = rng.next_u64() % 1_001;
        (
            lateral_units as f32 * 22.0 / 1_000.0,
            13.75 + ahead_units as f32 * 11.25 / 1_000.0,
        )
    })
}

pub const fn combo_multiplier(count: u32) -> u8 {
    match count {
        0..=4 => 1,
        5..=9 => 2,
        10..=14 => 3,
        15..=19 => 4,
        _ => 5,
    }
}

pub const fn cluck_direct_award(chicken_burst: bool, frenzy: bool) -> u32 {
    1u32.saturating_add(chicken_burst as u32)
        .saturating_add(frenzy as u32)
}

pub const fn cluck_combo_bonus(multiplier: u8, glass_cannon: bool, combo_frenzy: bool) -> u32 {
    (multiplier.saturating_sub(1) as u32)
        .saturating_mul(if glass_cannon { 2 } else { 1 })
        .saturating_mul(if combo_frenzy { 2 } else { 1 })
}

pub const fn cluck_objective(bucket: u32) -> u32 {
    bucket.saturating_add(OBJECTIVE_AWARD)
}
pub const fn cluck_critter_penalty(bucket: u32) -> u32 {
    bucket.saturating_sub(CRITTER_PENALTY)
}
pub const fn cluck_coin_award(bucket: u32, mega: bool) -> u32 {
    bucket.saturating_add(if mega { MEGA_COIN_AWARD } else { COIN_AWARD })
}
pub const fn cluck_wave_award(bucket: u32, ranked: bool) -> u32 {
    if ranked {
        bucket.saturating_add(RANKED_WAVE_AWARD)
    } else {
        bucket
    }
}
pub const fn cluck_terminal(chickens: u32, coins: u32) -> Result<u32, ArithmeticError> {
    match chickens.checked_add(coins) {
        Some(v) => Ok(v),
        None => Err(ArithmeticError),
    }
}
pub const fn cluck_terminal_display(chickens: u32, coins: u32) -> u32 {
    chickens.saturating_add(coins)
}

/// Saturating live buckets with a checked protocol terminal aggregate.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct CluckHunt {
    pub chickens: u32,
    pub coins: u32,
    pub objective_completed: bool,
    pub max_combo: u8,
}

impl CluckHunt {
    pub const fn new() -> Self {
        Self {
            chickens: 0,
            coins: 0,
            objective_completed: false,
            max_combo: 1,
        }
    }
    pub fn chicken_hit(
        &mut self,
        combo_count: u32,
        chicken_burst: bool,
        frenzy: bool,
        glass_cannon: bool,
        combo_frenzy: bool,
    ) -> u32 {
        let multiplier = combo_multiplier(combo_count);
        self.max_combo = if self.max_combo > multiplier {
            self.max_combo
        } else {
            multiplier
        };
        let award = cluck_direct_award(chicken_burst, frenzy).saturating_add(cluck_combo_bonus(
            multiplier,
            glass_cannon,
            combo_frenzy,
        ));
        self.chickens = self.chickens.saturating_add(award);
        award
    }
    pub fn coin(&mut self, mega: bool) -> u32 {
        let award = if mega { MEGA_COIN_AWARD } else { COIN_AWARD };
        self.coins = self.coins.saturating_add(award);
        award
    }
    pub fn critter_penalty(&mut self) {
        self.chickens = cluck_critter_penalty(self.chickens);
    }
    /// Returns true only on the one completion edge.
    pub fn complete_objective(&mut self) -> bool {
        if self.objective_completed {
            return false;
        }
        self.chickens = cluck_objective(self.chickens);
        self.objective_completed = true;
        true
    }
    pub fn wave(&mut self, ranked: bool) {
        self.chickens = cluck_wave_award(self.chickens, ranked);
    }
    pub const fn terminal_total(&self) -> Result<u32, ArithmeticError> {
        cluck_terminal(self.chickens, self.coins)
    }
}

pub fn coin_clock(current_ms: u64) -> u64 {
    current_ms
        .min(COIN_AND_PACKAGE_TIME_CAP_MS)
        .saturating_add(COIN_TIME_BONUS_MS)
        .min(COIN_AND_PACKAGE_TIME_CAP_MS)
}
pub fn package_clock(current_ms: u64) -> u64 {
    current_ms
        .saturating_add(PACKAGE_TIME_BONUS_MS)
        .min(COIN_AND_PACKAGE_TIME_CAP_MS)
}
pub fn time_pickup_clock(current_ms: u64) -> u64 {
    current_ms
        .saturating_add(TIME_PICKUP_BONUS_MS)
        .min(TIME_PICKUP_CAP_MS)
}
pub fn health_pickup(current: u32) -> u32 {
    current.saturating_add(35).min(100)
}

/// Floating-point gameplay-health counterpart. Non-finite values are clamped
/// by the same Rust `clamp` semantics used by the live health resource.
pub fn health_pickup_f32(current: f32) -> f32 {
    (current + 35.0).clamp(0.0, 100.0)
}

pub fn traffic_target(level: u32, rush_hour: bool, traffic_surge: bool) -> u32 {
    let baseline = 1u32.saturating_add(level / 2).min(8);
    baseline
        .saturating_mul(if rush_hour { 2 } else { 1 })
        .saturating_mul(if traffic_surge { 2 } else { 1 })
        .min(8)
}
pub fn traffic_speed(level: u32, speed_roll: f32, rush_hour: bool, traffic_surge: bool) -> f32 {
    ((5.0 + level as f32 * 0.7)
        * (0.85 + speed_roll * 0.30)
        * if rush_hour { 1.35 } else { 1.0 }
        * if traffic_surge { 1.25 } else { 1.0 })
    .min(11.5)
}
pub fn chicken_target(chicken_burst: bool, frenzy: bool) -> u32 {
    (14 * if chicken_burst { 2 } else { 1 } * if frenzy { 2 } else { 1 }).min(40)
}
pub fn critter_target(stampede: bool, critter_burst: bool) -> u32 {
    (5 * if stampede { 2 } else { 1 } + if critter_burst { 5 } else { 0 }).min(16)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RightOfWay {
    pub accumulator: i64,
    pub premium_bps: u32,
    pub delivery_chain: u32,
    pub max_delivery_chain: u32,
    pub carried_packages: u8,
    pub packages_delivered: u32,
    pub courtesy_count: u32,
    pub coins_collected: u32,
    pub animal_hits: u32,
    pub objective_completed: bool,
    pub guilt_remaining_ms: u64,
    pub remaining_ms: u64,
}

impl Default for RightOfWay {
    fn default() -> Self {
        Self {
            accumulator: 0,
            premium_bps: INITIAL_PREMIUM_BPS,
            delivery_chain: 0,
            max_delivery_chain: 0,
            carried_packages: 0,
            packages_delivered: 0,
            courtesy_count: 0,
            coins_collected: 0,
            animal_hits: 0,
            objective_completed: false,
            guilt_remaining_ms: 0,
            remaining_ms: 0,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PositiveTransition {
    pub base: u32,
    pub credited: u32,
    pub before: i64,
    pub after: i64,
}

impl RightOfWay {
    pub const fn with_remaining(remaining_ms: u64) -> Self {
        Self {
            remaining_ms,
            ..Self::new()
        }
    }
    pub const fn new() -> Self {
        Self {
            accumulator: 0,
            premium_bps: INITIAL_PREMIUM_BPS,
            delivery_chain: 0,
            max_delivery_chain: 0,
            carried_packages: 0,
            packages_delivered: 0,
            courtesy_count: 0,
            coins_collected: 0,
            animal_hits: 0,
            objective_completed: false,
            guilt_remaining_ms: 0,
            remaining_ms: 0,
        }
    }
    pub fn tick_guilt(&mut self, elapsed_ms: u64) {
        self.guilt_remaining_ms = self.guilt_remaining_ms.saturating_sub(elapsed_ms);
    }
    pub fn pickup_package(&mut self) -> bool {
        if self.carried_packages >= PACKAGE_CAPACITY {
            false
        } else {
            self.carried_packages += 1;
            true
        }
    }
    pub fn positive_award(&mut self, base: u32) -> Result<PositiveTransition, ArithmeticError> {
        let credited = credited_positive(base, self.premium_bps, self.guilt_remaining_ms != 0)?;
        let before = self.accumulator;
        self.accumulator = self
            .accumulator
            .checked_add(i64::from(credited))
            .ok_or(ArithmeticError)?;
        Ok(PositiveTransition {
            base,
            credited,
            before,
            after: self.accumulator,
        })
    }
    /// Deliver one carried package. Call repeatedly to preserve package order.
    pub fn deliver_package(&mut self) -> Result<Option<PositiveTransition>, ArithmeticError> {
        if self.carried_packages == 0 {
            return Ok(None);
        }
        // Build on a copy so a rejected checked transition is atomic.
        let mut next = *self;
        let base = PACKAGE_BASE_AWARD
            .checked_add(next.delivery_chain)
            .ok_or(ArithmeticError)?;
        let award = next.positive_award(base)?;
        next.delivery_chain = next.delivery_chain.checked_add(1).ok_or(ArithmeticError)?;
        next.max_delivery_chain = next.max_delivery_chain.max(next.delivery_chain);
        next.packages_delivered = next
            .packages_delivered
            .checked_add(1)
            .ok_or(ArithmeticError)?;
        next.carried_packages -= 1;
        next.remaining_ms = package_clock(next.remaining_ms);
        *self = next;
        Ok(Some(award))
    }
    pub fn coin(&mut self) -> Result<PositiveTransition, ArithmeticError> {
        let mut next = *self;
        let result = next.positive_award(COIN_AWARD)?;
        next.coins_collected = next.coins_collected.checked_add(1).ok_or(ArithmeticError)?;
        next.remaining_ms = coin_clock(next.remaining_ms);
        *self = next;
        Ok(result)
    }
    pub fn courtesy(&mut self) -> Result<PositiveTransition, ArithmeticError> {
        let mut next = *self;
        let result = next.positive_award(COURTESY_BASE_AWARD)?;
        if result.credited > 0 {
            next.courtesy_count = next.courtesy_count.checked_add(1).ok_or(ArithmeticError)?;
        }
        *self = next;
        Ok(result)
    }
    /// Apply the mission reward on its one round-wide completion edge.
    pub fn objective(&mut self) -> Result<Option<PositiveTransition>, ArithmeticError> {
        if self.objective_completed {
            return Ok(None);
        }
        let mut next = *self;
        let award = next.positive_award(OBJECTIVE_AWARD)?;
        next.objective_completed = true;
        *self = next;
        Ok(Some(award))
    }
    pub fn wave(&mut self, ranked: bool) -> Result<Option<PositiveTransition>, ArithmeticError> {
        if ranked {
            self.positive_award(RANKED_WAVE_AWARD).map(Some)
        } else {
            Ok(None)
        }
    }
    pub fn animal_hit(&mut self) -> Result<(i64, i64), ArithmeticError> {
        let mut next = *self;
        let before = next.accumulator;
        next.accumulator = next
            .accumulator
            .checked_add(ANIMAL_HIT_DELTA)
            .ok_or(ArithmeticError)?;
        next.premium_bps =
            ((u64::from(next.premium_bps) * u64::from(PREMIUM_DECAY_BPS)) / 10_000) as u32;
        next.delivery_chain = 0;
        next.guilt_remaining_ms = GUILT_MS;
        next.animal_hits = next.animal_hits.checked_add(1).ok_or(ArithmeticError)?;
        *self = next;
        Ok((before, self.accumulator))
    }
    pub fn terminal_total(&self) -> Result<u32, ArithmeticError> {
        u32::try_from(self.accumulator.max(0)).map_err(|_| ArithmeticError)
    }
}

/// Exact floor-based premium/guilt calculation with checked intermediates.
pub fn credited_positive(base: u32, premium_bps: u32, guilt: bool) -> Result<u32, ArithmeticError> {
    let premium_value = u64::from(base)
        .checked_mul(u64::from(premium_bps))
        .ok_or(ArithmeticError)?
        / 10_000;
    let credited = if guilt {
        premium_value
            .checked_mul(u64::from(GUILT_MULTIPLIER_BPS))
            .ok_or(ArithmeticError)?
            / 10_000
    } else {
        premium_value
    };
    u32::try_from(credited).map_err(|_| ArithmeticError)
}

pub fn courtesy_eligible(speed: f32, xz_distance: f32) -> bool {
    speed >= COURTESY_MIN_SPEED
        && xz_distance > CHICKEN_HIT_RADIUS
        && xz_distance <= COURTESY_OUTER_RADIUS
}

/// Per-chicken rearm and global cooldown state. A credited transition should
/// be applied with [`RightOfWay::courtesy`] only when this returns true.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct CourtesyGate {
    latched_chickens: BTreeSet<u32>,
    last_award_ms: Option<u64>,
}

impl CourtesyGate {
    pub fn observe(
        &mut self,
        chicken_stable_id: u32,
        active_ms: u64,
        speed: f32,
        xz_distance: f32,
    ) -> bool {
        if xz_distance > COURTESY_OUTER_RADIUS {
            self.latched_chickens.remove(&chicken_stable_id);
            return false;
        }
        if self.latched_chickens.contains(&chicken_stable_id)
            || !courtesy_eligible(speed, xz_distance)
            || self
                .last_award_ms
                .is_some_and(|last| active_ms < last.saturating_add(COURTESY_COOLDOWN_MS))
        {
            return false;
        }
        self.latched_chickens.insert(chicken_stable_id);
        self.last_award_ms = Some(active_ms);
        true
    }

    pub fn is_latched(&self, chicken_stable_id: u32) -> bool {
        self.latched_chickens.contains(&chicken_stable_id)
    }
}

pub const fn objective_for(conduct: Conduct, round_index: u32) -> (Objective, u32) {
    match (conduct, round_index % 3) {
        (Conduct::CluckHunt, 0) => (Objective::HitChickens, 10),
        (Conduct::CluckHunt, 1) => (Objective::CollectCoins, 6),
        (Conduct::CluckHunt, _) => (Objective::ReachCombo, 3),
        (Conduct::RightOfWay, 0) => (Objective::DeliverPackages, 3),
        (Conduct::RightOfWay, 1) => (Objective::CourtesyAwards, 3),
        (Conduct::RightOfWay, _) => (Objective::CollectCoins, 6),
    }
}

fn lp1(output: &mut Vec<u8>, value: &str) {
    assert!(value.len() <= u8::MAX as usize, "lp1 value too long");
    output.push(value.len() as u8);
    output.extend_from_slice(value.as_bytes());
}

pub fn schedule_bytes(seed: &[u8; 32], category: &str) -> Vec<u8> {
    let schedule = rotation_schedule(seed);
    let mut bytes = Vec::with_capacity(64 + category.len() + SCHEDULE_SEGMENTS * 33);
    lp1(&mut bytes, "roady.v2.schedule");
    bytes.extend_from_slice(&[PROTOCOL_VERSION, RULES_VERSION, POLICY_VERSION]);
    lp1(&mut bytes, MODE);
    lp1(&mut bytes, category);
    bytes.extend_from_slice(seed);
    bytes.extend_from_slice(&(SCHEDULE_SEGMENTS as u16).to_be_bytes());
    for window in schedule {
        bytes.push(window.effect as u8);
        bytes.extend_from_slice(&window.telegraph_start_ms.to_be_bytes());
        bytes.extend_from_slice(&window.active_start_ms.to_be_bytes());
        bytes.extend_from_slice(&window.active_end_ms.to_be_bytes());
        bytes.extend_from_slice(&window.cooldown_end_ms.to_be_bytes());
    }
    bytes
}

pub fn seed_commitment(seed: &[u8; 32]) -> [u8; 32] {
    let mut bytes = Vec::with_capacity(46);
    lp1(&mut bytes, "roady.v2.seed");
    bytes.extend_from_slice(seed);
    Sha256::digest(bytes).into()
}

pub fn schedule_commitment(seed: &[u8; 32], category: &str) -> [u8; 32] {
    Sha256::digest(schedule_bytes(seed, category)).into()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn seed(n: u8) -> [u8; 32] {
        core::array::from_fn(|i| n.wrapping_add(i as u8))
    }
    fn hex(bytes: &[u8]) -> String {
        bytes.iter().map(|b| format!("{b:02x}")).collect()
    }

    #[test]
    fn seed01_stream_anchors_and_draws() {
        let s = seed(1);
        let anchors = [
            (ROTATION_DOMAIN, 0x5995f419c4dc0c2e, 0xd57b9ede6427d32c),
            (
                SCHEDULED_EVENTS_DOMAIN,
                0x6d3eca77d6179b85,
                0xf28bbdc6ed34a573,
            ),
            (
                FRENZY_INTERVAL_DOMAIN,
                0x5db24a622cfa7394,
                0xf0641024cf58a791,
            ),
            (FRENZY_ROLL_DOMAIN, 0xda8b1128a64d8b06, 0xbd2f15367b6e6919),
            (FRENZY_KIND_DOMAIN, 0xe6eba56363494e8b, 0xcebdc4c26cdade04),
            (
                FRENZY_POSITION_DOMAIN,
                0x4b4ad0b4be929e2e,
                0x2f19e691358c35b2,
            ),
            (
                FRENZY_RELOCATION_DOMAIN,
                0x1678c319a195569d,
                0xa180f2b460780b20,
            ),
        ];
        for (domain, fnv, state) in anchors {
            assert_eq!(stream_fnv(&s, domain), fnv);
            assert_eq!(stream_state(&s, domain), state);
        }
        let mut rotation = stream(&s, ROTATION_DOMAIN);
        assert_eq!(
            [
                rotation.next_u64(),
                rotation.next_u64(),
                rotation.next_u64()
            ],
            [0x89ca1998c369bfad, 0x0aeaa2d3a4bd2ca7, 0xf7eb05ac42dd4bab]
        );
        let mut events = stream(&s, SCHEDULED_EVENTS_DOMAIN);
        assert_eq!(
            [events.next_u64(), events.next_u64()],
            [0x208d629eaedd81ba, 0xe8b774f1db6390e8]
        );
    }

    #[test]
    fn seed01_schedule_events_and_frenzy() {
        let s = seed(1);
        let schedule = rotation_schedule(&s);
        let expected = [
            Effect::Stampede,
            Effect::GlassCannon,
            Effect::Stampede,
            Effect::RushHour,
            Effect::GlassCannon,
            Effect::Stampede,
            Effect::RushHour,
            Effect::Stampede,
            Effect::RushHour,
            Effect::Stampede,
            Effect::RushHour,
            Effect::GlassCannon,
            Effect::RushHour,
            Effect::GlassCannon,
            Effect::RushHour,
            Effect::GlassCannon,
        ];
        assert_eq!(schedule.map(|w| w.effect), expected);
        assert_eq!(
            scheduled_events(&s, &schedule),
            [ScheduledEvent::ComboFrenzy, ScheduledEvent::CritterBurst]
        );
        let frenzy = frenzy_opportunities(&s, 60_000);
        assert_eq!(
            frenzy.iter().map(|v| v.at_ms).collect::<Vec<_>>(),
            [11446, 20456, 28802, 40458, 48549, 58060]
        );
        assert_eq!(
            frenzy.iter().map(|v| v.roll_residue).collect::<Vec<_>>(),
            [7564, 5850, 4625, 6756, 1830, 7341]
        );
        assert!(frenzy.last().unwrap().spawn && frenzy.last().unwrap().pity);
    }

    #[test]
    fn twenty_commitment_vectors() {
        let cluck = [
            "f287d212e7f0170ade4324886d33ea31f1ce45468e759e2bd4798bc9c6979ec1",
            "334e4c0f526c8141668fe47efc4f6f33084b1200acd437c5371bb92a6ff255c7",
            "949883a46b9af2b53f1baa4af4b43a3ba0fed43deb941fc046cb507576d781a7",
            "e91a3d1d49dc5ec2bf4fa2149e3415c74f525455b838aa805f3d9ceecd97c2c3",
            "65daafacc6cb058130efc7a7cb15ea6d2d6484199bd2e02a034626292d12af92",
            "bf950fa753b6451fa566b73eb32f36e7a256c572eea96c6a1aa8a1a32d4b4200",
            "460c8485332aae4908ea528652c873585f9f99558d37919a18b218a34e189924",
            "833ec49d6c2aada73be6f872c02330833dea01aeb488f21c5c096ff532ed67fe",
            "61c0f30a16a7c4d03b856b3a396ad64b33f61e72fca35eeffa74066726a33749",
            "45c7afc4e84f2733859099fc1803b91478e5b43a8a4ba7d827396b912b729e36",
            "c2a7d64ab7802bfa5eb3fd9feee5b347c9b77f585523a019413a2f1bb0d0700f",
            "fa8cae6c442c6ab16f1436b067628a660849f291548598ef306636ad25761225",
            "d418a5cac3e35063a237c22152b254787dfb17f4d74f2b717faa79aa24292e83",
            "ab0f87d6bd1aa0aa40becbc939999318b096919c40c06ed499737a3179a0c437",
            "9a7e90fd49090f41ad9bb1af30f947f41c7f994cd930dc83b6ccf36cb8b75ce1",
            "48a75a340a46570e3bb6bfea3aa6e6739567da78beea186d537e461f20c6cfdb",
            "44abc664e087b818fab7c3c52b9a5b9ec314fe965cf443527db883c4f670e4e9",
            "1ff8355a97c85e24d2e7c120134ba80dd8dc00c4911bce969cd049e962422d00",
            "1c661830b47f49760236f9ead230196d9dacb8c9b5af95e14ef3eef22ddfb16c",
            "3db7f479d56ddb5790c32765ca2c37adf0dccd1a6ba92fc3740ac722edc16228",
        ];
        let row = [
            "bb785fb44d72ad7ea1b957df9bcc95dffdd814a475e736a0e74beceee2d3049e",
            "4fa369f6bd2b5a2ddea60726a72a700559ae194cc8c5b19d61a15fe98a3defac",
            "3e611354756e820ee350fcdbac36412af08093c29cb6df261bef7daf3415d96a",
            "511707e40e877d8f2a4377ff0c3c7b1dae2e93d8c62313927e62f8006a273eee",
            "d5f192ef7a096d115a25c6fc39b00cf333dc637669b2953d17d99d874ae57b32",
            "4368c564e05348e06b53b8a044576fee3d7f4470b446fc45daa6dc4e3bf96a5c",
            "894f025408ad909581527fe27ddcbd57113e87e7ac9fbc74658840649d23f2f4",
            "dd0eb4a80b78f32fb7b948eb99f95b207a01e2db2482466e63ef92faf9dc7d99",
            "deb5d0a5f2d62d7b22a31f2319eead87c0c013c96b94a3233c18c568ba68c37a",
            "88f15f057071a469364c427fce1931954f55d5236c222c7c3098d2bc9589bcb5",
            "9ef2cdd58be7135073053fea3aa101b8b058ddb996fe9bcda20ca79edd688e31",
            "1d1cf93b3eb3168c41949ce65dd2af1ff3856db6d9c92ca845a3b98f9f5cd7c4",
            "2838cd512820b3f74124b35b9a247fd71a739faa19ceae1f9a58d5b2779e30f6",
            "ecaa99475910a42288628d6ff4b49eb6aedd22214fa73722428fe0e07d269e99",
            "bcbb71e41a8b97b9a178bb011c82f41b840754cd504a172cd2808e82d50d039a",
            "13727477ab58fe5d873de4be11366edc480ba823595cad8de60256078a673b00",
            "a5065400a24a96f354cdaeb067c6b17709a88e27d6d4a32d65f8fbac3c470e13",
            "9e54baef1562faa6d4ff68aa3ab9abc086faada8d224b537b6fe60e332a17ddf",
            "ac9548471d60f65d6ffc7521ca321a15c88f004a9d5579330522085149fec8c6",
            "abc51ca29d1dff0de62a6833101676833342353b169e0c15a986135d75cb38c8",
        ];
        let seed_hash = [
            "1f79a204b991758a8798f650465fc89634f967a3976312a2eaaff5912bbd8b48",
            "5e3366d30193ffc84bc561598cce68ff566448eff975c30f7950a14d9fc4093c",
            "9ee7af0b7c2ad1e12a27a376a4a176113385bbcb71ffa1a7c4c0cbe138c5babf",
            "f6e037508be4596534bc0a6687f88616cdab314834379309341e6432b6263298",
            "787bfeebf610c9e713c8e0b7138b4a0820cbc51748b89d7cd620f2eb8053360e",
            "a1a593496dce3d7c003a799e71defb6c60c3172d1a85781cf71187b715777fa4",
            "34a9bdfeaa3a8d478c3b97ee5840ec3f7993f35765f8feadefc4ee732b2dd43a",
            "8129054312f2fae543ce070e550e4b173e4ef85c77e6686c0a82ff9fe3b0426b",
            "2f9a7a704f2aa8676693f846837c94c571336c02ac22bd1646b1109864273d8e",
            "83d8883ac422c10822fad3fb0075ffba947ec9a4f69c9307359ab95c6357f4ea",
            "32ae12de16439abf296be1fdda0a4772bd799ee956bc97a5830364d966c40507",
            "542773e1dcdec7f27f7e4640faa0952e66cc7d16e2575b16bb71134af7f8652c",
            "687776b97fb817f5acf493072aa42ebb4cce1f8503e0c04692c9002c874eee2e",
            "e74ebafaf7b5b8858c6d083501d3231d52ab64dbeffc08bde9bfe9b2a79cbba7",
            "2e32647c9cf1421344e2ed837d99291d26ea98b7d880b7dbea9844bc66b9c58c",
            "22782874120611f968dc4df9a302534b092e5d85f97189863d6abe44f57fb2db",
            "ba4a559bb957375c603af17cafb662ef475cd5efcd16cc61ebd10b4fb617d663",
            "d713294ed3a7e5c2860d4c1880871222d4e7fc863866209c0a7e51ce0a7502fe",
            "9ba2de0f69174ed1c39d11abc5f7743df9fb2949c27d838af9190b48a175908f",
            "12f7e80a5403318c8cacd471cec2e8347359b3733d316ec264a17716f44b6b56",
        ];
        for i in 0..20 {
            let s = seed((i + 1) as u8);
            assert_eq!(hex(&schedule_commitment(&s, CLUCK_HUNT_CATEGORY)), cluck[i]);
            assert_eq!(hex(&schedule_commitment(&s, RIGHT_OF_WAY_CATEGORY)), row[i]);
            assert_eq!(hex(&seed_commitment(&s)), seed_hash[i]);
        }
    }

    #[test]
    fn scoring_boundaries_and_checked_row() {
        assert_eq!(cluck_terminal(u32::MAX, 1), Err(ArithmeticError));
        assert_eq!(coin_clock(90_001), 90_000);
        assert!(courtesy_eligible(4.0, 2.12));
        assert!(!courtesy_eligible(4.0, 1.0));
        let mut row = RightOfWay::with_remaining(1_000);
        for _ in 0..3 {
            assert!(row.pickup_package());
        }
        assert!(!row.pickup_package());
        assert_eq!(row.deliver_package().unwrap().unwrap().credited, 5);
        assert_eq!(row.deliver_package().unwrap().unwrap().credited, 6);
        assert_eq!(row.deliver_package().unwrap().unwrap().credited, 7);
        assert_eq!(row.remaining_ms, 10_000);
        row.animal_hit().unwrap();
        assert_eq!(row.premium_bps, 9_000);
        assert!(row.pickup_package());
        assert_eq!(row.deliver_package().unwrap().unwrap().credited, 2);
    }
}
