//! In-game Render Settings UI (`F4`) for live switching of upscaling modes,
//! render scale, and sharpening in Emergence.

use prism_game_api::{keys, EntityHandle, GameWorld, InputSnapshot};
use prism_scene::ui::{
    Anchors, Canvas, CanvasScaleMode, Margin, Node, Pivot, Style, Text, TextAlign,
};
use prism_scene::{Active, Name};

use crate::commands::{
    current_dlfg_config, current_dlss_pre_upscale, current_upscaler_config, dlfg_frames_label,
    dlfg_mode_label, dlss_pre_upscale_label, set_dlfg_config, set_dlss_pre_upscale,
    set_upscaler_config, upscaler_mode_label,
};

#[derive(Clone, Copy)]
struct ModeOption {
    id: u8,
    label: &'static str,
}

#[cfg(not(target_os = "android"))]
const MODE_OPTIONS: &[ModeOption] = &[
    ModeOption {
        id: 0,
        label: "Off",
    },
    ModeOption {
        id: 3,
        label: "Nearest",
    },
    ModeOption {
        id: 4,
        label: "Linear",
    },
    ModeOption {
        id: 1,
        label: "SGSR 2",
    },
    ModeOption {
        id: 2,
        label: "Arm ASR",
    },
    ModeOption {
        id: 5,
        label: "DLSS",
    },
    ModeOption {
        id: 6,
        label: "FSR*",
    },
];

#[cfg(target_os = "android")]
const MODE_OPTIONS: &[ModeOption] = &[
    ModeOption {
        id: 0,
        label: "Off",
    },
    ModeOption {
        id: 3,
        label: "Nearest",
    },
    ModeOption {
        id: 4,
        label: "Linear",
    },
    ModeOption {
        id: 1,
        label: "SGSR 2",
    },
    ModeOption {
        id: 2,
        label: "Arm ASR",
    },
];

const SCALE_PRESETS: &[f32] = &[0.50, 0.67, 0.75, 0.85, 1.00];
const SHARP_PRESETS: &[f32] = &[0.00, 0.25, 0.50, 0.75, 1.00];
const PRE_UPSCALE_LABELS: &[&str] = &["Linear", "Nearest", "Off"];
const DLFG_MODE_LABELS: &[&str] = &["Off", "On", "Auto"];
const DLFG_FRAMES_LABELS: &[&str] = &["2x (1)", "3x (2)", "4x (3)"];

const PANEL_W: f32 = 520.0;
const PANEL_H: f32 = 446.0;
const PAD: f32 = 16.0;
const BTN_H: f32 = 26.0;
const BTN_GAP: f32 = 6.0;

#[derive(Clone, Copy)]
struct ButtonEntities {
    bg: EntityHandle,
    text: EntityHandle,
}

struct PanelEntities {
    backdrop: EntityHandle,
    title: EntityHandle,
    mode_label: EntityHandle,
    mode_btns: Vec<ButtonEntities>,
    scale_label: EntityHandle,
    scale_btns: Vec<ButtonEntities>,
    sharp_label: EntityHandle,
    sharp_btns: Vec<ButtonEntities>,
    pre_label: EntityHandle,
    pre_btns: Vec<ButtonEntities>,
    dlfg_label: EntityHandle,
    dlfg_btns: Vec<ButtonEntities>,
    fg_mult_label: EntityHandle,
    fg_mult_btns: Vec<ButtonEntities>,
    status: EntityHandle,
    hint: EntityHandle,
}

#[derive(Clone, Copy)]
struct UiRect {
    x: f32,
    y: f32,
    w: f32,
    h: f32,
}

impl UiRect {
    fn contains(self, p: [f32; 2]) -> bool {
        p[0] >= self.x && p[0] <= self.x + self.w && p[1] >= self.y && p[1] <= self.y + self.h
    }
}

/// Interactive Render Settings overlay (`F4`).
#[derive(Default)]
pub struct RenderSettingsUi {
    pub open: bool,
    selected_row: usize,
    prev_keys: u16,
    panel: Option<PanelEntities>,
}

impl RenderSettingsUi {
    fn panel_rect(screen: [f32; 2]) -> UiRect {
        let w = PANEL_W.min((screen[0] - 24.0).max(280.0));
        let h = PANEL_H;
        UiRect {
            x: (screen[0] - w) * 0.5,
            y: (screen[1] - h) * 0.5,
            w,
            h,
        }
    }

    fn row_button_rect(panel: UiRect, row_y: f32, idx: usize, count: usize) -> UiRect {
        let inner_w = panel.w - PAD * 2.0;
        let btn_w =
            ((inner_w - BTN_GAP * (count.saturating_sub(1) as f32)) / count as f32).max(28.0);
        UiRect {
            x: panel.x + PAD + idx as f32 * (btn_w + BTN_GAP),
            y: row_y,
            w: btn_w,
            h: BTN_H,
        }
    }

    /// Handle `F4` toggle, keyboard adjustment, and mouse button clicks.
    /// Returns `true` when the panel is open and consumes UI clicks.
    pub fn handle_input(
        &mut self,
        input: &InputSnapshot,
        screen: [f32; 2],
        console_active: bool,
    ) -> bool {
        if console_active {
            return false;
        }

        let mut bits = 0u16;
        if input.key_pressed(keys::F4) {
            bits |= 1 << 0;
        }
        if input.key_pressed(keys::ARROW_UP) {
            bits |= 1 << 1;
        }
        if input.key_pressed(keys::ARROW_DOWN) {
            bits |= 1 << 2;
        }
        if input.key_pressed(keys::ARROW_LEFT) {
            bits |= 1 << 3;
        }
        if input.key_pressed(keys::ARROW_RIGHT) {
            bits |= 1 << 4;
        }
        if input.key_pressed(keys::ESCAPE) {
            bits |= 1 << 5;
        }
        let pressed = bits & !self.prev_keys;
        self.prev_keys = bits;

        if (pressed & (1 << 0)) != 0 {
            self.open = !self.open;
        }
        if !self.open {
            return false;
        }
        if (pressed & (1 << 5)) != 0 {
            self.open = false;
            return true;
        }

        let (mut mode_id, mut scale, mut sharp) = current_upscaler_config();
        let (mut dlfg_mode, mut dlfg_frames) = current_dlfg_config();
        let mut changed = false;

        if (pressed & (1 << 1)) != 0 {
            self.selected_row = (self.selected_row + 5) % 6;
        }
        if (pressed & (1 << 2)) != 0 {
            self.selected_row = (self.selected_row + 1) % 6;
        }
        let step_dir = if (pressed & (1 << 4)) != 0 {
            1i32
        } else if (pressed & (1 << 3)) != 0 {
            -1i32
        } else {
            0
        };
        if step_dir != 0 {
            match self.selected_row {
                0 => {
                    let cur_idx = MODE_OPTIONS
                        .iter()
                        .position(|m| m.id == mode_id)
                        .unwrap_or(0);
                    let len = MODE_OPTIONS.len();
                    let next_idx = if step_dir > 0 {
                        (cur_idx + 1) % len
                    } else {
                        (cur_idx + len - 1) % len
                    };
                    mode_id = MODE_OPTIONS[next_idx].id;
                    changed = true;
                }
                1 => {
                    scale = (scale + step_dir as f32 * 0.05).clamp(0.05, 1.0);
                    changed = true;
                }
                2 => {
                    sharp = (sharp + step_dir as f32 * 0.10).clamp(0.0, 1.0);
                    changed = true;
                }
                3 => {
                    let cur = current_dlss_pre_upscale();
                    let next = if step_dir > 0 {
                        (cur + 1) % 3
                    } else {
                        (cur + 2) % 3
                    };
                    set_dlss_pre_upscale(next);
                }
                4 => {
                    dlfg_mode = if step_dir > 0 {
                        (dlfg_mode + 1) % 3
                    } else {
                        (dlfg_mode + 2) % 3
                    };
                    set_dlfg_config(dlfg_mode, dlfg_frames);
                }
                _ => {
                    let idx = dlfg_frames.saturating_sub(1).min(2);
                    let next_idx = if step_dir > 0 {
                        (idx + 1) % 3
                    } else {
                        (idx + 2) % 3
                    };
                    dlfg_frames = next_idx + 1;
                    set_dlfg_config(dlfg_mode, dlfg_frames);
                }
            }
        }

        if !input.pointer_locked && input.left_clicked {
            let panel = Self::panel_rect(screen);
            if panel.contains(input.cursor) {
                let mode_y = panel.y + 68.0;
                for (i, opt) in MODE_OPTIONS.iter().enumerate() {
                    if Self::row_button_rect(panel, mode_y, i, MODE_OPTIONS.len())
                        .contains(input.cursor)
                    {
                        mode_id = opt.id;
                        self.selected_row = 0;
                        changed = true;
                    }
                }
                let scale_y = panel.y + 126.0;
                for (i, &val) in SCALE_PRESETS.iter().enumerate() {
                    if Self::row_button_rect(panel, scale_y, i, SCALE_PRESETS.len())
                        .contains(input.cursor)
                    {
                        scale = val;
                        self.selected_row = 1;
                        changed = true;
                    }
                }
                let sharp_y = panel.y + 184.0;
                for (i, &val) in SHARP_PRESETS.iter().enumerate() {
                    if Self::row_button_rect(panel, sharp_y, i, SHARP_PRESETS.len())
                        .contains(input.cursor)
                    {
                        sharp = val;
                        self.selected_row = 2;
                        changed = true;
                    }
                }
                let pre_y = panel.y + 242.0;
                for i in 0..PRE_UPSCALE_LABELS.len() {
                    if Self::row_button_rect(panel, pre_y, i, PRE_UPSCALE_LABELS.len())
                        .contains(input.cursor)
                    {
                        set_dlss_pre_upscale(i as u8);
                        self.selected_row = 3;
                    }
                }
                let dlfg_y = panel.y + 300.0;
                for i in 0..DLFG_MODE_LABELS.len() {
                    if Self::row_button_rect(panel, dlfg_y, i, DLFG_MODE_LABELS.len())
                        .contains(input.cursor)
                    {
                        dlfg_mode = i as u8;
                        set_dlfg_config(dlfg_mode, dlfg_frames);
                        self.selected_row = 4;
                    }
                }
                let fg_mult_y = panel.y + 358.0;
                for i in 0..DLFG_FRAMES_LABELS.len() {
                    if Self::row_button_rect(panel, fg_mult_y, i, DLFG_FRAMES_LABELS.len())
                        .contains(input.cursor)
                    {
                        dlfg_frames = (i as u8) + 1;
                        set_dlfg_config(dlfg_mode, dlfg_frames);
                        self.selected_row = 5;
                    }
                }
                if changed {
                    set_upscaler_config(mode_id, scale, sharp);
                }
                return true;
            }
        }

        if changed {
            set_upscaler_config(mode_id, scale, sharp);
        }
        false
    }

    fn ensure_entities(&mut self, world: &mut GameWorld<'_>) {
        if self.panel.is_some() {
            return;
        }
        let ui_canvas = Canvas {
            scale_mode: CanvasScaleMode::ConstantPixelSize,
            ..Canvas::default()
        };

        let spawn_child = |world: &mut GameWorld<'_>, name: &str| {
            let e = world.spawn();
            world.set(e, Name(name.to_string()));
            world.set(e, ui_canvas.clone());
            world.set(e, Node::default());
            world.set(e, Active(false));
            e
        };

        let backdrop = spawn_child(world, "rs_backdrop");
        let title = spawn_child(world, "rs_title");
        let mode_label = spawn_child(world, "rs_mode_label");
        let mode_btns = (0..MODE_OPTIONS.len())
            .map(|i| ButtonEntities {
                bg: spawn_child(world, &format!("rs_mode_bg_{i}")),
                text: spawn_child(world, &format!("rs_mode_txt_{i}")),
            })
            .collect();
        let scale_label = spawn_child(world, "rs_scale_label");
        let scale_btns = (0..SCALE_PRESETS.len())
            .map(|i| ButtonEntities {
                bg: spawn_child(world, &format!("rs_scale_bg_{i}")),
                text: spawn_child(world, &format!("rs_scale_txt_{i}")),
            })
            .collect();
        let sharp_label = spawn_child(world, "rs_sharp_label");
        let sharp_btns = (0..SHARP_PRESETS.len())
            .map(|i| ButtonEntities {
                bg: spawn_child(world, &format!("rs_sharp_bg_{i}")),
                text: spawn_child(world, &format!("rs_sharp_txt_{i}")),
            })
            .collect();
        let pre_label = spawn_child(world, "rs_pre_label");
        let pre_btns = (0..PRE_UPSCALE_LABELS.len())
            .map(|i| ButtonEntities {
                bg: spawn_child(world, &format!("rs_pre_bg_{i}")),
                text: spawn_child(world, &format!("rs_pre_txt_{i}")),
            })
            .collect();
        let dlfg_label = spawn_child(world, "rs_dlfg_label");
        let dlfg_btns = (0..DLFG_MODE_LABELS.len())
            .map(|i| ButtonEntities {
                bg: spawn_child(world, &format!("rs_dlfg_bg_{i}")),
                text: spawn_child(world, &format!("rs_dlfg_txt_{i}")),
            })
            .collect();
        let fg_mult_label = spawn_child(world, "rs_fg_mult_label");
        let fg_mult_btns = (0..DLFG_FRAMES_LABELS.len())
            .map(|i| ButtonEntities {
                bg: spawn_child(world, &format!("rs_fg_mult_bg_{i}")),
                text: spawn_child(world, &format!("rs_fg_mult_txt_{i}")),
            })
            .collect();
        let status = spawn_child(world, "rs_status");
        let hint = spawn_child(world, "rs_hint");

        self.panel = Some(PanelEntities {
            backdrop,
            title,
            mode_label,
            mode_btns,
            scale_label,
            scale_btns,
            sharp_label,
            sharp_btns,
            pre_label,
            pre_btns,
            dlfg_label,
            dlfg_btns,
            fg_mult_label,
            fg_mult_btns,
            status,
            hint,
        });
    }

    /// Synchronize ECS UI nodes with current upscaler settings.
    pub fn sync_ui(&mut self, world: &mut GameWorld<'_>, console_active: bool) {
        self.ensure_entities(world);
        let Some(p) = self.panel.as_ref() else {
            return;
        };
        let visible = self.open && !console_active;
        for &e in [
            p.backdrop,
            p.title,
            p.mode_label,
            p.scale_label,
            p.sharp_label,
            p.pre_label,
            p.dlfg_label,
            p.fg_mult_label,
            p.status,
            p.hint,
        ]
        .iter()
        {
            world.set(e, Active(visible));
        }
        for b in p
            .mode_btns
            .iter()
            .chain(p.scale_btns.iter())
            .chain(p.sharp_btns.iter())
            .chain(p.pre_btns.iter())
            .chain(p.dlfg_btns.iter())
            .chain(p.fg_mult_btns.iter())
        {
            world.set(b.bg, Active(visible));
            world.set(b.text, Active(visible));
        }
        if !visible {
            return;
        }

        let screen = world.screen_size();
        let panel = Self::panel_rect(screen);
        let (mode_id, scale, sharp) = current_upscaler_config();

        set_box(world, p.backdrop, panel, [0.06, 0.08, 0.12, 0.92]);
        set_text(
            world,
            p.title,
            UiRect {
                x: panel.x + PAD,
                y: panel.y + 12.0,
                w: panel.w - PAD * 2.0,
                h: 24.0,
            },
            "Render Settings — Upscaling (F4)",
            16.0,
            [0.95, 0.97, 1.0, 1.0],
            TextAlign::Left,
        );

        let row_marker = |idx: usize, selected: usize, label: &str| -> String {
            if idx == selected {
                format!("> {label}")
            } else {
                label.to_string()
            }
        };

        set_text(
            world,
            p.mode_label,
            UiRect {
                x: panel.x + PAD,
                y: panel.y + 44.0,
                w: panel.w - PAD * 2.0,
                h: 20.0,
            },
            &row_marker(0, self.selected_row, "Upscaler Mode"),
            13.0,
            [0.78, 0.86, 0.96, 1.0],
            TextAlign::Left,
        );
        let mode_y = panel.y + 68.0;
        for (i, opt) in MODE_OPTIONS.iter().enumerate() {
            let r = Self::row_button_rect(panel, mode_y, i, MODE_OPTIONS.len());
            let active = opt.id == mode_id;
            let bg = if active {
                [0.20, 0.52, 0.85, 0.95]
            } else {
                [0.14, 0.17, 0.23, 0.90]
            };
            let fg = if active {
                [1.0, 1.0, 1.0, 1.0]
            } else {
                [0.82, 0.86, 0.92, 1.0]
            };
            set_box(world, p.mode_btns[i].bg, r, bg);
            set_text(
                world,
                p.mode_btns[i].text,
                r,
                opt.label,
                12.0,
                fg,
                TextAlign::Center,
            );
        }

        set_text(
            world,
            p.scale_label,
            UiRect {
                x: panel.x + PAD,
                y: panel.y + 102.0,
                w: panel.w - PAD * 2.0,
                h: 20.0,
            },
            &row_marker(
                1,
                self.selected_row,
                &format!("Render Scale ({:.0}%)", scale * 100.0),
            ),
            13.0,
            [0.78, 0.86, 0.96, 1.0],
            TextAlign::Left,
        );
        let scale_y = panel.y + 126.0;
        for (i, &val) in SCALE_PRESETS.iter().enumerate() {
            let r = Self::row_button_rect(panel, scale_y, i, SCALE_PRESETS.len());
            let active = (scale - val).abs() < 0.03;
            let bg = if active {
                [0.20, 0.52, 0.85, 0.95]
            } else {
                [0.14, 0.17, 0.23, 0.90]
            };
            set_box(world, p.scale_btns[i].bg, r, bg);
            set_text(
                world,
                p.scale_btns[i].text,
                r,
                &format!("{:.0}%", val * 100.0),
                12.0,
                [0.95, 0.97, 1.0, 1.0],
                TextAlign::Center,
            );
        }

        set_text(
            world,
            p.sharp_label,
            UiRect {
                x: panel.x + PAD,
                y: panel.y + 160.0,
                w: panel.w - PAD * 2.0,
                h: 20.0,
            },
            &row_marker(2, self.selected_row, &format!("Sharpness ({sharp:.2})")),
            13.0,
            [0.78, 0.86, 0.96, 1.0],
            TextAlign::Left,
        );
        let sharp_y = panel.y + 184.0;
        for (i, &val) in SHARP_PRESETS.iter().enumerate() {
            let r = Self::row_button_rect(panel, sharp_y, i, SHARP_PRESETS.len());
            let active = (sharp - val).abs() < 0.06;
            let bg = if active {
                [0.20, 0.52, 0.85, 0.95]
            } else {
                [0.14, 0.17, 0.23, 0.90]
            };
            set_box(world, p.sharp_btns[i].bg, r, bg);
            set_text(
                world,
                p.sharp_btns[i].text,
                r,
                &format!("{val:.2}"),
                12.0,
                [0.95, 0.97, 1.0, 1.0],
                TextAlign::Center,
            );
        }

        set_text(
            world,
            p.pre_label,
            UiRect {
                x: panel.x + PAD,
                y: panel.y + 218.0,
                w: panel.w - PAD * 2.0,
                h: 20.0,
            },
            &row_marker(
                3,
                self.selected_row,
                &format!(
                    "DLSS Pre-Upscale ({})",
                    dlss_pre_upscale_label(current_dlss_pre_upscale())
                ),
            ),
            13.0,
            [0.78, 0.86, 0.96, 1.0],
            TextAlign::Left,
        );
        let pre_y = panel.y + 242.0;
        let pre_mode = current_dlss_pre_upscale() as usize;
        for (i, label) in PRE_UPSCALE_LABELS.iter().enumerate() {
            let r = Self::row_button_rect(panel, pre_y, i, PRE_UPSCALE_LABELS.len());
            let bg = if pre_mode == i {
                [0.20, 0.52, 0.85, 0.95]
            } else {
                [0.14, 0.17, 0.23, 0.90]
            };
            set_box(world, p.pre_btns[i].bg, r, bg);
            set_text(
                world,
                p.pre_btns[i].text,
                r,
                label,
                12.0,
                [0.95, 0.97, 1.0, 1.0],
                TextAlign::Center,
            );
        }

        let (dlfg_mode, dlfg_frames) = current_dlfg_config();
        set_text(
            world,
            p.dlfg_label,
            UiRect {
                x: panel.x + PAD,
                y: panel.y + 276.0,
                w: panel.w - PAD * 2.0,
                h: 20.0,
            },
            &row_marker(
                4,
                self.selected_row,
                &format!("Frame Gen (DLFG: {})", dlfg_mode_label(dlfg_mode)),
            ),
            13.0,
            [0.78, 0.86, 0.96, 1.0],
            TextAlign::Left,
        );
        let dlfg_y = panel.y + 300.0;
        for (i, label) in DLFG_MODE_LABELS.iter().enumerate() {
            let r = Self::row_button_rect(panel, dlfg_y, i, DLFG_MODE_LABELS.len());
            let bg = if usize::from(dlfg_mode) == i {
                [0.20, 0.52, 0.85, 0.95]
            } else {
                [0.14, 0.17, 0.23, 0.90]
            };
            set_box(world, p.dlfg_btns[i].bg, r, bg);
            set_text(
                world,
                p.dlfg_btns[i].text,
                r,
                label,
                12.0,
                [0.95, 0.97, 1.0, 1.0],
                TextAlign::Center,
            );
        }

        set_text(
            world,
            p.fg_mult_label,
            UiRect {
                x: panel.x + PAD,
                y: panel.y + 334.0,
                w: panel.w - PAD * 2.0,
                h: 20.0,
            },
            &row_marker(
                5,
                self.selected_row,
                &format!("FG Multiplier ({})", dlfg_frames_label(dlfg_frames)),
            ),
            13.0,
            [0.78, 0.86, 0.96, 1.0],
            TextAlign::Left,
        );
        let fg_mult_y = panel.y + 358.0;
        let fg_idx = usize::from(dlfg_frames.saturating_sub(1).min(2));
        for (i, label) in DLFG_FRAMES_LABELS.iter().enumerate() {
            let r = Self::row_button_rect(panel, fg_mult_y, i, DLFG_FRAMES_LABELS.len());
            let bg = if fg_idx == i {
                [0.20, 0.52, 0.85, 0.95]
            } else {
                [0.14, 0.17, 0.23, 0.90]
            };
            set_box(world, p.fg_mult_btns[i].bg, r, bg);
            set_text(
                world,
                p.fg_mult_btns[i].text,
                r,
                label,
                12.0,
                [0.95, 0.97, 1.0, 1.0],
                TextAlign::Center,
            );
        }

        let uses_scale = matches!(mode_id, 1..=5);
        let rw = if uses_scale {
            ((screen[0].max(1.0)) * scale).round().max(16.0) as u32
        } else {
            screen[0].max(1.0) as u32
        };
        let rh = if uses_scale {
            ((screen[1].max(1.0)) * scale).round().max(16.0) as u32
        } else {
            screen[1].max(1.0) as u32
        };
        let status_line = format!(
            "Active: {} | DLFG: {} ({}) | {}x{} -> {}x{}",
            upscaler_mode_label(mode_id),
            dlfg_mode_label(dlfg_mode),
            dlfg_frames_label(dlfg_frames),
            rw,
            rh,
            screen[0].max(1.0) as u32,
            screen[1].max(1.0) as u32
        );
        set_text(
            world,
            p.status,
            UiRect {
                x: panel.x + PAD,
                y: panel.y + 394.0,
                w: panel.w - PAD * 2.0,
                h: 20.0,
            },
            &status_line,
            12.0,
            [0.65, 0.92, 0.72, 1.0],
            TextAlign::Left,
        );

        #[cfg(not(target_os = "android"))]
        let hint_text = "Up/Down/Left/Right or Click | /upscaler /dlfg /dlss_pre";
        #[cfg(target_os = "android")]
        let hint_text = "Up/Down/Left/Right or Click | /upscaler <mode> [scale] [sharp]";

        set_text(
            world,
            p.hint,
            UiRect {
                x: panel.x + PAD,
                y: panel.y + 416.0,
                w: panel.w - PAD * 2.0,
                h: 18.0,
            },
            hint_text,
            11.0,
            [0.65, 0.70, 0.78, 1.0],
            TextAlign::Left,
        );
    }
}

fn set_box(world: &mut GameWorld<'_>, e: EntityHandle, r: UiRect, background: [f32; 4]) {
    world.set(
        e,
        Style {
            width: Some(r.w),
            height: Some(r.h),
            anchors: Anchors::TOP_LEFT,
            pivot: Pivot::TOP_LEFT,
            margin: Margin {
                left: r.x,
                top: r.y,
                ..Margin::default()
            },
            background,
            ..Style::default()
        },
    );
}

fn set_text(
    world: &mut GameWorld<'_>,
    e: EntityHandle,
    r: UiRect,
    content: &str,
    font_size: f32,
    color: [f32; 4],
    alignment: TextAlign,
) {
    world.set(
        e,
        Style {
            width: Some(r.w),
            height: Some(r.h),
            anchors: Anchors::TOP_LEFT,
            pivot: Pivot::TOP_LEFT,
            margin: Margin {
                left: r.x,
                top: r.y,
                ..Margin::default()
            },
            ..Style::default()
        },
    );
    world.set(
        e,
        Text {
            content: content.to_string(),
            font_size,
            color,
            alignment,
            wrap: false,
            ..Text::default()
        },
    );
}
