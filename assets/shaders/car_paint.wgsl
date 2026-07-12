// Stylized red metallic paint for the smooth ellipsoid car body.
// The mesh has baked ellipsoid geometry + analytic smooth normals, so the
// reflection vector sweeps naturally across the surface on WebGL2.
#import bevy_pbr::forward_io::VertexOutput

@group(#{MATERIAL_BIND_GROUP}) @binding(0) var<uniform> color: vec4<f32>;

@fragment
fn fragment(mesh: VertexOutput) -> @location(0) vec4<f32> {
    let n = normalize(mesh.world_normal);
    // Fixed orthographic isometric camera direction.
    let view = normalize(vec3<f32>(1.0, 1.0, 1.0));
    let reflected = reflect(-view, n);

    // Procedural world reflection: dark warm ground, bright horizon strip,
    // and blue sky. Smooth body normals make these bands curve across the car.
    let ground = vec3<f32>(0.035, 0.025, 0.020);
    let horizon = vec3<f32>(0.80, 0.84, 0.92);
    let sky = vec3<f32>(0.34, 0.52, 0.82);
    var environment = mix(ground, horizon, smoothstep(-0.34, 0.04, reflected.y));
    environment = mix(environment, sky, smoothstep(0.08, 0.62, reflected.y));

    let paint = color.rgb;
    let ndotv = max(dot(n, view), 0.0);
    let fresnel = pow(1.0 - ndotv, 3.0);

    // Metallic reflections are paint-colored. Preserve a small neutral share
    // in the brightest reflection so it still reads as glossy clearcoat.
    let metal_tint = mix(paint * 1.65, vec3<f32>(1.0), 0.10 + fresnel * 0.12);
    let reflection = environment * metal_tint * (1.15 + fresnel * 0.75);

    // Deliberately broad highlight: box normals could never meet a sharp
    // half-vector, while the rounded body now gives this a moving curved sweep.
    let sun = normalize(vec3<f32>(30.0, 25.0, 15.0));
    let half_vector = normalize(view + sun);
    let spec_amount = pow(max(dot(n, half_vector), 0.0), 4.0);
    let clearcoat = vec3<f32>(1.0, 0.88, 0.72) * spec_amount * 2.4;

    // Saturated red midtone prevents the reflection from becoming silver.
    let base = paint * (0.16 + 0.12 * max(n.y, 0.0));
    return vec4<f32>(base + reflection + clearcoat, 1.0);
}
