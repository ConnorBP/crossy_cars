use bevy::prelude::*;

/// Emitted when the car runs over a wandering chicken.
#[derive(Message)]
pub struct ChickenHit;

/// Emitted when the car collects a coin (bonus score + time).
#[derive(Message)]
pub struct CoinCollected;

/// Emitted exactly once when the player's oriented footprint first enters a
/// pond during an active round. Pond gameplay owns the persistent latch; this
/// message is the presentation/integration notification for that transition.
#[derive(Message, Clone, Copy, Debug, PartialEq)]
pub struct PondEntered {
    pub position: Vec3,
}

/// Emitted when the car collides with a solid obstacle (building / tree /
/// lamp post). `impact_speed` is the car's speed at the moment of impact,
/// used by the health system to compute damage.
#[derive(Message)]
pub struct ObstacleHit {
    pub impact_speed: f32,
}
