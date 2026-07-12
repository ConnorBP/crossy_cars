// Animated water-pond material.
// `base` is a configurable tint; `time` advances the ripple phase each frame.
// Ripples are computed purely in the fragment shader from the plane UVs, so the
// mesh stays a flat quad and no vertex displacement is required.
#import bevy_pbr::forward_io::VertexOutput

@group(#{MATERIAL_BIND_GROUP}) @binding(0) var<uniform> base: vec4<f32>;
@group(#{MATERIAL_BIND_GROUP}) @binding(1) var<uniform> time: vec4<f32>;

@fragment
fn fragment(mesh: VertexOutput) -> @location(0) vec4<f32> {
    // Remap UV from [0,1] to [-1,1] and measure radial distance from the center.
    let uv = mesh.uv * 2.0 - 1.0;
    let r = length(uv);

    // Outward-traveling sine wave (phase scrolls with time).
    let wave = sin(r * 12.0 - time.x * 3.0) * 0.5 + 0.5;

    // Two blues: deep center, lighter troughs.
    let deep = vec4<f32>(0.05, 0.25, 0.45, 1.0);
    let shallow = vec4<f32>(0.20, 0.55, 0.75, 1.0);
    let col = mix(deep, shallow, wave);

    // Blend slightly toward the configurable base tint.
    return mix(col, base, 0.3);
}
