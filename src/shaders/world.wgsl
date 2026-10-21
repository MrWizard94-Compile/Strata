struct Camera {
    view_projection: mat4x4<f32>,
    inverse_view_projection: mat4x4<f32>,
    eye_time: vec4<f32>,
    sun_fog: vec4<f32>,
    selected: vec4<f32>,
    // Appended day/night data. xyz colors are linear; w is glow or time of day.
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

// Shared by the background, distance haze, and water's environment reflection.
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
            vec3<f32>(0.10, 0.13, 0.20),
            linear_color(vec3<f32>(0.97, 0.96, 0.89)),
            camera.sky_horizon.w);
        color = mix(color, cloud_color, mask);
    }
    return vec4<f32>(color, 1.0);
}

struct TerrainVertex {
    @location(0) position: vec3<f32>,
    @location(1) normal: vec3<f32>,
    @location(2) color: vec4<f32>,
    @location(3) texture_layer: u32,
};

struct TerrainFragment {
    @builtin(position) clip: vec4<f32>,
    @location(0) world: vec3<f32>,
    @location(1) normal: vec3<f32>,
    @location(2) color: vec4<f32>,
    @location(3) @interpolate(flat) texture_layer: u32,
};

@vertex
fn vs_terrain(input: TerrainVertex) -> TerrainFragment {
    var out: TerrainFragment;
    out.clip = camera.view_projection * vec4<f32>(input.position, 1.0);
    out.world = input.position;
    out.normal = input.normal;
    out.color = input.color;
    out.texture_layer = input.texture_layer;
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
    let albedo = textureSample(material_tiles, material_sampler, uv, i32(input.texture_layer)).rgb
        * input.color.rgb;
    var color: vec3<f32>;

    if input.color.a < 0.99 {
        let t = camera.eye_time.w;
        let wave_a = dot(input.world.xz, vec2<f32>(0.53, 0.31)) + t * 0.7;
        let wave_b = dot(input.world.xz, vec2<f32>(-0.27, 0.72)) - t * 0.52;
        let water_normal = normalize(normal + vec3<f32>(cos(wave_a) * 0.055, 0., cos(wave_b) * 0.055));
        let reflected = reflect(-eye_direction, water_normal);
        let fresnel = 0.10 + 0.72 * pow(1.0 - max(dot(eye_direction, water_normal), 0.0), 4.0);
        let depth_color = albedo * (0.88 + sin(wave_a) * sin(wave_b) * 0.04);
        let half_direction = normalize(camera.sun_fog.xyz + eye_direction);
        let sparkle = pow(max(dot(water_normal, half_direction), 0.0), 220.0);
        color = mix(depth_color, atmosphere(reflected), fresnel) + vec3<f32>(1.0, 0.89, 0.66) * sparkle * 0.65 * camera.sky_horizon.w;
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
    return vec4<f32>(color, 1.0);
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
