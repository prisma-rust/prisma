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
use crate::icons::{icon_view, IconKind, IconSet};
use crate::material::Material;
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
    /// Open submenu labels inside the open menu, outermost first.
    ///
    /// Resolved from hover **after** input is processed, while the frame's nodes
    /// still exist, and then read back while mounting the next frame. Resolving
    /// during the build would query a tree that `begin_frame` just cleared.
    submenu_branch: Vec<String>,
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

    /// Labels of the open submenu branch, outermost first.
    pub fn submenu_branch(&self) -> &[String] {
        &self.submenu_branch
    }

    /// Recompute the open submenu branch from the tree's current hover.
    ///
    /// Call once per frame **after** [`crate::tree::UiTree::process_input`]: the
    /// answer depends on which node the pointer landed on, and on the panel that
    /// node belongs to. Returns whether the branch changed.
    pub fn sync_hover(
        &mut self,
        tree: &crate::tree::UiTree,
        entries_of: impl Fn(&str) -> Option<Vec<MenuEntry>>,
    ) -> bool {
        if self.open.is_some() {
            if let Some(MenuNodeKey::Trigger(hovered_id)) =
                tree.hovered().and_then(MenuNodeKey::parse)
            {
                if self.open.as_deref() != Some(hovered_id.as_str()) {
                    self.open = Some(hovered_id);
                    self.submenu_branch.clear();
                    return true;
                }
            }
        }
        let Some(menu_id) = self.open.clone() else {
            let changed = !self.submenu_branch.is_empty();
            self.submenu_branch.clear();
            return changed;
        };
        let Some(entries) = entries_of(&menu_id) else {
            return false;
        };
        let mut branch = resolve_branch(tree, &menu_id, &entries);
        if branch.is_empty() && self.pointer_in_branch(tree, &menu_id) {
            branch = self.submenu_branch.clone();
        }
        if branch == self.submenu_branch {
            return false;
        }
        self.submenu_branch = branch;
        true
    }

    /// Whether the pointer is inside a panel the current branch already opened.
    ///
    /// Only the branch's own panels count: the top-level menu panel holds plain
    /// rows too, and the pointer resting on one of those means the branch really
    /// has been left.
    fn pointer_in_branch(&self, tree: &crate::tree::UiTree, menu_id: &str) -> bool {
        let mut scope = menu_id.to_string();
        for label in &self.submenu_branch {
            scope = format!("{scope}/{label}");
            if tree.is_hovered_within(("menu", "dropdown", scope.as_str()).into_node_key()) {
                return true;
            }
        }
        false
    }

    /// Close any open submenu branch. Returns whether anything changed.
    pub fn close_submenus(&mut self) -> bool {
        !self.submenu_branch.is_empty() && {
            self.submenu_branch.clear();
            true
        }
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
                MenuNodeKey::Submenu { .. } => {}
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
    /// A submenu row inside `menu`, identified by its label.
    Submenu { menu: String, label: String },
    /// The invisible backdrop that closes menus when clicked.
    Dismiss,
}

impl MenuNodeKey {
    /// Stable node key for a trigger.
    pub fn trigger_key(id: &str) -> NodeKey {
        ("menu", "trigger", id).into_node_key()
    }

    /// Stable node key for a submenu row.
    pub fn submenu_key(menu: &str, label: &str) -> NodeKey {
        submenu_row_key(menu, label)
    }

    /// Stable node key for an item.
    pub fn item_key(menu: &str, command: &str) -> NodeKey {
        ("menu", "item", menu, command).into_node_key()
    }

    /// Stable node key for the backdrop that closes menus on an outside click.
    pub fn dismiss_key() -> NodeKey {
        ("menu", "dismiss").into_node_key()
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

/// Node key of a submenu row, shared by the builder and the hover resolver.
fn submenu_row_key(menu_id: &str, label: &str) -> NodeKey {
    ("menu", "submenu", menu_id, label).into_node_key()
}

fn register(key: NodeKey, node: MenuNodeKey) {
    REGISTRY.with(|registry| {
        registry.borrow_mut().insert(key, node);
    });
}

/// Clear the key registry.
///
/// Deliberately not called per frame: menus are built before input is
/// processed, but a click is handled on the frame *after* the one that
/// registered the key. Entries are stable for a given menu definition, so the
/// registry only needs resetting when that definition changes.
pub fn reset_key_registry() {
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

    /// A nested menu opened by hovering its row.
    pub fn submenu(label: impl Into<String>, entries: Vec<MenuEntry>) -> Self {
        Self::Submenu {
            label: label.into(),
            entries,
        }
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
    icons: IconSet,
}

impl MenuBar {
    pub fn new(theme: Theme, viewport: Vec2) -> Self {
        Self {
            theme,
            menus: Vec::new(),
            viewport,
            icons: IconSet::default(),
        }
    }

    pub fn icons(mut self, icons: IconSet) -> Self {
        self.icons = icons;
        self
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
        let icons = self.icons;

        let mut bar = ElementBox::default()
            .key(MENU_BAR_KEY)
            .height(Length::Px(theme.menu_bar_height))
            .padding(theme.menu_bar_padding)
            .gap(theme.menu_bar_gap)
            .align(Align::Center)
            .bg(menu_bar_glass_tint(&theme))
            .material(Material::frosted_glass(theme.glass_blur_sigma));

        for menu in &self.menus {
            let open = state.is_open(&menu.id);
            bar = bar.child(trigger_button(&theme, menu, open));
        }

        ctx.mount(bar);

        let Some(open_menu) = self.menus.iter().find(|menu| state.is_open(&menu.id)) else {
            return;
        };
        let overlay = ctx.tree.overlay_root(viewport);
        let trigger = MenuNodeKey::trigger_key(&open_menu.id);
        let branch = state.submenu_branch().to_vec();
        ctx.with_parent(overlay, |ctx| {
            let dismiss = MenuNodeKey::dismiss_key();
            register(dismiss, MenuNodeKey::Dismiss);
            ctx.mount(
                ElementBox::default()
                    .key(dismiss)
                    .absolute(0.0, theme.menu_bar_height)
                    .size(
                        Length::Px(viewport.x),
                        Length::Px((viewport.y - theme.menu_bar_height).max(0.0)),
                    )
                    .bg(Color::TRANSPARENT)
                    .clickable(),
            );

            ctx.mount(dropdown(
                &theme,
                &icons,
                &open_menu.id,
                &open_menu.entries,
                trigger,
                false,
            ));

            let mut entries: &[MenuEntry] = &open_menu.entries;
            let mut scope = open_menu.id.clone();
            for label in &branch {
                let Some(MenuEntry::Submenu {
                    entries: child_entries,
                    ..
                }) = entries.iter().find(
                    |entry| matches!(entry, MenuEntry::Submenu { label: l, .. } if l == label),
                )
                else {
                    break;
                };
                let row_key = submenu_row_key(&scope, label);
                scope = format!("{scope}/{label}");
                ctx.mount(dropdown(
                    &theme,
                    &icons,
                    &scope,
                    child_entries,
                    row_key,
                    true,
                ));
                entries = child_entries;
            }
        });
    }
}

/// Which submenu labels are open, decided by which row the pointer is on.
///
/// Hovering a submenu row opens it, which implicitly closes its siblings; moving
/// onto a plain row closes the whole branch. That is what makes a menu feel like
/// a menu rather than a sticky tree.
fn resolve_branch(tree: &crate::tree::UiTree, menu_id: &str, entries: &[MenuEntry]) -> Vec<String> {
    /// Whether the pointer is on any row reachable below `scope`.
    ///
    /// Used to keep an ancestor open while the pointer is further out along the
    /// same branch, which is the only way a four-level chain stays on screen.
    fn enters_subtree(tree: &crate::tree::UiTree, scope: &str, entries: &[MenuEntry]) -> bool {
        for entry in entries {
            let MenuEntry::Submenu {
                label,
                entries: children,
            } = entry
            else {
                continue;
            };
            let child_scope = format!("{scope}/{label}");
            let panel = ("menu", "dropdown", child_scope.as_str()).into_node_key();
            if tree.is_hovered(submenu_row_key(scope, label)) || tree.is_hovered_within(panel) {
                return true;
            }
            if enters_subtree(tree, &child_scope, children) {
                return true;
            }
        }
        false
    }

    fn deepest(
        tree: &crate::tree::UiTree,
        scope: &str,
        entries: &[MenuEntry],
        trail: &mut Vec<String>,
    ) -> bool {
        let mut best: Option<(String, String, &[MenuEntry])> = None;
        for entry in entries {
            let MenuEntry::Submenu {
                label,
                entries: children,
            } = entry
            else {
                continue;
            };
            let child_scope = format!("{scope}/{label}");
            let panel = ("menu", "dropdown", child_scope.as_str()).into_node_key();
            let on_row = tree.is_hovered(submenu_row_key(scope, label));
            let in_panel = tree.is_hovered_within(panel);
            let deeper = enters_subtree(tree, &child_scope, children);
            if on_row || in_panel || deeper {
                best = Some((label.clone(), child_scope, children));
                break;
            }
        }
        let Some((label, child_scope, children)) = best else {
            return false;
        };
        trail.push(label);
        let _ = deepest(tree, &child_scope, children, trail);
        true
    }

    let mut branch = Vec::new();
    deepest(tree, menu_id, entries, &mut branch);
    branch
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
    let label_color = if open { theme.text_strong } else { theme.text };
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
        .child(text(label_color, theme.font_size, &menu.title))
}

fn menu_bar_glass_tint(theme: &Theme) -> Color {
    theme
        .glass_tint
        .with_alpha((theme.glass_tint.a + 0.08).min(0.88))
}

fn dropdown_glass_tint(theme: &Theme) -> Color {
    theme
        .glass_tint
        .with_alpha((theme.glass_tint.a + 0.06).min(0.86))
}

/// The dropdown panel for one menu.
///
/// Top-level menus hang below their trigger; nested menus open beside the row
/// that spawned them, which is what keeps a deep chain readable.
fn dropdown(
    theme: &Theme,
    icons: &IconSet,
    menu_id: &str,
    entries: &[MenuEntry],
    trigger: NodeKey,
    nested: bool,
) -> ElementBox {
    let mut panel = ElementBox::default()
        .key(("menu", "dropdown", menu_id))
        .direction(FlexDirection::Column)
        .padding(theme.menu_dropdown_padding)
        .gap(1.0)
        .min_width(theme.menu_dropdown_min_width)
        .bg(dropdown_glass_tint(theme))
        .material(
            Material::frosted_glass(theme.glass_blur_sigma).with_radius(theme.menu_dropdown_radius),
        )
        .border(1.0, theme.border.with_alpha(0.75))
        .radius(theme.menu_dropdown_radius)
        .hoverable();

    panel = if nested {
        panel.anchored_beside(
            trigger,
            Vec2::new(0.0, -theme.menu_dropdown_padding.top),
            PopupAlign::Start,
        )
    } else {
        panel.anchored(
            trigger,
            Vec2::new(0.0, theme.menu_dropdown_offset),
            PopupAlign::Start,
        )
    };

    for entry in entries {
        panel = panel.child(entry_row(theme, icons, menu_id, entry));
    }
    panel
}

fn entry_row(theme: &Theme, icons: &IconSet, menu_id: &str, entry: &MenuEntry) -> ElementBox {
    match entry {
        MenuEntry::Separator => ElementBox::default()
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
            let text_color = if *enabled { theme.text } else { theme.muted };
            let indicator = if *checked {
                icon_view(icons, IconKind::Check, 13.0, theme.accent)
            } else {
                ElementBox::default().size(Length::Px(13.0), Length::Px(13.0))
            };
            item_row(theme, key, text_label, *enabled)
                .gap(8.0)
                .child(indicator)
                .child(text(text_color, theme.font_size, text_label))
        }
        MenuEntry::Submenu {
            label: text_label,
            entries,
        } => {
            let key = submenu_row_key(menu_id, text_label);
            register(
                key,
                MenuNodeKey::Submenu {
                    menu: menu_id.to_string(),
                    label: text_label.clone(),
                },
            );
            let _ = entries;
            ElementBox::default()
                .key(key)
                .height(Length::Px(theme.menu_item_height))
                .padding(theme.menu_item_padding)
                .radius(theme.menu_item_radius)
                .align(Align::Center)
                .gap(8.0)
                .bg(Color::TRANSPARENT)
                .hover_bg(theme.menu_hover)
                .hoverable()
                .child(text(theme.text, theme.font_size, text_label))
                .child(
                    ElementBox::default()
                        .width(Length::Grow(1.0))
                        .height(Length::Px(1.0)),
                )
                .child(icon_view(icons, IconKind::ChevronRight, 12.0, theme.muted))
        }
    }
}

fn item_row(theme: &Theme, key: NodeKey, _label: &str, enabled: bool) -> ElementBox {
    let row = ElementBox::default()
        .key(key)
        .height(Length::Px(theme.menu_item_height))
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
#[path = "menu_geometry_tests.rs"]
mod geometry_tests;

#[cfg(test)]
#[path = "menu_fullscreen_tests.rs"]
mod fullscreen_tests;

#[cfg(test)]
#[path = "menu_nested_tests.rs"]
mod nested_tests;

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
