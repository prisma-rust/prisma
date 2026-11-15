//! Composable enemy behaviors and boss encounter state machine for EchoWell.

/// Composable enemy AI behavior.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum EnemyBehavior {
    Patrol {
        min_x: f32,
        max_x: f32,
        speed: f32,
        dir: f32,
    },
    Hover {
        anchor_y: f32,
        amplitude: f32,
        frequency: f32,
        phase: f32,
    },
    Chase {
        aggro_radius: f32,
        speed: f32,
    },
    TurretProjectile {
        fire_interval: f32,
        timer: f32,
    },
}

/// Runtime enemy state decoupled from the 2D collision kernel.
#[derive(Clone, Debug)]
pub struct EnemyEntity {
    pub id: String,
    pub position: [f32; 2],
    pub velocity: [f32; 2],
    pub health: i32,
    pub max_health: i32,
    pub hit_cooldown: f32,
    pub behavior: EnemyBehavior,
}

impl EnemyEntity {
    pub fn new(
        id: impl Into<String>,
        position: [f32; 2],
        health: i32,
        behavior: EnemyBehavior,
    ) -> Self {
        Self {
            id: id.into(),
            position,
            velocity: [0.0, 0.0],
            health,
            max_health: health,
            hit_cooldown: 0.0,
            behavior,
        }
    }

    pub fn is_alive(&self) -> bool {
        self.health > 0
    }

    /// Apply damage if not in hit cooldown; returns `true` if damage was applied.
    pub fn apply_damage(&mut self, amount: i32, cooldown_secs: f32) -> bool {
        if !self.is_alive() || self.hit_cooldown > 0.0 {
            return false;
        }
        self.health = (self.health - amount).max(0);
        self.hit_cooldown = cooldown_secs;
        true
    }

    /// Step enemy behavior by `dt` seconds; returns `true` if a projectile was fired.
    pub fn step(&mut self, dt: f32, player_pos: [f32; 2]) -> bool {
        if !self.is_alive() {
            return false;
        }
        self.hit_cooldown = (self.hit_cooldown - dt).max(0.0);
        let mut fired = false;
        match &mut self.behavior {
            EnemyBehavior::Patrol {
                min_x,
                max_x,
                speed,
                dir,
            } => {
                self.position[0] += *speed * *dir * dt;
                if self.position[0] <= *min_x {
                    self.position[0] = *min_x;
                    *dir = 1.0;
                } else if self.position[0] >= *max_x {
                    self.position[0] = *max_x;
                    *dir = -1.0;
                }
            }
            EnemyBehavior::Hover {
                anchor_y,
                amplitude,
                frequency,
                phase,
            } => {
                *phase += dt * *frequency;
                self.position[1] = *anchor_y + (*phase).sin() * *amplitude;
            }
            EnemyBehavior::Chase {
                aggro_radius,
                speed,
            } => {
                let dx = player_pos[0] - self.position[0];
                let dy = player_pos[1] - self.position[1];
                let dist = (dx * dx + dy * dy).sqrt();
                if dist <= *aggro_radius && dist > 1e-3 {
                    self.position[0] += (dx / dist) * *speed * dt;
                    self.position[1] += (dy / dist) * *speed * dt;
                }
            }
            EnemyBehavior::TurretProjectile {
                fire_interval,
                timer,
            } => {
                *timer += dt;
                if *timer >= *fire_interval {
                    *timer -= *fire_interval;
                    fired = true;
                }
            }
        }
        fired
    }
}

/// Boss encounter phases.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BossPhase {
    Idle,
    Telegraph,
    Phase1,
    Phase2,
    Defeated,
}

/// Boss encounter state machine with room lock and persistent defeat state.
#[derive(Clone, Debug)]
pub struct BossEncounter {
    pub id: String,
    pub phase: BossPhase,
    pub health: i32,
    pub max_health: i32,
    pub room_locked: bool,
    pub telegraph_timer: f32,
    pub persistent_defeated: bool,
}

impl BossEncounter {
    pub fn new(id: impl Into<String>, max_health: i32, persistent_defeated: bool) -> Self {
        Self {
            id: id.into(),
            phase: if persistent_defeated {
                BossPhase::Defeated
            } else {
                BossPhase::Idle
            },
            health: if persistent_defeated { 0 } else { max_health },
            max_health,
            room_locked: false,
            telegraph_timer: 0.0,
            persistent_defeated,
        }
    }

    pub fn trigger(&mut self) {
        if self.phase == BossPhase::Idle && !self.persistent_defeated {
            self.phase = BossPhase::Telegraph;
            self.telegraph_timer = 0.75;
            self.room_locked = true;
        }
    }

    pub fn step(&mut self, dt: f32) {
        if self.phase == BossPhase::Telegraph {
            self.telegraph_timer -= dt;
            if self.telegraph_timer <= 0.0 {
                self.phase = BossPhase::Phase1;
            }
        }
    }

    pub fn apply_damage(&mut self, amount: i32) {
        if !matches!(self.phase, BossPhase::Phase1 | BossPhase::Phase2) {
            return;
        }
        self.health = (self.health - amount).max(0);
        if self.health == 0 {
            self.phase = BossPhase::Defeated;
            self.room_locked = false;
            self.persistent_defeated = true;
        } else if self.health * 2 <= self.max_health {
            self.phase = BossPhase::Phase2;
        }
    }

    pub fn reset_on_player_death(&mut self) {
        if !self.persistent_defeated {
            self.phase = BossPhase::Idle;
            self.health = self.max_health;
            self.room_locked = false;
            self.telegraph_timer = 0.0;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn enemy_behaviors_and_hit_cooldown() {
        let mut patrol = EnemyEntity::new(
            "p1",
            [10.0, 0.0],
            2,
            EnemyBehavior::Patrol {
                min_x: 0.0,
                max_x: 20.0,
                speed: 10.0,
                dir: 1.0,
            },
        );
        patrol.step(1.5, [0.0, 0.0]);
        assert!((patrol.position[0] - 20.0).abs() < 1e-4);
        assert!(patrol.apply_damage(1, 0.5));
        assert!(!patrol.apply_damage(1, 0.5));
        patrol.step(0.6, [0.0, 0.0]);
        assert!(patrol.apply_damage(1, 0.5));
        assert!(!patrol.is_alive());
    }

    #[test]
    fn boss_encounter_lifecycle_and_reset() {
        let mut boss = BossEncounter::new("sanctum-warden", 10, false);
        boss.trigger();
        assert!(boss.room_locked);
        assert_eq!(boss.phase, BossPhase::Telegraph);
        boss.step(1.0);
        assert_eq!(boss.phase, BossPhase::Phase1);
        boss.apply_damage(5);
        assert_eq!(boss.phase, BossPhase::Phase2);
        boss.reset_on_player_death();
        assert_eq!(boss.phase, BossPhase::Idle);
        assert!(!boss.room_locked);

        boss.trigger();
        boss.step(1.0);
        boss.apply_damage(10);
        assert_eq!(boss.phase, BossPhase::Defeated);
        assert!(!boss.room_locked);
        assert!(boss.persistent_defeated);
    }
}
