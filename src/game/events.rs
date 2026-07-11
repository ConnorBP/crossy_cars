use bevy::prelude::*;

/// Emitted when the car runs over a wandering chicken.
#[derive(Message)]
pub struct ChickenHit;

/// Emitted when the car collects a coin (bonus score + time).
#[derive(Message)]
pub struct CoinCollected;
