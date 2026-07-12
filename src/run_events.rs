//! Deterministic mid-run event scheduling and presentation.
//!
//! A fresh round receives two modifier-aware events. They occupy fixed,
//! non-overlapping eight-second windows on the active-play clock owned by
//! [`Difficulty`]. Pause entries preserve the plan and active event; only the
//! short-lived banner is recreated when `Playing` is entered again.

use bevy::audio::{AudioPlayer, AudioSource, PlaybackSettings, Volume};
use bevy::prelude::*;
use bevy::text::FontSize;

use crate::difficulty::Difficulty;
use crate::game::SpawnSet;
use crate::game::resources::RoundActive;
use crate::game::state::GameState;
use crate::modifiers::{ActiveModifier, ModifierKind};

const FIRST_EVENT_START: f32 = 15.0;
const FIRST_EVENT_END: f32 = 23.0;
const SECOND_EVENT_START: f32 = 40.0;
const SECOND_EVENT_END: f32 = 48.0;
const EVENT_STING_VOLUME: f32 = 0.35;

/// A temporary ruleset applied during one scheduled mid-run event.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum EventKind {
    TrafficSurge,
    ChickenBurst,
    ComboFrenzy,
    CritterBurst,
}

impl EventKind {
    /// Stable player-facing banner label.
    pub(crate) const fn display_name(self) -> &'static str {
        match self {
            Self::TrafficSurge => "Traffic Surge",
            Self::ChickenBurst => "Chicken Burst",
            Self::ComboFrenzy => "Combo Frenzy",
            Self::CritterBurst => "Critter Burst",
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
    pub(crate) const fn chicken_score_bonus(self) -> u32 {
        match self {
            Self::ChickenBurst => 1,
            _ => 0,
        }
    }

    /// Multiplier for the target penalty-critter population.
    pub(crate) const fn critter_count_multiplier(self) -> usize {
        match self {
            Self::CritterBurst => 2,
            _ => 1,
        }
    }

    /// Multiplier applied only to score above a combo's base point.
    pub(crate) const fn combo_bonus_multiplier(self) -> u32 {
        match self {
            Self::ComboFrenzy => 2,
            _ => 1,
        }
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

    pub(crate) const fn chicken_target(&self, base: usize) -> usize {
        match self.0 {
            Some(kind) => kind.chicken_target(base),
            None => base,
        }
    }

    pub(crate) const fn chicken_score_bonus(&self) -> u32 {
        match self.0 {
            Some(kind) => kind.chicken_score_bonus(),
            None => 0,
        }
    }

    pub(crate) const fn critter_count_multiplier(&self) -> usize {
        match self.0 {
            Some(kind) => kind.critter_count_multiplier(),
            None => 1,
        }
    }

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
        match modifier {
            // Standard has no same-flavor exclusion; rotate this pair so its
            // rounds also contribute to deterministic event reachability.
            ModifierKind::Standard => Self {
                first: EventKind::TrafficSurge,
                second: EventKind::CritterBurst,
            },
            ModifierKind::RushHour => Self {
                first: EventKind::ChickenBurst,
                second: EventKind::ComboFrenzy,
            },
            ModifierKind::ChickenFrenzy => Self {
                first: EventKind::CritterBurst,
                second: EventKind::TrafficSurge,
            },
            ModifierKind::Stampede => Self {
                first: EventKind::ComboFrenzy,
                second: EventKind::ChickenBurst,
            },
            ModifierKind::GlassCannon => Self {
                first: EventKind::CritterBurst,
                second: EventKind::TrafficSurge,
            },
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
            // Chaining guarantees the reader sees starts written by this
            // frame's transition and produces one sting for each message.
            .add_systems(
                Update,
                (tick_events, play_event_sting)
                    .chain()
                    .run_if(in_state(GameState::Playing)),
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

fn spawn_event_banner(mut commands: Commands) {
    commands
        .spawn((
            Node {
                position_type: PositionType::Absolute,
                top: px(104.0),
                left: px(0.0),
                width: Val::Percent(100.0),
                justify_content: JustifyContent::Center,
                ..default()
            },
            Visibility::Hidden,
            EventBannerRoot,
        ))
        .with_child((
            Text::new(""),
            TextFont {
                font_size: FontSize::Px(34.0),
                ..default()
            },
            TextColor(Color::WHITE),
            Node {
                padding: UiRect::axes(px(14.0), px(7.0)),
                ..default()
            },
            BackgroundColor(Color::srgba(0.015, 0.02, 0.035, 0.78)),
            EventBannerText,
        ));
}

fn tick_events(
    difficulty: Res<Difficulty>,
    plan: Res<EventPlan>,
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
    if let Some(kind) = active.0 {
        for (mut text, mut color) in &mut labels {
            **text = kind.display_name().to_owned();
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
    fn inactive_event_accessors_are_strictly_neutral() {
        let active = ActiveEvent(None);
        assert_eq!(active.traffic_count_multiplier(), 1);
        assert_eq!(active.traffic_speed_multiplier(), 1.0);
        assert_eq!(active.chicken_target(0), 0);
        assert_eq!(active.chicken_target(17), 17);
        assert_eq!(active.chicken_score_bonus(), 0);
        assert_eq!(active.critter_count_multiplier(), 1);
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
            assert_eq!(active.chicken_target(11), kind.chicken_target(11));
            assert_eq!(active.chicken_score_bonus(), kind.chicken_score_bonus());
            assert_eq!(
                active.critter_count_multiplier(),
                kind.critter_count_multiplier()
            );
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
        assert_eq!(traffic.critter_count_multiplier(), 1);
        assert_eq!(traffic.combo_bonus_multiplier(), 1);

        let chicken = EventKind::ChickenBurst;
        assert_eq!(chicken.traffic_count_multiplier(), 1);
        assert_eq!(chicken.traffic_speed_multiplier(), 1.0);
        assert_eq!(chicken.chicken_target(11), 22);
        assert_eq!(chicken.chicken_score_bonus(), 1);
        assert_eq!(chicken.critter_count_multiplier(), 1);
        assert_eq!(chicken.combo_bonus_multiplier(), 1);

        let combo = EventKind::ComboFrenzy;
        assert_eq!(combo.traffic_count_multiplier(), 1);
        assert_eq!(combo.traffic_speed_multiplier(), 1.0);
        assert_eq!(combo.chicken_target(11), 11);
        assert_eq!(combo.chicken_score_bonus(), 0);
        assert_eq!(combo.critter_count_multiplier(), 1);
        assert_eq!(combo.combo_bonus_multiplier(), 2);

        let critter = EventKind::CritterBurst;
        assert_eq!(critter.traffic_count_multiplier(), 1);
        assert_eq!(critter.traffic_speed_multiplier(), 1.0);
        assert_eq!(critter.chicken_target(11), 11);
        assert_eq!(critter.chicken_score_bonus(), 0);
        assert_eq!(critter.critter_count_multiplier(), 2);
        assert_eq!(critter.combo_bonus_multiplier(), 1);
    }

    #[test]
    fn labels_and_colors_are_distinct_and_stable() {
        assert_eq!(EventKind::TrafficSurge.display_name(), "Traffic Surge");
        assert_eq!(EventKind::ChickenBurst.display_name(), "Chicken Burst");
        assert_eq!(EventKind::ComboFrenzy.display_name(), "Combo Frenzy");
        assert_eq!(EventKind::CritterBurst.display_name(), "Critter Burst");

        for (index, left) in ALL_EVENTS.into_iter().enumerate() {
            for right in ALL_EVENTS.into_iter().skip(index + 1) {
                assert_ne!(left.display_name(), right.display_name());
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
