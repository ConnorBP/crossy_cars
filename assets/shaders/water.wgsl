// Stylized opaque pond water for the cached flat circle mesh. This is a
// WebGL2-safe fragment-only treatment: no displacement, scene textures,
// refraction, transparency, prepass, storage buffers, or normal-map noise.
#import bevy_pbr::forward_io::VertexOutput

@group(#{MATERIAL_BIND_GROUP}) @binding(0) var<uniform> deep: vec4<f32>;
@group(#{MATERIAL_BIND_GROUP}) @binding(1) var<uniform> shallow: vec4<f32>;
@group(#{MATERIAL_BIND_GROUP}) @binding(2) var<uniform> motion: vec4<f32>;
@group(#{MATERIAL_BIND_GROUP}) @binding(3) var<uniform> detail: vec4<f32>;

@fragment
fn fragment(mesh: VertexOutput) -> @location(0) vec4<f32> {
    // The unit circle's UVs remain ellipse-local when the mesh is scaled in
    // world space. Radial distance therefore gives stable pseudo-depth for all
    // three pond shapes without sampling scene depth.
    let local_uv = mesh.uv * 2.0 - 1.0;
    let radius = clamp(length(local_uv), 0.0, 1.0);
    let pseudo_depth = pow(clamp(1.0 - radius, 0.0, 1.0), detail.x);

    // One bounded, low-frequency shared mask. It participates in only the
    // shallow/deep color balance and the minor narrow edge accent below.
    let phase = motion.x;
    let frequency = motion.y;
    let wave_a = sin(dot(local_uv, vec2<f32>(1.00, 0.58)) * frequency + phase * 0.52);
    let wave_b = sin(dot(local_uv, vec2<f32>(-0.42, 0.91)) * frequency * 0.73 - phase * motion.z);
    let wave_mask = clamp(0.5 + 0.25 * (wave_a + wave_b), 0.0, 1.0);

    let depth_mix = clamp(pseudo_depth + (wave_mask - 0.5) * motion.w, 0.0, 1.0);
    var color = mix(shallow.rgb, deep.rgb, depth_mix);

    // A narrow shoreline-only, deliberately posterized highlight reads as
    // restrained graphic foam rather than a bright animated outline.
    let edge_start = 1.0 - detail.y;
    let edge_band = smoothstep(edge_start, 1.0, radius);
    let poster_steps = max(detail.z, 1.0);
    let poster_wave = floor(wave_mask * poster_steps) / poster_steps;
    let edge_accent = edge_band * poster_wave * detail.w;
    color += mix(vec3<f32>(0.18, 0.28, 0.27), shallow.rgb, 0.35) * edge_accent;

    return vec4<f32>(clamp(color, vec3<f32>(0.0), vec3<f32>(1.0)), 1.0);
}
