//! The app half of the hot-patch gate fixtures. Built with no features it is
//! the fat build; each cargo feature is one saved edit, built the way a
//! patch's thin build recompiles the crate. An edit touches only its own
//! items, so a gate report names exactly the types that edit reached.

use frust_core::{Component, ComponentWidget, ErasedWidget, Text, column, text};

// HomePage, the spike app's root: `d2-field-add` adds a State field (row D2),
// `d3-stack-wrap` is the observed row D3 edit (root `column()` wrapped in
// `stack()`), `d3-column-wrap` its control. `sentinel-bump` and `new-helper`
// are layout-preserving edits.

#[cfg(not(feature = "d2-field-add"))]
pub struct HomeState {
    pub count: u32,
}

#[cfg(feature = "d2-field-add")]
pub struct HomeState {
    pub count: u32,
    pub extra: u32,
}

pub struct HomePage;

impl Component for HomePage {
    type State = HomeState;
    #[cfg(not(feature = "d3-stack-wrap"))]
    type View = frust_core::FlexView<HomeState>;
    #[cfg(feature = "d3-stack-wrap")]
    type View = frust_core::StackView<HomeState>;

    fn init(&self) -> HomeState {
        HomeState {
            count: 0,
            #[cfg(feature = "d2-field-add")]
            extra: 4242,
        }
    }

    fn build(&self, state: &mut HomeState) -> Self::View {
        let root = column()
            .child(text(sentinel()))
            .child(text(format!("count: {}", shown(state.count))));
        #[cfg(feature = "d2-field-add")]
        let root = root.child(text(format!("extra={}", state.extra)));
        #[cfg(feature = "d3-column-wrap")]
        let root = column().child(root);
        #[cfg(feature = "d3-stack-wrap")]
        let root = frust_core::stack().child(root);
        root
    }
}

fn sentinel() -> &'static str {
    if cfg!(feature = "sentinel-bump") {
        "hotpatch-sentinel: v1"
    } else {
        "hotpatch-sentinel: v0"
    }
}

#[cfg(not(feature = "new-helper"))]
fn shown(count: u32) -> u32 {
    count
}

#[cfg(feature = "new-helper")]
fn shown(count: u32) -> u32 {
    doubled(count)
}

#[cfg(feature = "new-helper")]
fn doubled(n: u32) -> u32 {
    n.wrapping_mul(2)
}

pub fn mount_home() -> ComponentWidget<HomePage> {
    ComponentWidget::mount(&HomePage)
}

pub fn mount_home_erased() -> ErasedWidget<HomePage> {
    ErasedWidget::mount(&HomePage)
}

// Counter: `reorder` swaps two same-size State fields, `return-type` wraps
// its concrete view in a by-value `Padded`.

#[cfg(not(feature = "reorder"))]
pub struct CounterState {
    pub total: u32,
    pub step: u16,
    pub floor: u16,
}

#[cfg(feature = "reorder")]
pub struct CounterState {
    pub total: u32,
    pub floor: u16,
    pub step: u16,
}

pub struct Counter;

impl Component for Counter {
    type State = CounterState;
    #[cfg(not(feature = "return-type"))]
    type View = Text<CounterState>;
    #[cfg(feature = "return-type")]
    type View = frust_core::Padded<Text<CounterState>>;

    fn init(&self) -> CounterState {
        CounterState {
            total: 0,
            step: 1,
            floor: 0,
        }
    }

    fn build(&self, state: &mut CounterState) -> Self::View {
        let label = text(format!(
            "{} (+{}, >={})",
            state.total, state.step, state.floor
        ));
        #[cfg(feature = "return-type")]
        let label = frust_core::padded(label);
        label
    }
}

pub fn mount_counter() -> ComponentWidget<Counter> {
    ComponentWidget::mount(&Counter)
}

// A press handler: `closure-capture` makes its closure capture one more value.

#[cfg(not(feature = "closure-capture"))]
pub fn on_press(step: u32) -> Box<dyn Fn(&mut HomeState)> {
    Box::new(move |state: &mut HomeState| state.count += step)
}

#[cfg(feature = "closure-capture")]
pub fn on_press(step: u32) -> Box<dyn Fn(&mut HomeState)> {
    let ceiling = u64::from(step) * 100;
    Box::new(move |state: &mut HomeState| {
        state.count = (u64::from(state.count + step)).min(ceiling) as u32;
    })
}

// Badge is added by `badge-p1` and gains a field in `badge-p2`.

#[cfg(feature = "badge-p1")]
pub struct Badge {
    pub n: u32,
}

#[cfg(feature = "badge-p2")]
pub struct Badge {
    pub n: u32,
    pub max: u32,
}

#[cfg(feature = "badge-p1")]
pub fn badge(n: u32) -> Badge {
    Badge { n }
}

#[cfg(feature = "badge-p2")]
pub fn badge(n: u32) -> Badge {
    Badge { n, max: 99 }
}

// Token is added by `token-p1`, absent in `token-p2` and re-added with
// another layout by `token-p3`.

#[cfg(feature = "token-p1")]
pub struct Token {
    pub id: u32,
}

#[cfg(feature = "token-p3")]
pub struct Token {
    pub id: u64,
}

#[cfg(any(feature = "token-p1", feature = "token-p3"))]
pub fn token(id: u32) -> Token {
    Token { id: id.into() }
}

// Settings is a component added by `settings-p1`; `settings-p2` changes its
// State type's identity (a named struct becomes a tuple, as in row D).

#[cfg(any(feature = "settings-p1", feature = "settings-p2"))]
pub struct Settings;

#[cfg(feature = "settings-p1")]
pub struct SettingsState {
    pub dark: bool,
}

#[cfg(feature = "settings-p1")]
impl Component for Settings {
    type State = SettingsState;
    type View = Text<SettingsState>;

    fn init(&self) -> SettingsState {
        SettingsState { dark: false }
    }

    fn build(&self, state: &mut SettingsState) -> Self::View {
        text(format!("dark: {}", state.dark))
    }
}

#[cfg(feature = "settings-p2")]
impl Component for Settings {
    type State = (bool, u8);
    type View = Text<(bool, u8)>;

    fn init(&self) -> (bool, u8) {
        (false, 1)
    }

    fn build(&self, state: &mut (bool, u8)) -> Self::View {
        text(format!("dark: {}, scale: {}", state.0, state.1))
    }
}

#[cfg(any(feature = "settings-p1", feature = "settings-p2"))]
pub fn mount_settings() -> ComponentWidget<Settings> {
    ComponentWidget::mount(&Settings)
}
