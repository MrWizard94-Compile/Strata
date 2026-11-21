//! Textured mobs with AABBs in metres, gravity, and combat.

use glam::Vec3;

use crate::world::{World, meters_to_voxel, voxel_to_meters};

pub const HOLLOW_HEIGHT: f32 = 1.75;
pub const HOLLOW_WIDTH: f32 = 0.55;
pub const GRAZER_HEIGHT: f32 = 0.95;
pub const GRAZER_WIDTH: f32 = 1.15;
pub const HOLLOW_MAX_HEALTH: f32 = 20.0;
pub const GRAZER_MAX_HEALTH: f32 = 8.0;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Species {
    Hollow,
    Grazer,
}

impl Species {
    pub fn height(self) -> f32 {
        match self {
            Self::Hollow => HOLLOW_HEIGHT,
            Self::Grazer => GRAZER_HEIGHT,
        }
    }

    pub fn width(self) -> f32 {
        match self {
            Self::Hollow => HOLLOW_WIDTH,
            Self::Grazer => GRAZER_WIDTH,
        }
    }

    pub fn max_health(self) -> f32 {
        match self {
            Self::Hollow => HOLLOW_MAX_HEALTH,
            Self::Grazer => GRAZER_MAX_HEALTH,
        }
    }

    pub fn hostile(self) -> bool {
        matches!(self, Self::Hollow)
    }

    pub fn texture_layer(self) -> u32 {
        match self {
            Self::Hollow => crate::materials::HOLLOW_LAYER,
            Self::Grazer => crate::materials::GRAZER_LAYER,
        }
    }
}

#[derive(Clone, Debug)]
pub struct Entity {
    pub id: u32,
    pub species: Species,
    /// Feet-centre in metres.
    pub position: Vec3,
    pub velocity: Vec3,
    pub yaw: f32, // reserved for facing; AABB combat uses position only
    pub health: f32,
    pub grounded: bool,
}

impl Entity {
    pub fn spawn(id: u32, species: Species, feet: Vec3) -> Self {
        Self {
            id,
            species,
            position: feet,
            velocity: Vec3::ZERO,
            yaw: 0.0,
            health: species.max_health(),
            grounded: false,
        }
    }

    pub fn aabb(&self) -> (Vec3, Vec3) {
        let half = self.species.width() * 0.5;
        (
            self.position + Vec3::new(-half, 0.0, -half),
            self.position + Vec3::new(half, self.species.height(), half),
        )
    }

    pub fn alive(&self) -> bool {
        self.health > 0.0
    }
}

pub fn solid_in_aabb(world: &World, min: Vec3, max: Vec3) -> bool {
    let pad = Vec3::splat(0.001);
    let vmin = meters_to_voxel((min + pad).to_array());
    let vmax = meters_to_voxel((max - pad).to_array());
    for z in vmin[2]..=vmax[2] {
        for y in vmin[1]..=vmax[1] {
            for x in vmin[0]..=vmax[0] {
                if world.block_at(x, y, z).is_solid() {
                    return true;
                }
            }
        }
    }
    false
}

pub fn tick_entity(entity: &mut Entity, world: &World, dt: f32) {
    entity.velocity.y = (entity.velocity.y - 25.0 * dt).max(-48.0);
    let displacement = entity.velocity * dt;
    let steps = (displacement.abs().max_element() / 0.08).ceil().max(1.0) as usize;
    let step = displacement / steps as f32;
    entity.grounded = false;
    for _ in 0..steps {
        for axis in [0, 2, 1] {
            let mut candidate = entity.position;
            candidate[axis] += step[axis];
            let (min, max) = {
                let half = entity.species.width() * 0.5;
                (
                    candidate + Vec3::new(-half, 0.0, -half),
                    candidate + Vec3::new(half, entity.species.height(), half),
                )
            };
            if solid_in_aabb(world, min, max) {
                if axis == 1 && step.y < 0.0 {
                    entity.grounded = true;
                }
                entity.velocity[axis] = 0.0;
            } else {
                entity.position = candidate;
            }
        }
    }
}

pub fn aabb_overlap(a: (Vec3, Vec3), b: (Vec3, Vec3)) -> bool {
    (0..3).all(|axis| a.1[axis] > b.0[axis] && a.0[axis] < b.1[axis])
}

pub fn player_attack(
    origin: Vec3,
    direction: Vec3,
    entities: &mut [Entity],
    damage: f32,
    reach_meters: f32,
) -> bool {
    let mut best: Option<(usize, f32)> = None;
    for (index, entity) in entities.iter().enumerate() {
        if !entity.alive() || !entity.species.hostile() {
            continue;
        }
        let (min, max) = entity.aabb();
        let center = (min + max) * 0.5;
        let to = center - origin;
        let distance = to.length();
        if distance > reach_meters || distance < 1e-4 {
            continue;
        }
        if to.normalize().dot(direction) < 0.65 {
            continue;
        }
        if best.is_none_or(|(_, best_distance)| distance < best_distance) {
            best = Some((index, distance));
        }
    }
    if let Some((index, _)) = best {
        entities[index].health = (entities[index].health - damage).max(0.0);
        true
    } else {
        false
    }
}

pub fn hostile_melee(entity: &Entity, player_min: Vec3, player_max: Vec3, damage: f32) -> f32 {
    if entity.alive()
        && entity.species.hostile()
        && aabb_overlap(entity.aabb(), (player_min, player_max))
    {
        damage
    } else {
        0.0
    }
}

pub fn despawn_dead(entities: &mut Vec<Entity>) {
    entities.retain(|entity| entity.alive());
}

pub fn spawn_near_column(world: &World, column: [i32; 2]) -> Vec<Entity> {
    let ground = world.surface_height(column[0], column[1]);
    let feet = Vec3::from_array(voxel_to_meters([column[0], ground + 1, column[1]]));
    vec![
        Entity::spawn(1, Species::Hollow, feet + Vec3::new(3.0, 0.0, 0.0)),
        Entity::spawn(2, Species::Grazer, feet + Vec3::new(-3.0, 0.0, 2.0)),
    ]
}

pub fn mesh_entities(entities: &[Entity]) -> crate::world::Mesh {
    let mut mesh = crate::world::Mesh::default();
    for entity in entities {
        emit_box(&mut mesh, entity);
    }
    mesh
}

/// Per-face UV for a unit square. V=0 is the top of the authored 32×32 tile.
pub fn face_corner_uv(corner: usize) -> [f32; 2] {
    [[0.0, 1.0], [1.0, 1.0], [1.0, 0.0], [0.0, 0.0]][corner]
}

fn yaw_rotate(point: [f32; 3], yaw: f32, pivot: Vec3) -> [f32; 3] {
    let delta = Vec3::from_array(point) - pivot;
    let (sin, cos) = yaw.sin_cos();
    (pivot
        + Vec3::new(
            delta.x * cos - delta.z * sin,
            delta.y,
            delta.x * sin + delta.z * cos,
        ))
    .to_array()
}

fn yaw_rotate_dir(dir: [f32; 3], yaw: f32) -> [f32; 3] {
    let (sin, cos) = yaw.sin_cos();
    [
        dir[0] * cos - dir[2] * sin,
        dir[1],
        dir[0] * sin + dir[2] * cos,
    ]
}

fn emit_box(mesh: &mut crate::world::Mesh, entity: &Entity) {
    let (min, max) = entity.aabb();
    let pivot = Vec3::new(entity.position.x, entity.position.y, entity.position.z);
    let layer = entity.species.texture_layer();
    let sx = max.x - min.x;
    let sy = max.y - min.y;
    let sz = max.z - min.z;
    let faces = [
        (
            [1.0, 0.0, 0.0],
            [max.x, min.y, min.z],
            [0.0, 0.0, sz],
            [0.0, sy, 0.0],
        ),
        (
            [-1.0, 0.0, 0.0],
            [min.x, min.y, max.z],
            [0.0, 0.0, -sz],
            [0.0, sy, 0.0],
        ),
        (
            [0.0, 1.0, 0.0],
            [min.x, max.y, min.z],
            [sx, 0.0, 0.0],
            [0.0, 0.0, sz],
        ),
        (
            [0.0, -1.0, 0.0],
            [min.x, min.y, max.z],
            [sx, 0.0, 0.0],
            [0.0, 0.0, -sz],
        ),
        (
            [0.0, 0.0, 1.0],
            [max.x, min.y, max.z],
            [-sx, 0.0, 0.0],
            [0.0, sy, 0.0],
        ),
        (
            [0.0, 0.0, -1.0],
            [min.x, min.y, min.z],
            [sx, 0.0, 0.0],
            [0.0, sy, 0.0],
        ),
    ];
    for (normal, origin, u, v) in faces {
        let base = mesh.vertices.len() as u32;
        let corners = [
            origin,
            [origin[0] + u[0], origin[1] + u[1], origin[2] + u[2]],
            [
                origin[0] + u[0] + v[0],
                origin[1] + u[1] + v[1],
                origin[2] + u[2] + v[2],
            ],
            [origin[0] + v[0], origin[1] + v[1], origin[2] + v[2]],
        ];
        let rotated_normal = yaw_rotate_dir(normal, entity.yaw);
        for (corner, position) in corners.into_iter().enumerate() {
            mesh.vertices.push(crate::world::Vertex {
                position: yaw_rotate(position, entity.yaw, pivot),
                normal: rotated_normal,
                color: {
                    let tint = 0.92 + (entity.id % 4) as f32 * 0.02;
                    [tint, tint, tint, 1.0]
                },
                uv: face_corner_uv(corner),
                texture_layer: layer,
            });
        }
        mesh.indices
            .extend([base, base + 2, base + 1, base, base + 3, base + 2]);
        let start = mesh.indices.len() - 6;
        debug_assert!(
            crate::world::triangle_outward_dot(mesh, &mesh.indices[start..start + 3]) > 0.0
        );
        debug_assert!(
            crate::world::triangle_outward_dot(mesh, &mesh.indices[start + 3..start + 6]) > 0.0
        );
    }
}

#[cfg(test)]
pub fn albedo_not_flat(layer: u32) -> bool {
    let pixels = crate::materials::rgba_layers();
    let stride = (crate::materials::TILE_SIZE * crate::materials::TILE_SIZE * 4) as usize;
    let start = layer as usize * stride;
    let tile = &pixels[start..start + stride];
    let first = [tile[0], tile[1], tile[2]];
    tile.chunks_exact(4).any(|pixel| pixel[..3] != first)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::player::{PLAYER_EYE_HEIGHT, PLAYER_HALF_WIDTH, PLAYER_HEIGHT, Player};
    use crate::survival::{HOSTILE_CONTACT_DAMAGE, PLAYER_ATTACK_DAMAGE, apply_damage, is_dead};
    use crate::world::{Block, VOXEL_METERS};

    fn platform() -> World {
        let mut world = World::new(3);
        for x in -6..=6 {
            for z in -6..=6 {
                world.set_block([x, 200, z], Block::Stone);
            }
        }
        world
    }

    #[test]
    fn mob_sizes_are_relative_to_the_player_and_textures_are_authored() {
        const { assert!(PLAYER_HEIGHT > 1.5 && PLAYER_HEIGHT < 2.2) };
        assert!((HOLLOW_HEIGHT - PLAYER_HEIGHT).abs() / PLAYER_HEIGHT < 0.25);
        const { assert!(GRAZER_HEIGHT < PLAYER_HEIGHT) };
        const { assert!(GRAZER_WIDTH > PLAYER_HALF_WIDTH * 2.0) };
        assert!((GRAZER_HEIGHT - PLAYER_HEIGHT).abs() > 0.2);
        assert!((GRAZER_WIDTH - HOLLOW_WIDTH).abs() > 0.2);
        assert_ne!(GRAZER_HEIGHT, VOXEL_METERS);
        assert!(albedo_not_flat(Species::Hollow.texture_layer()));
        assert!(albedo_not_flat(Species::Grazer.texture_layer()));
        let pixels = crate::materials::rgba_layers();
        let stride = (crate::materials::TILE_SIZE * crate::materials::TILE_SIZE * 4) as usize;
        let start = Species::Hollow.texture_layer() as usize * stride;
        let unique: std::collections::HashSet<_> =
            pixels[start..start + stride].chunks_exact(4).collect();
        assert!(unique.len() > 12);
    }

    #[test]
    fn gravity_lands_a_mob_and_combat_kills_both_ways() {
        let world = platform();
        let top = 200.0 * VOXEL_METERS + VOXEL_METERS;
        let mut hollow = Entity::spawn(1, Species::Hollow, Vec3::new(0.25, top + 4.0, 0.25));
        assert_eq!(hollow.id, 1);
        hollow.yaw = 0.5;
        for _ in 0..240 {
            tick_entity(&mut hollow, &world, 1.0 / 60.0);
        }
        assert!(hollow.grounded);
        assert!(hollow.position.y < top + 0.05);

        let mut player = Player::new(&world);
        player.eye = Vec3::new(0.25, top + PLAYER_EYE_HEIGHT, 0.25);
        player.health = crate::survival::PLAYER_MAX_HEALTH;
        let damage = hostile_melee(
            &hollow,
            player.body_min(),
            player.body_max(),
            HOSTILE_CONTACT_DAMAGE,
        );
        assert!(damage > 0.0);
        player.health = apply_damage(player.health, damage);
        assert!(player.health < crate::survival::PLAYER_MAX_HEALTH);

        let mut pack = vec![hollow];
        player.look(0.0, 0.0);
        let aim =
            (pack[0].position + Vec3::Y * (HOLLOW_HEIGHT * 0.5) - player.eye).normalize_or_zero();
        assert!(player_attack(
            player.eye,
            aim,
            &mut pack,
            PLAYER_ATTACK_DAMAGE,
            6.0,
        ));
        while pack.iter().any(|entity| entity.alive()) {
            player_attack(player.eye, aim, &mut pack, PLAYER_ATTACK_DAMAGE, 6.0);
        }
        despawn_dead(&mut pack);
        assert!(pack.is_empty());
        player.health = 0.0;
        assert!(is_dead(player.health));
    }

    #[test]
    fn entity_face_samples_the_full_authored_tile() {
        let hollow = Entity::spawn(1, Species::Hollow, Vec3::new(3.0, 1.0, 4.0));
        let mesh = mesh_entities(std::slice::from_ref(&hollow));
        assert_eq!(mesh.vertices.len() % 4, 0);
        assert!(!mesh.vertices.is_empty());
        for quad in mesh.vertices.chunks_exact(4) {
            for (corner, vertex) in quad.iter().enumerate() {
                assert_eq!(
                    vertex.uv,
                    face_corner_uv(corner),
                    "face corner {corner} must cover the 32x32 tile"
                );
            }
            let min_u = quad
                .iter()
                .map(|vertex| vertex.uv[0])
                .fold(f32::INFINITY, f32::min);
            let max_u = quad
                .iter()
                .map(|vertex| vertex.uv[0])
                .fold(f32::NEG_INFINITY, f32::max);
            let min_v = quad
                .iter()
                .map(|vertex| vertex.uv[1])
                .fold(f32::INFINITY, f32::min);
            let max_v = quad
                .iter()
                .map(|vertex| vertex.uv[1])
                .fold(f32::NEG_INFINITY, f32::max);
            assert!((min_u - 0.0).abs() < 1e-5 && (max_u - 1.0).abs() < 1e-5);
            assert!((min_v - 0.0).abs() < 1e-5 && (max_v - 1.0).abs() < 1e-5);
        }
        let mut shifted = hollow.clone();
        shifted.position.x += 12.0;
        shifted.position.z -= 5.0;
        let moved = mesh_entities(&[shifted]);
        for (here, there) in mesh.vertices.iter().zip(&moved.vertices) {
            assert_eq!(
                here.uv, there.uv,
                "atlas UVs must not slide with world position"
            );
            assert_ne!(here.position, there.position);
        }
        let mut turned = hollow.clone();
        turned.yaw = 1.2;
        let yawed = mesh_entities(&[turned]);
        assert!(
            mesh.vertices
                .iter()
                .zip(&yawed.vertices)
                .any(|(here, there)| (here.position[0] - there.position[0]).abs() > 0.01)
        );
        assert!(
            yawed
                .vertices
                .iter()
                .all(|vertex| { (0..4).any(|corner| vertex.uv == face_corner_uv(corner)) })
        );
    }

    #[test]
    fn entity_triangles_wind_outward_for_the_shared_terrain_cull() {
        let mut hollow = Entity::spawn(1, Species::Hollow, Vec3::new(3.0, 1.0, 4.0));
        hollow.yaw = 0.4;
        let grazer = Entity::spawn(2, Species::Grazer, Vec3::new(-2.0, 0.5, 1.0));
        let mesh = mesh_entities(&[hollow, grazer]);
        assert!(!mesh.indices.is_empty());
        for tri in mesh.indices.chunks_exact(3) {
            assert!(
                crate::world::triangle_outward_dot(&mesh, tri) > 0.0,
                "entity triangles must match terrain back-face culling"
            );
        }
    }
}
