//! Deterministic mid-run event scheduling and presentation.
//!
//! A fresh round receives two modifier-aware events. They occupy fixed,
//! non-overlapping eight-second windows on the active-play clock owned by
//! [`Difficulty`]. Pause entries preserve the plan and active event; only the
//! short-lived banner is recreated when `Playing` is entered again.

use bevy::audio::{AudioPlayer, AudioSource, PlaybackSettings, Volume};
use bevy::prelude::*;
use bevy::text::FontSize;

use crate::audio::AudioBaseGain;
use crate::difficulty::Difficulty;
use crate::game::resources::{RoundActive, not_drowning};
use crate::game::state::GameState;
use crate::game::{SpawnSet, TouchStateSet};
use crate::modifiers::{ActiveModifier, ModifierKind};
use crate::touch::{
    TOUCH_EVENT_HEIGHT, TOUCH_EVENT_LEFT, TOUCH_EVENT_TOP, TOUCH_EVENT_WIDTH, TouchControlsActive,
};

const FIRST_EVENT_START: f32 = 15.0;
const FIRST_EVENT_END: f32 = 23.0;
const SECOND_EVENT_START: f32 = 40.0;
const SECOND_EVENT_END: f32 = 48.0;
const EVENT_DURATION_SECONDS: f32 = 8.0;
const EVENT_BAR_SEGMENTS: u8 = 8;
const EVENT_STING_VOLUME: f32 = 0.35;

// The objective and combo own centered vertical bands; the event uses the next
// band down but is right-aligned to leave the car readable. These dimensions
// model the painted panels (not transparent flex wrappers) and are shared by
// the active-layout audit below.
#[cfg(test)]
const OBJECTIVE_TOP: f32 = 54.0;
#[cfg(test)]
const OBJECTIVE_MAX_WIDTH: f32 = 420.0;
#[cfg(test)]
const OBJECTIVE_PANEL_HEIGHT: f32 = 38.0;
#[cfg(test)]
const COMBO_TOP: f32 = 98.0;
#[cfg(test)]
const COMBO_PANEL_WIDTH: f32 = 144.0;
#[cfg(test)]
const COMBO_PANEL_HEIGHT: f32 = 80.0;
const EVENT_TOP: f32 = 204.0;
const EVENT_RIGHT: f32 = 16.0;
const EVENT_PANEL_WIDTH: f32 = 300.0;
const EVENT_PANEL_HEIGHT: f32 = 64.0;
#[cfg(test)]
const MINIMAP_TOP: f32 = 62.0;
#[cfg(test)]
const MINIMAP_RIGHT: f32 = 72.0;
#[cfg(test)]
const MINIMAP_PANEL_SIZE: f32 = 136.0;
// Conservative screen-space readability budgets for externally rendered car
// and health content. The event clears the car horizontally and health
// vertically even in the 844x390 landscape viewport.
#[cfg(test)]
const CAR_CLEAR_HALF_WIDTH: f32 = 100.0;
#[cfg(test)]
const CAR_CLEAR_TOP: f32 = 145.0;
#[cfg(test)]
const CAR_CLEAR_BOTTOM: f32 = 270.0;
#[cfg(test)]
const HEALTH_PANEL_WIDTH: f32 = 256.0;
#[cfg(test)]
const HEALTH_CLEAR_TOP_SHORT: f32 = 300.0;

#[cfg(test)]
#[derive(Clone, Copy, Debug, PartialEq)]
struct UiBounds {
    left: f32,
    top: f32,
    right: f32,
    bottom: f32,
}

#[cfg(test)]
fn bounds_overlap(a: UiBounds, b: UiBounds) -> bool {
    a.left < b.right && a.right > b.left && a.top < b.bottom && a.bottom > b.top
}

#[cfg(test)]
fn centered_bounds(viewport_width: f32, top: f32, width: f32, height: f32) -> UiBounds {
    let width = width.min(viewport_width);
    let left = (viewport_width - width) * 0.5;
    UiBounds {
        left,
        top,
        right: left + width,
        bottom: top + height,
    }
}

/// Painted bounds for every simultaneously active status panel plus the
/// externally owned minimap. Keeping this combined model in one place catches
/// cross-feature regressions that pairwise owner tests cannot.
#[cfg(test)]
fn active_hud_bounds(viewport_width: f32) -> [UiBounds; 4] {
    let minimap_right = viewport_width - MINIMAP_RIGHT;
    [
        centered_bounds(
            viewport_width,
            OBJECTIVE_TOP,
            OBJECTIVE_MAX_WIDTH,
            OBJECTIVE_PANEL_HEIGHT,
        ),
        centered_bounds(
            viewport_width,
            COMBO_TOP,
            COMBO_PANEL_WIDTH,
            COMBO_PANEL_HEIGHT,
        ),
        UiBounds {
            left: viewport_width - EVENT_RIGHT - EVENT_PANEL_WIDTH,
            top: EVENT_TOP,
            right: viewport_width - EVENT_RIGHT,
            bottom: EVENT_TOP + EVENT_PANEL_HEIGHT,
        },
        UiBounds {
            left: minimap_right - MINIMAP_PANEL_SIZE,
            top: MINIMAP_TOP,
            right: minimap_right,
            bottom: MINIMAP_TOP + MINIMAP_PANEL_SIZE,
        },
    ]
}

/// A temporary ruleset applied during one scheduled mid-run event.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum EventKind {
    TrafficSurge,
    ChickenBurst,
    ComboFrenzy,
    CritterBurst,
}

impl EventKind {
    /// Engine-independent stable ID from the shared scoring-rules crate.
    pub(crate) const fn rules_id(self) -> roady_score_rules::EventId {
        match self {
            Self::TrafficSurge => roady_score_rules::EventId::TrafficSurge,
            Self::ChickenBurst => roady_score_rules::EventId::ChickenBurst,
            Self::ComboFrenzy => roady_score_rules::EventId::ComboFrenzy,
            Self::CritterBurst => roady_score_rules::EventId::CritterBurst,
        }
    }

    const fn from_rules_id(id: roady_score_rules::EventId) -> Self {
        match id {
            roady_score_rules::EventId::TrafficSurge => Self::TrafficSurge,
            roady_score_rules::EventId::ChickenBurst => Self::ChickenBurst,
            roady_score_rules::EventId::ComboFrenzy => Self::ComboFrenzy,
            roady_score_rules::EventId::CritterBurst => Self::CritterBurst,
        }
    }

    /// Stable player-facing banner label.
    pub(crate) const fn display_name(self) -> &'static str {
        match self {
            Self::TrafficSurge => "Traffic Surge",
            Self::ChickenBurst => "Chicken Burst",
            Self::ComboFrenzy => "Combo Frenzy",
            Self::CritterBurst => "Critter Burst",
        }
    }

    /// A short ASCII signature so event kinds are not communicated by color
    /// alone. ASCII is used deliberately so the cue survives font fallback.
    pub(crate) const fn semantic_signature(self) -> &'static str {
        match self {
            Self::TrafficSurge => ">> TRAFFIC",
            Self::ChickenBurst => "+ CHICKENS",
            Self::ComboFrenzy => "x2 COMBO",
            Self::CritterBurst => "** CRITTERS",
        }
    }

    /// Presentation-only accent color.
    pub(crate) const fn color(self) -> Color {
        match self {
            Self::TrafficSurge => Color::srgb(1.0, 0.20, 0.08),
            Self::ChickenBurst => Color::srgb(1.0, 0.82, 0.08),
            Self::ComboFrenzy => Color::srgb(0.72, 0.28, 1.0),
            Self::CritterBurst => Color::srgb(0.25, 0.92, 0.48),
        }
    }

    /// Multiplier for the target oncoming-traffic population.
    pub(crate) const fn traffic_count_multiplier(self) -> usize {
        match self {
            Self::TrafficSurge => 2,
            _ => 1,
        }
    }

    /// Multiplier for oncoming-traffic speed.
    pub(crate) const fn traffic_speed_multiplier(self) -> f32 {
        match self {
            Self::TrafficSurge => 1.25,
            _ => 1.0,
        }
    }

    /// Target chicken population for a supplied baseline.
    pub(crate) const fn chicken_target(self, base: usize) -> usize {
        match self {
            Self::ChickenBurst => base.saturating_mul(2),
            _ => base,
        }
    }

    /// Extra direct chicken-score points per hit.
    #[cfg(test)]
    pub(crate) const fn chicken_score_bonus(self) -> u32 {
        self.rules_id().chicken_score_bonus()
    }

    /// Multiplier applied only to score above a combo's base point.
    #[cfg(test)]
    pub(crate) const fn combo_bonus_multiplier(self) -> u32 {
        self.rules_id().combo_bonus_multiplier()
    }
}

/// Event currently affecting gameplay. `None` is deliberately a neutral
/// ruleset, allowing consumers to use these accessors without branching.
#[derive(Resource, Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ActiveEvent(pub Option<EventKind>);

impl ActiveEvent {
    pub(crate) const fn traffic_count_multiplier(&self) -> usize {
        match self.0 {
            Some(kind) => kind.traffic_count_multiplier(),
            None => 1,
        }
    }

    pub(crate) const fn traffic_speed_multiplier(&self) -> f32 {
        match self.0 {
            Some(kind) => kind.traffic_speed_multiplier(),
            None => 1.0,
        }
    }

    #[cfg(test)]
    pub(crate) const fn chicken_score_bonus(&self) -> u32 {
        match self.0 {
            Some(kind) => kind.chicken_score_bonus(),
            None => 0,
        }
    }

    #[cfg(test)]
    pub(crate) const fn combo_bonus_multiplier(&self) -> u32 {
        match self.0 {
            Some(kind) => kind.combo_bonus_multiplier(),
            None => 1,
        }
    }
}

/// The ordered event pair assigned to a round.
#[derive(Resource, Clone, Copy, Debug, PartialEq, Eq)]
pub struct EventPlan {
    pub(crate) first: EventKind,
    pub(crate) second: EventKind,
}

impl Default for EventPlan {
    fn default() -> Self {
        Self::for_kind(ModifierKind::Standard)
    }
}

impl EventPlan {
    /// Choose a repeatable pair from the active round modifier while excluding
    /// the event with that modifier's same gameplay flavor.
    pub(crate) const fn for_modifier(modifier: ActiveModifier) -> Self {
        Self::for_kind(modifier.0)
    }

    const fn for_kind(modifier: ModifierKind) -> Self {
        let [first, second] = roady_score_rules::reachable_events(modifier.rules_id());
        Self {
            first: EventKind::from_rules_id(first),
            second: EventKind::from_rules_id(second),
        }
    }
}

/// Emitted exactly once when a scheduled event becomes active.
#[derive(Message, Clone, Copy, Debug, PartialEq, Eq)]
pub struct RoundEventStarted(pub(crate) EventKind);

#[derive(Resource)]
struct EventAudio {
    click: Handle<AudioSource>,
}

impl FromWorld for EventAudio {
    fn from_world(world: &mut World) -> Self {
        let asset_server = world.resource::<AssetServer>();
        Self {
            click: asset_server.load("audio/click.wav"),
        }
    }
}

#[derive(Component)]
struct EventBannerRoot;

#[derive(Component)]
struct EventBannerText;

/// Return the single event scheduled at `elapsed`, if any. The half-open
/// ranges make all four boundaries unambiguous.
const fn scheduled_event(elapsed: f32, plan: EventPlan) -> Option<EventKind> {
    if elapsed >= FIRST_EVENT_START && elapsed < FIRST_EVENT_END {
        Some(plan.first)
    } else if elapsed >= SECOND_EVENT_START && elapsed < SECOND_EVENT_END {
        Some(plan.second)
    } else {
        None
    }
}

/// Presentation timing derived exclusively from the active-play clock. The
/// discrete bar and whole-second countdown provide duration clarity without
/// motion, pulsing, or wall-clock timers, so they remain stable while paused.
#[derive(Clone, Copy, Debug, PartialEq)]
struct EventDurationState {
    remaining_seconds: u32,
    // Retained as an exact normalized duration value for consumers/tests even
    // though the current banner renders the equivalent discrete segments.
    #[allow(dead_code)]
    remaining_fraction: f32,
    filled_bar_segments: u8,
}

/// Return display progress for either event window. Active windows never show
/// zero seconds or an empty bar: their half-open end boundary becomes inactive.
fn event_duration_state(elapsed: f32) -> Option<EventDurationState> {
    let end = if elapsed >= FIRST_EVENT_START && elapsed < FIRST_EVENT_END {
        FIRST_EVENT_END
    } else if elapsed >= SECOND_EVENT_START && elapsed < SECOND_EVENT_END {
        SECOND_EVENT_END
    } else {
        return None;
    };

    let remaining = end - elapsed;
    let remaining_fraction = remaining / EVENT_DURATION_SECONDS;
    Some(EventDurationState {
        remaining_seconds: remaining.ceil() as u32,
        remaining_fraction,
        filled_bar_segments: (remaining_fraction * f32::from(EVENT_BAR_SEGMENTS)).ceil() as u8,
    })
}

fn event_banner_text(kind: EventKind, duration: EventDurationState, touch_active: bool) -> String {
    let filled = usize::from(duration.filled_bar_segments);
    let empty = usize::from(EVENT_BAR_SEGMENTS - duration.filled_bar_segments);
    if touch_active {
        format!(
            "{}  [{}{}] {}s",
            kind.display_name(),
            "=".repeat(filled),
            "-".repeat(empty),
            duration.remaining_seconds,
        )
    } else {
        format!(
            "{}  {}\n[{}{}] {}s remaining",
            kind.semantic_signature(),
            kind.display_name(),
            "=".repeat(filled),
            "-".repeat(empty),
            duration.remaining_seconds,
        )
    }
}

/// Pure fresh-entry decision used by the setup system. A pause resume returns
/// `None`, preserving both resources; a fresh entry returns its plan together
/// with an explicitly cleared active event.
const fn fresh_event_setup(
    round_active: bool,
    modifier: ActiveModifier,
) -> Option<(EventPlan, ActiveEvent)> {
    if round_active {
        None
    } else {
        Some((EventPlan::for_modifier(modifier), ActiveEvent(None)))
    }
}

pub struct RunEventsPlugin;

impl Plugin for RunEventsPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<EventPlan>()
            .init_resource::<ActiveEvent>()
            .init_resource::<EventAudio>()
            .add_message::<RoundEventStarted>()
            .add_systems(OnEnter(GameState::Playing), arm_event_plan.in_set(SpawnSet))
            // The presentation root is intentionally recreated on every
            // Playing entry, including pause resumes, and always starts hidden.
            // Spawn after SpawnSet so a fresh setup is already complete.
            .add_systems(
                OnEnter(GameState::Playing),
                spawn_event_banner.after(SpawnSet),
            )
            .add_systems(
                OnExit(GameState::Playing),
                despawn_marker::<EventBannerRoot>,
            )
            // Chaining guarantees the sting reader sees starts written by
            // this frame's transition. `difficulty::tick_difficulty` has no
            // exported system/set ordering point, so explicit clock-before-
            // event ordering cannot be expressed from the two files this
            // feature is allowed to change; the schedule continues to read
            // the owner's active-play clock without altering event timing.
            .add_systems(
                Update,
                (tick_events, play_event_sting)
                    .chain()
                    .after(TouchStateSet)
                    .run_if(in_state(GameState::Playing))
                    .run_if(not_drowning),
            )
            .add_systems(
                Update,
                update_event_banner_layout
                    .after(TouchStateSet)
                    .run_if(in_state(GameState::Playing))
                    .run_if(not_drowning),
            );
    }
}

fn arm_event_plan(
    round_active: Res<RoundActive>,
    modifier: Res<ActiveModifier>,
    mut plan: ResMut<EventPlan>,
    mut active: ResMut<ActiveEvent>,
) {
    let Some((fresh_plan, cleared_event)) = fresh_event_setup(round_active.0, *modifier) else {
        return;
    };
    *plan = fresh_plan;
    *active = cleared_event;
}

fn spawn_event_banner(mut commands: Commands, touch: Res<TouchControlsActive>) {
    commands
        .spawn((
            event_banner_root_node(touch.0),
            Visibility::Hidden,
            EventBannerRoot,
        ))
        .with_child((
            Text::new(""),
            event_banner_font(touch.0),
            TextColor(Color::WHITE),
            event_banner_panel_node(touch.0),
            BackgroundColor(Color::srgba(0.015, 0.02, 0.035, 0.78)),
            EventBannerText,
        ));
}

fn event_banner_root_node(touch_active: bool) -> Node {
    if touch_active {
        Node {
            position_type: PositionType::Absolute,
            top: px(TOUCH_EVENT_TOP),
            left: px(TOUCH_EVENT_LEFT),
            ..default()
        }
    } else {
        Node {
            position_type: PositionType::Absolute,
            top: px(EVENT_TOP),
            right: px(EVENT_RIGHT),
            ..default()
        }
    }
}

fn event_banner_panel_node(touch_active: bool) -> Node {
    Node {
        width: px(if touch_active {
            TOUCH_EVENT_WIDTH
        } else {
            EVENT_PANEL_WIDTH
        }),
        height: px(if touch_active {
            TOUCH_EVENT_HEIGHT
        } else {
            EVENT_PANEL_HEIGHT
        }),
        padding: if touch_active {
            UiRect::axes(px(6.0), px(4.0))
        } else {
            UiRect::axes(px(12.0), px(6.0))
        },
        ..default()
    }
}

fn event_banner_font(touch_active: bool) -> TextFont {
    TextFont {
        font_size: FontSize::Px(if touch_active { 12.0 } else { 20.0 }),
        ..default()
    }
}

fn update_event_banner_layout(
    touch: Res<TouchControlsActive>,
    mut roots: Query<&mut Node, (With<EventBannerRoot>, Without<EventBannerText>)>,
    mut labels: Query<(&mut Node, &mut TextFont), With<EventBannerText>>,
) {
    if !touch.is_changed() {
        return;
    }
    for mut node in &mut roots {
        *node = event_banner_root_node(touch.0);
    }
    for (mut node, mut font) in &mut labels {
        *node = event_banner_panel_node(touch.0);
        *font = event_banner_font(touch.0);
    }
}

fn tick_events(
    difficulty: Res<Difficulty>,
    plan: Res<EventPlan>,
    touch: Res<TouchControlsActive>,
    mut active: ResMut<ActiveEvent>,
    mut starts: MessageWriter<RoundEventStarted>,
    mut roots: Query<&mut Visibility, With<EventBannerRoot>>,
    mut labels: Query<(&mut Text, &mut TextColor), With<EventBannerText>>,
) {
    let next = scheduled_event(difficulty.elapsed, *plan);
    if next != active.0 {
        active.0 = next;
        if let Some(kind) = next {
            starts.write(RoundEventStarted(kind));
        }
    }

    for mut visibility in &mut roots {
        *visibility = if active.0.is_some() {
            Visibility::Visible
        } else {
            Visibility::Hidden
        };
    }
    if let (Some(kind), Some(duration)) = (active.0, event_duration_state(difficulty.elapsed)) {
        for (mut text, mut color) in &mut labels {
            **text = event_banner_text(kind, duration, touch.0);
            color.0 = kind.color();
        }
    }
}

fn play_event_sting(
    mut starts: MessageReader<RoundEventStarted>,
    audio: Res<EventAudio>,
    mut commands: Commands,
) {
    for _ in starts.read() {
        commands.spawn((
            AudioPlayer::new(audio.click.clone()),
            PlaybackSettings::DESPAWN
                .with_speed(1.15)
                .with_volume(Volume::Linear(EVENT_STING_VOLUME)),
            AudioBaseGain(EVENT_STING_VOLUME),
        ));
    }
}

fn despawn_marker<M: Component>(mut commands: Commands, query: Query<Entity, With<M>>) {
    for entity in &query {
        commands.entity(entity).despawn();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const ALL_EVENTS: [EventKind; 4] = [
        EventKind::TrafficSurge,
        EventKind::ChickenBurst,
        EventKind::ComboFrenzy,
        EventKind::CritterBurst,
    ];
    const ALL_MODIFIERS: [ModifierKind; 5] = [
        ModifierKind::Standard,
        ModifierKind::RushHour,
        ModifierKind::ChickenFrenzy,
        ModifierKind::Stampede,
        ModifierKind::GlassCannon,
    ];

    fn same_flavor(modifier: ModifierKind) -> Option<EventKind> {
        match modifier {
            ModifierKind::Standard => None,
            ModifierKind::RushHour => Some(EventKind::TrafficSurge),
            ModifierKind::ChickenFrenzy => Some(EventKind::ChickenBurst),
            ModifierKind::Stampede => Some(EventKind::CritterBurst),
            ModifierKind::GlassCannon => Some(EventKind::ComboFrenzy),
        }
    }

    #[test]
    fn desktop_objective_combo_event_and_minimap_bounds_are_disjoint() {
        // This audit models the unchanged desktop event placement. The shared
        // touch-only all-panel audit lives in touch.rs.
        for (viewport_width, viewport_height) in [(1440.0, 900.0)] {
            let bounds = active_hud_bounds(viewport_width);
            let [objective, combo, event, _minimap] = bounds;

            for (index, left) in bounds.into_iter().enumerate() {
                assert!(left.left >= 0.0 && left.right <= viewport_width);
                assert!(left.top >= 0.0 && left.bottom <= viewport_height);
                for right in bounds.into_iter().skip(index + 1) {
                    assert!(
                        !bounds_overlap(left, right),
                        "active HUD overlap at {viewport_width}x{viewport_height}: {left:?} vs {right:?}"
                    );
                }
            }

            // Objective and combo retain strict centered bands. The lower
            // right event band clears a conservative center-car rectangle and
            // the bottom-center health panel at both target resolutions.
            assert!(objective.bottom <= combo.top);
            assert!(combo.bottom <= event.top);
            let car = UiBounds {
                left: viewport_width * 0.5 - CAR_CLEAR_HALF_WIDTH,
                top: CAR_CLEAR_TOP,
                right: viewport_width * 0.5 + CAR_CLEAR_HALF_WIDTH,
                bottom: CAR_CLEAR_BOTTOM,
            };
            let health = UiBounds {
                left: (viewport_width - HEALTH_PANEL_WIDTH) * 0.5,
                top: HEALTH_CLEAR_TOP_SHORT.min(viewport_height - 90.0),
                right: (viewport_width + HEALTH_PANEL_WIDTH) * 0.5,
                bottom: viewport_height,
            };
            assert!(!bounds_overlap(event, car));
            assert!(!bounds_overlap(event, health));
        }
    }

    #[test]
    fn inactive_event_accessors_are_strictly_neutral() {
        let active = ActiveEvent(None);
        assert_eq!(active.traffic_count_multiplier(), 1);
        assert_eq!(active.traffic_speed_multiplier(), 1.0);
        assert_eq!(active.chicken_score_bonus(), 0);
        assert_eq!(active.combo_bonus_multiplier(), 1);
    }

    #[test]
    fn event_kinds_have_exact_flavor_tuning_and_neutral_unrelated_tuning() {
        for kind in ALL_EVENTS {
            let active = ActiveEvent(Some(kind));
            assert_eq!(
                active.traffic_count_multiplier(),
                kind.traffic_count_multiplier()
            );
            assert_eq!(
                active.traffic_speed_multiplier(),
                kind.traffic_speed_multiplier()
            );
            assert_eq!(active.chicken_score_bonus(), kind.chicken_score_bonus());
            assert_eq!(
                active.combo_bonus_multiplier(),
                kind.combo_bonus_multiplier()
            );
        }

        let traffic = EventKind::TrafficSurge;
        assert_eq!(traffic.traffic_count_multiplier(), 2);
        assert_eq!(traffic.traffic_speed_multiplier(), 1.25);
        assert_eq!(traffic.chicken_target(11), 11);
        assert_eq!(traffic.chicken_score_bonus(), 0);
        assert_eq!(traffic.combo_bonus_multiplier(), 1);

        let chicken = EventKind::ChickenBurst;
        assert_eq!(chicken.traffic_count_multiplier(), 1);
        assert_eq!(chicken.traffic_speed_multiplier(), 1.0);
        assert_eq!(chicken.chicken_target(11), 22);
        assert_eq!(chicken.chicken_score_bonus(), 1);
        assert_eq!(chicken.combo_bonus_multiplier(), 1);

        let combo = EventKind::ComboFrenzy;
        assert_eq!(combo.traffic_count_multiplier(), 1);
        assert_eq!(combo.traffic_speed_multiplier(), 1.0);
        assert_eq!(combo.chicken_target(11), 11);
        assert_eq!(combo.chicken_score_bonus(), 0);
        assert_eq!(combo.combo_bonus_multiplier(), 2);

        let critter = EventKind::CritterBurst;
        assert_eq!(critter.traffic_count_multiplier(), 1);
        assert_eq!(critter.traffic_speed_multiplier(), 1.0);
        assert_eq!(critter.chicken_target(11), 11);
        assert_eq!(critter.chicken_score_bonus(), 0);
        assert_eq!(critter.combo_bonus_multiplier(), 1);
    }

    #[test]
    fn labels_signatures_and_colors_are_distinct_and_stable() {
        assert_eq!(EventKind::TrafficSurge.display_name(), "Traffic Surge");
        assert_eq!(EventKind::ChickenBurst.display_name(), "Chicken Burst");
        assert_eq!(EventKind::ComboFrenzy.display_name(), "Combo Frenzy");
        assert_eq!(EventKind::CritterBurst.display_name(), "Critter Burst");
        assert_eq!(EventKind::TrafficSurge.semantic_signature(), ">> TRAFFIC");
        assert_eq!(EventKind::ChickenBurst.semantic_signature(), "+ CHICKENS");
        assert_eq!(EventKind::ComboFrenzy.semantic_signature(), "x2 COMBO");
        assert_eq!(EventKind::CritterBurst.semantic_signature(), "** CRITTERS");

        for (index, left) in ALL_EVENTS.into_iter().enumerate() {
            for right in ALL_EVENTS.into_iter().skip(index + 1) {
                assert_ne!(left.display_name(), right.display_name());
                assert_ne!(left.semantic_signature(), right.semantic_signature());
                assert_ne!(left.color(), right.color());
            }
        }
    }

    #[test]
    fn plans_exclude_same_flavor_and_reach_every_event_in_both_slots() {
        let mut reached_first = [false; 4];
        let mut reached_second = [false; 4];
        for modifier in ALL_MODIFIERS {
            let plan = EventPlan::for_modifier(ActiveModifier(modifier));
            assert_eq!(plan, EventPlan::for_modifier(ActiveModifier(modifier)));
            assert_ne!(plan.first, plan.second);
            assert_ne!(Some(plan.first), same_flavor(modifier));
            assert_ne!(Some(plan.second), same_flavor(modifier));
            let first_index = ALL_EVENTS
                .iter()
                .position(|kind| *kind == plan.first)
                .unwrap();
            let second_index = ALL_EVENTS
                .iter()
                .position(|kind| *kind == plan.second)
                .unwrap();
            reached_first[first_index] = true;
            reached_second[second_index] = true;
        }
        assert_eq!(reached_first, [true; 4]);
        assert_eq!(reached_second, [true; 4]);
    }

    #[test]
    fn schedule_uses_exact_half_open_eight_second_windows() {
        let plan = EventPlan {
            first: EventKind::TrafficSurge,
            second: EventKind::ComboFrenzy,
        };
        assert_eq!(scheduled_event(14.999, plan), None);
        assert_eq!(scheduled_event(15.0, plan), Some(plan.first));
        assert_eq!(scheduled_event(22.999, plan), Some(plan.first));
        assert_eq!(scheduled_event(23.0, plan), None);
        assert_eq!(scheduled_event(39.999, plan), None);
        assert_eq!(scheduled_event(40.0, plan), Some(plan.second));
        assert_eq!(scheduled_event(47.999, plan), Some(plan.second));
        assert_eq!(scheduled_event(48.0, plan), None);
        assert_eq!(scheduled_event(f32::NEG_INFINITY, plan), None);
        assert_eq!(scheduled_event(f32::INFINITY, plan), None);
        assert_eq!(scheduled_event(f32::NAN, plan), None);
    }

    #[test]
    fn duration_progress_has_exact_half_open_boundaries() {
        assert_eq!(event_duration_state(14.999), None);
        assert_eq!(
            event_duration_state(15.0),
            Some(EventDurationState {
                remaining_seconds: 8,
                remaining_fraction: 1.0,
                filled_bar_segments: 8,
            })
        );
        assert_eq!(event_duration_state(15.001).unwrap().remaining_seconds, 8);
        assert_eq!(event_duration_state(15.001).unwrap().filled_bar_segments, 8);
        assert_eq!(
            event_duration_state(16.0),
            Some(EventDurationState {
                remaining_seconds: 7,
                remaining_fraction: 0.875,
                filled_bar_segments: 7,
            })
        );
        assert_eq!(
            event_duration_state(22.0),
            Some(EventDurationState {
                remaining_seconds: 1,
                remaining_fraction: 0.125,
                filled_bar_segments: 1,
            })
        );
        assert_eq!(event_duration_state(22.999).unwrap().remaining_seconds, 1);
        assert_eq!(event_duration_state(22.999).unwrap().filled_bar_segments, 1);
        assert_eq!(event_duration_state(23.0), None);

        assert_eq!(event_duration_state(39.999), None);
        assert_eq!(
            event_duration_state(40.0),
            Some(EventDurationState {
                remaining_seconds: 8,
                remaining_fraction: 1.0,
                filled_bar_segments: 8,
            })
        );
        assert_eq!(event_duration_state(47.999).unwrap().remaining_seconds, 1);
        assert_eq!(event_duration_state(47.999).unwrap().filled_bar_segments, 1);
        assert_eq!(event_duration_state(48.0), None);
        assert_eq!(event_duration_state(f32::NEG_INFINITY), None);
        assert_eq!(event_duration_state(f32::INFINITY), None);
        assert_eq!(event_duration_state(f32::NAN), None);
    }

    #[test]
    fn duration_banner_is_discrete_and_explicit() {
        let start = event_duration_state(FIRST_EVENT_START).unwrap();
        assert_eq!(
            event_banner_text(EventKind::TrafficSurge, start, false),
            ">> TRAFFIC  Traffic Surge\n[========] 8s remaining"
        );

        let final_second = event_duration_state(FIRST_EVENT_END - 0.5).unwrap();
        assert_eq!(
            event_banner_text(EventKind::TrafficSurge, final_second, false),
            ">> TRAFFIC  Traffic Surge\n[=-------] 1s remaining"
        );
    }

    #[test]
    fn touch_duration_banner_is_one_line_with_bar() {
        let start = event_duration_state(FIRST_EVENT_START).unwrap();
        let text = event_banner_text(EventKind::TrafficSurge, start, true);
        assert_eq!(text, "Traffic Surge  [========] 8s");
        assert!(!text.contains('\n'));
    }

    #[test]
    fn fresh_guard_arms_and_clears_only_inactive_rounds() {
        for modifier in ALL_MODIFIERS {
            assert_eq!(
                fresh_event_setup(false, ActiveModifier(modifier)),
                Some((
                    EventPlan::for_modifier(ActiveModifier(modifier)),
                    ActiveEvent(None),
                ))
            );
            assert_eq!(fresh_event_setup(true, ActiveModifier(modifier)), None);
        }
    }
}
