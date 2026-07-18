//! Isolated browser client for the additive ranked `/v3` protocol.
//!
//! This module deliberately does not share the legacy v1 submission path.  It
//! accepts only the frozen v3 capability tuple, starts one Worker-issued
//! session, and submits the immutable game-owned canonical ledger at terminal.

use bevy::prelude::*;
use roady_score_rules::v3::{self, canonical};
use serde::{Deserialize, Serialize};

use crate::game::state::GameState;
use crate::game_modes::{ActiveRunRules, Conduct, InjectedRankedSession, WorkerRankedReceipt};
use crate::ledger::{FinalGameOverSnapshot, V3LedgerState};
use crate::settings::Settings;

const API_URL: &str = match option_env!("LEADERBOARD_API_URL") {
    Some(value) => value,
    None => "",
};
const TURNSTILE_SITE_KEY: &str = match option_env!("LB_TURNSTILE_SITE_KEY") {
    Some(value) => value,
    None => "",
};
const CLIENT_KEY: &str = match option_env!("ROADY_V3_CLIENT_HMAC_KEY") {
    Some(value) => value,
    None => "",
};
const SIGNATURE_KEY_ID: &str = match option_env!("ROADY_V3_CLIENT_HMAC_KEY_ID") {
    Some(value) => value,
    None => "v3.client.1",
};
const MAX_SAFE_JSON_INTEGER: u64 = 9_007_199_254_740_991;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum RankedV3Phase {
    #[default]
    Disabled,
    Checking,
    Ready,
    Starting,
    Started,
    SubmittingScore,
    Live,
    Quarantined,
    FailedClosed,
}

/// Public menu-facing state. `failed_closed` is sticky for the lifetime of the
/// page: a later cached or successful response cannot recover a client session
/// which has observed malformed, stale, or mismatched protocol data.
#[derive(Resource, Debug, Default)]
pub struct RankedV3Client {
    pub phase: RankedV3Phase,
    pub message: String,
    epoch: u64,
    failed_closed: bool,
    capability_admitted: bool,
    start_request: Option<Conduct>,
    terminal_started: bool,
}

impl RankedV3Client {
    pub fn ranked_available(&self) -> bool {
        !self.failed_closed && self.capability_admitted && self.phase == RankedV3Phase::Ready
    }

    pub fn capability_admitted(&self) -> bool {
        !self.failed_closed && self.capability_admitted
    }

    pub fn request_ranked_start(&mut self, conduct: Conduct) -> bool {
        if !self.ranked_available() || self.start_request.is_some() {
            return false;
        }
        self.start_request = Some(conduct);
        self.phase = RankedV3Phase::Starting;
        self.message = "Verifying Ranked run…".into();
        true
    }

    fn fail(&mut self, message: impl Into<String>) {
        self.failed_closed = true;
        self.capability_admitted = false;
        self.start_request = None;
        self.phase = RankedV3Phase::FailedClosed;
        self.message = message.into();
    }
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct RankedCapability {
    enabled: bool,
    categories: Vec<String>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
struct Capabilities {
    ranked: RankedCapability,
    protocol_version: u8,
    protocol_id: String,
    rules_version: u8,
    rules_id: String,
    policy_version: u8,
    policy_id: String,
    mode: String,
}

fn decode_capabilities(body: &str) -> Result<bool, &'static str> {
    let value: Capabilities = serde_json::from_str(body).map_err(|_| "invalid capability JSON")?;
    if value.protocol_version != v3::PROTOCOL_VERSION
        || value.protocol_id != v3::PROTOCOL_ID
        || value.rules_version != v3::RULES_VERSION
        || value.rules_id != v3::RULES_VERSION_ID
        || value.policy_version != v3::POLICY_VERSION
        || value.policy_id != v3::POLICY_ID
        || value.mode != v3::MODE
        || value.ranked.categories != [v3::CLUCK_HUNT_CATEGORY, v3::RIGHT_OF_WAY_CATEGORY]
    {
        return Err("Ranked protocol mismatch");
    }
    Ok(value.ranked.enabled)
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
struct SessionResponse {
    session_id: String,
    challenge: String,
    mode: String,
    category_key: String,
    seed_hex: String,
    seed_commitment: String,
    schedule_hash: String,
    issued_at: u64,
    start_by_expiry: u64,
    proof: String,
    protocol_version: u8,
    protocol_id: String,
    rules_version: u8,
    rules_id: String,
    policy_version: u8,
    policy_id: String,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
struct StartResponse {
    started: bool,
    started_at: u64,
    proof: String,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct SessionRequest<'a> {
    mode: &'a str,
    category_key: &'a str,
    turnstile_token: &'a str,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct StartRequest<'a> {
    session_id: &'a str,
    proof: &'a str,
}

fn exact_hex32(value: &str) -> Result<[u8; 32], &'static str> {
    if value.len() != 64
        || !value
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
    {
        return Err("invalid canonical hex");
    }
    let mut result = [0; 32];
    for (index, byte) in result.iter_mut().enumerate() {
        *byte = u8::from_str_radix(&value[index * 2..index * 2 + 2], 16)
            .map_err(|_| "invalid canonical hex")?;
    }
    Ok(result)
}

fn hex(bytes: &[u8]) -> String {
    const TABLE: &[u8; 16] = b"0123456789abcdef";
    let mut result = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        result.push(TABLE[(byte >> 4) as usize] as char);
        result.push(TABLE[(byte & 15) as usize] as char);
    }
    result
}

fn base64url(bytes: &[u8]) -> String {
    const TABLE: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";
    let mut out = String::with_capacity((bytes.len() * 4 + 2) / 3);
    for chunk in bytes.chunks(3) {
        let b0 = chunk[0];
        let b1 = *chunk.get(1).unwrap_or(&0);
        let b2 = *chunk.get(2).unwrap_or(&0);
        out.push(TABLE[(b0 >> 2) as usize] as char);
        out.push(TABLE[(((b0 & 3) << 4) | (b1 >> 4)) as usize] as char);
        if chunk.len() > 1 {
            out.push(TABLE[(((b1 & 15) << 2) | (b2 >> 6)) as usize] as char);
        }
        if chunk.len() > 2 {
            out.push(TABLE[(b2 & 63) as usize] as char);
        }
    }
    out
}

fn decode_base64url_32(value: &str) -> Result<(), &'static str> {
    if value.len() != 43
        || !value
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
    {
        return Err("invalid canonical proof");
    }
    // A 32-byte unpadded value has 43 chars and its final character carries
    // only four significant bits. Re-encoding catches non-canonical aliases.
    let mut bytes = Vec::with_capacity(32);
    let decode = |b: u8| -> Option<u8> {
        Some(match b {
            b'A'..=b'Z' => b - b'A',
            b'a'..=b'z' => b - b'a' + 26,
            b'0'..=b'9' => b - b'0' + 52,
            b'-' => 62,
            b'_' => 63,
            _ => return None,
        })
    };
    for chunk in value.as_bytes().chunks(4) {
        let a = decode(chunk[0]).ok_or("invalid canonical proof")?;
        let b = decode(chunk[1]).ok_or("invalid canonical proof")?;
        bytes.push((a << 2) | (b >> 4));
        if chunk.len() > 2 {
            let c = decode(chunk[2]).ok_or("invalid canonical proof")?;
            bytes.push((b << 4) | (c >> 2));
            if chunk.len() > 3 {
                let d = decode(chunk[3]).ok_or("invalid canonical proof")?;
                bytes.push((c << 6) | d);
            }
        }
    }
    if bytes.len() != 32 || base64url(&bytes) != value {
        return Err("invalid canonical proof");
    }
    Ok(())
}

fn validate_session_response(
    response: SessionResponse,
    conduct: Conduct,
) -> Result<(SessionResponse, [u8; 32], [u8; 32], [u8; 32], Vec<u8>), &'static str> {
    let category = conduct.category();
    if response.protocol_version != v3::PROTOCOL_VERSION
        || response.protocol_id != v3::PROTOCOL_ID
        || response.rules_version != v3::RULES_VERSION
        || response.rules_id != v3::RULES_VERSION_ID
        || response.policy_version != v3::POLICY_VERSION
        || response.policy_id != v3::POLICY_ID
        || response.mode != v3::MODE
        || response.category_key != category
        || response.session_id.is_empty()
        || response.session_id.len() > 255
        || response.challenge.is_empty()
        || response.challenge.len() > 255
        || response.start_by_expiry
            != response
                .issued_at
                .checked_add(300_000)
                .ok_or("invalid start window")?
    {
        return Err("invalid Ranked session tuple");
    }
    decode_base64url_32(&response.proof)?;
    let seed = exact_hex32(&response.seed_hex)?;
    let seed_commitment = exact_hex32(&response.seed_commitment)?;
    let schedule_hash = exact_hex32(&response.schedule_hash)?;
    if v3::seed_commitment(&seed) != seed_commitment {
        return Err("seed commitment mismatch");
    }
    if v3::schedule_commitment(&seed, category) != schedule_hash {
        return Err("schedule commitment mismatch");
    }
    let input = canonical::SessionHeader {
        category,
        session_id: &response.session_id,
        challenge: &response.challenge,
        seed_commitment: &seed_commitment,
        schedule_hash: &schedule_hash,
        issued_at_ms: response.issued_at,
    };
    let unstarted = canonical::unstarted_session_header(&input, response.start_by_expiry)
        .map_err(|_| "invalid unstarted header")?;
    Ok((response, seed, seed_commitment, schedule_hash, unstarted))
}

fn finish_receipt(
    response: &SessionResponse,
    start: StartResponse,
    conduct: Conduct,
    seed: [u8; 32],
    seed_commitment: [u8; 32],
    schedule_hash: [u8; 32],
) -> Result<WorkerRankedReceipt, &'static str> {
    if !start.started || start.started_at == 0 {
        return Err("invalid start acknowledgement");
    }
    decode_base64url_32(&start.proof)?;
    let input = canonical::SessionHeader {
        category: conduct.category(),
        session_id: &response.session_id,
        challenge: &response.challenge,
        seed_commitment: &seed_commitment,
        schedule_hash: &schedule_hash,
        issued_at_ms: response.issued_at,
    };
    let started_header = canonical::started_session_header(&input, start.started_at)
        .map_err(|_| "invalid started header")?;
    WorkerRankedReceipt {
        session_id: response.session_id.clone(),
        challenge: response.challenge.clone(),
        seed,
        schedule: v3::rotation_schedule(&seed),
        conduct,
        category: conduct.category().into(),
        issued_at_ms: response.issued_at,
        started_at_ms: start.started_at,
        started_proof: start.proof,
        started_header,
        schedule_hash,
        seed_commitment,
    }
    .validate(conduct)
    .map_err(|_| "invalid started receipt")
}

fn api(path: &str) -> String {
    format!("{}{}", API_URL.trim_end_matches('/'), path)
}

fn path_segment(value: &str) -> String {
    let mut out = String::new();
    for byte in value.bytes() {
        if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.' | b'~') {
            out.push(byte as char);
        } else {
            out.push_str(&format!("%{byte:02X}"));
        }
    }
    out
}

#[derive(Clone, Debug)]
struct TerminalPackage {
    session_id: String,
    proof: String,
    category: String,
    schedule_hash: [u8; 32],
    seed_commitment: [u8; 32],
    terminal: canonical::ConductTerminal,
    final_root: [u8; 32],
    event_count: u32,
    ledger_bytes: Vec<u8>,
    evidence_hash: [u8; 32],
    name: String,
}

fn normalize_name(value: &str) -> Option<String> {
    let value = value.trim().to_ascii_uppercase();
    ((3..=5).contains(&value.len())
        && value.is_ascii()
        && value
            .bytes()
            .all(|b| b.is_ascii_uppercase() || b.is_ascii_digit()))
    .then_some(value)
}

fn terminal_package(
    rules: &ActiveRunRules,
    snapshot: &FinalGameOverSnapshot,
    state: &V3LedgerState,
    name: &str,
) -> Result<TerminalPackage, &'static str> {
    let receipt = rules.ranked_receipt().ok_or("not a Ranked run")?;
    let terminal = snapshot
        .terminal
        .clone()
        .ok_or("missing terminal snapshot")?;
    if terminal.conduct() != receipt.conduct.rules() || snapshot.reason.is_none() {
        return Err("terminal conduct mismatch");
    }
    let ledger = state.ledger.as_ref().ok_or("missing canonical ledger")?;
    if ledger.terminal().map_err(|_| "missing ledger terminal")? != &terminal
        || ledger.event_count() != snapshot.event_count
    {
        return Err("terminal snapshot mismatch");
    }
    let final_root = ledger.final_root().map_err(|_| "invalid final root")?;
    if snapshot.final_root != Some(final_root) || state.final_root != Some(final_root) {
        return Err("final root mismatch");
    }
    let evidence = ledger
        .evidence_bytes(&receipt.session_id)
        .map_err(|_| "invalid evidence")?;
    let evidence_hash = canonical::evidence_hash(&evidence);
    let name = normalize_name(name).ok_or("set a 3–5 character leaderboard name in Settings")?;
    Ok(TerminalPackage {
        session_id: receipt.session_id.clone(),
        proof: receipt.started_proof.clone(),
        category: receipt.category.clone(),
        schedule_hash: receipt.schedule_hash,
        seed_commitment: receipt.seed_commitment,
        terminal,
        final_root,
        event_count: ledger.event_count(),
        ledger_bytes: ledger.stored_bytes().to_vec(),
        evidence_hash,
        name,
    })
}

fn reason(value: v3::TerminalReason) -> &'static str {
    match value {
        v3::TerminalReason::TimeUp => "time_up",
        v3::TerminalReason::Wrecked => "wrecked",
        v3::TerminalReason::Drowned => "drowned",
    }
}
fn platform(value: v3::Platform) -> &'static str {
    match value {
        v3::Platform::Web => "web",
        v3::Platform::Native => "native",
    }
}

fn score_body(package: &TerminalPackage) -> serde_json::Value {
    let common =
        |terminal_reason, total, objective, duration, remaining, build: &str, platform_value| {
            serde_json::json!({
                "sessionId": package.session_id,
                "proof": package.proof,
                "name": package.name,
                "categoryKey": package.category,
                "terminalTotal": total,
                "objectiveCompleted": objective,
                "roundDurationMs": duration,
                "timeLeftMs": remaining,
                "gameOverReason": terminal_reason,
                "build": build,
                "platform": platform_value,
                "finalRoot": hex(&package.final_root),
                "scheduleHash": hex(&package.schedule_hash),
                "eventCount": package.event_count,
                "signatureKeyId": SIGNATURE_KEY_ID,
                "protocolVersion": v3::PROTOCOL_VERSION,
                "protocolId": v3::PROTOCOL_ID,
                "rulesVersion": v3::RULES_VERSION,
                "rulesId": v3::RULES_VERSION_ID,
                "policyVersion": v3::POLICY_VERSION,
                "policyId": v3::POLICY_ID,
                "mode": v3::MODE
            })
        };
    let body = match &package.terminal {
        canonical::ConductTerminal::CluckHunt(value) => {
            let mut body = common(
                reason(value.reason),
                value.total,
                value.objective_completed,
                value.duration_ms,
                value.remaining_ms,
                &value.build,
                platform(value.platform),
            );
            let map = body.as_object_mut().expect("JSON object");
            map.insert("chickens".into(), value.chickens.into());
            map.insert("coins".into(), value.coins.into());
            map.insert("maxCombo".into(), value.max_combo.into());
            body
        }
        canonical::ConductTerminal::RightOfWay(value) => {
            let mut body = common(
                reason(value.reason),
                value.total,
                value.objective_completed,
                value.duration_ms,
                value.remaining_ms,
                &value.build,
                platform(value.platform),
            );
            let map = body.as_object_mut().expect("JSON object");
            map.insert(
                "signedAccumulator".into(),
                value.accumulator.to_string().into(),
            );
            map.insert("premiumBps".into(), value.premium_bps.into());
            map.insert("packagesDelivered".into(), value.packages_delivered.into());
            map.insert("courtesyCount".into(), value.courtesy_count.into());
            map.insert("animalHits".into(), value.animal_hits.into());
            map.insert("maxDeliveryChain".into(), value.max_delivery_chain.into());
            body
        }
    };
    // Keep this mutable binding to make accidental post-construction fields
    // conspicuous in review; the Worker rejects every unknown key.
    body
}

fn validate_json_safe(package: &TerminalPackage) -> Result<(), &'static str> {
    let (duration, remaining) = match &package.terminal {
        canonical::ConductTerminal::CluckHunt(v) => (v.duration_ms, v.remaining_ms),
        canonical::ConductTerminal::RightOfWay(v) => (v.duration_ms, v.remaining_ms),
    };
    if duration > MAX_SAFE_JSON_INTEGER || remaining > MAX_SAFE_JSON_INTEGER {
        Err("terminal time is not an exact JSON integer")
    } else {
        Ok(())
    }
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
struct ScoreResponse {
    inserted: bool,
    rank: Option<u32>,
    global_rank: Option<u32>,
    category_key: String,
    total: u32,
    submitted_at: u64,
    status: String,
    evidence_capability: String,
    evidence_expires_at: u64,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
struct EvidenceResponse {
    accepted: bool,
    idempotent: bool,
    status: String,
    rank: Option<u32>,
}

#[derive(Clone, Debug)]
enum AsyncResult {
    Capability(Result<bool, String>),
    Started(Result<WorkerRankedReceipt, String>),
    Submitted(Result<SubmitOutcome, String>),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum SubmitOutcome {
    Live,
    Quarantined,
}

#[cfg(target_arch = "wasm32")]
mod browser {
    use super::*;
    use std::cell::RefCell;
    use wasm_bindgen::{JsCast, JsValue, closure::Closure};
    use wasm_bindgen_futures::JsFuture;

    thread_local! {
        static RESULTS: RefCell<Vec<(u64, AsyncResult)>> = const { RefCell::new(Vec::new()) };
        static CONTROLLERS: RefCell<Vec<web_sys::AbortController>> = const { RefCell::new(Vec::new()) };
    }

    pub fn cancel() {
        CONTROLLERS.with(|items| {
            for controller in items.borrow_mut().drain(..) {
                controller.abort();
            }
        });
    }
    pub fn take() -> Option<(u64, AsyncResult)> {
        RESULTS.with(|items| items.borrow_mut().pop())
    }
    fn push(epoch: u64, result: AsyncResult) {
        RESULTS.with(|items| items.borrow_mut().push((epoch, result)));
    }

    fn controller() -> Result<web_sys::AbortController, String> {
        let value = web_sys::AbortController::new()
            .map_err(|_| "browser cancellation unavailable".to_string())?;
        CONTROLLERS.with(|items| items.borrow_mut().push(value.clone()));
        Ok(value)
    }

    async fn fetch(
        url: &str,
        method: &str,
        body: Option<&str>,
        signature: Option<&str>,
        controller: &web_sys::AbortController,
    ) -> Result<(u16, String), String> {
        let window = web_sys::window().ok_or("browser window unavailable")?;
        let options = web_sys::RequestInit::new();
        options.set_method(method);
        options.set_signal(Some(&controller.signal()));
        // Avoid a stale browser HTTP-cache object unlocking Ranked. Reflect is
        // used so no additional web-sys feature changes generated bindings.
        js_sys::Reflect::set(options.as_ref(), &"cache".into(), &"no-store".into())
            .map_err(|_| "request cache control unavailable")?;
        let headers = web_sys::Headers::new().map_err(|_| "request headers unavailable")?;
        if let Some(body) = body {
            headers
                .set("Content-Type", "application/json")
                .map_err(|_| "content type rejected")?;
            options.set_body(&JsValue::from_str(body));
        }
        if let Some(signature) = signature {
            headers
                .set("X-Roady-Client-Signature", signature)
                .map_err(|_| "signature header rejected")?;
        }
        options.set_headers(headers.as_ref());
        let request = web_sys::Request::new_with_str_and_init(url, &options)
            .map_err(|_| "request construction failed")?;
        const TIMEOUT_MS: i32 = 15_000;
        let timeout = {
            let controller = controller.clone();
            Closure::<dyn FnMut()>::new(move || controller.abort())
        };
        let timer = window
            .set_timeout_with_callback_and_timeout_and_arguments_0(
                timeout.as_ref().unchecked_ref(),
                TIMEOUT_MS,
            )
            .map_err(|_| "request timeout unavailable")?;
        let response_value = JsFuture::from(window.fetch_with_request(&request)).await;
        window.clear_timeout_with_handle(timer);
        drop(timeout);
        let response_value = response_value.map_err(|_| {
            if controller.signal().aborted() {
                "request timed out or was cancelled".to_string()
            } else {
                "network request failed".to_string()
            }
        })?;
        let response: web_sys::Response = response_value
            .dyn_into()
            .map_err(|_| "invalid browser response")?;
        let status = response.status();
        let text = JsFuture::from(response.text().map_err(|_| "response body unavailable")?)
            .await
            .map_err(|_| "response body failed")?
            .as_string()
            .ok_or("response was not text")?;
        Ok((status, text))
    }

    fn bridge() -> Result<JsValue, String> {
        let value = js_sys::Reflect::get(&js_sys::global(), &"roadyLeaderboard".into())
            .map_err(|_| "browser bridge unavailable")?;
        if value.is_null() || value.is_undefined() {
            Err("browser bridge unavailable".into())
        } else {
            Ok(value)
        }
    }
    fn function(api: &JsValue, name: &str) -> Result<js_sys::Function, String> {
        js_sys::Reflect::get(api, &name.into())
            .map_err(|_| "browser bridge unavailable".to_string())?
            .dyn_into()
            .map_err(|_| "browser bridge unavailable".to_string())
    }
    async fn turnstile() -> Result<String, String> {
        let api = bridge()?;
        let promise = function(&api, "getTurnstileToken")?
            .call1(&api, &TURNSTILE_SITE_KEY.into())
            .map_err(|_| "Turnstile could not start")?;
        let value = JsFuture::from(js_sys::Promise::from(promise))
            .await
            .map_err(|_| "Turnstile failed")?;
        if js_sys::Reflect::get(&value, &"ok".into())
            .ok()
            .and_then(|v| v.as_bool())
            != Some(true)
        {
            return Err("Turnstile verification failed".into());
        }
        js_sys::Reflect::get(&value, &"token".into())
            .ok()
            .and_then(|v| v.as_string())
            .filter(|v| !v.is_empty())
            .ok_or_else(|| "Turnstile returned no token".into())
    }
    async fn hmac(data: &[u8]) -> Result<String, String> {
        let api = bridge()?;
        let encoded = base64url(data);
        let promise = function(&api, "hmacSha256Base64UrlBytes")?
            .call2(&api, &CLIENT_KEY.into(), &encoded.into())
            .map_err(|_| "score signature failed")?;
        let value = JsFuture::from(js_sys::Promise::from(promise))
            .await
            .map_err(|_| "score signature failed")?;
        let signature = value.as_string().ok_or("score signature failed")?;
        decode_base64url_32(&signature).map_err(str::to_string)?;
        Ok(signature)
    }

    pub fn capability(epoch: u64) {
        wasm_bindgen_futures::spawn_local(async move {
            let result = async {
                if API_URL.is_empty() {
                    return Err("Ranked service is not configured".into());
                }
                let controller = controller()?;
                let (status, body) =
                    fetch(&api("/v3/capabilities"), "GET", None, None, &controller).await?;
                if status != 200 {
                    return Err(format!("Ranked service returned HTTP {status}"));
                }
                decode_capabilities(&body).map_err(str::to_string)
            }
            .await;
            push(epoch, AsyncResult::Capability(result));
        });
    }

    pub fn start(epoch: u64, conduct: Conduct) {
        wasm_bindgen_futures::spawn_local(async move {
            let result = async {
                if TURNSTILE_SITE_KEY.is_empty() {
                    return Err("Turnstile is not configured".into());
                }
                let controller = controller()?;
                let token = turnstile().await?;
                if controller.signal().aborted() {
                    return Err("request cancelled".into());
                }
                let request = serde_json::to_string(&SessionRequest {
                    mode: v3::MODE,
                    category_key: conduct.category(),
                    turnstile_token: &token,
                })
                .map_err(|_| "session body failed")?;
                let (status, body) = fetch(
                    &api("/v3/session"),
                    "POST",
                    Some(&request),
                    None,
                    &controller,
                )
                .await?;
                if status != 200 {
                    return Err(format!("Ranked session rejected (HTTP {status})"));
                }
                let decoded: SessionResponse =
                    serde_json::from_str(&body).map_err(|_| "invalid session response")?;
                let (session, seed, seed_commitment, schedule_hash, _) =
                    validate_session_response(decoded, conduct).map_err(str::to_string)?;
                let start_body = serde_json::to_string(&StartRequest {
                    session_id: &session.session_id,
                    proof: &session.proof,
                })
                .map_err(|_| "start body failed")?;
                let path = format!("/v3/session/{}/start", path_segment(&session.session_id));
                let (status, body) =
                    fetch(&api(&path), "POST", Some(&start_body), None, &controller).await?;
                if status != 200 {
                    return Err(format!("Ranked start rejected (HTTP {status})"));
                }
                let start: StartResponse =
                    serde_json::from_str(&body).map_err(|_| "invalid start response")?;
                finish_receipt(
                    &session,
                    start,
                    conduct,
                    seed,
                    seed_commitment,
                    schedule_hash,
                )
                .map_err(str::to_string)
            }
            .await;
            push(epoch, AsyncResult::Started(result));
        });
    }

    pub fn submit(epoch: u64, package: TerminalPackage) {
        wasm_bindgen_futures::spawn_local(async move {
            let result = async {
                validate_json_safe(&package).map_err(str::to_string)?;
                if CLIENT_KEY.is_empty() {
                    return Err("Ranked signature key is not configured".into());
                }
                let score_controller = controller()?;
                let input = canonical::score_hmac_input(
                    &package.category,
                    &package.session_id,
                    &package.final_root,
                    &package.schedule_hash,
                    &package.seed_commitment,
                    &package.terminal,
                )
                .map_err(|_| "canonical score failed")?;
                let signature = hmac(&input).await?;
                let body = serde_json::to_string(&score_body(&package))
                    .map_err(|_| "score body failed")?;
                // Scores are intentionally never retried: a lost response is
                // non-idempotent because the one-time session may be consumed.
                let (status, response_body) = fetch(
                    &api("/v3/scores"),
                    "POST",
                    Some(&body),
                    Some(&signature),
                    &score_controller,
                )
                .await?;
                if status != 201 {
                    return Err(format!(
                        "Ranked score was not accepted (HTTP {status}); it will not be replayed"
                    ));
                }
                let score: ScoreResponse =
                    serde_json::from_str(&response_body).map_err(|_| "invalid score response")?;
                if !score.inserted
                    || score.rank.is_some()
                    || score.global_rank.is_some()
                    || score.category_key != package.category
                    || score.status != "pending"
                    || score.submitted_at == 0
                    || score.evidence_expires_at <= score.submitted_at
                    || score.total
                        != match &package.terminal {
                            canonical::ConductTerminal::CluckHunt(v) => v.total,
                            canonical::ConductTerminal::RightOfWay(v) => v.total,
                        }
                {
                    return Err("invalid pending score acknowledgement".into());
                }
                decode_base64url_32(&score.evidence_capability).map_err(str::to_string)?;
                let evidence = serde_json::json!({
                    "evidenceCapability": score.evidence_capability,
                    "finalRoot": hex(&package.final_root),
                    "ledgerBytes": base64url(&package.ledger_bytes),
                    "evidenceHash": hex(&package.evidence_hash)
                })
                .to_string();
                // Evidence is byte-idempotent. Retry only transport failures,
                // reusing this exact String and capability; never rebuild it.
                let mut last = String::new();
                for attempt in 0..=2 {
                    // A timed-out AbortController cannot be reused. Each retry
                    // gets a fresh controller while preserving the exact body
                    // String and capability bytes.
                    let evidence_controller = controller()?;
                    match fetch(
                        &api("/v3/evidence"),
                        "POST",
                        Some(&evidence),
                        None,
                        &evidence_controller,
                    )
                    .await
                    {
                        Ok((status, body)) if status == 200 || status == 201 => {
                            let ack: EvidenceResponse = serde_json::from_str(&body)
                                .map_err(|_| "invalid evidence response")?;
                            let valid_live =
                                ack.accepted && ack.status == "live" && ack.rank.is_some();
                            let valid_quarantine = !ack.accepted
                                && ack.status == "quarantined"
                                && ack.rank.is_none()
                                && ack.idempotent;
                            if valid_live {
                                return Ok(SubmitOutcome::Live);
                            }
                            if valid_quarantine {
                                return Ok(SubmitOutcome::Quarantined);
                            }
                            return Err("invalid evidence acknowledgement".into());
                        }
                        Ok((409, body)) => {
                            #[derive(Deserialize)]
                            #[serde(deny_unknown_fields)]
                            struct ErrorEnvelope {
                                error: ErrorDetail,
                            }
                            #[derive(Deserialize)]
                            #[serde(deny_unknown_fields)]
                            struct ErrorDetail {
                                code: String,
                                message: String,
                                #[serde(rename = "requestId")]
                                request_id: String,
                            }
                            let error: ErrorEnvelope = serde_json::from_str(&body)
                                .map_err(|_| "invalid evidence error response")?;
                            let _ = (&error.error.message, &error.error.request_id);
                            if error.error.code == "evidence_conflict" {
                                return Ok(SubmitOutcome::Quarantined);
                            }
                            return Err(format!("evidence stopped safely ({})", error.error.code));
                        }
                        Ok((status, _)) => {
                            return Err(format!("evidence rejected (HTTP {status})"));
                        }
                        Err(error) => {
                            last = error;
                            if attempt == 2 {
                                break;
                            }
                        }
                    }
                }
                Err(format!("evidence upload failed: {last}"))
            }
            .await;
            push(epoch, AsyncResult::Submitted(result));
        });
    }
}

#[cfg(not(target_arch = "wasm32"))]
mod browser {
    use super::*;
    pub fn cancel() {}
    pub fn take() -> Option<(u64, AsyncResult)> {
        None
    }
    pub fn capability(_epoch: u64) {}
    pub fn start(_epoch: u64, _conduct: Conduct) {}
    pub fn submit(_epoch: u64, _package: TerminalPackage) {}
}

fn menu_boundary(mut client: ResMut<RankedV3Client>, mut injected: ResMut<InjectedRankedSession>) {
    browser::cancel();
    injected.0 = None;
    client.start_request = None;
    client.terminal_started = false;
    client.capability_admitted = false;
    if client.failed_closed {
        return;
    }
    client.epoch = client.epoch.wrapping_add(1).max(1);
    client.phase = RankedV3Phase::Checking;
    client.message = "Checking Ranked service…".into();
    if !cfg!(target_arch = "wasm32") || API_URL.is_empty() {
        client.fail("Ranked is unavailable in this client");
        return;
    }
    browser::capability(client.epoch);
}

fn drive_client(
    mut client: ResMut<RankedV3Client>,
    mut injected: ResMut<InjectedRankedSession>,
    mut next: ResMut<NextState<GameState>>,
) {
    while let Some((epoch, result)) = browser::take() {
        if epoch != client.epoch {
            client.fail("A stale Ranked response was rejected");
            browser::cancel();
            return;
        }
        match result {
            AsyncResult::Capability(Ok(true)) => {
                client.capability_admitted = true;
                client.phase = RankedV3Phase::Ready;
                client.message = "Ranked verified".into();
            }
            AsyncResult::Capability(Ok(false)) => {
                client.capability_admitted = false;
                client.phase = RankedV3Phase::Disabled;
                client.message = "Ranked is currently offline".into();
            }
            AsyncResult::Capability(Err(error))
            | AsyncResult::Started(Err(error))
            | AsyncResult::Submitted(Err(error)) => {
                client.fail(error);
                browser::cancel();
            }
            AsyncResult::Started(Ok(receipt)) => {
                injected.0 = Some(receipt);
                client.phase = RankedV3Phase::Started;
                client.message = "Ranked run verified".into();
                next.set(GameState::Playing);
            }
            AsyncResult::Submitted(Ok(SubmitOutcome::Live)) => {
                client.phase = RankedV3Phase::Live;
                client.message = "Ranked score verified".into();
            }
            AsyncResult::Submitted(Ok(SubmitOutcome::Quarantined)) => {
                client.phase = RankedV3Phase::Quarantined;
                client.message = "Score held for verification".into();
            }
        }
    }
    // Avoid DerefMut on the idle path: Bevy treats any mutable dereference as
    // a change, and the menu debounces rebuilds from visible Ranked state.
    if client.start_request.is_some() {
        let conduct = client
            .start_request
            .take()
            .expect("start request checked immediately before take");
        client.epoch = client.epoch.wrapping_add(1).max(1);
        browser::cancel();
        browser::start(client.epoch, conduct);
    }
}

fn begin_terminal_submission(
    mut client: ResMut<RankedV3Client>,
    rules: Res<ActiveRunRules>,
    snapshot: Res<FinalGameOverSnapshot>,
    ledger: Res<V3LedgerState>,
    settings: Res<Settings>,
) {
    if client.terminal_started || !rules.is_ranked() || snapshot.terminal.is_none() {
        return;
    }
    client.terminal_started = true;
    let package = match terminal_package(&rules, &snapshot, &ledger, &settings.leaderboard_initials)
    {
        Ok(package) => package,
        Err(error) => {
            client.fail(format!("Ranked submission stopped safely: {error}"));
            return;
        }
    };
    client.epoch = client.epoch.wrapping_add(1).max(1);
    client.phase = RankedV3Phase::SubmittingScore;
    client.message = "Submitting canonical Ranked evidence…".into();
    browser::submit(client.epoch, package);
}

fn cancel_menu_request_on_exit(mut client: ResMut<RankedV3Client>) {
    if matches!(
        client.phase,
        RankedV3Phase::Checking | RankedV3Phase::Starting
    ) {
        browser::cancel();
        client.epoch = client.epoch.wrapping_add(1).max(1);
        client.fail("Ranked verification was cancelled after navigation");
    }
}

fn cancel_terminal_on_exit(mut client: ResMut<RankedV3Client>) {
    browser::cancel();
    client.epoch = client.epoch.wrapping_add(1).max(1);
    client.terminal_started = false;
    if matches!(client.phase, RankedV3Phase::SubmittingScore) {
        client.fail("Ranked submission was cancelled after navigation");
    }
}

pub struct CompetitiveV3Plugin;
impl Plugin for CompetitiveV3Plugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<RankedV3Client>()
            .add_systems(OnEnter(GameState::Menu), menu_boundary)
            .add_systems(OnExit(GameState::Menu), cancel_menu_request_on_exit)
            .add_systems(Update, drive_client)
            .add_systems(
                Update,
                begin_terminal_submission.run_if(in_state(GameState::GameOver)),
            )
            .add_systems(OnExit(GameState::GameOver), cancel_terminal_on_exit);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn capabilities(enabled: bool) -> String {
        format!(
            r#"{{"ranked":{{"enabled":{enabled},"categories":["rotation.v2.cluck_hunt","rotation.v2.right_of_way"]}},"protocolVersion":3,"protocolId":"roady-protocol.v3","rulesVersion":3,"rulesId":"roady-rules.v3","policyVersion":1,"policyId":"roady-ranked-policy.v3.1","mode":"rotation"}}"#
        )
    }

    #[test]
    fn idle_drive_client_does_not_mark_ranked_state_changed() {
        let mut app = App::new();
        app.init_resource::<RankedV3Client>()
            .init_resource::<InjectedRankedSession>()
            .init_resource::<NextState<GameState>>()
            .add_systems(Update, drive_client);
        app.world_mut().clear_trackers();
        app.update();
        assert!(
            !app.world()
                .get_resource_ref::<RankedV3Client>()
                .expect("ranked resource")
                .is_changed()
        );
    }

    #[test]
    fn capability_decoder_is_exact_and_order_bound() {
        assert_eq!(decode_capabilities(&capabilities(true)), Ok(true));
        assert_eq!(decode_capabilities(&capabilities(false)), Ok(false));
        assert!(
            decode_capabilities(
                &(capabilities(true)[..capabilities(true).len() - 1].to_owned()
                    + r#", "extra":1}"#)
            )
            .is_err()
        );
        assert!(
            decode_capabilities(&capabilities(true).replace(
                "cluck_hunt\",\"rotation.v2.right_of_way",
                "right_of_way\",\"rotation.v2.cluck_hunt"
            ))
            .is_err()
        );
        assert!(
            decode_capabilities(&capabilities(true).replace("roady-rules.v3", "roady-rules.v2"))
                .is_err()
        );
    }

    #[test]
    fn proof_and_hex_decoding_reject_aliases() {
        assert!(exact_hex32(&"00".repeat(32)).is_ok());
        assert!(exact_hex32(&"AA".repeat(32)).is_err());
        assert!(decode_base64url_32(&base64url(&[7; 32])).is_ok());
        let mut alias = base64url(&[7; 32]);
        alias.pop();
        alias.push('B');
        assert!(decode_base64url_32(&alias).is_err());
    }

    #[test]
    fn sticky_failure_never_reopens_ranked() {
        let mut client = RankedV3Client {
            phase: RankedV3Phase::Ready,
            capability_admitted: true,
            ..default()
        };
        assert!(client.request_ranked_start(Conduct::CluckHunt));
        client.fail("bad response");
        client.phase = RankedV3Phase::Ready;
        assert!(!client.ranked_available());
        assert!(!client.request_ranked_start(Conduct::RightOfWay));
    }

    #[test]
    fn both_conduct_bodies_have_exact_worker_field_sets_and_drowned() {
        let common = TerminalPackage {
            session_id: "S03".into(),
            proof: base64url(&[1; 32]),
            category: v3::CLUCK_HUNT_CATEGORY.into(),
            schedule_hash: [2; 32],
            seed_commitment: [3; 32],
            final_root: [4; 32],
            event_count: 1,
            ledger_bytes: vec![1],
            evidence_hash: [5; 32],
            name: "AAA".into(),
            terminal: canonical::ConductTerminal::CluckHunt(canonical::CluckTerminal {
                reason: v3::TerminalReason::Drowned,
                total: 42,
                chickens: 35,
                coins: 7,
                objective_completed: true,
                max_combo: 5,
                duration_ms: 60_000,
                remaining_ms: 5_000,
                build: "dev".into(),
                platform: v3::Platform::Web,
            }),
        };
        let cluck = score_body(&common);
        assert_eq!(cluck["gameOverReason"], "drowned");
        assert_eq!(cluck.as_object().unwrap().len(), 25);
        let mut row = common.clone();
        row.category = v3::RIGHT_OF_WAY_CATEGORY.into();
        row.terminal = canonical::ConductTerminal::RightOfWay(canonical::RightOfWayTerminal {
            reason: v3::TerminalReason::Drowned,
            total: 17,
            accumulator: 17,
            premium_bps: 9000,
            packages_delivered: 3,
            courtesy_count: 2,
            animal_hits: 1,
            max_delivery_chain: 3,
            objective_completed: true,
            duration_ms: 60_000,
            remaining_ms: 5_000,
            build: "dev".into(),
            platform: v3::Platform::Web,
        });
        let body = score_body(&row);
        assert_eq!(body["signedAccumulator"], "17");
        assert_eq!(body.as_object().unwrap().len(), 28);
    }

    #[test]
    fn casual_has_no_terminal_package_or_write_path() {
        let rules = ActiveRunRules::default();
        assert_eq!(rules.competition, crate::game_modes::Competition::Casual);
        assert!(
            terminal_package(
                &rules,
                &FinalGameOverSnapshot::default(),
                &V3LedgerState::default(),
                "AAA"
            )
            .is_err()
        );
    }
}
