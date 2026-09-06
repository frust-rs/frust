//! Desktop app identity: the platform-independent configuration the shared
//! winit core threads into window creation and carries for the per-OS shell
//! crates.
//!
//! Everything here is plain data — no `winit`, no platform types, no behavior.
//! The core itself consumes exactly one field ([`DesktopConfig::app_name`], as
//! the window title); the rest exists so a per-OS shell can read one config
//! rather than inventing its own (`app_id` becomes a Wayland `app_id`/X11
//! `WM_CLASS` or a Windows AppUserModelID; `window_icon` becomes an
//! `NSApplication` icon / `HICON` / X11 icon; `menu_spec` becomes an NSApp menu
//! bar or an HMENU).
//!
//! # Why the menu vocabulary lives here, below the facade
//!
//! [`MenuSpec`]/[`MenuItemSpec`] are app-facing types, so the obvious home
//! looks like the `frust` facade. It cannot be: the facade depends on the shell
//! crates, never the reverse, and both the shared core (which carries the spec)
//! and the per-OS shells (which build a native menu from it) need to name the
//! type. So the vocabulary is *defined* here — the lowest crate that all of
//! them share — and the facade *re-exports* it, exactly as it re-exports the
//! reactive seams it likewise cannot own.
//!
//! # Defaults are today's behavior
//!
//! [`DesktopConfig::default()`] reproduces the zero-config dev-preview window
//! byte-for-byte: the title falls back to [`DEFAULT_APP_NAME`], no icon, no
//! menu, and a close request quits. An app that never configures anything gets
//! precisely the window it got before this seam existed.

/// The window title used when no [`DesktopConfig::app_name`] is configured —
/// the dev-preview shell's historical hardcoded title, preserved so the
/// zero-config path is unchanged.
pub const DEFAULT_APP_NAME: &str = "Frust";

/// The desktop app's identity and native-integration configuration.
///
/// Built by the facade from an app's own declaration and handed to
/// [`run_desktop_with`](crate::run_desktop_with); every field is optional and
/// the [`Default`] value is today's dev-preview behavior (see the module docs).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DesktopConfig {
    /// The app's display name: the window title, and (for the per-OS shells)
    /// the name a macOS menu bar's application menu shows. `None` falls back to
    /// [`DEFAULT_APP_NAME`] for the title — see [`DesktopConfig::window_title`].
    pub app_name: Option<String>,
    /// The app's reverse-DNS identifier (`com.example.myapp`). Carried, never
    /// consumed by the shared core: the Linux shell turns it into a Wayland
    /// `app_id`/X11 `WM_CLASS` so the window pairs with its `.desktop` entry,
    /// and the Windows shell into an AppUserModelID so taskbar grouping and
    /// notifications attribute correctly.
    pub app_id: Option<String>,
    /// The window/taskbar icon as platform-independent RGBA (see [`IconData`]).
    /// Carried for the per-OS shells; the shared core never attaches it, since
    /// winit's own `WindowAttributes::with_window_icon` reaches X11 and Windows
    /// only and macOS wants an application icon rather than a window one.
    pub window_icon: Option<IconData>,
    /// The native menu bar to install, if any (see [`MenuSpec`]). Carried for
    /// the per-OS shells that can build one; a platform with no native menu
    /// (Linux, where the menu is widget-drawn) simply ignores it.
    pub menu_spec: Option<MenuSpec>,
    /// Whether closing the last window quits the app.
    ///
    /// Carried, not consumed by the shared core: with no extension installed a
    /// close request always exits the event loop (today's behavior, and what
    /// `true` means anyway). It exists for the macOS shell, where the platform
    /// convention is for an app to stay running with no window until ⌘Q —
    /// see `DesktopExtensions::on_close_requested`.
    pub quit_on_last_window_closed: bool,
}

impl Default for DesktopConfig {
    /// Hand-written rather than derived because
    /// [`quit_on_last_window_closed`](DesktopConfig::quit_on_last_window_closed)
    /// defaults to `true` — the shell's existing unconditional
    /// `event_loop.exit()` on `CloseRequested`.
    fn default() -> Self {
        Self {
            app_name: None,
            app_id: None,
            window_icon: None,
            menu_spec: None,
            quit_on_last_window_closed: true,
        }
    }
}

impl DesktopConfig {
    /// A config that reproduces today's zero-config preview window (see
    /// [`Default`]).
    pub fn new() -> Self {
        Self::default()
    }

    /// Set the app's display name (the window title).
    pub fn with_app_name(mut self, app_name: impl Into<String>) -> Self {
        self.app_name = Some(app_name.into());
        self
    }

    /// Set the app's reverse-DNS identifier.
    pub fn with_app_id(mut self, app_id: impl Into<String>) -> Self {
        self.app_id = Some(app_id.into());
        self
    }

    /// Set the window/taskbar icon.
    pub fn with_window_icon(mut self, icon: IconData) -> Self {
        self.window_icon = Some(icon);
        self
    }

    /// Set the native menu bar to install.
    pub fn with_menu_spec(mut self, menu_spec: MenuSpec) -> Self {
        self.menu_spec = Some(menu_spec);
        self
    }

    /// Set whether closing the last window quits the app.
    pub fn with_quit_on_last_window_closed(mut self, quit: bool) -> Self {
        self.quit_on_last_window_closed = quit;
        self
    }

    /// The title to give the preview window: the configured
    /// [`app_name`](DesktopConfig::app_name), else [`DEFAULT_APP_NAME`].
    ///
    /// The fallback lives here rather than in the field itself so a per-OS
    /// shell can still tell "the app named itself `Frust`" apart from "the app
    /// named itself nothing" — a macOS menu bar wants the real name or no
    /// application menu at all, not a placeholder.
    pub fn window_title(&self) -> &str {
        self.app_name.as_deref().unwrap_or(DEFAULT_APP_NAME)
    }
}

/// A decoded window/app icon: tightly-packed, non-premultiplied RGBA8 rows,
/// top-to-bottom — the one representation every desktop platform can be fed
/// from (winit's `Icon::from_rgba`, an `NSImage` bitmap rep, an `HICON` DIB).
///
/// Decoding a PNG/ICNS/ICO into this is the caller's job: this crate carries no
/// image decoder, and the tooling tier already owns the icon pipeline.
///
/// The `rgba`/`width`/`height` triple is an invariant, not three independent
/// fields (`rgba.len() == width * height * 4`), so the fields are private and
/// [`IconData::from_rgba`] is the only way in — an inconsistent icon reaches a
/// platform API as a buffer overrun, not a wrong picture.
///
/// `Debug` is hand-written rather than derived (see the manual `impl` below):
/// a derived one would dump the whole pixel buffer, drowning a log line in
/// thousands of byte values for even a small icon.
#[derive(Clone, PartialEq, Eq)]
pub struct IconData {
    rgba: Vec<u8>,
    width: u32,
    height: u32,
}

/// No real window/taskbar icon approaches this — winit's own `Icon::from_rgba`
/// already limits a *Windows* icon to `u16::MAX` per side (65535), and macOS/
/// Linux icons are conventionally well under 1024px. The cap forecloses an
/// absurd `width`/`height` (however it arrived — a corrupt decode, a
/// deliberately hostile input) from reaching a platform icon API at all, on
/// top of [`from_rgba`](IconData::from_rgba)'s own overflow-safe size check.
const MAX_ICON_SIDE: u32 = 4096;

impl IconData {
    /// Wrap decoded RGBA8 pixels, or `None` when they do not describe a
    /// `width × height` image: either dimension is zero or exceeds
    /// [`MAX_ICON_SIDE`], or `rgba.len() != width * height * 4`.
    ///
    /// `Option` rather than a `<Type>Error` enum because there is exactly one
    /// failure mode and nothing to match on — and this crate carries no
    /// `thiserror` dependency to add one with (version pins are law). The
    /// caller that decoded the image is the one holding the context worth
    /// reporting.
    ///
    /// **Check order matters.** The dimension cap runs *before* the size
    /// arithmetic below it, so a hostile `width`/`height` is rejected on a
    /// cheap comparison rather than reaching the multiply (or, on a caller
    /// that already allocated `rgba` to match, whatever cost that
    /// allocation carried) at all.
    pub fn from_rgba(rgba: Vec<u8>, width: u32, height: u32) -> Option<Self> {
        if width == 0 || height == 0 || width > MAX_ICON_SIDE || height > MAX_ICON_SIDE {
            return None;
        }
        // Checked, not `u64::from(..) * u64::from(..) * 4`: a u64 product of
        // two u32s can't overflow, but a *third* factor can — width = height =
        // 2^31 multiplies out to exactly 2^64, which wraps to 0 and would
        // validate an empty buffer against a two-billion-pixel image. The
        // `MAX_ICON_SIDE` cap above already forecloses this in practice, but
        // the arithmetic stays checked regardless — a size invariant this
        // load-bearing must not depend on a second, separate check staying in
        // sync with it.
        let expected = u64::from(width)
            .checked_mul(u64::from(height))
            .and_then(|pixels| pixels.checked_mul(4))?;
        if rgba.len() as u64 != expected {
            return None;
        }
        Some(Self {
            rgba,
            width,
            height,
        })
    }

    /// The tightly-packed RGBA8 pixels (`width * height * 4` bytes).
    pub fn rgba(&self) -> &[u8] {
        &self.rgba
    }

    /// The image width in pixels (never zero, never above [`MAX_ICON_SIDE`]).
    pub fn width(&self) -> u32 {
        self.width
    }

    /// The image height in pixels (never zero, never above [`MAX_ICON_SIDE`]).
    pub fn height(&self) -> u32 {
        self.height
    }
}

impl std::fmt::Debug for IconData {
    /// Prints `rgba.len()` rather than the buffer itself (see the type's doc
    /// comment) — the pixel count is all a log line needs to say.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("IconData")
            .field("rgba_len", &self.rgba.len())
            .field("width", &self.width)
            .field("height", &self.height)
            .finish()
    }
}

/// A native menu, as a platform-independent tree: the top-level value is the
/// menu *bar* (whose items are conventionally [`MenuItemSpec::Submenu`]s), and
/// the same type describes each submenu below it.
///
/// Nothing here is rendered by Frust — a per-OS shell translates it into the
/// host's own menu API, and reports an activation back through
/// `frust_reactive::push_menu_event` keyed by the activated item's
/// [`id`](MenuItemSpec::Item::id).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct MenuSpec {
    /// The menu's items, in display order.
    pub items: Vec<MenuItemSpec>,
}

impl MenuSpec {
    /// An empty menu.
    pub fn new() -> Self {
        Self::default()
    }

    /// Append one item, builder-style.
    pub fn with_item(mut self, item: MenuItemSpec) -> Self {
        self.items.push(item);
        self
    }

    /// Whether the menu has no items — a shell installs nothing at all rather
    /// than an empty menu bar.
    pub fn is_empty(&self) -> bool {
        self.items.is_empty()
    }
}

/// One entry in a [`MenuSpec`].
///
/// Deliberately **not** `#[non_exhaustive]`: a per-OS shell must match it
/// exhaustively, so a new variant is a compile error in every shell that would
/// otherwise silently drop the item from the menu it builds.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MenuItemSpec {
    /// An app-defined item. Activating it pushes [`id`](MenuItemSpec::Item::id)
    /// through `frust_reactive::push_menu_event`, which app code observes via
    /// the facade's `menu_events()`.
    Item {
        /// The app's own id for this item, echoed back verbatim on activation.
        id: String,
        /// The text shown in the menu.
        label: String,
        /// An accelerator in the cross-platform `"CmdOrCtrl+Shift+P"` shorthand
        /// (`Cmd`/`Ctrl`/`Alt`/`Shift` + a key, joined by `+`), or `None` for
        /// no keyboard shortcut.
        ///
        /// Kept a string rather than a parsed chord type because this crate
        /// binds no menu library: the per-OS shell parses it with whatever its
        /// menu backend already accepts, and is also where an unparseable
        /// accelerator is reported — validating it twice, in two vocabularies,
        /// would only let the two disagree.
        accelerator: Option<String>,
        /// Whether the item is selectable. `false` renders it greyed out.
        enabled: bool,
    },
    /// A platform-standard item ([`MenuRole`]) the host implements itself —
    /// About/Hide/Quit and friends. No activation is reported for these: the
    /// platform performs the action, so app code has nothing to handle.
    Role {
        /// Which standard action this item performs.
        role: MenuRole,
        /// An override for the platform's own label, or `None` to use it (the
        /// normal case — a localized system label beats a hand-written one).
        label: Option<String>,
    },
    /// A separator line.
    Separator,
    /// A nested menu.
    Submenu {
        /// The text shown for the submenu itself.
        label: String,
        /// The submenu's own contents.
        menu: MenuSpec,
    },
}

impl MenuItemSpec {
    /// An enabled, accelerator-free app item.
    pub fn item(id: impl Into<String>, label: impl Into<String>) -> Self {
        Self::Item {
            id: id.into(),
            label: label.into(),
            accelerator: None,
            enabled: true,
        }
    }

    /// A platform-standard item at the platform's own label.
    pub fn role(role: MenuRole) -> Self {
        Self::Role { role, label: None }
    }

    /// A separator line.
    pub fn separator() -> Self {
        Self::Separator
    }

    /// A nested menu under `label`.
    pub fn submenu(label: impl Into<String>, menu: MenuSpec) -> Self {
        Self::Submenu {
            label: label.into(),
            menu,
        }
    }

    /// Attach a keyboard accelerator (see
    /// [`Item::accelerator`](MenuItemSpec::Item::accelerator)).
    ///
    /// Returns the item unchanged for every other variant: a role item's
    /// shortcut is the platform's own (⌘Q for Quit), and a separator/submenu
    /// has nothing to activate.
    pub fn with_accelerator(mut self, accelerator: impl Into<String>) -> Self {
        if let Self::Item {
            accelerator: slot, ..
        } = &mut self
        {
            *slot = Some(accelerator.into());
        }
        self
    }

    /// Mark the item greyed out. Returns the item unchanged for every variant
    /// but [`MenuItemSpec::Item`] (see [`with_accelerator`](Self::with_accelerator)).
    pub fn disabled(mut self) -> Self {
        if let Self::Item { enabled, .. } = &mut self {
            *enabled = false;
        }
        self
    }

    /// Override the label of a [`MenuItemSpec::Role`] item. Returns the item
    /// unchanged for every other variant (their labels are set at
    /// construction).
    pub fn with_label(mut self, label: impl Into<String>) -> Self {
        if let Self::Role { label: slot, .. } = &mut self {
            *slot = Some(label.into());
        }
        self
    }

    /// The activation id this item reports, or `None` for an item that reports
    /// none (a role item, a separator, a submenu).
    pub fn id(&self) -> Option<&str> {
        match self {
            Self::Item { id, .. } => Some(id),
            Self::Role { .. } | Self::Separator | Self::Submenu { .. } => None,
        }
    }
}

/// A platform-standard menu action the host implements itself.
///
/// The set is the one a macOS application menu is expected to carry (the
/// platform whose menu conventions are strictest); a host that has no notion of
/// a given role simply omits the item rather than faking it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MenuRole {
    /// Show the standard about panel.
    About,
    /// Hide the application.
    Hide,
    /// Hide every other application.
    HideOthers,
    /// Show every hidden application.
    ShowAll,
    /// Minimize the focused window.
    Minimize,
    /// Close the focused window (which is a close *request* — see
    /// `DesktopExtensions::on_close_requested`).
    CloseWindow,
    /// Quit the application.
    Quit,
}

#[cfg(test)]
mod tests {
    use super::*;

    // --- DesktopConfig defaults ---

    #[test]
    fn the_default_config_reproduces_todays_preview_window() {
        let config = DesktopConfig::default();
        // The historical hardcoded title, now the documented fallback.
        assert_eq!(config.window_title(), "Frust");
        assert_eq!(config.app_name, None);
        assert_eq!(config.app_id, None);
        assert_eq!(config.window_icon, None);
        assert_eq!(config.menu_spec, None);
        // A close request quits, exactly as the unconditional
        // `event_loop.exit()` did before this seam existed.
        assert!(config.quit_on_last_window_closed);
        assert_eq!(DesktopConfig::new(), config);
    }

    #[test]
    fn a_configured_app_name_becomes_the_window_title() {
        let config = DesktopConfig::new().with_app_name("Huddle");
        assert_eq!(config.window_title(), "Huddle");
        assert_eq!(config.app_name.as_deref(), Some("Huddle"));
    }

    #[test]
    fn the_builders_set_each_field_independently() {
        let icon = IconData::from_rgba(vec![0; 4], 1, 1).expect("1x1 RGBA is valid");
        let config = DesktopConfig::new()
            .with_app_name("Huddle")
            .with_app_id("dev.frust.huddle")
            .with_window_icon(icon.clone())
            .with_menu_spec(MenuSpec::new().with_item(MenuItemSpec::role(MenuRole::Quit)))
            .with_quit_on_last_window_closed(false);

        assert_eq!(config.app_id.as_deref(), Some("dev.frust.huddle"));
        assert_eq!(config.window_icon, Some(icon));
        assert_eq!(config.menu_spec.map(|m| m.items.len()), Some(1));
        assert!(!config.quit_on_last_window_closed);
    }

    // --- IconData's invariant ---

    #[test]
    fn icon_data_accepts_exactly_width_times_height_times_four_bytes() {
        let icon = IconData::from_rgba(vec![7; 2 * 3 * 4], 2, 3).expect("2x3 RGBA is valid");
        assert_eq!(icon.width(), 2);
        assert_eq!(icon.height(), 3);
        assert_eq!(icon.rgba().len(), 24);
    }

    #[test]
    fn icon_data_rejects_a_buffer_that_does_not_match_its_dimensions() {
        // One byte short of a 2x2 image — the case that reaches a platform API
        // as a buffer overrun rather than a wrong picture.
        assert_eq!(IconData::from_rgba(vec![0; 15], 2, 2), None);
        assert_eq!(IconData::from_rgba(vec![0; 17], 2, 2), None);
    }

    #[test]
    fn icon_data_rejects_a_zero_dimension() {
        assert_eq!(IconData::from_rgba(Vec::new(), 0, 4), None);
        assert_eq!(IconData::from_rgba(Vec::new(), 4, 0), None);
    }

    #[test]
    fn icon_data_rejects_a_size_product_that_overflows_u64() {
        // width = height = 2^31: the naive `u64::from(w) * u64::from(h) * 4`
        // multiplies out to exactly 2^64, which wraps to 0 and would validate
        // an empty buffer against a two-billion-pixel image. The dimension cap
        // rejects this long before the multiply would even run, but the
        // checked arithmetic is what actually closes the overflow — assert
        // `None`, not just "doesn't panic": a debug build already panics on
        // unchecked overflow, so the real regression this guards is a release
        // build silently wrapping to a validated `Some`.
        assert_eq!(IconData::from_rgba(Vec::new(), 1 << 31, 1 << 31), None);
    }

    #[test]
    fn icon_data_rejects_a_dimension_just_over_the_cap() {
        assert_eq!(IconData::from_rgba(Vec::new(), MAX_ICON_SIDE + 1, 1), None);
        assert_eq!(IconData::from_rgba(Vec::new(), 1, MAX_ICON_SIDE + 1), None);
    }

    #[test]
    fn icon_data_accepts_the_max_allowed_dimension() {
        // A real `MAX_ICON_SIDE`-square buffer (67MB) is wasteful to allocate
        // just to prove the cap's boundary is inclusive — a `MAX_ICON_SIDE ×
        // 1` strip already exercises the same `width == MAX_ICON_SIDE` cap
        // comparison, at a 16KB buffer instead.
        let icon = IconData::from_rgba(vec![0; MAX_ICON_SIDE as usize * 4], MAX_ICON_SIDE, 1)
            .expect("MAX_ICON_SIDE is inclusive, not an exclusive bound");
        assert_eq!(icon.width(), MAX_ICON_SIDE);
        assert_eq!(icon.height(), 1);
    }

    // --- MenuSpec construction ---

    #[test]
    fn a_fresh_menu_spec_is_empty() {
        let menu = MenuSpec::new();
        assert!(menu.is_empty());
        assert_eq!(menu, MenuSpec::default());
    }

    #[test]
    fn menu_items_build_the_full_item_vocabulary() {
        let file = MenuSpec::new()
            .with_item(MenuItemSpec::item("file.open", "Open…").with_accelerator("CmdOrCtrl+O"))
            .with_item(MenuItemSpec::item("file.export", "Export").disabled())
            .with_item(MenuItemSpec::separator())
            .with_item(MenuItemSpec::role(MenuRole::Quit));
        let bar = MenuSpec::new().with_item(MenuItemSpec::submenu("File", file.clone()));

        assert!(!bar.is_empty());
        assert_eq!(
            bar.items,
            vec![MenuItemSpec::Submenu {
                label: "File".to_string(),
                menu: file,
            }]
        );
    }

    #[test]
    fn an_app_item_carries_its_id_accelerator_and_enabled_state() {
        let item = MenuItemSpec::item("file.open", "Open…").with_accelerator("CmdOrCtrl+O");
        assert_eq!(
            item,
            MenuItemSpec::Item {
                id: "file.open".to_string(),
                label: "Open…".to_string(),
                accelerator: Some("CmdOrCtrl+O".to_string()),
                enabled: true,
            }
        );
        assert_eq!(item.id(), Some("file.open"));
        assert!(matches!(
            item.disabled(),
            MenuItemSpec::Item { enabled: false, .. }
        ));
    }

    #[test]
    fn only_app_items_report_an_activation_id() {
        // The shell pushes `push_menu_event(id)` for exactly these: a role item
        // is performed by the platform and a separator/submenu activates
        // nothing.
        assert_eq!(MenuItemSpec::role(MenuRole::About).id(), None);
        assert_eq!(MenuItemSpec::separator().id(), None);
        assert_eq!(MenuItemSpec::submenu("File", MenuSpec::new()).id(), None);
    }

    #[test]
    fn the_item_builders_leave_the_wrong_variant_untouched() {
        // Documented no-ops (see each builder's doc comment), pinned so a
        // future edit can't silently start mutating a role/separator instead.
        let role = MenuItemSpec::role(MenuRole::Quit);
        assert_eq!(role.clone().with_accelerator("CmdOrCtrl+Q"), role);
        assert_eq!(role.clone().disabled(), role);
        assert_eq!(MenuItemSpec::separator().with_label("nope"), {
            MenuItemSpec::Separator
        });
    }

    #[test]
    fn a_role_item_can_override_the_platform_label() {
        assert_eq!(
            MenuItemSpec::role(MenuRole::About).with_label("About Huddle"),
            MenuItemSpec::Role {
                role: MenuRole::About,
                label: Some("About Huddle".to_string()),
            }
        );
    }
}
