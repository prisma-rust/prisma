//! Core game state machine, player stats, weapon inventory, shop mechanics, and pooling buckets.

use prism_game_api::physics2d::{CrowdAgent2D, SpatialGrid2D};
use prism_game_api::{EntityHandle, EntityPool, GameWorld};

use crate::config::{EnemyBehavior, GameConfig, StatModifiers, WeaponDef, WeaponKind};

pub const ARENA_HALF_W: f32 = 22.0;
pub const ARENA_HALF_H: f32 = 15.5;
pub const MAX_WEAPONS: usize = 6;

pub const POOL_ENEMY: u32 = 1;
pub const POOL_PROJECTILE: u32 = 2;
pub const POOL_PICKUP: u32 = 3;
pub const POOL_WARNING: u32 = 4;
pub const POOL_WHITE_BAR: u32 = 5;
pub const POOL_WEAPON: u32 = 6;
pub const POOL_MELEE_FX: u32 = 7;

#[derive(Debug, Clone)]
pub struct XorShift64(pub u64);

impl XorShift64 {
    pub fn new(seed: u64) -> Self {
        Self(if seed == 0 {
            0x9E37_79B9_7F4A_7C15
        } else {
            seed
        })
    }

    pub fn next_u64(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        self.0 = x;
        x
    }

    pub fn next_f32(&mut self) -> f32 {
        let v = (self.next_u64() >> 40) as u32;
        (v as f32) / ((1u32 << 24) as f32)
    }

    pub fn range_usize(&mut self, len: usize) -> usize {
        if len <= 1 {
            return 0;
        }
        (self.next_u64() as usize) % len
    }

    pub fn range_f32(&mut self, min: f32, max: f32) -> f32 {
        min + (max - min) * self.next_f32()
    }
}

#[derive(Debug, Clone)]
pub struct PlayerStats {
    pub hp: f32,
    pub max_hp: f32,
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
    pub level: u32,
    pub xp: u32,
    pub xp_to_next: u32,
    pub gold: u32,
    pub kills: u32,
    pub total_damage: u64,
}

impl Default for PlayerStats {
    fn default() -> Self {
        Self {
            hp: 100.0,
            max_hp: 100.0,
            hp_regen: 0.4,
            melee_damage: 2.0,
            ranged_damage: 2.0,
            damage_pct: 0.0,
            attack_speed_pct: 0.0,
            crit_chance_pct: 6.0,
            move_speed_pct: 0.0,
            dodge_pct: 4.0,
            armor: 1.0,
            pickup_radius: 3.4,
            luck: 0.0,
            range_bonus: 0.0,
            level: 1,
            xp: 0,
            xp_to_next: 24,
            gold: 25,
            kills: 0,
            total_damage: 0,
        }
    }
}

impl PlayerStats {
    pub fn move_speed(&self) -> f32 {
        let base = 6.2;
        (base * (1.0 + self.move_speed_pct / 100.0)).clamp(2.5, 14.0)
    }

    pub fn attack_speed_multiplier(&self) -> f32 {
        (1.0 + self.attack_speed_pct / 100.0).clamp(0.4, 4.0)
    }

    pub fn damage_multiplier(&self) -> f32 {
        (1.0 + self.damage_pct / 100.0).max(0.25)
    }

    pub fn armor_reduction(&self) -> f32 {
        if self.armor >= 0.0 {
            self.armor / (self.armor + 16.0)
        } else {
            self.armor / 20.0
        }
    }

    pub fn apply_modifiers(&mut self, mods: &StatModifiers) {
        self.max_hp = (self.max_hp + mods.max_hp).max(10.0);
        if mods.max_hp > 0.0 {
            self.hp = (self.hp + mods.max_hp).min(self.max_hp);
        } else {
            self.hp = self.hp.min(self.max_hp);
        }
        if mods.heal_now > 0.0 {
            self.hp = (self.hp + mods.heal_now).min(self.max_hp);
        }
        self.hp_regen = (self.hp_regen + mods.hp_regen).max(0.0);
        self.melee_damage += mods.melee_damage;
        self.ranged_damage += mods.ranged_damage;
        self.damage_pct += mods.damage_pct;
        self.attack_speed_pct += mods.attack_speed_pct;
        self.crit_chance_pct = (self.crit_chance_pct + mods.crit_chance_pct).clamp(0.0, 90.0);
        self.move_speed_pct = (self.move_speed_pct + mods.move_speed_pct).clamp(-50.0, 150.0);
        self.dodge_pct = (self.dodge_pct + mods.dodge_pct).clamp(0.0, 60.0);
        self.armor += mods.armor;
        self.pickup_radius = (self.pickup_radius + mods.pickup_radius).max(1.2);
        self.luck += mods.luck;
        self.range_bonus = (self.range_bonus + mods.range_bonus).max(-2.0);
    }

    /// Add XP and return the number of level-ups triggered.
    pub fn grant_xp(&mut self, amount: u32) -> u32 {
        self.xp = self.xp.saturating_add(amount);
        let mut gained = 0u32;
        while self.xp >= self.xp_to_next {
            self.xp -= self.xp_to_next;
            self.level += 1;
            self.xp_to_next = (24 + self.level * 14 + (self.level * self.level) * 2).min(2400);
            self.max_hp += 4.0;
            self.hp = (self.hp + 8.0).min(self.max_hp);
            gained += 1;
        }
        gained
    }
}

#[derive(Debug, Clone)]
pub struct EquippedWeapon {
    pub weapon_id: String,
    pub tier: u8,
    pub cooldown_remaining: f32,
    pub total_cooldown: f32,
    pub swing_timer: f32,
    pub swing_duration: f32,
    pub aim_angle: f32,
    pub entity: EntityHandle,
    pub fx_entity: EntityHandle,
    pub fx_timer: f32,
    pub sprite_applied: bool,
}

impl EquippedWeapon {
    pub fn new(weapon_id: impl Into<String>, tier: u8) -> Self {
        Self {
            weapon_id: weapon_id.into(),
            tier: tier.clamp(1, 4),
            cooldown_remaining: 0.15,
            total_cooldown: 0.75,
            swing_timer: 0.0,
            swing_duration: 0.18,
            aim_angle: 0.0,
            entity: EntityHandle::PLACEHOLDER,
            fx_entity: EntityHandle::PLACEHOLDER,
            fx_timer: 0.0,
            sprite_applied: false,
        }
    }

    pub fn compute_damage(&self, def: &WeaponDef, stats: &PlayerStats) -> f32 {
        let tier_idx = WeaponDef::tier_index(self.tier);
        let flat = match def.kind {
            WeaponKind::Ranged => stats.ranged_damage,
            WeaponKind::MeleeSweep | WeaponKind::MeleeThrust => stats.melee_damage,
        };
        let raw = def.base_damage * def.tier_damage_mult[tier_idx] + flat * def.damage_scaling;
        (raw * stats.damage_multiplier()).max(1.0)
    }

    pub fn compute_cooldown(&self, def: &WeaponDef, stats: &PlayerStats) -> f32 {
        let tier_idx = WeaponDef::tier_index(self.tier);
        let cd = def.cooldown * def.tier_cooldown_mult[tier_idx] / stats.attack_speed_multiplier();
        cd.max(0.08)
    }

    pub fn compute_range(&self, def: &WeaponDef, stats: &PlayerStats) -> f32 {
        let scale = match def.kind {
            WeaponKind::Ranged => 1.0,
            WeaponKind::MeleeSweep | WeaponKind::MeleeThrust => 0.45,
        };
        (def.range + stats.range_bonus * scale).max(1.5)
    }
}

#[derive(Debug, Clone)]
pub struct ActiveEnemy {
    pub uid: u32,
    pub def_index: usize,
    pub behavior: EnemyBehavior,
    pub entity: EntityHandle,
    pub hp_bg_entity: EntityHandle,
    pub hp_fill_entity: EntityHandle,
    pub pos: [f32; 2],
    pub knockback_vel: [f32; 2],
    pub hp: f32,
    pub max_hp: f32,
    pub speed: f32,
    pub radius: f32,
    pub weight: f32,
    pub contact_damage: f32,
    pub hit_flash: f32,
    pub action_timer: f32,
    pub telegraph_timer: f32,
    pub dash_dir: [f32; 2],
    pub dash_timer: f32,
    pub is_elite: bool,
}

#[derive(Debug, Clone)]
pub struct SpawnWarning {
    pub pos: [f32; 2],
    pub timer: f32,
    pub def_index: usize,
    pub entity: EntityHandle,
}

#[derive(Debug, Clone)]
pub struct ActiveProjectile {
    pub entity: EntityHandle,
    pub pos: [f32; 2],
    pub vel: [f32; 2],
    pub radius: f32,
    pub damage: f32,
    pub knockback: f32,
    pub crit_chance: f32,
    pub crit_mult: f32,
    pub pierce_left: u32,
    pub life: f32,
    pub from_enemy: bool,
    pub hit_enemy_uids: Vec<u32>,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum PickupKind {
    XpSmall(u32),
    XpLarge(u32),
    Gold(u32),
    Heal(f32),
}

#[derive(Debug, Clone)]
pub struct ActivePickup {
    pub entity: EntityHandle,
    pub kind: PickupKind,
    pub pos: [f32; 2],
    pub magnetized: bool,
    pub speed: f32,
}

#[derive(Debug, Clone)]
pub struct DamageNumber {
    pub world_pos: [f32; 2],
    pub vel_y: f32,
    pub age: f32,
    pub duration: f32,
    pub amount: u32,
    pub is_crit: bool,
    pub is_player_hit: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ShopOfferKind {
    Weapon { weapon_id: String, tier: u8 },
    Item { item_id: String },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ShopOffer {
    pub kind: ShopOfferKind,
    pub price: u32,
    pub locked: bool,
    pub purchased: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GamePhase {
    WaveCombat,
    LevelUpChoice {
        choices: Vec<String>,
        pending_levels: u32,
    },
    WaveClearSweep {
        timer_tenths: u32,
    },
    ShopPhase {
        offers: Vec<ShopOffer>,
        reroll_cost: u32,
        reroll_count: u32,
    },
    Victory,
    GameOver,
}

pub struct SurvivorsState {
    pub config: GameConfig,
    pub phase: GamePhase,
    pub rng: XorShift64,
    pub pool: EntityPool,
    pub spatial_grid: SpatialGrid2D,
    pub crowd_buf: Vec<CrowdAgent2D>,
    pub query_scratch: Vec<u32>,

    pub player_entity: EntityHandle,
    pub player_hp_bg: EntityHandle,
    pub player_hp_fill: EntityHandle,
    pub player_pos: [f32; 2],
    pub player_facing_left: bool,
    pub player_is_moving: bool,
    pub player_i_frames: f32,
    pub player_hit_flash: f32,
    pub player_regen_accum: f32,

    pub camera_pos: [f32; 2],
    pub ortho_half_height: f32,

    pub stats: PlayerStats,
    pub weapons: Vec<EquippedWeapon>,
    pub owned_items: Vec<String>,

    pub enemies: Vec<ActiveEnemy>,
    pub spawn_warnings: Vec<SpawnWarning>,
    pub projectiles: Vec<ActiveProjectile>,
    pub pickups: Vec<ActivePickup>,
    pub damage_numbers: Vec<DamageNumber>,
    pub hit_burst_entities: Vec<EntityHandle>,
    pub hit_burst_cursor: usize,

    pub wave_index: u32,
    pub wave_timer: f32,
    pub wave_sweep_timer: f32,
    pub spawn_timer: f32,
    pub boss_spawned: bool,
    pub next_enemy_uid: u32,
    pub pending_level_ups: u32,

    pub world_built: bool,
    pub status_banner: Option<(String, f32)>,
}

impl SurvivorsState {
    pub fn new(config: GameConfig) -> Self {
        let first_wave_dur = config.wave_for_number(1).duration_seconds;
        Self {
            config,
            phase: GamePhase::WaveCombat,
            rng: XorShift64::new(0x5A17_2026_1124_0001),
            pool: EntityPool::new(),
            spatial_grid: SpatialGrid2D::new(2.0),
            crowd_buf: Vec::with_capacity(256),
            query_scratch: Vec::with_capacity(64),
            player_entity: EntityHandle::PLACEHOLDER,
            player_hp_bg: EntityHandle::PLACEHOLDER,
            player_hp_fill: EntityHandle::PLACEHOLDER,
            player_pos: [0.0, 0.0],
            player_facing_left: false,
            player_is_moving: false,
            player_i_frames: 0.0,
            player_hit_flash: 0.0,
            player_regen_accum: 0.0,
            camera_pos: [0.0, 0.0],
            ortho_half_height: 11.0,
            stats: PlayerStats::default(),
            weapons: vec![
                EquippedWeapon::new("pistol", 1),
                EquippedWeapon::new("rusty_blade", 1),
            ],
            owned_items: Vec::new(),
            enemies: Vec::new(),
            spawn_warnings: Vec::new(),
            projectiles: Vec::new(),
            pickups: Vec::new(),
            damage_numbers: Vec::new(),
            hit_burst_entities: Vec::new(),
            hit_burst_cursor: 0,
            wave_index: 1,
            wave_timer: first_wave_dur,
            wave_sweep_timer: 0.0,
            spawn_timer: 0.35,
            boss_spawned: false,
            next_enemy_uid: 1,
            pending_level_ups: 0,
            world_built: false,
            status_banner: Some(("WAVE 1 - SURVIVE!".into(), 2.2)),
        }
    }

    pub fn set_banner(&mut self, text: impl Into<String>, duration: f32) {
        self.status_banner = Some((text.into(), duration));
    }

    pub fn roll_level_up_choices(&mut self, count: usize) -> Vec<String> {
        let mut available: Vec<String> = self
            .config
            .level_upgrades
            .iter()
            .map(|u| u.id.clone())
            .collect();
        let mut out = Vec::new();
        while out.len() < count && !available.is_empty() {
            let idx = self.rng.range_usize(available.len());
            out.push(available.swap_remove(idx));
        }
        out
    }

    pub fn enter_level_up_if_pending(&mut self, world: &mut GameWorld<'_>) -> bool {
        if self.pending_level_ups == 0 {
            return false;
        }
        self.pending_level_ups -= 1;
        let choices = self.roll_level_up_choices(4);
        self.phase = GamePhase::LevelUpChoice {
            choices,
            pending_levels: self.pending_level_ups,
        };
        world.play_sound("audio/level_up.wav", 0.55, false);
        true
    }

    pub fn select_level_up_choice(&mut self, choice_idx: usize, world: &mut GameWorld<'_>) -> bool {
        let selected_id = match &self.phase {
            GamePhase::LevelUpChoice { choices, .. } => choices.get(choice_idx).cloned(),
            _ => None,
        };
        let Some(upgrade_id) = selected_id else {
            return false;
        };
        if let Some(upg) = self.config.upgrade_by_id(&upgrade_id).cloned() {
            self.stats.apply_modifiers(&upg.modifiers);
            self.set_banner(format!("UPGRADE: {}", upg.name), 1.6);
        }
        world.play_sound("audio/shop_buy.wav", 0.45, false);

        if self.pending_level_ups > 0 {
            self.enter_level_up_if_pending(world);
        } else if self.wave_timer <= 0.0 {
            self.open_shop(world);
        } else {
            self.phase = GamePhase::WaveCombat;
        }
        true
    }

    pub fn roll_shop_offers(&mut self, existing: &[ShopOffer]) -> Vec<ShopOffer> {
        let mut offers = Vec::with_capacity(4);
        for slot in 0..4 {
            if let Some(prev) = existing.get(slot) {
                if prev.locked && !prev.purchased {
                    offers.push(prev.clone());
                    continue;
                }
            }
            let wave_tier_bonus = ((self.wave_index.saturating_sub(1)) / 3) as u8;
            let luck_roll = self.rng.next_f32() + (self.stats.luck / 200.0).clamp(0.0, 0.25);
            let is_weapon = slot < 2 || luck_roll < 0.52;
            if is_weapon {
                let w_idx = self.rng.range_usize(self.config.weapons.len());
                let def = &self.config.weapons[w_idx];
                let tier = if luck_roll > 0.92 && wave_tier_bonus >= 1 {
                    3
                } else if luck_roll > 0.72 || wave_tier_bonus >= 1 {
                    2
                } else {
                    1
                }
                .min(3);
                offers.push(ShopOffer {
                    kind: ShopOfferKind::Weapon {
                        weapon_id: def.id.clone(),
                        tier,
                    },
                    price: def.price_for_tier(tier),
                    locked: false,
                    purchased: false,
                });
            } else {
                let i_idx = self.rng.range_usize(self.config.shop_items.len());
                let item = &self.config.shop_items[i_idx];
                let scaled_price = item.price + self.wave_index * 2;
                offers.push(ShopOffer {
                    kind: ShopOfferKind::Item {
                        item_id: item.id.clone(),
                    },
                    price: scaled_price,
                    locked: false,
                    purchased: false,
                });
            }
        }
        offers
    }

    pub fn open_shop(&mut self, world: &mut GameWorld<'_>) {
        let offers = self.roll_shop_offers(&[]);
        let reroll_cost = 4 + self.wave_index * 2;
        self.phase = GamePhase::ShopPhase {
            offers,
            reroll_cost,
            reroll_count: 0,
        };
        world.play_sound("audio/wave_clear.wav", 0.55, false);
    }

    pub fn try_toggle_shop_lock(&mut self, slot: usize) -> bool {
        let GamePhase::ShopPhase { offers, .. } = &mut self.phase else {
            return false;
        };
        let Some(offer) = offers.get_mut(slot) else {
            return false;
        };
        if offer.purchased {
            return false;
        }
        offer.locked = !offer.locked;
        true
    }

    pub fn try_reroll_shop(&mut self, world: &mut GameWorld<'_>) -> bool {
        let (prev_offers, cost, reroll_count) = match &self.phase {
            GamePhase::ShopPhase {
                offers,
                reroll_cost,
                reroll_count,
            } => (offers.clone(), *reroll_cost, *reroll_count),
            _ => return false,
        };
        if self.stats.gold < cost {
            self.set_banner("NOT ENOUGH GOLD!", 1.1);
            return false;
        }
        self.stats.gold -= cost;
        let next_offers = self.roll_shop_offers(&prev_offers);
        let next_count = reroll_count + 1;
        let next_cost = cost + 2 + next_count;
        self.phase = GamePhase::ShopPhase {
            offers: next_offers,
            reroll_cost: next_cost,
            reroll_count: next_count,
        };
        world.play_sound("audio/shop_reroll.wav", 0.45, false);
        true
    }

    pub fn try_buy_shop_offer(&mut self, slot: usize, world: &mut GameWorld<'_>) -> bool {
        let (offer_kind, price) = match &self.phase {
            GamePhase::ShopPhase { offers, .. } => {
                let Some(offer) = offers.get(slot) else {
                    return false;
                };
                if offer.purchased {
                    return false;
                }
                (offer.kind.clone(), offer.price)
            }
            _ => return false,
        };
        if self.stats.gold < price {
            self.set_banner("NOT ENOUGH GOLD!", 1.1);
            return false;
        }

        match offer_kind {
            ShopOfferKind::Weapon { weapon_id, tier } => {
                let merge_idx = if tier < 4 {
                    self.weapons
                        .iter()
                        .position(|w| w.weapon_id == weapon_id && w.tier == tier)
                } else {
                    None
                };
                if self.weapons.len() >= MAX_WEAPONS {
                    if let Some(idx) = merge_idx {
                        self.stats.gold -= price;
                        self.weapons[idx].tier += 1;
                        self.set_banner(format!("MERGED TO TIER {}!", self.weapons[idx].tier), 1.5);
                    } else {
                        self.set_banner("WEAPON SLOTS FULL (MAX 6)!", 1.2);
                        return false;
                    }
                } else {
                    self.stats.gold -= price;
                    self.weapons.push(EquippedWeapon::new(weapon_id, tier));
                }
            }
            ShopOfferKind::Item { item_id } => {
                let Some(item) = self.config.shop_item_by_id(&item_id).cloned() else {
                    return false;
                };
                self.stats.gold -= price;
                self.stats.apply_modifiers(&item.modifiers);
                self.owned_items.push(item.id);
                self.set_banner(format!("BOUGHT {}", item.name), 1.3);
            }
        }

        if let GamePhase::ShopPhase { offers, .. } = &mut self.phase {
            if let Some(offer) = offers.get_mut(slot) {
                offer.purchased = true;
                offer.locked = false;
            }
        }
        world.play_sound("audio/shop_buy.wav", 0.5, false);
        true
    }

    /// Merge two identical `(weapon_id, tier)` weapons into one `tier + 1` weapon.
    pub fn try_combine_weapons(&mut self, world: &mut GameWorld<'_>) -> bool {
        if !matches!(self.phase, GamePhase::ShopPhase { .. }) {
            return false;
        }
        for i in 0..self.weapons.len() {
            if self.weapons[i].tier >= 4 {
                continue;
            }
            for j in (i + 1)..self.weapons.len() {
                if self.weapons[i].weapon_id == self.weapons[j].weapon_id
                    && self.weapons[i].tier == self.weapons[j].tier
                {
                    let removed = self.weapons.remove(j);
                    if removed.entity != EntityHandle::PLACEHOLDER {
                        self.pool.release(world, POOL_WEAPON, removed.entity);
                    }
                    if removed.fx_entity != EntityHandle::PLACEHOLDER {
                        self.pool.release(world, POOL_MELEE_FX, removed.fx_entity);
                    }
                    self.weapons[i].tier += 1;
                    let name = self
                        .config
                        .weapon_by_id(&self.weapons[i].weapon_id)
                        .map_or("Weapon", |d| d.name.as_str());
                    self.set_banner(
                        format!("SYNTHESIZED {} T{}!", name, self.weapons[i].tier),
                        1.6,
                    );
                    world.play_sound("audio/level_up.wav", 0.5, false);
                    return true;
                }
            }
        }
        false
    }

    pub fn has_combinable_weapons(&self) -> bool {
        for i in 0..self.weapons.len() {
            if self.weapons[i].tier >= 4 {
                continue;
            }
            for j in (i + 1)..self.weapons.len() {
                if self.weapons[i].weapon_id == self.weapons[j].weapon_id
                    && self.weapons[i].tier == self.weapons[j].tier
                {
                    return true;
                }
            }
        }
        false
    }

    pub fn try_sell_weapon(&mut self, slot: usize, world: &mut GameWorld<'_>) -> bool {
        if !matches!(self.phase, GamePhase::ShopPhase { .. }) || self.weapons.len() <= 1 {
            return false;
        }
        if slot >= self.weapons.len() {
            return false;
        }
        let removed = self.weapons.remove(slot);
        if removed.entity != EntityHandle::PLACEHOLDER {
            self.pool.release(world, POOL_WEAPON, removed.entity);
        }
        if removed.fx_entity != EntityHandle::PLACEHOLDER {
            self.pool.release(world, POOL_MELEE_FX, removed.fx_entity);
        }
        let refund = self
            .config
            .weapon_by_id(&removed.weapon_id)
            .map_or(5, |d| d.sell_value_for_tier(removed.tier));
        self.stats.gold += refund;
        self.set_banner(format!("SOLD FOR +{refund}G"), 1.2);
        world.play_sound("audio/pickup_coin.wav", 0.45, false);
        true
    }

    pub fn start_next_wave(&mut self, world: &mut GameWorld<'_>) -> bool {
        if !matches!(self.phase, GamePhase::ShopPhase { .. }) {
            return false;
        }
        if self.wave_index >= self.config.total_waves() {
            self.phase = GamePhase::Victory;
            return true;
        }
        self.wave_index += 1;
        self.wave_timer = self
            .config
            .wave_for_number(self.wave_index)
            .duration_seconds;
        self.spawn_timer = 0.25;
        self.boss_spawned = false;
        self.stats.hp = self.stats.max_hp;
        self.phase = GamePhase::WaveCombat;
        self.set_banner(format!("WAVE {} - FIGHT!", self.wave_index), 2.0);
        world.play_sound("audio/wave_start.wav", 0.55, false);
        true
    }

    pub fn reset_run(&mut self, world: &mut GameWorld<'_>) {
        for enemy in self.enemies.drain(..) {
            self.pool.release(world, POOL_ENEMY, enemy.entity);
            self.pool.release(world, POOL_WHITE_BAR, enemy.hp_bg_entity);
            self.pool
                .release(world, POOL_WHITE_BAR, enemy.hp_fill_entity);
        }
        for warn in self.spawn_warnings.drain(..) {
            self.pool.release(world, POOL_WARNING, warn.entity);
        }
        for proj in self.projectiles.drain(..) {
            self.pool.release(world, POOL_PROJECTILE, proj.entity);
        }
        for pickup in self.pickups.drain(..) {
            self.pool.release(world, POOL_PICKUP, pickup.entity);
        }
        for weapon in self.weapons.drain(..) {
            if weapon.entity != EntityHandle::PLACEHOLDER {
                self.pool.release(world, POOL_WEAPON, weapon.entity);
            }
            if weapon.fx_entity != EntityHandle::PLACEHOLDER {
                self.pool.release(world, POOL_MELEE_FX, weapon.fx_entity);
            }
        }
        self.damage_numbers.clear();
        self.owned_items.clear();
        self.stats = PlayerStats::default();
        self.player_pos = [0.0, 0.0];
        self.player_i_frames = 0.0;
        self.player_hit_flash = 0.0;
        self.player_regen_accum = 0.0;
        self.pending_level_ups = 0;
        self.wave_index = 1;
        self.wave_timer = self.config.wave_for_number(1).duration_seconds;
        self.spawn_timer = 0.35;
        self.boss_spawned = false;
        self.weapons = vec![
            EquippedWeapon::new("pistol", 1),
            EquippedWeapon::new("rusty_blade", 1),
        ];
        self.phase = GamePhase::WaveCombat;
        self.set_banner("WAVE 1 - SURVIVE!", 2.0);
        world.play_sound("audio/wave_start.wav", 0.55, false);
    }
}
