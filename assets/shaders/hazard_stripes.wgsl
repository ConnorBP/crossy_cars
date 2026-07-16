// Animated road-hazard chevron bar (title underline / footer ribbon).
// WebGL2-safe: uniform vec4s only, no storage buffers.
#import bevy_ui::ui_vertex_output::UiVertexOutput

@group(1) @binding(0) var<uniform> color_a: vec4<f32>; // stripe color (yellow)
@group(1) @binding(1) var<uniform> color_b: vec4<f32>; // gap color (near-black)
// params: x = time (secs), y = stripe width in px, z = scroll speed px/s, w = master alpha
@group(1) @binding(2) var<uniform> params: vec4<f32>;

@fragment
fn fragment(in: UiVertexOutput) -> @location(0) vec4<f32> {
    let p = in.uv * in.size;
    let stripe_w = max(params.y, 2.0);
    // 45-degree stripes scrolling horizontally
    let t = p.x + p.y - params.x * params.z;
    let band = fract(t / (stripe_w * 2.0));
    // soft edge ~1px for cheap AA
    let edge = 1.0 / (stripe_w * 2.0);
    let m = smoothstep(0.5 - edge, 0.5 + edge, band);
    var col = mix(color_a, color_b, m);

    // fade the bar out toward left/right ends for a "painted on road" feel
    let end_fade = smoothstep(0.0, 0.06, in.uv.x) * smoothstep(1.0, 0.94, in.uv.x);
    col.a = col.a * end_fade * params.w;
    return col;
}
