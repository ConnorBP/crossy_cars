use bevy::{camera::ScalingMode, prelude::*};

use crate::car::Car;
use crate::game::resources::GameConfig;
use crate::game::state::GameState;

/// Exponential smoothing rate for camera follow / zoom (higher = snappier).
const SMOOTH: f32 = 6.0;

pub struct CameraPlugin;

impl Plugin for CameraPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(Startup, spawn_camera)
            .add_systems(Update, follow_camera.run_if(in_state(GameState::Playing)));
    }
}

fn spawn_camera(mut commands: Commands) {
    commands.spawn((
        Camera3d::default(),
        Projection::from(OrthographicProjection {
            scaling_mode: ScalingMode::FixedVertical {
                viewport_height: 10.0,
            },
            ..OrthographicProjection::default_3d()
        }),
        Transform::from_xyz(12.0, 12.0, 12.0).looking_at(Vec3::ZERO, Vec3::Y),
    ));
}

fn follow_camera(
    car: Query<(&Transform, &Car), (With<Car>, Without<Camera3d>)>,
    mut camera: Query<(&mut Transform, &mut Projection), (With<Camera3d>, Without<Car>)>,
    cfg: Res<GameConfig>,
    time: Res<Time>,
) {
    let Ok((car_t, car)) = car.single() else {
        return;
    };
    let Ok((mut cam_t, mut proj)) = camera.single_mut() else {
        return;
    };

    let dt = time.delta_secs();
    let t = 1.0 - (-SMOOTH * dt).exp();

    // Look-ahead: nudge the view in the car's forward direction (model -Z).
    let fwd = car_t.rotation * Vec3::new(0.0, 0.0, -1.0);
    let look_target = car_t.translation + fwd * 1.5;
    let desired = car_t.translation + cfg.cam_offset + fwd * 1.5;

    // Smoothed follow: exponential lerp toward the desired iso position.
    cam_t.translation = cam_t.translation.lerp(desired, t);

    // Rotation tracks the car so the iso angle stays consistent while the
    // focus point leads slightly into the car's travel direction.
    cam_t.look_at(look_target, Vec3::Y);

    // Speed zoom: widen the orthographic viewport as speed rises. The current
    // viewport_height lives in the projection itself, so we read-modify it.
    let ratio = if cfg.max_speed > 0.0 {
        car.speed.abs() / cfg.max_speed
    } else {
        0.0
    };
    let target_vh = 10.0 + ratio * 2.0;
    let current_vh = match &*proj {
        Projection::Orthographic(o) => match o.scaling_mode {
            ScalingMode::FixedVertical { viewport_height } => viewport_height,
            _ => 10.0,
        },
        _ => 10.0,
    };
    let vh = current_vh + (target_vh - current_vh) * t;
    if let Projection::Orthographic(ref mut o) = *proj {
        o.scaling_mode = ScalingMode::FixedVertical { viewport_height: vh };
    }
}
