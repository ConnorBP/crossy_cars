// Pulsing arcade CTA button: rounded-rect SDF fill + animated rim glow.
// WebGL2-safe: uniform vec4s only.
#import bevy_ui::ui_vertex_output::UiVertexOutput

@group(1) @binding(0) var<uniform> color_fill: vec4<f32>;
@group(1) @binding(1) var<uniform> color_glow: vec4<f32>;
// params: x = time, y = pulse speed (Hz-ish), z = base glow strength 0..1,
//         w = boost (hover/press adds up to 1.0; reduced-motion freezes pulse at mid)
@group(1) @binding(2) var<uniform> params: vec4<f32>;
// params2: x = reduced_motion (0/1), y/z/w reserved
@group(1) @binding(3) var<uniform> params2: vec4<f32>;

// Signed distance to a rounded rectangle centred at origin.
fn sd_rounded_rect(p: vec2<f32>, half_size: vec2<f32>, radius: f32) -> f32 {
    let q = abs(p) - half_size + vec2<f32>(radius);
    return length(max(q, vec2<f32>(0.0))) + min(max(q.x, q.y), 0.0) - radius;
}

@fragment
fn fragment(in: UiVertexOutput) -> @location(0) vec4<f32> {
    let half_size = 0.5 * in.size;
    let p = in.uv * in.size - half_size;
    // use the node's top-left corner radius for the whole shape
    let radius = in.border_radius.x;

    // Leave a margin so the glow has room to breathe outside the fill.
    let glow_room = 18.0;
    let body_half = half_size - vec2<f32>(glow_room);
    let d = sd_rounded_rect(p, body_half, radius);

    var pulse = 0.5 + 0.5 * sin(params.x * params.y * 6.28318);
    pulse = mix(pulse, 0.5, params2.x); // reduced motion: hold steady
    let strength = clamp(params.z + params.w + pulse * 0.35, 0.0, 2.0);

    // Fill with a slight vertical gradient for depth.
    let grad = mix(1.08, 0.86, in.uv.y);
    var fill = vec4<f32>(color_fill.rgb * grad, color_fill.a);

    // Inner bevel line just inside the edge.
    let bevel = smoothstep(-3.0, -1.5, d) * (1.0 - smoothstep(-1.5, 0.0, d));
    fill = vec4<f32>(mix(fill.rgb, vec3<f32>(1.0), bevel * 0.35), fill.a);

    // Outer glow: exponential falloff outside the shape.
    let glow_a = exp(-max(d, 0.0) * 0.22) * strength;
    let glow = vec4<f32>(color_glow.rgb, color_glow.a * glow_a);

    // Composite: fill inside (AA edge), glow outside.
    let inside = 1.0 - smoothstep(-1.0, 1.0, d);
    return vec4<f32>(
        mix(glow.rgb, fill.rgb, inside),
        max(fill.a * inside, glow.a * (1.0 - inside)),
    );
}
