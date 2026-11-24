//! Data-driven configuration loaders for sprites, weapons, enemies, waves, and shop items.

use std::collections::BTreeMap;

use prism_game_api::{EntityHandle, GameWorld};
use prism_scene::components::{SpriteAnimation, SpriteRenderer};
use prism_scene::SpriteAtlasConfig;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum WeaponKind {
    Ranged,
    MeleeSweep,
    MeleeThrust,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WeaponDef {
    pub id: String,
    pub name: String,
    pub kind: WeaponKind,
    pub sprite: String,
    pub projectile_sprite: String,
    pub fx_sprite: String,
    pub base_damage: f32,
    pub damage_scaling: f32,
    pub cooldown: f32,
    pub range: f32,
    pub crit_chance_bonus: f32,
    pub crit_multiplier: f32,
    pub knockback: f32,
    pub projectile_speed: f32,
    pub projectiles_per_shot: u32,
    pub spread_degrees: f32,
    pub pierce: u32,
    pub arc_half_degrees: f32,
    pub base_price: u32,
    pub tier_damage_mult: [f32; 4],
    pub tier_cooldown_mult: [f32; 4],
}

impl WeaponDef {
    pub fn tier_index(tier: u8) -> usize {
        (tier.clamp(1, 4) - 1) as usize
    }

    pub fn price_for_tier(&self, tier: u8) -> u32 {
        let idx = Self::tier_index(tier);
        let mult = [1u32, 2, 4, 7][idx];
        self.base_price.saturating_mul(mult)
    }

    pub fn sell_value_for_tier(&self, tier: u8) -> u32 {
        (self.price_for_tier(tier) / 2).max(4)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WeaponsFile {
    pub weapons: Vec<WeaponDef>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum EnemyBehavior {
    Chaser,
    Charger,
    RangedShooter,
    Boss,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EnemyDef {
    pub id: String,
    pub name: String,
    pub sprite: String,
    pub behavior: EnemyBehavior,
    pub base_hp: f32,
    pub hp_per_wave: f32,
    pub speed: f32,
    pub radius: f32,
    pub weight: f32,
    pub contact_damage: f32,
    pub xp_value: u32,
    pub gold_chance: f32,
    pub gold_value: u32,
    pub heal_chance: f32,
    pub is_elite: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EnemiesFile {
    pub enemies: Vec<EnemyDef>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WaveDef {
    pub wave: u32,
    pub duration_seconds: f32,
    pub spawn_interval: f32,
    pub batch_size: u32,
    pub max_active_enemies: usize,
    pub clear_gold_bonus: u32,
    pub enemy_weights: BTreeMap<String, u32>,
    pub boss_spawn: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WavesFile {
    pub waves: Vec<WaveDef>,
}

#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct StatModifiers {
    pub max_hp: f32,
    pub heal_now: f32,
    pub hp_regen: f32,
    pub melee_damage: f32,
    pub ranged_damage: f32,
    pub damage_pct: f32,
    pub attack_speed_pct: f32,
    pub crit_chance_pct: f32,
    pub move_speed_pct: f32,
    pub dodge_pct: f32,
    pub armor: f32,
    pub pickup_radius: f32,
    pub luck: f32,
    pub range_bonus: f32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UpgradeDef {
    pub id: String,
    pub name: String,
    pub description: String,
    pub modifiers: StatModifiers,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ShopItemDef {
    pub id: String,
    pub name: String,
    pub description: String,
    pub price: u32,
    pub modifiers: StatModifiers,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UpgradesAndItemsFile {
    pub level_upgrades: Vec<UpgradeDef>,
    pub shop_items: Vec<ShopItemDef>,
}

#[derive(Debug, Clone)]
pub struct GameConfig {
    pub sprites: SpriteAtlasConfig,
    pub weapons: Vec<WeaponDef>,
    pub enemies: Vec<EnemyDef>,
    pub waves: Vec<WaveDef>,
    pub level_upgrades: Vec<UpgradeDef>,
    pub shop_items: Vec<ShopItemDef>,
}

impl GameConfig {
    pub fn load_from_world(world: &GameWorld<'_>) -> Result<Self, String> {
        let sprites_json = world
            .read_asset_text("data/sprites.json")
            .map_err(|e| format!("read data/sprites.json: {e}"))?;
        let weapons_json = world
            .read_asset_text("data/weapons.json")
            .map_err(|e| format!("read data/weapons.json: {e}"))?;
        let enemies_json = world
            .read_asset_text("data/enemies.json")
            .map_err(|e| format!("read data/enemies.json: {e}"))?;
        let waves_json = world
            .read_asset_text("data/waves.json")
            .map_err(|e| format!("read data/waves.json: {e}"))?;
        let items_json = world
            .read_asset_text("data/upgrades_and_items.json")
            .map_err(|e| format!("read data/upgrades_and_items.json: {e}"))?;

        Self::from_json_strings(
            &sprites_json,
            &weapons_json,
            &enemies_json,
            &waves_json,
            &items_json,
        )
    }

    pub fn from_json_strings(
        sprites_json: &str,
        weapons_json: &str,
        enemies_json: &str,
        waves_json: &str,
        items_json: &str,
    ) -> Result<Self, String> {
        let sprites = SpriteAtlasConfig::from_json_str(sprites_json)
            .map_err(|e| format!("parse sprites.json: {e}"))?;
        let weapons_file: WeaponsFile =
            serde_json::from_str(weapons_json).map_err(|e| format!("parse weapons.json: {e}"))?;
        let enemies_file: EnemiesFile =
            serde_json::from_str(enemies_json).map_err(|e| format!("parse enemies.json: {e}"))?;
        let waves_file: WavesFile =
            serde_json::from_str(waves_json).map_err(|e| format!("parse waves.json: {e}"))?;
        let items_file: UpgradesAndItemsFile = serde_json::from_str(items_json)
            .map_err(|e| format!("parse upgrades_and_items.json: {e}"))?;

        if weapons_file.weapons.is_empty()
            || enemies_file.enemies.is_empty()
            || waves_file.waves.is_empty()
            || items_file.level_upgrades.is_empty()
            || items_file.shop_items.is_empty()
        {
            return Err("survivors config files must contain at least one entry each".into());
        }

        Ok(Self {
            sprites,
            weapons: weapons_file.weapons,
            enemies: enemies_file.enemies,
            waves: waves_file.waves,
            level_upgrades: items_file.level_upgrades,
            shop_items: items_file.shop_items,
        })
    }

    pub fn weapon_by_id(&self, id: &str) -> Option<&WeaponDef> {
        self.weapons.iter().find(|w| w.id == id)
    }

    pub fn enemy_index_by_id(&self, id: &str) -> Option<usize> {
        self.enemies.iter().position(|e| e.id == id)
    }

    pub fn upgrade_by_id(&self, id: &str) -> Option<&UpgradeDef> {
        self.level_upgrades.iter().find(|u| u.id == id)
    }

    pub fn shop_item_by_id(&self, id: &str) -> Option<&ShopItemDef> {
        self.shop_items.iter().find(|i| i.id == id)
    }

    pub fn wave_for_number(&self, wave_num: u32) -> &WaveDef {
        let idx = (wave_num.saturating_sub(1) as usize).min(self.waves.len() - 1);
        &self.waves[idx]
    }

    pub fn total_waves(&self) -> u32 {
        self.waves.len() as u32
    }

    /// Apply a data-configured sprite slice and animation to `entity`.
    pub fn apply_sprite(
        &self,
        world: &mut GameWorld<'_>,
        entity: EntityHandle,
        sprite_key: &str,
        (sorting_layer, order_in_layer): (i32, i32),
        color: [f32; 4],
        flip_x: bool,
    ) {
        let Some(slice) = self.sprites.get(sprite_key) else {
            log::warn!("survivors: missing sprite key '{sprite_key}' in data/sprites.json");
            return;
        };
        let mut renderer = SpriteRenderer {
            sorting_layer,
            order_in_layer,
            color,
            flip_x,
            ..SpriteRenderer::default()
        };
        let mut anim = SpriteAnimation::default();
        slice.apply_to(&mut renderer, Some(&mut anim));
        world.set(entity, renderer);
        if slice.frames > 1 && slice.fps > 0.0 {
            world.set(entity, anim);
        } else {
            world.remove::<SpriteAnimation>(entity);
        }
    }
}
