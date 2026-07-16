// Fullscreen readability layer between the live game background and the menu:
// radial vignette + subtle diagonal speed-lines drifting outward.
// WebGL2-safe: uniform vec4s only.
#import bevy_ui::ui_vertex_output::UiVertexOutput

@group(1) @binding(0) var<uniform> color: vec4<f32>; // tint (usually near-black)
// params: x = time, y = vignette strength 0..1, z = speed-line strength 0..1, w = reduced_motion (0/1)
@group(1) @binding(1) var<uniform> params: vec4<f32>;

@fragment
fn fragment(in: UiVertexOutput) -> @location(0) vec4<f32> {
    // Radial vignette: clear-ish in the middle, dark toward edges/corners.
    let c = in.uv - vec2<f32>(0.5);
    let r = length(c * vec2<f32>(1.0, 1.15)); // slightly stronger top/bottom
    let vig = smoothstep(0.25, 0.85, r) * params.y + 0.25 * params.y;

    // Speed lines: faint dark diagonal streaks drifting away from center.
    var lines = 0.0;
    if (params.w < 0.5) {
        let ang = atan2(c.y, c.x);
        let streak = sin(ang * 24.0 + r * 6.0 - params.x * 0.6);
        lines = max(streak, 0.0) * smoothstep(0.35, 0.8, r) * params.z;
    }

    let a = clamp(vig + lines * 0.15, 0.0, 0.92);
    return vec4<f32>(color.rgb, color.a * a);
}
