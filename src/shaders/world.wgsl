struct Camera {
    view_projection: mat4x4<f32>,
    inverse_view_projection: mat4x4<f32>,
    eye_time: vec4<f32>,
    sun_fog: vec4<f32>,
    selected: vec4<f32>,
    // sky/fog rgb are linear; ambient and sun_color are multipliers.
    // sky_horizon.w is sun glow, sky_zenith.w is time of day; other w lanes are 0.
    sky_horizon: vec4<f32>,
    sky_zenith: vec4<f32>,
    fog_color: vec4<f32>,
    ambient_ground: vec4<f32>,
    ambient_sky: vec4<f32>,
    sun_color: vec4<f32>,
};

@group(0) @binding(0) var<uniform> camera: Camera;
@group(0) @binding(1) var material_tiles: texture_2d_array<f32>;
@group(0) @binding(2) var material_sampler: sampler;

// Historical AO/variation/direction brightness. Kept off the camera uniform so
// atmosphere parameters can change without rewriting packed-quad data.
struct TerrainLighting {
    brightness: array<vec4<f32>, 21>,
};

@group(0) @binding(3) var<uniform> terrain_lighting: TerrainLighting;

fn linear_color(color: vec3<f32>) -> vec3<f32> {
    return pow(max(color, vec3<f32>(0.0)), vec3<f32>(2.2));
}

fn hash(point: vec3<f32>) -> f32 {
    var p = fract(point * 0.1031);
    p += dot(p, p.yzx + vec3<f32>(33.33));
    return fract((p.x + p.y) * p.z);
}

fn noise(point: vec3<f32>) -> f32 {
    let cell = floor(point);
    let f = fract(point);
    let blend = f * f * (3.0 - 2.0 * f);
    return mix(
        mix(mix(hash(cell), hash(cell + vec3<f32>(1., 0., 0.)), blend.x),
            mix(hash(cell + vec3<f32>(0., 1., 0.)), hash(cell + vec3<f32>(1., 1., 0.)), blend.x), blend.y),
        mix(mix(hash(cell + vec3<f32>(0., 0., 1.)), hash(cell + vec3<f32>(1., 0., 1.)), blend.x),
            mix(hash(cell + vec3<f32>(0., 1., 1.)), hash(cell + vec3<f32>(1., 1., 1.)), blend.x), blend.y), blend.z);
}

// Sky and water reflection. Distance haze uses camera.fog_color.
fn atmosphere(direction: vec3<f32>) -> vec3<f32> {
    let up = clamp(direction.y, 0.0, 1.0);
    let sun_amount = max(dot(direction, camera.sun_fog.xyz), 0.0);
    let glow = camera.sky_horizon.w;
    let horizon = camera.sky_horizon.xyz;
    let zenith = camera.sky_zenith.xyz;
    var color = mix(horizon, zenith, pow(up, 0.48));
    color += linear_color(vec3<f32>(1.0, 0.68, 0.34)) * pow(sun_amount, 12.0) * 0.22 * glow;
    color += vec3<f32>(1.0, 0.88, 0.64) * pow(sun_amount, 128.0) * 0.18 * glow;
    let disk = smoothstep(0.99955, 0.99978, sun_amount) * glow;
    color = mix(color, vec3<f32>(1.0, 0.94, 0.76), disk);
    return color;
}

struct SkyVertex {
    @builtin(position) clip: vec4<f32>,
    @location(0) ray: vec3<f32>,
};

@vertex
fn vs_sky(@builtin(vertex_index) index: u32) -> SkyVertex {
    let points = array<vec2<f32>, 3>(vec2<f32>(-1., -1.), vec2<f32>(3., -1.), vec2<f32>(-1., 3.));
    let xy = points[index];
    let world = camera.inverse_view_projection * vec4<f32>(xy, 1.0, 1.0);
    var out: SkyVertex;
    out.clip = vec4<f32>(xy, 1.0, 1.0);
    out.ray = world.xyz / world.w - camera.eye_time.xyz;
    return out;
}

@fragment
fn fs_sky(input: SkyVertex) -> @location(0) vec4<f32> {
    let ray = normalize(input.ray);
    var color = atmosphere(ray);
    if ray.y > 0.035 {
        let drift = camera.eye_time.w * 0.004;
        let cloud_point = vec3<f32>(ray.x / (ray.y + 0.15), ray.z / (ray.y + 0.15), drift);
        let cloud = noise(cloud_point * 1.8) * 0.66 + noise(cloud_point * 4.1) * 0.24 + noise(cloud_point * 8.2) * 0.1;
        let mask = smoothstep(0.57, 0.76, cloud) * smoothstep(0.035, 0.23, ray.y) * 0.62;
        let cloud_color = mix(
            linear_color(vec3<f32>(0.10, 0.13, 0.20)),
            linear_color(vec3<f32>(0.97, 0.96, 0.89)),
            camera.sky_horizon.w);
        color = mix(color, cloud_color, mask);
    }
    return vec4<f32>(color, 1.0);
}

// Chunk-local greedy quad. Bit layout is PackedQuad in src/world.rs.
struct QuadInput {
    @location(0) word0: u32,
    @location(1) word1: u32,
};

// Per-draw chunk origin. Camera-relative rendering can subtract an anchor here
// without rewriting resident quad buffers.
struct ChunkOrigin {
    x: i32,
    y: i32,
    z: i32,
    pad: i32,
};

var<immediate> chunk_origin: ChunkOrigin;

struct TerrainFragment {
    @builtin(position) clip: vec4<f32>,
    @location(0) world: vec3<f32>,
    @location(1) normal: vec3<f32>,
    @location(2) brightness: f32,
    @location(3) @interpolate(flat) texture_layer: u32,
    @location(4) @interpolate(flat) water: u32,
    @location(5) @interpolate(flat) cutout: u32,
};

fn corner_brightness(ao: u32, variation: u32, direction: u32) -> f32 {
    let index = ao + variation * 4u + direction * 28u;
    let packed = terrain_lighting.brightness[index / 4u];
    let lane = index % 4u;
    if lane == 0u { return packed.x; }
    if lane == 1u { return packed.y; }
    if lane == 2u { return packed.z; }
    return packed.w;
}

fn triangle_corner(vertex_index: u32, positive: bool, diagonal_high: bool) -> u32 {
    // Same four patterns as DecodedQuad::triangle_corners.
    if positive && diagonal_high {
        let pattern = array<u32, 6>(0u, 1u, 3u, 1u, 2u, 3u);
        return pattern[vertex_index];
    }
    if positive {
        let pattern = array<u32, 6>(0u, 1u, 2u, 0u, 2u, 3u);
        return pattern[vertex_index];
    }
    if diagonal_high {
        let pattern = array<u32, 6>(0u, 3u, 1u, 1u, 3u, 2u);
        return pattern[vertex_index];
    }
    let pattern = array<u32, 6>(0u, 2u, 1u, 0u, 3u, 2u);
    return pattern[vertex_index];
}

@vertex
fn vs_terrain(input: QuadInput, @builtin(vertex_index) vertex_index: u32) -> TerrainFragment {
    let x = input.word0 & 63u;
    let y = (input.word0 >> 6u) & 63u;
    let z = (input.word0 >> 12u) & 63u;
    let width = f32(((input.word0 >> 18u) & 31u) + 1u);
    let height = f32(((input.word0 >> 23u) & 31u) + 1u);
    let axis = (input.word0 >> 28u) & 3u;
    let positive = ((input.word0 >> 30u) & 1u) == 1u;
    let diagonal_high = (input.word0 >> 31u) == 1u;
    let corner = triangle_corner(vertex_index, positive, diagonal_high);
    var uv = vec2<f32>(0.0, 0.0);
    if corner == 1u { uv = vec2<f32>(width, 0.0); }
    else if corner == 2u { uv = vec2<f32>(width, height); }
    else if corner == 3u { uv = vec2<f32>(0.0, height); }
    var local = vec3<f32>(f32(x), f32(y), f32(z));
    let u_axis = (axis + 1u) % 3u;
    let v_axis = (axis + 2u) % 3u;
    if u_axis == 0u { local.x += uv.x; }
    else if u_axis == 1u { local.y += uv.x; }
    else { local.z += uv.x; }
    if v_axis == 0u { local.x += uv.y; }
    else if v_axis == 1u { local.y += uv.y; }
    else { local.z += uv.y; }
    let world = vec3<f32>(f32(chunk_origin.x), f32(chunk_origin.y), f32(chunk_origin.z)) + local;
    var normal = vec3<f32>(0.0);
    let sign = select(-1.0, 1.0, positive);
    if axis == 0u { normal.x = sign; }
    else if axis == 1u { normal.y = sign; }
    else { normal.z = sign; }
    let ao = (input.word1 >> (corner * 2u)) & 3u;
    let variation = (input.word1 >> 13u) & 7u;
    var direction = 2u;
    if axis == 1u { direction = select(0u, 1u, positive); }
    var out: TerrainFragment;
    out.clip = camera.view_projection * vec4<f32>(world, 1.0);
    out.world = world;
    out.normal = normal;
    out.brightness = corner_brightness(ao, variation, direction);
    out.texture_layer = (input.word1 >> 8u) & 31u;
    out.water = (input.word1 >> 16u) & 1u;
    out.cutout = (input.word1 >> 17u) & 1u;
    return out;
}

@fragment
fn fs_terrain(input: TerrainFragment) -> @location(0) vec4<f32> {
    let eye_direction = normalize(camera.eye_time.xyz - input.world);
    let distance = length(camera.eye_time.xyz - input.world);
    let normal = normalize(input.normal);
    // World-axis coordinates repeat one 32x32 tile per block, including merged
    // faces and negative coordinates. Side V increases from top to bottom.
    var uv = input.world.xz;
    if abs(normal.x) > 0.5 {
        uv = vec2<f32>(input.world.z, -input.world.y);
    } else if abs(normal.z) > 0.5 {
        uv = vec2<f32>(input.world.x, -input.world.y);
    }
    let texel = textureSample(material_tiles, material_sampler, uv, i32(input.texture_layer));
    // Leaf cutout is sampled in every pass; only the cutout bit discards.
    // Sampling stays outside the branch so the texture lookup is uniform.
    if input.cutout != 0u && texel.a < 0.5 {
        discard;
    }
    let albedo = texel.rgb * input.brightness;
    var color: vec3<f32>;
    var alpha = 1.0;

    if input.water != 0u {
        let t = camera.eye_time.w;
        let wave_a = dot(input.world.xz, vec2<f32>(0.53, 0.31)) + t * 0.7;
        let wave_b = dot(input.world.xz, vec2<f32>(-0.27, 0.72)) - t * 0.52;
        let water_normal = normalize(normal + vec3<f32>(cos(wave_a) * 0.055, 0., cos(wave_b) * 0.055));
        let reflected = reflect(-eye_direction, water_normal);
        let fresnel = 0.10 + 0.72 * pow(1.0 - max(dot(eye_direction, water_normal), 0.0), 4.0);
        let hemisphere = water_normal.y * 0.5 + 0.5;
        let ambient = mix(camera.ambient_ground.xyz, camera.ambient_sky.xyz, hemisphere);
        let sun_term = camera.sun_color.xyz * max(dot(water_normal, camera.sun_fog.xyz), 0.0);
        let depth_color = albedo * (0.88 + sin(wave_a) * sin(wave_b) * 0.04) * (ambient + sun_term);
        let half_vector = camera.sun_fog.xyz + eye_direction;
        let half_direction = half_vector / max(length(half_vector), 1e-4);
        let sparkle = pow(max(dot(water_normal, half_direction), 0.0), 220.0);
        color = mix(depth_color, atmosphere(reflected), fresnel) + vec3<f32>(1.0, 0.89, 0.66) * sparkle * 0.65 * camera.sky_horizon.w;
        alpha = mix(0.55, 0.9, fresnel);
    } else {
        let diffuse = max(dot(normal, camera.sun_fog.xyz), 0.0);
        let hemisphere = normal.y * 0.5 + 0.5;
        let ambient = mix(camera.ambient_ground.xyz, camera.ambient_sky.xyz, hemisphere);
        let sunlight = camera.sun_color.xyz * diffuse;
        color = albedo * (ambient + sunlight);
    }

    let fog_start = camera.sun_fog.w * 0.22;
    let fog_span = max(camera.sun_fog.w - fog_start, 1.0);
    let fog = smoothstep(0.0, 1.0, (distance - fog_start) / fog_span);
    color = mix(color, camera.fog_color.xyz, fog * 0.94);
    return vec4<f32>(color, alpha);
}

@vertex
fn vs_selection(@builtin(vertex_index) index: u32) -> @builtin(position) vec4<f32> {
    let corners = array<vec3<f32>, 8>(
        vec3<f32>(0., 0., 0.), vec3<f32>(1., 0., 0.), vec3<f32>(1., 1., 0.), vec3<f32>(0., 1., 0.),
        vec3<f32>(0., 0., 1.), vec3<f32>(1., 0., 1.), vec3<f32>(1., 1., 1.), vec3<f32>(0., 1., 1.));
    let edges = array<u32, 24>(0u,1u,1u,2u,2u,3u,3u,0u,4u,5u,5u,6u,6u,7u,7u,4u,0u,4u,1u,5u,2u,6u,3u,7u);
    let local = (corners[edges[index]] - vec3<f32>(0.5)) * 1.006 + vec3<f32>(0.5);
    return camera.view_projection * vec4<f32>(camera.selected.xyz + local, 1.0);
}

@fragment
fn fs_selection() -> @location(0) vec4<f32> {
    return vec4<f32>(0.94, 0.90, 0.66, 1.0);
}
