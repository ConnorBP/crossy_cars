//! Responsive, WebGL2-safe presentation for the production main menu.
//! Gameplay state, persistence, Settings, and leaderboard data remain owned
//! by their existing plugins; this module owns presentation and selection.

use bevy::{
    ecs::system::SystemParam,
    input::{
        gamepad::{Gamepad, GamepadButton},
        touch::Touches,
    },
    prelude::*,
    render::render_resource::AsBindGroup,
    shader::ShaderRef,
    window::PrimaryWindow,
};

use crate::competitive_v3::{RankedV3Client, RankedV3Phase};
use crate::game::{RestartRequested, settings_closed, state::GameState};
use crate::game_modes::{Competition, Conduct, ManualCondition, SelectedGameMode};
use crate::modifiers::{ModifierKind, SelectedModifier};
use crate::persist::{Medal, ProductBests, medal_for, product_medal};
use crate::settings::Settings;

const CONDITIONS: [ModifierKind; 5] = [
    ModifierKind::Standard,
    ModifierKind::RushHour,
    ModifierKind::ChickenFrenzy,
    ModifierKind::Stampede,
    ModifierKind::GlassCannon,
];

const TIPS: [&str; 5] = [
    "Chickens give at least +1 each",
    "Coins give +1 and add time",
    "Complete the round mission once for +10",
    "Space brakes, then reverses when stopped",
    "Hold the handbrake while steering to drift",
];

#[derive(AsBindGroup, Asset, TypePath, Debug, Clone)]
pub struct HazardStripeMaterial {
    #[uniform(0)]
    pub color_a: Vec4,
    #[uniform(1)]
    pub color_b: Vec4,
    #[uniform(2)]
    pub params: Vec4,
}
impl UiMaterial for HazardStripeMaterial {
    fn fragment_shader() -> ShaderRef {
        "shaders/hazard_stripes.wgsl".into()
    }
}

#[derive(AsBindGroup, Asset, TypePath, Debug, Clone)]
pub struct GlowButtonMaterial {
    #[uniform(0)]
    pub color_fill: Vec4,
    #[uniform(1)]
    pub color_glow: Vec4,
    #[uniform(2)]
    pub params: Vec4,
    #[uniform(3)]
    pub params2: Vec4,
}
impl UiMaterial for GlowButtonMaterial {
    fn fragment_shader() -> ShaderRef {
        "shaders/glow_button.wgsl".into()
    }
}

#[derive(AsBindGroup, Asset, TypePath, Debug, Clone)]
pub struct MenuCardMaterial {
    #[uniform(0)]
    pub color_top: Vec4,
    #[uniform(1)]
    pub color_bottom: Vec4,
    #[uniform(2)]
    pub color_accent: Vec4,
    #[uniform(3)]
    pub params: Vec4,
}
impl UiMaterial for MenuCardMaterial {
    fn fragment_shader() -> ShaderRef {
        "shaders/menu_card.wgsl".into()
    }
}

#[derive(AsBindGroup, Asset, TypePath, Debug, Clone)]
pub struct VignetteMaterial {
    #[uniform(0)]
    pub color: Vec4,
    #[uniform(1)]
    pub params: Vec4,
}
impl UiMaterial for VignetteMaterial {
    fn fragment_shader() -> ShaderRef {
        "shaders/menu_vignette.wgsl".into()
    }
}

#[derive(Resource, Clone, Copy, Debug, Default, PartialEq, Eq)]
enum MenuLayout {
    #[default]
    Wide,
    Portrait,
    ShortLandscape,
}

impl MenuLayout {
    fn classify(width: f32, height: f32) -> Self {
        if height <= 480.0 && width > height {
            Self::ShortLandscape
        } else if width < 760.0 || height > width * 1.1 {
            Self::Portrait
        } else {
            Self::Wide
        }
    }
}

#[derive(Resource, Default)]
struct MenuMetrics {
    viewport_width: f32,
    card_width: f32,
    gap: f32,
}
#[derive(Resource, Default)]
struct MenuBuiltAt(f32);

#[derive(SystemParam)]
struct MenuRecords<'w> {
    products: Res<'w, ProductBests>,
}
#[derive(Resource, Clone, Copy, Debug, PartialEq, Eq)]
struct ProductFocus {
    index: usize,
    capability_admitted: bool,
}

impl Default for ProductFocus {
    fn default() -> Self {
        Self {
            // Fail-closed contractual fallback: Casual Cluck Hunt.
            index: 2,
            capability_admitted: false,
        }
    }
}

impl ProductFocus {
    const PRODUCTS: [(Competition, Conduct); 4] = [
        (Competition::Ranked, Conduct::CluckHunt),
        (Competition::Ranked, Conduct::RightOfWay),
        (Competition::Casual, Conduct::CluckHunt),
        (Competition::Casual, Conduct::RightOfWay),
    ];

    fn product(self) -> (Competition, Conduct) {
        Self::PRODUCTS[self.index]
    }

    fn set_default_for_gate(&mut self, admitted: bool) {
        if self.capability_admitted != admitted {
            self.capability_admitted = admitted;
            // Exact enabled tuple defaults to Ranked Cluck Hunt. Every other
            // capability state falls back to Casual Cluck Hunt.
            self.index = if admitted { 0 } else { 2 };
        }
    }

    fn move_focus(&mut self, delta: i32, ranked_enabled: bool) {
        let mut index = self.index as i32;
        for _ in 0..Self::PRODUCTS.len() {
            index = (index + delta).rem_euclid(Self::PRODUCTS.len() as i32);
            if ranked_enabled || index >= 2 {
                self.index = index as usize;
                return;
            }
        }
    }
}

#[derive(Resource, Default)]
struct TipState {
    index: usize,
    elapsed: f32,
}

/// Selection captured before a touch can press a side card. A completed swipe
/// resolves from this baseline, preventing the press and release from each
/// advancing the carousel once.
#[derive(Resource, Default)]
struct SwipeStartSelection(Option<ModifierKind>);

#[derive(Component)]
pub(crate) struct ResponsiveMenuRoot;
#[derive(Component)]
struct TitleLetter(usize);
#[derive(Component)]
struct CarouselRow(f32);
#[derive(Component)]
struct ConditionCard(usize);
#[derive(Component)]
struct CarouselDot(usize);
#[derive(Component)]
struct TipText;
/// The one explicit menu activation control. Product cells only move focus;
/// keyboard Enter/Space and this button share the same activation path.
#[derive(Component)]
struct DriveHint;
#[derive(Component)]
struct DriveLabel;
#[derive(Component, Default)]
struct GlowAnimation(f32);
#[derive(Component, Clone, Copy, Debug, PartialEq, Eq)]
enum MenuAction {
    Previous,
    Next,
    Select(usize),
    Product(Competition, Conduct),
    Drive,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum DrivePresentation {
    Drive,
    Connecting,
    RankedUnavailable,
}

impl DrivePresentation {
    fn label(self) -> &'static str {
        match self {
            Self::Drive => "DRIVE",
            Self::Connecting => "CONNECTING",
            Self::RankedUnavailable => "RANKED UNAVAILABLE",
        }
    }

    fn enabled(self) -> bool {
        self == Self::Drive
    }
}

fn drive_presentation(focus: ProductFocus, ranked: &RankedV3Client) -> DrivePresentation {
    if focus.product().0 == Competition::Casual {
        DrivePresentation::Drive
    } else if ranked.phase == RankedV3Phase::Starting {
        DrivePresentation::Connecting
    } else if ranked.ranked_available() {
        DrivePresentation::Drive
    } else {
        DrivePresentation::RankedUnavailable
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
struct MenuRect {
    left: f32,
    top: f32,
    width: f32,
    height: f32,
}

impl MenuRect {
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

    #[cfg(test)]
    fn overlaps(self, other: Self) -> bool {
        self.left < other.right()
            && self.right() > other.left
            && self.top < other.bottom()
            && self.bottom() > other.top
    }
}

const SHORT_PRODUCT_GRID_TOP: f32 = 256.0;
const SHORT_PRODUCT_GRID_HEIGHT: f32 = 70.0;
const SHORT_DRIVE_TOP: f32 = 330.0;
const SHORT_DRIVE_WIDTH: f32 = 244.0;
const SHORT_DRIVE_HEIGHT: f32 = 36.0;

fn drive_rect(layout: MenuLayout, width: f32, height: f32) -> MenuRect {
    match layout {
        MenuLayout::ShortLandscape => MenuRect {
            left: (width - SHORT_DRIVE_WIDTH) * 0.5,
            top: SHORT_DRIVE_TOP,
            width: SHORT_DRIVE_WIDTH,
            height: SHORT_DRIVE_HEIGHT,
        },
        MenuLayout::Portrait => {
            let drive_width = (width * 0.8).clamp(220.0, 360.0);
            MenuRect {
                left: (width - drive_width) * 0.5,
                top: (height - 122.0).max(0.0),
                width: drive_width,
                height: 52.0,
            }
        }
        MenuLayout::Wide => MenuRect {
            left: (width - 300.0) * 0.5,
            top: (height - 76.0).max(0.0),
            width: 300.0,
            height: 52.0,
        },
    }
}

fn short_product_grid_rect(width: f32) -> MenuRect {
    MenuRect {
        left: (width - 500.0) * 0.5,
        top: SHORT_PRODUCT_GRID_TOP,
        width: 500.0,
        height: SHORT_PRODUCT_GRID_HEIGHT,
    }
}

pub struct MenuPlugin;
impl Plugin for MenuPlugin {
    fn build(&self, app: &mut App) {
        app.add_plugins((
            UiMaterialPlugin::<HazardStripeMaterial>::default(),
            UiMaterialPlugin::<GlowButtonMaterial>::default(),
            UiMaterialPlugin::<MenuCardMaterial>::default(),
            UiMaterialPlugin::<VignetteMaterial>::default(),
        ))
        .init_resource::<MenuLayout>()
        .init_resource::<MenuMetrics>()
        .init_resource::<MenuBuiltAt>()
        .init_resource::<ProductFocus>()
        .init_resource::<TipState>()
        .init_resource::<SwipeStartSelection>()
        .add_systems(OnEnter(GameState::Menu), spawn_menu)
        .add_systems(OnExit(GameState::Menu), despawn_menu)
        .add_systems(
            Update,
            (
                sync_product_default,
                rebuild_responsive_menu,
                menu_keyboard,
                menu_swipe,
                menu_buttons,
                menu_touch_actions,
                (animate_menu_geometry, animate_menu_materials),
            )
                .chain()
                .run_if(in_state(GameState::Menu))
                .run_if(settings_closed),
        );
    }
}

fn to_vec4(color: Color) -> Vec4 {
    color.to_linear().to_vec4()
}
fn yellow() -> Color {
    Color::srgb(1.0, 0.83, 0.10)
}
fn ink() -> Color {
    Color::srgb(0.07, 0.07, 0.09)
}
fn text() -> Color {
    Color::srgb(0.93, 0.93, 0.91)
}
fn dim() -> Color {
    Color::srgb(0.64, 0.65, 0.66)
}
fn panel() -> Color {
    Color::srgba(0.07, 0.075, 0.10, 0.94)
}
fn font(size: f32) -> TextFont {
    TextFont {
        font_size: FontSize::Px(size),
        ..default()
    }
}

fn medal_points(medal: Medal) -> usize {
    match medal {
        Medal::None => 0,
        Medal::Bronze => 1,
        Medal::Silver => 2,
        Medal::Gold => 3,
    }
}
fn medal_thresholds(kind: ModifierKind) -> [u32; 3] {
    match kind {
        ModifierKind::Standard => [20, 40, 70],
        ModifierKind::RushHour => [15, 30, 55],
        ModifierKind::ChickenFrenzy => [35, 65, 100],
        ModifierKind::Stampede => [15, 25, 45],
        ModifierKind::GlassCannon => [25, 50, 80],
    }
}
fn condition_tagline(kind: ModifierKind) -> &'static str {
    match kind {
        ModifierKind::Standard => "Clean asphalt. No excuses.",
        ModifierKind::RushHour => "More traffic, moving faster.",
        ModifierKind::ChickenFrenzy => "A much larger, valuable flock.",
        ModifierKind::Stampede => "More road animals fight back.",
        ModifierKind::GlassCannon => "Big combo bonuses. Bigger damage.",
    }
}

fn cycle_selection(selected: &mut SelectedModifier, delta: i32) {
    let current = selected.0.index() as i32;
    selected.0 = CONDITIONS[(current + delta).rem_euclid(CONDITIONS.len() as i32) as usize];
}

fn sync_product_default(ranked: Res<RankedV3Client>, mut focus: ResMut<ProductFocus>) {
    // Use the admission resource's exact capability bit as well as Ready;
    // Starting/Started may retain admission but cannot be selected twice.
    focus.set_default_for_gate(ranked.capability_admitted() && ranked.ranked_available());
}

fn activate_product(
    competition: Competition,
    conduct: Conduct,
    selected: &SelectedModifier,
    mode: &mut SelectedGameMode,
    ranked: &mut RankedV3Client,
    restart: &mut RestartRequested,
    next: &mut NextState<GameState>,
) {
    match competition {
        Competition::Casual => {
            mode.competition = Competition::Casual;
            mode.conduct = conduct;
            mode.manual_condition = ManualCondition::from(selected.0);
            restart.0 = false;
            next.set(GameState::Playing);
        }
        Competition::Ranked => {
            if ranked.request_ranked_start(conduct) {
                mode.competition = Competition::Ranked;
                mode.conduct = conduct;
                // Ranked never retains a manual selection as admission state.
                mode.manual_condition = ManualCondition::Standard;
                restart.0 = false;
            }
        }
    }
}

fn menu_keyboard(
    keys: Res<ButtonInput<KeyCode>>,
    gamepads: Query<&Gamepad>,
    mut selected: ResMut<SelectedModifier>,
    mut focus: ResMut<ProductFocus>,
    mut mode: ResMut<SelectedGameMode>,
    mut ranked: ResMut<RankedV3Client>,
    mut restart: ResMut<RestartRequested>,
    mut next: ResMut<NextState<GameState>>,
) {
    let gp_left = gamepads.iter().any(|gamepad| {
        gamepad.just_pressed(GamepadButton::DPadLeft) || gamepad.just_pressed(GamepadButton::West)
    });
    let gp_right = gamepads.iter().any(|gamepad| {
        gamepad.just_pressed(GamepadButton::DPadRight) || gamepad.just_pressed(GamepadButton::East)
    });
    let gp_up = gamepads
        .iter()
        .any(|gamepad| gamepad.just_pressed(GamepadButton::DPadUp));
    let gp_down = gamepads
        .iter()
        .any(|gamepad| gamepad.just_pressed(GamepadButton::DPadDown));
    let gp_activate = gamepads.iter().any(|gamepad| {
        gamepad.just_pressed(GamepadButton::South) || gamepad.just_pressed(GamepadButton::Start)
    });

    // Product focus is a deterministic row-major order. Disabled Ranked cells
    // are skipped by keyboard and gamepad traversal, not merely ignored later.
    let reverse_tab = keys.just_pressed(KeyCode::Tab)
        && keys.any_pressed([KeyCode::ShiftLeft, KeyCode::ShiftRight]);
    if (keys.just_pressed(KeyCode::Tab) && !reverse_tab) || gp_right || gp_down {
        focus.move_focus(1, ranked.ranked_available());
    }
    if reverse_tab || keys.just_pressed(KeyCode::Backspace) || gp_left || gp_up {
        focus.move_focus(-1, ranked.ranked_available());
    }

    // Manual condition mutation is Casual-only. A Ranked-focused cell exposes
    // the forced rotation and consumes no carousel/change input.
    if focus.product().0 == Competition::Casual {
        if keys.any_just_pressed([KeyCode::ArrowLeft, KeyCode::KeyA]) {
            cycle_selection(&mut selected, -1);
        }
        if keys.any_just_pressed([KeyCode::ArrowRight, KeyCode::KeyD]) {
            cycle_selection(&mut selected, 1);
        }
    }
    if keys.any_just_pressed([KeyCode::Enter, KeyCode::Space]) || gp_activate {
        let (competition, conduct) = focus.product();
        activate_product(
            competition,
            conduct,
            &selected,
            &mut mode,
            &mut ranked,
            &mut restart,
            &mut next,
        );
    }
}

fn menu_swipe(
    touches: Res<Touches>,
    focus: Res<ProductFocus>,
    mut selected: ResMut<SelectedModifier>,
    mut start_selection: ResMut<SwipeStartSelection>,
) {
    if touches.iter_just_pressed().next().is_some() {
        start_selection.0 = Some(selected.0);
    }
    for touch in touches.iter_just_released() {
        let delta = touch.position() - touch.start_position();
        // Carousel swipes are mutable only while a Casual cell owns focus.
        if focus.product().0 == Competition::Casual
            && delta.x.abs() > 48.0
            && delta.x.abs() > delta.y.abs() * 1.4
        {
            selected.0 = start_selection.0.unwrap_or(selected.0);
            cycle_selection(&mut selected, if delta.x < 0.0 { 1 } else { -1 });
        }
        start_selection.0 = None;
    }
}

fn resolve_activation_action(
    action: MenuAction,
    focus: &mut ProductFocus,
) -> Option<(Competition, Conduct)> {
    match action {
        MenuAction::Product(competition, conduct) => {
            focus.index = ProductFocus::PRODUCTS
                .iter()
                .position(|product| *product == (competition, conduct))
                .expect("known product");
            None
        }
        MenuAction::Drive => Some(focus.product()),
        _ => None,
    }
}

fn menu_buttons(
    interactions: Query<(&Interaction, &MenuAction), Changed<Interaction>>,
    mut selected: ResMut<SelectedModifier>,
    mut focus: ResMut<ProductFocus>,
    mut mode: ResMut<SelectedGameMode>,
    mut ranked: ResMut<RankedV3Client>,
    mut restart: ResMut<RestartRequested>,
    mut next: ResMut<NextState<GameState>>,
) {
    for (interaction, action) in &interactions {
        if *interaction != Interaction::Pressed {
            continue;
        }
        match *action {
            MenuAction::Previous if focus.product().0 == Competition::Casual => {
                cycle_selection(&mut selected, -1)
            }
            MenuAction::Next if focus.product().0 == Competition::Casual => {
                cycle_selection(&mut selected, 1)
            }
            MenuAction::Select(index) if focus.product().0 == Competition::Casual => {
                selected.0 = CONDITIONS[index]
            }
            action @ (MenuAction::Product(_, _) | MenuAction::Drive) => {
                // Product cells are selection-only. Repeated clicks/taps never
                // activate a run; this also preserves the rebuild debounce by
                // requiring a separate press on the stable DRIVE target.
                if let Some((competition, conduct)) = resolve_activation_action(action, &mut focus)
                {
                    activate_product(
                        competition,
                        conduct,
                        &selected,
                        &mut mode,
                        &mut ranked,
                        &mut restart,
                        &mut next,
                    );
                }
            }
            _ => {}
        }
    }
}

fn short_product_at(point: Vec2, width: f32) -> Option<(Competition, Conduct)> {
    let grid = short_product_grid_rect(width);
    if !grid.contains(point) {
        return None;
    }
    let local = point - Vec2::new(grid.left, grid.top);
    let column = usize::from(local.x >= (grid.width + 6.0) * 0.5);
    let row = usize::from(local.y >= 35.0);
    Some(ProductFocus::PRODUCTS[row * 2 + column])
}

/// Bevy's WebGL touch-to-Interaction bridge is not reliable on all mobile
/// browsers. Resolve the exact same short-landscape button geometry from raw
/// touches as a fallback; desktop clicks and other layouts still use Button
/// Interaction. A just-pressed touch can produce at most one action.
fn menu_touch_actions(
    touches: Res<Touches>,
    windows: Query<&Window, With<PrimaryWindow>>,
    layout: Res<MenuLayout>,
    mut focus: ResMut<ProductFocus>,
    selected: Res<SelectedModifier>,
    mut mode: ResMut<SelectedGameMode>,
    mut ranked: ResMut<RankedV3Client>,
    mut restart: ResMut<RestartRequested>,
    mut next: ResMut<NextState<GameState>>,
) {
    if *layout != MenuLayout::ShortLandscape {
        return;
    }
    let Ok(window) = windows.single() else { return };
    let drive = drive_rect(*layout, window.width(), window.height());
    for touch in touches.iter_just_pressed() {
        let point = touch.position();
        if drive.contains(point) {
            let (competition, conduct) = focus.product();
            activate_product(
                competition,
                conduct,
                &selected,
                &mut mode,
                &mut ranked,
                &mut restart,
                &mut next,
            );
            return;
        }
        if let Some((competition, conduct)) = short_product_at(point, window.width()) {
            let _ =
                resolve_activation_action(MenuAction::Product(competition, conduct), &mut focus);
            return;
        }
    }
}

fn despawn_menu(mut commands: Commands, roots: Query<Entity, With<ResponsiveMenuRoot>>) {
    for entity in &roots {
        commands.entity(entity).despawn();
    }
}

fn spawn_menu(
    mut commands: Commands,
    windows: Query<&Window, With<PrimaryWindow>>,
    records: MenuRecords,
    settings: Res<Settings>,
    selected: Res<SelectedModifier>,
    focus: Res<ProductFocus>,
    ranked: Res<RankedV3Client>,
    time: Res<Time>,
    mut layout: ResMut<MenuLayout>,
    mut metrics: ResMut<MenuMetrics>,
    mut built_at: ResMut<MenuBuiltAt>,
    mut stripe_materials: ResMut<Assets<HazardStripeMaterial>>,
    mut glow_materials: ResMut<Assets<GlowButtonMaterial>>,
    mut card_materials: ResMut<Assets<MenuCardMaterial>>,
    mut vignette_materials: ResMut<Assets<VignetteMaterial>>,
) {
    let (width, height) = windows
        .single()
        .map(|window| (window.width(), window.height()))
        .unwrap_or((1280.0, 800.0));
    *layout = MenuLayout::classify(width, height);
    built_at.0 = time.elapsed_secs();
    build_menu(
        &mut commands,
        *layout,
        width,
        height,
        &records.products,
        &settings,
        selected.0,
        *focus,
        &ranked,
        &mut metrics,
        &mut stripe_materials,
        &mut glow_materials,
        &mut card_materials,
        &mut vignette_materials,
    );
}

#[allow(clippy::too_many_arguments)]
fn rebuild_responsive_menu(
    mut commands: Commands,
    roots: Query<Entity, With<ResponsiveMenuRoot>>,
    windows: Query<&Window, With<PrimaryWindow>>,
    records: MenuRecords,
    settings: Res<Settings>,
    selected: Res<SelectedModifier>,
    focus: Res<ProductFocus>,
    ranked: Res<RankedV3Client>,
    time: Res<Time>,
    mut layout: ResMut<MenuLayout>,
    mut metrics: ResMut<MenuMetrics>,
    mut built_at: ResMut<MenuBuiltAt>,
    mut stripe_materials: ResMut<Assets<HazardStripeMaterial>>,
    mut glow_materials: ResMut<Assets<GlowButtonMaterial>>,
    mut card_materials: ResMut<Assets<MenuCardMaterial>>,
    mut vignette_materials: ResMut<Assets<VignetteMaterial>>,
) {
    let Ok(window) = windows.single() else { return };
    let next_layout = MenuLayout::classify(window.width(), window.height());
    let needs_rebuild = roots.is_empty()
        || *layout != next_layout
        || records.products.is_changed()
        || settings.is_changed()
        || focus.is_changed()
        || ranked.is_changed();
    if !needs_rebuild {
        return;
    }
    for entity in &roots {
        commands.entity(entity).despawn();
    }
    *layout = next_layout;
    built_at.0 = time.elapsed_secs();
    build_menu(
        &mut commands,
        next_layout,
        window.width(),
        window.height(),
        &records.products,
        &settings,
        selected.0,
        *focus,
        &ranked,
        &mut metrics,
        &mut stripe_materials,
        &mut glow_materials,
        &mut card_materials,
        &mut vignette_materials,
    );
}

fn chip(parent: &mut ChildSpawnerCommands, label: String, small: bool) {
    parent
        .spawn((
            Node {
                padding: UiRect::axes(px(if small { 7.0 } else { 10.0 }), px(4.0)),
                border: UiRect::all(px(1.0)),
                border_radius: BorderRadius::all(px(6.0)),
                ..default()
            },
            BackgroundColor(panel()),
            BorderColor::all(yellow().with_alpha(0.38)),
        ))
        .with_children(|chip| {
            chip.spawn((
                Text::new(label),
                font(if small { 10.0 } else { 13.0 }),
                TextColor(yellow()),
            ));
        });
}

fn arrow_button(parent: &mut ChildSpawnerCommands, label: &str, action: MenuAction) {
    parent
        .spawn((
            Button,
            action,
            Node {
                width: px(44.0),
                height: px(54.0),
                align_items: AlignItems::Center,
                justify_content: JustifyContent::Center,
                border: UiRect::all(px(1.0)),
                border_radius: BorderRadius::all(px(9.0)),
                ..default()
            },
            BackgroundColor(panel()),
            BorderColor::all(yellow().with_alpha(0.55)),
        ))
        .with_children(|button| {
            button.spawn((Text::new(label), font(25.0), TextColor(yellow())));
        });
}

#[allow(clippy::too_many_arguments)]
fn build_menu(
    commands: &mut Commands,
    layout: MenuLayout,
    window_width: f32,
    window_height: f32,
    product_bests: &ProductBests,
    settings: &Settings,
    selected: ModifierKind,
    focus: ProductFocus,
    ranked: &RankedV3Client,
    metrics: &mut MenuMetrics,
    stripe_materials: &mut Assets<HazardStripeMaterial>,
    glow_materials: &mut Assets<GlowButtonMaterial>,
    card_materials: &mut Assets<MenuCardMaterial>,
    vignette_materials: &mut Assets<VignetteMaterial>,
) {
    let short = layout == MenuLayout::ShortLandscape;
    let portrait = layout == MenuLayout::Portrait;
    let compact = short || portrait;
    let gap = if compact { 12.0 } else { 20.0 };
    let (card_width, card_height, viewport_width) = if short {
        (205.0, 82.0, (window_width * 0.64).clamp(260.0, 590.0))
    } else if portrait {
        (
            (window_width * 0.72).clamp(220.0, 340.0),
            236.0,
            window_width * 0.96,
        )
    } else {
        let viewport = (window_width - 310.0).clamp(620.0, 830.0);
        (
            ((viewport - 2.0 * gap) / 3.0).clamp(180.0, 250.0),
            270.0,
            viewport,
        )
    };
    *metrics = MenuMetrics {
        viewport_width,
        card_width,
        gap,
    };
    let reduced = f32::from(settings.reduced_motion);
    let (focused_competition, focused_conduct) = focus.product();
    let earned: usize = if focused_competition == Competition::Casual {
        CONDITIONS
            .iter()
            .map(|kind| {
                medal_points(product_medal(
                    Competition::Casual,
                    focused_conduct,
                    *kind,
                    product_bests.get(Competition::Casual, focused_conduct, *kind),
                ))
            })
            .sum()
    } else {
        medal_points(product_medal(
            Competition::Ranked,
            focused_conduct,
            selected,
            product_bests.get(Competition::Ranked, focused_conduct, selected),
        ))
    };

    commands.spawn((
        ResponsiveMenuRoot,
        Node {
            position_type: PositionType::Absolute,
            width: percent(100.0),
            height: percent(100.0),
            ..default()
        },
        Pickable::IGNORE,
        MaterialNode(vignette_materials.add(VignetteMaterial {
            color: to_vec4(Color::srgb(0.02, 0.02, 0.04)),
            params: Vec4::new(0.0, 0.84, 0.48, reduced),
        })),
        GlobalZIndex(1),
    ));

    commands
        .spawn((
            ResponsiveMenuRoot,
            Node {
                position_type: PositionType::Absolute,
                width: percent(100.0),
                height: percent(100.0),
                flex_direction: FlexDirection::Column,
                align_items: AlignItems::Center,
                justify_content: JustifyContent::SpaceBetween,
                // Reserve the bottom activation lane on non-short layouts;
                // short landscape uses audited absolute product/DRIVE lanes.
                padding: UiRect {
                    left: px(if short { 8.0 } else { 12.0 }),
                    right: px(if short { 8.0 } else { 12.0 }),
                    top: px(if short { 5.0 } else { 12.0 }),
                    bottom: px(if short {
                        5.0
                    } else if portrait {
                        130.0
                    } else {
                        76.0
                    }),
                },
                ..default()
            },
            GlobalZIndex(2),
        ))
        .with_children(|root| {
            root.spawn(Node {
                width: percent(100.0),
                column_gap: px(8.0),
                ..default()
            })
            .with_children(|bar| {
                let focused_best =
                    product_bests.get(focused_competition, focused_conduct, selected);
                chip(bar, format!("PRODUCT BEST {focused_best}"), short);
                chip(
                    bar,
                    format!(
                        "MEDALS {earned}/{}",
                        if focused_competition == Competition::Ranked {
                            3
                        } else {
                            15
                        }
                    ),
                    short,
                );
            });

            let title_size = if short {
                34.0
            } else if portrait {
                50.0
            } else {
                76.0
            };
            root.spawn(Node {
                flex_direction: FlexDirection::Column,
                align_items: AlignItems::Center,
                row_gap: px(if short { 2.0 } else { 5.0 }),
                ..default()
            })
            .with_children(|title| {
                title
                    .spawn(Node {
                        align_items: AlignItems::FlexEnd,
                        column_gap: px(1.0),
                        ..default()
                    })
                    .with_children(|letters| {
                        for (index, character) in "ROADY CAR".chars().enumerate() {
                            if character == ' ' {
                                letters.spawn(Node {
                                    width: px(title_size * 0.30),
                                    ..default()
                                });
                            } else {
                                letters.spawn((
                                    TitleLetter(index),
                                    Node {
                                        position_type: PositionType::Relative,
                                        ..default()
                                    },
                                    Text::new(character.to_string()),
                                    font(title_size),
                                    TextColor(if index < 5 { yellow() } else { text() }),
                                    TextShadow {
                                        offset: Vec2::splat(title_size * 0.045),
                                        color: Color::srgba(0.0, 0.0, 0.0, 0.86),
                                    },
                                ));
                            }
                        }
                    });
                title.spawn((
                    Node {
                        width: px(if short {
                            290.0
                        } else if portrait {
                            window_width * 0.78
                        } else {
                            520.0
                        }),
                        height: px(if short { 7.0 } else { 11.0 }),
                        ..default()
                    },
                    MaterialNode(stripe_materials.add(HazardStripeMaterial {
                        color_a: to_vec4(yellow()),
                        color_b: to_vec4(ink()),
                        params: Vec4::new(
                            0.0,
                            14.0,
                            if settings.reduced_motion { 0.0 } else { 26.0 },
                            0.9,
                        ),
                    })),
                ));
                if !short {
                    title.spawn((
                        Text::new("ISOMETRIC MAYHEM DELIVERY SERVICE"),
                        font(if portrait { 10.0 } else { 12.0 }),
                        TextColor(dim()),
                    ));
                }
            });

            root.spawn(Node {
                display: if focus.product().0 == Competition::Casual {
                    Display::Flex
                } else {
                    Display::None
                },
                position_type: if short {
                    PositionType::Absolute
                } else {
                    PositionType::Relative
                },
                top: if short { px(160.0) } else { Val::Auto },
                flex_direction: FlexDirection::Column,
                align_items: AlignItems::Center,
                row_gap: px(if short { 3.0 } else { 8.0 }),
                ..default()
            })
            .with_children(|region| {
                region
                    .spawn(Node {
                        align_items: AlignItems::Center,
                        column_gap: px(if compact { 7.0 } else { 13.0 }),
                        ..default()
                    })
                    .with_children(|carousel| {
                        if !portrait {
                            arrow_button(carousel, "<", MenuAction::Previous);
                        }
                        carousel
                            .spawn(Node {
                                width: px(viewport_width),
                                height: px(card_height + if short { 8.0 } else { 22.0 }),
                                overflow: Overflow::clip(),
                                align_items: AlignItems::FlexEnd,
                                ..default()
                            })
                            .with_children(|viewport| {
                                viewport.spawn((
                            CarouselRow(0.0),
                            Node {
                                position_type: PositionType::Absolute,
                                left: px(0.0),
                                height: px(card_height + if short { 8.0 } else { 22.0 }),
                                align_items: AlignItems::FlexEnd,
                                column_gap: px(gap),
                                ..default()
                            },
                        ))
                        .with_children(|row| {
                            for (index, kind) in CONDITIONS.iter().copied().enumerate() {
                                spawn_condition_card(
                                    row,
                                    index,
                                    kind,
                                    product_bests.get(
                                        Competition::Casual,
                                        focused_conduct,
                                        kind,
                                    ),
                                    card_width,
                                    card_height,
                                    layout,
                                    card_materials,
                                );
                            }
                        });
                            });
                        if !portrait {
                            arrow_button(carousel, ">", MenuAction::Next);
                        }
                    });
                if portrait {
                    region
                        .spawn(Node {
                            column_gap: px(8.0),
                            ..default()
                        })
                        .with_children(|dots| {
                            for index in 0..CONDITIONS.len() {
                                dots.spawn((
                                    CarouselDot(index),
                                    Button,
                                    MenuAction::Select(index),
                                    Node {
                                        width: px(10.0),
                                        height: px(10.0),
                                        border_radius: BorderRadius::MAX,
                                        ..default()
                                    },
                                    BackgroundColor(Color::srgba(1.0, 1.0, 1.0, 0.25)),
                                ));
                            }
                        });
                }
            });

            let selected_color = selected.color();
            let manual_visible = focus.product().0 == Competition::Casual;
            root.spawn(Node {
                position_type: if short {
                    PositionType::Absolute
                } else {
                    PositionType::Relative
                },
                top: if short {
                    px(SHORT_PRODUCT_GRID_TOP - 13.0)
                } else {
                    Val::Auto
                },
                flex_direction: FlexDirection::Column,
                align_items: AlignItems::Center,
                row_gap: px(if short { 2.0 } else { 6.0 }),
                ..default()
            })
            .with_children(|bottom| {
                bottom.spawn((
                    Text::new(if manual_visible {
                        format!(
                            "CASUAL CONDITION · {} · MUTABLE",
                            selected.display_name().to_uppercase()
                        )
                    } else {
                        "RANKED CONDITION · FORCED 16-SEGMENT ROTATION · NO MANUAL RECEIPT"
                            .to_string()
                    }),
                    font(if short { 8.0 } else { 10.0 }),
                    TextColor(if manual_visible {
                        selected_color
                    } else {
                        yellow()
                    }),
                ));
                // Four explicit products: row-major focus is Ranked CH/ROW,
                // then Casual CH/ROW. Manual controls affect Casual only.
                bottom
                    .spawn(Node {
                        width: px(if short {
                            500.0
                        } else if portrait {
                            (window_width * 0.90).min(430.0)
                        } else {
                            620.0
                        }),
                        display: Display::Grid,
                        grid_template_columns: RepeatedGridTrack::flex(2, 1.0),
                        grid_template_rows: RepeatedGridTrack::auto(2),
                        column_gap: px(if short { 6.0 } else { 10.0 }),
                        row_gap: px(if short { 4.0 } else { 8.0 }),
                        ..default()
                    })
                    .with_children(|grid| {
                        for (product_index, (competition, conduct, label)) in [
                            (
                                Competition::Ranked,
                                Conduct::CluckHunt,
                                "RANKED · CLUCK HUNT",
                            ),
                            (
                                Competition::Ranked,
                                Conduct::RightOfWay,
                                "RANKED · RIGHT OF WAY",
                            ),
                            (
                                Competition::Casual,
                                Conduct::CluckHunt,
                                "CASUAL · CLUCK HUNT",
                            ),
                            (
                                Competition::Casual,
                                Conduct::RightOfWay,
                                "CASUAL · RIGHT OF WAY",
                            ),
                        ]
                        .into_iter()
                        .enumerate()
                        {
                            let enabled =
                                competition == Competition::Casual || ranked.ranked_available();
                            let focused = focus.index == product_index;
                            let color = if enabled { selected_color } else { dim() };
                            let condition = if competition == Competition::Ranked {
                                "FORCED ROTATION".to_string()
                            } else {
                                selected.display_name().to_uppercase()
                            };
                            let best = product_bests.get(competition, conduct, selected);
                            let medal = product_medal(competition, conduct, selected, best);
                            let mut cell_entity = grid.spawn((
                                GlowAnimation::default(),
                                Node {
                                    min_width: px(0.0),
                                    height: px(if short { 31.0 } else { 46.0 }),
                                    border: UiRect::all(px(if focused { 3.0 } else { 1.0 })),
                                    border_radius: BorderRadius::all(px(10.0)),
                                    align_items: AlignItems::Center,
                                    justify_content: JustifyContent::Center,
                                    flex_direction: FlexDirection::Column,
                                    ..default()
                                },
                                BorderColor::all(if focused {
                                    yellow()
                                } else {
                                    color.with_alpha(0.45)
                                }),
                                MaterialNode(glow_materials.add(GlowButtonMaterial {
                                    color_fill: to_vec4(color),
                                    color_glow: to_vec4(color),
                                    params: Vec4::new(
                                        0.0,
                                        if enabled { 0.45 } else { 0.0 },
                                        0.25,
                                        0.0,
                                    ),
                                    params2: Vec4::new(reduced, 0.0, 0.0, 0.0),
                                })),
                            ));
                            // Even unavailable Ranked products remain selectable
                            // so the explicit action control can explain its
                            // disabled state. Cells themselves never activate.
                            cell_entity.insert((Button, MenuAction::Product(competition, conduct)));
                            cell_entity.with_children(|cell| {
                                cell.spawn((
                                    Text::new(if enabled {
                                        format!("{} {label}", if focused { ">" } else { "" })
                                    } else {
                                        format!("[LOCKED] {label}")
                                    }),
                                    font(if short { 8.0 } else { 11.0 }),
                                    TextColor(if enabled { ink() } else { text() }),
                                ));
                                if !short {
                                    cell.spawn((
                                        Text::new(format!(
                                            "{condition} · BEST {best} · {}",
                                            medal.label().to_uppercase()
                                        )),
                                        font(8.0),
                                        TextColor(if enabled {
                                            ink().with_alpha(0.72)
                                        } else {
                                            text()
                                        }),
                                    ));
                                }
                            });
                        }
                    });
                if !short {
                    bottom.spawn((
                        Text::new(if ranked.ranked_available() {
                            "RANKED ADMITTED · EXACT v3 TUPLE + ORDERED CATEGORIES VERIFIED"
                                .to_string()
                        } else {
                            format!(
                                "RANKED DISABLED · {} · CHOOSE A CASUAL CELL",
                                ranked.message
                            )
                        }),
                        font(10.0),
                        TextColor(if ranked.ranked_available() {
                            yellow()
                        } else {
                            dim()
                        }),
                    ));
                }
                if !short {
                    bottom.spawn((
                        TipText,
                        Text::new(TIPS[0]),
                        font(if portrait { 11.0 } else { 13.0 }),
                        TextColor(dim()),
                    ));
                }
            });

            // Exactly one activation target exists at every responsive size.
            // It is absolute in short landscape so product/status reflow can
            // never move it away from the audited touch rectangle.
            let drive = drive_rect(layout, window_width, window_height);
            let presentation = drive_presentation(focus, ranked);
            let drive_color = if presentation.enabled() {
                selected_color
            } else {
                dim()
            };
            root.spawn((
                DriveHint,
                GlowAnimation::default(),
                Button,
                MenuAction::Drive,
                Node {
                    position_type: PositionType::Absolute,
                    left: px(drive.left),
                    top: px(drive.top),
                    width: px(drive.width),
                    height: px(drive.height),
                    border: UiRect::all(px(2.0)),
                    border_radius: BorderRadius::all(px(10.0)),
                    align_items: AlignItems::Center,
                    justify_content: JustifyContent::Center,
                    ..default()
                },
                BorderColor::all(if presentation.enabled() {
                    yellow()
                } else {
                    dim().with_alpha(0.65)
                }),
                MaterialNode(glow_materials.add(GlowButtonMaterial {
                    color_fill: to_vec4(drive_color),
                    color_glow: to_vec4(drive_color),
                    params: Vec4::new(
                        0.0,
                        if presentation.enabled() { 0.60 } else { 0.0 },
                        0.25,
                        0.0,
                    ),
                    params2: Vec4::new(reduced, 0.0, 0.0, 0.0),
                })),
            ))
            .with_children(|button| {
                button.spawn((
                    DriveLabel,
                    Text::new(presentation.label()),
                    font(if short { 17.0 } else { 21.0 }),
                    TextColor(if presentation.enabled() {
                        ink()
                    } else {
                        text()
                    }),
                ));
            });
        });
}

fn spawn_condition_card(
    parent: &mut ChildSpawnerCommands,
    index: usize,
    kind: ModifierKind,
    best: u32,
    width: f32,
    height: f32,
    layout: MenuLayout,
    materials: &mut Assets<MenuCardMaterial>,
) {
    let short = layout == MenuLayout::ShortLandscape;
    let medal = medal_for(kind, best);
    let earned = medal_points(medal);
    let thresholds = medal_thresholds(kind);
    let next = thresholds.get(earned).copied();
    parent
        .spawn((
            ConditionCard(index),
            Button,
            MenuAction::Select(index),
            Node {
                position_type: PositionType::Relative,
                width: px(width),
                height: px(height),
                flex_shrink: 0.0,
                flex_direction: FlexDirection::Column,
                align_items: AlignItems::Center,
                justify_content: JustifyContent::SpaceBetween,
                padding: UiRect::all(px(if short { 8.0 } else { 13.0 })),
                border_radius: BorderRadius::all(px(14.0)),
                ..default()
            },
            MaterialNode(materials.add(MenuCardMaterial {
                color_top: to_vec4(Color::srgb(0.13, 0.13, 0.17)),
                color_bottom: to_vec4(Color::srgb(0.07, 0.07, 0.10)),
                color_accent: to_vec4(kind.color()),
                params: Vec4::ZERO,
            })),
        ))
        .with_children(|card| {
            card.spawn((
                Text::new(kind.display_name().to_uppercase()),
                font(if short { 15.0 } else { 20.0 }),
                TextColor(kind.color()),
            ));
            if !short {
                card.spawn((
                    Text::new(condition_tagline(kind)),
                    font(11.0),
                    TextColor(dim()),
                    TextLayout::justify(Justify::Center),
                ));
            }
            card.spawn(Node {
                flex_direction: FlexDirection::Column,
                align_items: AlignItems::Center,
                ..default()
            })
            .with_children(|score| {
                score.spawn((
                    Text::new("BEST"),
                    font(if short { 8.0 } else { 10.0 }),
                    TextColor(dim()),
                ));
                score.spawn((
                    Text::new(best.to_string()),
                    font(if short { 24.0 } else { 33.0 }),
                    TextColor(text()),
                ));
            });
            card.spawn(Node {
                display: if short { Display::None } else { Display::Flex },
                flex_direction: FlexDirection::Column,
                align_items: AlignItems::Center,
                row_gap: px(4.0),
                ..default()
            })
            .with_children(|medals| {
                medals
                    .spawn(Node {
                        column_gap: px(8.0),
                        ..default()
                    })
                    .with_children(|pips| {
                        for tier in 0..3 {
                            let color = [
                                Color::srgb(0.80, 0.50, 0.20),
                                Color::srgb(0.78, 0.78, 0.82),
                                Color::srgb(1.0, 0.84, 0.0),
                            ][tier];
                            pips.spawn((
                                Node {
                                    width: px(if short { 10.0 } else { 15.0 }),
                                    height: px(if short { 10.0 } else { 15.0 }),
                                    border: UiRect::all(px(2.0)),
                                    border_radius: BorderRadius::MAX,
                                    ..default()
                                },
                                BackgroundColor(if tier < earned { color } else { Color::NONE }),
                                BorderColor::all(if tier < earned {
                                    color
                                } else {
                                    dim().with_alpha(0.5)
                                }),
                            ));
                        }
                    });
                medals.spawn((
                    Text::new(next.map_or_else(
                        || "ALL MEDALS EARNED".into(),
                        |target| format!("NEXT AT {target}"),
                    )),
                    font(if short { 8.0 } else { 10.0 }),
                    TextColor(dim()),
                ));
            });
        });
}

#[allow(clippy::type_complexity)]
fn animate_menu_geometry(
    time: Res<Time>,
    settings: Res<Settings>,
    selected: Res<SelectedModifier>,
    metrics: Res<MenuMetrics>,
    built_at: Res<MenuBuiltAt>,
    mut tips: ResMut<TipState>,
    mut rows: Query<(&mut CarouselRow, &mut Node), Without<ConditionCard>>,
    mut cards: Query<(&ConditionCard, &mut Node, &MaterialNode<MenuCardMaterial>)>,
    mut dots: Query<(&CarouselDot, &mut BackgroundColor), Without<ConditionCard>>,
    mut letters: Query<(&TitleLetter, &mut Node), (Without<ConditionCard>, Without<CarouselRow>)>,
    mut tip_text: Query<(&mut Text, &mut TextColor), With<TipText>>,
    mut card_materials: ResMut<Assets<MenuCardMaterial>>,
) {
    let now = time.elapsed_secs();
    let dt = time.delta_secs();
    let selected_index = selected.0.index();
    let target = metrics.viewport_width * 0.5
        - metrics.card_width * 0.5
        - selected_index as f32 * (metrics.card_width + metrics.gap);
    let k = if settings.reduced_motion {
        1.0
    } else {
        1.0 - (-10.0 * dt).exp()
    };
    for (mut row, mut node) in &mut rows {
        row.0 += (target - row.0) * k;
        node.left = px(row.0);
    }
    for (card, mut node, handle) in &mut cards {
        let active = card.0 == selected_index;
        node.top = px(if active { -10.0 } else { 0.0 });
        if let Some(mut material) = card_materials.get_mut(handle) {
            material.params = Vec4::new(
                now,
                f32::from(active),
                1.0,
                f32::from(settings.reduced_motion),
            );
        }
    }
    for (dot, mut background) in &mut dots {
        background.0 = if dot.0 == selected_index {
            yellow()
        } else {
            Color::srgba(1.0, 1.0, 1.0, 0.25)
        };
    }
    for (letter, mut node) in &mut letters {
        node.top = if settings.reduced_motion {
            px(0.0)
        } else {
            let local = now - built_at.0 - letter.0 as f32 * 0.04;
            let t = (local / 0.42).clamp(0.0, 1.0);
            px(-55.0 * (1.0 - t).powi(3)
                + if local > 0.42 {
                    (now * 2.0 + letter.0 as f32).sin() * 2.0
                } else {
                    0.0
                })
        };
    }
    tips.elapsed += dt;
    if tips.elapsed >= 4.0 {
        tips.elapsed = 0.0;
        tips.index = (tips.index + 1) % TIPS.len();
        for (mut value, _) in &mut tip_text {
            value.0 = TIPS[tips.index].into();
        }
    }
    let tip_alpha = if settings.reduced_motion {
        1.0
    } else {
        (tips.elapsed / 0.35)
            .min(((4.0 - tips.elapsed) / 0.35).min(1.0))
            .clamp(0.0, 1.0)
    };
    for (_, mut color) in &mut tip_text {
        color.0 = dim().with_alpha(tip_alpha);
    }
}

fn animate_menu_materials(
    time: Res<Time>,
    settings: Res<Settings>,
    selected: Res<SelectedModifier>,
    focus: Res<ProductFocus>,
    ranked: Res<RankedV3Client>,
    stripes: Query<&MaterialNode<HazardStripeMaterial>>,
    vignettes: Query<&MaterialNode<VignetteMaterial>>,
    mut glows: Query<(
        &MaterialNode<GlowButtonMaterial>,
        &Interaction,
        &MenuAction,
        &mut GlowAnimation,
    )>,
    mut drive_labels: Query<(&mut Text, &mut TextColor), With<DriveLabel>>,
    mut stripe_materials: ResMut<Assets<HazardStripeMaterial>>,
    mut vignette_materials: ResMut<Assets<VignetteMaterial>>,
    mut glow_materials: ResMut<Assets<GlowButtonMaterial>>,
) {
    let now = time.elapsed_secs();
    let dt = time.delta_secs();
    let k = if settings.reduced_motion {
        1.0
    } else {
        1.0 - (-10.0 * dt).exp()
    };
    for handle in &stripes {
        if let Some(mut material) = stripe_materials.get_mut(handle) {
            material.params.x = now;
            material.params.z = if settings.reduced_motion { 0.0 } else { 26.0 };
        }
    }
    for handle in &vignettes {
        if let Some(mut material) = vignette_materials.get_mut(handle) {
            material.params.x = now;
            material.params.w = f32::from(settings.reduced_motion);
        }
    }
    let presentation = drive_presentation(*focus, &ranked);
    for (mut label, mut color) in &mut drive_labels {
        label.0 = presentation.label().into();
        color.0 = if presentation.enabled() {
            ink()
        } else {
            text()
        };
    }
    for (handle, interaction, action, mut animation) in &mut glows {
        let enabled = match action {
            MenuAction::Drive => presentation.enabled(),
            MenuAction::Product(Competition::Ranked, _) => ranked.ranked_available(),
            _ => true,
        };
        let target = if !enabled {
            0.0
        } else {
            match *interaction {
                Interaction::Pressed => 1.0,
                Interaction::Hovered => 0.55,
                Interaction::None => 0.0,
            }
        };
        animation.0 += (target - animation.0) * k;
        if let Some(mut material) = glow_materials.get_mut(handle) {
            let color = if enabled { selected.0.color() } else { dim() };
            material.color_fill = to_vec4(color);
            material.color_glow = to_vec4(color);
            material.params.x = now;
            material.params.w = animation.0;
            material.params2.x = f32::from(settings.reduced_motion);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn product_focus_defaults_and_skips_disabled_ranked_cells() {
        let mut focus = ProductFocus::default();
        assert_eq!(focus.product(), (Competition::Casual, Conduct::CluckHunt));
        focus.move_focus(1, false);
        assert_eq!(focus.product(), (Competition::Casual, Conduct::RightOfWay));
        focus.move_focus(1, false);
        assert_eq!(focus.product(), (Competition::Casual, Conduct::CluckHunt));
        focus.set_default_for_gate(true);
        assert_eq!(focus.product(), (Competition::Ranked, Conduct::CluckHunt));
        focus.move_focus(1, true);
        assert_eq!(focus.product(), (Competition::Ranked, Conduct::RightOfWay));
        focus.set_default_for_gate(false);
        assert_eq!(focus.product(), (Competition::Casual, Conduct::CluckHunt));
    }

    #[test]
    fn responsive_layout_covers_portrait_short_landscape_and_wide() {
        assert_eq!(MenuLayout::classify(390.0, 844.0), MenuLayout::Portrait);
        assert_eq!(MenuLayout::classify(420.0, 840.0), MenuLayout::Portrait);
        assert_eq!(
            MenuLayout::classify(844.0, 390.0),
            MenuLayout::ShortLandscape
        );
        assert_eq!(
            MenuLayout::classify(960.0, 480.0),
            MenuLayout::ShortLandscape
        );
        assert_eq!(MenuLayout::classify(1280.0, 800.0), MenuLayout::Wide);
        assert_eq!(MenuLayout::classify(1440.0, 900.0), MenuLayout::Wide);
    }

    #[test]
    fn swipe_resolution_uses_touch_start_selection_not_pressed_side_card() {
        let mut selected = SelectedModifier(ModifierKind::RushHour);
        selected.0 = ModifierKind::Standard;
        cycle_selection(&mut selected, 1);
        assert_eq!(selected.0, ModifierKind::RushHour);

        selected.0 = ModifierKind::Standard;
        cycle_selection(&mut selected, -1);
        assert_eq!(selected.0, ModifierKind::GlassCannon);
    }

    #[test]
    fn carousel_wraps_and_maps_stable_condition_indices() {
        let mut selected = SelectedModifier(ModifierKind::Standard);
        cycle_selection(&mut selected, -1);
        assert_eq!(selected.0, ModifierKind::GlassCannon);
        cycle_selection(&mut selected, 1);
        assert_eq!(selected.0, ModifierKind::Standard);
        for (index, kind) in CONDITIONS.into_iter().enumerate() {
            assert_eq!(kind.index(), index);
        }
    }

    #[test]
    fn cards_use_authoritative_medal_thresholds() {
        for kind in CONDITIONS {
            let [bronze, silver, gold] = medal_thresholds(kind);
            assert_eq!(medal_for(kind, bronze), Medal::Bronze);
            assert_eq!(medal_for(kind, silver), Medal::Silver);
            assert_eq!(medal_for(kind, gold), Medal::Gold);
        }
    }

    #[test]
    fn product_actions_only_select_and_drive_is_the_only_activation_action() {
        let mut focus = ProductFocus::default();
        let product = MenuAction::Product(Competition::Casual, Conduct::RightOfWay);
        assert_eq!(resolve_activation_action(product, &mut focus), None);
        assert_eq!(focus.product(), (Competition::Casual, Conduct::RightOfWay));
        // A second product activation remains selection-only.
        assert_eq!(resolve_activation_action(product, &mut focus), None);
        assert_eq!(
            resolve_activation_action(MenuAction::Drive, &mut focus),
            Some((Competition::Casual, Conduct::RightOfWay))
        );
    }

    #[test]
    fn drive_labels_follow_casual_ranked_disabled_and_connecting_states() {
        let casual = ProductFocus::default();
        let mut ranked_focus = casual;
        ranked_focus.index = 0;
        let mut ranked = RankedV3Client::default();
        assert_eq!(
            drive_presentation(casual, &ranked),
            DrivePresentation::Drive
        );
        assert_eq!(
            drive_presentation(ranked_focus, &ranked),
            DrivePresentation::RankedUnavailable
        );
        ranked.phase = RankedV3Phase::Starting;
        assert_eq!(
            drive_presentation(ranked_focus, &ranked),
            DrivePresentation::Connecting
        );
        assert!(!DrivePresentation::Connecting.enabled());
        assert!(!DrivePresentation::RankedUnavailable.enabled());
    }

    #[test]
    fn short_landscape_drive_geometry_is_single_clear_stable_target() {
        let drive = drive_rect(MenuLayout::ShortLandscape, 844.0, 390.0);
        let products = short_product_grid_rect(844.0);
        let settings = MenuRect {
            left: 844.0 - 12.0 - 104.0,
            top: 10.0,
            width: 104.0,
            height: 34.0,
        };
        assert!(drive.contains(Vec2::new(422.0, 340.0)));
        assert_eq!(
            drive,
            MenuRect {
                left: 300.0,
                top: 330.0,
                width: 244.0,
                height: 36.0,
            }
        );
        assert!(products.bottom() <= 326.0);
        assert!(!drive.overlaps(products));
        assert!(!drive.overlaps(settings));
        assert!(drive.bottom() <= 366.0);
        assert_eq!(
            short_product_at(Vec2::new(548.0, 309.0), 844.0),
            Some((Competition::Casual, Conduct::RightOfWay))
        );
        assert_eq!(short_product_at(Vec2::new(422.0, 340.0), 844.0), None);
    }
}
