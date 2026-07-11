use bevy::{camera::ScalingMode, prelude::*};

use crate::car::Car;
use crate::game::resources::GameConfig;
use crate::game::state::GameState;

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
    car: Query<&Transform, (With<Car>, Without<Camera3d>)>,
    mut camera: Query<&mut Transform, (With<Camera3d>, Without<Car>)>,
    cfg: Res<GameConfig>,
) {
    let Ok(car_t) = car.single() else {
        return;
    };
    let Ok(mut cam_t) = camera.single_mut() else {
        return;
    };
    let pos = car_t.translation;
    *cam_t = Transform::from_translation(pos + cfg.cam_offset).looking_at(pos, Vec3::Y);
}
