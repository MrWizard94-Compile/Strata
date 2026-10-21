use crate::{save::PlayerSave, world::World};
use glam::Vec3;

pub struct Player {
    pub eye: Vec3,
    pub yaw: f32,
    pub pitch: f32,
    pub flying: bool,
    pub grounded: bool,
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
        let mut player = Self {
            eye: Vec3::new(64.5, world.surface_height(64, -64) as f32 + 3.0, -63.5),
            yaw: -0.35,
            pitch: -0.12,
            flying: false,
            grounded: false,
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
            velocity: Vec3::ZERO,
        }
    }
    pub fn snapshot(&self) -> PlayerSave {
        PlayerSave {
            eye: self.eye.to_array(),
            yaw: self.yaw,
            pitch: self.pitch,
            flying: self.flying,
        }
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
        self.eye.y =
            world.surface_height(self.eye.x.floor() as i32, self.eye.z.floor() as i32) as f32 + 3.0;
        // Test the entire body, including adjacent cells, trees and player edits.
        // Search is bounded even when edited columns fill the permitted height.
        while self.eye.y < 512.0 && Self::collides(self.eye, world) {
            self.eye.y += 1.0;
        }
        self.eye.y = self.eye.y.min(512.0);
        if Self::collides(self.eye, world) {
            self.flying = true;
        }
        self.velocity = Vec3::ZERO;
        self.grounded = false;
    }
    pub fn occupies(&self, block: [i32; 3]) -> bool {
        let min = self.eye + Vec3::new(-0.3, -1.6, -0.3);
        let max = self.eye + Vec3::new(0.3, 0.15, 0.3);
        (0..3).all(|i| max[i] > block[i] as f32 && min[i] < block[i] as f32 + 1.0)
    }
    fn collides(eye: Vec3, world: &World) -> bool {
        let min = (eye + Vec3::new(-0.3, -1.6, -0.3) + Vec3::splat(0.001))
            .floor()
            .as_ivec3();
        let max = (eye + Vec3::new(0.3, 0.15, 0.3) - Vec3::splat(0.001))
            .floor()
            .as_ivec3();
        for z in min.z..=max.z {
            for y in min.y..=max.y {
                for x in min.x..=max.x {
                    if world.block_at(x, y, z).is_solid() {
                        return true;
                    }
                }
            }
        }
        false
    }
    pub fn tick(&mut self, world: &World, motion: Motion, dt: f32) {
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
        self.eye.x = self.eye.x.clamp(-999_900.0, 999_900.0);
        self.eye.z = self.eye.z.clamp(-999_900.0, 999_900.0);
        self.eye.y = self.eye.y.clamp(-64.0, 512.0);
        if self.eye.y <= -60.0 {
            self.recover_above_surface(world);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::world::Block;
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
        player.eye = Vec3::new(0.5, 220.0, 0.5);
        for _ in 0..240 {
            player.tick(&world, Motion::default(), 1.0 / 60.0);
        }
        assert!(player.grounded);
        assert!((202.59..202.76).contains(&player.eye.y), "{}", player.eye.y);
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
        player.eye = Vec3::new(0.5, 202.7, 0.5);
        assert!(player.occupies([0, 201, 0]));
        assert!(!player.occupies([1, 201, 0]));
        assert!(!player.occupies([0, 200, 0]));
    }
    #[test]
    fn look_is_bounded_and_direction_is_normalized() {
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
        player.eye = Vec3::new(0.5, 201.5, 0.5);
        assert!(Player::collides(player.eye, &world));
        assert!(!player.toggle_flight(&world));
        assert!(player.flying);
        player.tick(
            &world,
            Motion {
                vertical: 1.0,
                ..Default::default()
            },
            0.1,
        );
        assert!(player.toggle_flight(&world));
        assert!(!player.flying);
        assert!(!Player::collides(player.eye, &world));
    }
    #[test]
    fn spawn_and_fall_recovery_clear_trees_and_player_construction() {
        let mut tree_world = World::new(396);
        let ground = tree_world.surface_height(64, -64);
        // Generator 2 moved the forests, so seed 396 no longer promises a trunk
        // in this spawn column. Plant one when the column is open; recovery
        // still has to lift the body clear of that solid.
        if !tree_world.block_at(64, ground + 1, -64).is_solid() {
            tree_world.set_block([64, ground + 1, -64], Block::Wood);
            tree_world.set_block([64, ground + 2, -64], Block::Leaves);
        }
        let player = Player::new(&tree_world);
        assert!(!Player::collides(player.eye, &tree_world));
        assert!(player.eye.y > tree_world.surface_height(64, -64) as f32 + 3.0);
        let mut world = World::new(728_491);
        let ground = world.surface_height(64, -64);
        for y in ground + 1..=ground + 12 {
            world.set_block([64, y, -64], Block::Stone);
        }
        let mut player = Player::new(&world);
        assert!(!Player::collides(player.eye, &world));
        player.eye.y = -61.0;
        player.tick(&world, Motion::default(), 1.0 / 60.0);
        assert!(!Player::collides(player.eye, &world));
        assert!(player.eye.y > (ground + 12) as f32 + 1.6);
    }
}
