// Mode-select card background: vertical gradient, mode-colored accent strip,
// selection rim, and a diagonal shine sweep when the card becomes selected.
// WebGL2-safe: uniform vec4s only.
#import bevy_ui::ui_vertex_output::UiVertexOutput

@group(1) @binding(0) var<uniform> color_top: vec4<f32>;    // gradient top
@group(1) @binding(1) var<uniform> color_bottom: vec4<f32>; // gradient bottom
@group(1) @binding(2) var<uniform> color_accent: vec4<f32>; // per-mode theme color
// params: x = time, y = selected (0..1, eased in Rust), z = shine progress (0..1), w = reduced_motion (0/1)
@group(1) @binding(3) var<uniform> params: vec4<f32>;

fn sd_rounded_rect(p: vec2<f32>, half_size: vec2<f32>, radius: f32) -> f32 {
    let q = abs(p) - half_size + vec2<f32>(radius);
    return length(max(q, vec2<f32>(0.0))) + min(max(q.x, q.y), 0.0) - radius;
}

@fragment
fn fragment(in: UiVertexOutput) -> @location(0) vec4<f32> {
    let half_size = 0.5 * in.size;
    let p = in.uv * in.size - half_size;
    let radius = in.border_radius.x;
    let d = sd_rounded_rect(p, half_size, radius);
    let selected = params.y;

    // Base: vertical gradient, slightly brighter when selected.
    var col = mix(color_top, color_bottom, in.uv.y);
    col = vec4<f32>(col.rgb * (1.0 + selected * 0.18), col.a);

    // Accent strip along the top edge (mode theme color).
    let strip = 1.0 - smoothstep(4.0, 7.0, in.uv.y * in.size.y);
    col = vec4<f32>(mix(col.rgb, color_accent.rgb, strip * 0.9), col.a);

    // Faint diagonal texture so big cards don't look flat (linear space, keep tiny).
    let tex = sin((p.x + p.y) * 0.11) * 0.003;
    col = vec4<f32>(col.rgb + vec3<f32>(tex), col.a);

    // Selection rim: accent-colored line hugging the border, with a soft
    // breathing outer halo (frozen under reduced motion).
    var breathe = 0.5 + 0.5 * sin(params.x * 2.4);
    breathe = mix(breathe, 0.5, params.w);
    let rim = (1.0 - smoothstep(-2.5, -0.5, d)) * smoothstep(-5.0, -2.5, d);
    let halo = exp(min(d, 0.0) * 0.35) * 0.20 * (0.6 + breathe * 0.4);
    let rim_amount = (rim + halo) * selected;
    col = vec4<f32>(mix(col.rgb, color_accent.rgb * 1.35, clamp(rim_amount, 0.0, 1.0)), col.a);

    // Shine sweep: a bright diagonal band crossing the card once (z: 0 -> 1).
    let sweep = params.z;
    if (sweep > 0.0 && sweep < 1.0 && params.w < 0.5) {
        let band_pos = (in.uv.x + in.uv.y * 0.5) / 1.5;           // 0..1 across the diagonal
        let center = sweep * 1.4 - 0.2;                            // overshoot both ends
        let band = 1.0 - smoothstep(0.0, 0.12, abs(band_pos - center));
        col = vec4<f32>(col.rgb + vec3<f32>(band * 0.45), col.a);
    }

    // Cut to the rounded shape with a ~1.5px AA edge.
    let inside = 1.0 - smoothstep(-1.5, 0.0, d);
    return vec4<f32>(col.rgb, col.a * inside);
}
