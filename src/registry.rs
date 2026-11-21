//! Versioned block and item registration. Mods add definitions; they do not
//! edit the compiled builtin enum to exist.

use std::collections::HashMap;

use crate::world::Block;

pub const REGISTRY_VERSION: u32 = 1;
pub const BUILTIN_NAMESPACE: &str = "strata";

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct BlockDef {
    pub id: u16,
    pub name: String,
    pub solid: bool,
    pub placeable: bool,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ItemDef {
    pub id: u16,
    pub name: String,
}

#[derive(Clone, Debug)]
pub struct Registry {
    pub version: u32,
    blocks: HashMap<String, BlockDef>,
    items: HashMap<String, ItemDef>,
    next_block: u16,
    next_item: u16,
}

impl Registry {
    pub fn builtins() -> Self {
        let mut registry = Self {
            version: REGISTRY_VERSION,
            blocks: HashMap::new(),
            items: HashMap::new(),
            next_block: 0,
            next_item: 0,
        };
        for block in Block::all() {
            let ident = match block {
                Block::Air => "air",
                Block::Grass => "grass",
                Block::Dirt => "dirt",
                Block::Stone => "stone",
                Block::Sand => "sand",
                Block::Water => "water",
                Block::Wood => "wood",
                Block::Leaves => "leaves",
                Block::Snow => "snow",
                Block::Clay => "clay",
                Block::Crystal => "crystal",
                Block::Basalt => "basalt",
                Block::CoalOre => "coal_ore",
                Block::IronOre => "iron_ore",
                Block::GoldOre => "gold_ore",
                Block::DiamondOre => "diamond_ore",
                Block::Motor => "motor",
                Block::Shaft => "shaft",
                Block::Gear => "gear",
                Block::Belt => "belt",
                Block::Assembler => "assembler",
            };
            let name = format!("{BUILTIN_NAMESPACE}:{ident}");
            registry.blocks.insert(
                name.clone(),
                BlockDef {
                    id: block as u16,
                    name,
                    solid: block.is_solid(),
                    placeable: block.is_placeable(),
                },
            );
            registry.next_block = registry.next_block.max(block as u16 + 1);
        }
        registry.register_item_named("strata:wood_log").unwrap();
        registry.register_item_named("strata:stone_chip").unwrap();
        registry.register_item_named("strata:pick").unwrap();
        registry
    }

    pub fn block(&self, name: &str) -> Option<&BlockDef> {
        self.blocks.get(name)
    }

    pub fn item(&self, name: &str) -> Option<&ItemDef> {
        self.items.get(name)
    }

    #[allow(dead_code)]
    pub fn register_block(
        &mut self,
        namespaced: &str,
        solid: bool,
        placeable: bool,
    ) -> Result<u16, String> {
        validate_name(namespaced)?;
        if self.blocks.contains_key(namespaced) {
            return Err(format!("{namespaced} is already registered"));
        }
        let id = self.next_block;
        self.next_block = self
            .next_block
            .checked_add(1)
            .ok_or("block id space exhausted")?;
        self.blocks.insert(
            namespaced.to_owned(),
            BlockDef {
                id,
                name: namespaced.to_owned(),
                solid,
                placeable,
            },
        );
        Ok(id)
    }

    #[allow(dead_code)]
    pub fn register_item(&mut self, namespaced: &str) -> Result<u16, String> {
        self.register_item_named(namespaced)
    }

    fn register_item_named(&mut self, namespaced: &str) -> Result<u16, String> {
        validate_name(namespaced)?;
        if self.items.contains_key(namespaced) {
            return Err(format!("{namespaced} is already registered"));
        }
        let id = self.next_item;
        self.next_item = self
            .next_item
            .checked_add(1)
            .ok_or("item id space exhausted")?;
        self.items.insert(
            namespaced.to_owned(),
            ItemDef {
                id,
                name: namespaced.to_owned(),
            },
        );
        Ok(id)
    }
}

fn validate_name(name: &str) -> Result<(), String> {
    let mut parts = name.split(':');
    let ns = parts.next().unwrap_or("");
    let ident = parts.next().unwrap_or("");
    if parts.next().is_some()
        || ns.is_empty()
        || ident.is_empty()
        || !ns
            .chars()
            .all(|ch| ch.is_ascii_lowercase() || ch.is_ascii_digit() || ch == '_')
        || !ident
            .chars()
            .all(|ch| ch.is_ascii_lowercase() || ch.is_ascii_digit() || ch == '_')
    {
        return Err(format!("invalid content name {name}"));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn registering_a_namespaced_block_leaves_builtins_loaded() {
        let mut registry = Registry::builtins();
        assert_eq!(registry.version, REGISTRY_VERSION);
        assert_eq!(registry.block("strata:stone").map(|def| def.id), Some(3));
        assert_eq!(
            registry.block("strata:assembler").map(|def| def.id),
            Some(20)
        );
        let id = registry
            .register_block("testmod:copper", true, true)
            .unwrap();
        assert_eq!(registry.block("testmod:copper").map(|def| def.id), Some(id));
        assert_eq!(registry.block("strata:grass").unwrap().name, "strata:grass");
        assert!(registry.item("strata:pick").is_some());
        assert!(registry.register_item("testmod:ingot").is_ok());
        assert_eq!(
            registry
                .item("testmod:ingot")
                .map(|item| item.name.as_str()),
            Some("testmod:ingot")
        );
        assert!(registry.register_block("strata:stone", true, true).is_err());
    }
}
