//! Canonical big-endian bytes and SHA-256 chaining for protocol v2.
//!
//! Event records contain the event domain exactly once and never contain the
//! previous hash. Stored ledger entries are `event_record || event_hash`, where
//! `event_hash = SHA-256(previous_hash || event_record)`.

use super::{
    Conduct, EventKind, FrenzyPhase, MODE, Objective, POLICY_VERSION, PROTOCOL_VERSION, Platform,
    RULES_VERSION, RotationWindow, SCHEDULE_SEGMENTS, TerminalReason, rotation_schedule,
};
use sha2::{Digest, Sha256};

pub const MAX_EVENTS: u32 = 4_096;
pub const MAX_LEDGER_BYTES: usize = 262_144;
pub const MAX_EVENT_RECORD_BYTES: usize = 192;
pub const MAX_EVIDENCE_BYTES: usize = 524_288;
pub const MAX_LP4_BYTES: usize = 524_288;
pub const MAX_BUILD_BYTES: usize = 64;

/// Frozen cross-language RightOfWay score-HMAC fixture from contract §11.7.
pub const RIGHT_OF_WAY_SCORE_HMAC_GOLDEN_LEN: usize = 208;
pub const RIGHT_OF_WAY_SCORE_HMAC_GOLDEN_HEX: &str = "0e726f6164792e76322e73636f726502020108726f746174696f6e18726f746174696f6e2e76312e72696768745f6f665f776179035330311111111111111111111111111111111111111111111111111111111111111111bb785fb44d72ad7ea1b957df9bcc95dffdd814a475e736a0e74beceee2d3049e1f79a204b991758a8798f650465fc89634f967a3976312a2eaaff5912bbd8b480101000000110000000000000011000023280000000300000002000000010000000301000000000000ea6000000000000013880364657601";
pub const RIGHT_OF_WAY_SCORE_HMAC_GOLDEN_BASE64URL: &str =
    "_FkmJU_oSw6ycX5CsaTlcu0V4dYKPTy4zhjgIjA8Gkw";

const SESSION_DOMAIN: &str = "roady.v2.session";
const SCORE_DOMAIN: &str = "roady.v2.score";
const EVENT_DOMAIN: &str = "roady.v2.event";
const ROOT_DOMAIN: &str = "roady.v2.root";
const SCHEDULE_DOMAIN: &str = "roady.v2.schedule";
const PROOF_DOMAIN: &str = "roady.v2.proof";
const EVIDENCE_DOMAIN: &str = "roady.v2.evidence";

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CanonicalError {
    EmptyLp1,
    Lp1TooLong { len: usize },
    Lp4TooLong { len: usize },
    BuildTooLong { len: usize },
    EventRecordTooLong { len: usize },
    TooManyEvents,
    LedgerTooLong { len: usize },
    EvidenceTooLong { len: usize },
    InvalidSequence { expected: u32, actual: u32 },
    EventAfterTerminal,
    MissingTerminal,
}

impl core::fmt::Display for CanonicalError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::EmptyLp1 => f.write_str("lp1 value must not be empty"),
            Self::Lp1TooLong { len } => write!(f, "lp1 value is {len} bytes; maximum is 255"),
            Self::Lp4TooLong { len } => {
                write!(f, "lp4 value is {len} bytes; maximum is {MAX_LP4_BYTES}")
            }
            Self::BuildTooLong { len } => {
                write!(f, "build is {len} bytes; maximum is {MAX_BUILD_BYTES}")
            }
            Self::EventRecordTooLong { len } => write!(
                f,
                "event record is {len} bytes; maximum is {MAX_EVENT_RECORD_BYTES}"
            ),
            Self::TooManyEvents => write!(f, "event count exceeds {MAX_EVENTS}"),
            Self::LedgerTooLong { len } => write!(
                f,
                "canonical ledger is {len} bytes; maximum is {MAX_LEDGER_BYTES}"
            ),
            Self::EvidenceTooLong { len } => write!(
                f,
                "evidence envelope is {len} bytes; maximum is {MAX_EVIDENCE_BYTES}"
            ),
            Self::InvalidSequence { expected, actual } => {
                write!(f, "event sequence is {actual}; expected {expected}")
            }
            Self::EventAfterTerminal => f.write_str("no event may follow Terminal"),
            Self::MissingTerminal => f.write_str("ledger has no Terminal event"),
        }
    }
}

impl std::error::Error for CanonicalError {}

/// Minimal canonical writer. All integer methods use big-endian order.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct CanonicalWriter {
    bytes: Vec<u8>,
}

impl CanonicalWriter {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn with_capacity(capacity: usize) -> Self {
        Self {
            bytes: Vec::with_capacity(capacity),
        }
    }

    pub fn u8(&mut self, value: u8) {
        self.bytes.push(value);
    }

    pub fn u16(&mut self, value: u16) {
        self.bytes.extend_from_slice(&value.to_be_bytes());
    }

    pub fn u32(&mut self, value: u32) {
        self.bytes.extend_from_slice(&value.to_be_bytes());
    }

    pub fn i32(&mut self, value: i32) {
        self.bytes.extend_from_slice(&value.to_be_bytes());
    }

    pub fn u64(&mut self, value: u64) {
        self.bytes.extend_from_slice(&value.to_be_bytes());
    }

    pub fn i64(&mut self, value: i64) {
        self.bytes.extend_from_slice(&value.to_be_bytes());
    }

    pub fn raw(&mut self, value: &[u8]) {
        self.bytes.extend_from_slice(value);
    }

    pub fn raw32(&mut self, value: &[u8; 32]) {
        self.raw(value);
    }

    pub fn lp1(&mut self, value: &str) -> Result<(), CanonicalError> {
        let value = value.as_bytes();
        if value.is_empty() {
            return Err(CanonicalError::EmptyLp1);
        }
        if value.len() > u8::MAX as usize {
            return Err(CanonicalError::Lp1TooLong { len: value.len() });
        }
        self.u8(value.len() as u8);
        self.raw(value);
        Ok(())
    }

    pub fn lp4(&mut self, value: &[u8]) -> Result<(), CanonicalError> {
        if value.len() > MAX_LP4_BYTES {
            return Err(CanonicalError::Lp4TooLong { len: value.len() });
        }
        self.u32(value.len() as u32);
        self.raw(value);
        Ok(())
    }

    pub fn as_slice(&self) -> &[u8] {
        &self.bytes
    }

    pub fn into_bytes(self) -> Vec<u8> {
        self.bytes
    }
}

pub fn lp1(value: &str) -> Result<Vec<u8>, CanonicalError> {
    let mut writer = CanonicalWriter::with_capacity(value.len() + 1);
    writer.lp1(value)?;
    Ok(writer.into_bytes())
}

pub fn lp4(value: &[u8]) -> Result<Vec<u8>, CanonicalError> {
    let mut writer = CanonicalWriter::with_capacity(value.len() + 4);
    writer.lp4(value)?;
    Ok(writer.into_bytes())
}

#[derive(Clone, Copy, Debug)]
pub struct SessionHeader<'a> {
    pub category: &'a str,
    pub session_id: &'a str,
    pub challenge: &'a str,
    pub seed_commitment: &'a [u8; 32],
    pub schedule_hash: &'a [u8; 32],
    pub issued_at_ms: u64,
}

fn session_header_prefix(input: &SessionHeader<'_>) -> Result<CanonicalWriter, CanonicalError> {
    let mut writer = CanonicalWriter::with_capacity(160);
    writer.lp1(SESSION_DOMAIN)?;
    writer.u8(PROTOCOL_VERSION);
    writer.u8(RULES_VERSION);
    writer.u8(POLICY_VERSION);
    writer.lp1(MODE)?;
    writer.lp1(input.category)?;
    writer.lp1(input.session_id)?;
    writer.lp1(input.challenge)?;
    writer.raw32(input.seed_commitment);
    writer.raw32(input.schedule_hash);
    writer.u64(input.issued_at_ms);
    Ok(writer)
}

pub fn unstarted_session_header(
    input: &SessionHeader<'_>,
    start_by_expiry_ms: u64,
) -> Result<Vec<u8>, CanonicalError> {
    let mut writer = session_header_prefix(input)?;
    writer.u64(start_by_expiry_ms);
    writer.u8(0);
    writer.u64(0);
    Ok(writer.into_bytes())
}

pub fn started_session_header(
    input: &SessionHeader<'_>,
    started_at_ms: u64,
) -> Result<Vec<u8>, CanonicalError> {
    let mut writer = session_header_prefix(input)?;
    writer.u64(0);
    writer.u8(1);
    writer.u64(started_at_ms);
    Ok(writer.into_bytes())
}

/// Bytes authenticated by the Worker proof HMAC.
pub fn worker_proof_input(session_header: &[u8]) -> Vec<u8> {
    let mut writer = CanonicalWriter::with_capacity(PROOF_DOMAIN.len() + 1 + session_header.len());
    // Frozen nonempty domain is infallible.
    writer.lp1(PROOF_DOMAIN).expect("proof domain fits lp1");
    writer.raw(session_header);
    writer.into_bytes()
}

pub fn schedule_bytes(seed: &[u8; 32], category: &str) -> Result<Vec<u8>, CanonicalError> {
    schedule_bytes_for_windows(seed, category, &rotation_schedule(seed))
}

pub fn schedule_bytes_for_windows(
    seed: &[u8; 32],
    category: &str,
    schedule: &[RotationWindow; SCHEDULE_SEGMENTS],
) -> Result<Vec<u8>, CanonicalError> {
    let mut writer = CanonicalWriter::with_capacity(600 + category.len());
    writer.lp1(SCHEDULE_DOMAIN)?;
    writer.u8(PROTOCOL_VERSION);
    writer.u8(RULES_VERSION);
    writer.u8(POLICY_VERSION);
    writer.lp1(MODE)?;
    writer.lp1(category)?;
    writer.raw32(seed);
    writer.u16(SCHEDULE_SEGMENTS as u16);
    for window in schedule {
        writer.u8(window.effect as u8);
        writer.u64(window.telegraph_start_ms);
        writer.u64(window.active_start_ms);
        writer.u64(window.active_end_ms);
        writer.u64(window.cooldown_end_ms);
    }
    Ok(writer.into_bytes())
}

pub fn sha256(bytes: &[u8]) -> [u8; 32] {
    Sha256::digest(bytes).into()
}

pub fn schedule_hash(seed: &[u8; 32], category: &str) -> Result<[u8; 32], CanonicalError> {
    Ok(sha256(&schedule_bytes(seed, category)?))
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CluckTerminal {
    pub reason: TerminalReason,
    pub total: u32,
    pub chickens: u32,
    pub coins: u32,
    pub objective_completed: bool,
    pub max_combo: u8,
    pub duration_ms: u64,
    pub remaining_ms: u64,
    pub build: String,
    pub platform: Platform,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RightOfWayTerminal {
    pub reason: TerminalReason,
    pub total: u32,
    pub accumulator: i64,
    pub premium_bps: u32,
    pub packages_delivered: u32,
    pub courtesy_count: u32,
    pub animal_hits: u32,
    pub max_delivery_chain: u32,
    pub objective_completed: bool,
    pub duration_ms: u64,
    pub remaining_ms: u64,
    pub build: String,
    pub platform: Platform,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ConductTerminal {
    CluckHunt(CluckTerminal),
    RightOfWay(RightOfWayTerminal),
}

impl ConductTerminal {
    pub const fn conduct(&self) -> Conduct {
        match self {
            Self::CluckHunt(_) => Conduct::CluckHunt,
            Self::RightOfWay(_) => Conduct::RightOfWay,
        }
    }

    pub fn canonical_bytes(&self) -> Result<Vec<u8>, CanonicalError> {
        let mut writer = CanonicalWriter::with_capacity(96);
        match self {
            Self::CluckHunt(value) => {
                validate_build(&value.build)?;
                writer.u8(Conduct::CluckHunt as u8);
                writer.u8(value.reason as u8);
                writer.u32(value.total);
                writer.u32(value.chickens);
                writer.u32(value.coins);
                writer.u8(value.objective_completed as u8);
                writer.u8(value.max_combo);
                writer.u64(value.duration_ms);
                writer.u64(value.remaining_ms);
                writer.lp1(&value.build)?;
                writer.u8(value.platform as u8);
            }
            Self::RightOfWay(value) => {
                validate_build(&value.build)?;
                writer.u8(Conduct::RightOfWay as u8);
                writer.u8(value.reason as u8);
                writer.u32(value.total);
                writer.i64(value.accumulator);
                writer.u32(value.premium_bps);
                writer.u32(value.packages_delivered);
                writer.u32(value.courtesy_count);
                writer.u32(value.animal_hits);
                writer.u32(value.max_delivery_chain);
                writer.u8(value.objective_completed as u8);
                writer.u64(value.duration_ms);
                writer.u64(value.remaining_ms);
                writer.lp1(&value.build)?;
                writer.u8(value.platform as u8);
            }
        }
        Ok(writer.into_bytes())
    }
}

fn validate_build(build: &str) -> Result<(), CanonicalError> {
    if build.len() > MAX_BUILD_BYTES {
        Err(CanonicalError::BuildTooLong { len: build.len() })
    } else {
        Ok(())
    }
}

/// One protocol event payload. `PackageDelivery` always represents exactly one
/// package; a three-package drop-off must append three consecutive events.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum EventPayload {
    ChickenHit {
        base: u32,
        event_bonus: u32,
        frenzy_bonus: u32,
        combo_before: u8,
        combo_after: u8,
        bucket_before: u32,
        bucket_after: u32,
    },
    CoinCollected {
        mega: bool,
        base: u32,
        combo_before: u8,
        combo_after: u8,
        bucket_before: u32,
        bucket_after: u32,
        remaining_before_ms: u64,
        remaining_after_ms: u64,
    },
    TimePickup {
        remaining_before_ms: u64,
        remaining_after_ms: u64,
    },
    ObjectiveCompletedCluck {
        objective: Objective,
        target: u32,
        base_reward: u32,
        bucket_before: u32,
        bucket_after: u32,
    },
    CritterPenalty {
        penalty: u32,
        bucket_before: u32,
        bucket_after: u32,
        cooldown_after_ms: u64,
    },
    SegmentChanged {
        segment_kind: u8,
        effect_or_event: u8,
        active: bool,
        start_ms: u64,
        end_ms: u64,
    },
    Terminal(ConductTerminal),
    PackagePickup {
        carried_before: u8,
        carried_after: u8,
    },
    PackageDelivery {
        delivered_ordinal_within_dropoff: u8,
        chain_index: u32,
        base: u32,
        premium_bps: u32,
        guilt: bool,
        credited: u32,
        accumulator_before: i64,
        accumulator_after: i64,
        remaining_before_ms: u64,
        remaining_after_ms: u64,
    },
    CourtesyAward {
        chicken_stable_id: u32,
        premium_bps: u32,
        guilt: bool,
        credited: u32,
        accumulator_before: i64,
        accumulator_after: i64,
        cooldown_after_ms: u32,
    },
    AnimalHit {
        animal_kind: u8,
        delta: i32,
        premium_before_bps: u32,
        premium_after_bps: u32,
        guilt_after_ms: u64,
        accumulator_before: i64,
        accumulator_after: i64,
    },
    WaveAward {
        base: u32,
        premium_bps: u32,
        guilt: bool,
        credited: u32,
        accumulator_before: i64,
        accumulator_after: i64,
    },
    CoinAward {
        base: u32,
        premium_bps: u32,
        guilt: bool,
        credited: u32,
        accumulator_before: i64,
        accumulator_after: i64,
        remaining_before_ms: u64,
        remaining_after_ms: u64,
    },
    FrenzyChanged {
        phase: FrenzyPhase,
        start_ms: u64,
        end_ms: u64,
    },
    ObjectiveCompletedRightOfWay {
        objective: Objective,
        target: u32,
        base: u32,
        premium_bps: u32,
        guilt: bool,
        credited: u32,
        accumulator_before: i64,
        accumulator_after: i64,
    },
}

impl EventPayload {
    pub const fn kind(&self) -> EventKind {
        match self {
            Self::ChickenHit { .. } => EventKind::ChickenHit,
            Self::CoinCollected { .. } => EventKind::CoinCollected,
            Self::TimePickup { .. } => EventKind::TimePickup,
            Self::ObjectiveCompletedCluck { .. } | Self::ObjectiveCompletedRightOfWay { .. } => {
                EventKind::ObjectiveCompleted
            }
            Self::CritterPenalty { .. } => EventKind::CritterPenalty,
            Self::SegmentChanged { .. } => EventKind::SegmentChanged,
            Self::Terminal(_) => EventKind::Terminal,
            Self::PackagePickup { .. } => EventKind::PackagePickup,
            Self::PackageDelivery { .. } => EventKind::PackageDelivery,
            Self::CourtesyAward { .. } => EventKind::CourtesyAward,
            Self::AnimalHit { .. } => EventKind::AnimalHit,
            Self::WaveAward { .. } => EventKind::WaveAward,
            Self::CoinAward { .. } => EventKind::CoinAward,
            Self::FrenzyChanged { .. } => EventKind::FrenzyChanged,
        }
    }

    fn write(&self, writer: &mut CanonicalWriter) -> Result<(), CanonicalError> {
        match self {
            Self::ChickenHit {
                base,
                event_bonus,
                frenzy_bonus,
                combo_before,
                combo_after,
                bucket_before,
                bucket_after,
            } => {
                writer.u32(*base);
                writer.u32(*event_bonus);
                writer.u32(*frenzy_bonus);
                writer.u8(*combo_before);
                writer.u8(*combo_after);
                writer.u32(*bucket_before);
                writer.u32(*bucket_after);
            }
            Self::CoinCollected {
                mega,
                base,
                combo_before,
                combo_after,
                bucket_before,
                bucket_after,
                remaining_before_ms,
                remaining_after_ms,
            } => {
                writer.u8(*mega as u8);
                writer.u32(*base);
                writer.u8(*combo_before);
                writer.u8(*combo_after);
                writer.u32(*bucket_before);
                writer.u32(*bucket_after);
                writer.u64(*remaining_before_ms);
                writer.u64(*remaining_after_ms);
            }
            Self::TimePickup {
                remaining_before_ms,
                remaining_after_ms,
            } => {
                writer.u64(*remaining_before_ms);
                writer.u64(*remaining_after_ms);
            }
            Self::ObjectiveCompletedCluck {
                objective,
                target,
                base_reward,
                bucket_before,
                bucket_after,
            } => {
                writer.u8(*objective as u8);
                writer.u32(*target);
                writer.u32(*base_reward);
                writer.u32(*bucket_before);
                writer.u32(*bucket_after);
            }
            Self::CritterPenalty {
                penalty,
                bucket_before,
                bucket_after,
                cooldown_after_ms,
            } => {
                writer.u32(*penalty);
                writer.u32(*bucket_before);
                writer.u32(*bucket_after);
                writer.u64(*cooldown_after_ms);
            }
            Self::SegmentChanged {
                segment_kind,
                effect_or_event,
                active,
                start_ms,
                end_ms,
            } => {
                writer.u8(*segment_kind);
                writer.u8(*effect_or_event);
                writer.u8(*active as u8);
                writer.u64(*start_ms);
                writer.u64(*end_ms);
            }
            Self::Terminal(terminal) => writer.raw(&terminal.canonical_bytes()?),
            Self::PackagePickup {
                carried_before,
                carried_after,
            } => {
                writer.u8(*carried_before);
                writer.u8(*carried_after);
            }
            Self::PackageDelivery {
                delivered_ordinal_within_dropoff,
                chain_index,
                base,
                premium_bps,
                guilt,
                credited,
                accumulator_before,
                accumulator_after,
                remaining_before_ms,
                remaining_after_ms,
            } => {
                writer.u8(*delivered_ordinal_within_dropoff);
                writer.u32(*chain_index);
                writer.u32(*base);
                writer.u32(*premium_bps);
                writer.u8(*guilt as u8);
                writer.u32(*credited);
                writer.i64(*accumulator_before);
                writer.i64(*accumulator_after);
                writer.u64(*remaining_before_ms);
                writer.u64(*remaining_after_ms);
            }
            Self::CourtesyAward {
                chicken_stable_id,
                premium_bps,
                guilt,
                credited,
                accumulator_before,
                accumulator_after,
                cooldown_after_ms,
            } => {
                writer.u32(*chicken_stable_id);
                writer.u32(*premium_bps);
                writer.u8(*guilt as u8);
                writer.u32(*credited);
                writer.i64(*accumulator_before);
                writer.i64(*accumulator_after);
                writer.u32(*cooldown_after_ms);
            }
            Self::AnimalHit {
                animal_kind,
                delta,
                premium_before_bps,
                premium_after_bps,
                guilt_after_ms,
                accumulator_before,
                accumulator_after,
            } => {
                writer.u8(*animal_kind);
                writer.i32(*delta);
                writer.u32(*premium_before_bps);
                writer.u32(*premium_after_bps);
                writer.u64(*guilt_after_ms);
                writer.i64(*accumulator_before);
                writer.i64(*accumulator_after);
            }
            Self::WaveAward {
                base,
                premium_bps,
                guilt,
                credited,
                accumulator_before,
                accumulator_after,
            } => {
                writer.u32(*base);
                writer.u32(*premium_bps);
                writer.u8(*guilt as u8);
                writer.u32(*credited);
                writer.i64(*accumulator_before);
                writer.i64(*accumulator_after);
            }
            Self::CoinAward {
                base,
                premium_bps,
                guilt,
                credited,
                accumulator_before,
                accumulator_after,
                remaining_before_ms,
                remaining_after_ms,
            } => {
                writer.u32(*base);
                writer.u32(*premium_bps);
                writer.u8(*guilt as u8);
                writer.u32(*credited);
                writer.i64(*accumulator_before);
                writer.i64(*accumulator_after);
                writer.u64(*remaining_before_ms);
                writer.u64(*remaining_after_ms);
            }
            Self::FrenzyChanged {
                phase,
                start_ms,
                end_ms,
            } => {
                writer.u8(*phase as u8);
                writer.u64(*start_ms);
                writer.u64(*end_ms);
            }
            Self::ObjectiveCompletedRightOfWay {
                objective,
                target,
                base,
                premium_bps,
                guilt,
                credited,
                accumulator_before,
                accumulator_after,
            } => {
                writer.u8(*objective as u8);
                writer.u32(*target);
                writer.u32(*base);
                writer.u32(*premium_bps);
                writer.u8(*guilt as u8);
                writer.u32(*credited);
                writer.i64(*accumulator_before);
                writer.i64(*accumulator_after);
            }
        }
        Ok(())
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Event {
    pub seq: u32,
    pub active_ms: u64,
    pub payload: EventPayload,
}

pub fn event_record(event: &Event) -> Result<Vec<u8>, CanonicalError> {
    let mut writer = CanonicalWriter::with_capacity(96);
    writer.lp1(EVENT_DOMAIN)?;
    writer.u32(event.seq);
    writer.u64(event.active_ms);
    writer.u8(event.payload.kind() as u8);
    event.payload.write(&mut writer)?;
    let bytes = writer.into_bytes();
    if bytes.len() > MAX_EVENT_RECORD_BYTES {
        return Err(CanonicalError::EventRecordTooLong { len: bytes.len() });
    }
    Ok(bytes)
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StoredEvent {
    pub record: Vec<u8>,
    pub event_hash: [u8; 32],
}

impl StoredEvent {
    pub fn canonical_bytes(&self) -> Vec<u8> {
        let mut bytes = Vec::with_capacity(self.record.len() + 32);
        bytes.extend_from_slice(&self.record);
        bytes.extend_from_slice(&self.event_hash);
        bytes
    }
}

pub fn chain_event(previous_hash: &[u8; 32], event: &Event) -> Result<StoredEvent, CanonicalError> {
    let record = event_record(event)?;
    let mut hasher = Sha256::new();
    hasher.update(previous_hash);
    hasher.update(&record);
    let event_hash = hasher.finalize().into();
    Ok(StoredEvent { record, event_hash })
}

/// Bounded, append-only canonical ledger. Construction hashes the exact started
/// session header to obtain h0.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CanonicalLedger {
    h0: [u8; 32],
    last_hash: [u8; 32],
    event_count: u32,
    stored_bytes: Vec<u8>,
    terminal: Option<ConductTerminal>,
}

impl CanonicalLedger {
    pub fn new(started_session_header: &[u8]) -> Self {
        let h0 = sha256(started_session_header);
        Self {
            h0,
            last_hash: h0,
            event_count: 0,
            stored_bytes: Vec::new(),
            terminal: None,
        }
    }

    pub fn append(&mut self, event: &Event) -> Result<[u8; 32], CanonicalError> {
        if self.terminal.is_some() {
            return Err(CanonicalError::EventAfterTerminal);
        }
        if self.event_count >= MAX_EVENTS {
            return Err(CanonicalError::TooManyEvents);
        }
        if event.seq != self.event_count {
            return Err(CanonicalError::InvalidSequence {
                expected: self.event_count,
                actual: event.seq,
            });
        }
        let stored = chain_event(&self.last_hash, event)?;
        let stored_len = stored.record.len() + stored.event_hash.len();
        let new_len = self
            .stored_bytes
            .len()
            .checked_add(stored_len)
            .ok_or(CanonicalError::LedgerTooLong { len: usize::MAX })?;
        if new_len > MAX_LEDGER_BYTES {
            return Err(CanonicalError::LedgerTooLong { len: new_len });
        }

        self.stored_bytes.extend_from_slice(&stored.record);
        self.stored_bytes.extend_from_slice(&stored.event_hash);
        self.last_hash = stored.event_hash;
        self.event_count += 1;
        if let EventPayload::Terminal(terminal) = &event.payload {
            self.terminal = Some(terminal.clone());
        }
        Ok(self.last_hash)
    }

    pub const fn h0(&self) -> &[u8; 32] {
        &self.h0
    }

    pub const fn last_hash(&self) -> &[u8; 32] {
        &self.last_hash
    }

    pub const fn event_count(&self) -> u32 {
        self.event_count
    }

    pub fn stored_bytes(&self) -> &[u8] {
        &self.stored_bytes
    }

    pub fn terminal(&self) -> Result<&ConductTerminal, CanonicalError> {
        self.terminal
            .as_ref()
            .ok_or(CanonicalError::MissingTerminal)
    }

    pub fn evidence_bytes(&self, session_id: &str) -> Result<Vec<u8>, CanonicalError> {
        self.terminal()?;
        evidence_bytes(session_id, self.event_count, &self.stored_bytes)
    }

    pub fn evidence_hash(&self, session_id: &str) -> Result<[u8; 32], CanonicalError> {
        Ok(sha256(&self.evidence_bytes(session_id)?))
    }

    pub fn final_root(&self) -> Result<[u8; 32], CanonicalError> {
        final_root(&self.h0, &self.last_hash, self.terminal()?)
    }
}

pub fn evidence_bytes(
    session_id: &str,
    event_count: u32,
    stored_ledger: &[u8],
) -> Result<Vec<u8>, CanonicalError> {
    if event_count > MAX_EVENTS {
        return Err(CanonicalError::TooManyEvents);
    }
    if stored_ledger.len() > MAX_LEDGER_BYTES {
        return Err(CanonicalError::LedgerTooLong {
            len: stored_ledger.len(),
        });
    }
    let mut writer = CanonicalWriter::with_capacity(stored_ledger.len() + session_id.len() + 32);
    writer.lp1(EVIDENCE_DOMAIN)?;
    writer.lp1(session_id)?;
    writer.u32(event_count);
    writer.lp4(stored_ledger)?;
    let bytes = writer.into_bytes();
    if bytes.len() > MAX_EVIDENCE_BYTES {
        return Err(CanonicalError::EvidenceTooLong { len: bytes.len() });
    }
    Ok(bytes)
}

pub fn evidence_hash(evidence_bytes: &[u8]) -> [u8; 32] {
    sha256(evidence_bytes)
}

/// Canonical 64-character lowercase JSON representation of an evidence hash.
pub fn evidence_hash_hex(evidence_bytes: &[u8]) -> String {
    evidence_hash(evidence_bytes)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

pub fn final_root(
    h0: &[u8; 32],
    h_n: &[u8; 32],
    terminal: &ConductTerminal,
) -> Result<[u8; 32], CanonicalError> {
    let aggregates = terminal.canonical_bytes()?;
    let mut writer = CanonicalWriter::with_capacity(1 + ROOT_DOMAIN.len() + 64 + aggregates.len());
    writer.lp1(ROOT_DOMAIN)?;
    writer.raw32(h0);
    writer.raw32(h_n);
    writer.raw(&aggregates);
    Ok(sha256(writer.as_slice()))
}

/// Exact bytes authenticated by the v2 client score HMAC.
pub fn score_hmac_input(
    category: &str,
    session_id: &str,
    final_root: &[u8; 32],
    schedule_hash: &[u8; 32],
    seed_commitment: &[u8; 32],
    terminal: &ConductTerminal,
) -> Result<Vec<u8>, CanonicalError> {
    let aggregates = terminal.canonical_bytes()?;
    let mut writer = CanonicalWriter::with_capacity(180 + category.len() + session_id.len());
    writer.lp1(SCORE_DOMAIN)?;
    writer.u8(PROTOCOL_VERSION);
    writer.u8(RULES_VERSION);
    writer.u8(POLICY_VERSION);
    writer.lp1(MODE)?;
    writer.lp1(category)?;
    writer.lp1(session_id)?;
    writer.raw32(final_root);
    writer.raw32(schedule_hash);
    writer.raw32(seed_commitment);
    writer.raw(&aggregates);
    Ok(writer.into_bytes())
}

#[cfg(test)]
mod tests {
    use super::*;
    use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
    use hmac::{Hmac, Mac};

    fn hex(bytes: &[u8]) -> String {
        bytes.iter().map(|byte| format!("{byte:02x}")).collect()
    }

    fn right_of_way_terminal() -> ConductTerminal {
        ConductTerminal::RightOfWay(RightOfWayTerminal {
            reason: TerminalReason::TimeUp,
            total: 17,
            accumulator: 17,
            premium_bps: 9_000,
            packages_delivered: 3,
            courtesy_count: 2,
            animal_hits: 1,
            max_delivery_chain: 3,
            objective_completed: true,
            duration_ms: 60_000,
            remaining_ms: 5_000,
            build: "dev".into(),
            platform: Platform::Web,
        })
    }

    #[test]
    fn big_endian_writers_and_prefixes_are_exact() {
        let mut writer = CanonicalWriter::new();
        writer.u8(0x12);
        writer.u16(0x3456);
        writer.u32(0x789a_bcde);
        writer.i32(-2);
        writer.u64(0x0102_0304_0506_0708);
        writer.i64(-3);
        writer.lp1("A").unwrap();
        writer.lp4(&[0xbb, 0xcc]).unwrap();
        assert_eq!(
            hex(writer.as_slice()),
            "123456789abcdefffffffe0102030405060708fffffffffffffffd014100000002bbcc".to_string()
        );
        assert_eq!(lp1(""), Err(CanonicalError::EmptyLp1));
        assert!(matches!(
            lp1(&"x".repeat(256)),
            Err(CanonicalError::Lp1TooLong { .. })
        ));
        assert!(matches!(
            lp4(&vec![0; MAX_LP4_BYTES + 1]),
            Err(CanonicalError::Lp4TooLong { .. })
        ));
    }

    #[test]
    fn session_headers_only_change_the_frozen_start_tail() {
        let seed_commitment = [0x11; 32];
        let schedule_hash = [0x22; 32];
        let input = SessionHeader {
            category: super::super::RIGHT_OF_WAY_CATEGORY,
            session_id: "S01",
            challenge: "C01",
            seed_commitment: &seed_commitment,
            schedule_hash: &schedule_hash,
            issued_at_ms: 100,
        };
        let unstarted = unstarted_session_header(&input, 200).unwrap();
        let started = started_session_header(&input, 150).unwrap();
        assert_eq!(
            &unstarted[..unstarted.len() - 17],
            &started[..started.len() - 17]
        );
        assert_eq!(
            &unstarted[unstarted.len() - 17..],
            &[0, 0, 0, 0, 0, 0, 0, 200, 0, 0, 0, 0, 0, 0, 0, 0, 0]
        );
        assert_eq!(
            &started[started.len() - 17..],
            &[0, 0, 0, 0, 0, 0, 0, 0, 1, 0, 0, 0, 0, 0, 0, 0, 150]
        );
        assert!(worker_proof_input(&started).starts_with(&lp1(PROOF_DOMAIN).unwrap()));
    }

    #[test]
    fn schedule_matches_the_frozen_commitment_bytes() {
        let seed: [u8; 32] = core::array::from_fn(|index| index as u8 + 1);
        let bytes = schedule_bytes(&seed, super::super::RIGHT_OF_WAY_CATEGORY).unwrap();
        assert_eq!(
            sha256(&bytes),
            super::super::schedule_commitment(&seed, super::super::RIGHT_OF_WAY_CATEGORY)
        );
        assert_eq!(&bytes[..18], &lp1(SCHEDULE_DOMAIN).unwrap());
    }

    #[test]
    fn package_delivery_is_one_record_and_chain_has_no_duplicate_domain_or_previous_hash() {
        let previous = [0x55; 32];
        let event = Event {
            seq: 0,
            active_ms: 9,
            payload: EventPayload::PackageDelivery {
                delivered_ordinal_within_dropoff: 2,
                chain_index: 3,
                base: 7,
                premium_bps: 9_000,
                guilt: true,
                credited: 3,
                accumulator_before: -1,
                accumulator_after: 2,
                remaining_before_ms: 1_000,
                remaining_after_ms: 4_000,
            },
        };
        let stored = chain_event(&previous, &event).unwrap();
        let domain = lp1(EVENT_DOMAIN).unwrap();
        assert_eq!(&stored.record[..domain.len()], &domain);
        assert_eq!(
            stored
                .record
                .windows(domain.len())
                .filter(|part| *part == domain)
                .count(),
            1
        );
        assert!(!stored.record.windows(32).any(|part| part == previous));
        let mut expected_hash_input = previous.to_vec();
        expected_hash_input.extend_from_slice(&stored.record);
        assert_eq!(stored.event_hash, sha256(&expected_hash_input));
        assert_eq!(stored.canonical_bytes().len(), stored.record.len() + 32);
    }

    #[test]
    fn evidence_terminal_and_root_layouts_are_exact() {
        let terminal = right_of_way_terminal();
        let mut ledger = CanonicalLedger::new(b"started-header");
        ledger
            .append(&Event {
                seq: 0,
                active_ms: 60_000,
                payload: EventPayload::Terminal(terminal.clone()),
            })
            .unwrap();
        assert_eq!(ledger.event_count(), 1);
        let evidence = ledger.evidence_bytes("S01").unwrap();
        let prefix = lp1(EVIDENCE_DOMAIN).unwrap();
        assert_eq!(&evidence[..prefix.len()], &prefix);
        assert_eq!(ledger.evidence_hash("S01").unwrap(), sha256(&evidence));
        let hash_hex = evidence_hash_hex(&evidence);
        assert_eq!(hash_hex.len(), 64);
        assert!(
            hash_hex
                .bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
        );

        let aggregates = terminal.canonical_bytes().unwrap();
        assert_eq!(aggregates[0], Conduct::RightOfWay as u8);
        let mut root_input = lp1(ROOT_DOMAIN).unwrap();
        root_input.extend_from_slice(ledger.h0());
        root_input.extend_from_slice(ledger.last_hash());
        root_input.extend_from_slice(&aggregates);
        assert_eq!(ledger.final_root().unwrap(), sha256(&root_input));
        assert_eq!(
            ledger.append(&Event {
                seq: 1,
                active_ms: 60_001,
                payload: EventPayload::TimePickup {
                    remaining_before_ms: 0,
                    remaining_after_ms: 1
                }
            }),
            Err(CanonicalError::EventAfterTerminal)
        );
    }

    #[test]
    fn right_of_way_score_hmac_golden() {
        let schedule =
            decode_hex("bb785fb44d72ad7ea1b957df9bcc95dffdd814a475e736a0e74beceee2d3049e");
        let seed = decode_hex("1f79a204b991758a8798f650465fc89634f967a3976312a2eaaff5912bbd8b48");
        let input = score_hmac_input(
            super::super::RIGHT_OF_WAY_CATEGORY,
            "S01",
            &[0x11; 32],
            schedule.as_slice().try_into().unwrap(),
            seed.as_slice().try_into().unwrap(),
            &right_of_way_terminal(),
        )
        .unwrap();
        assert_eq!(input.len(), RIGHT_OF_WAY_SCORE_HMAC_GOLDEN_LEN);
        assert_eq!(hex(&input), RIGHT_OF_WAY_SCORE_HMAC_GOLDEN_HEX);
        let mut mac = Hmac::<Sha256>::new_from_slice(b"roady-v2-test-client-key").unwrap();
        mac.update(&input);
        assert_eq!(
            URL_SAFE_NO_PAD.encode(mac.finalize().into_bytes()),
            RIGHT_OF_WAY_SCORE_HMAC_GOLDEN_BASE64URL
        );
    }

    #[test]
    fn event_and_ledger_bounds_fail_before_mutation() {
        let too_long = ConductTerminal::CluckHunt(CluckTerminal {
            reason: TerminalReason::Wrecked,
            total: 0,
            chickens: 0,
            coins: 0,
            objective_completed: false,
            max_combo: 1,
            duration_ms: 0,
            remaining_ms: 0,
            build: "x".repeat(MAX_BUILD_BYTES + 1),
            platform: Platform::Native,
        });
        assert!(matches!(
            event_record(&Event {
                seq: 0,
                active_ms: 0,
                payload: EventPayload::Terminal(too_long)
            }),
            Err(CanonicalError::BuildTooLong { .. })
        ));

        let mut ledger = CanonicalLedger::new(b"header");
        let large = EventPayload::PackageDelivery {
            delivered_ordinal_within_dropoff: 0,
            chain_index: 0,
            base: 0,
            premium_bps: 0,
            guilt: false,
            credited: 0,
            accumulator_before: 0,
            accumulator_after: 0,
            remaining_before_ms: 0,
            remaining_after_ms: 0,
        };
        while ledger.stored_bytes().len()
            + event_record(&Event {
                seq: ledger.event_count(),
                active_ms: 0,
                payload: large.clone(),
            })
            .unwrap()
            .len()
            + 32
            <= MAX_LEDGER_BYTES
        {
            ledger
                .append(&Event {
                    seq: ledger.event_count(),
                    active_ms: 0,
                    payload: large.clone(),
                })
                .unwrap();
        }
        let count = ledger.event_count();
        let len = ledger.stored_bytes().len();
        assert!(matches!(
            ledger.append(&Event {
                seq: count,
                active_ms: 0,
                payload: large
            }),
            Err(CanonicalError::LedgerTooLong { .. })
        ));
        assert_eq!(ledger.event_count(), count);
        assert_eq!(ledger.stored_bytes().len(), len);
        assert!(matches!(
            evidence_bytes("S", MAX_EVENTS + 1, &[]),
            Err(CanonicalError::TooManyEvents)
        ));
    }

    fn decode_hex(value: &str) -> Vec<u8> {
        value
            .as_bytes()
            .chunks_exact(2)
            .map(|pair| {
                let text = core::str::from_utf8(pair).unwrap();
                u8::from_str_radix(text, 16).unwrap()
            })
            .collect()
    }
}
