//! Cloudflare leaderboard web client and in-game UI.
//!
//! Implements the client side of `LEADERBOARD_ARCHITECTURE.md` §5 (client
//! HMAC), §10 (Turnstile), and §12 (client integration). The game-facing UI is
//! all Bevy; fetch / Turnstile / WebCrypto use a browser JS bridge on web and
//! degrade gracefully to "unavailable" on native.
//!
//! # Security
//!
//! `ROADY_LEADERBOARD_CLIENT_HMAC_KEY` is an **extractable nuisance key**
//! embedded in the public WASM binary. It deters trivial unsigned API calls
//! but does NOT prove honest gameplay (architecture §1). An attacker who
//! extracts the WASM can recover or reuse this key. It is never stored beyond
//! its compile-time embedded value in the binary, and the same key is installed
//! as a Worker runtime secret (`LB_CLIENT_HMAC_KEY`). This is accepted as
//! defense in depth alongside Turnstile, one-time sessions, rate limits, and
//! plausibility caps — none of which individually prove an honest score.
//!
//! # Non-blocking design
//!
//! All network operations are fire-and-forget `spawn_local` tasks on web. The
//! Bevy game loop never awaits them. Results are communicated back through
//! small thread-local queues polled by lightweight `Update` systems. Each
//! submission carries a monotonically increasing epoch tag; polling discards
//! any result whose epoch no longer matches the current submission, so a
//! result that lands after the player restarts or returns to menu is ignored.
//! The submit queue is also cleared on `GameOver` exit as a backstop.

use bevy::{prelude::*, text::FontSize, window::PrimaryWindow};

use crate::car::InputFrozen;
use crate::combos::{Combo, ComboUpdateSet};
use crate::game::resources::{GameOverReason, RoundActive, Score, TimeLeft};
use crate::game::state::GameState;
use crate::game::{KeyboardStateSet, RestartRequested, SpawnSet, TouchStateSet, settings_closed};
use crate::game_modes::ActiveRunRules;
use crate::modifiers::ActiveModifier;
use crate::objectives::ActiveObjective;
use crate::palette;
use crate::settings::Settings;
use crate::touch::TouchControlsActive;
#[cfg(test)]
use crate::ui::pause_content_bounds;
use crate::ui::{GAMEOVER_STATUS_STRIP_HEIGHT, GameOverCoreRoot, UiBounds, is_mobile_viewport};

// ─── Build-time configuration ───────────────────────────────────────────────
//
// These compile-time env vars have safe disabled defaults: an empty API URL
// disables all leaderboard features; an empty HMAC key or Turnstile site key
// disables submissions while still allowing a read-only board on web.
//
// The HMAC key is embedded in the public WASM and is extractable. See the
// module-level security note above.

const LEADERBOARD_API_URL: &str = match option_env!("LEADERBOARD_API_URL") {
    Some(v) => v,
    None => "",
};

const CLIENT_HMAC_KEY: &str = match option_env!("ROADY_LEADERBOARD_CLIENT_HMAC_KEY") {
    Some(v) => v,
    None => "",
};

const TURNSTILE_SITE_KEY: &str = match option_env!("LB_TURNSTILE_SITE_KEY") {
    Some(v) => v,
    None => "",
};

const BUILD_VERSION: &str = env!("CARGO_PKG_VERSION");

/// Number of global leaderboard entries fetched and displayed on both the
/// Menu and Pause screens. Pause pairs these ten rows into five compact lines.
#[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]
const BOARD_LIMIT: u8 = 10;

/// Soft review threshold for elapsed round duration: 30 minutes. Time pickups
/// can extend active play without a gameplay-derived elapsed-time maximum, so
/// neither client nor Worker rejects at this threshold. The Worker accepts and
/// flags longer rounds for moderation.
#[allow(dead_code)] // soft review constant is asserted by native tests
const MAX_ROUND_DURATION_MS: u64 = 1_800_000;

/// Hard reject bound for elapsed round duration: the largest integer safely
/// representable as a JavaScript `Number` (`Number.MAX_SAFE_INTEGER`,
/// 2^53 - 1). A u64 above this cannot round-trip through JSON for exact
/// canonical signing, so the client rejects it before any network call. This
/// mirrors the Worker's `Number.isSafeInteger` check; values up to and
/// including this bound are sent to the Worker.
#[allow(dead_code)] // used by the wasm submission guard and native tests
const MAX_SAFE_INTEGER_MS: u64 = 9_007_199_254_740_991;

#[allow(dead_code)] // used by the wasm submission guard and native tests
fn valid_round_duration_ms(duration: u64) -> bool {
    duration <= MAX_SAFE_INTEGER_MS
}

fn leaderboard_enabled() -> bool {
    !LEADERBOARD_API_URL.is_empty()
}

/// Submission requires the API URL, HMAC key, Turnstile site key, *and* a web
/// target. Native builds never submit (no fetch / WebCrypto / Turnstile).
fn submission_enabled() -> bool {
    leaderboard_enabled()
        && !CLIENT_HMAC_KEY.is_empty()
        && !TURNSTILE_SITE_KEY.is_empty()
        && cfg!(target_arch = "wasm32")
}

fn platform_str() -> &'static str {
    if cfg!(target_arch = "wasm32") {
        "web"
    } else {
        "native"
    }
}

#[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]
fn api_base() -> &'static str {
    LEADERBOARD_API_URL.trim_end_matches('/')
}

#[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]
fn board_url() -> String {
    format!("{}/v1/leaderboard?limit={BOARD_LIMIT}", api_base())
}

#[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]
fn session_url() -> String {
    format!("{}/v1/session", api_base())
}

#[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]
fn scores_url() -> String {
    format!("{}/v1/scores", api_base())
}

// ─── Pure logic ──────────────────────────────────────────────────────────────

/// Normalize initials to uppercase ASCII `[A-Z0-9]{3,5}`. Returns `None` if
/// the trimmed input contains invalid characters or is the wrong length. This
/// matches the server-side `normalizeName` in `leaderboard/src/validation.ts`.
fn normalize_initials(raw: &str) -> Option<String> {
    let trimmed = raw.trim();
    if !(3..=5).contains(&trimmed.len()) || !trimmed.is_ascii() {
        return None;
    }
    let normalized = trimmed.to_ascii_uppercase();
    if !normalized
        .bytes()
        .all(|c| c.is_ascii_uppercase() || c.is_ascii_digit())
    {
        return None;
    }
    Some(normalized)
}

/// Input for the canonical score HMAC bytes. Field order and types mirror the
/// backend's `canonicalScoreBytes` in `leaderboard/src/security.ts`.
#[allow(dead_code)] // used on wasm32 (web_bridge) and in tests
#[derive(Clone, Debug, Default, PartialEq, Eq)]
struct CanonicalScoreInput {
    session_id: String,
    proof: String,
    name: String,
    condition: u8,
    terminal_total: u32,
    chickens: u32,
    coins: u32,
    objective_completed: bool,
    max_combo: u32,
    round_duration_ms: u64,
    time_left_ms: u64,
    game_over_reason: String,
    build: String,
    platform: String,
}

/// Build the canonical client submission HMAC bytes (architecture §5).
///
/// Fixed field order, one ASCII LF (`\n`) separator, **no trailing LF**.
/// Integers are canonical base-10 (no leading `+` or unnecessary leading
/// zeroes). The name is already normalized to uppercase `[A-Z0-9]{3,5}`.
///
/// This must produce byte-identical output to the Worker's
/// `canonicalScoreBytes` in `leaderboard/src/security.ts`.
#[allow(dead_code)] // used on wasm32 (web_bridge) and in tests
fn canonical_score_bytes(input: &CanonicalScoreInput) -> Vec<u8> {
    let objective = if input.objective_completed { "1" } else { "0" };
    let parts: Vec<String> = vec![
        "roady.v1.score".to_string(),
        input.session_id.clone(),
        input.proof.clone(),
        input.name.clone(),
        input.condition.to_string(),
        input.terminal_total.to_string(),
        input.chickens.to_string(),
        input.coins.to_string(),
        objective.to_string(),
        input.max_combo.to_string(),
        input.round_duration_ms.to_string(),
        input.time_left_ms.to_string(),
        input.game_over_reason.clone(),
        input.build.clone(),
        input.platform.clone(),
    ];
    parts.join("\n").into_bytes()
}

/// Encode bytes as unpadded base64url (RFC 4648 §5, no `=`).
///
/// Used only in tests to verify against known vectors. The actual HMAC
/// signature is computed by the browser's WebCrypto and returned as
/// base64url from the JS bridge.
#[allow(dead_code)] // used in tests only
fn to_base64url(bytes: &[u8]) -> String {
    const TABLE: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";
    let mut out = String::with_capacity((bytes.len() * 4 + 2) / 3);
    for chunk in bytes.chunks(3) {
        let b = [
            chunk[0],
            *chunk.get(1).unwrap_or(&0),
            *chunk.get(2).unwrap_or(&0),
        ];
        out.push(TABLE[(b[0] >> 2) as usize] as char);
        out.push(TABLE[(((b[0] & 0x03) << 4) | (b[1] >> 4)) as usize] as char);
        if chunk.len() > 1 {
            out.push(TABLE[(((b[1] & 0x0f) << 2) | (b[2] >> 6)) as usize] as char);
        }
        if chunk.len() > 2 {
            out.push(TABLE[(b[2] & 0x3f) as usize] as char);
        }
    }
    out
}

// ─── Submission state machine ────────────────────────────────────────────────

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum SubmissionState {
    #[default]
    Idle,
    Ready,
    EnteringInitials,
    Submitting,
    Submitted,
    Failed,
    Skipped,
    Unavailable,
}

/// Transition when the player presses Enter with valid initials.
fn transition_on_submit(current: SubmissionState) -> SubmissionState {
    match current {
        SubmissionState::EnteringInitials => SubmissionState::Submitting,
        _ => current,
    }
}

/// Transition when the player opts into submission from the `Ready` prompt
/// (presses `L` or taps the SUBMIT touch zone).
fn transition_on_opt_in(current: SubmissionState) -> SubmissionState {
    match current {
        SubmissionState::Ready => SubmissionState::EnteringInitials,
        _ => current,
    }
}

/// Transition when the player presses Escape / taps Skip.
fn transition_on_skip(current: SubmissionState) -> SubmissionState {
    match current {
        SubmissionState::EnteringInitials | SubmissionState::Failed => SubmissionState::Skipped,
        _ => current,
    }
}

/// Transition when the async submission chain returns both ranks.
fn transition_on_success(current: SubmissionState) -> SubmissionState {
    match current {
        SubmissionState::Submitting => SubmissionState::Submitted,
        _ => current,
    }
}

/// Transition when the async submission chain returns an error.
fn transition_on_error(current: SubmissionState) -> SubmissionState {
    match current {
        SubmissionState::Submitting => SubmissionState::Failed,
        _ => current,
    }
}

/// Transition when the player explicitly retries from the Failed state.
/// Retry returns to editable initials and does not itself start a network
/// chain, preventing automatic retry loops after offline/verification errors.
fn transition_on_retry(current: SubmissionState) -> SubmissionState {
    match current {
        SubmissionState::Failed => SubmissionState::EnteringInitials,
        _ => current,
    }
}

/// Whether regular Game Over restart/menu input should be suspended.
/// Only `EnteringInitials` and `Failed` own the keyboard; all other states
/// allow normal restart/menu navigation.
fn input_suspended(state: SubmissionState) -> bool {
    matches!(
        state,
        SubmissionState::EnteringInitials | SubmissionState::Failed
    )
}

fn interactive_modal(state: SubmissionState) -> bool {
    matches!(
        state,
        SubmissionState::EnteringInitials | SubmissionState::Failed
    )
}

/// Desktop retains the established layered presentation. On mobile, an
/// interaction-owning submission state replaces (rather than collides with)
/// the normal terminal core.
fn normal_gameover_core_visible(state: SubmissionState, mobile: bool) -> bool {
    !(mobile && interactive_modal(state))
}

fn gameover_status_bounds(width: f32, height: f32, state: SubmissionState) -> UiBounds {
    if is_mobile_viewport(width, height) && interactive_modal(state) {
        UiBounds {
            left: 0.0,
            top: 0.0,
            width,
            height,
        }
    } else if is_mobile_viewport(width, height) {
        UiBounds {
            left: 0.0,
            top: (height - GAMEOVER_STATUS_STRIP_HEIGHT).max(0.0),
            width,
            height: GAMEOVER_STATUS_STRIP_HEIGHT.min(height.max(0.0)),
        }
    } else {
        // The desktop panel continues to size to its content at bottom + 12.
        UiBounds {
            left: 0.0,
            top: 0.0,
            width,
            height: 0.0,
        }
    }
}

/// Initial Game Over submission policy. A remembered valid name is the
/// durable preference requested by the client flow and submits future scores
/// automatically; an absent/corrupt name keeps the original opt-in prompt.
/// Auto submission is still disabled when the web stack is unavailable.
#[derive(Clone, Debug, PartialEq, Eq)]
enum SubmissionStartDecision {
    Unavailable,
    AwaitOptIn,
    AutoSubmit(String),
}

fn submission_start_decision(
    can_submit: bool,
    remembered_initials: &str,
) -> SubmissionStartDecision {
    if !can_submit {
        return SubmissionStartDecision::Unavailable;
    }
    match normalize_initials(remembered_initials) {
        Some(name) => SubmissionStartDecision::AutoSubmit(name),
        None => SubmissionStartDecision::AwaitOptIn,
    }
}

/// Decide whether a valid manual submission should update the remembered
/// name. This is called when the player explicitly confirms valid initials,
/// before the asynchronous chain starts; remembering that opt-in does not
/// depend on the network succeeding. Returning `None` for unchanged/invalid
/// input avoids unnecessary settings writes while still allowing a manually
/// edited name to replace the old one after an explicit retry.
fn remembered_initials_update(current: &str, submitted: &str) -> Option<String> {
    let normalized = normalize_initials(submitted)?;
    if normalize_initials(current).as_deref() == Some(normalized.as_str()) {
        None
    } else {
        Some(normalized)
    }
}

// ─── Touch grid mapping ──────────────────────────────────────────────────────

/// A touch-grid action for the initials entry grid.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum GridAction {
    Char(char),
    Backspace,
    Submit,
    Skip,
}

type GridBottomControl = (&'static str, GridAction);

const INITIALS_BOTTOM_CONTROLS: &[GridBottomControl] = &[
    ("BACK", GridAction::Backspace),
    ("SUBMIT", GridAction::Submit),
    ("SKIP", GridAction::Skip),
];
const FAILED_BOTTOM_CONTROLS: &[GridBottomControl] =
    &[("RETRY", GridAction::Submit), ("SKIP", GridAction::Skip)];

/// Normalized full-window regions used by both the mobile modal's absolute UI
/// nodes and structural overlap tests. Adjacent regions deliberately retain a
/// 2% vertical gap so borders and wrapped glyphs cannot collide.
#[derive(Clone, Copy, Debug, PartialEq)]
struct ModalRegion {
    left: f32,
    top: f32,
    width: f32,
    height: f32,
}

impl ModalRegion {
    fn right(self) -> f32 {
        self.left + self.width
    }

    fn bottom(self) -> f32 {
        self.top + self.height
    }

    #[cfg(test)]
    fn is_disjoint(self, other: Self) -> bool {
        self.right() <= other.left
            || other.right() <= self.left
            || self.bottom() <= other.top
            || other.bottom() <= self.top
    }
}

const MOBILE_INITIALS_HEADER_REGION: ModalRegion = ModalRegion {
    left: 0.05,
    top: 0.02,
    width: 0.90,
    height: 0.16,
};
const MOBILE_CONSENT_REGION: ModalRegion = ModalRegion {
    left: 0.05,
    top: 0.20,
    width: 0.90,
    height: 0.18,
};
const MOBILE_CHARACTERS_REGION: ModalRegion = ModalRegion {
    left: 0.05,
    top: 0.40,
    width: 0.90,
    height: 0.45,
};
const MOBILE_ACTIONS_REGION: ModalRegion = ModalRegion {
    left: 0.05,
    top: 0.87,
    width: 0.90,
    height: 0.10,
};
const MOBILE_FAILED_HEADER_REGION: ModalRegion = ModalRegion {
    left: 0.05,
    top: 0.05,
    width: 0.90,
    height: 0.77,
};

/// Bottom controls shown for each interactive submission state. Keeping this
/// separate from character-grid visibility prevents a Failed modal from
/// displaying actions that its input handler does not accept.
fn grid_bottom_controls(state: SubmissionState) -> &'static [GridBottomControl] {
    match state {
        SubmissionState::EnteringInitials => INITIALS_BOTTOM_CONTROLS,
        SubmissionState::Failed => FAILED_BOTTOM_CONTROLS,
        _ => &[],
    }
}

fn grid_shows_characters(state: SubmissionState) -> bool {
    state == SubmissionState::EnteringInitials
}

/// Map a Failed modal action to its retry/skip transition. `None` means the
/// action is not a control in that modal.
fn failed_grid_transition(action: GridAction) -> Option<SubmissionState> {
    match action {
        GridAction::Submit => Some(transition_on_retry(SubmissionState::Failed)),
        GridAction::Skip => Some(transition_on_skip(SubmissionState::Failed)),
        GridAction::Char(_) | GridAction::Backspace => None,
    }
}

/// 6×6 grid of A–Z + 0–9 (36 chars), then bottom action zones. Normalized
/// coordinates (0..1, origin top-left). This is a pure function testable
/// without a window.
fn grid_action_for_normalized(x: f32, y: f32) -> Option<GridAction> {
    // Character grid uses the exact visible region, including its outer edge.
    if (MOBILE_CHARACTERS_REGION.top..=MOBILE_CHARACTERS_REGION.bottom()).contains(&y)
        && (MOBILE_CHARACTERS_REGION.left..=MOBILE_CHARACTERS_REGION.right()).contains(&x)
    {
        let col = (((x - MOBILE_CHARACTERS_REGION.left) / (MOBILE_CHARACTERS_REGION.width / 6.0))
            .floor() as usize)
            .min(5);
        let row = (((y - MOBILE_CHARACTERS_REGION.top) / (MOBILE_CHARACTERS_REGION.height / 6.0))
            .floor() as usize)
            .min(5);
        const CHARS: &[char] = &[
            'A', 'B', 'C', 'D', 'E', 'F', //
            'G', 'H', 'I', 'J', 'K', 'L', //
            'M', 'N', 'O', 'P', 'Q', 'R', //
            'S', 'T', 'U', 'V', 'W', 'X', //
            'Y', 'Z', '0', '1', '2', '3', //
            '4', '5', '6', '7', '8', '9', //
        ];
        let idx = row * 6 + col;
        let ch = *CHARS.get(idx)?;
        return Some(GridAction::Char(ch));
    }
    // Bottom buttons use the exact visible action-region height.
    if (MOBILE_ACTIONS_REGION.top..=MOBILE_ACTIONS_REGION.bottom()).contains(&y) {
        if (0.05..=0.30).contains(&x) {
            return Some(GridAction::Backspace);
        }
        if (0.35..=0.65).contains(&x) {
            return Some(GridAction::Submit);
        }
        if (0.70..=0.95).contains(&x) {
            return Some(GridAction::Skip);
        }
    }
    None
}

// ─── Display helpers ─────────────────────────────────────────────────────────

/// Show the typed initials padded with underscores to 5 slots.
fn format_initials_display(initials: &str) -> String {
    let mut display = initials.to_string();
    while display.len() < 5 {
        display.push('_');
    }
    display
}

const SUBMISSION_CONSENT_DISCLOSURE: &str = "Submitting stores this name and auto-submits future completed rounds.\n\
Clear Leaderboard Name in Settings to revoke consent and stop auto-submit.";

/// Convert only ranked-eligible terminal reasons to backend literals. Local
/// outcomes have no serialization fallback and must never masquerade as wrecks.
fn game_over_reason_str(reason: GameOverReason) -> Option<&'static str> {
    match reason {
        GameOverReason::TimeUp => Some("time_up"),
        GameOverReason::Wrecked => Some("wrecked"),
        GameOverReason::Drowned => None,
    }
}

#[allow(dead_code)] // used by friendly_error on wasm32 and native tests
fn is_payload_validation_code(code: &str) -> bool {
    matches!(
        code,
        "invalid_body"
            | "invalid_name"
            | "invalid_condition"
            | "invalid_total"
            | "invalid_chickens"
            | "invalid_coins"
            | "total_mismatch"
            | "invalid_objective"
            | "invalid_combo"
            | "implausible_combo"
            | "invalid_duration"
            | "invalid_time_left"
            | "invalid_reason"
            | "invalid_build"
            | "invalid_platform"
            | "score_over_cap"
    )
}

/// Map a backend error code to a player-friendly message. Validation failures
/// visibly retain their code (useful for support and non-retryable diagnosis),
/// while retryable session/service failures say so explicitly. Network and
/// browser Turnstile failures are classified separately at their call sites.
#[allow(dead_code)] // used on wasm32 (web_bridge) and in tests
fn friendly_error(code: Option<&str>, message: Option<&str>, fallback: &str) -> String {
    match code {
        Some("rate_limited") => "SERVER [rate_limited]: retry later.".to_string(),
        Some("turnstile_failed") => {
            "TURNSTILE [turnstile_failed]: verification failed; retry.".to_string()
        }
        Some(code @ "invalid_session")
        | Some(code @ "expired_session")
        | Some(code @ "replay")
        | Some(code @ "condition_mismatch") => {
            format!("SERVER [{code}]: session expired; retry to request a new session.")
        }
        Some("invalid_proof") => {
            "SERVER [invalid_proof]: session verification failed; retry.".to_string()
        }
        Some(code @ "invalid_signature") | Some(code @ "missing_signature") => {
            format!("SERVER [{code}]: signature rejected; retry.")
        }
        Some("insert_failed") => "SERVER [insert_failed]: retry with a new session.".to_string(),
        Some(code) if is_payload_validation_code(code) => format!(
            "VALIDATION [{code}]: {}; retry unchanged will fail.",
            message.unwrap_or(fallback)
        ),
        Some(code) => format!("SERVER [{code}]: {}; retry.", message.unwrap_or(fallback)),
        None => format!("NETWORK/SERVER [http]: {fallback}; retry."),
    }
}

// ─── Data types ──────────────────────────────────────────────────────────────

// JSON (de)serialization types live inside `web_bridge` (wasm32 only) to avoid
// dead-code warnings on native. `friendly_error` stays here because it is also
// used in tests.

/// Terminal score snapshot taken at `GameOver` enter, used for the submission
/// body and canonical HMAC.
#[derive(Clone, Debug, Default, PartialEq)]
struct ScoreSnapshot {
    condition: u8,
    terminal_total: u32,
    chickens: u32,
    coins: u32,
    objective_completed: bool,
    max_combo: u32,
    round_duration_ms: u64,
    time_left_ms: u64,
    game_over_reason: Option<String>,
    build: String,
    platform: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum BoardStatus {
    Idle,
    Fetching,
    Fetched,
    Error(String),
    Unavailable,
}

impl Default for BoardStatus {
    fn default() -> Self {
        Self::Idle
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
struct BoardEntry {
    rank: u32,
    name: String,
    score: u32,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct SubmissionRanks {
    global: u32,
    condition: u32,
}

/// Shared Menu/Pause leaderboard state. `cached_entries` holds the last
/// successful fetch for offline display when a later fetch fails.
#[derive(Resource, Default)]
struct LeaderboardBoard {
    status: BoardStatus,
    entries: Vec<BoardEntry>,
    cached_entries: Vec<BoardEntry>,
    /// Tags async reads so a result from a prior Menu/Pause lifecycle cannot
    /// overwrite the current panel after rapid state transitions.
    fetch_epoch: u64,
}

/// Game Over submission state.
#[derive(Resource, Default)]
struct LeaderboardSubmission {
    state: SubmissionState,
    initials: String,
    snapshot: ScoreSnapshot,
    ranks: Option<SubmissionRanks>,
    error: Option<String>,
    /// Monotonic tag stamped onto each async submission so the polling system
    /// can discard results from a superseded submission (restart / retry).
    submit_epoch: u64,
}

/// Peak combo multiplier reached during the current round (1..=5).
#[derive(Resource)]
struct PeakCombo(u32);

impl Default for PeakCombo {
    fn default() -> Self {
        Self(1)
    }
}

/// Elapsed round time in milliseconds, accumulated during `Playing`.
#[derive(Resource, Default)]
struct RoundElapsedMs(u64);

// ─── UI components ───────────────────────────────────────────────────────────

#[derive(Component)]
struct LeaderboardBoardRoot;

#[derive(Component)]
struct LeaderboardBoardTitle;

#[derive(Component)]
struct LeaderboardBoardText;

#[derive(Component)]
struct LeaderboardGameOverRoot;

#[derive(Component)]
struct LeaderboardGameOverPanel;

#[derive(Component)]
struct LeaderboardGameOverText;

#[derive(Component)]
struct LeaderboardTouchGrid(bool);

// ─── Web bridge (wasm32 only) ────────────────────────────────────────────────

#[cfg(target_arch = "wasm32")]
mod web_bridge {
    use super::*;
    use std::cell::RefCell;
    use wasm_bindgen::{JsCast, JsValue, closure::Closure};
    use wasm_bindgen_futures::JsFuture;

    thread_local! {
        // Small result queues (rather than latest-only slots) prevent a stale,
        // slower request from overwriting a newer lifecycle's completed result.
        // Epoch checks below still decide which queued result is authoritative.
        pub static BOARD_RESULTS: RefCell<Vec<(u64, Result<Vec<BoardEntry>, String>)>> =
            RefCell::new(Vec::new());
        pub static SUBMIT_RESULTS: RefCell<Vec<(u64, Result<SubmissionRanks, String>)>> =
            RefCell::new(Vec::new());
    }

    pub fn clear_board() {
        BOARD_RESULTS.with(|s| s.borrow_mut().clear());
    }

    pub fn clear_submit() {
        SUBMIT_RESULTS.with(|s| s.borrow_mut().clear());
    }

    /// Cancel every pending JS Turnstile challenge. The bridge settles each
    /// associated Promise, allowing its Rust future to finish and be discarded
    /// by the submission epoch after Game Over exits.
    pub fn cancel_turnstile_requests() {
        let Ok(api) = js_api() else {
            return;
        };
        let Ok(func) = js_fn(&api, "cancelTurnstileRequests") else {
            return;
        };
        let _ = func.call0(&api);
    }

    pub fn take_board() -> Option<(u64, Result<Vec<BoardEntry>, String>)> {
        BOARD_RESULTS.with(|s| s.borrow_mut().pop())
    }

    pub fn take_submit() -> Option<(u64, Result<SubmissionRanks, String>)> {
        SUBMIT_RESULTS.with(|s| s.borrow_mut().pop())
    }

    // ── JSON types (wasm32 only) ─────────────────────────────────────────

    use serde::{Deserialize, Serialize};

    #[derive(Deserialize)]
    struct LeaderboardResponse {
        entries: Vec<LeaderboardEntry>,
    }

    #[derive(Deserialize)]
    struct LeaderboardEntry {
        rank: u32,
        name: String,
        score: u32,
    }

    #[derive(Serialize)]
    struct SessionBody {
        condition: u8,
        #[serde(rename = "turnstileToken")]
        turnstile_token: String,
    }

    #[derive(Deserialize)]
    struct SessionResponse {
        #[serde(rename = "sessionId")]
        session_id: String,
        proof: String,
    }

    #[derive(Serialize)]
    struct ScoreBody {
        #[serde(rename = "sessionId")]
        session_id: String,
        proof: String,
        name: String,
        condition: u8,
        terminal_total: u32,
        chickens: u32,
        coins: u32,
        objective_completed: bool,
        max_combo: u32,
        round_duration_ms: u64,
        time_left_ms: u64,
        game_over_reason: String,
        build: String,
        platform: String,
    }

    #[derive(Deserialize)]
    struct SubmitResponse {
        /// Rank within the submitted road condition.
        rank: u32,
        #[serde(rename = "globalRank")]
        global_rank: u32,
    }

    #[derive(Deserialize)]
    struct ErrorResponse {
        error: ErrorDetail,
    }

    #[derive(Deserialize)]
    struct ErrorDetail {
        code: String,
        message: String,
    }

    fn parse_error(body: &str) -> Option<(String, String)> {
        let resp: ErrorResponse = serde_json::from_str(body).ok()?;
        Some((resp.error.code, resp.error.message))
    }

    // ── web-sys fetch ────────────────────────────────────────────────────

    /// Fetch JSON via `web-sys` with a 15s abort timeout. Returns
    /// `(status, body_text)`. The abort timer is cleared after the response
    /// arrives or an error short-circuits, so it can never fire once the
    /// future has settled.
    async fn fetch_json(
        url: &str,
        method: &str,
        body: Option<&str>,
        signature: Option<&str>,
    ) -> Result<(u16, String), String> {
        let window = web_sys::window().ok_or("no window")?;
        let opts = web_sys::RequestInit::new();
        opts.set_method(method);

        let headers = web_sys::Headers::new().map_err(|e| fmt_js(e, "headers"))?;
        // A plain GET is a CORS "simple request" and should not trigger an
        // OPTIONS preflight. Only JSON requests with a body need Content-Type.
        if body.is_some() {
            headers
                .set("Content-Type", "application/json")
                .map_err(|e| fmt_js(e, "content-type"))?;
        }
        if let Some(sig) = signature {
            headers
                .set("X-Roady-Client-Signature", sig)
                .map_err(|e| fmt_js(e, "signature header"))?;
        }
        opts.set_headers(headers.as_ref());

        if let Some(body) = body {
            let body_val = JsValue::from_str(body);
            opts.set_body(&body_val);
        }

        // 15s timeout via AbortController. The timer is cleared once the
        // request settles (response or error) so it cannot abort a request
        // that already completed.
        const FETCH_TIMEOUT_MS: i32 = 15_000;
        let controller =
            web_sys::AbortController::new().map_err(|e| fmt_js(e, "abort controller"))?;
        opts.set_signal(Some(&controller.signal()));

        let abort_cb = {
            let controller = controller.clone();
            Closure::<dyn FnMut()>::new(move || controller.abort())
        };
        let timer = window
            .set_timeout_with_callback_and_timeout_and_arguments_0(
                abort_cb.as_ref().unchecked_ref(),
                FETCH_TIMEOUT_MS,
            )
            .map_err(|e| fmt_js(e, "timeout"))?;

        let result = async {
            let request = web_sys::Request::new_with_str_and_init(url, &opts)
                .map_err(|e| fmt_js(e, "request"))?;

            let fetch_promise = window.fetch_with_request(&request);
            let resp_val = JsFuture::from(fetch_promise).await.map_err(|e| {
                if controller.signal().aborted() {
                    "Request timed out".to_string()
                } else {
                    fmt_js(e, "fetch")
                }
            })?;
            let response: web_sys::Response = resp_val
                .dyn_into()
                .map_err(|_| "response cast".to_string())?;

            let status = response.status();
            let text_promise = response.text().map_err(|e| fmt_js(e, "text"))?;
            let text_val = JsFuture::from(text_promise).await.map_err(|e| {
                if controller.signal().aborted() {
                    "Request timed out".to_string()
                } else {
                    fmt_js(e, "text await")
                }
            })?;
            let text = text_val.as_string().unwrap_or_default();
            Ok((status, text))
        }
        .await;

        // Clear the timeout so it can never fire after the future settled.
        window.clear_timeout_with_handle(timer);
        drop(abort_cb);
        result
    }

    // ── JS bridge calls ──────────────────────────────────────────────────

    /// Call `window.roadyLeaderboard.hmacSha256Base64Url(key, data)` and
    /// await the returned Promise. The JS function uses WebCrypto HMAC-SHA-256
    /// and returns unpadded base64url.
    async fn js_hmac(key: &str, data: &str) -> Result<String, String> {
        let api = js_api()?;
        let func = js_fn(&api, "hmacSha256Base64Url")?;
        let promise = func
            .call2(&api, &key.into(), &data.into())
            .map_err(|e| fmt_js(e, "hmac call"))?;
        let result = JsFuture::from(js_sys::Promise::from(promise))
            .await
            .map_err(|e| fmt_js(e, "hmac promise"))?;
        result
            .as_string()
            .ok_or_else(|| "hmac result not a string".to_string())
    }

    /// Call `window.roadyLeaderboard.getTurnstileToken(siteKey)` and await
    /// the returned Promise. The JS function renders a Cloudflare Turnstile
    /// widget and resolves with `{ ok: true, token }` or `{ ok: false, error }`.
    async fn js_turnstile(site_key: &str) -> Result<String, String> {
        let api = js_api()?;
        let func = js_fn(&api, "getTurnstileToken")?;
        let promise = func
            .call1(&api, &site_key.into())
            .map_err(|e| fmt_js(e, "turnstile call"))?;
        let result = JsFuture::from(js_sys::Promise::from(promise))
            .await
            .map_err(|e| fmt_js(e, "turnstile promise"))?;

        let ok =
            js_sys::Reflect::get(&result, &"ok".into()).map_err(|e| fmt_js(e, "turnstile ok"))?;
        if ok.as_bool() == Some(true) {
            let token = js_sys::Reflect::get(&result, &"token".into())
                .map_err(|e| fmt_js(e, "turnstile token"))?;
            token
                .as_string()
                .ok_or_else(|| "turnstile token not a string".to_string())
        } else {
            let error = js_sys::Reflect::get(&result, &"error".into())
                .map_err(|e| fmt_js(e, "turnstile error"))?;
            Err(error.as_string().unwrap_or_else(|| "unknown".to_string()))
        }
    }

    fn js_api() -> Result<JsValue, String> {
        let global = js_sys::global();
        let api = js_sys::Reflect::get(&global, &"roadyLeaderboard".into())
            .map_err(|e| fmt_js(e, "global roadyLeaderboard"))?;
        if api.is_undefined() || api.is_null() {
            return Err("JS bridge not found".to_string());
        }
        Ok(api)
    }

    fn js_fn(api: &JsValue, name: &str) -> Result<js_sys::Function, String> {
        let func = js_sys::Reflect::get(api, &name.into())
            .map_err(|e| fmt_js(e, &format!("bridge fn {name}")))?;
        func.dyn_into::<js_sys::Function>()
            .map_err(|_| format!("{name} is not a function"))
    }

    fn fmt_js(e: JsValue, ctx: &str) -> String {
        format!("{ctx}: {:?}", e.as_string().unwrap_or_else(|| "?".into()))
    }

    // ── Board fetch ──────────────────────────────────────────────────────

    pub fn fetch_board(epoch: u64) {
        let url = board_url();
        wasm_bindgen_futures::spawn_local(async move {
            let result = async {
                let (status, body) = fetch_json(&url, "GET", None, None).await?;
                if status != 200 {
                    return Err(format!("HTTP {status}"));
                }
                let resp: LeaderboardResponse =
                    serde_json::from_str(&body).map_err(|e| format!("parse: {e}"))?;
                Ok(resp
                    .entries
                    .into_iter()
                    .map(|e| BoardEntry {
                        rank: e.rank,
                        name: e.name,
                        score: e.score,
                    })
                    .collect::<Vec<_>>())
            }
            .await;
            BOARD_RESULTS.with(|s| s.borrow_mut().push((epoch, result)));
        });
    }

    // ── Full submission chain ────────────────────────────────────────────

    pub fn start_submission(epoch: u64, snapshot: ScoreSnapshot, initials: String) {
        // Enforce the JS-safe-integer hard max before requesting a Turnstile
        // token or sending any payload: a u64 above Number.MAX_SAFE_INTEGER
        // cannot round-trip through JSON for exact canonical signing. Rust's
        // u64 type already guarantees a non-negative integral client value, so
        // the client does not pre-reject the 30-minute soft review cap
        // (MAX_ROUND_DURATION_MS); the Worker accepts longer rounds and adds
        // a deterministic moderation flag.
        if !valid_round_duration_ms(snapshot.round_duration_ms) {
            SUBMIT_RESULTS.with(|results| {
                results.borrow_mut().push((
                    epoch,
                    Err(format!(
                        "VALIDATION [invalid_duration]: round_duration_ms must be a safe integer <= {MAX_SAFE_INTEGER_MS}; retry unchanged will fail."
                    )),
                ));
            });
            return;
        }

        let session_url = session_url();
        let scores_url = scores_url();
        let site_key = TURNSTILE_SITE_KEY.to_string();
        let client_key = CLIENT_HMAC_KEY.to_string();

        wasm_bindgen_futures::spawn_local(async move {
            // Tag every result with this submission's epoch so the polling
            // system discards results from a superseded submission (e.g. after
            // the player restarted before the chain completed).
            let set_submit = move |result: Result<SubmissionRanks, String>| {
                SUBMIT_RESULTS.with(|s| s.borrow_mut().push((epoch, result)));
            };

            // 1. Turnstile token.
            let turnstile_token = match js_turnstile(&site_key).await {
                Ok(t) => t,
                Err(e) => {
                    set_submit(Err(format!("TURNSTILE [browser]: {e}; retry.")));
                    return;
                }
            };

            // 2. POST /v1/session.
            let session_body = serde_json::to_string(&SessionBody {
                condition: snapshot.condition,
                turnstile_token,
            })
            .unwrap_or_default();

            let (status, body) =
                match fetch_json(&session_url, "POST", Some(&session_body), None).await {
                    Ok(r) => r,
                    Err(e) => {
                        set_submit(Err(format!("NETWORK [session]: {e}; retry.")));
                        return;
                    }
                };

            if status != 200 {
                let (code, msg) = parse_error(&body).unzip();
                set_submit(Err(friendly_error(
                    code.as_deref(),
                    msg.as_deref(),
                    &format!("HTTP {status}"),
                )));
                return;
            }

            let session: SessionResponse = match serde_json::from_str(&body) {
                Ok(s) => s,
                Err(e) => {
                    set_submit(Err(format!(
                        "SERVER [invalid_session_response]: {e}; retry."
                    )));
                    return;
                }
            };

            // 3. Build canonical bytes and compute HMAC via WebCrypto.
            let canonical_input = CanonicalScoreInput {
                session_id: session.session_id.clone(),
                proof: session.proof.clone(),
                name: initials.clone(),
                condition: snapshot.condition,
                terminal_total: snapshot.terminal_total,
                chickens: snapshot.chickens,
                coins: snapshot.coins,
                objective_completed: snapshot.objective_completed,
                max_combo: snapshot.max_combo,
                round_duration_ms: snapshot.round_duration_ms,
                time_left_ms: snapshot.time_left_ms,
                game_over_reason: snapshot
                    .game_over_reason
                    .clone()
                    .expect("submission requires an eligible terminal reason"),
                build: snapshot.build.clone(),
                platform: snapshot.platform.clone(),
            };
            let canonical_str =
                String::from_utf8(canonical_score_bytes(&canonical_input)).unwrap_or_default();

            let signature = match js_hmac(&client_key, &canonical_str).await {
                Ok(s) => s,
                Err(e) => {
                    set_submit(Err(format!("CLIENT [signature]: {e}; retry.")));
                    return;
                }
            };

            // 4. POST /v1/scores with the signature header.
            let score_body = match serde_json::to_string(&ScoreBody {
                session_id: session.session_id,
                proof: session.proof,
                name: initials,
                condition: snapshot.condition,
                terminal_total: snapshot.terminal_total,
                chickens: snapshot.chickens,
                coins: snapshot.coins,
                objective_completed: snapshot.objective_completed,
                max_combo: snapshot.max_combo,
                round_duration_ms: snapshot.round_duration_ms,
                time_left_ms: snapshot.time_left_ms,
                game_over_reason: snapshot
                    .game_over_reason
                    .expect("submission requires an eligible terminal reason"),
                build: snapshot.build,
                platform: snapshot.platform,
            }) {
                Ok(body) => body,
                Err(e) => {
                    set_submit(Err(format!("CLIENT [score_payload]: {e}; retry.")));
                    return;
                }
            };

            let (status, body) =
                match fetch_json(&scores_url, "POST", Some(&score_body), Some(&signature)).await {
                    Ok(r) => r,
                    Err(e) => {
                        set_submit(Err(format!("NETWORK [score]: {e}; retry.")));
                        return;
                    }
                };

            if status != 201 {
                let (code, msg) = parse_error(&body).unzip();
                set_submit(Err(friendly_error(
                    code.as_deref(),
                    msg.as_deref(),
                    &format!("HTTP {status}"),
                )));
                return;
            }

            let resp: SubmitResponse = match serde_json::from_str(&body) {
                Ok(s) => s,
                Err(e) => {
                    set_submit(Err(format!("SERVER [invalid_score_response]: {e}; retry.")));
                    return;
                }
            };

            set_submit(Ok(SubmissionRanks {
                global: resp.global_rank,
                condition: resp.rank,
            }));
        });
    }
}

// ─── Platform-agnostic wrappers ──────────────────────────────────────────────

#[cfg(target_arch = "wasm32")]
fn start_board_fetch(epoch: u64) {
    web_bridge::fetch_board(epoch);
}

#[cfg(not(target_arch = "wasm32"))]
fn start_board_fetch(_epoch: u64) {}

#[cfg(target_arch = "wasm32")]
fn start_submission(epoch: u64, snapshot: &ScoreSnapshot, initials: &str) {
    web_bridge::start_submission(epoch, snapshot.clone(), initials.to_string());
}

#[cfg(not(target_arch = "wasm32"))]
fn start_submission(_epoch: u64, _snapshot: &ScoreSnapshot, _initials: &str) {}

#[cfg(target_arch = "wasm32")]
fn clear_board_result() {
    web_bridge::clear_board();
}

#[cfg(not(target_arch = "wasm32"))]
fn clear_board_result() {}

#[cfg(target_arch = "wasm32")]
fn clear_submit_result() {
    web_bridge::clear_submit();
}

#[cfg(not(target_arch = "wasm32"))]
fn clear_submit_result() {}

#[cfg(target_arch = "wasm32")]
fn cancel_turnstile_requests() {
    web_bridge::cancel_turnstile_requests();
}

#[cfg(not(target_arch = "wasm32"))]
fn cancel_turnstile_requests() {}

#[cfg(target_arch = "wasm32")]
fn poll_board_result() -> Option<(u64, Result<Vec<BoardEntry>, String>)> {
    web_bridge::take_board()
}

#[cfg(not(target_arch = "wasm32"))]
fn poll_board_result() -> Option<(u64, Result<Vec<BoardEntry>, String>)> {
    None
}

#[cfg(target_arch = "wasm32")]
fn poll_submit_result() -> Option<(u64, Result<SubmissionRanks, String>)> {
    web_bridge::take_submit()
}

#[cfg(not(target_arch = "wasm32"))]
fn poll_submit_result() -> Option<(u64, Result<SubmissionRanks, String>)> {
    None
}

// ─── Keyboard key mappings ───────────────────────────────────────────────────

const LETTER_KEYS: [(KeyCode, char); 26] = [
    (KeyCode::KeyA, 'A'),
    (KeyCode::KeyB, 'B'),
    (KeyCode::KeyC, 'C'),
    (KeyCode::KeyD, 'D'),
    (KeyCode::KeyE, 'E'),
    (KeyCode::KeyF, 'F'),
    (KeyCode::KeyG, 'G'),
    (KeyCode::KeyH, 'H'),
    (KeyCode::KeyI, 'I'),
    (KeyCode::KeyJ, 'J'),
    (KeyCode::KeyK, 'K'),
    (KeyCode::KeyL, 'L'),
    (KeyCode::KeyM, 'M'),
    (KeyCode::KeyN, 'N'),
    (KeyCode::KeyO, 'O'),
    (KeyCode::KeyP, 'P'),
    (KeyCode::KeyQ, 'Q'),
    (KeyCode::KeyR, 'R'),
    (KeyCode::KeyS, 'S'),
    (KeyCode::KeyT, 'T'),
    (KeyCode::KeyU, 'U'),
    (KeyCode::KeyV, 'V'),
    (KeyCode::KeyW, 'W'),
    (KeyCode::KeyX, 'X'),
    (KeyCode::KeyY, 'Y'),
    (KeyCode::KeyZ, 'Z'),
];

const DIGIT_KEYS: [(KeyCode, char); 10] = [
    (KeyCode::Digit0, '0'),
    (KeyCode::Digit1, '1'),
    (KeyCode::Digit2, '2'),
    (KeyCode::Digit3, '3'),
    (KeyCode::Digit4, '4'),
    (KeyCode::Digit5, '5'),
    (KeyCode::Digit6, '6'),
    (KeyCode::Digit7, '7'),
    (KeyCode::Digit8, '8'),
    (KeyCode::Digit9, '9'),
];

/// Keys that `gameover_input` in `game/mod.rs` reacts to. These are cleared
/// while the initials UI owns the keyboard.
const GAMEOVER_KEYS: [KeyCode; 5] = [
    KeyCode::Enter,
    KeyCode::Space,
    KeyCode::KeyR,
    KeyCode::Escape,
    KeyCode::KeyQ,
];

// ─── Bevy systems ────────────────────────────────────────────────────────────

#[derive(Component, Clone, Copy, Debug, PartialEq, Eq)]
enum BoardPlacement {
    Menu,
    Paused,
}

/// What [`begin_board_fetch`] should do given the current board state and
/// platform. Extracted as a pure function so the "always refresh cached
/// unless already fetching" policy is unit-testable without a network.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum BoardFetchAction {
    /// Native or unconfigured: mark the board unavailable, retain any cache.
    Unavailable,
    /// A fetch is already in flight; do not start a redundant one.
    KeepInFlight,
    /// Start a fresh fetch, retaining cached entries for "Refreshing" display.
    RefreshFromCache,
}

fn board_fetch_action(status: &BoardStatus, enabled: bool, is_web: bool) -> BoardFetchAction {
    if !enabled || !is_web {
        BoardFetchAction::Unavailable
    } else if *status == BoardStatus::Fetching {
        BoardFetchAction::KeepInFlight
    } else {
        BoardFetchAction::RefreshFromCache
    }
}

fn begin_board_fetch(board: &mut LeaderboardBoard) {
    match board_fetch_action(
        &board.status,
        leaderboard_enabled(),
        cfg!(target_arch = "wasm32"),
    ) {
        BoardFetchAction::Unavailable => {
            // Native or unconfigured: retain any successful in-memory cache
            // while clearly marking live reads unavailable.
            board.status = BoardStatus::Unavailable;
        }
        BoardFetchAction::KeepInFlight => {}
        BoardFetchAction::RefreshFromCache => {
            // Always refresh on lifecycle entry rather than silently reusing a
            // short-lived session cache. Cached entries are retained and shown
            // as "Refreshing - cached" while the fetch is in flight; rapid
            // Menu/Playing/Pause transitions that arrive before the fetch
            // completes leave the in-flight request running (guarded by
            // epoch).
            board.entries = board.cached_entries.clone();
            board.status = BoardStatus::Fetching;
            board.fetch_epoch = board.fetch_epoch.wrapping_add(1).max(1);
            clear_board_result();
            start_board_fetch(board.fetch_epoch);
        }
    }
}

/// Force a fresh board fetch after a successful score submission. Unlike
/// [`begin_board_fetch`], this starts a new request even when one is already
/// in flight, because the in-flight fetch predates the submitted score and
/// would return stale rankings.
fn refresh_board_after_submit(board: &mut LeaderboardBoard) {
    if !leaderboard_enabled() || !cfg!(target_arch = "wasm32") {
        board.status = BoardStatus::Unavailable;
    } else {
        board.entries = board.cached_entries.clone();
        board.status = BoardStatus::Fetching;
        board.fetch_epoch = board.fetch_epoch.wrapping_add(1).max(1);
        clear_board_result();
        start_board_fetch(board.fetch_epoch);
    }
}

#[cfg(test)]
fn pause_board_bounds(width: f32, height: f32) -> UiBounds {
    UiBounds {
        left: 14.0,
        top: 12.0,
        width: 560.0_f32.min(width * 0.55),
        // Title + up to five paired score rows, including panel padding.
        height: 96.0_f32.min((height - 12.0).max(0.0)),
    }
}

fn menu_board_visible(width: f32, height: f32) -> bool {
    !is_mobile_viewport(width, height)
}

fn on_menu_enter(
    mut commands: Commands,
    windows: Query<&Window, With<PrimaryWindow>>,
    mut board: ResMut<LeaderboardBoard>,
) {
    let desktop = windows
        .single()
        .map(|window| menu_board_visible(window.width(), window.height()))
        .unwrap_or(true);
    if desktop {
        spawn_board_ui(&mut commands, BoardPlacement::Menu);
    }
    begin_board_fetch(&mut board);
}

fn on_paused_enter(mut commands: Commands, mut board: ResMut<LeaderboardBoard>) {
    spawn_board_ui(&mut commands, BoardPlacement::Paused);
    begin_board_fetch(&mut board);
}

fn spawn_board_ui(commands: &mut Commands, placement: BoardPlacement) {
    let (top, bottom, left, right, width) = match placement {
        // The Menu board remains bottom-right, clear of its centered start UI
        // and the top-right Settings opener.
        BoardPlacement::Menu => (Val::Auto, px(12.0), Val::Auto, px(14.0), px(300.0)),
        // Pause board occupies the top strip and uses compact paired rows.
        // Even when width is clamped on a narrow viewport, it stays above the
        // centered pause controls and left of the top-right Settings opener.
        BoardPlacement::Paused => (px(12.0), Val::Auto, px(14.0), Val::Auto, px(560.0)),
    };
    commands
        .spawn((
            Node {
                position_type: PositionType::Absolute,
                top,
                bottom,
                left,
                right,
                width,
                max_width: Val::Percent(if placement == BoardPlacement::Paused {
                    55.0
                } else {
                    92.0
                }),
                max_height: Val::Percent(94.0),
                padding: UiRect::all(if placement == BoardPlacement::Paused {
                    px(7.0)
                } else {
                    px(10.0)
                }),
                flex_direction: FlexDirection::Column,
                overflow: Overflow::clip(),
                ..default()
            },
            BackgroundColor(Color::srgba(0.0, 0.0, 0.0, 0.68)),
            GlobalZIndex(50),
            LeaderboardBoardRoot,
            placement,
        ))
        .with_children(|p| {
            p.spawn((
                Text::new("GLOBAL LEADERBOARD"),
                TextFont {
                    font_size: FontSize::Px(if placement == BoardPlacement::Paused {
                        14.0
                    } else {
                        16.0
                    }),
                    ..default()
                },
                TextColor(palette::HUD_ACCENT.into()),
                Node {
                    margin: UiRect::bottom(px(4.0)),
                    ..default()
                },
                LeaderboardBoardTitle,
                placement,
            ));
            p.spawn((
                Text::new("Loading..."),
                TextFont {
                    font_size: FontSize::Px(if placement == BoardPlacement::Paused {
                        11.0
                    } else {
                        13.0
                    }),
                    ..default()
                },
                TextColor(palette::HUD_TEXT.into()),
                LeaderboardBoardText,
                placement,
            ));
        });
}

fn update_menu_board_visibility(
    mut commands: Commands,
    windows: Query<&Window, With<PrimaryWindow>>,
    roots: Query<(Entity, &BoardPlacement), With<LeaderboardBoardRoot>>,
) {
    let Ok(window) = windows.single() else {
        return;
    };
    let should_exist = menu_board_visible(window.width(), window.height());
    let menu_roots = roots
        .iter()
        .filter(|(_, placement)| **placement == BoardPlacement::Menu)
        .map(|(entity, _)| entity)
        .collect::<Vec<_>>();
    if should_exist && menu_roots.is_empty() {
        spawn_board_ui(&mut commands, BoardPlacement::Menu);
    } else if !should_exist {
        for entity in menu_roots {
            commands.entity(entity).despawn();
        }
    }
}

fn on_board_exit(mut commands: Commands, q: Query<Entity, With<LeaderboardBoardRoot>>) {
    for e in &q {
        commands.entity(e).despawn();
    }
    // Do not clear the shared async slot here: when Menu -> Playing -> Paused
    // happens quickly, the Pause OnEnter fetch is newer than Menu OnExit in
    // the transition schedule. Epoch validation safely ignores genuinely old
    // results without allowing the exit hook to erase that fresh request.
}

fn update_board(
    mut board: ResMut<LeaderboardBoard>,
    mut title_q: Query<
        (&mut Text, &BoardPlacement),
        (With<LeaderboardBoardTitle>, Without<LeaderboardBoardText>),
    >,
    mut text_q: Query<
        (&mut Text, &BoardPlacement),
        (With<LeaderboardBoardText>, Without<LeaderboardBoardTitle>),
    >,
) {
    // Poll for async fetch result (web only; native always returns None).
    // Drain stale queued epochs until this lifecycle's result is found.
    while let Some((epoch, result)) = poll_board_result() {
        if epoch == board.fetch_epoch {
            match result {
                Ok(entries) => {
                    board.cached_entries = entries.clone();
                    board.entries = entries;
                    board.status = BoardStatus::Fetched;
                }
                Err(msg) => {
                    board.status = BoardStatus::Error(msg);
                }
            }
            break;
        }
    }

    // Update title.
    for (mut text, placement) in &mut title_q {
        **text = match (&board.status, placement) {
            (BoardStatus::Unavailable, BoardPlacement::Paused) => "GLOBAL BOARD (unavailable)",
            (BoardStatus::Error(_), BoardPlacement::Paused) => "GLOBAL BOARD (offline)",
            (_, BoardPlacement::Paused) => "GLOBAL BOARD",
            (BoardStatus::Unavailable, BoardPlacement::Menu) => "GLOBAL LEADERBOARD (unavailable)",
            (BoardStatus::Error(_), BoardPlacement::Menu) => "GLOBAL LEADERBOARD (offline)",
            (_, BoardPlacement::Menu) => "GLOBAL LEADERBOARD",
        }
        .to_string();
    }

    // Pause uses compact two-column rows so all BOARD_LIMIT entries stay in
    // its left rail without intruding into the central pause controls.
    for (mut text, placement) in &mut text_q {
        **text = match placement {
            BoardPlacement::Menu => format_board_text(&board),
            BoardPlacement::Paused => format_pause_board_text(&board),
        };
    }
}

fn format_board_text(board: &LeaderboardBoard) -> String {
    match &board.status {
        BoardStatus::Idle => cached_board_text(&board.cached_entries, "Cached")
            .unwrap_or_else(|| "Loading...".to_string()),
        BoardStatus::Fetching => cached_board_text(&board.cached_entries, "Refreshing - cached")
            .unwrap_or_else(|| "Loading...".to_string()),
        BoardStatus::Fetched => {
            if board.entries.is_empty() {
                "No scores yet".to_string()
            } else {
                board
                    .entries
                    .iter()
                    .take(BOARD_LIMIT as usize)
                    .map(|e| format!("#{} {} {}", e.rank, e.name, e.score))
                    .collect::<Vec<_>>()
                    .join("\n")
            }
        }
        BoardStatus::Error(_) => cached_board_text(&board.cached_entries, "Offline (cached)")
            .unwrap_or_else(|| "Offline - no cached data".to_string()),
        BoardStatus::Unavailable => {
            cached_board_text(&board.cached_entries, "Unavailable (cached)")
                .unwrap_or_else(|| "Online leaderboard\nunavailable".to_string())
        }
    }
}

fn cached_board_text(entries: &[BoardEntry], label: &str) -> Option<String> {
    if entries.is_empty() {
        return None;
    }
    let rows = entries
        .iter()
        .take(BOARD_LIMIT as usize)
        .map(|e| format!("#{} {} {}", e.rank, e.name, e.score))
        .collect::<Vec<_>>()
        .join("\n");
    Some(format!("{label}\n{rows}"))
}

fn format_pause_board_text(board: &LeaderboardBoard) -> String {
    let (label, entries): (Option<&str>, &[BoardEntry]) = match &board.status {
        BoardStatus::Fetched => (None, &board.entries),
        BoardStatus::Fetching if !board.cached_entries.is_empty() => {
            (Some("Refreshing - cached"), &board.cached_entries)
        }
        BoardStatus::Error(_) if !board.cached_entries.is_empty() => {
            (Some("Offline (cached)"), &board.cached_entries)
        }
        BoardStatus::Unavailable if !board.cached_entries.is_empty() => {
            (Some("Unavailable (cached)"), &board.cached_entries)
        }
        _ => return format_board_text(board),
    };
    if entries.is_empty() {
        return "No scores yet".to_string();
    }

    let rows = entries
        .iter()
        .take(BOARD_LIMIT as usize)
        .map(|entry| format!("#{} {} {}", entry.rank, entry.name, entry.score))
        .collect::<Vec<_>>();
    let mut lines = Vec::new();
    if let Some(label) = label {
        lines.push(label.to_string());
    }
    for pair in rows.chunks(2) {
        let right = pair.get(1).map(String::as_str).unwrap_or("");
        lines.push(format!("{:<20}{right}", pair[0]));
    }
    lines.join("\n")
}

fn on_gameover_enter(
    mut commands: Commands,
    score: Res<Score>,
    reason: Res<GameOverReason>,
    active_modifier: Res<ActiveModifier>,
    objective: Res<ActiveObjective>,
    time_left: Res<TimeLeft>,
    peak_combo: Res<PeakCombo>,
    elapsed: Res<RoundElapsedMs>,
    settings: Res<Settings>,
    active_v3_product: Option<Res<ActiveRunRules>>,
    mut submission: ResMut<LeaderboardSubmission>,
) {
    let total = score.chickens + score.coins;
    submission.snapshot = ScoreSnapshot {
        condition: active_modifier.0.index() as u8,
        terminal_total: total,
        chickens: score.chickens,
        coins: score.coins,
        objective_completed: objective.completed,
        max_combo: peak_combo.0.max(1).min(5),
        round_duration_ms: elapsed.0,
        time_left_ms: (time_left.0.max(0.0) * 1000.0) as u64,
        game_over_reason: game_over_reason_str(*reason).map(str::to_string),
        build: BUILD_VERSION.to_string(),
        platform: platform_str().to_string(),
    };
    submission.initials.clear();
    submission.ranks = None;
    submission.error = None;
    // Fresh nonzero epoch for this Game Over session so any in-flight result
    // from a previous round is treated as stale by the polling system.
    submission.submit_epoch = submission.submit_epoch.wrapping_add(1).max(1);

    clear_submit_result();
    // Every explicit v3 Ranked/Casual x conduct product is owned exclusively
    // by `competitive_v3`.  The frozen v1 auto-submit path remains available
    // to legacy harnesses where ActiveRunRules is absent, but must never
    // capture one of the four new products.
    if active_v3_product.is_some() {
        submission.state = SubmissionState::Unavailable;
        spawn_gameover_ui(&mut commands);
        return;
    }
    // Drowning is explicitly local/unranked in legacy v1. Do not build, sign,
    // or send a terminal payload, even with remembered-name auto-submit.
    if submission.snapshot.game_over_reason.is_none() {
        submission.state = SubmissionState::Unavailable;
        spawn_gameover_ui(&mut commands);
        return;
    }
    match submission_start_decision(submission_enabled(), &settings.leaderboard_initials) {
        SubmissionStartDecision::Unavailable => {
            submission.state = SubmissionState::Unavailable;
        }
        SubmissionStartDecision::AwaitOptIn => {
            submission.state = SubmissionState::Ready;
        }
        SubmissionStartDecision::AutoSubmit(name) => {
            submission.initials = name;
            submission.state = SubmissionState::Submitting;
            // Every call starts a fresh Turnstile → session → HMAC chain; no
            // one-time session or proof is reused across rounds.
            start_submission(
                submission.submit_epoch,
                &submission.snapshot,
                &submission.initials,
            );
        }
    }
    spawn_gameover_ui(&mut commands);
}

fn snapshot_condition_name(condition: u8) -> &'static str {
    match condition {
        0 => "Standard",
        1 => "Rush Hour",
        2 => "Chicken Frenzy",
        3 => "Stampede",
        4 => "Glass Cannon",
        _ => "Unknown",
    }
}

fn submission_context(snapshot: &ScoreSnapshot) -> String {
    format!(
        "SCORE {}  |  {}  |  {}",
        snapshot.terminal_total,
        snapshot_condition_name(snapshot.condition),
        if snapshot.objective_completed {
            "OBJECTIVE COMPLETE"
        } else {
            "OBJECTIVE INCOMPLETE"
        }
    )
}

fn spawn_gameover_ui(commands: &mut Commands) {
    commands
        .spawn((
            Node {
                position_type: PositionType::Absolute,
                bottom: px(12.0),
                left: px(0.0),
                width: Val::Percent(100.0),
                align_items: AlignItems::Center,
                justify_content: JustifyContent::Center,
                ..default()
            },
            GlobalZIndex(50),
            LeaderboardGameOverRoot,
        ))
        .with_children(|p| {
            p.spawn((
                Node {
                    max_width: px(560.0),
                    width: Val::Percent(92.0),
                    padding: UiRect::all(px(10.0)),
                    flex_direction: FlexDirection::Column,
                    ..default()
                },
                BackgroundColor(Color::srgba(0.0, 0.0, 0.0, 0.5)),
                LeaderboardGameOverPanel,
            ))
            .with_child((
                Text::new(""),
                TextFont {
                    font_size: FontSize::Px(15.0),
                    ..default()
                },
                TextColor(palette::HUD_TEXT.into()),
                Node::default(),
                LeaderboardGameOverText,
            ));
        });
}

/// Visible controls aligned exactly with [`grid_action_for_normalized`].
/// Spawned only while touch can edit/retry/skip. Character visibility and the
/// state-specific bottom actions are selected independently by pure helpers.
fn spawn_touch_initials_grid(commands: &mut Commands, state: SubmissionState) {
    let show_characters = grid_shows_characters(state);
    const CHARS: &[char] = &[
        'A', 'B', 'C', 'D', 'E', 'F', 'G', 'H', 'I', 'J', 'K', 'L', 'M', 'N', 'O', 'P', 'Q', 'R',
        'S', 'T', 'U', 'V', 'W', 'X', 'Y', 'Z', '0', '1', '2', '3', '4', '5', '6', '7', '8', '9',
    ];
    commands
        .spawn((
            Node {
                position_type: PositionType::Absolute,
                top: Val::Percent(0.0),
                right: Val::Percent(0.0),
                bottom: Val::Percent(0.0),
                left: Val::Percent(0.0),
                ..default()
            },
            GlobalZIndex(55),
            LeaderboardTouchGrid(show_characters),
        ))
        .with_children(|root| {
            if show_characters {
                // Keep consent details in the touch-only UI's free space above
                // the keypad so the bottom action buttons cannot obscure them.
                root.spawn((
                    Node {
                        position_type: PositionType::Absolute,
                        left: Val::Percent(MOBILE_CONSENT_REGION.left * 100.0),
                        top: Val::Percent(MOBILE_CONSENT_REGION.top * 100.0),
                        width: Val::Percent(MOBILE_CONSENT_REGION.width * 100.0),
                        height: Val::Percent(MOBILE_CONSENT_REGION.height * 100.0),
                        padding: UiRect::all(px(5.0)),
                        align_items: AlignItems::Center,
                        justify_content: JustifyContent::Center,
                        ..default()
                    },
                    BackgroundColor(Color::srgba(0.0, 0.0, 0.0, 0.82)),
                    Text::new(SUBMISSION_CONSENT_DISCLOSURE),
                    TextFont {
                        font_size: FontSize::Px(12.0),
                        ..default()
                    },
                    TextColor(palette::HUD_TEXT.into()),
                ));
                for (index, ch) in CHARS.iter().enumerate() {
                    let col = index % 6;
                    let row = index / 6;
                    root.spawn((
                        Node {
                            position_type: PositionType::Absolute,
                            left: Val::Percent(
                                (MOBILE_CHARACTERS_REGION.left
                                    + col as f32 * MOBILE_CHARACTERS_REGION.width / 6.0)
                                    * 100.0,
                            ),
                            top: Val::Percent(
                                (MOBILE_CHARACTERS_REGION.top
                                    + row as f32 * MOBILE_CHARACTERS_REGION.height / 6.0)
                                    * 100.0,
                            ),
                            width: Val::Percent(MOBILE_CHARACTERS_REGION.width / 6.0 * 100.0),
                            height: Val::Percent(MOBILE_CHARACTERS_REGION.height / 6.0 * 100.0),
                            align_items: AlignItems::Center,
                            justify_content: JustifyContent::Center,
                            border: UiRect::all(px(1.0)),
                            ..default()
                        },
                        BorderColor::all(Color::srgba(1.0, 0.78, 0.12, 0.65)),
                        BackgroundColor(Color::srgba(0.02, 0.02, 0.03, 0.78)),
                        Text::new(ch.to_string()),
                        TextFont {
                            font_size: FontSize::Px(16.0),
                            ..default()
                        },
                        TextColor(palette::HUD_TEXT.into()),
                    ));
                }
            }
            for &(label, action) in grid_bottom_controls(state) {
                let (left, width) = match action {
                    GridAction::Backspace => (5.0, 25.0),
                    GridAction::Submit => (35.0, 30.0),
                    GridAction::Skip => (70.0, 25.0),
                    GridAction::Char(_) => continue,
                };
                root.spawn((
                    Node {
                        position_type: PositionType::Absolute,
                        left: Val::Percent(left),
                        top: Val::Percent(MOBILE_ACTIONS_REGION.top * 100.0),
                        width: Val::Percent(width),
                        height: Val::Percent(MOBILE_ACTIONS_REGION.height * 100.0),
                        align_items: AlignItems::Center,
                        justify_content: JustifyContent::Center,
                        border: UiRect::all(px(1.0)),
                        ..default()
                    },
                    BorderColor::all(Color::srgba(1.0, 0.78, 0.12, 0.8)),
                    BackgroundColor(Color::srgba(0.02, 0.02, 0.03, 0.9)),
                    Text::new(label),
                    TextFont {
                        font_size: FontSize::Px(15.0),
                        ..default()
                    },
                    TextColor(palette::HUD_ACCENT.into()),
                ));
            }
        });
}

fn on_gameover_exit(
    mut commands: Commands,
    q: Query<Entity, Or<(With<LeaderboardGameOverRoot>, With<LeaderboardTouchGrid>)>>,
    mut submission: ResMut<LeaderboardSubmission>,
) {
    for e in &q {
        commands.entity(e).despawn();
    }
    // Bound the JS challenge lifecycle to Game Over. This must happen before
    // clearing the result queue/state so the challenge Promise is settled and
    // its temporary widget/container are removed immediately.
    cancel_turnstile_requests();
    submission.state = SubmissionState::Idle;
    submission.initials.clear();
    submission.ranks = None;
    submission.error = None;
    // Drop any pending async result so it can't be polled after leaving Game
    // Over (backstop alongside the epoch tag).
    clear_submit_result();
}

#[allow(clippy::too_many_arguments)]
fn update_gameover_submission(
    mut commands: Commands,
    windows: Query<&Window, With<PrimaryWindow>>,
    mut submission: ResMut<LeaderboardSubmission>,
    mut board: ResMut<LeaderboardBoard>,
    touch_active: Res<TouchControlsActive>,
    touch_grid_q: Query<(Entity, &LeaderboardTouchGrid)>,
    mut root_q: Query<&mut Node, With<LeaderboardGameOverRoot>>,
    mut panel_q: Query<
        (&mut Node, &mut BackgroundColor),
        (
            With<LeaderboardGameOverPanel>,
            Without<LeaderboardGameOverRoot>,
        ),
    >,
    mut core_q: Query<&mut Visibility, With<GameOverCoreRoot>>,
    mut text_q: Query<
        (&mut Text, &mut Node),
        (
            With<LeaderboardGameOverText>,
            Without<LeaderboardGameOverRoot>,
            Without<LeaderboardGameOverPanel>,
        ),
    >,
) {
    // Poll for async submission results (web only; native returns None).
    // Drain stale epochs until this submission's result is found (e.g. a chain
    // that completed after the player restarted or retried).
    while let Some((epoch, result)) = poll_submit_result() {
        if epoch == submission.submit_epoch {
            match result {
                Ok(ranks) => {
                    submission.ranks = Some(ranks);
                    submission.state = transition_on_success(submission.state);
                    // Refresh the shared board so the new ranking appears on
                    // the next Menu/Pause visit. A fresh fetch is forced even
                    // if one is in flight, because it predates this score.
                    refresh_board_after_submit(&mut board);
                }
                Err(msg) => {
                    submission.error = Some(msg);
                    submission.state = transition_on_error(submission.state);
                }
            }
            break;
        }
    }

    let (width, height) = windows
        .single()
        .map(|window| (window.width(), window.height()))
        .unwrap_or((1440.0, 900.0));
    let mobile = is_mobile_viewport(width, height);
    let modal = mobile && interactive_modal(submission.state);
    let bounds = gameover_status_bounds(width, height, submission.state);
    for mut root in &mut root_q {
        if mobile {
            root.top = px(bounds.top);
            root.bottom = Val::Auto;
            root.left = px(bounds.left);
            root.width = Val::Percent(100.0);
            root.height = px(bounds.height);
            root.align_items = AlignItems::Center;
        } else {
            root.top = Val::Auto;
            root.bottom = px(12.0);
            root.left = px(0.0);
            root.width = Val::Percent(100.0);
            root.height = Val::Auto;
        }
    }
    for (mut panel, mut background) in &mut panel_q {
        if mobile {
            panel.width = Val::Percent(100.0);
            panel.max_width = Val::Percent(100.0);
            panel.height = Val::Percent(100.0);
            panel.padding = if modal {
                UiRect::all(px(0.0))
            } else {
                UiRect::axes(px(10.0), px(4.0))
            };
            panel.align_items = AlignItems::Center;
            panel.justify_content = JustifyContent::Center;
            background.0 = if modal {
                Color::srgba(0.0, 0.0, 0.0, 0.94)
            } else {
                Color::srgba(0.0, 0.0, 0.0, 0.78)
            };
        } else {
            panel.max_width = px(560.0);
            panel.width = Val::Percent(92.0);
            panel.height = Val::Auto;
            panel.padding = UiRect::all(px(10.0));
            panel.align_items = default();
            panel.justify_content = default();
            background.0 = Color::srgba(0.0, 0.0, 0.0, 0.5);
        }
    }
    let visible = normal_gameover_core_visible(submission.state, mobile);
    for mut visibility in &mut core_q {
        *visibility = if visible {
            Visibility::Inherited
        } else {
            Visibility::Hidden
        };
    }

    // Touch initials uses the character grid; the interactive mobile failure
    // modal keeps only retry/skip actions. Ready itself is the compact strip.
    let needs_touch_grid = touch_active.0
        && (submission.state == SubmissionState::EnteringInitials
            || (mobile && submission.state == SubmissionState::Failed));
    let show_characters = grid_shows_characters(submission.state);
    let grid_matches = touch_grid_q
        .iter()
        .any(|(_, grid)| grid.0 == show_characters);
    if needs_touch_grid && !grid_matches {
        for (entity, _) in &touch_grid_q {
            commands.entity(entity).despawn();
        }
        spawn_touch_initials_grid(&mut commands, submission.state);
    } else if !needs_touch_grid {
        for (entity, _) in &touch_grid_q {
            commands.entity(entity).despawn();
        }
    }

    // A touch entry grid may also be shown on desktop. Only the mobile modal
    // needs the reserved absolute header; desktop keeps its established text.
    let touch_modal = modal && touch_active.0;
    let mut text = if touch_modal {
        mobile_touch_modal_header_text(
            submission.state,
            &submission.snapshot,
            &submission.initials,
            submission.error.as_deref(),
        )
    } else {
        format_submission_text(
            submission.state,
            &submission.initials,
            submission.ranks,
            submission.error.as_deref(),
            touch_active.0,
        )
    };
    if modal && !touch_modal {
        text = format!("{}\n{}", submission_context(&submission.snapshot), text);
    } else if mobile && !modal {
        text = mobile_status_text(
            submission.state,
            &submission.initials,
            submission.ranks,
            touch_active.0,
        );
    }
    for (mut t, mut node) in &mut text_q {
        **t = text.clone();
        if touch_modal {
            let region = mobile_modal_header_region(submission.state);
            node.position_type = PositionType::Absolute;
            node.left = Val::Percent(region.left * 100.0);
            node.top = Val::Percent(region.top * 100.0);
            node.width = Val::Percent(region.width * 100.0);
            node.height = Val::Percent(region.height * 100.0);
            node.align_items = AlignItems::Center;
            node.justify_content = JustifyContent::Center;
        } else {
            node.position_type = PositionType::Relative;
            node.left = Val::Auto;
            node.top = Val::Auto;
            node.width = Val::Auto;
            node.height = Val::Auto;
            node.align_items = default();
            node.justify_content = default();
        }
    }
}

fn mobile_modal_header_region(state: SubmissionState) -> ModalRegion {
    if state == SubmissionState::Failed {
        MOBILE_FAILED_HEADER_REGION
    } else {
        MOBILE_INITIALS_HEADER_REGION
    }
}

/// Touch modals render only context/initials or failure copy in the reserved
/// header. Consent is owned exclusively by the grid's consent node, avoiding
/// duplicate disclosure/instructions behind the character cells.
fn mobile_touch_modal_header_text(
    state: SubmissionState,
    snapshot: &ScoreSnapshot,
    initials: &str,
    error: Option<&str>,
) -> String {
    match state {
        SubmissionState::EnteringInitials => format!(
            "SUBMIT TO LEADERBOARD\n{}\nINITIALS: [{}]",
            submission_context(snapshot),
            format_initials_display(initials)
        ),
        SubmissionState::Failed => format!(
            "SUBMISSION FAILED\n{}\n{}\nSaved name remains active; clear it in Settings to stop future auto-submit.",
            submission_context(snapshot),
            error.unwrap_or("Unknown error")
        ),
        _ => String::new(),
    }
}

fn mobile_status_text(
    state: SubmissionState,
    initials: &str,
    ranks: Option<SubmissionRanks>,
    touch_active: bool,
) -> String {
    match state {
        SubmissionState::Ready => {
            if touch_active {
                "LEADERBOARD: tap bottom-center to submit score".to_string()
            } else {
                "LEADERBOARD: press L to submit score".to_string()
            }
        }
        SubmissionState::Submitting if !initials.is_empty() => {
            format!("LEADERBOARD: submitting as {initials}...")
        }
        SubmissionState::Submitting => "LEADERBOARD: submitting score...".to_string(),
        SubmissionState::Submitted => ranks
            .map(|rank| {
                format!(
                    "SUBMITTED  |  GLOBAL #{}  |  CONDITION #{}",
                    rank.global, rank.condition
                )
            })
            .unwrap_or_else(|| "SUBMITTED!".to_string()),
        SubmissionState::Unavailable => "Online leaderboard submission unavailable".to_string(),
        SubmissionState::Idle | SubmissionState::Skipped => String::new(),
        // Interactive states use the full-screen modal path instead.
        SubmissionState::EnteringInitials | SubmissionState::Failed => String::new(),
    }
}

#[allow(clippy::too_many_arguments)]
fn format_submission_text(
    state: SubmissionState,
    initials: &str,
    ranks: Option<SubmissionRanks>,
    error: Option<&str>,
    touch_active: bool,
) -> String {
    match state {
        SubmissionState::Idle => String::new(),
        SubmissionState::Ready => {
            let mut text = format!(
                "SUBMIT TO LEADERBOARD\n{SUBMISSION_CONSENT_DISCLOSURE}\nPress L to enter initials"
            );
            if touch_active {
                text.push_str(" | tap SUBMIT");
            }
            text
        }
        SubmissionState::EnteringInitials => {
            let display = format_initials_display(initials);
            let mut text = format!(
                "SUBMIT TO LEADERBOARD\nINITIALS: [{}]\n{SUBMISSION_CONSENT_DISCLOSURE}\nA-Z 0-9 type | BKSP delete | ENTER submit | ESC skip",
                display
            );
            if touch_active {
                text.push_str("\nTap the aligned keypad below");
            }
            text
        }
        SubmissionState::Submitting => {
            if initials.is_empty() {
                "Submitting score...".to_string()
            } else {
                format!("Submitting score as {initials}...")
            }
        }
        SubmissionState::Submitted => match ranks {
            Some(r) => format!(
                "SUBMITTED!  GLOBAL #{}  |  CONDITION #{}",
                r.global, r.condition
            ),
            None => "SUBMITTED!".to_string(),
        },
        SubmissionState::Failed => {
            let msg = error.unwrap_or("Unknown error");
            let mut text = format!(
                "SUBMISSION FAILED\n{msg}\nENTER/SPACE/R: edit initials, then submit again | ESC/Q: skip this round\nSaved name remains active; clear it in Settings to stop future auto-submit."
            );
            if touch_active {
                text.push_str(
                    "\nTouch: bottom-center edits initials; submit again after editing | bottom-right skips",
                );
            }
            text
        }
        SubmissionState::Skipped => String::new(),
        SubmissionState::Unavailable => "Online leaderboard submission unavailable".to_string(),
    }
}

/// Game Over input for the leaderboard initials/submission UI.
///
/// Runs after `TouchStateSet` and before `KeyboardStateSet` so it can consume
/// keys and cancel pending touch transitions before `gameover_input` sees
/// them. In the `Ready` opt-in state, `L` or a touch on the SUBMIT zone opens
/// the initials UI while normal restart/menu navigation proceeds otherwise.
/// While in `EnteringInitials` or `Failed`, regular restart/menu keys are
/// suspended.
#[allow(clippy::too_many_arguments)]
fn leaderboard_gameover_input(
    mut keys: ResMut<ButtonInput<KeyCode>>,
    touches: Res<Touches>,
    windows: Query<&Window, With<PrimaryWindow>>,
    mut submission: ResMut<LeaderboardSubmission>,
    mut settings: ResMut<Settings>,
    mut touch_active: ResMut<TouchControlsActive>,
    mut restart: ResMut<RestartRequested>,
    mut next_state: ResMut<NextState<GameState>>,
) {
    // Ready: opt-in prompt. `L` or a touch on the SUBMIT zone opens the
    // initials UI. All other input falls through to normal Game Over
    // navigation (restart/menu), so gameover keys are NOT cleared here and
    // only submit-zone touches have their pending touch transition cancelled.
    if submission.state == SubmissionState::Ready {
        let mut handled_keyboard = false;
        if keys.just_pressed(KeyCode::KeyL) {
            submission.state = transition_on_opt_in(submission.state);
            handled_keyboard = true;
        }
        let mut handled_touch = false;
        if let Ok(window) = windows.single() {
            for touch in touches.iter_just_pressed() {
                touch_active.0 = true;
                if let Some(GridAction::Submit) = touch_grid_action(touch.position(), window) {
                    submission.state = transition_on_opt_in(submission.state);
                    handled_touch = true;
                }
            }
        }
        if handled_keyboard {
            // L is not a normal Game Over action, but explicitly consume it so
            // it cannot leak to any later feature sharing the same key.
            keys.clear_just_pressed(KeyCode::KeyL);
        }
        if handled_touch {
            restart.0 = false;
            next_state.reset();
        }
        return;
    }

    // Remembered-name auto submission and its terminal result deliberately do
    // not own Game Over controls: restart/menu stays immediately available.
    // Failed is the one exception because it offers explicit retry/skip.
    if !input_suspended(submission.state) {
        return;
    }

    match submission.state {
        SubmissionState::EnteringInitials => {
            // --- Keyboard ---
            for (code, ch) in LETTER_KEYS {
                if keys.just_pressed(code) && submission.initials.len() < 5 {
                    submission.initials.push(ch);
                }
            }
            for (code, ch) in DIGIT_KEYS {
                if keys.just_pressed(code) && submission.initials.len() < 5 {
                    submission.initials.push(ch);
                }
            }
            if keys.just_pressed(KeyCode::Backspace) {
                submission.initials.pop();
            }
            if keys.just_pressed(KeyCode::Enter) {
                if let Some(normalized) = normalize_initials(&submission.initials) {
                    submission.initials = normalized;
                    if let Some(remembered) = remembered_initials_update(
                        &settings.leaderboard_initials,
                        &submission.initials,
                    ) {
                        settings.leaderboard_initials = remembered;
                    }
                    submission.state = transition_on_submit(submission.state);
                    submission.submit_epoch = submission.submit_epoch.wrapping_add(1).max(1);
                    start_submission(
                        submission.submit_epoch,
                        &submission.snapshot,
                        &submission.initials,
                    );
                }
            }
            if keys.just_pressed(KeyCode::Escape) {
                submission.state = transition_on_skip(submission.state);
            }

            // --- Touch grid ---
            let mut handled_touch = false;
            if let Ok(window) = windows.single() {
                for touch in touches.iter_just_pressed() {
                    touch_active.0 = true;
                    handled_touch = true;
                    let Some(action) = touch_grid_action(touch.position(), window) else {
                        continue;
                    };
                    match action {
                        GridAction::Char(c) => {
                            if submission.initials.len() < 5 {
                                submission.initials.push(c);
                            }
                        }
                        GridAction::Backspace => {
                            submission.initials.pop();
                        }
                        GridAction::Submit => {
                            // Keyboard submit may already have transitioned in
                            // this frame; never launch a second fresh chain.
                            if submission.state == SubmissionState::EnteringInitials {
                                if let Some(normalized) = normalize_initials(&submission.initials) {
                                    submission.initials = normalized;
                                    if let Some(remembered) = remembered_initials_update(
                                        &settings.leaderboard_initials,
                                        &submission.initials,
                                    ) {
                                        settings.leaderboard_initials = remembered;
                                    }
                                    submission.state = transition_on_submit(submission.state);
                                    submission.submit_epoch =
                                        submission.submit_epoch.wrapping_add(1).max(1);
                                    start_submission(
                                        submission.submit_epoch,
                                        &submission.snapshot,
                                        &submission.initials,
                                    );
                                }
                            }
                        }
                        GridAction::Skip => {
                            submission.state = transition_on_skip(submission.state);
                        }
                    }
                }
            }
            if handled_touch {
                restart.0 = false;
                next_state.reset();
            }
        }
        SubmissionState::Failed => {
            // ENTER / SPACE / R → retry; ESC / Q → skip.
            if keys.just_pressed(KeyCode::Enter)
                || keys.just_pressed(KeyCode::Space)
                || keys.just_pressed(KeyCode::KeyR)
            {
                submission.state = transition_on_retry(submission.state);
                submission.error = None;
            }
            if keys.just_pressed(KeyCode::Escape) || keys.just_pressed(KeyCode::KeyQ) {
                submission.state = transition_on_skip(submission.state);
            }

            // Touch: RETRY (the SUBMIT action zone) → retry, SKIP → skip.
            let mut handled_touch = false;
            if let Ok(window) = windows.single() {
                for touch in touches.iter_just_pressed() {
                    touch_active.0 = true;
                    handled_touch = true;
                    if let Some(action) = touch_grid_action(touch.position(), window) {
                        if let Some(next) = failed_grid_transition(action) {
                            submission.state = next;
                            if next == SubmissionState::EnteringInitials {
                                submission.error = None;
                            }
                        }
                    }
                }
            }
            if handled_touch {
                restart.0 = false;
                next_state.reset();
            }
        }
        _ => {}
    }

    // Clear the keys that `gameover_input` would react to, so typing or
    // retry/skip don't accidentally restart or return to menu.
    for key in GAMEOVER_KEYS {
        keys.clear_just_pressed(key);
    }
}

/// Convert a touch position to a grid action using the window's logical size.
fn touch_grid_action(position: Vec2, window: &Window) -> Option<GridAction> {
    let size = Vec2::new(window.width(), window.height());
    if size.x <= 0.0 || size.y <= 0.0 || !size.is_finite() || !position.is_finite() {
        return None;
    }
    let x = (position.x / size.x).clamp(0.0, 1.0);
    let y = (position.y / size.y).clamp(0.0, 1.0);
    grid_action_for_normalized(x, y)
}

// ─── Round tracking ──────────────────────────────────────────────────────────

/// Reset peak combo and elapsed time on a fresh round (skipped on pause
/// resume when `RoundActive` is still true).
fn reset_round_tracking(
    round_active: Res<RoundActive>,
    mut peak: ResMut<PeakCombo>,
    mut elapsed: ResMut<RoundElapsedMs>,
) {
    if round_active.0 {
        return;
    }
    peak.0 = 1;
    elapsed.0 = 0;
}

/// Accumulate elapsed time and track the peak combo multiplier during play.
///
/// Runs after `ComboUpdateSet` so the multiplier is final for the frame, and
/// is gated on `InputFrozen` so the round-elapsed clock doesn't advance during
/// the countdown (mirroring `tick_timeleft` / `tick_difficulty`). The combo is
/// 1 throughout the countdown, so skipping peak tracking then is harmless.
fn track_round_stats(
    combo: Res<Combo>,
    mut peak: ResMut<PeakCombo>,
    mut elapsed: ResMut<RoundElapsedMs>,
    time: Res<Time>,
    input_frozen: Res<InputFrozen>,
) {
    if input_frozen.0 {
        return;
    }
    elapsed.0 = elapsed
        .0
        .saturating_add((time.delta_secs() * 1000.0) as u64);
    if combo.multiplier > peak.0 {
        peak.0 = combo.multiplier;
    }
}

// ─── Plugin ──────────────────────────────────────────────────────────────────

pub struct LeaderboardPlugin;

impl Plugin for LeaderboardPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<LeaderboardBoard>()
            .init_resource::<LeaderboardSubmission>()
            .init_resource::<PeakCombo>()
            .init_resource::<RoundElapsedMs>()
            // Shared full global board: refresh independently on Menu/Pause
            // entry, retain successful data for graceful offline display, and
            // use mutually exclusive lifecycles so panels never overlap.
            .add_systems(OnEnter(GameState::Menu), on_menu_enter)
            .add_systems(OnExit(GameState::Menu), on_board_exit)
            .add_systems(OnEnter(GameState::Paused), on_paused_enter)
            .add_systems(OnExit(GameState::Paused), on_board_exit)
            .add_systems(
                Update,
                (
                    update_menu_board_visibility.run_if(in_state(GameState::Menu)),
                    update_board,
                )
                    .chain()
                    .run_if(in_state(GameState::Menu).or_else(in_state(GameState::Paused))),
            )
            // Game Over: snapshot, initials input, submission polling.
            .add_systems(OnEnter(GameState::GameOver), on_gameover_enter)
            .add_systems(OnExit(GameState::GameOver), on_gameover_exit)
            .add_systems(
                Update,
                update_gameover_submission.run_if(in_state(GameState::GameOver)),
            )
            .add_systems(
                Update,
                leaderboard_gameover_input
                    .after(TouchStateSet)
                    .before(KeyboardStateSet)
                    .run_if(in_state(GameState::GameOver))
                    .run_if(settings_closed),
            )
            // Round tracking for peak combo and elapsed time.
            .add_systems(
                OnEnter(GameState::Playing),
                reset_round_tracking.in_set(SpawnSet),
            )
            .add_systems(
                Update,
                track_round_stats
                    .after(ComboUpdateSet)
                    .run_if(in_state(GameState::Playing)),
            );
    }
}

// ─── Tests ───────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    // ── Initials normalization ──────────────────────────────────────────

    #[test]
    fn normalize_initials_accepts_valid_3_to_5_alphanumeric() {
        assert_eq!(normalize_initials("abc"), Some("ABC".to_string()));
        assert_eq!(normalize_initials("  abc  "), Some("ABC".to_string()));
        assert_eq!(normalize_initials("ABC"), Some("ABC".to_string()));
        assert_eq!(normalize_initials("A1B2C"), Some("A1B2C".to_string()));
        assert_eq!(normalize_initials("123"), Some("123".to_string()));
        assert_eq!(normalize_initials("ABCDE"), Some("ABCDE".to_string()));
        assert_eq!(normalize_initials("zzz"), Some("ZZZ".to_string()));
    }

    #[test]
    fn normalize_initials_rejects_wrong_length_and_invalid_chars() {
        assert_eq!(normalize_initials(""), None);
        assert_eq!(normalize_initials("AB"), None); // too short
        assert_eq!(normalize_initials("ABCDEF"), None); // too long
        assert_eq!(normalize_initials("AB-"), None); // dash
        assert_eq!(normalize_initials("AB!"), None); // punctuation
        assert_eq!(normalize_initials("A B"), None); // interior space (only ends trimmed)
        assert_eq!(normalize_initials("ab_"), None); // underscore
        assert_eq!(normalize_initials("ÅBC"), None); // non-ASCII
    }

    // ── Canonical serialization ──────────────────────────────────────────

    #[test]
    fn canonical_score_bytes_exact_with_all_fields() {
        let input = CanonicalScoreInput {
            session_id: "sess-aaa".to_string(),
            proof: "proof-bbb".to_string(),
            name: "AAA".to_string(),
            condition: 0,
            terminal_total: 42,
            chickens: 30,
            coins: 12,
            objective_completed: true,
            max_combo: 4,
            round_duration_ms: 65000,
            time_left_ms: 0,
            game_over_reason: "time_up".to_string(),
            build: "0.1.0".to_string(),
            platform: "web".to_string(),
        };
        let bytes = canonical_score_bytes(&input);
        let expected = "roady.v1.score\nsess-aaa\nproof-bbb\nAAA\n0\n42\n30\n12\n1\n4\n65000\n0\ntime_up\n0.1.0\nweb";
        assert_eq!(bytes.as_slice(), expected.as_bytes());
    }

    #[test]
    fn canonical_score_bytes_objective_false_emits_0() {
        let input = CanonicalScoreInput {
            session_id: "s".to_string(),
            proof: "p".to_string(),
            name: "ZZZ".to_string(),
            condition: 3,
            terminal_total: 5,
            chickens: 2,
            coins: 3,
            objective_completed: false,
            max_combo: 1,
            round_duration_ms: 30000,
            time_left_ms: 30000,
            game_over_reason: "wrecked".to_string(),
            build: "1.0.0".to_string(),
            platform: "native".to_string(),
        };
        let bytes = canonical_score_bytes(&input);
        let expected =
            "roady.v1.score\ns\np\nZZZ\n3\n5\n2\n3\n0\n1\n30000\n30000\nwrecked\n1.0.0\nnative";
        assert_eq!(bytes.as_slice(), expected.as_bytes());
    }

    #[test]
    fn canonical_score_bytes_no_trailing_lf() {
        let input = CanonicalScoreInput {
            session_id: "x".to_string(),
            proof: "y".to_string(),
            name: "Q".to_string(),
            condition: 1,
            terminal_total: 0,
            chickens: 0,
            coins: 0,
            objective_completed: false,
            max_combo: 1,
            round_duration_ms: 0,
            time_left_ms: 0,
            game_over_reason: "time_up".to_string(),
            build: "0".to_string(),
            platform: "web".to_string(),
        };
        let bytes = canonical_score_bytes(&input);
        assert!(!bytes.ends_with(b"\n"));
        // Exactly 15 fields → 14 separators.
        assert_eq!(bytes.iter().filter(|&&b| b == b'\n').count(), 14);
    }

    #[test]
    fn canonical_score_bytes_extended_duration_is_stable_and_exact() {
        let input = CanonicalScoreInput {
            session_id: "s".to_string(),
            proof: "p".to_string(),
            name: "AAA".to_string(),
            condition: 0,
            terminal_total: 1614,
            chickens: 1000,
            coins: 614,
            objective_completed: true,
            max_combo: 5,
            round_duration_ms: 161_400,
            time_left_ms: 0,
            game_over_reason: "time_up".to_string(),
            build: "1".to_string(),
            platform: "web".to_string(),
        };
        let first = canonical_score_bytes(&input);
        let second = canonical_score_bytes(&input);
        assert_eq!(first, second);
        assert!(std::str::from_utf8(&first).unwrap().contains("\n161400\n"));

        let mut changed = input;
        changed.round_duration_ms += 1;
        assert_ne!(first, canonical_score_bytes(&changed));
    }

    #[test]
    fn client_duration_contract_uses_soft_review_and_js_safe_hard_bound() {
        assert_eq!(MAX_ROUND_DURATION_MS, 1_800_000);
        assert_eq!(MAX_SAFE_INTEGER_MS, 9_007_199_254_740_991);
        assert!(valid_round_duration_ms(161_400));
        assert!(valid_round_duration_ms(MAX_ROUND_DURATION_MS));
        assert!(valid_round_duration_ms(MAX_ROUND_DURATION_MS + 1));
        assert!(valid_round_duration_ms(MAX_SAFE_INTEGER_MS));
        assert!(!valid_round_duration_ms(MAX_SAFE_INTEGER_MS + 1));
    }

    #[test]
    fn canonical_score_bytes_integers_are_canonical_base10() {
        let input = CanonicalScoreInput {
            session_id: "s".to_string(),
            proof: "p".to_string(),
            name: "AAA".to_string(),
            condition: 0,
            terminal_total: 007,
            chickens: 003,
            coins: 004,
            objective_completed: true,
            max_combo: 2,
            round_duration_ms: 60000,
            time_left_ms: 0,
            game_over_reason: "time_up".to_string(),
            build: "1".to_string(),
            platform: "web".to_string(),
        };
        let bytes = canonical_score_bytes(&input);
        let s = std::str::from_utf8(&bytes).unwrap();
        // Rust's Display for integers never emits leading zeros or plus signs.
        assert!(s.contains("\n7\n3\n4\n"));
        assert!(!s.contains("+"));
    }

    // ── Base64url ────────────────────────────────────────────────────────

    #[test]
    fn base64url_known_vectors() {
        assert_eq!(to_base64url(b""), "");
        assert_eq!(to_base64url(b"f"), "Zg");
        assert_eq!(to_base64url(b"fo"), "Zm8");
        assert_eq!(to_base64url(b"foo"), "Zm9v");
        assert_eq!(to_base64url(b"foob"), "Zm9vYg");
        assert_eq!(to_base64url(b"fooba"), "Zm9vYmE");
        assert_eq!(to_base64url(b"foobar"), "Zm9vYmFy");
        // No padding character.
        assert!(!to_base64url(b"foo").contains('='));
    }

    // ── State transitions ────────────────────────────────────────────────

    #[test]
    fn submission_state_submit_skip_and_terminal() {
        use SubmissionState::*;

        assert_eq!(transition_on_submit(EnteringInitials), Submitting);
        assert_eq!(transition_on_skip(EnteringInitials), Skipped);
        assert_eq!(transition_on_success(Submitting), Submitted);
        assert_eq!(transition_on_error(Submitting), Failed);
        assert_eq!(transition_on_retry(Failed), EnteringInitials);
        assert_eq!(transition_on_skip(Failed), Skipped);

        // Terminal states are sticky.
        assert_eq!(transition_on_submit(Submitted), Submitted);
        assert_eq!(transition_on_submit(Skipped), Skipped);
        assert_eq!(transition_on_success(Submitted), Submitted);
        assert_eq!(transition_on_error(Skipped), Skipped);
        assert_eq!(transition_on_submit(Unavailable), Unavailable);

        // Idle ignores unexpected events.
        assert_eq!(transition_on_success(Idle), Idle);
        assert_eq!(transition_on_error(Idle), Idle);

        // Ready opt-in opens initials; other states are unaffected.
        assert_eq!(transition_on_opt_in(Ready), EnteringInitials);
        assert_eq!(transition_on_opt_in(Idle), Idle);
        assert_eq!(transition_on_opt_in(Submitting), Submitting);
        assert_eq!(transition_on_opt_in(Skipped), Skipped);
    }

    #[test]
    fn shared_mobile_breakpoint_controls_menu_board() {
        assert!(is_mobile_viewport(844.0, 390.0));
        assert!(is_mobile_viewport(960.0, 480.0));
        assert!(!is_mobile_viewport(1440.0, 900.0));
        assert!(!menu_board_visible(844.0, 390.0));
        assert!(!menu_board_visible(960.0, 480.0));
        assert!(menu_board_visible(1440.0, 900.0));
    }

    #[test]
    fn mobile_pause_board_stays_disjoint_from_centered_pause_content() {
        let board = pause_board_bounds(844.0, 390.0);
        let pause = pause_content_bounds(844.0, 390.0);
        assert_eq!(board.top + board.height, 108.0);
        assert_eq!(pause.top, 133.0);
        assert!(board.is_disjoint(pause));
    }

    #[test]
    fn mobile_gameover_core_and_status_strip_are_disjoint() {
        let core = crate::ui::gameover_core_bounds(844.0, 390.0);
        let status = gameover_status_bounds(844.0, 390.0, SubmissionState::Ready);
        assert_eq!(core.height, 338.0);
        assert_eq!(status.top, 338.0);
        assert_eq!(status.height, GAMEOVER_STATUS_STRIP_HEIGHT);
        assert!(core.is_disjoint(status));
    }

    #[test]
    fn interactive_mobile_states_replace_core_and_terminal_states_restore_it() {
        for state in [SubmissionState::EnteringInitials, SubmissionState::Failed] {
            assert!(!normal_gameover_core_visible(state, true));
            assert_eq!(gameover_status_bounds(844.0, 390.0, state).height, 390.0);
        }
        for state in [
            SubmissionState::Ready,
            SubmissionState::Submitting,
            SubmissionState::Submitted,
            SubmissionState::Skipped,
            SubmissionState::Unavailable,
        ] {
            assert!(normal_gameover_core_visible(state, true), "{state:?}");
            assert_eq!(gameover_status_bounds(844.0, 390.0, state).height, 52.0);
        }
        // Desktop behavior is never replaced by the mobile modal policy.
        assert!(normal_gameover_core_visible(
            SubmissionState::EnteringInitials,
            false
        ));
    }

    #[test]
    fn resize_policy_switches_in_place_between_mobile_and_desktop() {
        let mobile_core = crate::ui::gameover_core_bounds(844.0, 390.0);
        let desktop_core = crate::ui::gameover_core_bounds(1440.0, 900.0);
        assert_eq!(mobile_core.height, 338.0);
        assert_eq!(desktop_core.height, 900.0);
        assert_eq!(
            gameover_status_bounds(844.0, 390.0, SubmissionState::Ready).height,
            52.0
        );
        assert_eq!(
            gameover_status_bounds(1440.0, 900.0, SubmissionState::Ready).height,
            0.0
        );
    }

    #[test]
    fn input_suspended_only_for_initials_and_failed() {
        assert!(input_suspended(SubmissionState::EnteringInitials));
        assert!(input_suspended(SubmissionState::Failed));
        assert!(!input_suspended(SubmissionState::Idle));
        assert!(!input_suspended(SubmissionState::Ready));
        assert!(!input_suspended(SubmissionState::Submitting));
        assert!(!input_suspended(SubmissionState::Submitted));
        assert!(!input_suspended(SubmissionState::Skipped));
        assert!(!input_suspended(SubmissionState::Unavailable));
    }

    #[test]
    fn remembered_name_controls_gameover_start_without_retrying() {
        assert_eq!(
            submission_start_decision(true, " abc "),
            SubmissionStartDecision::AutoSubmit("ABC".to_string())
        );
        assert_eq!(
            submission_start_decision(true, ""),
            SubmissionStartDecision::AwaitOptIn
        );
        assert_eq!(
            submission_start_decision(true, "BAD-NAME"),
            SubmissionStartDecision::AwaitOptIn
        );
        assert_eq!(
            submission_start_decision(true, "ÅBC"),
            SubmissionStartDecision::AwaitOptIn
        );
        assert_eq!(
            submission_start_decision(false, "ABC"),
            SubmissionStartDecision::Unavailable
        );
        // A failed automatic chain becomes Failed; only explicit retry input
        // can move it back to entry, so there is no automatic retry loop.
        assert_eq!(
            transition_on_error(SubmissionState::Submitting),
            SubmissionState::Failed
        );
        assert_eq!(
            transition_on_retry(SubmissionState::Failed),
            SubmissionState::EnteringInitials
        );
    }

    #[test]
    fn manual_name_persistence_requires_valid_changed_initials() {
        assert_eq!(
            remembered_initials_update("", " ab1 "),
            Some("AB1".to_string())
        );
        assert_eq!(remembered_initials_update("AB1", "AB1"), None);
        assert_eq!(remembered_initials_update(" ab1 ", "AB1"), None);
        assert_eq!(remembered_initials_update("OLD", "xy"), None);
        assert_eq!(remembered_initials_update("OLD", "X-Y"), None);
    }

    #[test]
    fn success_text_labels_global_and_condition_ranks() {
        let text = format_submission_text(
            SubmissionState::Submitted,
            "AAA",
            Some(SubmissionRanks {
                global: 17,
                condition: 4,
            }),
            None,
            false,
        );
        assert!(text.contains("GLOBAL #17"));
        assert!(text.contains("CONDITION #4"));
    }

    #[test]
    fn opt_in_ui_discloses_name_storage_auto_submit_and_revocation() {
        for state in [SubmissionState::Ready, SubmissionState::EnteringInitials] {
            let text = format_submission_text(state, "ABC", None, None, false);
            assert!(text.contains("stores this name"), "{state:?}: {text}");
            assert!(
                text.contains("auto-submits future completed rounds"),
                "{state:?}: {text}"
            );
            assert!(
                text.contains("Clear Leaderboard Name in Settings to revoke consent"),
                "{state:?}: {text}"
            );
        }
    }

    #[test]
    fn failure_text_explains_retry_and_saved_name_status() {
        let text = format_submission_text(
            SubmissionState::Failed,
            "ABC",
            None,
            Some("Network offline"),
            false,
        );
        assert!(text.contains("edit initials, then submit again"));
        assert!(text.contains("skip this round"));
        assert!(text.contains("Saved name remains active"));
        assert!(text.contains("clear it in Settings"));
    }

    // ── Touch grid mapping ───────────────────────────────────────────────

    #[test]
    fn mobile_modal_regions_are_structurally_disjoint() {
        let initials_regions = [
            MOBILE_INITIALS_HEADER_REGION,
            MOBILE_CONSENT_REGION,
            MOBILE_CHARACTERS_REGION,
            MOBILE_ACTIONS_REGION,
        ];
        for (index, region) in initials_regions.iter().enumerate() {
            assert!(region.left >= 0.0 && region.top >= 0.0);
            assert!(region.right() <= 1.0 && region.bottom() <= 1.0);
            for other in &initials_regions[index + 1..] {
                assert!(region.is_disjoint(*other), "{region:?} overlaps {other:?}");
            }
        }
        assert!(MOBILE_FAILED_HEADER_REGION.is_disjoint(MOBILE_ACTIONS_REGION));
        assert!((MOBILE_CONSENT_REGION.top - 0.20).abs() < 0.000_001);
        assert!((MOBILE_CONSENT_REGION.bottom() - 0.38).abs() < 0.000_001);
        assert!((MOBILE_CHARACTERS_REGION.top - 0.40).abs() < 0.000_001);
        assert!((MOBILE_CHARACTERS_REGION.bottom() - 0.85).abs() < 0.000_001);
        assert!((MOBILE_ACTIONS_REGION.top - 0.87).abs() < 0.000_001);
        assert!((MOBILE_ACTIONS_REGION.bottom() - 0.97).abs() < 0.000_001);
    }

    #[test]
    fn mobile_touch_headers_do_not_duplicate_grid_consent_or_instructions() {
        let snapshot = ScoreSnapshot {
            terminal_total: 42,
            condition: 1,
            objective_completed: true,
            ..default()
        };
        let entering = mobile_touch_modal_header_text(
            SubmissionState::EnteringInitials,
            &snapshot,
            "ABC",
            None,
        );
        assert!(entering.contains("SCORE 42"));
        assert!(entering.contains("INITIALS: [ABC__]"));
        assert!(!entering.contains("stores this name"));
        assert!(!entering.contains("Tap the aligned keypad"));

        let failed = mobile_touch_modal_header_text(
            SubmissionState::Failed,
            &snapshot,
            "ABC",
            Some("Offline"),
        );
        assert!(failed.contains("SUBMISSION FAILED"));
        assert!(failed.contains("Offline"));
        assert!(!failed.contains("bottom-center"));
        assert_eq!(
            mobile_modal_header_region(SubmissionState::Failed),
            MOBILE_FAILED_HEADER_REGION
        );
    }

    #[test]
    fn grid_maps_corners_and_center_to_correct_chars() {
        // Top-left cell and exact outer edge → 'A'.
        assert_eq!(
            grid_action_for_normalized(0.10, 0.42),
            Some(GridAction::Char('A'))
        );
        assert_eq!(
            grid_action_for_normalized(MOBILE_CHARACTERS_REGION.left, MOBILE_CHARACTERS_REGION.top,),
            Some(GridAction::Char('A'))
        );
        // Bottom-right cell and exact outer edge → '9'.
        assert_eq!(
            grid_action_for_normalized(0.90, 0.82),
            Some(GridAction::Char('9'))
        );
        assert_eq!(
            grid_action_for_normalized(
                MOBILE_CHARACTERS_REGION.right(),
                MOBILE_CHARACTERS_REGION.bottom(),
            ),
            Some(GridAction::Char('9'))
        );
        // Middle-ish → 'O' (row 2, col 2 → index 14 → 'O').
        assert_eq!(
            grid_action_for_normalized(0.38, 0.56),
            Some(GridAction::Char('O'))
        );
    }

    #[test]
    fn grid_maps_bottom_buttons() {
        assert_eq!(
            grid_action_for_normalized(0.15, 0.92),
            Some(GridAction::Backspace)
        );
        assert_eq!(
            grid_action_for_normalized(0.50, 0.92),
            Some(GridAction::Submit)
        );
        assert_eq!(
            grid_action_for_normalized(0.85, 0.92),
            Some(GridAction::Skip)
        );
    }

    #[test]
    fn bottom_controls_have_exact_state_specific_labels_and_actions() {
        assert_eq!(
            grid_bottom_controls(SubmissionState::EnteringInitials),
            &[
                ("BACK", GridAction::Backspace),
                ("SUBMIT", GridAction::Submit),
                ("SKIP", GridAction::Skip),
            ]
        );
        assert_eq!(
            grid_bottom_controls(SubmissionState::Failed),
            &[("RETRY", GridAction::Submit), ("SKIP", GridAction::Skip),]
        );
        assert!(grid_shows_characters(SubmissionState::EnteringInitials));
        assert!(!grid_shows_characters(SubmissionState::Failed));
    }

    #[test]
    fn failed_bottom_controls_are_all_live_and_take_expected_transitions() {
        for &(_, action) in grid_bottom_controls(SubmissionState::Failed) {
            assert!(failed_grid_transition(action).is_some(), "{action:?}");
        }
        assert_eq!(
            failed_grid_transition(GridAction::Submit),
            Some(SubmissionState::EnteringInitials)
        );
        assert_eq!(
            failed_grid_transition(GridAction::Skip),
            Some(SubmissionState::Skipped)
        );
        assert_eq!(failed_grid_transition(GridAction::Backspace), None);
    }

    #[test]
    fn grid_returns_none_outside_zones() {
        assert!(grid_action_for_normalized(0.50, 0.25).is_none()); // above grid
        assert!(grid_action_for_normalized(0.50, 0.86).is_none()); // gap between grid and buttons
        assert!(grid_action_for_normalized(0.01, 0.50).is_none()); // left of grid
    }

    // ── Display helpers ──────────────────────────────────────────────────

    #[test]
    fn board_uses_all_entries_and_cached_offline_fallback() {
        let entries = (1..=BOARD_LIMIT as u32)
            .map(|rank| BoardEntry {
                rank,
                name: format!("P{rank:02}"),
                score: 100 - rank,
            })
            .collect::<Vec<_>>();
        let fetched = LeaderboardBoard {
            status: BoardStatus::Fetched,
            entries: entries.clone(),
            cached_entries: entries.clone(),
            fetch_epoch: 0,
        };
        assert_eq!(
            format_board_text(&fetched).lines().count(),
            BOARD_LIMIT as usize
        );

        let offline = LeaderboardBoard {
            status: BoardStatus::Error("network".to_string()),
            entries: Vec::new(),
            cached_entries: entries,
            fetch_epoch: 0,
        };
        let text = format_board_text(&offline);
        assert!(text.starts_with("Offline (cached)\n"));
        assert!(text.contains("#10 P10 90"));

        let compact = format_pause_board_text(&fetched);
        assert_eq!(compact.lines().count(), (BOARD_LIMIT as usize + 1) / 2);
        assert!(compact.contains("#1"));
        assert!(compact.contains("#10"));
    }

    #[test]
    fn initials_display_pads_to_five() {
        assert_eq!(format_initials_display(""), "_____");
        assert_eq!(format_initials_display("A"), "A____");
        assert_eq!(format_initials_display("ABC"), "ABC__");
        assert_eq!(format_initials_display("ABCDE"), "ABCDE");
    }

    #[test]
    fn rendered_leaderboard_labels_are_ascii() {
        let ranks = Some(SubmissionRanks {
            global: 17,
            condition: 4,
        });
        for state in [
            SubmissionState::Ready,
            SubmissionState::EnteringInitials,
            SubmissionState::Submitting,
            SubmissionState::Submitted,
            SubmissionState::Failed,
            SubmissionState::Unavailable,
        ] {
            assert!(
                format_submission_text(state, "ABC", ranks, Some("Offline"), true).is_ascii(),
                "{state:?}"
            );
            assert!(mobile_status_text(state, "ABC", ranks, true).is_ascii());
        }
        assert!(SUBMISSION_CONSENT_DISCLOSURE.is_ascii());
        assert!(
            submission_context(&ScoreSnapshot {
                terminal_total: 42,
                condition: 1,
                objective_completed: true,
                ..default()
            })
            .is_ascii()
        );
    }

    // ── Error formatting ─────────────────────────────────────────────────

    #[test]
    fn friendly_error_distinguishes_codes_and_retryability() {
        assert_eq!(
            friendly_error(Some("rate_limited"), None, "fallback"),
            "SERVER [rate_limited]: retry later."
        );
        assert_eq!(
            friendly_error(Some("turnstile_failed"), None, "fallback"),
            "TURNSTILE [turnstile_failed]: verification failed; retry."
        );
        assert_eq!(
            friendly_error(Some("replay"), None, "fallback"),
            "SERVER [replay]: session expired; retry to request a new session."
        );
        assert_eq!(
            friendly_error(
                Some("invalid_duration"),
                Some("round_duration_ms out of range"),
                "fallback"
            ),
            "VALIDATION [invalid_duration]: round_duration_ms out of range; retry unchanged will fail."
        );
        assert_eq!(
            friendly_error(
                Some("score_over_cap"),
                Some("terminal_total exceeds plausibility cap"),
                "fallback"
            ),
            "VALIDATION [score_over_cap]: terminal_total exceeds plausibility cap; retry unchanged will fail."
        );
    }

    #[test]
    fn friendly_error_falls_back_without_losing_unknown_code() {
        assert_eq!(
            friendly_error(Some("unknown_code"), Some("custom"), "fallback"),
            "SERVER [unknown_code]: custom; retry."
        );
        assert_eq!(
            friendly_error(None, Some("ignored because no structured code"), "HTTP 500"),
            "NETWORK/SERVER [http]: HTTP 500; retry."
        );
    }

    // ── Game over reason ─────────────────────────────────────────────────

    #[test]
    fn game_over_reason_maps_to_backend_strings() {
        assert_eq!(
            game_over_reason_str(GameOverReason::TimeUp),
            Some("time_up")
        );
        assert_eq!(
            game_over_reason_str(GameOverReason::Wrecked),
            Some("wrecked")
        );
        assert_eq!(game_over_reason_str(GameOverReason::Drowned), None);
    }

    // ── Board fetch policy ───────────────────────────────────────────────

    #[test]
    fn board_fetch_action_always_refreshes_cached_unless_fetching() {
        // With cached entries and Fetched status → refresh, not silent reuse.
        assert_eq!(
            board_fetch_action(&BoardStatus::Fetched, true, true),
            BoardFetchAction::RefreshFromCache
        );
        // Without cache, Idle status → still refresh.
        assert_eq!(
            board_fetch_action(&BoardStatus::Idle, true, true),
            BoardFetchAction::RefreshFromCache
        );
        // Error status with cache → still refresh.
        assert_eq!(
            board_fetch_action(&BoardStatus::Error("offline".to_string()), true, true),
            BoardFetchAction::RefreshFromCache
        );
        // Already fetching → keep the in-flight request (no redundant fetch).
        assert_eq!(
            board_fetch_action(&BoardStatus::Fetching, true, true),
            BoardFetchAction::KeepInFlight
        );
        // Native or unconfigured → unavailable.
        assert_eq!(
            board_fetch_action(&BoardStatus::Fetched, true, false),
            BoardFetchAction::Unavailable
        );
        assert_eq!(
            board_fetch_action(&BoardStatus::Fetched, false, true),
            BoardFetchAction::Unavailable
        );
    }

    #[test]
    fn refresh_after_submit_overrides_in_flight_fetch() {
        // board_fetch_action keeps an in-flight fetch, but a successful
        // submission must force a new one (the in-flight request predates the
        // submitted score and would return stale rankings). On native (test
        // host) the board is marked unavailable; the force-refresh path is
        // web-only.
        let entries = vec![BoardEntry {
            rank: 1,
            name: "AAA".to_string(),
            score: 100,
        }];
        let mut board = LeaderboardBoard {
            status: BoardStatus::Fetching,
            entries: entries.clone(),
            cached_entries: entries,
            fetch_epoch: 3,
        };
        refresh_board_after_submit(&mut board);
        assert_eq!(board.status, BoardStatus::Unavailable);
        assert_eq!(board.fetch_epoch, 3); // unchanged on native
        assert!(!board.cached_entries.is_empty()); // cache retained
    }

    // ── Cached-round and in-flight display ───────────────────────────────

    #[test]
    fn fetching_with_cache_shows_refreshing_cached_label() {
        let entries = vec![
            BoardEntry {
                rank: 1,
                name: "AAA".to_string(),
                score: 100,
            },
            BoardEntry {
                rank: 2,
                name: "BBB".to_string(),
                score: 90,
            },
        ];
        let board = LeaderboardBoard {
            status: BoardStatus::Fetching,
            entries: entries.clone(),
            cached_entries: entries,
            fetch_epoch: 0,
        };
        let text = format_board_text(&board);
        assert!(text.starts_with("Refreshing - cached\n"));
        assert!(text.contains("#1 AAA 100"));
        assert!(text.contains("#2 BBB 90"));

        let compact = format_pause_board_text(&board);
        assert!(compact.starts_with("Refreshing - cached\n"));
    }

    #[test]
    fn fetching_without_cache_shows_loading() {
        let board = LeaderboardBoard {
            status: BoardStatus::Fetching,
            entries: Vec::new(),
            cached_entries: Vec::new(),
            fetch_epoch: 0,
        };
        assert_eq!(format_board_text(&board), "Loading...");
        // Pause falls through to format_board_text when cache is empty.
        assert_eq!(format_pause_board_text(&board), "Loading...");
    }

    #[test]
    fn in_flight_fetch_preserves_cached_display_across_transitions() {
        // Simulate a board mid-fetch (in-flight) with cached entries from a
        // prior successful fetch. During rapid Menu/Playing/Pause transitions
        // the display shows "Refreshing - cached" while the epoch-guarded
        // result arrives, and begin_board_fetch keeps the in-flight request.
        let entries = vec![BoardEntry {
            rank: 1,
            name: "OLD".to_string(),
            score: 50,
        }];
        let board = LeaderboardBoard {
            status: BoardStatus::Fetching,
            entries: entries.clone(),
            cached_entries: entries,
            fetch_epoch: 7,
        };
        assert_eq!(
            board_fetch_action(&board.status, true, true),
            BoardFetchAction::KeepInFlight
        );
        assert_eq!(board.fetch_epoch, 7); // epoch unchanged by action
        assert!(format_board_text(&board).contains("Refreshing - cached"));
        assert!(format_pause_board_text(&board).contains("Refreshing - cached"));
    }
}
