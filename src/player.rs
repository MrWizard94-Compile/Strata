//! First-person body movement.
//!
//! Walking accelerates toward a wish speed and sheds it through friction, so
//! starts and stops take a few fixed ticks instead of snapping. Callers pass
//! the same `dt` for every tick (the app uses 60 Hz); a repeated input
//! sequence is therefore deterministic. Render frame time only chooses how
//! many of those ticks run. The body is an axis-aligned box swept against
//! the voxel grid; full blocks are not auto-stepped.

use crate::{save::PlayerSave, world::World};
use glam::Vec3;

/// Horizontal wish speed while walking, metres per second.
const WALK_SPEED: f32 = 5.0;
/// Horizontal wish speed while sprinting forward. Clearly faster than walking.
const SPRINT_SPEED: f32 = 8.0;
/// Horizontal wish speed while sneaking. Sprint does not stack with this.
const SNEAK_SPEED: f32 = 1.6;
/// Ground acceleration coefficient. Must stay above [`GROUND_FRICTION`] so
/// holding a direction converges on the wish speed instead of a lower plateau.
const GROUND_ACCEL: f32 = 10.0;
/// Ground friction coefficient. Speed is multiplied by about `1 - rate * dt`
/// each tick when there is no accel replacing it.
const GROUND_FRICTION: f32 = 8.0;
/// Air acceleration. Small on purpose: a jump keeps most of its momentum.
const AIR_ACCEL: f32 = 2.0;
/// Air drag coefficient. Light, so long jumps do not ice-skate forever.
const AIR_DRAG: f32 = 0.15;
/// Upward speed applied once each time a jump is accepted.
const JUMP_VELOCITY: f32 = 8.2;
/// Constant downward acceleration, metres per second squared.
const GRAVITY: f32 = 25.0;
/// Most negative vertical speed the walker will keep.
const TERMINAL_VELOCITY: f32 = 48.0;
/// Ticks after walking off a ledge during which a jump is still accepted.
const COYOTE_TICKS: u8 = 6;
/// Ticks, including the press, during which an early jump is remembered.
const JUMP_BUFFER_TICKS: u8 = 6;
const FLIGHT_SPEED: f32 = 14.0;
const FLIGHT_SPRINT_SPEED: f32 = 36.0;
const HALF_WIDTH: f32 = 0.3;
const FEET_BELOW_EYE: f32 = 1.6;
const HEAD_ABOVE_EYE: f32 = 0.15;
/// Shrinks the discrete overlap sample so a body flush with a face is not inside it.
const SKIN: f32 = 0.001;
/// Gap left between a leading face and the voxel plane it hits.
const SWEEP_SKIN: f32 = 1.0e-4;
/// Solid contacts resolved in one tick. Leftover motion is dropped.
const MAX_SWEEP_ITERS: u32 = 4;
/// Empty planes one axis query may cross. A fixed tick is well under this;
/// the bound also covers the oversized steps used by tests.
const MAX_SWEEP_PLANES: u32 = 96;

pub struct Player {
    pub eye: Vec3,
    pub yaw: f32,
    pub pitch: f32,
    pub flying: bool,
    pub grounded: bool,
    velocity: Vec3,
    jump: JumpMemory,
}

#[derive(Default, Clone, Copy)]
struct JumpMemory {
    /// Remaining ticks of coyote time. Zero when a jump is not forgiven.
    coyote: u8,
    /// Remaining ticks of a jump press that has not yet been consumed.
    buffer: u8,
    /// Jump was held on the previous tick. Used only to notice a fresh press.
    held: bool,
}

#[derive(Default, Clone, Copy)]
pub struct Motion {
    pub forward: f32,
    pub right: f32,
    pub vertical: f32,
    pub sprint: bool,
    pub jump: bool,
    pub sneak: bool,
}

#[derive(Clone, Copy)]
struct SweepHit {
    /// Fraction of the current remaining displacement, in `(0, 1]`, or `0`
    /// when the body is already inside a solid and moving deeper.
    t: f32,
    axis: usize,
    /// Downward contact with a floor. Ceilings and walls leave this false.
    floor: bool,
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
            jump: JumpMemory::default(),
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
            jump: JumpMemory::default(),
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
        self.jump.coyote = 0;
        self.jump.buffer = 0;
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
        self.jump.coyote = 0;
        self.jump.buffer = 0;
    }

    pub fn occupies(&self, block: [i32; 3]) -> bool {
        let (min, max) = body_extents(self.eye);
        (0..3).all(|i| max[i] > block[i] as f32 && min[i] < block[i] as f32 + 1.0)
    }

    fn collides(eye: Vec3, world: &World) -> bool {
        let (min, max) = body_extents(eye);
        let min = (min + Vec3::splat(SKIN)).floor().as_ivec3();
        let max = (max - Vec3::splat(SKIN)).floor().as_ivec3();
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

    /// True when any column under the feet still has a solid to stand on.
    fn has_footing(eye: Vec3, world: &World) -> bool {
        let (min, max) = body_extents(eye);
        let y = (min.y - 0.05).floor() as i32;
        let x0 = (min.x + SKIN).floor() as i32;
        let x1 = (max.x - SKIN).floor() as i32;
        let z0 = (min.z + SKIN).floor() as i32;
        let z1 = (max.z - SKIN).floor() as i32;
        for z in z0..=z1 {
            for x in x0..=x1 {
                if world.block_at(x, y, z).is_solid() {
                    return true;
                }
            }
        }
        false
    }

    pub fn tick(&mut self, world: &World, motion: Motion, dt: f32) {
        let dt = if dt.is_finite() { dt.max(0.0) } else { 0.0 };
        if !self.velocity.is_finite() {
            self.velocity = Vec3::ZERO;
        }
        if self.flying {
            self.integrate_flight(motion, dt);
        } else {
            self.integrate_walk(world, motion, dt);
        }
        self.contain(world);
    }

    fn integrate_flight(&mut self, motion: Motion, dt: f32) {
        let speed = if motion.sprint {
            FLIGHT_SPRINT_SPEED
        } else {
            FLIGHT_SPEED
        };
        let direction = planar_direction(self.yaw, motion.forward, motion.right);
        self.eye += (direction + Vec3::Y * motion.vertical) * speed * dt;
        self.grounded = false;
        self.velocity = Vec3::ZERO;
        // Keep the latch current so leaving flight does not look like a new press.
        self.jump.held = motion.jump;
        self.jump.buffer = 0;
        self.jump.coyote = 0;
    }

    fn integrate_walk(&mut self, world: &World, motion: Motion, dt: f32) {
        self.note_jump_input(motion.jump);
        let was_grounded = self.grounded;
        self.apply_horizontal(motion, dt);
        let mut jumped = false;
        if self.try_jump() {
            self.velocity.y = JUMP_VELOCITY;
            jumped = true;
        }
        self.velocity.y = (self.velocity.y - GRAVITY * dt).max(-TERMINAL_VELOCITY);
        // A jump this tick must be allowed to leave a sneaked edge.
        let edge_guard = motion.sneak && self.grounded;
        self.collide_move(world, dt, edge_guard);
        // Landing is known only after the sweep. This is the buffered press,
        // and the held-jump repeat. It arms one impulse; the next tick lifts.
        if !jumped && self.try_jump() {
            self.velocity.y = JUMP_VELOCITY;
            jumped = true;
        }
        if !jumped && self.jump.buffer > 0 {
            self.jump.buffer -= 1;
        }
        self.update_coyote(was_grounded, jumped);
    }

    fn note_jump_input(&mut self, jump: bool) {
        if jump && !self.jump.held {
            self.jump.buffer = JUMP_BUFFER_TICKS;
        }
        self.jump.held = jump;
    }

    /// One accepted jump per takeoff. Holding jump arms the next landing only;
    /// it does not add a second impulse in the air. Coyote time uses the buffer
    /// from a real press, so walking off a ledge while the key is up can still
    /// be saved, and holding the key on the ground jumps before the ledge.
    fn try_jump(&mut self) -> bool {
        let available = self.grounded || self.jump.coyote > 0;
        let request = self.jump.buffer > 0 || (self.jump.held && self.grounded);
        if !available || !request {
            return false;
        }
        self.jump.buffer = 0;
        self.jump.coyote = 0;
        self.grounded = false;
        true
    }

    fn update_coyote(&mut self, was_grounded: bool, jumped: bool) {
        if jumped {
            self.jump.coyote = 0;
            self.grounded = false;
            return;
        }
        if self.grounded {
            self.jump.coyote = COYOTE_TICKS;
        } else if was_grounded {
            // The window starts the tick contact is lost, and is counted down
            // on later airborne ticks.
            self.jump.coyote = COYOTE_TICKS;
        } else {
            self.jump.coyote = self.jump.coyote.saturating_sub(1);
        }
    }

    fn apply_horizontal(&mut self, motion: Motion, dt: f32) {
        let direction = planar_direction(self.yaw, motion.forward, motion.right);
        let wish_speed = if motion.sneak {
            SNEAK_SPEED
        } else if motion.sprint && motion.forward > 0.0 {
            SPRINT_SPEED
        } else {
            WALK_SPEED
        };
        if self.grounded {
            damp_horizontal(&mut self.velocity, GROUND_FRICTION, dt);
            if direction.length_squared() > 0.0 {
                accelerate(&mut self.velocity, direction, wish_speed, GROUND_ACCEL, dt);
            }
        } else {
            damp_horizontal(&mut self.velocity, AIR_DRAG, dt);
            if direction.length_squared() > 0.0 {
                accelerate(&mut self.velocity, direction, wish_speed, AIR_ACCEL, dt);
            }
        }
    }

    fn collide_move(&mut self, world: &World, dt: f32, edge_guard: bool) {
        if dt <= 0.0 {
            return;
        }
        let had_ground = self.grounded;
        self.grounded = false;
        let mut disp = self.velocity * dt;
        if !disp.is_finite() {
            self.velocity = Vec3::ZERO;
            return;
        }
        // Sneak holds the edge for the whole tick. An arrival from the air
        // keeps the support it just hit for the rest of this tick only, so the
        // pre-impact horizontal remainder cannot walk off that lip.
        let mut footing_guard = edge_guard;
        for _ in 0..MAX_SWEEP_ITERS {
            if disp.length_squared() <= 1.0e-12 {
                break;
            }
            let Some(hit) = earliest_hit(self.eye, disp, world, footing_guard) else {
                self.eye += disp;
                disp = Vec3::ZERO;
                break;
            };
            let span = disp[hit.axis].abs();
            let skin_t = if span > 1.0e-8 {
                SWEEP_SKIN / span
            } else {
                0.0
            };
            let t = (hit.t - skin_t).clamp(0.0, hit.t);
            self.eye += disp * t;
            disp *= 1.0 - t;
            disp[hit.axis] = 0.0;
            self.velocity[hit.axis] = 0.0;
            if hit.floor {
                self.grounded = true;
                if !had_ground {
                    footing_guard = true;
                }
            }
        }
        // Four contacts were not enough. Drop the rest instead of tunneling,
        // and clear the velocity that would try the same move again next tick.
        if disp.length_squared() > 1.0e-12 {
            for axis in 0..3 {
                if disp[axis].abs() > 1.0e-8 {
                    self.velocity[axis] = 0.0;
                }
            }
        }
        let resting = had_ground
            && self.velocity.y.abs() <= 1.0e-4
            && Self::has_footing(self.eye, world)
            && !Self::collides(self.eye, world);
        if resting {
            self.grounded = true;
        }
    }

    fn contain(&mut self, world: &World) {
        if !self.eye.is_finite() {
            self.eye = Vec3::new(
                if self.eye.x.is_finite() {
                    self.eye.x
                } else {
                    0.5
                },
                -61.0,
                if self.eye.z.is_finite() {
                    self.eye.z
                } else {
                    0.5
                },
            );
        }
        self.eye.x = self.eye.x.clamp(-999_900.0, 999_900.0);
        self.eye.z = self.eye.z.clamp(-999_900.0, 999_900.0);
        self.eye.y = self.eye.y.clamp(-64.0, 512.0);
        if self.eye.y <= -60.0 {
            self.recover_above_surface(world);
        }
    }

    #[cfg(test)]
    fn horizontal_speed(&self) -> f32 {
        self.velocity.x.hypot(self.velocity.z)
    }
}

fn earliest_hit(eye: Vec3, disp: Vec3, world: &World, footing_guard: bool) -> Option<SweepHit> {
    let solid = earliest_solid(eye, disp, world);
    let footing = if footing_guard {
        earliest_footing_loss(eye, disp, world)
    } else {
        None
    };
    match (solid, footing) {
        (Some(solid), Some(edge)) if hit_before(edge, solid) => Some(edge),
        (Some(solid), _) => Some(solid),
        (None, edge) => edge,
    }
}

fn earliest_solid(eye: Vec3, disp: Vec3, world: &World) -> Option<SweepHit> {
    let mut best: Option<SweepHit> = None;
    for axis in 0..3 {
        let delta = disp[axis];
        if delta.abs() <= 1.0e-8 {
            continue;
        }
        let Some(t) = axis_solid_time(eye, disp, axis, world) else {
            continue;
        };
        consider(
            &mut best,
            SweepHit {
                t,
                axis,
                floor: axis == 1 && delta < 0.0,
            },
        );
    }
    best
}

/// Earliest fraction at which the leading face enters a solid cell.
fn axis_solid_time(eye: Vec3, disp: Vec3, axis: usize, world: &World) -> Option<f32> {
    let delta = disp[axis];
    let (min, max) = body_extents(eye);
    let lead = if delta > 0.0 { max[axis] } else { min[axis] };
    if let Some(cell) = interior_cell(lead)
        && face_is_solid(eye, axis, cell, world)
    {
        return Some(0.0);
    }
    let mut plane = first_plane(lead, delta);
    let step = if delta > 0.0 { 1.0 } else { -1.0 };
    for _ in 0..MAX_SWEEP_PLANES {
        let t = (plane - lead) / delta;
        if t > 1.0 + 1.0e-5 {
            return None;
        }
        if t >= -1.0e-6 {
            let t = t.clamp(0.0, 1.0);
            let cell = entered_cell(plane, delta);
            if face_is_solid(eye + disp * t, axis, cell, world) {
                return Some(t);
            }
        }
        plane += step;
    }
    // The plane walk gave up while displacement remains. Treat that as an
    // immediate stop so the body cannot skip the unsearched cells.
    Some(0.0)
}

fn interior_cell(lead: f32) -> Option<i32> {
    let floor = lead.floor();
    let frac = lead - floor;
    if !(1.0e-4..=1.0 - 1.0e-4).contains(&frac) {
        return None;
    }
    Some(floor as i32)
}

fn first_plane(lead: f32, delta: f32) -> f32 {
    if delta > 0.0 {
        if (lead - lead.floor()).abs() <= 1.0e-5 {
            lead
        } else {
            lead.ceil()
        }
    } else {
        lead.floor()
    }
}

fn entered_cell(plane: f32, delta: f32) -> i32 {
    let index = plane.floor() as i32;
    if delta > 0.0 { index } else { index - 1 }
}

fn face_is_solid(eye: Vec3, axis: usize, cell: i32, world: &World) -> bool {
    let (min, max) = body_extents(eye);
    let mut span = [(0, -1); 3];
    for other in 0..3 {
        if other == axis {
            span[other] = (cell, cell);
            continue;
        }
        let Some(range) = covered_cells(min[other], max[other]) else {
            return false;
        };
        span[other] = range;
    }
    for z in span[2].0..=span[2].1 {
        for y in span[1].0..=span[1].1 {
            for x in span[0].0..=span[0].1 {
                if world.block_at(x, y, z).is_solid() {
                    return true;
                }
            }
        }
    }
    false
}

/// Cells the box covers on one axis. An exact far boundary is excluded so a
/// body flush with a ceiling or wall can still slide along it.
fn covered_cells(min_v: f32, max_v: f32) -> Option<(i32, i32)> {
    let lo = min_v.floor() as i32;
    let hi = (max_v - 1.0e-4).floor() as i32;
    if hi < lo { None } else { Some((lo, hi)) }
}

fn earliest_footing_loss(eye: Vec3, disp: Vec3, world: &World) -> Option<SweepHit> {
    if !Player::has_footing(eye, world) {
        return None;
    }
    let mut best: Option<SweepHit> = None;
    let (min, max) = body_extents(eye);
    for axis in [0usize, 2] {
        let delta = disp[axis];
        if delta.abs() <= 1.0e-8 {
            continue;
        }
        for edge in [min[axis] + SKIN, max[axis] - SKIN] {
            let mut plane = if delta > 0.0 {
                let ceil = edge.ceil();
                if (ceil - edge).abs() <= 1.0e-5 {
                    edge + 1.0
                } else {
                    ceil
                }
            } else {
                let floor = edge.floor();
                if (edge - floor).abs() <= 1.0e-5 {
                    edge - 1.0
                } else {
                    floor
                }
            };
            let step = if delta > 0.0 { 1.0 } else { -1.0 };
            for _ in 0..4 {
                let t = (plane - edge) / delta;
                if t > 1.0 {
                    break;
                }
                if t > 1.0e-6 {
                    // Step far enough past the integer that floor() changes.
                    // A 0.1 mm probe can sit exactly on the plane, where the
                    // cell is still counted and the ledge looks safe.
                    let past = if delta > 0.0 {
                        plane + 1.0e-3
                    } else {
                        plane - 1.0e-3
                    };
                    let t_past = ((past - edge) / delta).min(1.0);
                    if t_past > 0.0 && !Player::has_footing(eye + disp * t_past, world) {
                        consider(
                            &mut best,
                            SweepHit {
                                t,
                                axis,
                                floor: false,
                            },
                        );
                        break;
                    }
                }
                plane += step;
            }
        }
    }
    best
}

fn consider(best: &mut Option<SweepHit>, hit: SweepHit) {
    if best.is_none_or(|current| hit_before(hit, current)) {
        *best = Some(hit);
    }
}

/// Earlier time wins. Equal times prefer a floor, then X, then Z, then a ceiling.
fn hit_before(hit: SweepHit, other: SweepHit) -> bool {
    const TIE: f32 = 1.0e-5;
    if hit.t + TIE < other.t {
        return true;
    }
    if other.t + TIE < hit.t {
        return false;
    }
    hit_rank(hit) < hit_rank(other)
}

fn hit_rank(hit: SweepHit) -> u8 {
    if hit.floor {
        0
    } else if hit.axis == 0 {
        1
    } else if hit.axis == 2 {
        2
    } else {
        3
    }
}

fn body_extents(eye: Vec3) -> (Vec3, Vec3) {
    (
        eye + Vec3::new(-HALF_WIDTH, -FEET_BELOW_EYE, -HALF_WIDTH),
        eye + Vec3::new(HALF_WIDTH, HEAD_ABOVE_EYE, HALF_WIDTH),
    )
}

fn planar_direction(yaw: f32, forward: f32, right: f32) -> Vec3 {
    let ahead = Vec3::new(yaw.sin(), 0.0, -yaw.cos());
    let side = Vec3::new(yaw.cos(), 0.0, yaw.sin());
    (ahead * forward + side * right).normalize_or_zero()
}

fn damp_horizontal(velocity: &mut Vec3, rate: f32, dt: f32) {
    let speed = velocity.x.hypot(velocity.z);
    if speed <= 1.0e-4 {
        velocity.x = 0.0;
        velocity.z = 0.0;
        return;
    }
    let scaled = (speed - speed * rate * dt).max(0.0) / speed;
    velocity.x *= scaled;
    velocity.z *= scaled;
}

fn accelerate(velocity: &mut Vec3, wish_dir: Vec3, wish_speed: f32, accel: f32, dt: f32) {
    let current = velocity.x * wish_dir.x + velocity.z * wish_dir.z;
    let add = wish_speed - current;
    if add <= 0.0 {
        return;
    }
    let amount = (accel * wish_speed * dt).min(add);
    velocity.x += wish_dir.x * amount;
    velocity.z += wish_dir.z * amount;
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::world::{Biome, Block};

    const DT: f32 = 1.0 / 60.0;
    fn platform() -> World {
        let mut world = World::new(3);
        for x in -3..=3 {
            for z in -3..=3 {
                world.set_block([x, 200, z], Block::Stone);
            }
        }
        world
    }

    fn place(player: &mut Player, eye: Vec3) {
        player.flying = false;
        player.eye = eye;
        player.velocity = Vec3::ZERO;
        player.grounded = false;
        player.jump = JumpMemory::default();
    }

    fn settle(player: &mut Player, world: &World) {
        for _ in 0..300 {
            player.tick(world, Motion::default(), DT);
            if player.grounded {
                return;
            }
        }
        panic!("walker did not land at {}", player.eye);
    }

    fn face_positive_x(player: &mut Player) {
        player.yaw = std::f32::consts::FRAC_PI_2;
    }

    fn walk(forward: f32, right: f32, sprint: bool) -> Motion {
        Motion {
            forward,
            right,
            sprint,
            ..Motion::default()
        }
    }

    fn fill_box(world: &mut World, min: [i32; 3], max: [i32; 3], block: Block) {
        for x in min[0]..=max[0] {
            for y in min[1]..=max[1] {
                for z in min[2]..=max[2] {
                    world.set_block([x, y, z], block);
                }
            }
        }
    }

    fn open_platform() -> World {
        let mut world = World::new(3);
        fill_box(&mut world, [-6, 160, -6], [6, 260, 6], Block::Air);
        fill_box(&mut world, [-6, 200, -6], [6, 200, 6], Block::Stone);
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

    #[test]
    fn acceleration_approaches_walk_speed_and_friction_stops() {
        let world = open_platform();
        let mut player = Player::new(&world);
        place(&mut player, Vec3::new(0.5, 210.0, 0.5));
        settle(&mut player, &world);
        face_positive_x(&mut player);
        player.tick(&world, walk(1.0, 0.0, false), DT);
        let first = player.horizontal_speed();
        assert!(
            (0.4..2.0).contains(&first),
            "first tick should ease in, got {first}"
        );
        let mut previous = first;
        for _ in 0..50 {
            player.tick(&world, walk(1.0, 0.0, false), DT);
            let speed = player.horizontal_speed();
            assert!(
                speed + 1.0e-3 >= previous,
                "{speed} dropped below {previous}"
            );
            assert!(speed <= WALK_SPEED + 0.05, "{speed}");
            previous = speed;
        }
        assert!(
            (WALK_SPEED - 0.08..WALK_SPEED + 0.05).contains(&previous),
            "{previous}"
        );
        for _ in 0..8 {
            player.tick(&world, Motion::default(), DT);
        }
        assert!(
            player.horizontal_speed() > 1.0,
            "friction should not zero speed in a handful of ticks"
        );
        for _ in 0..40 {
            player.tick(&world, Motion::default(), DT);
        }
        assert!(
            player.horizontal_speed() < 0.2,
            "friction should finish the stop, got {}",
            player.horizontal_speed()
        );
    }

    #[test]
    fn air_control_is_weaker_than_ground_reversal() {
        let world = open_platform();
        let ground_speed = {
            let mut player = Player::new(&world);
            place(&mut player, Vec3::new(0.5, 210.0, 0.5));
            settle(&mut player, &world);
            face_positive_x(&mut player);
            for _ in 0..40 {
                player.tick(&world, walk(1.0, 0.0, false), DT);
            }
            for _ in 0..15 {
                player.tick(&world, walk(-1.0, 0.0, false), DT);
            }
            assert!(player.grounded);
            player.velocity.x
        };
        assert!(
            ground_speed < -2.0,
            "ground reversal should commit, got {ground_speed}"
        );

        let mut player = Player::new(&world);
        place(&mut player, Vec3::new(0.5, 210.0, 0.5));
        settle(&mut player, &world);
        face_positive_x(&mut player);
        for _ in 0..40 {
            player.tick(&world, walk(1.0, 0.0, false), DT);
        }
        player.tick(&world, walk(1.0, 0.0, false), DT);
        let mut jumped = Motion {
            jump: true,
            ..walk(1.0, 0.0, false)
        };
        player.tick(&world, jumped, DT);
        assert!(player.velocity.y > 0.0);
        jumped.jump = false;
        jumped.forward = -1.0;
        for _ in 0..15 {
            player.tick(&world, jumped, DT);
            assert!(
                !player.grounded,
                "air sample landed early at {}",
                player.eye
            );
        }
        assert!(
            player.velocity.x > 1.0,
            "air reverse should still carry forward momentum, got {}",
            player.velocity.x
        );
    }

    #[test]
    fn sprint_requires_forward_motion_and_releases_otherwise() {
        let world = open_platform();
        let speed_after = |motion: Motion| {
            let mut player = Player::new(&world);
            place(&mut player, Vec3::new(0.5, 210.0, 0.5));
            settle(&mut player, &world);
            face_positive_x(&mut player);
            for _ in 0..50 {
                player.tick(&world, motion, DT);
            }
            player.horizontal_speed()
        };
        let sprint = speed_after(walk(1.0, 0.0, true));
        let walk_speed = speed_after(walk(1.0, 0.0, false));
        let back = speed_after(walk(-1.0, 0.0, true));
        let strafe = speed_after(walk(0.0, 1.0, true));
        let diagonal = speed_after(walk(1.0, 1.0, true));
        assert!(
            (SPRINT_SPEED - 0.08..SPRINT_SPEED + 0.05).contains(&sprint),
            "{sprint}"
        );
        assert!(
            (WALK_SPEED - 0.08..WALK_SPEED + 0.05).contains(&walk_speed),
            "{walk_speed}"
        );
        assert!(sprint > walk_speed + 2.0);
        assert!(
            (WALK_SPEED - 0.08..WALK_SPEED + 0.05).contains(&back),
            "backward sprint must stay at a walk, got {back}"
        );
        assert!(
            (WALK_SPEED - 0.08..WALK_SPEED + 0.05).contains(&strafe),
            "strafe sprint must stay at a walk, got {strafe}"
        );
        assert!(
            (SPRINT_SPEED - 0.08..SPRINT_SPEED + 0.05).contains(&diagonal),
            "forward strafe may sprint, got {diagonal}"
        );

        let mut player = Player::new(&world);
        place(&mut player, Vec3::new(0.5, 210.0, 0.5));
        settle(&mut player, &world);
        face_positive_x(&mut player);
        for _ in 0..50 {
            player.tick(&world, walk(1.0, 0.0, true), DT);
        }
        assert!(player.horizontal_speed() > 7.5);
        // Park back on the platform so the coast measures friction, not a fall.
        let velocity = player.velocity;
        player.eye.x = 0.5;
        player.eye.z = 0.5;
        player.velocity = velocity;
        for _ in 0..45 {
            player.tick(&world, walk(0.0, 0.0, true), DT);
            assert!(player.grounded, "coast left the ground at {}", player.eye);
        }
        assert!(
            player.horizontal_speed() < 0.3,
            "holding sprint with no direction must coast to a stop, got {}",
            player.horizontal_speed()
        );
    }

    #[test]
    fn coyote_time_forgives_a_late_jump_and_then_expires() {
        let mut world = World::new(3);
        fill_box(&mut world, [-2, 160, -2], [2, 240, 2], Block::Air);
        fill_box(&mut world, [-2, 200, -1], [1, 200, 1], Block::Stone);

        let leave_airborne = || {
            let mut player = Player::new(&world);
            place(&mut player, Vec3::new(0.5, 210.0, 0.5));
            settle(&mut player, &world);
            face_positive_x(&mut player);
            for _ in 0..240 {
                player.tick(&world, walk(1.0, 0.0, false), DT);
                if !player.grounded {
                    return player;
                }
            }
            panic!("never left the ledge at {}", player.eye);
        };

        let mut forgiven = leave_airborne();
        for _ in 0..COYOTE_TICKS - 1 {
            forgiven.tick(&world, Motion::default(), DT);
            assert!(!forgiven.grounded);
        }
        let y = forgiven.eye.y;
        forgiven.tick(
            &world,
            Motion {
                jump: true,
                ..Motion::default()
            },
            DT,
        );
        assert!(
            forgiven.eye.y > y + 0.05,
            "coyote jump should rise, {} -> {}",
            y,
            forgiven.eye.y
        );

        let mut expired = leave_airborne();
        for _ in 0..COYOTE_TICKS {
            expired.tick(&world, Motion::default(), DT);
        }
        let y = expired.eye.y;
        let falling = expired.velocity.y;
        expired.tick(
            &world,
            Motion {
                jump: true,
                ..Motion::default()
            },
            DT,
        );
        assert!(expired.velocity.y <= falling);
        assert!(expired.eye.y < y, "expired coyote must keep falling");
    }

    #[test]
    fn jump_buffer_covers_an_early_press_and_not_an_older_one() {
        let world = open_platform();
        let land_tick = {
            let mut player = Player::new(&world);
            place(&mut player, Vec3::new(0.5, 230.0, 0.5));
            let mut found = None;
            for tick in 0..400 {
                player.tick(&world, Motion::default(), DT);
                if player.grounded {
                    found = Some(tick);
                    break;
                }
            }
            found.expect("fall did not land")
        };
        assert!(land_tick > JUMP_BUFFER_TICKS as usize + 2);

        // A consumed buffer clears `grounded` on the landing tick, so follow the
        // lowest point and any rise after it instead of the grounded flag.
        let rise_after_contact = |press: usize| {
            let mut player = Player::new(&world);
            place(&mut player, Vec3::new(0.5, 230.0, 0.5));
            let mut low = f32::MAX;
            let mut rise = 0.0_f32;
            for tick in 0..land_tick + 40 {
                player.tick(
                    &world,
                    Motion {
                        jump: tick == press,
                        ..Motion::default()
                    },
                    DT,
                );
                if player.eye.y < low - 0.01 {
                    low = player.eye.y;
                    rise = 0.0;
                } else {
                    rise = rise.max(player.eye.y - low);
                }
            }
            (low, rise)
        };

        let earliest = land_tick - (JUMP_BUFFER_TICKS as usize - 1);
        let (low, rise) = rise_after_contact(earliest);
        assert!(
            (202.59..202.76).contains(&low),
            "early press changed the fall, low {low}"
        );
        assert!(
            rise > 0.2,
            "press at {earliest} (landing {land_tick}) should buffer, rose {rise}"
        );
        let (_, same_tick_rise) = rise_after_contact(land_tick);
        assert!(
            same_tick_rise > 0.2,
            "same-tick press rose {same_tick_rise}"
        );

        let stale = land_tick - JUMP_BUFFER_TICKS as usize;
        let (low, rise) = rise_after_contact(stale);
        assert!((202.59..202.76).contains(&low), "stale press low {low}");
        assert!(
            rise < 0.05,
            "press at {stale} should expire, rose {rise} from {low}"
        );
    }

    #[test]
    fn one_jump_impulse_per_takeoff_and_held_jump_repeats_on_landing() {
        let world = open_platform();
        let mut player = Player::new(&world);
        place(&mut player, Vec3::new(0.5, 210.0, 0.5));
        settle(&mut player, &world);
        player.tick(
            &world,
            Motion {
                jump: true,
                ..Motion::default()
            },
            DT,
        );
        let mut previous = player.velocity.y;
        assert!(previous > 0.0);
        for _ in 0..25 {
            player.tick(&world, Motion::default(), DT);
            if player.grounded {
                break;
            }
            assert!(
                player.velocity.y < previous,
                "a single press must not boost again in the air"
            );
            previous = player.velocity.y;
        }

        let mut player = Player::new(&world);
        place(&mut player, Vec3::new(0.5, 210.0, 0.5));
        settle(&mut player, &world);
        let hold = Motion {
            jump: true,
            ..Motion::default()
        };
        let mut takeoffs = 0;
        let mut previous = 0.0;
        for _ in 0..180 {
            player.tick(&world, hold, DT);
            if previous <= 0.0 && player.velocity.y > 0.0 {
                takeoffs += 1;
            }
            if player.velocity.y > 0.0 && previous > 0.0 {
                assert!(
                    player.velocity.y < previous,
                    "held jump must not stack impulses while rising"
                );
            }
            previous = player.velocity.y;
        }
        assert!(
            takeoffs >= 2,
            "holding jump should take off again after landing, saw {takeoffs}"
        );
    }

    #[test]
    fn ceiling_hit_cancels_upward_velocity_without_penetration() {
        let mut world = open_platform();
        let mut player = Player::new(&world);
        place(&mut player, Vec3::new(0.5, 210.0, 0.5));
        settle(&mut player, &world);
        // Built after the fall so the body settles on the floor, under the lid.
        fill_box(&mut world, [-2, 203, -2], [2, 203, 2], Block::Stone);
        let start_y = player.eye.y;
        let mut peak_up = 0.0_f32;
        let mut cancelled = false;
        for _ in 0..40 {
            player.tick(
                &world,
                Motion {
                    jump: true,
                    ..Motion::default()
                },
                DT,
            );
            assert!(
                !Player::collides(player.eye, &world),
                "embedded at {}",
                player.eye
            );
            let head = player.eye.y + HEAD_ABOVE_EYE;
            // Collision insets the body by SKIN, so the raw head can sit a
            // fraction of a millimetre into the block face without embedding.
            assert!(head < 203.0 + 0.01, "head penetrated the ceiling at {head}");
            peak_up = peak_up.max(player.velocity.y);
            if peak_up > 2.0 && player.velocity.y <= 0.0 {
                cancelled = true;
                assert!(
                    player.eye.y - start_y < 0.5,
                    "upward speed died at a normal apex, rise {}",
                    player.eye.y - start_y
                );
                break;
            }
        }
        assert!(cancelled, "ceiling never cancelled the jump");
    }

    #[test]
    fn full_block_ledge_blocks_walking_until_a_jump() {
        let mut world = World::new(3);
        fill_box(&mut world, [-4, 160, -3], [8, 240, 3], Block::Air);
        fill_box(&mut world, [-4, 200, -3], [1, 200, 3], Block::Stone);
        fill_box(&mut world, [2, 200, -3], [8, 201, 3], Block::Stone);
        let mut player = Player::new(&world);
        place(&mut player, Vec3::new(0.5, 210.0, 0.5));
        settle(&mut player, &world);
        face_positive_x(&mut player);
        let stand = player.eye.y;
        for _ in 0..120 {
            player.tick(&world, walk(1.0, 0.0, false), DT);
        }
        assert!(player.grounded);
        assert!(
            (player.eye.y - stand).abs() < 0.08,
            "walking stepped up, eye {}",
            player.eye.y
        );
        assert!(
            player.eye.x < 2.2,
            "walking passed the one-block face at x {}",
            player.eye.x
        );
        assert!(!Player::collides(player.eye, &world));

        let mut player = Player::new(&world);
        place(&mut player, Vec3::new(0.5, 210.0, 0.5));
        settle(&mut player, &world);
        face_positive_x(&mut player);
        // Hold jump only until the body is past the face. A held jump clears
        // grounded on the landing tick, so release it and require a real rest
        // on the upper surface.
        let mut released = false;
        let mut landed = false;
        for _ in 0..240 {
            let over_lip = player.eye.x > 2.15 && player.eye.y > stand + 0.35;
            if over_lip {
                released = true;
            }
            player.tick(
                &world,
                Motion {
                    forward: 1.0,
                    jump: !released,
                    ..Motion::default()
                },
                DT,
            );
            if released && player.grounded && player.eye.y > stand + 0.8 && player.eye.x > 2.0 {
                landed = true;
                break;
            }
        }
        assert!(landed, "jump never stayed on the ledge, at {}", player.eye);
        assert!(!Player::collides(player.eye, &world));
    }

    #[test]
    fn wall_slide_and_outside_corner_do_not_snag() {
        let mut world = open_platform();
        for z in -2..=8 {
            world.set_block([4, 201, z], Block::Stone);
            world.set_block([4, 202, z], Block::Stone);
        }
        let mut player = Player::new(&world);
        place(&mut player, Vec3::new(0.5, 210.0, 0.5));
        settle(&mut player, &world);
        // Face +Z and hold into the wall on +X while walking along it.
        player.yaw = std::f32::consts::PI;
        player.eye.x = 3.15;
        player.eye.z = -1.0;
        let motion = walk(1.0, -1.0, false);
        for _ in 0..120 {
            player.tick(&world, motion, DT);
            assert!(!Player::collides(player.eye, &world), "{}", player.eye);
            assert!(player.eye.x < 3.75, "crossed the wall at {}", player.eye.x);
        }
        assert!(
            player.eye.z > 3.0,
            "diagonal input into a wall failed to slide, z {}",
            player.eye.z
        );
        assert!(
            player.velocity.z > 2.0,
            "slide speed collapsed to {}",
            player.velocity.z
        );

        // The wall ends. Continuing to hold into it must pass the corner.
        let mut world = open_platform();
        for z in 0..=2 {
            world.set_block([4, 201, z], Block::Stone);
            world.set_block([4, 202, z], Block::Stone);
        }
        let mut player = Player::new(&world);
        place(&mut player, Vec3::new(0.5, 210.0, 0.5));
        settle(&mut player, &world);
        player.yaw = std::f32::consts::PI;
        player.eye.x = 3.15;
        player.eye.z = -1.5;
        for _ in 0..150 {
            player.tick(&world, motion, DT);
            assert!(!Player::collides(player.eye, &world), "{}", player.eye);
        }
        assert!(
            player.eye.z > 4.0,
            "outside corner snagged the body at {}",
            player.eye
        );
    }

    #[test]
    fn high_speed_landing_and_thin_wall_do_not_tunnel() {
        let mut world = World::new(3);
        fill_box(&mut world, [-1, 120, -1], [1, 420, 1], Block::Air);
        fill_box(&mut world, [-1, 200, -1], [1, 200, 1], Block::Stone);
        let mut player = Player::new(&world);
        place(&mut player, Vec3::new(0.5, 360.0, 0.5));
        let mut saw_terminal = false;
        let mut lowest_near_ground = f32::MAX;
        for _ in 0..700 {
            player.tick(&world, Motion::default(), DT);
            assert!(player.velocity.y >= -TERMINAL_VELOCITY - 1.0e-3);
            assert!(player.eye.is_finite());
            if player.eye.y > 280.0 && player.velocity.y <= -TERMINAL_VELOCITY + 1.0e-3 {
                saw_terminal = true;
                let before = player.eye.y;
                player.tick(&world, Motion::default(), DT);
                let step = player.eye.y - before;
                assert!(
                    (step + TERMINAL_VELOCITY * DT).abs() < 0.02,
                    "terminal step {step}"
                );
            }
            if player.eye.y < 230.0 {
                lowest_near_ground = lowest_near_ground.min(player.eye.y);
            }
            if player.grounded && player.eye.y < 230.0 {
                break;
            }
        }
        assert!(saw_terminal);
        assert!(player.grounded);
        assert!(
            (202.59..202.76).contains(&player.eye.y),
            "landed at {}",
            player.eye.y
        );
        assert!(
            lowest_near_ground > 202.5,
            "fell through the floor to {lowest_near_ground}"
        );
        assert!(!Player::collides(player.eye, &world));

        // Already at the contact band with a downward speed the tick will clamp
        // to terminal. An unsplit move would push the body through the floor.
        let mut player = Player::new(&world);
        place(&mut player, Vec3::new(0.5, 202.62, 0.5));
        player.velocity.y = -90.0;
        player.tick(&world, Motion::default(), DT);
        assert!(player.grounded, "fast landing missed at {}", player.eye);
        assert_eq!(player.velocity.y, 0.0);
        assert!(!Player::collides(player.eye, &world));
        assert!(
            (202.59..202.76).contains(&player.eye.y),
            "tunneled or hovered at {}",
            player.eye.y
        );

        world.set_block([2, 201, 0], Block::Stone);
        world.set_block([2, 202, 0], Block::Stone);
        let mut player = Player::new(&world);
        place(&mut player, Vec3::new(1.5, 202.62, 0.5));
        player.velocity.x = 40.0;
        player.tick(&world, Motion::default(), DT);
        assert_eq!(player.velocity.x, 0.0);
        assert!(
            (1.68..1.71).contains(&player.eye.x),
            "thin wall was missed or crossed at {}",
            player.eye.x
        );
        assert!(!Player::collides(player.eye, &world));
    }

    #[test]
    fn fixed_tick_sequence_is_deterministic_and_gravity_stays_bounded() {
        let world = open_platform();
        let simulate = || {
            let mut player = Player::new(&world);
            place(&mut player, Vec3::new(0.5, 230.0, 0.5));
            face_positive_x(&mut player);
            for i in 0..160 {
                let motion = Motion {
                    forward: if i < 40 {
                        1.0
                    } else if i < 70 {
                        -1.0
                    } else {
                        0.0
                    },
                    right: if (20..50).contains(&i) { 1.0 } else { 0.0 },
                    sprint: (10..60).contains(&i),
                    jump: i == 30 || (80..90).contains(&i),
                    sneak: (100..130).contains(&i),
                    ..Motion::default()
                };
                player.tick(&world, motion, DT);
            }
            player
        };
        let first = simulate();
        let second = simulate();
        assert_eq!(
            first.eye.to_array().map(f32::to_bits),
            second.eye.to_array().map(f32::to_bits)
        );
        assert_eq!(
            first.velocity.to_array().map(f32::to_bits),
            second.velocity.to_array().map(f32::to_bits)
        );
        assert_eq!(first.grounded, second.grounded);

        // Nine metres of fall in one step. Skipping collision, or stopping the
        // sweep short of the floor, leaves this body outside the stand band.
        let mut player = Player::new(&world);
        place(&mut player, Vec3::new(0.5, 212.0, 0.5));
        player.velocity.y = -40.0;
        player.tick(&world, Motion::default(), 0.5);
        assert!(player.velocity.is_finite() && player.eye.is_finite());
        assert!(player.velocity.y >= -TERMINAL_VELOCITY - 1.0e-3);
        assert!(
            player.grounded,
            "large step missed the floor at {}",
            player.eye
        );
        assert!(
            (202.59..202.76).contains(&player.eye.y),
            "large step ended at {}",
            player.eye.y
        );
        assert!(!Player::collides(player.eye, &world));
        assert!(Player::has_footing(player.eye, &world));
    }

    #[test]
    fn sprint_terminal_fall_catches_block_lip() {
        let mut world = World::new(3);
        fill_box(&mut world, [-2, 160, -2], [3, 260, 2], Block::Air);
        world.set_block([0, 200, 0], Block::Stone);
        let mut player = Player::new(&world);
        place(&mut player, Vec3::new(1.2, 202.7, 0.5));
        player.velocity = Vec3::new(8.0, -TERMINAL_VELOCITY, 0.0);
        player.tick(&world, Motion::default(), DT);
        assert!(
            player.grounded,
            "sprint off the lip missed the landing at {}",
            player.eye
        );
        assert!(
            (202.59..202.76).contains(&player.eye.y),
            "lip landing height {}",
            player.eye.y
        );
        assert!(
            (1.20..1.31).contains(&player.eye.x),
            "slid past the lip to x {}",
            player.eye.x
        );
        assert!(
            Player::has_footing(player.eye, &world),
            "landed without the stone underfoot at {}",
            player.eye
        );
        assert!(!Player::collides(player.eye, &world));
        assert_eq!(player.velocity.y, 0.0);
    }

    #[test]
    fn centered_fast_fall_with_horizontal_speed_lands() {
        let world = open_platform();
        let mut player = Player::new(&world);
        place(&mut player, Vec3::new(0.5, 203.2, 0.5));
        player.velocity = Vec3::new(8.0, -TERMINAL_VELOCITY, 0.0);
        player.tick(&world, Motion::default(), DT);
        assert!(player.grounded, "centered fall missed at {}", player.eye);
        assert!(
            (202.59..202.76).contains(&player.eye.y),
            "centered fall height {}",
            player.eye.y
        );
        assert!(
            (0.55..0.70).contains(&player.eye.x),
            "horizontal speed was dropped or over-applied, x {}",
            player.eye.x
        );
        assert!(Player::has_footing(player.eye, &world), "{}", player.eye);
        assert!(!Player::collides(player.eye, &world));
    }

    #[test]
    fn sneak_diagonal_single_block_and_l_corner() {
        let mut world = World::new(3);
        fill_box(&mut world, [-2, 160, -2], [3, 240, 3], Block::Air);
        world.set_block([0, 200, 0], Block::Stone);
        let mut player = Player::new(&world);
        place(&mut player, Vec3::new(0.5, 210.0, 0.5));
        settle(&mut player, &world);
        face_positive_x(&mut player);
        let diagonal = Motion {
            forward: 1.0,
            right: 1.0,
            sneak: true,
            ..Motion::default()
        };
        for _ in 0..200 {
            player.tick(&world, diagonal, DT);
            assert!(
                player.grounded && player.eye.y > 202.4,
                "diagonal sneak left the block at {}",
                player.eye
            );
            assert!(
                Player::has_footing(player.eye, &world),
                "diagonal sneak lost the block at {}",
                player.eye
            );
            assert!(!Player::collides(player.eye, &world));
        }
        assert!(
            player.eye.x > 0.8 && player.eye.z > 0.8,
            "diagonal sneak did not reach the corner at {}",
            player.eye
        );

        let mut world = World::new(3);
        fill_box(&mut world, [-2, 160, -2], [4, 240, 4], Block::Air);
        for (x, z) in [(0, 0), (1, 0), (2, 0), (2, 1), (2, 2)] {
            world.set_block([x, 200, z], Block::Stone);
        }
        let mut player = Player::new(&world);
        place(&mut player, Vec3::new(0.5, 210.0, 0.5));
        settle(&mut player, &world);
        face_positive_x(&mut player);
        let along = Motion {
            forward: 1.0,
            sneak: true,
            ..Motion::default()
        };
        let mut turned = false;
        for _ in 0..400 {
            if !turned && player.eye.x > 2.4 {
                player.yaw = std::f32::consts::PI;
                turned = true;
            }
            player.tick(&world, along, DT);
            assert!(
                player.grounded && player.eye.y > 202.4,
                "L sneak fell at {}",
                player.eye
            );
            assert!(
                Player::has_footing(player.eye, &world),
                "L sneak lost footing at {}",
                player.eye
            );
            assert!(!Player::collides(player.eye, &world));
        }
        assert!(turned, "never reached the stem of the L at {}", player.eye);
        assert!(
            player.eye.z > 1.8 && player.eye.x > 2.0,
            "sneak did not follow the L to {}",
            player.eye
        );
    }

    #[test]
    fn sneak_retreats_and_jump_leaves_the_ledge() {
        let mut world = World::new(3);
        fill_box(&mut world, [-2, 160, -2], [8, 240, 2], Block::Air);
        fill_box(&mut world, [0, 200, -1], [5, 200, 1], Block::Stone);

        let mut player = Player::new(&world);
        place(&mut player, Vec3::new(2.5, 210.0, 0.5));
        settle(&mut player, &world);
        face_positive_x(&mut player);
        let retreat = Motion {
            forward: -1.0,
            sneak: true,
            ..Motion::default()
        };
        for _ in 0..300 {
            player.tick(&world, retreat, DT);
            assert!(
                player.grounded && player.eye.y > 202.4,
                "sneak retreat left the platform at {}",
                player.eye
            );
            assert!(
                Player::has_footing(player.eye, &world),
                "sneak retreat lost footing at {}",
                player.eye
            );
        }
        assert!(
            (-0.40..0.50).contains(&player.eye.x),
            "sneak retreat did not hold the near lip at {}",
            player.eye
        );

        let mut player = Player::new(&world);
        place(&mut player, Vec3::new(6.05, 210.0, 0.5));
        settle(&mut player, &world);
        face_positive_x(&mut player);
        let hold = Motion {
            forward: 1.0,
            sneak: true,
            ..Motion::default()
        };
        for _ in 0..40 {
            player.tick(&world, hold, DT);
            assert!(
                player.grounded && Player::has_footing(player.eye, &world),
                "pre-jump sneak walked off at {}",
                player.eye
            );
        }
        let lip_x = player.eye.x;
        assert!(
            (5.6..6.35).contains(&lip_x),
            "not waiting on the far lip at {lip_x}"
        );
        for _ in 0..90 {
            player.tick(
                &world,
                Motion {
                    forward: 1.0,
                    sneak: true,
                    jump: true,
                    ..Motion::default()
                },
                DT,
            );
        }
        assert!(
            player.eye.x > lip_x + 0.25,
            "sneak jump stayed on the lip, from {lip_x} to {}",
            player.eye.x
        );
        assert!(
            player.eye.y < 202.0 || !player.grounded,
            "sneak jump never left the floor at {}",
            player.eye
        );
        assert!(
            !Player::has_footing(player.eye, &world),
            "sneak jump still supported at {}",
            player.eye
        );
    }

    #[test]
    fn sneak_holds_ledges_at_chunk_borders() {
        // Block just inside the border, and which way sneak approaches the drop.
        let cases = [
            ("x=32", 31, true),
            ("x=0/-1", 0, false),
            ("x=-32", -33, true),
            ("x=-33", -32, false),
        ];
        for (label, block_x, positive_x) in cases {
            let mut world = World::new(3);
            fill_box(
                &mut world,
                [block_x - 2, 160, -2],
                [block_x + 2, 240, 2],
                Block::Air,
            );
            world.set_block([block_x, 200, 0], Block::Stone);
            let mut player = Player::new(&world);
            place(&mut player, Vec3::new(block_x as f32 + 0.5, 210.0, 0.5));
            settle(&mut player, &world);
            if positive_x {
                face_positive_x(&mut player);
            } else {
                player.yaw = -std::f32::consts::FRAC_PI_2;
            }
            let start_x = player.eye.x;
            let motion = Motion {
                forward: 1.0,
                sneak: true,
                ..Motion::default()
            };
            for _ in 0..240 {
                player.tick(&world, motion, DT);
                assert!(
                    player.grounded && player.eye.y > 202.4,
                    "{label} sneak fell at {}",
                    player.eye
                );
                assert!(
                    Player::has_footing(player.eye, &world),
                    "{label} lost footing at {}",
                    player.eye
                );
                assert!(!Player::collides(player.eye, &world), "{label}");
            }
            let progress = if positive_x {
                player.eye.x - start_x
            } else {
                start_x - player.eye.x
            };
            assert!(
                progress > 0.35,
                "{label} did not reach the lip, progress {progress} at {}",
                player.eye
            );
        }
    }

    #[test]
    fn sneak_stops_at_edges_and_does_not_sprint() {
        let mut world = World::new(3);
        fill_box(&mut world, [-2, 160, -2], [6, 240, 2], Block::Air);
        fill_box(&mut world, [0, 200, -1], [3, 200, 1], Block::Stone);
        let mut player = Player::new(&world);
        place(&mut player, Vec3::new(0.8, 210.0, 0.5));
        settle(&mut player, &world);
        face_positive_x(&mut player);
        let motion = Motion {
            forward: 1.0,
            sprint: true,
            sneak: true,
            ..Motion::default()
        };
        let mut moving = 0.0_f32;
        for _ in 0..40 {
            player.tick(&world, motion, DT);
            moving = moving.max(player.horizontal_speed());
        }
        assert!(moving < 2.2, "sneak accepted sprint speed {moving}");
        assert!(moving > 0.8, "sneak never got going, {moving}");
        for _ in 0..180 {
            player.tick(&world, motion, DT);
            assert!(!Player::collides(player.eye, &world));
        }
        assert!(player.grounded, "sneak walked off at {}", player.eye);
        assert!(player.eye.y > 202.4, "sneak fell to {}", player.eye.y);
        assert!(
            player.eye.x > 2.0,
            "sneak did not reach the lip, x {}",
            player.eye.x
        );
        assert!(
            Player::has_footing(player.eye, &world),
            "stopped without footing at {}",
            player.eye
        );

        let mut player = Player::new(&world);
        place(&mut player, Vec3::new(0.8, 210.0, 0.5));
        settle(&mut player, &world);
        face_positive_x(&mut player);
        for _ in 0..180 {
            player.tick(&world, walk(1.0, 0.0, false), DT);
        }
        assert!(
            !player.grounded || player.eye.y < 201.5,
            "unsneaked walk should leave the platform, at {}",
            player.eye
        );
    }

    #[test]
    fn flight_sprint_stays_available_in_every_direction() {
        let world = platform();
        let mut sprinting = Player::new(&world);
        assert!(sprinting.toggle_flight(&world));
        sprinting.yaw = 0.0;
        let start = sprinting.eye.z;
        sprinting.tick(&world, walk(-1.0, 0.0, true), DT);
        assert!(sprinting.flying);
        assert!(
            sprinting.eye.z - start > 0.5,
            "delta {}",
            sprinting.eye.z - start
        );
        let mut cruising = Player::new(&world);
        assert!(cruising.toggle_flight(&world));
        cruising.yaw = 0.0;
        let start = cruising.eye.z;
        cruising.tick(&world, walk(-1.0, 0.0, false), DT);
        assert!(cruising.eye.z - start < 0.3);
    }

    fn lip_eye(overlap: f32, vx: f32, vz: f32) -> Vec3 {
        let x = if vx > 0.0 {
            1.0 - overlap + HALF_WIDTH
        } else if vx < 0.0 {
            overlap - HALF_WIDTH
        } else {
            0.5
        };
        let z = if vz > 0.0 {
            1.0 - overlap + HALF_WIDTH
        } else if vz < 0.0 {
            overlap - HALF_WIDTH
        } else {
            0.5
        };
        Vec3::new(x, 201.0 + 0.02 + FEET_BELOW_EYE, z)
    }

    fn supported(player: &Player, world: &World) -> bool {
        player.grounded
            && (202.59..202.76).contains(&player.eye.y)
            && Player::has_footing(player.eye, world)
            && !Player::collides(player.eye, world)
    }

    #[test]
    fn swept_lip_matrix_lands_for_every_overlap() {
        let angles = [0.0_f32, 15.0, 30.0, 45.0, 60.0, 90.0];
        let signs = [(1.0_f32, 1.0_f32), (-1.0, 1.0), (1.0, -1.0), (-1.0, -1.0)];
        let vys = [-8.0_f32, -15.0, -32.0, -48.0];
        let overlaps = [0.02_f32, 0.05, 0.10, 0.15, 0.20];
        for &vy in &vys {
            let mut passed = 0usize;
            let mut total = 0usize;
            let mut first_miss = String::new();
            for &deg in &angles {
                let radians = deg.to_radians();
                for &(sign_x, sign_z) in &signs {
                    let vx = 8.0 * radians.cos() * sign_x;
                    let vz = 8.0 * radians.sin() * sign_z;
                    for &overlap in &overlaps {
                        total += 1;
                        let mut world = World::new(3);
                        world.set_block([0, 200, 0], Block::Stone);
                        let mut player = Player::new(&world);
                        place(&mut player, lip_eye(overlap, vx, vz));
                        player.velocity = Vec3::new(vx, vy, vz);
                        player.tick(&world, Motion::default(), DT);
                        if supported(&player, &world) {
                            passed += 1;
                        } else if first_miss.is_empty() {
                            first_miss = format!(
                                "deg={deg} sign=({sign_x},{sign_z}) overlap={overlap} eye={} grounded={} footing={}",
                                player.eye,
                                player.grounded,
                                Player::has_footing(player.eye, &world)
                            );
                        }
                    }
                }
            }
            println!("lip vy={vy}: {passed}/{total}");
            assert!(
                first_miss.is_empty(),
                "vy={vy} passed {passed}/{total}; first miss {first_miss}"
            );
        }
    }

    #[test]
    fn negative_corner_lip_lands() {
        let mut passed = 0usize;
        for &overlap in &[0.04_f32, 0.08] {
            for &vy in &[-8.0_f32, -48.0] {
                let mut world = World::new(3);
                world.set_block([-1, 200, -1], Block::Stone);
                let speed = 8.0 / std::f32::consts::SQRT_2;
                let mut player = Player::new(&world);
                place(
                    &mut player,
                    Vec3::new(
                        -1.0 + overlap - HALF_WIDTH,
                        201.0 + 0.02 + FEET_BELOW_EYE,
                        -1.0 + overlap - HALF_WIDTH,
                    ),
                );
                player.velocity = Vec3::new(-speed, vy, -speed);
                player.tick(&world, Motion::default(), DT);
                assert!(
                    supported(&player, &world),
                    "negative corner overlap={overlap} vy={vy} at {} grounded={}",
                    player.eye,
                    player.grounded
                );
                passed += 1;
            }
        }
        println!("negative corner: {passed}/4");
    }

    #[test]
    fn sprint_jump_runway_lands_on_every_frame_phase() {
        let mut landed_phases = 0usize;
        for phase in 0..8 {
            let mut world = World::new(3);
            fill_box(&mut world, [-2, 160, -2], [40, 260, 2], Block::Air);
            fill_box(&mut world, [0, 200, -1], [8, 200, 1], Block::Stone);
            world.set_block([12, 200, 0], Block::Stone);
            let mut player = Player::new(&world);
            let start_x = 2.0 + phase as f32 * SPRINT_SPEED * DT / 8.0;
            place(&mut player, Vec3::new(start_x, 210.0, 0.5));
            settle(&mut player, &world);
            face_positive_x(&mut player);
            let jump_at = 7.2;
            let mut jumped = false;
            let mut landed = false;
            for _ in 0..400 {
                let takeoff = !jumped && player.eye.x >= jump_at;
                if takeoff {
                    jumped = true;
                }
                player.tick(
                    &world,
                    Motion {
                        forward: 1.0,
                        sprint: true,
                        jump: takeoff,
                        ..Motion::default()
                    },
                    DT,
                );
                if jumped && player.grounded && player.eye.x > 9.0 {
                    landed = true;
                    break;
                }
            }
            assert!(landed, "phase {phase} never landed, at {}", player.eye);
            assert!(
                supported(&player, &world),
                "phase {phase} missed the block at {}",
                player.eye
            );
            assert!(
                (12.0..13.0).contains(&player.eye.x),
                "phase {phase} landed off the single block at {}",
                player.eye
            );
            landed_phases += 1;
        }
        println!("sprint-jump runway phases: {landed_phases}/8");
    }

    #[test]
    fn starting_overlap_does_not_launch_or_trap() {
        let mut world = World::new(3);
        fill_box(&mut world, [-2, 160, -2], [6, 240, 2], Block::Air);
        fill_box(&mut world, [-2, 200, -1], [6, 200, 1], Block::Stone);
        world.set_block([2, 201, 0], Block::Stone);
        world.set_block([2, 202, 0], Block::Stone);
        let buried = Vec3::new(1.72, 202.62, 0.5);

        let mut leaving = Player::new(&world);
        place(&mut leaving, buried);
        leaving.grounded = true;
        leaving.velocity = Vec3::new(-4.0, 0.0, 0.0);
        leaving.tick(&world, Motion::default(), DT);
        assert!(
            leaving.eye.x < buried.x - 0.01,
            "overlap trapped the outward move at {}",
            leaving.eye
        );
        assert!(
            leaving.eye.y < buried.y + 0.05,
            "overlap launched the body to {}",
            leaving.eye.y
        );
        assert!(!Player::collides(leaving.eye, &world));

        let mut pushing = Player::new(&world);
        place(&mut pushing, buried);
        pushing.grounded = true;
        pushing.velocity = Vec3::new(4.0, 0.0, 0.0);
        pushing.tick(&world, Motion::default(), DT);
        assert!(
            pushing.eye.x <= buried.x + 1.0e-3,
            "overlap shoved the body to {}",
            pushing.eye
        );
        assert!(
            pushing.eye.y <= buried.y + 0.02,
            "overlap launched upward to {}",
            pushing.eye.y
        );
    }
}
