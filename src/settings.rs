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

/// Player preferences shared by settings UI and runtime systems.
#[derive(Resource, Clone, Copy, Debug, PartialEq, Eq)]
pub struct Settings {
    pub master_volume: u8,
    pub muted: bool,
    pub reduced_motion: bool,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            master_volume: 100,
            muted: false,
            reduced_motion: false,
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

#[derive(Component, Default, Clone, Copy, Debug, PartialEq, Eq)]
enum SettingRow {
    #[default]
    Volume,
    Mute,
    ReducedMotion,
    Back,
}

impl SettingRow {
    const ALL: [Self; 4] = [Self::Volume, Self::Mute, Self::ReducedMotion, Self::Back];

    fn index(self) -> usize {
        match self {
            Self::Volume => 0,
            Self::Mute => 1,
            Self::ReducedMotion => 2,
            Self::Back => 3,
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
        SettingRow::Back => false,
    }
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
        KeyCode::KeyO,
        // Prevent the underlying Paused menu from restarting/quitting while
        // this modal owns focus.
        KeyCode::KeyR,
        KeyCode::KeyQ,
    ] {
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

fn touch_row(position: Vec2) -> Option<SettingRow> {
    match position.y {
        y if (0.25..0.39).contains(&y) => Some(SettingRow::Volume),
        y if (0.39..0.52).contains(&y) => Some(SettingRow::Mute),
        y if (0.52..0.65).contains(&y) => Some(SettingRow::ReducedMotion),
        y if (0.65..0.82).contains(&y) => Some(SettingRow::Back),
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
    if position.x < 0.42 {
        adjust_selection(settings, row, Adjustment::Left);
        Activation::Changed
    } else if position.x > 0.58 {
        adjust_selection(settings, row, Adjustment::Right);
        Activation::Changed
    } else {
        activate_selection(settings, row)
    }
}

fn settings_input(
    mut keys: ResMut<ButtonInput<KeyCode>>,
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
                    if normalized_touch(touch.position(), window)
                        .is_some_and(|position| position.x >= 0.72 && position.y <= 0.22)
                    {
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
            }
        }
        if open.0 {
            clear_modal_keys(&mut keys);
        }
        return;
    }

    // O and Escape are modal close keys and take precedence over row actions.
    if keys.just_pressed(KeyCode::Escape) || keys.just_pressed(KeyCode::KeyO) {
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

    if keys.just_pressed(KeyCode::Enter) || keys.just_pressed(KeyCode::Space) {
        if activate_selection(&mut settings, selected_row) == Activation::Close {
            open.0 = false;
        }
    }

    if let Ok(window) = windows.single() {
        let mut handled_touch = false;
        for touch in touches.iter_just_pressed() {
            touch_active.0 = true;
            let Some(position) = normalized_touch(touch.position(), window) else {
                continue;
            };
            handled_touch = true;
            if apply_touch_row(position, &mut *selection, &mut *settings) == Activation::Close {
                open.0 = false;
                break;
            }
        }
        if handled_touch {
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
        commands.spawn((
            Node {
                position_type: PositionType::Absolute,
                top: Val::Percent(3.0),
                right: Val::Percent(3.0),
                width: Val::Percent(25.0),
                height: Val::Percent(15.0),
                align_items: AlignItems::Center,
                justify_content: JustifyContent::Center,
                ..default()
            },
            BackgroundColor(Color::srgba(0.02, 0.02, 0.03, 0.72)),
            Text::new("O  SETTINGS"),
            TextFont {
                font_size: FontSize::Px(21.0),
                ..default()
            },
            TextColor(palette::HUD_ACCENT.into()),
            GlobalZIndex(80),
            SettingsOpenerRoot,
        ));
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
                    width: Val::Percent(76.0),
                    height: Val::Percent(88.0),
                    max_width: px(760.0),
                    padding: UiRect::all(px(22.0)),
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
                        font_size: FontSize::Px(48.0),
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
                            min_height: px(48.0),
                            padding: UiRect::axes(px(16.0), px(8.0)),
                            align_items: AlignItems::Center,
                            justify_content: JustifyContent::Center,
                            ..default()
                        },
                        BackgroundColor(Color::NONE),
                        Text::default(),
                        TextFont {
                            font_size: FontSize::Px(26.0),
                            ..default()
                        },
                        TextColor(palette::HUD_TEXT.into()),
                        row,
                    ));
                }

                panel.spawn((
                    Text::new("Arrows: select/change  |  Enter/Space: toggle  |  Esc/O: back\nTouch a row; use its left/right side to change"),
                    TextFont {
                        font_size: FontSize::Px(16.0),
                        ..default()
                    },
                    TextColor(Color::srgba(0.78, 0.8, 0.86, 1.0)),
                    Node {
                        align_self: AlignSelf::Center,
                        ..default()
                    },
                ));
            });
        });
}

fn update_settings_rows(
    settings: Res<Settings>,
    selection: Res<SettingsSelection>,
    mut rows: Query<(&SettingRow, &mut Text, &mut TextColor, &mut BackgroundColor)>,
) {
    for (row, mut text, mut color, mut background) in &mut rows {
        **text = match row {
            SettingRow::Volume => format!("<  Volume  {}%  >", settings.master_volume),
            SettingRow::Mute => {
                format!("<  Mute  {}  >", if settings.muted { "On" } else { "Off" })
            }
            SettingRow::ReducedMotion => format!(
                "<  Reduced Motion  {}  >",
                if settings.reduced_motion { "On" } else { "Off" }
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
}

fn encode_settings(settings: Settings) -> String {
    let settings = settings.normalized();
    format!(
        "v1:{}:{}:{}",
        settings.master_volume,
        u8::from(settings.muted),
        u8::from(settings.reduced_motion)
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
    if fields.next() != Some("v1") {
        return Settings::default();
    }
    let Some(master_volume) = fields.next().and_then(|value| value.parse::<u8>().ok()) else {
        return Settings::default();
    };
    let Some(muted) = fields.next().and_then(parse_bit) else {
        return Settings::default();
    };
    let Some(reduced_motion) = fields.next().and_then(parse_bit) else {
        return Settings::default();
    };
    if fields.next().is_some() || master_volume > 100 || master_volume % VOLUME_STEP != 0 {
        return Settings::default();
    }
    Settings {
        master_volume,
        muted,
        reduced_motion,
    }
}

/// Decode the current schema, or import the old web-only mute bit only when
/// the settings schema is absent. The bool reports a successful migration.
#[cfg_attr(not(any(test, target_arch = "wasm32")), allow(dead_code))]
fn decode_or_migrate(schema: Option<&str>, legacy_muted: Option<&str>) -> (Settings, bool) {
    if let Some(schema) = schema {
        return (decode_settings(schema), false);
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
            let _ = save_settings(loaded);
        }
    }
    #[cfg(not(target_arch = "wasm32"))]
    {
        *settings = std::fs::read_to_string(FILE_PATH)
            .ok()
            .map(|value| decode_settings(&value))
            .unwrap_or_default();
    }
}

fn persist_changed_settings(settings: Res<Settings>) {
    if settings.is_changed() {
        let _ = save_settings(*settings);
    }
}

fn save_settings(settings: Settings) -> bool {
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
        };
        assert_eq!(encode_settings(settings), "v1:40:1:1");
        assert_eq!(decode_settings(&encode_settings(settings)), settings);

        for corrupt in [
            "",
            "v2:40:1:1",
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
        assert_eq!(
            activate_selection(&mut settings, SettingRow::Back),
            Activation::Close
        );
    }
}
