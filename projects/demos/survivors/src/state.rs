//! Core game state machine, player stats, weapon inventory, shop mechanics, and pooling buckets.

use prism_game_api::physics2d::{CrowdAgent2D, SpatialGrid2D};
use prism_game_api::{EntityHandle, EntityPool, GameWorld};

use crate::config::{EnemyBehavior, GameConfig, StatModifiers, WeaponDef, WeaponKind};
use crate::i18n::{tr, Language};

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

pub const SETTINGS_SAVE_KEY: &str = "survivors.settings";
pub const RECORDS_SAVE_KEY: &str = "survivors.records";
pub const API_PROBE_SAVE_KEY: &str = "survivors.api_probe";
pub const SPLASH_DURATION: f32 = 2.4;

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

/// Active modal overlay shown atop either the main menu or the pause screen.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MenuModal {
    None,
    Settings,
    ApiDiagnostics,
}

/// Selectable starter weapon pairs on the main menu.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum StarterLoadout {
    #[default]
    Balanced,
    Gunslinger,
    Arcanist,
    Bruiser,
}

impl StarterLoadout {
    pub const ALL: [Self; 4] = [
        Self::Balanced,
        Self::Gunslinger,
        Self::Arcanist,
        Self::Bruiser,
    ];

    pub fn label(self) -> &'static str {
        self.label_for(Language::EnUs)
    }

    pub fn label_for(self, lang: Language) -> &'static str {
        match self {
            Self::Balanced => tr(lang, "均衡型", "BALANCED"),
            Self::Gunslinger => tr(lang, "枪手型", "GUNSLINGER"),
            Self::Arcanist => tr(lang, "奥术型", "ARCANIST"),
            Self::Bruiser => tr(lang, "重装型", "BRUISER"),
        }
    }

    pub fn subtitle(self) -> &'static str {
        self.subtitle_for(Language::EnUs)
    }

    pub fn subtitle_for(self, lang: Language) -> &'static str {
        match self {
            Self::Balanced => tr(lang, "手枪 + 锈蚀短刀", "Pistol + Rusty Blade"),
            Self::Gunslinger => tr(lang, "手枪 + 速射冲锋枪", "Pistol + Rapid SMG"),
            Self::Arcanist => tr(lang, "奥术法杖 + 骑士长枪", "Arcane Wand + Lance Spear"),
            Self::Bruiser => tr(lang, "霰弹枪 + 锈蚀短刀", "Shotgun + Rusty Blade"),
        }
    }

    pub fn weapon_ids(self) -> [&'static str; 2] {
        match self {
            Self::Balanced => ["pistol", "rusty_blade"],
            Self::Gunslinger => ["pistol", "smg"],
            Self::Arcanist => ["arcane_wand", "spear"],
            Self::Bruiser => ["shotgun", "rusty_blade"],
        }
    }

    pub fn next(self) -> Self {
        match self {
            Self::Balanced => Self::Gunslinger,
            Self::Gunslinger => Self::Arcanist,
            Self::Arcanist => Self::Bruiser,
            Self::Bruiser => Self::Balanced,
        }
    }

    pub fn prev(self) -> Self {
        match self {
            Self::Balanced => Self::Bruiser,
            Self::Gunslinger => Self::Balanced,
            Self::Arcanist => Self::Gunslinger,
            Self::Bruiser => Self::Arcanist,
        }
    }
}

/// User-configurable preferences persisted via `GameWorld::save_data`.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct PersistentSettings {
    #[serde(default)]
    pub language: Language,
    pub sfx_volume: f32,
    pub muted: bool,
    pub show_damage_numbers: bool,
    pub show_api_telemetry: bool,
}

impl Default for PersistentSettings {
    fn default() -> Self {
        Self {
            language: Language::default(),
            sfx_volume: 0.7,
            muted: false,
            show_damage_numbers: true,
            show_api_telemetry: false,
        }
    }
}

impl PersistentSettings {
    pub fn effective_sfx_volume(&self) -> f32 {
        if self.muted {
            0.0
        } else {
            self.sfx_volume.clamp(0.0, 1.0)
        }
    }

    pub fn load(world: &GameWorld<'_>) -> Self {
        world
            .load_data(SETTINGS_SAVE_KEY)
            .and_then(|raw| serde_json::from_str::<Self>(&raw).ok())
            .unwrap_or_default()
    }

    pub fn save(&self, world: &mut GameWorld<'_>) -> bool {
        serde_json::to_string(self)
            .map(|json| world.save_data(SETTINGS_SAVE_KEY, &json))
            .unwrap_or(false)
    }
}

/// Lifetime run statistics persisted via `GameWorld::save_data`.
#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct CareerRecords {
    pub runs_played: u32,
    pub victories: u32,
    pub best_wave: u32,
    pub best_kills: u32,
    pub best_level: u32,
    pub total_kills: u64,
}

impl CareerRecords {
    pub fn load(world: &GameWorld<'_>) -> Self {
        world
            .load_data(RECORDS_SAVE_KEY)
            .and_then(|raw| serde_json::from_str::<Self>(&raw).ok())
            .unwrap_or_default()
    }

    pub fn save(&self, world: &mut GameWorld<'_>) -> bool {
        serde_json::to_string(self)
            .map(|json| world.save_data(RECORDS_SAVE_KEY, &json))
            .unwrap_or(false)
    }
}

/// Result of a single engine API verification probe.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ApiCheckEntry {
    pub name: &'static str,
    pub passed: bool,
    pub detail: String,
}

/// Aggregated report produced by `SurvivorsState::run_api_diagnostics`.
#[derive(Debug, Clone, Default)]
pub struct ApiDiagnosticReport {
    pub entries: Vec<ApiCheckEntry>,
    pub passed_count: usize,
    pub total_count: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GamePhase {
    Splash,
    MainMenu,
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
    Paused {
        from_sweep_tenths: Option<u32>,
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

    pub splash_elapsed: f32,
    pub splash_sound_id: Option<u32>,
    pub splash_sound_played: bool,
    pub menu_cursor: usize,
    pub pause_cursor: usize,
    pub settings_cursor: usize,
    pub modal: MenuModal,
    pub selected_loadout: StarterLoadout,
    pub settings: PersistentSettings,
    pub records: CareerRecords,
    pub persistence_loaded: bool,
    pub run_recorded: bool,
    pub last_scene_generation: u64,
    pub api_report: Option<ApiDiagnosticReport>,
    pub test_sound_id: Option<u32>,
    pub last_dt: f32,
}

impl SurvivorsState {
    pub fn new(config: GameConfig) -> Self {
        let first_wave_dur = config.wave_for_number(1).duration_seconds;
        let default_loadout = StarterLoadout::default();
        let [w0, w1] = default_loadout.weapon_ids();
        Self {
            config,
            phase: GamePhase::Splash,
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
            weapons: vec![EquippedWeapon::new(w0, 1), EquippedWeapon::new(w1, 1)],
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
            status_banner: None,
            splash_elapsed: 0.0,
            splash_sound_id: None,
            splash_sound_played: false,
            menu_cursor: 0,
            pause_cursor: 0,
            settings_cursor: 0,
            modal: MenuModal::None,
            selected_loadout: default_loadout,
            settings: PersistentSettings::default(),
            records: CareerRecords::default(),
            persistence_loaded: false,
            run_recorded: false,
            last_scene_generation: 0,
            api_report: None,
            test_sound_id: None,
            last_dt: 1.0 / 60.0,
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
        self.play_sfx(world, "audio/level_up.wav", 0.55, false);
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
        self.play_sfx(world, "audio/shop_buy.wav", 0.45, false);

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
        self.play_sfx(world, "audio/wave_clear.wav", 0.55, false);
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
        self.play_sfx(world, "audio/shop_reroll.wav", 0.45, false);
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
        self.play_sfx(world, "audio/shop_buy.wav", 0.5, false);
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
                    self.play_sfx(world, "audio/level_up.wav", 0.5, false);
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
        self.play_sfx(world, "audio/pickup_coin.wav", 0.45, false);
        true
    }

    pub fn start_next_wave(&mut self, world: &mut GameWorld<'_>) -> bool {
        if !matches!(self.phase, GamePhase::ShopPhase { .. }) {
            return false;
        }
        if self.wave_index >= self.config.total_waves() {
            self.phase = GamePhase::Victory;
            self.record_run_end(true, world);
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
        let lang = self.settings.language;
        let banner = if lang.is_zh() {
            format!("第 {} 波 - 迎战！", self.wave_index)
        } else {
            format!("WAVE {} - FIGHT!", self.wave_index)
        };
        self.set_banner(banner, 2.0);
        self.play_sfx(world, "audio/wave_start.wav", 0.55, false);
        true
    }

    pub fn toggle_language(&mut self, world: &mut GameWorld<'_>) {
        self.settings.language = self.settings.language.toggle();
        self.settings.save(world);
    }

    pub fn play_sfx(
        &self,
        world: &mut GameWorld<'_>,
        path: &str,
        base_volume: f32,
        repeat: bool,
    ) -> Option<u32> {
        let vol = (base_volume * self.settings.effective_sfx_volume()).clamp(0.0, 1.0);
        if vol <= 0.001 {
            return None;
        }
        world.play_sound(path, vol, repeat)
    }

    pub fn stop_tracked_sounds(&mut self, world: &mut GameWorld<'_>) {
        if let Some(id) = self.splash_sound_id.take() {
            world.stop_sound(id);
        }
        if let Some(id) = self.test_sound_id.take() {
            world.stop_sound(id);
        }
    }

    pub fn sync_persistence_and_generation(&mut self, world: &mut GameWorld<'_>) {
        if !self.persistence_loaded {
            self.settings = PersistentSettings::load(world);
            self.records = CareerRecords::load(world);
            self.persistence_loaded = true;
        }
        let gen = world.scene_generation();
        if gen != self.last_scene_generation {
            self.last_scene_generation = gen;
            self.world_built = false;
        }
    }

    pub fn clear_combat_entities(&mut self, world: &mut GameWorld<'_>) {
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
    }

    pub fn enter_main_menu(&mut self, world: &mut GameWorld<'_>) {
        self.stop_tracked_sounds(world);
        self.clear_combat_entities(world);
        self.modal = MenuModal::None;
        self.menu_cursor = 0;
        self.pause_cursor = 0;
        self.settings_cursor = 0;
        self.status_banner = None;
        self.phase = GamePhase::MainMenu;
    }

    pub fn start_new_run(&mut self, world: &mut GameWorld<'_>) {
        self.stop_tracked_sounds(world);
        self.modal = MenuModal::None;
        self.reset_run(world);
    }

    pub fn record_run_end(&mut self, victory: bool, world: &mut GameWorld<'_>) {
        if self.run_recorded {
            return;
        }
        self.run_recorded = true;
        self.records.runs_played = self.records.runs_played.saturating_add(1);
        if victory {
            self.records.victories = self.records.victories.saturating_add(1);
        }
        self.records.best_wave = self.records.best_wave.max(self.wave_index);
        self.records.best_kills = self.records.best_kills.max(self.stats.kills);
        self.records.best_level = self.records.best_level.max(self.stats.level);
        self.records.total_kills = self
            .records
            .total_kills
            .saturating_add(u64::from(self.stats.kills));
        self.records.save(world);
    }

    pub fn reset_records(&mut self, world: &mut GameWorld<'_>) {
        self.records = CareerRecords::default();
        self.records.save(world);
    }

    pub fn run_api_diagnostics(&mut self, world: &mut GameWorld<'_>) {
        let mut entries = Vec::new();

        let screen_from_api = world.screen_size();
        let screen_from_input = world.input().screen;
        entries.push(ApiCheckEntry {
            name: "screen_size + input",
            passed: screen_from_api[0] >= 0.0
                && screen_from_api[1] >= 0.0
                && (screen_from_api[0] - screen_from_input[0]).abs() < 1.0,
            detail: format!("{:.0}x{:.0}", screen_from_api[0], screen_from_api[1]),
        });

        let has_weapons = world.asset_exists("data/weapons.json");
        let has_atlas = world.asset_exists("sprites/atlas.png");
        let has_missing = world.asset_exists("data/__nonexistent_probe__.json");
        let text_ok = world.read_asset_text("data/weapons.json").is_ok();
        entries.push(ApiCheckEntry {
            name: "asset_exists + read_asset_text",
            passed: has_weapons && has_atlas && !has_missing && text_ok,
            detail: if has_weapons && has_atlas && !has_missing && text_ok {
                "assets verified".to_string()
            } else {
                format!("w:{has_weapons} a:{has_atlas} m:{has_missing} r:{text_ok}")
            },
        });

        let probe_val = format!("gen={};wave={}", world.scene_generation(), self.wave_index);
        let saved = world.save_data(API_PROBE_SAVE_KEY, &probe_val);
        let loaded = world.load_data(API_PROBE_SAVE_KEY);
        let save_ok = saved && loaded.as_deref() == Some(probe_val.as_str());
        entries.push(ApiCheckEntry {
            name: "save_data + load_data",
            passed: save_ok,
            detail: if save_ok {
                "round-trip OK".to_string()
            } else {
                format!("saved={saved} loaded={loaded:?}")
            },
        });

        let sound_id = world.play_sound("audio/shop_buy.wav", 0.0, false);
        if let Some(id) = sound_id {
            world.stop_sound(id);
        }
        entries.push(ApiCheckEntry {
            name: "play_sound + stop_sound",
            passed: true,
            detail: match sound_id {
                Some(id) => format!("id={id} stopped"),
                None => "no audio backend (headless)".to_string(),
            },
        });

        let entity_ok = if self.player_hp_bg != EntityHandle::PLACEHOLDER
            && world.is_alive(self.player_hp_bg)
        {
            let before = world.is_entity_active(self.player_hp_bg);
            let toggled = world.set_entity_active(self.player_hp_bg, !before);
            let during = world.is_entity_active(self.player_hp_bg);
            let restored = world.set_entity_active(self.player_hp_bg, before);
            let after = world.is_entity_active(self.player_hp_bg);
            toggled && restored && during == !before && after == before
        } else {
            false
        };
        entries.push(ApiCheckEntry {
            name: "set_entity_active + is_entity_active",
            passed: entity_ok,
            detail: if entity_ok {
                "toggle verified".to_string()
            } else {
                "target entity missing".to_string()
            },
        });

        let gen = world.scene_generation();
        world.set_camera_input_suppressed(true);
        entries.push(ApiCheckEntry {
            name: "scene_generation + camera_suppress",
            passed: gen == self.last_scene_generation,
            detail: format!("gen={gen}"),
        });

        let scene_saved = world.save_scene_state();
        entries.push(ApiCheckEntry {
            name: "save_scene_state",
            passed: scene_saved,
            detail: if scene_saved {
                "host saved".to_string()
            } else {
                "host unsupported".to_string()
            },
        });

        let passed_count = entries.iter().filter(|e| e.passed).count();
        let total_count = entries.len();
        self.api_report = Some(ApiDiagnosticReport {
            entries,
            passed_count,
            total_count,
        });
    }

    pub fn reset_run(&mut self, world: &mut GameWorld<'_>) {
        self.clear_combat_entities(world);
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
        self.run_recorded = false;
        let [w0, w1] = self.selected_loadout.weapon_ids();
        self.weapons = vec![EquippedWeapon::new(w0, 1), EquippedWeapon::new(w1, 1)];
        self.phase = GamePhase::WaveCombat;
        let banner = tr(
            self.settings.language,
            "第 1 波 - 生存下去！",
            "WAVE 1 - SURVIVE!",
        );
        self.set_banner(banner, 2.0);
        self.play_sfx(world, "audio/wave_start.wav", 0.55, false);
    }
}
