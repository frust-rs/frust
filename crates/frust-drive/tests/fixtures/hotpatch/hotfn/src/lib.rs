//! Stand-in for `frust-hotpatch`'s `HotFunction`: the trait whose `call_it`
//! monomorphisation is the jump-table key the identity gate reads.

pub mod hot_fn {
    /// A function called through the jump table, its arguments as a tuple.
    pub trait HotFunction<Args, Marker> {
        type Return;

        fn call_it(&mut self, args: Args) -> Self::Return;
    }

    /// Arity marker for three-argument functions.
    pub struct Fn3Marker;

    impl<T, A, B, C, R> HotFunction<(A, B, C), Fn3Marker> for T
    where
        T: FnMut(A, B, C) -> R,
    {
        type Return = R;

        fn call_it(&mut self, args: (A, B, C)) -> R {
            let (a, b, c) = args;
            self(a, b, c)
        }
    }
}
