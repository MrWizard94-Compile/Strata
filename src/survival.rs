//! Gathering, crafting, and player vitality. Survival does not require a factory.

use crate::world::Block;

pub const PLAYER_MAX_HEALTH: f32 = 20.0;
pub const HOSTILE_CONTACT_DAMAGE: f32 = 2.0;
pub const PLAYER_ATTACK_DAMAGE: f32 = 8.0;

#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash)]
pub enum Item {
    Block(Block),
    Pick,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Stack {
    pub item: Item,
    pub count: u16,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Inventory {
    stacks: Vec<Stack>,
}

impl Inventory {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn stacks(&self) -> &[Stack] {
        &self.stacks
    }

    pub fn count(&self, item: Item) -> u16 {
        self.stacks
            .iter()
            .filter(|stack| stack.item == item)
            .map(|stack| stack.count)
            .sum()
    }

    pub fn add(&mut self, item: Item, count: u16) {
        if count == 0 {
            return;
        }
        if let Some(stack) = self.stacks.iter_mut().find(|stack| stack.item == item) {
            stack.count = stack.count.saturating_add(count);
        } else {
            self.stacks.push(Stack { item, count });
        }
    }

    pub fn take(&mut self, item: Item, count: u16) -> bool {
        if self.count(item) < count {
            return false;
        }
        let mut remaining = count;
        self.stacks.retain_mut(|stack| {
            if remaining == 0 || stack.item != item {
                return true;
            }
            if stack.count > remaining {
                stack.count -= remaining;
                remaining = 0;
                true
            } else {
                remaining -= stack.count;
                false
            }
        });
        remaining == 0
    }
}

/// Breaking a resource yields that block as an item. Air and water yield nothing.
pub fn drop_from(block: Block) -> Option<Item> {
    match block {
        Block::Air | Block::Water => None,
        other => Some(Item::Block(other)),
    }
}

/// 3 timber + 2 limestone → a placeable pick, represented as the pick item.
pub fn pick_recipe_inputs() -> [(Item, u16); 2] {
    [
        (Item::Block(Block::Wood), 3),
        (Item::Block(Block::Stone), 2),
    ]
}

pub fn try_craft_pick(inventory: &mut Inventory) -> Result<Item, &'static str> {
    for (item, count) in pick_recipe_inputs() {
        if inventory.count(item) < count {
            return Err("missing ingredients");
        }
    }
    for (item, count) in pick_recipe_inputs() {
        assert!(inventory.take(item, count));
    }
    inventory.add(Item::Pick, 1);
    Ok(Item::Pick)
}

/// The pick can be placed as a lumen crystal marker block.
pub fn placeable_from(item: Item) -> Option<Block> {
    match item {
        Item::Block(block) if block.is_placeable() => Some(block),
        Item::Pick => Some(Block::Crystal),
        Item::Block(_) => None,
    }
}

pub fn is_dead(health: f32) -> bool {
    health <= 0.0
}

pub fn apply_damage(health: f32, amount: f32) -> f32 {
    (health - amount).max(0.0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::world::World;

    #[test]
    fn gather_craft_and_place_do_not_require_kinetic_blocks() {
        let mut world = World::new(1);
        world.set_block([2, 150, 0], Block::Wood);
        world.set_block([3, 150, 0], Block::Wood);
        world.set_block([4, 150, 0], Block::Wood);
        world.set_block([5, 150, 0], Block::Stone);
        world.set_block([6, 150, 0], Block::Stone);
        let mut inventory = Inventory::new();
        for x in 2..=6 {
            let broken = world.block_at(x, 150, 0);
            world.set_block([x, 150, 0], Block::Air);
            if let Some(item) = drop_from(broken) {
                inventory.add(item, 1);
            }
        }
        assert_eq!(inventory.count(Item::Block(Block::Wood)), 3);
        assert_eq!(inventory.count(Item::Block(Block::Stone)), 2);
        assert_eq!(try_craft_pick(&mut inventory).unwrap(), Item::Pick);
        assert_eq!(inventory.count(Item::Pick), 1);
        assert!(
            inventory
                .stacks()
                .iter()
                .any(|stack| stack.item == Item::Pick)
        );
        let placed = placeable_from(Item::Pick).unwrap();
        world.set_block([8, 150, 0], placed);
        assert_eq!(world.block_at(8, 150, 0), Block::Crystal);
        assert_eq!(inventory.count(Item::Block(Block::Motor)), 0);
    }

    #[test]
    fn health_zero_is_dead() {
        assert!(!is_dead(PLAYER_MAX_HEALTH));
        assert!(is_dead(apply_damage(PLAYER_MAX_HEALTH, PLAYER_MAX_HEALTH)));
        assert!(is_dead(0.0));
    }
}
