//! Persistent player settings and the Menu/Paused settings overlay.
//!
//! On the web, reduced motion defaults to the operating-system/browser
//! `prefers-reduced-motion` value only when no settings schema exists. A
//! persisted player choice (including an explicit `false`) always takes
//! precedence; native platforms use the conservative `false` default.

use bevy::{
    prelude::*,
    text::{FontSize, TextLayout},
    window::PrimaryWindow,
};

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
const OPENER_DESKTOP_WIDTH: f32 = 126.0;
const OPENER_DESKTOP_HEIGHT: f32 = 42.0;
const OPENER_DESKTOP_TOP: f32 = 14.0;
const OPENER_DESKTOP_RIGHT: f32 = 16.0;
const OPENER_DESKTOP_FONT_SIZE: f32 = 19.0;
const OPENER_MOBILE_WIDTH: f32 = 104.0;
const OPENER_MOBILE_HEIGHT: f32 = 34.0;
const OPENER_MOBILE_TOP: f32 = 10.0;
const OPENER_MOBILE_RIGHT: f32 = 12.0;
const OPENER_MOBILE_FONT_SIZE: f32 = 15.0;
const SETTINGS_OPENER_LABEL: &str = "SETTINGS";
const SETTINGS_DEFAULT_FOOTER: &str = "Up/Down select | Left/Right change | Enter toggle | Esc/O back\nTouch/click: row center toggles; sides change";
const SETTINGS_NAME_FOOTER: &str = "Type A-Z/0-9 (3-5) | Backspace delete | Left clear\nTouch: center adds A | right cycles | left clears\nClearing revokes consent and stops future automatic submissions";
const SETTINGS_PANEL_WIDTH_RATIO: f32 = 0.88;
const SETTINGS_PANEL_HEIGHT_RATIO: f32 = 0.88;
const SETTINGS_PANEL_MAX_WIDTH: f32 = 760.0;
const SETTINGS_PANEL_INSET: f32 = 14.0;
const SETTINGS_ACTION_LEFT_END: f32 = 1.0 / 3.0;
const SETTINGS_ACTION_RIGHT_START: f32 = 2.0 / 3.0;
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
    /// are three to five ASCII letters/digits and represent explicit consent
    /// to automatic future score submissions; clearing the name revokes it.
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
struct SettingsPanel;

#[derive(Component)]
struct SettingsHeader;

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

#[derive(Clone, Copy, Debug, PartialEq)]
struct OpenerLayout {
    top: f32,
    right: f32,
    width: f32,
    height: f32,
    font_size: f32,
}

fn opener_layout(window_width: f32, window_height: f32) -> Option<OpenerLayout> {
    if window_width <= 0.0
        || window_height <= 0.0
        || !window_width.is_finite()
        || !window_height.is_finite()
    {
        return None;
    }
    let mobile = window_height <= 480.0 || window_width <= 960.0;
    Some(if mobile {
        OpenerLayout {
            top: OPENER_MOBILE_TOP,
            right: OPENER_MOBILE_RIGHT,
            width: OPENER_MOBILE_WIDTH,
            height: OPENER_MOBILE_HEIGHT,
            font_size: OPENER_MOBILE_FONT_SIZE,
        }
    } else {
        OpenerLayout {
            top: OPENER_DESKTOP_TOP,
            right: OPENER_DESKTOP_RIGHT,
            width: OPENER_DESKTOP_WIDTH,
            height: OPENER_DESKTOP_HEIGHT,
            font_size: OPENER_DESKTOP_FONT_SIZE,
        }
    })
}

fn opener_node(layout: OpenerLayout) -> Node {
    Node {
        position_type: PositionType::Absolute,
        top: px(layout.top),
        right: px(layout.right),
        width: px(layout.width),
        height: px(layout.height),
        border: UiRect::all(px(2.0)),
        border_radius: BorderRadius::all(px(9.0)),
        align_items: AlignItems::Center,
        justify_content: JustifyContent::Center,
        ..default()
    }
}

fn opener_text_layout() -> TextLayout {
    TextLayout::justify(Justify::Center)
}

fn opener_hit(position: Vec2, window_width: f32, window_height: f32) -> bool {
    let Some(layout) = opener_layout(window_width, window_height) else {
        return false;
    };
    let left = (window_width - layout.right - layout.width) / window_width;
    let right = (window_width - layout.right) / window_width;
    let top = layout.top / window_height;
    let bottom = (layout.top + layout.height) / window_height;
    (left..=right).contains(&position.x) && (top..=bottom).contains(&position.y)
}

/// Pixel bounds in top-left window coordinates. The same values are assigned
/// to the absolute Bevy UI nodes and used for pointer hit-testing.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
struct SettingsRect {
    left: f32,
    top: f32,
    width: f32,
    height: f32,
}

impl SettingsRect {
    fn right(self) -> f32 {
        self.left + self.width
    }

    fn bottom(self) -> f32 {
        self.top + self.height
    }

    fn contains(self, point: Vec2) -> bool {
        (self.left..=self.right()).contains(&point.x)
            && (self.top..=self.bottom()).contains(&point.y)
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
struct SettingsLayout {
    panel: SettingsRect,
    header: SettingsRect,
    rows: [SettingsRect; 5],
    footer: SettingsRect,
}

/// Build one deterministic responsive geometry model. Child bounds are local
/// to `panel`; `row_window_rect` converts them to window coordinates.
fn settings_layout(window_width: f32, window_height: f32) -> Option<SettingsLayout> {
    if window_width <= 0.0
        || window_height <= 0.0
        || !window_width.is_finite()
        || !window_height.is_finite()
    {
        return None;
    }

    let mobile = window_height <= 480.0 || window_width <= 960.0;
    let panel_width = (window_width * SETTINGS_PANEL_WIDTH_RATIO).min(SETTINGS_PANEL_MAX_WIDTH);
    let panel_height = window_height * SETTINGS_PANEL_HEIGHT_RATIO;
    let panel = SettingsRect {
        left: (window_width - panel_width) / 2.0,
        top: (window_height - panel_height) / 2.0,
        width: panel_width,
        height: panel_height,
    };
    let content_width = (panel_width - SETTINGS_PANEL_INSET * 2.0).max(0.0);
    let header = SettingsRect {
        left: SETTINGS_PANEL_INSET,
        top: if mobile { 8.0 } else { 18.0 },
        width: content_width,
        height: if mobile { 36.0 } else { 44.0 },
    };
    let footer_height = if mobile { 60.0 } else { 42.0 };
    let footer_bottom = if mobile { 8.0 } else { 18.0 };
    let footer = SettingsRect {
        left: SETTINGS_PANEL_INSET,
        top: (panel_height - footer_bottom - footer_height).max(0.0),
        width: content_width,
        height: footer_height,
    };

    let vertical_inset = if mobile { 6.0 } else { 24.0 };
    let rows_top = header.bottom() + vertical_inset;
    let rows_bottom = (footer.top - vertical_inset).max(rows_top);
    let available = rows_bottom - rows_top;
    let row_count = SettingRow::ALL.len() as f32;
    let preferred_row_height: f32 = if mobile { 36.0 } else { 52.0 };
    let row_height = preferred_row_height.min(available / row_count);
    let gap = if SettingRow::ALL.len() > 1 {
        ((available - row_height * row_count) / (row_count - 1.0))
            .max(0.0)
            .min(if mobile { 10.0 } else { 24.0 })
    } else {
        0.0
    };
    let rows_height = row_height * row_count + gap * (row_count - 1.0);
    let first_row_top = rows_top + (available - rows_height).max(0.0) / 2.0;
    let rows = std::array::from_fn(|index| SettingsRect {
        left: SETTINGS_PANEL_INSET,
        top: first_row_top + index as f32 * (row_height + gap),
        width: content_width,
        height: row_height,
    });

    Some(SettingsLayout {
        panel,
        header,
        rows,
        footer,
    })
}

fn row_window_rect(layout: &SettingsLayout, row: SettingRow) -> SettingsRect {
    let local = layout.rows[row.index()];
    SettingsRect {
        left: layout.panel.left + local.left,
        top: layout.panel.top + local.top,
        ..local
    }
}

fn touch_row(position: Vec2, window_width: f32, window_height: f32) -> Option<SettingRow> {
    if !position.is_finite() {
        return None;
    }
    let layout = settings_layout(window_width, window_height)?;
    SettingRow::ALL
        .into_iter()
        .find(|row| row_window_rect(&layout, *row).contains(position))
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum RowActionZone {
    Left,
    Center,
    Right,
}

fn row_action_zone(
    position: Vec2,
    window_width: f32,
    window_height: f32,
    row: SettingRow,
) -> Option<RowActionZone> {
    if !position.is_finite() {
        return None;
    }
    let layout = settings_layout(window_width, window_height)?;
    let bounds = row_window_rect(&layout, row);
    if !bounds.contains(position) {
        return None;
    }
    let fraction = (position.x - bounds.left) / bounds.width.max(f32::EPSILON);
    Some(if fraction < SETTINGS_ACTION_LEFT_END {
        RowActionZone::Left
    } else if fraction > SETTINGS_ACTION_RIGHT_START {
        RowActionZone::Right
    } else {
        RowActionZone::Center
    })
}

fn apply_touch_row(
    position: Vec2,
    window_width: f32,
    window_height: f32,
    selection: &mut SettingsSelection,
    settings: &mut Settings,
) -> Activation {
    let Some(row) = touch_row(position, window_width, window_height) else {
        return Activation::None;
    };
    let zone = row_action_zone(position, window_width, window_height, row)
        .unwrap_or(RowActionZone::Center);
    selection.0 = row;
    if row == SettingRow::Back {
        return Activation::Close;
    }
    if row == SettingRow::LeaderboardName {
        let changed = match zone {
            RowActionZone::Left => adjust_selection(settings, row, Adjustment::Left),
            RowActionZone::Right => {
                if settings.leaderboard_initials.is_empty() {
                    append_initial(&mut settings.leaderboard_initials, 'A')
                } else {
                    cycle_last_initial(&mut settings.leaderboard_initials)
                }
            }
            RowActionZone::Center => append_initial(&mut settings.leaderboard_initials, 'A'),
        };
        return if changed {
            Activation::Changed
        } else {
            Activation::None
        };
    }
    match zone {
        RowActionZone::Left => {
            if adjust_selection(settings, row, Adjustment::Left) {
                Activation::Changed
            } else {
                Activation::None
            }
        }
        RowActionZone::Right => {
            if adjust_selection(settings, row, Adjustment::Right) {
                Activation::Changed
            } else {
                Activation::None
            }
        }
        RowActionZone::Center => activate_selection(settings, row),
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
                    if normalized_touch(touch.position(), window).is_some_and(|position| {
                        opener_hit(position, window.width(), window.height())
                    }) {
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
                        .is_some_and(|position| {
                            opener_hit(position, window.width(), window.height())
                        });
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
            let position = touch.position();
            if !position.is_finite()
                || touch_row(position, window.width(), window.height()).is_none()
            {
                continue;
            }
            handled_pointer = true;
            if apply_touch_row(
                position,
                window.width(),
                window.height(),
                &mut *selection,
                &mut *settings,
            ) == Activation::Close
            {
                open.0 = false;
                break;
            }
        }
        if open.0 && mouse.just_pressed(MouseButton::Left) {
            if let Some(position) = window.cursor_position() {
                if touch_row(position, window.width(), window.height()).is_some() {
                    handled_pointer = true;
                    if apply_touch_row(
                        position,
                        window.width(),
                        window.height(),
                        &mut *selection,
                        &mut *settings,
                    ) == Activation::Close
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
    windows: Query<&Window, With<PrimaryWindow>>,
    overlay: Query<Entity, With<SettingsOverlayRoot>>,
    mut opener: Query<(Entity, &mut Node, &mut TextFont), With<SettingsOpenerRoot>>,
    mut panel_geometry: Query<
        (Entity, &mut Node),
        (With<SettingsPanel>, Without<SettingsOpenerRoot>),
    >,
    mut header_geometry: Query<
        (Entity, &mut Node),
        (
            With<SettingsHeader>,
            Without<SettingsOpenerRoot>,
            Without<SettingsPanel>,
            Without<SettingRow>,
            Without<SettingsFooter>,
        ),
    >,
    mut row_geometry: Query<
        (Entity, &SettingRow, &mut Node),
        (
            Without<SettingsOpenerRoot>,
            Without<SettingsPanel>,
            Without<SettingsHeader>,
            Without<SettingsFooter>,
        ),
    >,
    mut footer_geometry: Query<
        (Entity, &mut Node),
        (
            With<SettingsFooter>,
            Without<SettingsOpenerRoot>,
            Without<SettingsPanel>,
            Without<SettingsHeader>,
            Without<SettingRow>,
        ),
    >,
) {
    let show_opener = settings_context(*state.get()) && !open.0;
    let layout = windows
        .single()
        .ok()
        .and_then(|window| opener_layout(window.width(), window.height()))
        .unwrap_or(OpenerLayout {
            top: OPENER_DESKTOP_TOP,
            right: OPENER_DESKTOP_RIGHT,
            width: OPENER_DESKTOP_WIDTH,
            height: OPENER_DESKTOP_HEIGHT,
            font_size: OPENER_DESKTOP_FONT_SIZE,
        });
    if show_opener && opener.is_empty() {
        commands
            .spawn((
                opener_node(layout),
                BackgroundColor(Color::srgba(0.035, 0.04, 0.055, 0.94)),
                BorderColor::all(palette::HUD_ACCENT),
                Text::new(SETTINGS_OPENER_LABEL),
                TextFont {
                    font_size: FontSize::Px(layout.font_size),
                    ..default()
                },
                TextColor(palette::HUD_ACCENT.into()),
                opener_text_layout(),
            ))
            .insert(GlobalZIndex(80))
            .insert(SettingsOpenerRoot);
    } else if show_opener {
        for (_, mut node, mut font) in &mut opener {
            node.top = px(layout.top);
            node.right = px(layout.right);
            node.width = px(layout.width);
            node.height = px(layout.height);
            font.font_size = FontSize::Px(layout.font_size);
        }
    } else {
        for (entity, _, _) in &mut opener {
            commands.entity(entity).despawn();
        }
    }

    let settings_geometry = windows
        .single()
        .ok()
        .and_then(|window| settings_layout(window.width(), window.height()));
    if open.0 && overlay.is_empty() {
        if let Some(layout) = settings_geometry {
            spawn_overlay(&mut commands, &layout);
        }
    } else if open.0 {
        if let Some(layout) = settings_geometry {
            for (_, mut node) in &mut panel_geometry {
                apply_rect_to_node(&mut node, layout.panel);
            }
            for (_, mut node) in &mut header_geometry {
                apply_rect_to_node(&mut node, layout.header);
            }
            for (_, row, mut node) in &mut row_geometry {
                apply_rect_to_node(&mut node, layout.rows[row.index()]);
            }
            for (_, mut node) in &mut footer_geometry {
                apply_rect_to_node(&mut node, layout.footer);
            }
        }
    } else {
        for entity in &overlay {
            commands.entity(entity).despawn();
        }
    }
}

fn apply_rect_to_node(node: &mut Node, rect: SettingsRect) {
    node.position_type = PositionType::Absolute;
    node.left = px(rect.left);
    node.top = px(rect.top);
    node.width = px(rect.width);
    node.height = px(rect.height);
}

fn positioned_node(rect: SettingsRect) -> Node {
    let mut node = Node::default();
    apply_rect_to_node(&mut node, rect);
    node
}

fn spawn_overlay(commands: &mut Commands, layout: &SettingsLayout) {
    commands
        .spawn((
            Node {
                position_type: PositionType::Absolute,
                top: px(0.0),
                left: px(0.0),
                width: Val::Percent(100.0),
                height: Val::Percent(100.0),
                ..default()
            },
            BackgroundColor(Color::srgba(0.0, 0.0, 0.0, 0.82)),
            GlobalZIndex(100),
            SettingsOverlayRoot,
        ))
        .with_children(|root| {
            root.spawn((
                positioned_node(layout.panel),
                BackgroundColor(Color::srgba(0.035, 0.04, 0.055, 0.97)),
                SettingsPanel,
            ))
            .with_children(|panel| {
                let mut header_node = positioned_node(layout.header);
                header_node.align_items = AlignItems::Center;
                header_node.justify_content = JustifyContent::Center;
                panel.spawn((
                    Text::new("SETTINGS"),
                    TextFont {
                        font_size: FontSize::Px(34.0),
                        ..default()
                    },
                    TextColor(palette::HUD_ACCENT.into()),
                    header_node,
                    SettingsHeader,
                ));

                for row in SettingRow::ALL {
                    let mut row_node = positioned_node(layout.rows[row.index()]);
                    row_node.align_items = AlignItems::Center;
                    row_node.justify_content = JustifyContent::Center;
                    panel.spawn((
                        row_node,
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

                let mut footer_node = positioned_node(layout.footer);
                footer_node.align_items = AlignItems::Center;
                footer_node.justify_content = JustifyContent::Center;
                panel.spawn((
                    Text::new(SETTINGS_DEFAULT_FOOTER),
                    TextFont {
                        font_size: FontSize::Px(12.0),
                        ..default()
                    },
                    TextColor(Color::srgba(0.78, 0.8, 0.86, 1.0)),
                    footer_node,
                    SettingsFooter,
                ));
            });
        });
}

fn setting_row_text(row: SettingRow, settings: &Settings) -> String {
    match row {
        SettingRow::Volume => format!("[ Volume  {}% ]", settings.master_volume),
        SettingRow::Mute => {
            format!("[ Mute  {} ]", if settings.muted { "On" } else { "Off" })
        }
        SettingRow::ReducedMotion => format!(
            "[ Reduced Motion  {} ]",
            if settings.reduced_motion { "On" } else { "Off" }
        ),
        SettingRow::LeaderboardName => format!(
            "[clear]  Leaderboard Name  {}  [edit]",
            if settings.leaderboard_initials.is_empty() {
                "[none]"
            } else {
                &settings.leaderboard_initials
            }
        ),
        SettingRow::Back => "Back".to_string(),
    }
}

fn update_settings_rows(
    settings: Res<Settings>,
    selection: Res<SettingsSelection>,
    mut rows: Query<(
        Entity,
        &SettingRow,
        &mut Text,
        &mut TextColor,
        &mut BackgroundColor,
    )>,
    mut footer: Query<&mut Text, (With<SettingsFooter>, Without<SettingRow>)>,
) {
    for (_, row, mut text, mut color, mut background) in &mut rows {
        **text = setting_row_text(*row, &settings);
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
            SETTINGS_NAME_FOOTER
        } else {
            SETTINGS_DEFAULT_FOOTER
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
/// the settings schema is absent. A persisted schema has precedence over the
/// OS reduced-motion default, including when it explicitly stores `false`.
/// The bool reports a successful migration that should be saved canonically.
#[cfg_attr(not(any(test, target_arch = "wasm32")), allow(dead_code))]
fn decode_or_migrate(
    schema: Option<&str>,
    legacy_muted: Option<&str>,
    prefers_reduced_motion: bool,
) -> (Settings, bool) {
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

    let mut loaded = Settings {
        reduced_motion: prefers_reduced_motion,
        ..default()
    };
    if let Some(muted) = legacy_muted.and_then(|value| value.trim().parse::<bool>().ok()) {
        loaded.muted = muted;
        return (loaded, true);
    }
    (loaded, false)
}

#[cfg(target_arch = "wasm32")]
fn os_prefers_reduced_motion() -> bool {
    web_sys::window()
        .and_then(|window| {
            window
                .match_media("(prefers-reduced-motion: reduce)")
                .ok()
                .flatten()
        })
        .is_some_and(|media_query| media_query.matches())
}

#[cfg(not(target_arch = "wasm32"))]
fn os_prefers_reduced_motion() -> bool {
    false
}

fn load_settings(mut settings: ResMut<Settings>) {
    let prefers_reduced_motion = os_prefers_reduced_motion();
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
                decode_or_migrate(schema.as_deref(), legacy.as_deref(), prefers_reduced_motion)
            })
            .unwrap_or_else(|| decode_or_migrate(None, None, prefers_reduced_motion));
        *settings = loaded;
        if migrated {
            let _ = save_settings(&*settings);
        }
    }
    #[cfg(not(target_arch = "wasm32"))]
    {
        let (loaded, migrated) = std::fs::read_to_string(FILE_PATH)
            .ok()
            .map(|value| decode_or_migrate(Some(&value), None, prefers_reduced_motion))
            .unwrap_or_else(|| decode_or_migrate(None, None, prefers_reduced_motion));
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
        let (migrated, did_migrate) = decode_or_migrate(Some("v1:30:1:0"), None, true);
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
    fn opener_mouse_hit_bounds_match_visible_button_at_target_viewports() {
        for (width, height, expected) in [
            (
                844.0,
                390.0,
                OpenerLayout {
                    top: 10.0,
                    right: 12.0,
                    width: 104.0,
                    height: 34.0,
                    font_size: 15.0,
                },
            ),
            (
                1440.0,
                900.0,
                OpenerLayout {
                    top: 14.0,
                    right: 16.0,
                    width: 126.0,
                    height: 42.0,
                    font_size: 19.0,
                },
            ),
        ] {
            assert_eq!(opener_layout(width, height), Some(expected));
            let left = (width - expected.right - expected.width) / width;
            let right = (width - expected.right) / width;
            let top = expected.top / height;
            let bottom = (expected.top + expected.height) / height;
            assert!(opener_hit(Vec2::new(left, top), width, height));
            assert!(opener_hit(Vec2::new(right, bottom), width, height));
            assert!(!opener_hit(Vec2::new(left - 0.001, top), width, height));
            assert!(!opener_hit(Vec2::new(right, bottom + 0.001), width, height));
        }
        assert!(!opener_hit(Vec2::ZERO, 0.0, 390.0));
    }

    #[test]
    fn opener_node_and_text_layout_center_label_at_target_viewports() {
        for (width, height, expected) in [
            (
                844.0,
                390.0,
                OpenerLayout {
                    top: 10.0,
                    right: 12.0,
                    width: 104.0,
                    height: 34.0,
                    font_size: 15.0,
                },
            ),
            (
                1440.0,
                900.0,
                OpenerLayout {
                    top: 14.0,
                    right: 16.0,
                    width: 126.0,
                    height: 42.0,
                    font_size: 19.0,
                },
            ),
        ] {
            let layout = opener_layout(width, height).unwrap();
            assert_eq!(layout, expected);

            let node = opener_node(layout);
            assert_eq!(node.position_type, PositionType::Absolute);
            assert_eq!(node.top, px(expected.top));
            assert_eq!(node.right, px(expected.right));
            assert_eq!(node.width, px(expected.width));
            assert_eq!(node.height, px(expected.height));
            assert_eq!(node.align_items, AlignItems::Center);
            assert_eq!(node.justify_content, JustifyContent::Center);

            let text_layout = opener_text_layout();
            assert_eq!(text_layout.justify, Justify::Center);
        }
    }

    #[test]
    fn settings_player_labels_are_ascii() {
        assert_eq!(SETTINGS_OPENER_LABEL, "SETTINGS");
        assert!(SETTINGS_OPENER_LABEL.is_ascii());
        assert!(SETTINGS_DEFAULT_FOOTER.is_ascii());
        assert!(SETTINGS_NAME_FOOTER.is_ascii());
        for row in SettingRow::ALL {
            assert!(setting_row_text(row, &Settings::default()).is_ascii());
        }
    }

    #[test]
    fn centered_panel_bounds_come_from_shared_layout() {
        let desktop = settings_layout(1600.0, 900.0).unwrap().panel;
        assert!((desktop.left / 1600.0 - 0.2625).abs() < 0.000_001);
        assert!((desktop.right() / 1600.0 - 0.7375).abs() < 0.000_001);

        let mobile = settings_layout(390.0, 844.0).unwrap().panel;
        assert!((mobile.left / 390.0 - 0.06).abs() < 0.000_001);
        assert!((mobile.right() / 390.0 - 0.94).abs() < 0.000_001);

        assert!(settings_layout(0.0, 390.0).is_none());
        assert!(settings_layout(f32::NAN, 390.0).is_none());
    }

    #[test]
    fn settings_visual_rows_equal_touch_bounds_at_target_viewports() {
        for (width, height) in [(844.0, 390.0), (1440.0, 900.0)] {
            let layout = settings_layout(width, height).unwrap();
            for row in SettingRow::ALL {
                let bounds = row_window_rect(&layout, row);
                let center_y = bounds.top + bounds.height / 2.0;

                // Every inclusive visual edge belongs to exactly this row.
                for point in [
                    Vec2::new(bounds.left, bounds.top),
                    Vec2::new(bounds.right(), bounds.top),
                    Vec2::new(bounds.left, bounds.bottom()),
                    Vec2::new(bounds.right(), bounds.bottom()),
                ] {
                    assert_eq!(touch_row(point, width, height), Some(row), "{row:?}");
                }
                // Immediately outside every edge is not this row.
                for point in [
                    Vec2::new(bounds.left - 0.25, center_y),
                    Vec2::new(bounds.right() + 0.25, center_y),
                    Vec2::new(bounds.left + bounds.width / 2.0, bounds.top - 0.25),
                    Vec2::new(bounds.left + bounds.width / 2.0, bounds.bottom() + 0.25),
                ] {
                    assert_ne!(touch_row(point, width, height), Some(row), "{row:?}");
                }

                let left_x = bounds.left + bounds.width * 0.2;
                let center_x = bounds.left + bounds.width * 0.5;
                let right_x = bounds.left + bounds.width * 0.8;
                assert_eq!(
                    row_action_zone(Vec2::new(bounds.left, center_y), width, height, row),
                    Some(RowActionZone::Left)
                );
                assert_eq!(
                    row_action_zone(Vec2::new(bounds.right(), center_y), width, height, row),
                    Some(RowActionZone::Right)
                );
                assert_eq!(
                    row_action_zone(Vec2::new(left_x, center_y), width, height, row),
                    Some(RowActionZone::Left)
                );
                assert_eq!(
                    row_action_zone(Vec2::new(center_x, center_y), width, height, row),
                    Some(RowActionZone::Center)
                );
                assert_eq!(
                    row_action_zone(Vec2::new(right_x, center_y), width, height, row),
                    Some(RowActionZone::Right)
                );
                // Exact action boundaries belong to center; either side maps
                // deterministically to its adjacent action.
                for (fraction, expected) in [
                    (SETTINGS_ACTION_LEFT_END - 0.000_01, RowActionZone::Left),
                    (SETTINGS_ACTION_LEFT_END, RowActionZone::Center),
                    (SETTINGS_ACTION_RIGHT_START, RowActionZone::Center),
                    (SETTINGS_ACTION_RIGHT_START + 0.000_01, RowActionZone::Right),
                ] {
                    assert_eq!(
                        row_action_zone(
                            Vec2::new(bounds.left + bounds.width * fraction, center_y),
                            width,
                            height,
                            row,
                        ),
                        Some(expected)
                    );
                }
            }

            // Explicit gaps between visual rows are not interactive.
            for pair in layout.rows.windows(2) {
                let gap = (pair[0].bottom() + pair[1].top) / 2.0;
                assert_eq!(
                    touch_row(
                        Vec2::new(width / 2.0, layout.panel.top + gap),
                        width,
                        height,
                    ),
                    None
                );
            }
        }
    }

    #[test]
    fn settings_pointer_actions_use_row_local_left_center_right_zones() {
        let (width, height) = (1440.0, 900.0);
        let layout = settings_layout(width, height).unwrap();
        let point = |row: SettingRow, fraction: f32| {
            let bounds = row_window_rect(&layout, row);
            Vec2::new(
                bounds.left + bounds.width * fraction,
                bounds.top + bounds.height / 2.0,
            )
        };
        let mut settings = Settings::default();
        let mut selection = SettingsSelection::default();
        assert_eq!(
            apply_touch_row(
                point(SettingRow::Volume, 0.2),
                width,
                height,
                &mut selection,
                &mut settings,
            ),
            Activation::Changed
        );
        assert_eq!(settings.master_volume, 90);
        assert_eq!(
            apply_touch_row(
                point(SettingRow::Mute, 0.5),
                width,
                height,
                &mut selection,
                &mut settings,
            ),
            Activation::Changed
        );
        assert!(settings.muted);
        assert_eq!(
            apply_touch_row(
                point(SettingRow::ReducedMotion, 0.8),
                width,
                height,
                &mut selection,
                &mut settings,
            ),
            Activation::Changed
        );
        assert!(settings.reduced_motion);
        assert_eq!(
            apply_touch_row(
                point(SettingRow::Back, 0.5),
                width,
                height,
                &mut selection,
                &mut settings,
            ),
            Activation::Close
        );
    }

    #[test]
    fn reduced_motion_precedence_uses_os_only_without_a_schema() {
        let (os_reduced, did_migrate) = decode_or_migrate(None, None, true);
        assert!(!did_migrate);
        assert!(os_reduced.reduced_motion);

        let (os_standard, did_migrate) = decode_or_migrate(None, None, false);
        assert!(!did_migrate);
        assert!(!os_standard.reduced_motion);

        let (explicit_false, did_migrate) = decode_or_migrate(Some("v2:100:0:0:"), None, true);
        assert!(!did_migrate);
        assert!(!explicit_false.reduced_motion);

        let (explicit_true, did_migrate) = decode_or_migrate(Some("v2:100:0:1:"), None, false);
        assert!(!did_migrate);
        assert!(explicit_true.reduced_motion);
    }

    #[test]
    fn legacy_mute_migrates_only_when_schema_is_absent() {
        let (migrated, did_migrate) = decode_or_migrate(None, Some("true"), true);
        assert!(did_migrate);
        assert!(migrated.muted);
        assert!(migrated.reduced_motion);
        assert_eq!(migrated.master_volume, 100);

        let (schema, did_migrate) = decode_or_migrate(Some("broken"), Some("true"), true);
        assert!(!did_migrate);
        assert_eq!(schema, Settings::default());
    }

    #[cfg(not(target_arch = "wasm32"))]
    #[test]
    fn native_os_reduced_motion_default_is_safe() {
        assert!(!os_prefers_reduced_motion());
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
        let width = 1200.0;
        let height = 800.0;
        let layout = settings_layout(width, height).unwrap();
        let name_bounds = row_window_rect(&layout, SettingRow::LeaderboardName);
        let name_point = |fraction: f32| {
            Vec2::new(
                name_bounds.left + name_bounds.width * fraction,
                name_bounds.top + name_bounds.height / 2.0,
            )
        };
        assert_eq!(
            touch_row(name_point(0.5), width, height),
            Some(SettingRow::LeaderboardName)
        );

        let mut touch_settings = Settings::default();
        let mut touch_selection = SettingsSelection::default();
        assert_eq!(
            apply_touch_row(
                name_point(0.5),
                width,
                height,
                &mut touch_selection,
                &mut touch_settings,
            ),
            Activation::Changed
        );
        assert_eq!(touch_settings.leaderboard_initials, "A");
        assert_eq!(touch_selection.0, SettingRow::LeaderboardName);
        assert_eq!(
            apply_touch_row(
                name_point(0.8),
                width,
                height,
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
