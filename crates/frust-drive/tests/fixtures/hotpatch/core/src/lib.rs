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

/// A build result held behind a trait object, like frust-core's `AnyView`: a
/// new concrete view type changes nothing in the holder's layout.
pub struct AnyView<S> {
    inner: Box<dyn View<S>>,
}

impl<S> AnyView<S> {
    pub fn new(view: impl View<S> + 'static) -> Self {
        Self {
            inner: Box::new(view),
        }
    }

    pub fn describe(&self) -> String {
        self.inner.describe()
    }
}

/// The retained child element and its geometry, like frust-core's `ChildPod`.
pub struct ChildPod {
    element: AnyView<()>,
    origin: [f64; 2],
    size: [f64; 2],
    captured: bool,
}

impl ChildPod {
    fn new() -> Self {
        Self {
            element: AnyView::new(Empty),
            origin: [0.0; 2],
            size: [0.0; 2],
            captured: false,
        }
    }
}

struct Empty;

impl View<()> for Empty {
    fn describe(&self) -> String {
        String::new()
    }
}

/// A reactive owner handle, like frust-core's `Owner`.
pub struct Owner {
    id: u64,
}

/// A retained component with the layout of the real `frust-core`
/// `ComponentWidget` under the `hotpatch` feature (`crates/frust-core/src/
/// component.rs:133-162`): `repr(C)`, the seam witness first, the state boxed,
/// and both the previous view and the child element erased. A new concrete
/// view type therefore reaches the layout gate only as new types.
#[repr(C)]
pub struct ComponentWidget<C: Component> {
    witness: hotpatch::SeamWitness,
    state: Option<Box<C::State>>,
    prev: AnyView<C::State>,
    child: ChildPod,
    owner: Owner,
    next_id: u64,
    disposed: bool,
}

impl<C: Component> ComponentWidget<C> {
    pub fn mount(component: &C) -> Self {
        let witness = hotpatch::SeamWitness::of::<C>();
        let mut state = Box::new(component.init());
        let prev = hotpatch::build_erased(component, &mut state, witness);
        Self {
            witness,
            state: Some(state),
            prev: AnyView::new(prev),
            child: ChildPod::new(),
            owner: Owner { id: 1 },
            next_id: 1,
            disposed: false,
        }
    }

    pub fn describe(&self) -> String {
        format!(
            "{} disposed={} owner={} next={} captured={} {:?}{:?}",
            self.prev.describe(),
            self.disposed,
            self.owner.id,
            self.next_id,
            self.child.captured,
            (self.child.origin, self.witness),
            (self.child.size, self.child.element.describe()),
        )
    }

    pub fn state(&self) -> &C::State {
        self.state.as_deref().expect("live widget has state")
    }
}

/// The contrast: a component widget that keeps its last build result by
/// value, so the concrete view type is part of this widget's layout. The real
/// crate no longer has this shape (its `prev` is an erased `AnyView`); it
/// stays here to show what the gate would refuse in that world. The state is
/// boxed as in the real widget, so only the view type moves its layout.
#[repr(C)]
pub struct ByValueWidget<C: Component> {
    witness: hotpatch::SeamWitness,
    state: Option<Box<C::State>>,
    prev: C::View,
    disposed: bool,
}

impl<C: Component> ByValueWidget<C> {
    pub fn mount(component: &C) -> Self {
        let witness = hotpatch::SeamWitness::of::<C>();
        let mut state = Box::new(component.init());
        let prev = hotpatch::build_erased(component, &mut state, witness);
        Self {
            witness,
            state: Some(state),
            prev,
            disposed: false,
        }
    }

    pub fn describe(&self) -> String {
        format!("{} disposed={}", self.prev.describe(), self.disposed)
    }

    pub fn state(&self) -> &C::State {
        self.state.as_deref().expect("live widget has state")
    }
}

pub mod hotpatch {
    use super::Component;
    use frust_hotpatch::hot_fn::HotFunction;

    /// The size and alignment of a component's state in the creating image.
    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    #[repr(C)]
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
