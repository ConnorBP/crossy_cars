//! Centralized color palette so the whole game can be re-themed from one place.
use bevy::prelude::Color;

pub const SKY: Color = Color::srgb(0.53, 0.81, 0.92);
pub const GRASS_LIGHT: Color = Color::srgb(0.30, 0.60, 0.30);
pub const GRASS_DARK: Color = Color::srgb(0.42, 0.60, 0.42);
pub const ASPHALT: Color = Color::srgb(0.13, 0.13, 0.14);
pub const CONCRETE: Color = Color::srgb(0.72, 0.71, 0.68);
pub const LANE_WHITE: Color = Color::srgb(0.9, 0.9, 0.85);

pub const CAR_BODY: Color = Color::srgb(0.90, 0.10, 0.10);
pub const CAR_CABIN: Color = Color::srgb(0.10, 0.10, 0.20);
pub const CAR_WHEEL: Color = Color::srgb(0.05, 0.05, 0.05);

pub const COIN: Color = Color::srgb(1.00, 0.84, 0.10);

pub const HUD_TEXT: Color = Color::WHITE;
pub const HUD_ACCENT: Color = Color::srgb(1.0, 0.8, 0.0);
