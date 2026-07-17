//! Responsive, WebGL2-safe presentation for the production main menu.
//! Gameplay state, persistence, Settings, and leaderboard data remain owned
//! by their existing plugins; this module owns presentation and selection.

use bevy::{
    input::touch::Touches, prelude::*, render::render_resource::AsBindGroup, shader::ShaderRef,
    window::PrimaryWindow,
};

use crate::game::{RestartRequested, settings_closed, state::GameState};
use crate::game_modes::{Conduct, SelectedGameMode};
use crate::modifiers::{ModifierKind, SelectedModifier};
use crate::persist::{BestScore, ConditionBests, Medal, medal_for};
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
#[derive(Component)]
struct DriveHint;
#[derive(Component, Default)]
struct GlowAnimation(f32);
#[derive(Component, Clone, Copy)]
enum MenuAction {
    Previous,
    Next,
    Select(usize),
    Drive,
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
        .init_resource::<TipState>()
        .init_resource::<SwipeStartSelection>()
        .add_systems(OnEnter(GameState::Menu), spawn_menu)
        .add_systems(OnExit(GameState::Menu), despawn_menu)
        .add_systems(
            Update,
            (
                rebuild_responsive_menu,
                menu_keyboard,
                menu_swipe,
                menu_buttons,
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

fn menu_keyboard(
    keys: Res<ButtonInput<KeyCode>>,
    mut selected: ResMut<SelectedModifier>,
    mut mode: ResMut<SelectedGameMode>,
    mut restart: ResMut<RestartRequested>,
    mut next: ResMut<NextState<GameState>>,
) {
    if keys.any_just_pressed([KeyCode::ArrowLeft, KeyCode::KeyA]) {
        cycle_selection(&mut selected, -1);
    }
    if keys.any_just_pressed([KeyCode::ArrowRight, KeyCode::KeyD]) {
        cycle_selection(&mut selected, 1);
    }
    // Compact keyboard conduct selector until the four-cell capability-gated
    // menu lands. Casual remains the default and performs no network work.
    if keys.just_pressed(KeyCode::KeyC) {
        mode.conduct = match mode.conduct {
            Conduct::CluckHunt => Conduct::RightOfWay,
            Conduct::RightOfWay => Conduct::CluckHunt,
        };
    }
    if keys.any_just_pressed([KeyCode::Enter, KeyCode::Space]) {
        restart.0 = false;
        next.set(GameState::Playing);
    }
}

fn menu_swipe(
    touches: Res<Touches>,
    mut selected: ResMut<SelectedModifier>,
    mut start_selection: ResMut<SwipeStartSelection>,
) {
    if touches.iter_just_pressed().next().is_some() {
        start_selection.0 = Some(selected.0);
    }
    for touch in touches.iter_just_released() {
        let delta = touch.position() - touch.start_position();
        if delta.x.abs() > 48.0 && delta.x.abs() > delta.y.abs() * 1.4 {
            selected.0 = start_selection.0.unwrap_or(selected.0);
            cycle_selection(&mut selected, if delta.x < 0.0 { 1 } else { -1 });
        }
        start_selection.0 = None;
    }
}

fn menu_buttons(
    interactions: Query<(&Interaction, &MenuAction), Changed<Interaction>>,
    mut selected: ResMut<SelectedModifier>,
    mut restart: ResMut<RestartRequested>,
    mut next: ResMut<NextState<GameState>>,
) {
    for (interaction, action) in &interactions {
        if *interaction != Interaction::Pressed {
            continue;
        }
        match *action {
            MenuAction::Previous => cycle_selection(&mut selected, -1),
            MenuAction::Next => cycle_selection(&mut selected, 1),
            MenuAction::Select(index) => selected.0 = CONDITIONS[index],
            MenuAction::Drive => {
                restart.0 = false;
                next.set(GameState::Playing);
            }
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
    best: Res<BestScore>,
    condition_bests: Res<ConditionBests>,
    settings: Res<Settings>,
    selected: Res<SelectedModifier>,
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
        best.0,
        &condition_bests,
        &settings,
        selected.0,
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
    best: Res<BestScore>,
    condition_bests: Res<ConditionBests>,
    settings: Res<Settings>,
    selected: Res<SelectedModifier>,
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
        || best.is_changed()
        || condition_bests.is_changed()
        || settings.is_changed();
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
        best.0,
        &condition_bests,
        &settings,
        selected.0,
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
    global_best: u32,
    condition_bests: &ConditionBests,
    settings: &Settings,
    selected: ModifierKind,
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
        (205.0, 142.0, (window_width * 0.64).clamp(260.0, 590.0))
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
    let earned: usize = CONDITIONS
        .iter()
        .map(|kind| medal_points(medal_for(*kind, condition_bests.by_kind[kind.index()])))
        .sum();

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
                padding: UiRect::axes(
                    px(if short { 8.0 } else { 12.0 }),
                    px(if short { 5.0 } else { 12.0 }),
                ),
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
                chip(bar, format!("BEST {global_best}"), short);
                chip(bar, format!("MEDALS {earned}/15"), short);
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
                                    condition_bests.by_kind[kind.index()],
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
            root.spawn(Node {
                flex_direction: FlexDirection::Column,
                align_items: AlignItems::Center,
                row_gap: px(if short { 2.0 } else { 6.0 }),
                ..default()
            })
            .with_children(|bottom| {
                bottom
                    .spawn((
                        Button,
                        MenuAction::Drive,
                        GlowAnimation::default(),
                        Node {
                            width: px(if short {
                                260.0
                            } else if portrait {
                                window_width * 0.80
                            } else {
                                330.0
                            }),
                            height: px(if short { 58.0 } else { 92.0 }),
                            border_radius: BorderRadius::all(px(18.0)),
                            align_items: AlignItems::Center,
                            justify_content: JustifyContent::Center,
                            flex_direction: FlexDirection::Column,
                            ..default()
                        },
                        MaterialNode(glow_materials.add(GlowButtonMaterial {
                            color_fill: to_vec4(selected_color),
                            color_glow: to_vec4(selected_color),
                            params: Vec4::new(0.0, 0.55, 0.34, 0.0),
                            params2: Vec4::new(reduced, 0.0, 0.0, 0.0),
                        })),
                    ))
                    .with_children(|button| {
                        button.spawn((
                            Text::new("DRIVE"),
                            font(if short { 27.0 } else { 38.0 }),
                            TextColor(ink()),
                        ));
                        button.spawn((
                            DriveHint,
                            Text::new(format!(
                                "{} - {}",
                                selected.display_name().to_uppercase(),
                                if portrait { "TAP" } else { "ENTER / SPACE" }
                            )),
                            font(if short { 9.0 } else { 11.0 }),
                            TextColor(ink().with_alpha(0.68)),
                        ));
                    });
                if !short {
                    bottom.spawn((
                        TipText,
                        Text::new(TIPS[0]),
                        font(if portrait { 11.0 } else { 13.0 }),
                        TextColor(dim()),
                    ));
                }
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
    layout: Res<MenuLayout>,
    built_at: Res<MenuBuiltAt>,
    mut tips: ResMut<TipState>,
    mut rows: Query<(&mut CarouselRow, &mut Node), Without<ConditionCard>>,
    mut cards: Query<(&ConditionCard, &mut Node, &MaterialNode<MenuCardMaterial>)>,
    mut dots: Query<(&CarouselDot, &mut BackgroundColor), Without<ConditionCard>>,
    mut letters: Query<(&TitleLetter, &mut Node), (Without<ConditionCard>, Without<CarouselRow>)>,
    mut tip_text: Query<(&mut Text, &mut TextColor), (With<TipText>, Without<DriveHint>)>,
    mut drive_hints: Query<&mut Text, (With<DriveHint>, Without<TipText>)>,
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
    let drive_label = format!(
        "{} - {}",
        selected.0.display_name().to_uppercase(),
        if *layout == MenuLayout::Portrait {
            "TAP"
        } else {
            "ENTER / SPACE"
        }
    );
    for mut hint in &mut drive_hints {
        hint.0.clone_from(&drive_label);
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
    stripes: Query<&MaterialNode<HazardStripeMaterial>>,
    vignettes: Query<&MaterialNode<VignetteMaterial>>,
    mut glows: Query<(
        &MaterialNode<GlowButtonMaterial>,
        &Interaction,
        &mut GlowAnimation,
    )>,
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
    for (handle, interaction, mut animation) in &mut glows {
        let target = match *interaction {
            Interaction::Pressed => 1.0,
            Interaction::Hovered => 0.55,
            Interaction::None => 0.0,
        };
        animation.0 += (target - animation.0) * k;
        if let Some(mut material) = glow_materials.get_mut(handle) {
            material.color_fill = to_vec4(selected.0.color());
            material.color_glow = to_vec4(selected.0.color());
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
}
