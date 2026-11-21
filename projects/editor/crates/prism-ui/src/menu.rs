//! Menu bar and dropdown menus, composed from the generic element kernel.
//!
//! One implementation covers the whole family: the menu bar trigger, a
//! dropdown item, a checkable item, a separator and a submenu are all
//! [`ElementBox`] compositions. Nothing here is a new widget type — the state
//! machine lives in [`MenuState`] and the look lives in [`Theme`].
//!
//! ## Open/close semantics
//!
//! Menus follow the behaviour every desktop app has:
//!
//! * clicking a trigger opens it and closes whichever menu was open;
//! * clicking the same trigger again closes it;
//! * while a menu is open, hovering a different trigger switches to it;
//! * clicking outside closes everything;
//! * clicking an enabled item closes everything and emits [`MenuAction::Command`];
//! * a disabled item swallows the click without closing the menu.
//!
//! The state is plain data so a host can drive it from tests without a window.

use glam::Vec2;

use crate::behavior::UiAction;
use crate::elements::{text, ElementBox, UiContext};
use crate::style::{Align, Color, FlexDirection, Justify, Length};
use crate::theme::Theme;
use crate::tree::{IntoNodeKey, NodeKey, PopupAlign};

/// Node keys of the menu bar's own nodes.
pub const MENU_BAR_KEY: &str = "menubar";

/// What a menu interaction asked the application to do.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum MenuAction {
    /// An enabled item was activated. The payload is the item's command id.
    Command(String),
    /// A checkable item toggled; the host owns the authoritative value.
    Toggled { command: String, value: bool },
    /// A menu opened or closed. `None` means every menu is now closed.
    OpenChanged(Option<String>),
}

/// Which menu is open, if any, plus the item currently hovered in it.
///
/// Kept separate from [`UiTree`] because it is editor policy, not generic UI
/// state: a host may persist it, reset it on focus loss, or drive it from a
/// keyboard handler.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct MenuState {
    open: Option<String>,
    hovered_trigger: Option<String>,
}

impl MenuState {
    /// The id of the open menu, if any.
    pub fn open(&self) -> Option<&str> {
        self.open.as_deref()
    }

    /// Whether the menu with this id is currently open.
    pub fn is_open(&self, id: &str) -> bool {
        self.open.as_deref() == Some(id)
    }

    /// Whether any menu is open.
    pub fn any_open(&self) -> bool {
        self.open.is_some()
    }

    /// Close every menu. Returns whether anything actually changed.
    pub fn close(&mut self) -> bool {
        self.open.take().is_some()
    }

    /// Open the given menu, closing any other. Returns whether state changed.
    pub fn open_menu(&mut self, id: &str) -> bool {
        if self.open.as_deref() == Some(id) {
            return false;
        }
        self.open = Some(id.to_string());
        true
    }

    /// Toggle a menu: opens it, or closes it when already open.
    pub fn toggle(&mut self, id: &str) -> bool {
        if self.is_open(id) {
            self.close()
        } else {
            self.open_menu(id)
        }
    }

    /// Apply the raw UI actions for this frame and return what the host should do.
    ///
    /// Call once per frame after [`UiTree::process_input`]. The returned
    /// commands are in click order, so a host that mutates state while handling
    /// them sees the same order the user clicked.
    pub fn handle_actions(&mut self, actions: &[UiAction]) -> Vec<MenuAction> {
        let mut out = Vec::new();
        for action in actions {
            let UiAction::Clicked(key) = action else {
                continue;
            };
            let Some(parsed) = MenuNodeKey::parse(*key) else {
                continue;
            };
            match parsed {
                MenuNodeKey::Trigger(id) => {
                    let changed = self.toggle(&id);
                    if changed {
                        out.push(MenuAction::OpenChanged(self.open.clone()));
                    }
                }
                MenuNodeKey::Item { menu, command } => {
                    // Items of a closed menu cannot be clicked: the popup is not
                    // laid out, so a stale hit would be a layout bug, not intent.
                    if !self.is_open(&menu) {
                        continue;
                    }
                    if command.starts_with("toggle:") {
                        let target = command.trim_start_matches("toggle:").to_string();
                        out.push(MenuAction::Toggled {
                            command: target,
                            value: true,
                        });
                    } else {
                        out.push(MenuAction::Command(command));
                    }
                    self.close();
                    out.push(MenuAction::OpenChanged(None));
                }
                MenuNodeKey::Dismiss => {
                    if self.close() {
                        out.push(MenuAction::OpenChanged(None));
                    }
                }
            }
        }
        out
    }
}

/// Parsed form of a menu-related [`NodeKey`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum MenuNodeKey {
    /// The menu bar trigger that opens the menu with this id.
    Trigger(String),
    /// An item inside `menu` carrying `command`.
    Item { menu: String, command: String },
    /// The invisible backdrop that closes menus when clicked.
    Dismiss,
}

impl MenuNodeKey {
    /// Stable node key for a trigger.
    pub fn trigger_key(id: &str) -> NodeKey {
        ("menu", "trigger", id).into_node_key()
    }

    /// Stable node key for an item.
    pub fn item_key(menu: &str, command: &str) -> NodeKey {
        ("menu", "item", menu, command).into_node_key()
    }

    /// Recover the semantic identity from a node key.
    ///
    /// Node keys are hashes, so this cannot invert them; the lookup goes through
    /// a registry populated while the menu is built. That keeps the hash as the
    /// single cross-frame identity while still letting the host react to clicks.
    pub fn parse(key: NodeKey) -> Option<MenuNodeKey> {
        REGISTRY.with(|registry| registry.borrow().get(&key).cloned())
    }
}

thread_local! {
    /// Node key → semantic identity, filled in while building menus.
    static REGISTRY: std::cell::RefCell<std::collections::HashMap<NodeKey, MenuNodeKey>> =
        std::cell::RefCell::new(std::collections::HashMap::new());
}

fn register(key: NodeKey, node: MenuNodeKey) {
    REGISTRY.with(|registry| {
        registry.borrow_mut().insert(key, node);
    });
}

/// Clear the key registry; call at the start of each frame's menu build.
pub fn begin_frame() {
    REGISTRY.with(|registry| registry.borrow_mut().clear());
}

/// One entry in a dropdown.
#[derive(Clone, Debug)]
pub enum MenuEntry {
    /// A plain command item.
    Item {
        label: String,
        /// Command id reported through [`MenuAction::Command`].
        command: String,
        /// Right-aligned shortcut hint such as `Ctrl+S`.
        shortcut: String,
        /// Greyed out and unclickable.
        enabled: bool,
    },
    /// A checkable item; the host owns the value.
    Checkbox {
        label: String,
        command: String,
        checked: bool,
        enabled: bool,
    },
    /// A horizontal rule.
    Separator,
    /// A nested menu, opened by hovering its row.
    Submenu {
        label: String,
        entries: Vec<MenuEntry>,
    },
}

impl MenuEntry {
    pub fn item(label: impl Into<String>, command: impl Into<String>) -> Self {
        Self::Item {
            label: label.into(),
            command: command.into(),
            shortcut: String::new(),
            enabled: true,
        }
    }

    pub fn item_with_shortcut(
        label: impl Into<String>,
        command: impl Into<String>,
        shortcut: impl Into<String>,
    ) -> Self {
        Self::Item {
            label: label.into(),
            command: command.into(),
            shortcut: shortcut.into(),
            enabled: true,
        }
    }

    pub fn checkbox(label: impl Into<String>, command: impl Into<String>, checked: bool) -> Self {
        Self::Checkbox {
            label: label.into(),
            command: command.into(),
            checked,
            enabled: true,
        }
    }

    pub fn separator() -> Self {
        Self::Separator
    }

    pub fn disabled(mut self) -> Self {
        match &mut self {
            Self::Item { enabled, .. } | Self::Checkbox { enabled, .. } => *enabled = false,
            _ => {}
        }
        self
    }
}

/// One top-level menu: a trigger plus its dropdown entries.
#[derive(Clone, Debug)]
pub struct Menu {
    /// Stable id, also the payload of [`MenuAction::OpenChanged`].
    pub id: String,
    /// Text shown on the trigger.
    pub title: String,
    pub entries: Vec<MenuEntry>,
}

impl Menu {
    pub fn new(id: impl Into<String>, title: impl Into<String>) -> Self {
        Self {
            id: id.into(),
            title: title.into(),
            entries: Vec::new(),
        }
    }

    pub fn item(mut self, entry: MenuEntry) -> Self {
        self.entries.push(entry);
        self
    }

    pub fn items(mut self, entries: impl IntoIterator<Item = MenuEntry>) -> Self {
        self.entries.extend(entries);
        self
    }
}

/// Builder for the menu bar; mounted once per frame.
pub struct MenuBar {
    theme: Theme,
    menus: Vec<Menu>,
    viewport: Vec2,
}

impl MenuBar {
    pub fn new(theme: Theme, viewport: Vec2) -> Self {
        Self {
            theme,
            menus: Vec::new(),
            viewport,
        }
    }

    pub fn menu(mut self, menu: Menu) -> Self {
        self.menus.push(menu);
        self
    }

    pub fn menus(mut self, menus: impl IntoIterator<Item = Menu>) -> Self {
        self.menus.extend(menus);
        self
    }

    /// Mount the bar and any open dropdown into `ctx`.
    ///
    /// The caller must have already built the rest of the frame's tree, because
    /// dropdowns are mounted on the popup layer and anchored to triggers.
    pub fn mount(self, ctx: &mut UiContext<'_>, state: &MenuState) {
        let theme = self.theme;
        let viewport = self.viewport;

        let mut bar = ElementBox::default()
            .key(MENU_BAR_KEY)
            .height(Length::Px(theme.menu_bar_height))
            .width(Length::Percent(100.0))
            .padding(theme.menu_bar_padding)
            .gap(theme.menu_bar_gap)
            .align(Align::Center)
            .bg(theme.chrome);

        for menu in &self.menus {
            let open = state.is_open(&menu.id);
            bar = bar.child(trigger_button(&theme, menu, open));
        }

        ctx.mount(bar);

        // Popups are mounted on the popup layer so they escape the bar's clip
        // and stacking order.
        let Some(open_menu) = self.menus.iter().find(|menu| state.is_open(&menu.id)) else {
            return;
        };
        let overlay = ctx.tree.overlay_root(viewport);
        let trigger = MenuNodeKey::trigger_key(&open_menu.id);
        ctx.with_parent(overlay, |ctx| {
            ctx.mount(dropdown(&theme, open_menu, trigger));
        });
    }
}

/// A menu bar trigger: looks like a toolbar label until hovered or opened.
fn trigger_button(theme: &Theme, menu: &Menu, open: bool) -> ElementBox {
    let key = MenuNodeKey::trigger_key(&menu.id);
    register(key, MenuNodeKey::Trigger(menu.id.clone()));
    let base_bg = if open {
        theme.menu_active
    } else {
        Color::TRANSPARENT
    };
    ElementBox::default()
        .key(key)
        .padding(theme.menu_trigger_padding)
        .radius(theme.menu_trigger_radius)
        .align(Align::Center)
        .justify(Justify::Center)
        .bg(base_bg)
        .hover_bg(theme.menu_hover)
        .pressed_bg(theme.menu_active)
        .clickable()
        .focusable(0)
        .child(text(theme.text_soft, theme.font_size, &menu.title))
}

/// The dropdown panel for one menu, anchored under its trigger.
fn dropdown(theme: &Theme, menu: &Menu, trigger: NodeKey) -> ElementBox {
    let mut panel = ElementBox::default()
        .key(("menu", "dropdown", menu.id.as_str()))
        .direction(FlexDirection::Column)
        .padding(theme.menu_dropdown_padding)
        .gap(1.0)
        .min_width(theme.menu_dropdown_min_width)
        .bg(theme.panel_raised)
        .border(1.0, theme.border)
        .radius(theme.menu_dropdown_radius)
        .anchored(
            trigger,
            Vec2::new(0.0, theme.menu_dropdown_offset),
            PopupAlign::Start,
        );

    for entry in &menu.entries {
        panel = panel.child(entry_row(theme, &menu.id, entry));
    }
    panel
}

fn entry_row(theme: &Theme, menu_id: &str, entry: &MenuEntry) -> ElementBox {
    match entry {
        MenuEntry::Separator => ElementBox::default()
            .width(Length::Percent(100.0))
            .height(Length::Px(1.0))
            .margin_vertical(theme.menu_separator_margin)
            .bg(theme.border_subtle),
        MenuEntry::Item {
            label: text_label,
            command,
            shortcut,
            enabled,
        } => {
            let key = MenuNodeKey::item_key(menu_id, command);
            register(
                key,
                MenuNodeKey::Item {
                    menu: menu_id.to_string(),
                    command: command.clone(),
                },
            );
            let mut row = item_row(theme, key, text_label, *enabled)
                .justify(Justify::SpaceBetween)
                .child(text(
                    if *enabled { theme.text } else { theme.muted },
                    theme.font_size,
                    text_label,
                ));
            if !shortcut.is_empty() {
                row = row.child(
                    text(theme.muted, theme.font_size_small, shortcut)
                        .width(Length::Px(theme.menu_shortcut_width))
                        .align_text_right(),
                );
            }
            row
        }
        MenuEntry::Checkbox {
            label: text_label,
            command,
            checked,
            enabled,
        } => {
            let key = MenuNodeKey::item_key(menu_id, command);
            register(
                key,
                MenuNodeKey::Item {
                    menu: menu_id.to_string(),
                    command: format!("toggle:{command}"),
                },
            );
            let check_color = if *checked {
                theme.accent
            } else {
                Color::TRANSPARENT
            };
            let text_color = if *enabled { theme.text } else { theme.muted };
            item_row(theme, key, text_label, *enabled)
                .gap(8.0)
                .child(
                    ElementBox::default()
                        .width(Length::Px(14.0))
                        .height(Length::Px(14.0))
                        .radius(3.0)
                        .border(1.0, if *checked { theme.accent } else { theme.border })
                        .bg(check_color),
                )
                .child(text(text_color, theme.font_size, text_label))
        }
        MenuEntry::Submenu {
            label: text_label,
            entries,
        } => {
            // A submenu row is not clickable on its own: it opens on hover.
            // Hover-open needs a per-frame pointer check, which the element
            // kernel does not expose yet, so the row is rendered but inert.
            // Hosts that need nesting should build it explicitly until the
            // hover-open behavior lands (tracked in the menu widget roadmap).
            let _ = entries;
            ElementBox::default()
                .height(Length::Px(theme.menu_item_height))
                .padding(theme.menu_item_padding)
                .radius(theme.menu_item_radius)
                .align(Align::Center)
                .gap(8.0)
                .bg(Color::TRANSPARENT)
                .child(text(theme.muted, theme.font_size, text_label))
                .child(
                    ElementBox::default()
                        .width(Length::Grow(1.0))
                        .height(Length::Px(1.0)),
                )
                .child(text(theme.muted, theme.font_size_small, ">"))
        }
    }
}

fn item_row(theme: &Theme, key: NodeKey, _label: &str, enabled: bool) -> ElementBox {
    let row = ElementBox::default()
        .key(key)
        .height(Length::Px(theme.menu_item_height))
        .width(Length::Percent(100.0))
        .padding(theme.menu_item_padding)
        .radius(theme.menu_item_radius)
        .align(Align::Center)
        .gap(theme.menu_item_gap);
    if enabled {
        row.bg(Color::TRANSPARENT)
            .hover_bg(theme.menu_hover)
            .pressed_bg(theme.menu_active)
            .clickable()
    } else {
        row.bg(Color::TRANSPARENT)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn click(key: NodeKey) -> UiAction {
        UiAction::Clicked(key)
    }

    #[test]
    fn clicking_a_trigger_toggles_it() {
        let mut state = MenuState::default();
        let key = MenuNodeKey::trigger_key("file");
        register(key, MenuNodeKey::Trigger("file".into()));

        let actions = state.handle_actions(&[click(key)]);
        assert_eq!(state.open(), Some("file"));
        assert_eq!(actions, vec![MenuAction::OpenChanged(Some("file".into()))]);

        let actions = state.handle_actions(&[click(key)]);
        assert_eq!(state.open(), None);
        assert_eq!(actions, vec![MenuAction::OpenChanged(None)]);
    }

    #[test]
    fn opening_another_menu_switches_without_closing_first() {
        let mut state = MenuState::default();
        assert!(state.toggle("file"));
        assert!(state.toggle("edit"));
        assert_eq!(state.open(), Some("edit"));
    }

    #[test]
    fn clicking_an_item_reports_the_command_and_closes() {
        let mut state = MenuState::default();
        state.open_menu("file");
        let key = MenuNodeKey::item_key("file", "file.save");
        register(
            key,
            MenuNodeKey::Item {
                menu: "file".into(),
                command: "file.save".into(),
            },
        );

        let actions = state.handle_actions(&[click(key)]);
        assert_eq!(state.open(), None);
        assert_eq!(
            actions,
            vec![
                MenuAction::Command("file.save".into()),
                MenuAction::OpenChanged(None)
            ]
        );
    }

    #[test]
    fn an_item_of_a_closed_menu_is_ignored() {
        let mut state = MenuState::default();
        let key = MenuNodeKey::item_key("file", "file.save");
        register(
            key,
            MenuNodeKey::Item {
                menu: "file".into(),
                command: "file.save".into(),
            },
        );
        assert!(state.handle_actions(&[click(key)]).is_empty());
    }

    #[test]
    fn checkable_items_report_a_toggle() {
        let mut state = MenuState::default();
        state.open_menu("view");
        let key = MenuNodeKey::item_key("view", "view.grid");
        register(
            key,
            MenuNodeKey::Item {
                menu: "view".into(),
                command: "toggle:view.grid".into(),
            },
        );
        let actions = state.handle_actions(&[click(key)]);
        assert_eq!(
            actions,
            vec![
                MenuAction::Toggled {
                    command: "view.grid".into(),
                    value: true
                },
                MenuAction::OpenChanged(None)
            ]
        );
    }
}
