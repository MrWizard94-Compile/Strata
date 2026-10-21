//! Break/place gestures, reach, and the eight-slot material hotbar.
//!
//! The hotbar is not a fixed list of materials. It stores eight choices drawn
//! from [`Block::all`], filtered by [`Block::is_placeable`]. Adding a solid
//! block to the registry makes it appear in the picker without another edit.

use crate::daylight;
use crate::world::{Block, RayHit, World};

/// Eye-to-face distance, in blocks, for the outline and for break/place.
pub const REACH_BLOCKS: f32 = 5.0;

/// Wait after an edit before a held mouse button edits again.
/// The press itself edits immediately. Further edits run on the fixed 60 Hz tick.
pub const EDIT_REPEAT_SECONDS: f32 = 0.25;

/// A single frame may catch up at most this much simulation time.
/// The cap is shorter than [`EDIT_REPEAT_SECONDS`], so a hitch cannot burst edits.
pub const FRAME_HITCH_CAP_SECONDS: f32 = 0.1;

pub const HOTBAR_SLOTS: usize = 8;

/// Inclusive vertical range already enforced by the editable column.
pub const MIN_EDIT_Y: i32 = -30;
pub const MAX_EDIT_Y: i32 = 158;

const MAX_STORED_EDITS: usize = 99_999;

const BODY_MESSAGE: &str = "Step back to place a block here.";
const HEIGHT_MESSAGE: &str = "This build supports edits between -30 and 158 blocks high.";
const LIMIT_MESSAGE: &str = "This world's current edit limit has been reached.";

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct RepeatFires {
    pub break_block: bool,
    pub place_block: bool,
}

/// Held mouse buttons. Break and place keep separate cooldowns.
#[derive(Clone, Debug, Default)]
pub struct EditRepeat {
    break_held: bool,
    place_held: bool,
    break_cooldown: f32,
    place_cooldown: f32,
}

impl EditRepeat {
    pub fn new() -> Self {
        Self::default()
    }

    /// Arm `place` (right button) or break (left button).
    /// Always returns true: the caller edits on the press, then waits out the cooldown
    /// across [`Self::simulation_tick`] steps. The frame hitch cap is shorter than the
    /// cooldown, so the ticks drained on the press frame cannot fire a second edit.
    pub fn press(&mut self, place: bool) -> bool {
        self.arm(place);
        true
    }

    pub fn release(&mut self, place: bool) {
        if place {
            self.place_held = false;
            self.place_cooldown = 0.0;
        } else {
            self.break_held = false;
            self.break_cooldown = 0.0;
        }
    }

    pub fn clear(&mut self) {
        *self = Self::new();
    }

    /// One fixed simulation step. The playing loop calls this once per drained tick.
    pub fn simulation_tick(&mut self) -> RepeatFires {
        self.advance(daylight::SIM_TICK_SECONDS)
    }

    fn advance(&mut self, dt: f32) -> RepeatFires {
        RepeatFires {
            break_block: self.tick(false, dt),
            place_block: self.tick(true, dt),
        }
    }

    fn arm(&mut self, place: bool) {
        if place {
            self.place_held = true;
            self.place_cooldown = EDIT_REPEAT_SECONDS;
        } else {
            self.break_held = true;
            self.break_cooldown = EDIT_REPEAT_SECONDS;
        }
    }

    fn tick(&mut self, place: bool, dt: f32) -> bool {
        let (held, cooldown) = if place {
            (self.place_held, &mut self.place_cooldown)
        } else {
            (self.break_held, &mut self.break_cooldown)
        };
        if !held || !dt.is_finite() || dt <= 0.0 {
            return false;
        }
        *cooldown -= dt;
        if *cooldown > 0.0 {
            return false;
        }
        *cooldown = EDIT_REPEAT_SECONDS;
        true
    }
}

/// Materials offered to a new player. Older saves that omit a hotbar use this too.
pub fn default_hotbar() -> [Block; HOTBAR_SLOTS] {
    [
        Block::Grass,
        Block::Stone,
        Block::Wood,
        Block::Leaves,
        Block::Sand,
        Block::Clay,
        Block::Crystal,
        Block::Basalt,
    ]
}

/// Placeable blocks in save-id order. This is the picker catalog.
pub fn placeable_blocks() -> Vec<Block> {
    Block::all().filter(|block| block.is_placeable()).collect()
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Hotbar {
    slots: [Block; HOTBAR_SLOTS],
    selected: usize,
}

impl Default for Hotbar {
    fn default() -> Self {
        Self {
            slots: default_hotbar(),
            selected: 0,
        }
    }
}

impl Hotbar {
    pub fn from_saved(slots: [Block; HOTBAR_SLOTS], selected: usize) -> Self {
        assert!(selected < HOTBAR_SLOTS);
        assert!(slots.iter().all(|block| block.is_placeable()));
        Self { slots, selected }
    }

    pub fn slots(&self) -> [Block; HOTBAR_SLOTS] {
        self.slots
    }

    pub fn selected(&self) -> usize {
        self.selected
    }

    pub fn selected_block(&self) -> Block {
        self.slots[self.selected]
    }

    pub fn select(&mut self, index: usize) {
        if index < HOTBAR_SLOTS {
            self.selected = index;
        }
    }

    /// `steps` is positive toward higher slot numbers. The wheel wraps.
    pub fn scroll(&mut self, steps: i32) {
        let len = HOTBAR_SLOTS as i32;
        self.selected = (self.selected as i32 + steps).rem_euclid(len) as usize;
    }

    /// Equip `block` into the selected slot. Air and fluids are refused.
    pub fn assign(&mut self, block: Block) -> bool {
        if !block.is_placeable() {
            return false;
        }
        self.slots[self.selected] = block;
        true
    }
}

pub fn targeted_block(world: &World, eye: [f32; 3], direction: [f32; 3]) -> Option<RayHit> {
    world.raycast(eye, direction, REACH_BLOCKS)
}

/// Cell drawn by the selection outline. Same reach as break and place.
pub fn outline_target(world: &World, eye: [f32; 3], direction: [f32; 3]) -> Option<[i32; 3]> {
    targeted_block(world, eye, direction).map(|hit| hit.block)
}

pub struct EditQuery<'a> {
    pub world: &'a World,
    pub eye: [f32; 3],
    pub direction: [f32; 3],
    pub place: bool,
    pub material: Block,
    pub edit_count: usize,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EditOutcome {
    Applied { position: [i32; 3], block: Block },
    Miss,
    Rejected(&'static str),
}

/// Drain `frame_seconds` into `accumulator`, capping the added time at
/// [`FRAME_HITCH_CAP_SECONDS`], and return how many fixed simulation steps to run.
///
/// The playing loop and the repeat tests both use this, so a hitch cannot
/// advance the hotbar cooldown by more time than the day clock.
pub fn drain_simulation_ticks(accumulator: &mut f32, frame_seconds: f32) -> u32 {
    if !accumulator.is_finite() || *accumulator < 0.0 {
        *accumulator = 0.0;
    }
    // NaN and negative frames must not move the clock. Positive infinity is a
    // runaway frame and is capped like any other hitch.
    if !frame_seconds.is_nan() && frame_seconds > 0.0 {
        *accumulator += frame_seconds.min(FRAME_HITCH_CAP_SECONDS);
    }
    let mut ticks = 0u32;
    while *accumulator >= daylight::SIM_TICK_SECONDS {
        *accumulator -= daylight::SIM_TICK_SECONDS;
        ticks += 1;
    }
    ticks
}

pub fn plan_edit(query: &EditQuery<'_>, body_blocks: impl Fn([i32; 3]) -> bool) -> EditOutcome {
    let Some(hit) = targeted_block(query.world, query.eye, query.direction) else {
        return EditOutcome::Miss;
    };
    let position = if query.place { hit.previous } else { hit.block };
    if query.place && body_blocks(position) {
        return EditOutcome::Rejected(BODY_MESSAGE);
    }
    if !(MIN_EDIT_Y..=MAX_EDIT_Y).contains(&position[1]) {
        return EditOutcome::Rejected(HEIGHT_MESSAGE);
    }
    if query.edit_count >= MAX_STORED_EDITS {
        return EditOutcome::Rejected(LIMIT_MESSAGE);
    }
    if query.place && !query.material.is_placeable() {
        return EditOutcome::Rejected("That material cannot be placed.");
    }
    let block = if query.place {
        query.material
    } else {
        Block::Air
    };
    EditOutcome::Applied { position, block }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn query<'a>(
        world: &'a World,
        eye: [f32; 3],
        place: bool,
        material: Block,
        edit_count: usize,
    ) -> EditQuery<'a> {
        EditQuery {
            world,
            eye,
            direction: [1.0, 0.0, 0.0],
            place,
            material,
            edit_count,
        }
    }

    fn hitch_steps(frame_seconds: f32) -> u32 {
        let mut accumulator = 0.0;
        drain_simulation_ticks(&mut accumulator, frame_seconds)
    }

    fn run_ticks(repeat: &mut EditRepeat, ticks: u32) -> RepeatFires {
        let mut fires = RepeatFires::default();
        for _ in 0..ticks {
            let step = repeat.simulation_tick();
            fires.break_block |= step.break_block;
            fires.place_block |= step.place_block;
        }
        fires
    }

    /// Ticks from the press, including any already drained, until `place` fires.
    fn repeat_after(repeat: &mut EditRepeat, already: u32, place: bool) -> u32 {
        let mut total = already;
        for _ in 0..120 {
            total += 1;
            let fires = repeat.simulation_tick();
            let fired = if place {
                fires.place_block
            } else {
                fires.break_block
            };
            if fired {
                let elapsed = total as f32 * daylight::SIM_TICK_SECONDS;
                let previous = elapsed - daylight::SIM_TICK_SECONDS;
                assert!(
                    previous < EDIT_REPEAT_SECONDS + 1e-4,
                    "tick {total} fired early at {previous}s"
                );
                assert!(
                    elapsed + 1e-4 >= EDIT_REPEAT_SECONDS,
                    "tick {total} fired before the cooldown ({elapsed}s)"
                );
                return total;
            }
        }
        panic!("held button did not repeat");
    }

    #[test]
    fn press_acts_immediately_and_hold_repeats_on_the_simulation_tick() {
        let mut repeat = EditRepeat::new();
        assert!(repeat.press(false), "the press itself is an edit");
        let hitch = hitch_steps(1.0);
        assert!(hitch > 0);
        assert!((hitch as f32) * daylight::SIM_TICK_SECONDS < EDIT_REPEAT_SECONDS);
        assert_eq!(
            run_ticks(&mut repeat, hitch),
            RepeatFires::default(),
            "a one-second frame is capped and must not repeat"
        );
        let fired = repeat_after(&mut repeat, hitch, false);
        assert!(hitch < fired);
        let again = repeat_after(&mut repeat, 0, false);
        assert!(again > 1);
    }

    #[test]
    fn a_hitch_does_not_burst_and_release_cancels_the_hold() {
        let mut repeat = EditRepeat::new();
        assert!(repeat.press(false));
        assert_eq!(
            run_ticks(&mut repeat, hitch_steps(1.0)),
            RepeatFires::default()
        );
        repeat.release(false);
        assert_eq!(run_ticks(&mut repeat, 120), RepeatFires::default());
        assert!(repeat.press(false));
        let partial = hitch_steps(FRAME_HITCH_CAP_SECONDS);
        assert_eq!(run_ticks(&mut repeat, partial), RepeatFires::default());
        assert!(partial < repeat_after(&mut repeat, partial, false));
    }

    #[test]
    fn break_and_place_cooldowns_are_independent() {
        let mut repeat = EditRepeat::new();
        assert!(repeat.press(false));
        let break_at = repeat_after(&mut repeat, 0, false);
        assert!(break_at > hitch_steps(FRAME_HITCH_CAP_SECONDS));
        repeat.release(false);
        assert!(repeat.press(true));
        let mut place_at = 0;
        for tick in 1..120 {
            let fires = repeat.simulation_tick();
            assert!(!fires.break_block, "a released break button must stay idle");
            if fires.place_block {
                place_at = tick;
                break;
            }
        }
        assert!(place_at > 1, "place waits out its own cooldown");
        let elapsed = place_at as f32 * daylight::SIM_TICK_SECONDS;
        let previous = elapsed - daylight::SIM_TICK_SECONDS;
        assert!(previous < EDIT_REPEAT_SECONDS + 1e-4);
        assert!(elapsed + 1e-4 >= EDIT_REPEAT_SECONDS);
    }

    #[test]
    fn non_finite_frames_do_not_drain_extra_ticks() {
        let mut accumulator = f32::NAN;
        let recovered = drain_simulation_ticks(&mut accumulator, 1.0);
        assert!(recovered > 0);
        assert!(accumulator.is_finite() && accumulator >= 0.0);
        accumulator = -5.0;
        assert_eq!(drain_simulation_ticks(&mut accumulator, f32::NAN), 0);
        assert_eq!(accumulator, 0.0);
        assert_eq!(
            drain_simulation_ticks(&mut accumulator, f32::INFINITY),
            recovered
        );
        assert_eq!(drain_simulation_ticks(&mut accumulator, -1.0), 0);

        let mut repeat = EditRepeat::new();
        assert!(repeat.press(true));
        assert_eq!(run_ticks(&mut repeat, 0), RepeatFires::default());
        repeat.clear();
        assert_eq!(
            run_ticks(&mut repeat, hitch_steps(5.0)),
            RepeatFires::default()
        );
    }

    #[test]
    fn reach_limit_gates_the_outline_and_both_edit_actions() {
        let eye = [0.5, 150.5, 0.5];
        let mut near = World::new(1);
        near.set_block([5, 150, 0], Block::Stone);
        assert_eq!(
            outline_target(&near, eye, [1.0, 0.0, 0.0]),
            Some([5, 150, 0])
        );
        assert_eq!(
            plan_edit(&query(&near, eye, false, Block::Dirt, 0), |_| false),
            EditOutcome::Applied {
                position: [5, 150, 0],
                block: Block::Air,
            }
        );

        let mut far = World::new(1);
        far.set_block([6, 150, 0], Block::Stone);
        assert_eq!(outline_target(&far, eye, [1.0, 0.0, 0.0]), None);
        assert_eq!(
            plan_edit(&query(&far, eye, true, Block::Dirt, 0), |_| false),
            EditOutcome::Miss
        );

        // Near face is exactly REACH_BLOCKS away and still counts.
        let mut edge = World::new(1);
        edge.set_block([6, 150, 0], Block::Stone);
        let edge_eye = [1.0, 150.5, 0.5];
        assert_eq!(
            outline_target(&edge, edge_eye, [1.0, 0.0, 0.0]),
            Some([6, 150, 0])
        );

        // The same boundary looking along −X: block −5 occupies [−5, −4),
        // so the near face is 5 blocks from an eye at x = 1.
        let mut west = World::new(1);
        west.set_block([-5, 150, 0], Block::Stone);
        let west_eye = [1.0, 150.5, 0.5];
        let west_dir = [-1.0, 0.0, 0.0];
        assert_eq!(
            outline_target(&west, west_eye, west_dir),
            Some([-5, 150, 0])
        );
        let break_west = EditQuery {
            world: &west,
            eye: west_eye,
            direction: west_dir,
            place: false,
            material: Block::Dirt,
            edit_count: 0,
        };
        assert_eq!(
            plan_edit(&break_west, |_| false),
            EditOutcome::Applied {
                position: [-5, 150, 0],
                block: Block::Air,
            }
        );
        let mut place_west = break_west;
        place_west.place = true;
        assert_eq!(
            plan_edit(&place_west, |_| false),
            EditOutcome::Applied {
                position: [-4, 150, 0],
                block: Block::Dirt,
            }
        );
        let mut beyond = World::new(1);
        beyond.set_block([-6, 150, 0], Block::Stone);
        assert_eq!(outline_target(&beyond, west_eye, west_dir), None);
        assert_eq!(
            plan_edit(
                &EditQuery {
                    world: &beyond,
                    eye: west_eye,
                    direction: west_dir,
                    place: true,
                    material: Block::Dirt,
                    edit_count: 0,
                },
                |_| false
            ),
            EditOutcome::Miss
        );
    }

    #[test]
    fn placement_keeps_the_body_check_and_vertical_bounds() {
        let mut world = World::new(1);
        world.set_block([2, 150, 0], Block::Stone);
        let eye = [0.5, 150.5, 0.5];
        assert_eq!(
            plan_edit(&query(&world, eye, true, Block::Dirt, 0), |position| {
                position == [1, 150, 0]
            }),
            EditOutcome::Rejected(BODY_MESSAGE)
        );
        assert_eq!(
            plan_edit(&query(&world, eye, true, Block::Dirt, 0), |_| false),
            EditOutcome::Applied {
                position: [1, 150, 0],
                block: Block::Dirt,
            }
        );
        assert_eq!(
            plan_edit(&query(&world, eye, true, Block::Water, 0), |_| false),
            EditOutcome::Rejected("That material cannot be placed.")
        );

        world.set_block([2, MAX_EDIT_Y, 0], Block::Stone);
        assert!(matches!(
            plan_edit(
                &query(
                    &world,
                    [0.5, MAX_EDIT_Y as f32 + 0.5, 0.5],
                    false,
                    Block::Dirt,
                    0
                ),
                |_| false
            ),
            EditOutcome::Applied {
                position: [2, MAX_EDIT_Y, 0],
                ..
            }
        ));
        world.set_block([2, MAX_EDIT_Y + 1, 0], Block::Stone);
        assert_eq!(
            plan_edit(
                &query(
                    &world,
                    [0.5, (MAX_EDIT_Y + 1) as f32 + 0.5, 0.5],
                    false,
                    Block::Dirt,
                    0
                ),
                |_| false
            ),
            EditOutcome::Rejected(HEIGHT_MESSAGE)
        );
        world.set_block([2, MIN_EDIT_Y - 1, 0], Block::Stone);
        for x in 0..2 {
            world.set_block([x, MIN_EDIT_Y - 1, 0], Block::Air);
        }
        assert_eq!(
            plan_edit(
                &query(
                    &world,
                    [0.5, (MIN_EDIT_Y - 1) as f32 + 0.5, 0.5],
                    true,
                    Block::Dirt,
                    0
                ),
                |_| false
            ),
            EditOutcome::Rejected(HEIGHT_MESSAGE)
        );
        assert_eq!(
            plan_edit(
                &query(&world, eye, false, Block::Dirt, MAX_STORED_EDITS),
                |_| false
            ),
            EditOutcome::Rejected(LIMIT_MESSAGE)
        );
    }

    #[test]
    fn picker_lists_every_placeable_block() {
        let listed = placeable_blocks();
        let expected: Vec<Block> = Block::all().filter(|block| block.is_placeable()).collect();
        assert_eq!(
            listed, expected,
            "the picker must follow the registry, including blocks added later"
        );
        assert!(expected.contains(&Block::Dirt));
        assert!(expected.contains(&Block::Snow));
        assert!(expected.contains(&Block::CoalOre));
        assert!(expected.contains(&Block::IronOre));
        assert!(expected.contains(&Block::GoldOre));
        assert!(expected.contains(&Block::DiamondOre));
        assert!(expected.len() > HOTBAR_SLOTS);
        assert!(expected.iter().all(|block| block.is_placeable()));
        assert!(!listed.contains(&Block::Air));
        assert!(!listed.contains(&Block::Water));
        let ids: Vec<u8> = listed.iter().map(|block| *block as u8).collect();
        let mut sorted = ids.clone();
        sorted.sort_unstable();
        sorted.dedup();
        assert_eq!(ids, sorted, "catalog ids are unique and sorted");
    }

    #[test]
    fn hotbar_assignment_writes_only_the_selected_slot() {
        let mut hotbar = Hotbar::default();
        assert_eq!(hotbar.slots(), default_hotbar());
        assert!(default_hotbar().iter().all(|block| block.is_placeable()));
        hotbar.select(4);
        hotbar.scroll(1);
        assert_eq!(hotbar.selected(), 5);
        let before = hotbar.slots();
        assert!(hotbar.assign(Block::Snow));
        assert_eq!(hotbar.slots()[5], Block::Snow);
        assert_eq!(hotbar.selected_block(), Block::Snow);
        for (index, slot) in hotbar.slots().into_iter().enumerate() {
            if index != 5 {
                assert_eq!(slot, before[index]);
            }
        }
        assert!(!hotbar.assign(Block::Water));
        assert!(!hotbar.assign(Block::Air));
        assert_eq!(hotbar.slots()[5], Block::Snow);
        hotbar.select(HOTBAR_SLOTS);
        assert_eq!(hotbar.selected(), 5);
        hotbar.scroll(-6);
        assert_eq!(hotbar.selected(), 7);
        hotbar.select(0);
        hotbar.scroll(-1);
        assert_eq!(hotbar.selected(), 7);
    }
}
