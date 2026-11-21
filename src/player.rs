use crate::{
    save::PlayerSave,
    survival::PLAYER_MAX_HEALTH,
    world::{VOXEL_METERS, World, meters_to_voxel, voxel_to_meters},
};
use glam::Vec3;

pub const PLAYER_HEIGHT: f32 = 1.8;
pub const PLAYER_EYE_HEIGHT: f32 = 1.62;
pub const PLAYER_HALF_WIDTH: f32 = 0.3;

pub struct Player {
    pub eye: Vec3,
    pub yaw: f32,
    pub pitch: f32,
    pub flying: bool,
    pub grounded: bool,
    pub health: f32,
    velocity: Vec3,
}

#[derive(Default, Clone, Copy)]
pub struct Motion {
    pub forward: f32,
    pub right: f32,
    pub vertical: f32,
    pub sprint: bool,
    pub jump: bool,
}

impl Player {
    pub fn new(world: &World) -> Self {
        let feet = voxel_to_meters([64, world.surface_height(64, -64) + 1, -64]);
        let mut player = Self {
            eye: Vec3::new(
                feet[0] + VOXEL_METERS * 0.5,
                feet[1] + PLAYER_EYE_HEIGHT,
                feet[2] + VOXEL_METERS * 0.5,
            ),
            yaw: -0.35,
            pitch: -0.12,
            flying: false,
            grounded: false,
            health: PLAYER_MAX_HEALTH,
            velocity: Vec3::ZERO,
        };
        player.recover_above_surface(world);
        player
    }
    pub fn restore(save: PlayerSave) -> Self {
        Self {
            eye: Vec3::from_array(save.eye),
            yaw: save.yaw,
            pitch: save.pitch,
            flying: save.flying,
            grounded: false,
            health: save.health,
            velocity: Vec3::ZERO,
        }
    }
    pub fn snapshot(&self) -> PlayerSave {
        PlayerSave {
            eye: self.eye.to_array(),
            yaw: self.yaw,
            pitch: self.pitch,
            flying: self.flying,
            health: self.health,
        }
    }
    pub fn body_min(&self) -> Vec3 {
        Self::aabb_min(self.eye)
    }
    pub fn body_max(&self) -> Vec3 {
        Self::aabb_max(self.eye)
    }
    pub fn standing_height() -> f32 {
        PLAYER_HEIGHT
    }
    fn aabb_min(eye: Vec3) -> Vec3 {
        eye + Vec3::new(-PLAYER_HALF_WIDTH, -PLAYER_EYE_HEIGHT, -PLAYER_HALF_WIDTH)
    }
    fn aabb_max(eye: Vec3) -> Vec3 {
        eye + Vec3::new(
            PLAYER_HALF_WIDTH,
            PLAYER_HEIGHT - PLAYER_EYE_HEIGHT,
            PLAYER_HALF_WIDTH,
        )
    }
    pub fn direction(&self) -> Vec3 {
        Vec3::new(
            self.yaw.sin() * self.pitch.cos(),
            self.pitch.sin(),
            -self.yaw.cos() * self.pitch.cos(),
        )
    }
    pub fn look(&mut self, dx: f32, dy: f32) {
        self.yaw = (self.yaw + dx).rem_euclid(std::f32::consts::TAU);
        self.pitch = (self.pitch + dy).clamp(-1.54, 1.54);
    }
    pub fn toggle_flight(&mut self, world: &World) -> bool {
        if self.flying && Self::collides(self.eye, world) {
            return false;
        }
        self.flying = !self.flying;
        self.grounded = false;
        self.velocity = Vec3::ZERO;
        true
    }
    fn recover_above_surface(&mut self, world: &World) {
        let column = meters_to_voxel(self.eye.to_array());
        let ground = world.surface_height(column[0], column[2]) as f32 * VOXEL_METERS;
        self.eye.y = ground + VOXEL_METERS + PLAYER_EYE_HEIGHT;
        while self.eye.y < 256.0 && Self::collides(self.eye, world) {
            self.eye.y += VOXEL_METERS;
        }
        self.eye.y = self.eye.y.min(256.0);
        if Self::collides(self.eye, world) {
            self.flying = true;
        }
        self.velocity = Vec3::ZERO;
        self.grounded = false;
    }
    pub fn occupies(&self, block: [i32; 3]) -> bool {
        let min = Self::aabb_min(self.eye);
        let max = Self::aabb_max(self.eye);
        let block_min = Vec3::from_array(voxel_to_meters(block));
        let block_max = block_min + Vec3::splat(VOXEL_METERS);
        (0..3).all(|i| max[i] > block_min[i] && min[i] < block_max[i])
    }
    fn collides(eye: Vec3, world: &World) -> bool {
        let min = Self::aabb_min(eye) + Vec3::splat(0.001);
        let max = Self::aabb_max(eye) - Vec3::splat(0.001);
        let vmin = meters_to_voxel(min.to_array());
        let vmax = meters_to_voxel(max.to_array());
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
    pub fn tick(&mut self, world: &World, mut motion: Motion, dt: f32) {
        if self.health <= 0.0 {
            motion = Motion::default();
            self.flying = false;
        }
        let forward = Vec3::new(self.yaw.sin(), 0.0, -self.yaw.cos());
        let right = Vec3::new(self.yaw.cos(), 0.0, self.yaw.sin());
        let direction = (forward * motion.forward + right * motion.right).normalize_or_zero();
        if self.flying {
            let speed = if motion.sprint { 36.0 } else { 14.0 };
            self.eye += (direction + Vec3::Y * motion.vertical) * speed * dt;
            self.grounded = false;
        } else {
            let speed = if motion.sprint { 8.5 } else { 5.0 };
            self.velocity.x = direction.x * speed;
            self.velocity.z = direction.z * speed;
            if motion.jump && self.grounded {
                self.velocity.y = 8.2;
            }
            self.velocity.y = (self.velocity.y - 25.0 * dt).max(-48.0);
            let displacement = self.velocity * dt;
            let steps = (displacement.abs().max_element() / 0.15).ceil().max(1.0) as usize;
            let step = displacement / steps as f32;
            self.grounded = false;
            for _ in 0..steps {
                for axis in [0, 2, 1] {
                    let mut candidate = self.eye;
                    candidate[axis] += step[axis];
                    if Self::collides(candidate, world) {
                        if axis == 1 && step.y < 0.0 {
                            self.grounded = true;
                        }
                        self.velocity[axis] = 0.0;
                    } else {
                        self.eye = candidate;
                    }
                }
            }
        }
        self.eye.x = self.eye.x.clamp(-499_950.0, 499_950.0);
        self.eye.z = self.eye.z.clamp(-499_950.0, 499_950.0);
        self.eye.y = self.eye.y.clamp(-32.0, 256.0);
        if self.eye.y <= -30.0 {
            self.recover_above_surface(world);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::world::{Biome, Block};
    fn platform() -> World {
        let mut world = World::new(3);
        for x in -3..=3 {
            for z in -3..=3 {
                world.set_block([x, 200, z], Block::Stone);
            }
        }
        world
    }
    #[test]
    fn falling_player_lands_and_can_jump_without_tunneling() {
        let world = platform();
        let mut player = Player::new(&world);
        let top = 200.0 * VOXEL_METERS + VOXEL_METERS;
        player.eye = Vec3::new(0.25, top + 10.0, 0.25);
        for _ in 0..240 {
            player.tick(&world, Motion::default(), 1.0 / 60.0);
        }
        assert!(player.grounded);
        let expected = top + PLAYER_EYE_HEIGHT;
        assert!(
            (expected - 0.08..expected + 0.08).contains(&player.eye.y),
            "{}",
            player.eye.y
        );
        let ground_y = player.eye.y;
        player.tick(
            &world,
            Motion {
                jump: true,
                ..Default::default()
            },
            1.0 / 60.0,
        );
        assert!(player.eye.y > ground_y);
    }
    #[test]
    fn placement_excludes_player_body_but_not_remote_blocks() {
        let mut player = Player::new(&platform());
        let top = 200.0 * VOXEL_METERS + VOXEL_METERS;
        player.eye = Vec3::new(0.25, top + PLAYER_EYE_HEIGHT, 0.25);
        assert!(player.occupies([0, 201, 0]));
        assert!(!player.occupies([2, 201, 0]));
        assert!(!player.occupies([0, 200, 0]));
        assert_eq!(Player::standing_height(), PLAYER_HEIGHT);
    }
    #[test]
    fn look_is_bounded_and_direction_is_normalized() {
        const { assert!(PLAYER_HEIGHT > 1.5 && PLAYER_HEIGHT < 2.2) };
        let mut player = Player::new(&platform());
        player.look(1000.0, 1000.0);
        assert!((player.direction().length() - 1.0).abs() < 0.0001);
        assert!(player.pitch <= 1.54);
        assert!(player.yaw < std::f32::consts::TAU);
    }
    #[test]
    fn walking_cannot_start_while_flight_body_intersects_terrain() {
        let world = platform();
        let mut player = Player::new(&world);
        assert!(player.toggle_flight(&world));
        player.eye = Vec3::new(0.25, 200.0 * VOXEL_METERS + 0.2, 0.25);
        assert!(Player::collides(player.eye, &world));
        assert!(!player.toggle_flight(&world));
        assert!(player.flying);
        player.tick(
            &world,
            Motion {
                vertical: 1.0,
                ..Default::default()
            },
            0.25,
        );
        assert!(player.toggle_flight(&world));
        assert!(!player.flying);
        assert!(!Player::collides(player.eye, &world));
    }
    #[test]
    fn spawn_and_fall_recovery_clear_trees_and_player_construction() {
        let mut tree_world = World::new(396);
        let ground = tree_world.surface_height(64, -64);
        // Spawn is fixed at (64, -64). Trunks only sit on offsets 2..=6 of a
        // 9-block grid, and 64 rem 9 is 1, so this column is never a natural
        // trunk. Plant one and require recovery to clear it. The natural case
        // is `a_natural_trunk_grows_without_being_planted`.
        if !tree_world.block_at(64, ground + 1, -64).is_solid() {
            tree_world.set_block([64, ground + 1, -64], Block::Wood);
            tree_world.set_block([64, ground + 2, -64], Block::Leaves);
        }
        let player = Player::new(&tree_world);
        assert!(!Player::collides(player.eye, &tree_world));
        assert!(
            player.eye.y
                > tree_world.surface_height(64, -64) as f32 * VOXEL_METERS + PLAYER_EYE_HEIGHT
        );
        let mut world = World::new(728_491);
        let ground = world.surface_height(64, -64);
        for y in ground + 1..=ground + 12 {
            world.set_block([64, y, -64], Block::Stone);
        }
        let mut player = Player::new(&world);
        assert!(!Player::collides(player.eye, &world));
        player.eye.y = -31.0;
        player.tick(&world, Motion::default(), 1.0 / 60.0);
        assert!(!Player::collides(player.eye, &world));
        assert!(player.eye.y > (ground + 12) as f32 * VOXEL_METERS + PLAYER_EYE_HEIGHT - 0.05);
    }
    #[test]
    fn a_natural_trunk_grows_without_being_planted() {
        let world = World::new(20261021);
        assert!(
            world.edits().is_empty(),
            "the trunk must come from the generator"
        );
        assert_eq!(world.biome_at(-321, -354), Biome::Forest);
        assert_eq!(world.surface_height(-321, -354), 35);
        assert_eq!(world.block_at(-321, 35, -354), Block::Grass);
        for y in 36..=40 {
            assert_eq!(world.block_at(-321, y, -354), Block::Wood, "y={y}");
        }
        assert_eq!(world.block_at(-321, 41, -354), Block::Leaves);
    }
}
