//! Persistent player settings and the Menu/Paused settings overlay.
//!
//! Reduced motion is persisted here as a foundation setting. Gameplay and
//! presentation systems do not consume it yet.

use bevy::{prelude::*, text::FontSize, window::PrimaryWindow};

use crate::game::{KeyboardStateSet, RestartRequested, TouchStateSet, state::GameState};
use crate::palette;
use crate::touch::TouchControlsActive;

#[cfg(target_arch = "wasm32")]
const STORAGE_KEY: &str = "roady_car_settings";
#[cfg(target_arch = "wasm32")]
const LEGACY_MUTE_STORAGE_KEY: &str = "roady_car_audio_muted";
#[cfg(not(target_arch = "wasm32"))]
const FILE_PATH: &str = "settings.txt";

const VOLUME_STEP: u8 = 10;
const OPENER_LEFT: f32 = 0.67;
const OPENER_TOP: f32 = 0.03;
const OPENER_RIGHT: f32 = 0.97;
const OPENER_BOTTOM: f32 = 0.18;
const MAX_INITIALS: usize = 5;

const INITIAL_KEYS: [(KeyCode, char); 36] = [
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

/// Player preferences shared by settings UI and runtime systems.
#[derive(Resource, Clone, Debug, PartialEq, Eq)]
pub struct Settings {
    pub master_volume: u8,
    pub muted: bool,
    pub reduced_motion: bool,
    /// Empty means that the leaderboard should ask for a name. Persisted names
    /// are three to five ASCII letters/digits.
    pub leaderboard_initials: String,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            master_volume: 100,
            muted: false,
            reduced_motion: false,
            leaderboard_initials: String::new(),
        }
    }
}

impl Settings {
    /// Master gain in Bevy's linear 0..=1 range.
    pub fn master_gain(&self) -> f32 {
        clamp_volume(self.master_volume as i16) as f32 / 100.0
    }

    fn normalized(mut self) -> Self {
        self.master_volume = quantize_volume(self.master_volume);
        if !valid_initials(&self.leaderboard_initials) {
            self.leaderboard_initials.clear();
        }
        self
    }
}

/// Public gate for input systems that are able to opt out while this modal is
/// active. Existing game/touch state systems predate this resource.
#[derive(Resource, Default, Clone, Copy, Debug, PartialEq, Eq)]
pub struct SettingsOpen(pub bool);

#[derive(Resource, Default, Clone, Copy, Debug, PartialEq, Eq)]
struct SettingsSelection(SettingRow);

#[derive(Component)]
struct SettingsOverlayRoot;

#[derive(Component)]
struct SettingsOpenerRoot;

#[derive(Component)]
struct SettingsFooter;

#[derive(Component, Default, Clone, Copy, Debug, PartialEq, Eq)]
enum SettingRow {
    #[default]
    Volume,
    Mute,
    ReducedMotion,
    LeaderboardName,
    Back,
}

impl SettingRow {
    const ALL: [Self; 5] = [
        Self::Volume,
        Self::Mute,
        Self::ReducedMotion,
        Self::LeaderboardName,
        Self::Back,
    ];

    fn index(self) -> usize {
        match self {
            Self::Volume => 0,
            Self::Mute => 1,
            Self::ReducedMotion => 2,
            Self::LeaderboardName => 3,
            Self::Back => 4,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Adjustment {
    Left,
    Right,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Activation {
    None,
    Changed,
    Close,
}

pub struct SettingsPlugin;

impl Plugin for SettingsPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<Settings>()
            .init_resource::<SettingsOpen>()
            .init_resource::<SettingsSelection>()
            .add_systems(Startup, load_settings)
            // Resolve after legacy raw-touch state handling, then cancel that
            // pending transition when the tap belongs to this modal. Keyboard
            // modal keys are cleared before keyboard state systems see them.
            .add_systems(
                Update,
                settings_input.after(TouchStateSet).before(KeyboardStateSet),
            )
            .add_systems(Update, sync_settings_ui.after(settings_input))
            .add_systems(Update, update_settings_rows.after(sync_settings_ui))
            .add_systems(Update, persist_changed_settings.after(settings_input));
    }
}

fn clamp_volume(volume: i16) -> u8 {
    volume.clamp(0, 100) as u8
}

fn quantize_volume(volume: u8) -> u8 {
    let clamped = volume.min(100);
    ((clamped.saturating_add(VOLUME_STEP / 2)) / VOLUME_STEP * VOLUME_STEP).min(100)
}

fn adjusted_volume(volume: u8, adjustment: Adjustment) -> u8 {
    let volume = quantize_volume(volume) as i16;
    let delta = match adjustment {
        Adjustment::Left => -(VOLUME_STEP as i16),
        Adjustment::Right => VOLUME_STEP as i16,
    };
    clamp_volume(volume + delta)
}

fn move_selection(current: SettingRow, delta: i8) -> SettingRow {
    let len = SettingRow::ALL.len() as i8;
    let index = (current.index() as i8 + delta).rem_euclid(len) as usize;
    SettingRow::ALL[index]
}

fn adjust_selection(settings: &mut Settings, row: SettingRow, adjustment: Adjustment) -> bool {
    match row {
        SettingRow::Volume => {
            let next = adjusted_volume(settings.master_volume, adjustment);
            let changed = next != settings.master_volume;
            settings.master_volume = next;
            changed
        }
        SettingRow::Mute => {
            let next = adjustment == Adjustment::Right;
            let changed = next != settings.muted;
            settings.muted = next;
            changed
        }
        SettingRow::ReducedMotion => {
            let next = adjustment == Adjustment::Right;
            let changed = next != settings.reduced_motion;
            settings.reduced_motion = next;
            changed
        }
        SettingRow::LeaderboardName => {
            if adjustment == Adjustment::Left {
                let changed = !settings.leaderboard_initials.is_empty();
                settings.leaderboard_initials.clear();
                changed
            } else {
                false
            }
        }
        SettingRow::Back => false,
    }
}

fn append_initial(initials: &mut String, value: char) -> bool {
    if initials.len() >= MAX_INITIALS || !value.is_ascii_uppercase() && !value.is_ascii_digit() {
        return false;
    }
    initials.push(value);
    true
}

fn cycle_last_initial(initials: &mut String) -> bool {
    let Some(last) = initials.pop() else {
        return false;
    };
    let next = match last {
        'A'..='Y' | '0'..='8' => char::from_u32(last as u32 + 1).unwrap_or('A'),
        'Z' => '0',
        '9' => 'A',
        _ => 'A',
    };
    initials.push(next);
    true
}

fn activate_selection(settings: &mut Settings, row: SettingRow) -> Activation {
    match row {
        SettingRow::Volume => Activation::None,
        SettingRow::Mute => {
            settings.muted = !settings.muted;
            Activation::Changed
        }
        SettingRow::ReducedMotion => {
            settings.reduced_motion = !settings.reduced_motion;
            Activation::Changed
        }
        SettingRow::LeaderboardName => Activation::None,
        SettingRow::Back => Activation::Close,
    }
}

fn settings_context(state: GameState) -> bool {
    matches!(state, GameState::Menu | GameState::Paused)
}

fn clear_modal_keys(keys: &mut ButtonInput<KeyCode>) {
    for key in [
        KeyCode::ArrowUp,
        KeyCode::ArrowDown,
        KeyCode::ArrowLeft,
        KeyCode::ArrowRight,
        KeyCode::Enter,
        KeyCode::Space,
        KeyCode::Escape,
        KeyCode::Backspace,
    ] {
        keys.clear_just_pressed(key);
    }
    // This also consumes O/R/Q so typing a leaderboard name never leaks into
    // the Menu or Paused screen beneath the modal.
    for (key, _) in INITIAL_KEYS {
        keys.clear_just_pressed(key);
    }
}

fn normalized_touch(position: Vec2, window: &Window) -> Option<Vec2> {
    let size = Vec2::new(window.width(), window.height());
    if size.x <= 0.0 || size.y <= 0.0 || !size.is_finite() || !position.is_finite() {
        return None;
    }
    Some(Vec2::new(
        (position.x / size.x).clamp(0.0, 1.0),
        (position.y / size.y).clamp(0.0, 1.0),
    ))
}

fn opener_hit(position: Vec2) -> bool {
    (OPENER_LEFT..=OPENER_RIGHT).contains(&position.x)
        && (OPENER_TOP..=OPENER_BOTTOM).contains(&position.y)
}

fn touch_row(position: Vec2) -> Option<SettingRow> {
    match position.y {
        y if (0.20..0.33).contains(&y) => Some(SettingRow::Volume),
        y if (0.33..0.45).contains(&y) => Some(SettingRow::Mute),
        y if (0.45..0.57).contains(&y) => Some(SettingRow::ReducedMotion),
        y if (0.57..0.70).contains(&y) => Some(SettingRow::LeaderboardName),
        y if (0.70..0.84).contains(&y) => Some(SettingRow::Back),
        _ => None,
    }
}

fn apply_touch_row(
    position: Vec2,
    selection: &mut SettingsSelection,
    settings: &mut Settings,
) -> Activation {
    let Some(row) = touch_row(position) else {
        return Activation::None;
    };
    selection.0 = row;
    if row == SettingRow::Back {
        return Activation::Close;
    }
    if row == SettingRow::LeaderboardName {
        let changed = if position.x < 0.42 {
            adjust_selection(settings, row, Adjustment::Left)
        } else if position.x > 0.58 {
            if settings.leaderboard_initials.is_empty() {
                append_initial(&mut settings.leaderboard_initials, 'A')
            } else {
                cycle_last_initial(&mut settings.leaderboard_initials)
            }
        } else {
            append_initial(&mut settings.leaderboard_initials, 'A')
        };
        return if changed {
            Activation::Changed
        } else {
            Activation::None
        };
    }
    if position.x < 0.42 {
        if adjust_selection(settings, row, Adjustment::Left) {
            Activation::Changed
        } else {
            Activation::None
        }
    } else if position.x > 0.58 {
        if adjust_selection(settings, row, Adjustment::Right) {
            Activation::Changed
        } else {
            Activation::None
        }
    } else {
        activate_selection(settings, row)
    }
}

fn settings_input(
    mut keys: ResMut<ButtonInput<KeyCode>>,
    mut mouse: ResMut<ButtonInput<MouseButton>>,
    touches: Res<Touches>,
    windows: Query<&Window, With<PrimaryWindow>>,
    state: Res<State<GameState>>,
    mut settings: ResMut<Settings>,
    mut open: ResMut<SettingsOpen>,
    mut selection: ResMut<SettingsSelection>,
    mut touch_active: ResMut<TouchControlsActive>,
    mut restart: ResMut<RestartRequested>,
    mut next_state: ResMut<NextState<GameState>>,
) {
    let in_context = settings_context(*state.get());
    if open.0 && !in_context {
        open.0 = false;
        clear_modal_keys(&mut keys);
        return;
    }

    if !open.0 {
        if in_context && keys.just_pressed(KeyCode::KeyO) {
            open.0 = true;
            selection.0 = SettingRow::Volume;
            clear_modal_keys(&mut keys);
        }

        if in_context {
            if let Ok(window) = windows.single() {
                for touch in touches.iter_just_pressed() {
                    touch_active.0 = true;
                    if normalized_touch(touch.position(), window).is_some_and(opener_hit) {
                        open.0 = true;
                        selection.0 = SettingRow::Volume;
                        // TouchPlugin sees the same raw first-touch event. Its
                        // pending Menu/Paused action is canceled so no state
                        // hooks run and the underlying screen remains intact.
                        restart.0 = false;
                        next_state.reset();
                        break;
                    }
                }
                if !open.0 && mouse.just_pressed(MouseButton::Left) {
                    let clicked_opener = window
                        .cursor_position()
                        .and_then(|position| normalized_touch(position, window))
                        .is_some_and(opener_hit);
                    // Raw mouse input is owned only when it intersects the
                    // exact visible settings button bounds.
                    if clicked_opener {
                        open.0 = true;
                        selection.0 = SettingRow::Volume;
                        mouse.clear_just_pressed(MouseButton::Left);
                    }
                }
            }
        }
        if open.0 {
            clear_modal_keys(&mut keys);
        }
        return;
    }

    // O remains the shortcut except while the name row owns alphanumeric
    // input. Escape always closes.
    if keys.just_pressed(KeyCode::Escape)
        || (selection.0 != SettingRow::LeaderboardName && keys.just_pressed(KeyCode::KeyO))
    {
        open.0 = false;
        clear_modal_keys(&mut keys);
        return;
    }

    if keys.just_pressed(KeyCode::ArrowUp) {
        selection.0 = move_selection(selection.0, -1);
    } else if keys.just_pressed(KeyCode::ArrowDown) {
        selection.0 = move_selection(selection.0, 1);
    }

    let selected_row = selection.0;
    if keys.just_pressed(KeyCode::ArrowLeft) {
        adjust_selection(&mut settings, selected_row, Adjustment::Left);
    } else if keys.just_pressed(KeyCode::ArrowRight) {
        adjust_selection(&mut settings, selected_row, Adjustment::Right);
    }

    if selected_row == SettingRow::LeaderboardName {
        if keys.just_pressed(KeyCode::Backspace) {
            settings.leaderboard_initials.pop();
        }
        for (key, value) in INITIAL_KEYS {
            if keys.just_pressed(key) {
                append_initial(&mut settings.leaderboard_initials, value);
            }
        }
    }

    if keys.just_pressed(KeyCode::Enter) || keys.just_pressed(KeyCode::Space) {
        if activate_selection(&mut settings, selected_row) == Activation::Close {
            open.0 = false;
        }
    }

    if let Ok(window) = windows.single() {
        let mut handled_pointer = false;
        for touch in touches.iter_just_pressed() {
            touch_active.0 = true;
            let Some(position) = normalized_touch(touch.position(), window) else {
                continue;
            };
            handled_pointer = true;
            if apply_touch_row(position, &mut *selection, &mut *settings) == Activation::Close {
                open.0 = false;
                break;
            }
        }
        if open.0 && mouse.just_pressed(MouseButton::Left) {
            if let Some(position) = window
                .cursor_position()
                .and_then(|position| normalized_touch(position, window))
            {
                if touch_row(position).is_some() {
                    handled_pointer = true;
                    if apply_touch_row(position, &mut *selection, &mut *settings)
                        == Activation::Close
                    {
                        open.0 = false;
                    }
                    mouse.clear_just_pressed(MouseButton::Left);
                }
            }
        }
        if handled_pointer {
            // Raw Touches cannot be consumed, so cancel TouchPlugin's pending
            // transition and keep the modal over its underlying state.
            restart.0 = false;
            next_state.reset();
        }
    }

    clear_modal_keys(&mut keys);
}

fn sync_settings_ui(
    mut commands: Commands,
    state: Res<State<GameState>>,
    open: Res<SettingsOpen>,
    overlay: Query<Entity, With<SettingsOverlayRoot>>,
    opener: Query<Entity, With<SettingsOpenerRoot>>,
) {
    let show_opener = settings_context(*state.get()) && !open.0;
    if show_opener && opener.is_empty() {
        commands
            .spawn((
                Node {
                    position_type: PositionType::Absolute,
                    top: Val::Percent(OPENER_TOP * 100.0),
                    right: Val::Percent((1.0 - OPENER_RIGHT) * 100.0),
                    width: Val::Percent((OPENER_RIGHT - OPENER_LEFT) * 100.0),
                    height: Val::Percent((OPENER_BOTTOM - OPENER_TOP) * 100.0),
                    border: UiRect::all(px(2.0)),
                    border_radius: BorderRadius::all(px(9.0)),
                    align_items: AlignItems::Center,
                    justify_content: JustifyContent::Center,
                    ..default()
                },
                BackgroundColor(Color::srgba(0.035, 0.04, 0.055, 0.94)),
                BorderColor::all(palette::HUD_ACCENT),
                Text::new("⚙ SETTINGS"),
                TextFont {
                    font_size: FontSize::Px(16.0),
                    ..default()
                },
                TextColor(palette::HUD_ACCENT.into()),
            ))
            .insert(GlobalZIndex(80))
            .insert(SettingsOpenerRoot);
    } else if !show_opener {
        for entity in &opener {
            commands.entity(entity).despawn();
        }
    }

    if open.0 && overlay.is_empty() {
        spawn_overlay(&mut commands);
    } else if !open.0 {
        for entity in &overlay {
            commands.entity(entity).despawn();
        }
    }
}

fn spawn_overlay(commands: &mut Commands) {
    commands
        .spawn((
            Node {
                position_type: PositionType::Absolute,
                top: px(0.0),
                left: px(0.0),
                width: Val::Percent(100.0),
                height: Val::Percent(100.0),
                align_items: AlignItems::Center,
                justify_content: JustifyContent::Center,
                ..default()
            },
            BackgroundColor(Color::srgba(0.0, 0.0, 0.0, 0.82)),
            GlobalZIndex(100),
            SettingsOverlayRoot,
        ))
        .with_children(|root| {
            root.spawn((
                Node {
                    width: Val::Percent(88.0),
                    height: Val::Percent(88.0),
                    max_width: px(760.0),
                    padding: UiRect::all(px(14.0)),
                    flex_direction: FlexDirection::Column,
                    align_items: AlignItems::Stretch,
                    justify_content: JustifyContent::SpaceBetween,
                    ..default()
                },
                BackgroundColor(Color::srgba(0.035, 0.04, 0.055, 0.97)),
            ))
            .with_children(|panel| {
                panel.spawn((
                    Text::new("SETTINGS"),
                    TextFont {
                        font_size: FontSize::Px(34.0),
                        ..default()
                    },
                    TextColor(palette::HUD_ACCENT.into()),
                    Node {
                        align_self: AlignSelf::Center,
                        ..default()
                    },
                ));

                for row in SettingRow::ALL {
                    panel.spawn((
                        Node {
                            width: Val::Percent(100.0),
                            min_height: px(38.0),
                            padding: UiRect::axes(px(10.0), px(5.0)),
                            align_items: AlignItems::Center,
                            justify_content: JustifyContent::Center,
                            ..default()
                        },
                        BackgroundColor(Color::NONE),
                        Text::default(),
                        TextFont {
                            font_size: FontSize::Px(16.0),
                            ..default()
                        },
                        TextColor(palette::HUD_TEXT.into()),
                        row,
                    ));
                }

                panel.spawn((
                    Text::new("↑/↓ select • ←/→ change • Enter toggle • Esc/O back\nTouch: left/right to change"),
                    TextFont {
                        font_size: FontSize::Px(12.0),
                        ..default()
                    },
                    TextColor(Color::srgba(0.78, 0.8, 0.86, 1.0)),
                    Node {
                        align_self: AlignSelf::Center,
                        ..default()
                    },
                    SettingsFooter,
                ));
            });
        });
}

fn update_settings_rows(
    settings: Res<Settings>,
    selection: Res<SettingsSelection>,
    mut rows: Query<(&SettingRow, &mut Text, &mut TextColor, &mut BackgroundColor)>,
    mut footer: Query<&mut Text, (With<SettingsFooter>, Without<SettingRow>)>,
) {
    for (row, mut text, mut color, mut background) in &mut rows {
        **text = match row {
            SettingRow::Volume => format!("‹  Volume  {}%  ›", settings.master_volume),
            SettingRow::Mute => {
                format!("‹  Mute  {}  ›", if settings.muted { "On" } else { "Off" })
            }
            SettingRow::ReducedMotion => format!(
                "‹  Reduced Motion  {}  ›",
                if settings.reduced_motion { "On" } else { "Off" }
            ),
            SettingRow::LeaderboardName => format!(
                "‹ clear  Leaderboard Name  {}  edit ›",
                if settings.leaderboard_initials.is_empty() {
                    "—"
                } else {
                    &settings.leaderboard_initials
                }
            ),
            SettingRow::Back => "Back".to_string(),
        };
        let selected = *row == selection.0;
        color.0 = if selected {
            palette::HUD_ACCENT.into()
        } else {
            palette::HUD_TEXT.into()
        };
        background.0 = if selected {
            Color::srgba(0.18, 0.2, 0.25, 0.9)
        } else {
            Color::NONE
        };
    }
    for mut text in &mut footer {
        **text = if selection.0 == SettingRow::LeaderboardName {
            "Type A-Z/0-9 (3-5) • Backspace delete • ← clear\nTouch/click: center adds A • right cycles • left clears"
        } else {
            "↑/↓ select • ←/→ change • Enter toggle • Esc/O back\nTouch/click: row center toggles; sides change"
        }
        .to_string();
    }
}

fn valid_initials(value: &str) -> bool {
    (value.is_empty() || (3..=MAX_INITIALS).contains(&value.len()))
        && value
            .bytes()
            .all(|byte| byte.is_ascii_uppercase() || byte.is_ascii_digit())
}

fn encode_settings(settings: &Settings) -> String {
    let settings = settings.clone().normalized();
    format!(
        "v2:{}:{}:{}:{}",
        settings.master_volume,
        u8::from(settings.muted),
        u8::from(settings.reduced_motion),
        settings.leaderboard_initials
    )
}

fn parse_bit(value: &str) -> Option<bool> {
    match value {
        "0" => Some(false),
        "1" => Some(true),
        _ => None,
    }
}

fn decode_settings(value: &str) -> Settings {
    let mut fields = value.trim().split(':');
    let Some(version @ ("v1" | "v2")) = fields.next() else {
        return Settings::default();
    };
    let Some(master_volume) = fields.next().and_then(|value| value.parse::<u8>().ok()) else {
        return Settings::default();
    };
    let Some(muted) = fields.next().and_then(parse_bit) else {
        return Settings::default();
    };
    let Some(reduced_motion) = fields.next().and_then(parse_bit) else {
        return Settings::default();
    };
    let leaderboard_initials = if version == "v2" {
        let Some(initials) = fields.next() else {
            return Settings::default();
        };
        if !valid_initials(initials) {
            return Settings::default();
        }
        initials.to_string()
    } else {
        String::new()
    };
    if fields.next().is_some() || master_volume > 100 || master_volume % VOLUME_STEP != 0 {
        return Settings::default();
    }
    Settings {
        master_volume,
        muted,
        reduced_motion,
        leaderboard_initials,
    }
}

/// Decode the current schema, or import the old web-only mute bit only when
/// the settings schema is absent. The bool reports a successful migration.
#[cfg_attr(not(any(test, target_arch = "wasm32")), allow(dead_code))]
fn decode_or_migrate(schema: Option<&str>, legacy_muted: Option<&str>) -> (Settings, bool) {
    if let Some(schema) = schema {
        let loaded = decode_settings(schema);
        let canonical_v1 = format!(
            "v1:{}:{}:{}",
            loaded.master_volume,
            u8::from(loaded.muted),
            u8::from(loaded.reduced_motion)
        );
        let migrated = schema.trim() == canonical_v1;
        return (loaded, migrated);
    }
    if let Some(muted) = legacy_muted.and_then(|value| value.trim().parse::<bool>().ok()) {
        return (Settings { muted, ..default() }, true);
    }
    (Settings::default(), false)
}

fn load_settings(mut settings: ResMut<Settings>) {
    #[cfg(target_arch = "wasm32")]
    {
        let (loaded, migrated) = web_sys::window()
            .and_then(|window| window.local_storage().ok().flatten())
            .map(|storage| {
                let schema = storage.get_item(STORAGE_KEY).ok().flatten();
                let legacy = if schema.is_none() {
                    storage.get_item(LEGACY_MUTE_STORAGE_KEY).ok().flatten()
                } else {
                    None
                };
                decode_or_migrate(schema.as_deref(), legacy.as_deref())
            })
            .unwrap_or((Settings::default(), false));
        *settings = loaded;
        if migrated {
            let _ = save_settings(&*settings);
        }
    }
    #[cfg(not(target_arch = "wasm32"))]
    {
        let (loaded, migrated) = std::fs::read_to_string(FILE_PATH)
            .ok()
            .map(|value| decode_or_migrate(Some(&value), None))
            .unwrap_or((Settings::default(), false));
        *settings = loaded;
        if migrated {
            let _ = save_settings(&*settings);
        }
    }
}

fn persist_changed_settings(settings: Res<Settings>) {
    if settings.is_changed() {
        let _ = save_settings(&*settings);
    }
}

fn save_settings(settings: &Settings) -> bool {
    let encoded = encode_settings(settings);
    #[cfg(target_arch = "wasm32")]
    {
        let Some(window) = web_sys::window() else {
            return false;
        };
        let Ok(Some(storage)) = window.local_storage() else {
            return false;
        };
        storage.set_item(STORAGE_KEY, &encoded).is_ok()
    }
    #[cfg(not(target_arch = "wasm32"))]
    {
        atomic_write_native(FILE_PATH, encoded.as_bytes())
    }
}

#[cfg(not(target_arch = "wasm32"))]
fn atomic_write_native(path: &str, bytes: &[u8]) -> bool {
    use std::io::Write;
    use std::path::Path;

    let destination = Path::new(path);
    let temporary = destination.with_extension("tmp");
    let result = std::fs::File::create(&temporary).and_then(|mut file| {
        file.write_all(bytes)?;
        file.sync_all()
    });
    if result.is_err() {
        let _ = std::fs::remove_file(&temporary);
        return false;
    }
    if std::fs::rename(&temporary, destination).is_ok() {
        return true;
    }

    let backup = destination.with_extension("bak");
    if !destination.exists()
        || (backup.exists() && std::fs::remove_file(&backup).is_err())
        || std::fs::rename(destination, &backup).is_err()
    {
        let _ = std::fs::remove_file(&temporary);
        return false;
    }
    if std::fs::rename(&temporary, destination).is_ok() {
        let _ = std::fs::remove_file(&backup);
        true
    } else {
        let _ = std::fs::rename(&backup, destination);
        let _ = std::fs::remove_file(&temporary);
        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn schema_round_trips_and_corruption_defaults() {
        let settings = Settings {
            master_volume: 40,
            muted: true,
            reduced_motion: true,
            leaderboard_initials: "R2D2".to_string(),
        };
        assert_eq!(encode_settings(&settings), "v2:40:1:1:R2D2");
        assert_eq!(decode_settings(&encode_settings(&settings)), settings);

        let invalid_draft = Settings {
            leaderboard_initials: "AB".to_string(),
            ..default()
        };
        assert_eq!(encode_settings(&invalid_draft), "v2:100:0:0:");

        for corrupt in [
            "",
            "v3:40:1:1:ABC",
            "v2:40:1:1",
            "v2:40:1:1:AB",
            "v2:40:1:1:TOOLONG",
            "v2:40:1:1:abc",
            "v2:40:1:1:A-B",
            "v1:101:1:1",
            "v1:45:1:1",
            "v1:40:true:1",
            "v1:40:1",
            "v1:40:1:1:extra",
        ] {
            assert_eq!(decode_settings(corrupt), Settings::default(), "{corrupt}");
        }
    }

    #[test]
    fn v1_schema_migrates_with_an_empty_leaderboard_name() {
        let (migrated, did_migrate) = decode_or_migrate(Some("v1:30:1:0"), None);
        assert!(did_migrate);
        assert_eq!(migrated.master_volume, 30);
        assert!(migrated.muted);
        assert!(!migrated.reduced_motion);
        assert!(migrated.leaderboard_initials.is_empty());
        assert_eq!(encode_settings(&migrated), "v2:30:1:0:");
    }

    #[test]
    fn initials_validation_and_editing_are_bounded() {
        for valid in ["", "ABC", "A1B2", "12345"] {
            assert!(valid_initials(valid), "{valid}");
        }
        for invalid in ["A", "AB", "ABCDEF", "abc", "A_B"] {
            assert!(!valid_initials(invalid), "{invalid}");
        }

        let mut initials = String::new();
        for value in ['A', 'B', '3', 'D', '5'] {
            assert!(append_initial(&mut initials, value));
        }
        assert!(!append_initial(&mut initials, 'Z'));
        assert_eq!(initials, "AB3D5");
        assert!(cycle_last_initial(&mut initials));
        assert_eq!(initials, "AB3D6");
    }

    #[test]
    fn opener_mouse_hit_bounds_match_visible_button() {
        assert!(opener_hit(Vec2::new(OPENER_LEFT, OPENER_TOP)));
        assert!(opener_hit(Vec2::new(OPENER_RIGHT, OPENER_BOTTOM)));
        assert!(!opener_hit(Vec2::new(OPENER_LEFT - 0.001, OPENER_TOP)));
        assert!(!opener_hit(Vec2::new(OPENER_RIGHT, OPENER_BOTTOM + 0.001)));
    }

    #[test]
    fn legacy_mute_migrates_only_when_schema_is_absent() {
        let (migrated, did_migrate) = decode_or_migrate(None, Some("true"));
        assert!(did_migrate);
        assert!(migrated.muted);
        assert_eq!(migrated.master_volume, 100);

        let (schema, did_migrate) = decode_or_migrate(Some("broken"), Some("true"));
        assert!(!did_migrate);
        assert_eq!(schema, Settings::default());
    }

    #[test]
    fn volume_quantization_and_adjustment_clamp_to_steps() {
        assert_eq!(quantize_volume(0), 0);
        assert_eq!(quantize_volume(14), 10);
        assert_eq!(quantize_volume(15), 20);
        assert_eq!(quantize_volume(255), 100);
        assert_eq!(adjusted_volume(0, Adjustment::Left), 0);
        assert_eq!(adjusted_volume(100, Adjustment::Right), 100);
        assert_eq!(adjusted_volume(50, Adjustment::Left), 40);
    }

    #[test]
    fn selection_wraps_and_rows_adjust_or_activate() {
        assert_eq!(move_selection(SettingRow::Volume, -1), SettingRow::Back);
        assert_eq!(move_selection(SettingRow::Back, 1), SettingRow::Volume);
        assert_eq!(
            move_selection(SettingRow::ReducedMotion, 1),
            SettingRow::LeaderboardName
        );
        assert_eq!(
            touch_row(Vec2::new(0.5, 0.60)),
            Some(SettingRow::LeaderboardName)
        );

        let mut touch_settings = Settings::default();
        let mut touch_selection = SettingsSelection::default();
        assert_eq!(
            apply_touch_row(
                Vec2::new(0.5, 0.60),
                &mut touch_selection,
                &mut touch_settings,
            ),
            Activation::Changed
        );
        assert_eq!(touch_settings.leaderboard_initials, "A");
        assert_eq!(touch_selection.0, SettingRow::LeaderboardName);
        assert_eq!(
            apply_touch_row(
                Vec2::new(0.8, 0.60),
                &mut touch_selection,
                &mut touch_settings,
            ),
            Activation::Changed
        );
        assert_eq!(touch_settings.leaderboard_initials, "B");

        let mut settings = Settings::default();
        assert!(adjust_selection(
            &mut settings,
            SettingRow::Volume,
            Adjustment::Left
        ));
        assert_eq!(settings.master_volume, 90);
        assert_eq!(
            activate_selection(&mut settings, SettingRow::Mute),
            Activation::Changed
        );
        assert!(settings.muted);
        settings.leaderboard_initials = "ABC".to_string();
        assert!(adjust_selection(
            &mut settings,
            SettingRow::LeaderboardName,
            Adjustment::Left
        ));
        assert!(settings.leaderboard_initials.is_empty());
        assert_eq!(
            activate_selection(&mut settings, SettingRow::Back),
            Activation::Close
        );
    }
}
