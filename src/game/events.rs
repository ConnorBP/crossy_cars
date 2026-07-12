use bevy::prelude::*;

/// Emitted when the car runs over a wandering chicken.
#[derive(Message)]
pub struct ChickenHit;

/// Emitted when the car collects a coin (bonus score + time).
#[derive(Message)]
pub struct CoinCollected;

/// Emitted when the car collides with a solid obstacle (building / tree /
/// lamp post). `impact_speed` is the car's speed at the moment of impact,
/// used by the health system to compute damage.
#[derive(Message)]
pub struct ObstacleHit {
    pub impact_speed: f32,
}
