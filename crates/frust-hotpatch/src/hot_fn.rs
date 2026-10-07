// Ported from subsecond 0.7.10 (DioxusLabs, MIT OR Apache-2.0),
// https://github.com/DioxusLabs/dioxus/tree/main/packages/subsecond.
//
//! The call-side detour: [`HotFn`] looks its function up in the current
//! [`JumpTable`](crate::JumpTable) on every call and jumps to the patched version when one is
//! mapped, else runs the original.

use std::backtrace::Backtrace;
use std::cell::Cell;
use std::marker::PhantomData;
use std::mem::{size_of, transmute, transmute_copy};
use std::panic::AssertUnwindSafe;
use std::sync::atomic::{AtomicU64, Ordering};

use crate::get_jump_table;

/// Calls `f` through the jump table, retrying it when a [`HotFnPanic`] unwinds out of it.
///
/// If `f`'s own code changed, the patched version runs. If code *above* a nested hot call changed,
/// that inner call is stale: it unwinds with a [`HotFnPanic`] to the nearest enclosing `call`,
/// which re-runs its closure. Any other panic resumes unwinding untouched.
pub fn call<O>(mut f: impl FnMut() -> O) -> O {
    // Only run in debug mode - the rest of this function folds away.
    if !cfg!(debug_assertions) {
        return f();
    }

    let mut hotfn = HotFn::current(f);
    loop {
        let res = std::panic::catch_unwind(AssertUnwindSafe(|| hotfn.call(())));
        let err = match res {
            Ok(res) => return res,
            Err(err) => err,
        };
        if err.downcast_ref::<HotFnPanic>().is_none() {
            std::panic::resume_unwind(err);
        }
    }
}

/// The unwind payload of a stale hot call, caught and retried by the next [`call`] up the stack.
#[derive(Debug)]
pub struct HotFnPanic {
    _backtrace: Backtrace,
}

/// The address a hot function currently resolves to.
#[non_exhaustive]
#[derive(PartialEq, Eq, Hash, Clone, Copy, Debug)]
pub struct HotFnPtr(pub u64);

impl HotFnPtr {
    /// Wrap a raw address. The safe way to get one is [`HotFn::ptr_address`].
    ///
    /// # Safety
    ///
    /// `index` must be the address of a valid function.
    pub unsafe fn new(index: u64) -> Self {
        Self(index)
    }
}

/// Patched-dispatch misses since the last patch (see [`fall_through_count`]).
static FALL_THROUGHS: AtomicU64 = AtomicU64::new(0);

thread_local! {
    // Already `const`: the lint misfires on Android, where `thread_local!` expands differently.
    #[allow(clippy::missing_const_for_thread_local)]
    static LAST_FELL_THROUGH: Cell<bool> = const { Cell::new(false) };
}

/// Record one jump-table lookup's outcome: `missed` means a table was installed but held no entry
/// for this call's own address, so the original code ran.
fn record_lookup(missed: bool) {
    LAST_FELL_THROUGH.with(|c| c.set(missed));
    if missed {
        FALL_THROUGHS.fetch_add(1, Ordering::Relaxed);
    }
}

pub(crate) fn reset_fall_throughs() {
    FALL_THROUGHS.store(0, Ordering::Relaxed);
}

/// Whether this thread's most recent [`HotFn`] call found a jump table installed but no mapping
/// for its own address, and so ran the original code.
///
/// A purely diagnostic read; dispatch never consults it. After a patch that changes a generic
/// call's type parameters (e.g. a component's state type), the patched monomorphisation has a new
/// address the running binary never calls, so the old code keeps running: this is how a caller
/// notices. A miss is also normal for any hot function the patch did not recompile, so read it
/// for a call the caller expects the patch to cover.
pub fn last_call_fell_through() -> bool {
    LAST_FELL_THROUGH.with(Cell::get)
}

/// How many [`HotFn`] calls, on any thread, missed the jump table since the last patch was
/// applied (see [`last_call_fell_through`]). Reset to zero by every successful `apply_patch`.
pub fn fall_through_count() -> u64 {
    FALL_THROUGHS.load(Ordering::Relaxed)
}

/// A hot-reloadable function: [`HotFn::call`] runs the newest version the jump table maps.
pub struct HotFn<A, M, F>
where
    F: HotFunction<A, M>,
{
    inner: F,
    _marker: PhantomData<(A, M)>,
}

impl<A, M, F: HotFunction<A, M>> HotFn<A, M, F> {
    /// Wrap `f`; every [`HotFn::call`] resolves it against the current jump table.
    pub const fn current(f: F) -> HotFn<A, M, F> {
        HotFn {
            inner: f,
            _marker: PhantomData,
        }
    }

    /// Call the function with `args`.
    ///
    /// A [`HotFnPanic`] from [`HotFn::try_call`] is raised as a panic whose payload is that
    /// `HotFnPanic`, so it unwinds to the next enclosing [`call`]. Use `try_call` to handle it
    /// here.
    pub fn call(&mut self, args: A) -> F::Return {
        match self.try_call(args) {
            Ok(ret) => ret,
            Err(stale) => std::panic::panic_any(stale),
        }
    }

    /// The address this function currently resolves to, which may differ from the original.
    ///
    /// A runtime that memoizes state across patches can compare it between patches: an unchanged
    /// address means the state "above" the function is still valid. Tracking it over time is the
    /// caller's job.
    pub fn ptr_address(&self) -> HotFnPtr {
        if size_of::<F>() == size_of::<fn()>() {
            // SAFETY: `F` is exactly pointer-sized, so copying its bytes as a `usize` reads the
            // whole value and nothing past it; the value is only reported, never called.
            let ptr: usize = unsafe { transmute_copy(&self.inner) };
            return HotFnPtr(ptr as u64);
        }

        let known_fn_ptr = <F as HotFunction<A, M>>::call_it as *const () as usize;
        // SAFETY: the table reference is used only within this call, never held across a patch.
        if let Some(jump_table) = unsafe { get_jump_table() }
            && let Some(ptr) = jump_table.map.get(&(known_fn_ptr as u64)).copied()
        {
            return HotFnPtr(ptr);
        }

        HotFnPtr(known_fn_ptr as u64)
    }

    /// Call the function with `args`, returning a [`HotFnPanic`] instead of unwinding when it is
    /// stale and cannot be updated in place.
    pub fn try_call(&mut self, args: A) -> Result<F::Return, HotFnPanic> {
        if !cfg!(debug_assertions) {
            return Ok(self.inner.call_it(args));
        }

        // A pointer-sized `F` is a function pointer: it is looked up by its own value. A ZST
        // (a fn item or a capture-less closure) or a larger closure is looked up through its
        // `call_it` monomorphisation, which is a real, patchable function.
        if size_of::<F>() == size_of::<fn()>() {
            // SAFETY: `F` is pointer-sized, which for an `FnMut` is a function pointer; that is
            // `call_as_ptr`'s whole contract.
            return Ok(unsafe { self.inner.call_as_ptr(args) });
        }

        // SAFETY: the table reference is used only within this call, never held across a patch.
        if let Some(jump_table) = unsafe { get_jump_table() } {
            let known_fn_ptr = <F as HotFunction<A, M>>::call_it as *const () as u64;
            if let Some(ptr) = jump_table.map.get(&known_fn_ptr).copied() {
                record_lookup(false);
                // SAFETY: the patch builder mapped `call_it`'s link-time address to the patched
                // `call_it` of this same monomorphisation, so the target has exactly `call_it`'s
                // signature; `apply_patch` rebased it into the loaded library, which is never
                // unloaded. Function pointers need no alignment beyond 1.
                let call_it = unsafe {
                    transmute::<*const (), fn(&mut F, A) -> F::Return>(ptr as usize as *const ())
                };
                return Ok(call_it(&mut self.inner, args));
            }
            record_lookup(true);
        } else {
            record_lookup(false);
        }

        Ok(self.inner.call_it(args))
    }

    /// Call the function through an explicit [`HotFnPtr`] (from [`HotFn::ptr_address`]).
    ///
    /// # Safety
    ///
    /// `ptr` must address a function whose argument layouts have not changed since `ptr` was taken.
    pub unsafe fn try_call_with_ptr(
        &mut self,
        ptr: HotFnPtr,
        args: A,
    ) -> Result<F::Return, HotFnPanic> {
        if !cfg!(debug_assertions) {
            return Ok(self.inner.call_it(args));
        }

        if size_of::<F>() == size_of::<fn()>() {
            // SAFETY: `F` is pointer-sized, which for an `FnMut` is a function pointer.
            return Ok(unsafe { self.inner.call_as_ptr(args) });
        }

        // SAFETY: the caller guarantees `ptr` is a function with `call_it`'s signature for this
        // monomorphisation.
        let call_it = unsafe {
            transmute::<*const (), fn(&mut F, A) -> F::Return>(ptr.0 as usize as *const ())
        };
        Ok(call_it(&mut self.inner, args))
    }
}

/// A function that can be detoured through the jump table: implemented for every `FnMut` of up to
/// nine arguments (taken as a tuple). `FnOnce` is excluded because a patch may re-run the function.
pub trait HotFunction<Args, Marker> {
    /// The function's return type.
    type Return;

    /// The plain function-pointer type `Self` is transmuted to by [`HotFunction::call_as_ptr`].
    type Real;

    /// Call the function with its arguments as a tuple.
    ///
    /// The `rust-call` ABI is unstable, so this wrapper gives every implementing type a stable,
    /// real function per monomorphisation: the address the jump table is keyed on.
    fn call_it(&mut self, args: Args) -> Self::Return;

    /// Call `self` as a function pointer, resolved through the jump table.
    ///
    /// # Safety
    ///
    /// `Self` must be a function pointer (the same size and representation as [`Self::Real`]).
    unsafe fn call_as_ptr(&mut self, args: Args) -> Self::Return;
}

/// Clears the top byte (the aarch64 Android pointer tag) so the address matches the untagged
/// jump-table key; returns the untagged address and the tag to re-apply.
#[cfg(all(target_pointer_width = "64", target_os = "android"))]
#[inline]
fn split_pointer_tag(addr: u64) -> (u64, u64) {
    (addr & 0x00FF_FFFF_FFFF_FFFF, addr & 0xFF00_0000_0000_0000)
}

#[cfg(not(all(target_pointer_width = "64", target_os = "android")))]
#[inline]
fn split_pointer_tag(addr: u64) -> (u64, u64) {
    (addr, 0)
}

macro_rules! impl_hot_function {
    (
        $(
            ($marker:ident, $($arg:ident),*)
        ),*
    ) => {
        $(
            /// Arity marker; hidden to keep [`HotFunction`] effectively sealed.
            #[doc(hidden)]
            pub struct $marker;

            impl<T, $($arg,)* R> HotFunction<($($arg,)*), $marker> for T
            where
                T: FnMut($($arg),*) -> R,
            {
                type Return = R;
                type Real = fn($($arg),*) -> R;

                fn call_it(&mut self, args: ($($arg,)*)) -> Self::Return {
                    #[allow(non_snake_case)]
                    let ( $($arg,)* ) = args;
                    self($($arg),*)
                }

                unsafe fn call_as_ptr(&mut self, args: ($($arg,)*)) -> Self::Return {
                    // SAFETY: the table reference is used only within this call.
                    if let Some(jump_table) = unsafe { get_jump_table() } {
                        // SAFETY: the caller guarantees `Self` is a function pointer, i.e. the
                        // same size and representation as `Self::Real`.
                        let real =
                            unsafe { transmute_copy::<Self, Self::Real>(&*self) } as *const ();
                        // Android may tag the top byte (MTE / pointer tagging); the jump table is
                        // keyed on untagged addresses, so look up without the tag and re-apply it.
                        let (real, tag) = split_pointer_tag(real as usize as u64);
                        if let Some(ptr) = jump_table.map.get(&real).copied() {
                            record_lookup(false);
                            #[allow(non_snake_case)]
                            let ( $($arg,)* ) = args;
                            // SAFETY: the patch builder mapped this function's address to its
                            // patched version with the same signature, rebased into the loaded
                            // (never unloaded) patch library. On 32-bit targets the host still
                            // writes 64-bit addresses, so truncating to `usize` is lossless.
                            let patched = unsafe {
                                let addr = (ptr | tag) as usize as *const ();
                                transmute::<*const (), Self::Real>(addr)
                            };
                            return patched($($arg),*);
                        }
                        record_lookup(true);
                    } else {
                        record_lookup(false);
                    }
                    self.call_it(args)
                }
            }
        )*
    };
}

impl_hot_function!(
    (Fn0Marker,),
    (Fn1Marker, A),
    (Fn2Marker, A, B),
    (Fn3Marker, A, B, C),
    (Fn4Marker, A, B, C, D),
    (Fn5Marker, A, B, C, D, E),
    (Fn6Marker, A, B, C, D, E, F),
    (Fn7Marker, A, B, C, D, E, F, G),
    (Fn8Marker, A, B, C, D, E, F, G, H),
    (Fn9Marker, A, B, C, D, E, F, G, H, I)
);

#[cfg(test)]
mod tests {
    use super::*;

    fn add(a: u32, b: u32) -> u32 {
        a + b
    }

    #[test]
    fn unpatched_calls_run_the_original() {
        assert_eq!(HotFn::current(add).call((2, 3)), 5);
        let fp: fn(u32, u32) -> u32 = add;
        assert_eq!(HotFn::current(fp).call((4, 5)), 9);
        let offset = 10;
        assert_eq!(HotFn::current(|x: u32| x + offset).call((1,)), 11);
        assert_eq!(call(|| 7), 7);
    }

    #[test]
    fn no_table_is_not_a_fall_through() {
        HotFn::current(add).call((1, 1));
        assert!(!last_call_fell_through());
    }

    #[test]
    fn ptr_address_of_a_fn_pointer_is_its_value() {
        let fp: fn(u32, u32) -> u32 = add;
        assert_eq!(HotFn::current(fp).ptr_address().0, fp as usize as u64);
    }

    #[test]
    fn call_retries_after_a_hot_fn_panic() {
        let mut attempts = 0;
        let out = call(|| {
            attempts += 1;
            if attempts == 1 {
                std::panic::panic_any(HotFnPanic {
                    _backtrace: Backtrace::disabled(),
                });
            }
            attempts
        });
        assert_eq!(out, 2);
    }

    #[test]
    fn foreign_panics_propagate_through_call() {
        let res = std::panic::catch_unwind(|| call(|| -> u32 { panic!("not ours") }));
        assert!(res.is_err());
    }

    #[test]
    fn split_pointer_tag_round_trips() {
        let addr = 0x0000_7fff_1234_5678;
        let (untagged, tag) = split_pointer_tag(addr);
        assert_eq!(untagged | tag, addr);
    }
}
