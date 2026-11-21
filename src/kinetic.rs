//! Create × Factorio kinetic slice: rotation, stress, belts, assembler backpressure.
//! Survival remains playable without placing any of these blocks.

use std::collections::{HashMap, VecDeque};

use crate::world::{Block, World};

pub const SOURCE_RPM: f32 = 32.0;
pub const SOURCE_CAPACITY: f32 = 16.0;
pub const SHAFT_STRESS: f32 = 1.0;
pub const GEAR_STRESS: f32 = 2.0;
pub const BELT_STRESS: f32 = 1.0;
pub const ASSEMBLER_STRESS: f32 = 8.0;
pub const BELT_TICKS_PER_CELL: u32 = 8;
pub const ASSEMBLER_CRAFT_TICKS: u32 = 20;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum KineticKind {
    Motor,
    Shaft,
    Gear,
    Belt,
    Assembler,
}

impl KineticKind {
    pub fn from_block(block: Block) -> Option<Self> {
        match block {
            Block::Motor => Some(Self::Motor),
            Block::Shaft => Some(Self::Shaft),
            Block::Gear => Some(Self::Gear),
            Block::Belt => Some(Self::Belt),
            Block::Assembler => Some(Self::Assembler),
            _ => None,
        }
    }

    pub fn stress(self) -> f32 {
        match self {
            Self::Motor => 0.0,
            Self::Shaft => SHAFT_STRESS,
            Self::Gear => GEAR_STRESS,
            Self::Belt => BELT_STRESS,
            Self::Assembler => ASSEMBLER_STRESS,
        }
    }
}

#[derive(Clone, Debug)]
pub struct Machine {
    pub kind: KineticKind,
    pub rpm: f32,
    pub direction: i8,
    pub stalled: bool,
    pub belt_item: Option<Block>,
    pub belt_progress: u32,
    pub input: VecDeque<Block>,
    pub output: Option<Block>,
    pub craft_progress: u32,
}

impl Machine {
    pub fn new(kind: KineticKind) -> Self {
        Self {
            kind,
            rpm: 0.0,
            direction: 1,
            stalled: false,
            belt_item: None,
            belt_progress: 0,
            input: VecDeque::new(),
            output: None,
            craft_progress: 0,
        }
    }
}

#[derive(Clone, Debug, Default)]
pub struct KineticWorld {
    machines: HashMap<[i32; 3], Machine>,
}

impl KineticWorld {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn machine(&self, position: [i32; 3]) -> Option<&Machine> {
        self.machines.get(&position)
    }

    #[allow(dead_code)]
    pub fn machine_mut(&mut self, position: [i32; 3]) -> Option<&mut Machine> {
        self.machines.get_mut(&position)
    }

    pub fn on_block(&mut self, position: [i32; 3], block: Block) {
        match KineticKind::from_block(block) {
            Some(kind) => {
                self.machines.insert(position, Machine::new(kind));
            }
            None => {
                self.machines.remove(&position);
            }
        }
    }

    #[allow(dead_code)]
    pub fn insert_belt_item(&mut self, position: [i32; 3], item: Block) -> bool {
        if let Some(machine) = self.machines.get_mut(&position)
            && machine.kind == KineticKind::Belt
            && machine.belt_item.is_none()
        {
            machine.belt_item = Some(item);
            machine.belt_progress = 0;
            return true;
        }
        false
    }

    pub fn tick(&mut self, world: &World) {
        self.sync_from_world(world);
        self.solve_rotation();
        self.tick_belts();
        self.tick_assemblers();
    }

    fn sync_from_world(&mut self, world: &World) {
        let positions: Vec<_> = self.machines.keys().copied().collect();
        for position in positions {
            let block = world.block_at(position[0], position[1], position[2]);
            match KineticKind::from_block(block) {
                None => {
                    self.machines.remove(&position);
                }
                Some(kind) if self.machines.get(&position).map(|m| m.kind) != Some(kind) => {
                    self.machines.insert(position, Machine::new(kind));
                }
                Some(_) => {}
            }
        }
    }

    fn neighbors(position: [i32; 3]) -> [[i32; 3]; 6] {
        let [x, y, z] = position;
        [
            [x + 1, y, z],
            [x - 1, y, z],
            [x, y + 1, z],
            [x, y - 1, z],
            [x, y, z + 1],
            [x, y, z - 1],
        ]
    }

    fn solve_rotation(&mut self) {
        let keys: Vec<_> = self.machines.keys().copied().collect();
        let mut visited = HashMap::new();
        let mut component_id = 0u32;
        for &start in &keys {
            if visited.contains_key(&start) {
                continue;
            }
            let mut stack = vec![start];
            let mut members = Vec::new();
            while let Some(node) = stack.pop() {
                if visited.insert(node, component_id).is_some() {
                    continue;
                }
                members.push(node);
                for neighbor in Self::neighbors(node) {
                    if self.machines.contains_key(&neighbor) {
                        stack.push(neighbor);
                    }
                }
            }
            let mut load = 0.0;
            let mut sources = 0.0;
            let mut direction = 1i8;
            for &member in &members {
                let machine = &self.machines[&member];
                load += machine.kind.stress();
                if machine.kind == KineticKind::Motor {
                    sources += SOURCE_CAPACITY;
                    direction = machine.direction;
                }
            }
            let powered = sources > 0.0;
            let stalled = !powered || load > sources;
            let rpm = if stalled { 0.0 } else { SOURCE_RPM };
            for &member in &members {
                if let Some(machine) = self.machines.get_mut(&member) {
                    machine.stalled = stalled;
                    machine.rpm = rpm;
                    machine.direction = direction;
                }
            }
            component_id += 1;
        }
    }

    fn tick_belts(&mut self) {
        let belts: Vec<_> = self
            .machines
            .iter()
            .filter(|(_, machine)| machine.kind == KineticKind::Belt)
            .map(|(&position, machine)| (position, machine.direction, machine.rpm, machine.stalled))
            .collect();
        let mut moves = Vec::new();
        for (position, direction, rpm, stalled) in belts {
            let Some(machine) = self.machines.get_mut(&position) else {
                continue;
            };
            if stalled || rpm == 0.0 || machine.belt_item.is_none() {
                continue;
            }
            machine.belt_progress += 1;
            if machine.belt_progress >= BELT_TICKS_PER_CELL {
                machine.belt_progress = 0;
                let item = machine.belt_item.take();
                let next = [position[0] + i32::from(direction), position[1], position[2]];
                moves.push((next, item));
            }
        }
        for (next, item) in moves {
            if let (Some(item), Some(dest)) = (item, self.machines.get_mut(&next)) {
                if dest.kind == KineticKind::Belt && dest.belt_item.is_none() {
                    dest.belt_item = Some(item);
                    dest.belt_progress = 0;
                } else if dest.kind == KineticKind::Assembler && dest.input.len() < 4 {
                    dest.input.push_back(item);
                }
            }
        }
    }

    fn tick_assemblers(&mut self) {
        for machine in self.machines.values_mut() {
            if machine.kind != KineticKind::Assembler {
                continue;
            }
            if machine.stalled || machine.rpm == 0.0 {
                continue;
            }
            if machine.output.is_some() {
                continue;
            }
            if machine.craft_progress == 0 {
                if machine.input.len() < 2 {
                    continue;
                }
                let _ = machine.input.pop_front();
                let _ = machine.input.pop_front();
            }
            machine.craft_progress += 1;
            if machine.craft_progress >= ASSEMBLER_CRAFT_TICKS {
                machine.craft_progress = 0;
                machine.output = Some(Block::Crystal);
            }
        }
    }
}

/// Connect a motor through shafts to a consumer and return that consumer's RPM.
#[allow(dead_code)]
pub fn spin_chain(
    world: &mut World,
    kinetic: &mut KineticWorld,
    motor: [i32; 3],
    consumer: [i32; 3],
    path: &[[i32; 3]],
) -> f32 {
    world.set_block(motor, Block::Motor);
    kinetic.on_block(motor, Block::Motor);
    for &cell in path {
        world.set_block(cell, Block::Shaft);
        kinetic.on_block(cell, Block::Shaft);
    }
    world.set_block(consumer, Block::Assembler);
    kinetic.on_block(consumer, Block::Assembler);
    kinetic.tick(world);
    kinetic.machine(consumer).map(|m| m.rpm).unwrap_or(0.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn source_spins_until_stress_stalls_and_belts_move_with_assembler_backpressure() {
        let mut world = World::new(1);
        let mut kinetic = KineticWorld::new();
        let motor = [0, 180, 0];
        let shaft = [1, 180, 0];
        let assembler = [2, 180, 0];
        let rpm = spin_chain(&mut world, &mut kinetic, motor, assembler, &[shaft]);
        assert!((rpm - SOURCE_RPM).abs() < 1e-4);
        assert!(!kinetic.machine(assembler).unwrap().stalled);

        for x in 3..20 {
            world.set_block([x, 180, 0], Block::Assembler);
            kinetic.on_block([x, 180, 0], Block::Assembler);
        }
        kinetic.tick(&world);
        assert!(kinetic.machine(motor).unwrap().stalled);
        assert_eq!(kinetic.machine(motor).unwrap().rpm, 0.0);

        let mut world = World::new(1);
        let mut kinetic = KineticWorld::new();
        world.set_block([0, 181, 0], Block::Motor);
        kinetic.on_block([0, 181, 0], Block::Motor);
        for x in 1..=4 {
            world.set_block([x, 181, 0], Block::Belt);
            kinetic.on_block([x, 181, 0], Block::Belt);
        }
        world.set_block([5, 181, 0], Block::Assembler);
        kinetic.on_block([5, 181, 0], Block::Assembler);
        kinetic.tick(&world);
        assert!(kinetic.insert_belt_item([1, 181, 0], Block::Wood));
        for _ in 0..BELT_TICKS_PER_CELL * 4 {
            kinetic.tick(&world);
        }
        assert!(
            kinetic.machine([4, 181, 0]).unwrap().belt_item.is_some()
                || !kinetic.machine([5, 181, 0]).unwrap().input.is_empty()
        );

        let assembler = kinetic.machine_mut([5, 181, 0]).unwrap();
        assembler.input.clear();
        assembler.input.push_back(Block::Wood);
        assembler.input.push_back(Block::Stone);
        assembler.output = None;
        assembler.craft_progress = 0;
        assembler.rpm = SOURCE_RPM;
        assembler.stalled = false;
        for _ in 0..ASSEMBLER_CRAFT_TICKS {
            kinetic.tick(&world);
        }
        assert_eq!(
            kinetic.machine([5, 181, 0]).unwrap().output,
            Some(Block::Crystal)
        );
        let before = kinetic.machine([5, 181, 0]).unwrap().output;
        kinetic
            .machine_mut([5, 181, 0])
            .unwrap()
            .input
            .push_back(Block::Wood);
        kinetic
            .machine_mut([5, 181, 0])
            .unwrap()
            .input
            .push_back(Block::Stone);
        for _ in 0..ASSEMBLER_CRAFT_TICKS {
            kinetic.tick(&world);
        }
        assert_eq!(kinetic.machine([5, 181, 0]).unwrap().output, before);
    }
}
