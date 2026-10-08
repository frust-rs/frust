//! Stand-in for `frust-core`: just enough of its shape for an app crate's
//! DWARF and symbols to look like a real app's to the hot-patch gates.
//! Retained widgets are generic over the app's component, so the app crate
//! instantiates them, and every build goes through the `checked_build` seam
//! called as `HotFunction::call_it`.

use std::marker::PhantomData;

/// A declarative view over state `S`.
pub trait View<S> {
    fn describe(&self) -> String;
}

/// An app component: its own state type and the concrete view it builds.
pub trait Component: 'static {
    type State: 'static;
    type View: View<Self::State> + 'static;

    fn init(&self) -> Self::State;

    fn build(&self, state: &mut Self::State) -> Self::View;
}

/// A text leaf.
pub struct Text<S> {
    label: String,
    size: f32,
    _state: PhantomData<fn(&S)>,
}

pub fn text<S>(label: impl Into<String>) -> Text<S> {
    Text {
        label: label.into(),
        size: 14.0,
        _state: PhantomData,
    }
}

impl<S> View<S> for Text<S> {
    fn describe(&self) -> String {
        format!("text({}, {})", self.label, self.size)
    }
}

/// A flex column; its children are erased, so nesting keeps the type.
pub struct FlexView<S> {
    children: Vec<Box<dyn View<S>>>,
    gap: f64,
}

pub fn column<S>() -> FlexView<S> {
    FlexView {
        children: Vec::new(),
        gap: 8.0,
    }
}

impl<S> FlexView<S> {
    pub fn child(mut self, view: impl View<S> + 'static) -> Self {
        self.children.push(Box::new(view));
        self
    }
}

impl<S> View<S> for FlexView<S> {
    fn describe(&self) -> String {
        let parts: Vec<String> = self.children.iter().map(|c| c.describe()).collect();
        format!("column[{}; {}]", self.gap, parts.join(", "))
    }
}

/// A z-stack; a different concrete type from [`FlexView`].
pub struct StackView<S> {
    children: Vec<Box<dyn View<S>>>,
    alignment: u8,
    clip: bool,
}

pub fn stack<S>() -> StackView<S> {
    StackView {
        children: Vec::new(),
        alignment: 4,
        clip: false,
    }
}

impl<S> StackView<S> {
    pub fn child(mut self, view: impl View<S> + 'static) -> Self {
        self.children.push(Box::new(view));
        self
    }
}

impl<S> View<S> for StackView<S> {
    fn describe(&self) -> String {
        let parts: Vec<String> = self.children.iter().map(|c| c.describe()).collect();
        format!(
            "stack[{}, {}; {}]",
            self.alignment,
            self.clip,
            parts.join(", ")
        )
    }
}

/// A wrapper holding its child by value.
pub struct Padded<V> {
    inner: V,
    insets: [f32; 4],
}

pub fn padded<V>(inner: V) -> Padded<V> {
    Padded {
        inner,
        insets: [8.0; 4],
    }
}

impl<S, V: View<S>> View<S> for Padded<V> {
    fn describe(&self) -> String {
        format!("padded({:?}, {})", self.insets, self.inner.describe())
    }
}

/// A retained component that keeps its last build result by value, so
/// the concrete view type is part of this widget's layout.
pub struct ComponentWidget<C: Component> {
    state: C::State,
    prev: C::View,
    disposed: bool,
}

impl<C: Component> ComponentWidget<C> {
    pub fn mount(component: &C) -> Self {
        let mut state = component.init();
        let prev = hotpatch::build_erased(component, &mut state, hotpatch::SeamWitness::of::<C>());
        Self {
            state,
            prev,
            disposed: false,
        }
    }

    pub fn describe(&self) -> String {
        format!("{} disposed={}", self.prev.describe(), self.disposed)
    }

    pub fn state(&self) -> &C::State {
        &self.state
    }
}

/// A retained component that keeps its last build result boxed behind a
/// trait object, so its layout is the same whatever the view type is.
pub struct ErasedWidget<C: Component> {
    state: C::State,
    prev: Box<dyn View<C::State>>,
}

impl<C: Component> ErasedWidget<C> {
    pub fn mount(component: &C) -> Self {
        let mut state = component.init();
        let prev = hotpatch::build_erased(component, &mut state, hotpatch::SeamWitness::of::<C>());
        Self {
            state,
            prev: Box::new(prev),
        }
    }

    pub fn describe(&self) -> String {
        self.prev.describe()
    }

    pub fn state(&self) -> &C::State {
        &self.state
    }
}

pub mod hotpatch {
    use super::Component;
    use frust_hotpatch::hot_fn::HotFunction;

    /// The size and alignment of a component's state in the creating image.
    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    pub struct SeamWitness {
        pub size: usize,
        pub align: usize,
    }

    impl SeamWitness {
        pub fn of<C: Component>() -> Self {
            Self {
                size: size_of::<C::State>(),
                align: align_of::<C::State>(),
            }
        }
    }

    /// Runs `c.build(s)` through the seam, like frust-core's `build_erased`.
    pub fn build_erased<C: Component>(c: &C, s: &mut C::State, witness: SeamWitness) -> C::View {
        let mut hot = checked_build::<C>;
        hot.call_it((c, s, witness))
    }

    fn checked_build<C: Component>(c: &C, s: &mut C::State, witness: SeamWitness) -> C::View {
        assert_eq!(witness, SeamWitness::of::<C>());
        c.build(s)
    }
}
