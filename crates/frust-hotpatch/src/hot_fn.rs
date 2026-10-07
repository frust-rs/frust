// Ported from subsecond 0.7.10 (DioxusLabs, MIT OR Apache-2.0),
// https://github.com/DioxusLabs/dioxus/tree/main/packages/subsecond.
//
//! The call-side detour: in a debug build, [`HotFn`] looks its function up in the installed
//! [`JumpTable`](crate::JumpTable) on every call and jumps to the patched version when one is
//! mapped, else runs the original.
//!
//! The lookup key is the address of the function's [`HotFunction::call_it`] monomorphisation, a
//! real function per closure or fn-item type, so any `FnMut` dispatches soundly. A plain function
//! pointer can instead be keyed on its own value, but only through the explicitly typed
//! [`HotFn::from_fn_ptr`]; nothing infers it from a type's size.

use std::cell::Cell;
use std::convert::Infallible;
use std::marker::PhantomData;
use std::mem::transmute;
use std::sync::atomic::{AtomicU64, Ordering};

use crate::{AddressMap, get_jump_table};

/// Call `f` once through the jump table.
///
/// In a debug build, if the installed table maps `f`'s `call_it` address, the patched version
/// runs; otherwise `f` itself does. There is no stale-call detection and no retry: a panic from
/// `f` unwinds through `call` untouched, and code already running when a patch lands finishes as
/// the old code. Equivalent to `HotFn::current(f).call(())`; a release build calls `f` directly.
pub fn call<O>(f: impl FnMut() -> O) -> O {
    HotFn::current(f).call(())
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
/// for its own key, and so ran the function compiled into the calling image.
///
/// A purely diagnostic read; dispatch never consults it. The key is the address of `call_it` as
/// seen by the image the call is made from, and the table's keys are the running executable's
/// (base image) addresses. So a miss means one of three things:
///
/// - Stale code: after a patch that changes a generic call's type parameters (e.g. a component's
///   state type), the patched monomorphisation has a new symbol the running binary never calls,
///   so the base caller's old code keeps running.
/// - Nothing to patch: a hot function the patch did not recompile has no entry.
/// - A benign patch-image caller: a call made from patch code (e.g. a patched component's rebuild
///   calling a nested component's hot function) computes the patch image's own `call_it`
///   address, which is never a key. A call from the newest patch's code already runs the newest
///   code; one from an older patch's code still reachable through its vtables runs that older
///   patch's version.
///
/// This crate records only the outcome, not the missed key, so it cannot tell these apart: read
/// it for a call made from base-image code that the caller expects the patch to cover.
pub fn last_call_fell_through() -> bool {
    LAST_FELL_THROUGH.with(Cell::get)
}

/// How many [`HotFn`] calls, on any thread, missed the jump table since the last patch was
/// applied (see [`last_call_fell_through`]). Reset to zero by every successful `apply_patch`.
///
/// The count includes the benign misses of calls made from patch-image code, so after a patch
/// to an app with nested components it is normally non-zero. It stays a plain count in this
/// crate; telling a stale-code miss from a benign one needs the missed key (whose address range
/// names the calling image), which a restart rule built on this diagnostic must record instead.
pub fn fall_through_count() -> u64 {
    FALL_THROUGHS.load(Ordering::Relaxed)
}

/// Whether lookups strip the top-byte pointer tag. Only aarch64 Android tags pointers (MTE / TBI);
/// the jump table is keyed on untagged addresses.
const STRIP_POINTER_TAG: bool = cfg!(all(target_pointer_width = "64", target_os = "android"));

/// Split `addr` into its untagged address and its top-byte tag (the aarch64 Android pointer tag).
fn split_pointer_tag(addr: u64) -> (u64, u64) {
    (addr & 0x00FF_FFFF_FFFF_FFFF, addr & 0xFF00_0000_0000_0000)
}

/// Look `addr` up in `map`. With `strip_tag`, the lookup uses the untagged address and the tag is
/// re-applied to the patched address it returns.
fn resolve(map: &AddressMap, addr: u64, strip_tag: bool) -> Option<u64> {
    let (key, tag) = if strip_tag {
        split_pointer_tag(addr)
    } else {
        (addr, 0)
    };
    map.get(&key).map(|patched| patched | tag)
}

/// The jump-table key of `F`'s [`HotFunction::call_it`] monomorphisation.
fn call_it_address<A, M, F: HotFunction<A, M>>() -> u64 {
    <F as HotFunction<A, M>>::call_it as *const () as usize as u64
}

/// The fn-pointer operations a [`HotFn::from_fn_ptr`] value dispatches through. Only that
/// constructor builds one, and its `F: FnPointer` bound proves `F` is a plain function pointer.
struct FnPointerOps<A, R, F> {
    address: fn(&F) -> u64,
    call_at: unsafe fn(u64, A) -> R,
}

/// A hot-reloadable function: [`HotFn::call`] runs the newest version the jump table maps.
pub struct HotFn<A, M, F>
where
    F: HotFunction<A, M>,
{
    inner: F,
    /// `Some` only when built by [`HotFn::from_fn_ptr`]: dispatch keys on the pointer's value.
    by_value: Option<FnPointerOps<A, F::Return, F>>,
    _marker: PhantomData<(A, M)>,
}

impl<A, M, F: HotFunction<A, M>> HotFn<A, M, F> {
    /// Wrap `f`, keyed on its `call_it` monomorphisation. Sound for any `FnMut`: a fn item, a
    /// closure of any size, or a function pointer (which then dispatches as one `call_it` shared
    /// by every pointer of that type; use [`HotFn::from_fn_ptr`] to key on the pointer itself).
    pub const fn current(f: F) -> HotFn<A, M, F> {
        HotFn {
            inner: f,
            by_value: None,
            _marker: PhantomData,
        }
    }

    /// Wrap the function pointer `f`, keyed on its own value: the jump table maps the pointed-to
    /// function's address to its patched version. `F` must be a non-higher-ranked
    /// `fn(A, ..) -> R` of up to nine arguments ([`FnPointer`]); wrap anything else with
    /// [`HotFn::current`].
    pub fn from_fn_ptr(f: F) -> HotFn<A, M, F>
    where
        F: FnPointer<A, M>,
    {
        HotFn {
            inner: f,
            by_value: Some(FnPointerOps {
                address: <F as FnPointer<A, M>>::address,
                call_at: <F as FnPointer<A, M>>::call_at,
            }),
            _marker: PhantomData,
        }
    }

    /// Call the function with `args`.
    ///
    /// In a debug build this is one jump-table lookup (see [`HotFn::try_call`]): the patched
    /// version runs when the installed table maps this function's key, else the original. No
    /// stale-call detection, no retry; panics propagate untouched. A release build calls the
    /// function directly and never reads the table.
    pub fn call(&mut self, args: A) -> F::Return {
        match self.try_call(args) {
            Ok(ret) => ret,
            Err(never) => match never {},
        }
    }

    /// The address a call would currently reach: the jump table's mapping for this function's key
    /// when one is installed and maps it, else the key itself. The key is the `call_it` address,
    /// or for [`HotFn::from_fn_ptr`] the pointer's value. A release build never reads the table
    /// and always reports the key.
    ///
    /// A runtime that memoizes state across patches can compare it between patches: an unchanged
    /// address means the state "above" the function is still valid. Tracking it over time is the
    /// caller's job.
    pub fn ptr_address(&self) -> HotFnPtr {
        let key = self.key();
        if !cfg!(debug_assertions) {
            return HotFnPtr(key);
        }
        // SAFETY: the table reference is used only within this call, never held across a patch.
        let resolved = unsafe { get_jump_table() }
            .and_then(|table| resolve(&table.map, key, STRIP_POINTER_TAG));
        HotFnPtr(resolved.unwrap_or(key))
    }

    /// Call the function with `args` through one jump-table lookup.
    ///
    /// Debug builds only: the key (the `call_it` address, or for [`HotFn::from_fn_ptr`] the
    /// pointer's value, with the aarch64 Android pointer tag stripped) is looked up in the
    /// installed table; a hit calls the patched address, a miss or an empty table calls the
    /// original, and the outcome is recorded for [`last_call_fell_through`] /
    /// [`fall_through_count`]. A release build calls the function directly.
    ///
    /// Never fails: the `Result` keeps subsecond's call shape, and the error type is
    /// [`Infallible`].
    pub fn try_call(&mut self, args: A) -> Result<F::Return, Infallible> {
        if !cfg!(debug_assertions) {
            return Ok(self.inner.call_it(args));
        }

        let key = self.key();
        // SAFETY: the table reference is used only within this block, never held across a patch.
        let target = match unsafe { get_jump_table() } {
            None => {
                record_lookup(false);
                None
            }
            Some(table) => {
                let target = resolve(&table.map, key, STRIP_POINTER_TAG);
                record_lookup(target.is_none());
                target
            }
        };

        match target {
            // SAFETY: the lookup above found `target` as the installed table's entry for this
            // function's own key. Tables are installed only by `apply_patch`, whose safety
            // contract requires two things of every entry: it is the patched version of the keyed
            // function in a table built against this exact executable, and every argument, return
            // and capture type of that function (here `A`, `F::Return` and `F` itself, a
            // closure's captures included) keeps its layout between the running image and the
            // patch. Together they give `target` the signature and the argument layouts
            // `call_at` passes for this `HotFn`. It lives in the loaded patch library, which is
            // never unloaded.
            Some(target) => Ok(unsafe { self.call_at(target, args) }),
            None => Ok(self.inner.call_it(args)),
        }
    }

    /// Call the function at `ptr` directly, bypassing the jump table.
    ///
    /// `ptr` is consulted on every call, in every build profile (release included): the table is
    /// never read and no fall-through is recorded.
    ///
    /// # Safety
    ///
    /// For a [`HotFn::current`] value, `ptr` must be this monomorphisation's `call_it` (signature
    /// `fn(&mut F, A) -> F::Return`) or its patched equivalent. For a [`HotFn::from_fn_ptr`]
    /// value, `ptr` must be a function of type `F`: the pointer's own target or its patched
    /// equivalent. Either way the argument and return layouts (for [`HotFn::current`], `F`'s own
    /// layout too: a closure's captures) must not have changed since `ptr` was taken.
    /// [`HotFn::ptr_address`] on a `HotFn` built the same way returns such an address.
    pub unsafe fn try_call_with_ptr(
        &mut self,
        ptr: HotFnPtr,
        args: A,
    ) -> Result<F::Return, Infallible> {
        // SAFETY: this function's contract is exactly `call_at`'s.
        Ok(unsafe { self.call_at(ptr.0, args) })
    }

    /// The jump-table key: the pointer's value for a [`HotFn::from_fn_ptr`] value, else the
    /// `call_it` address.
    fn key(&self) -> u64 {
        match &self.by_value {
            Some(ops) => (ops.address)(&self.inner),
            None => call_it_address::<A, M, F>(),
        }
    }

    /// Call the function at `address`.
    ///
    /// # Safety
    ///
    /// For a `by_value` (fn-pointer) `HotFn`, `address` must be a function of type `F`; otherwise
    /// it must be a function with `call_it`'s signature for this monomorphisation.
    unsafe fn call_at(&mut self, address: u64, args: A) -> F::Return {
        if let Some(ops) = &self.by_value {
            // SAFETY: `ops` exists only for a `from_fn_ptr` value, so `F` is a plain function
            // pointer and `ops.call_at` is its `FnPointer::call_at`, whose contract (`address`
            // is a function of type `F`) is this function's.
            return unsafe { (ops.call_at)(address, args) };
        }
        // SAFETY: the caller guarantees `address` has `call_it`'s exact signature for this
        // monomorphisation. On 32-bit targets the host still writes 64-bit addresses, so
        // truncating to `usize` is lossless; function pointers need no alignment beyond 1.
        let call_it = unsafe {
            transmute::<*const (), fn(&mut F, A) -> F::Return>(address as usize as *const ())
        };
        call_it(&mut self.inner, args)
    }
}

/// A function that can be detoured through the jump table: implemented for every `FnMut` of up to
/// nine arguments (taken as a tuple). `FnOnce` is excluded because a patch may re-run the function.
pub trait HotFunction<Args, Marker> {
    /// The function's return type.
    type Return;

    /// Call the function with its arguments as a tuple.
    ///
    /// The `rust-call` ABI is unstable, so this wrapper gives every implementing type a stable,
    /// real function per monomorphisation: the address the jump table is keyed on.
    fn call_it(&mut self, args: Args) -> Self::Return;
}

mod sealed {
    /// Restricts [`FnPointer`](super::FnPointer) to the plain function-pointer types below.
    pub trait Sealed {}
}

/// A plain, non-higher-ranked function-pointer type `fn(A, ..) -> R` of up to nine arguments: the
/// only types [`HotFn::from_fn_ptr`] accepts. Sealed; implemented by this crate only.
pub trait FnPointer<Args, Marker>: HotFunction<Args, Marker> + Copy + sealed::Sealed {
    /// The function's address, the jump-table key.
    fn address(&self) -> u64;

    /// Call the function at `address` as `Self`.
    ///
    /// # Safety
    ///
    /// `address` must be a function of exactly type `Self`.
    unsafe fn call_at(address: u64, args: Args) -> Self::Return;
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

                fn call_it(&mut self, args: ($($arg,)*)) -> Self::Return {
                    #[allow(non_snake_case)]
                    let ( $($arg,)* ) = args;
                    self($($arg),*)
                }
            }

            impl<$($arg,)* R> sealed::Sealed for fn($($arg),*) -> R {}

            impl<$($arg,)* R> FnPointer<($($arg,)*), $marker> for fn($($arg),*) -> R {
                fn address(&self) -> u64 {
                    *self as usize as u64
                }

                unsafe fn call_at(address: u64, args: ($($arg,)*)) -> R {
                    // SAFETY: the caller guarantees `address` is a function of exactly this type.
                    // On 32-bit targets the host still writes 64-bit addresses, so truncating to
                    // `usize` is lossless; function pointers need no alignment beyond 1.
                    let f = unsafe {
                        transmute::<*const (), fn($($arg),*) -> R>(address as usize as *const ())
                    };
                    #[allow(non_snake_case)]
                    let ( $($arg,)* ) = args;
                    f($($arg),*)
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
    use crate::JumpTable;
    use crate::patch::test_support;

    fn add(a: u32, b: u32) -> u32 {
        a + b
    }

    fn mul(a: u32, b: u32) -> u32 {
        a * b
    }

    /// A stand-in for the patched `call_it` of `F`: the same signature, different behaviour.
    fn patched_call_it<F>(_f: &mut F, args: (u32, u32)) -> u32 {
        args.0 * args.1
    }

    fn call_it_key_of<A, M, F: HotFunction<A, M>>(_f: &F) -> u64 {
        call_it_address::<A, M, F>()
    }

    fn patched_call_it_of<F>(_f: &F) -> u64 {
        patched_call_it::<F> as *const () as usize as u64
    }

    fn table(entries: &[(u64, u64)]) -> JumpTable {
        JumpTable {
            lib: "/nonexistent/frust-hotpatch-test.so".into(),
            map: entries.iter().copied().collect(),
            aslr_reference: 0,
            new_base_address: 0,
            ifunc_count: 0,
        }
    }

    #[test]
    fn unpatched_calls_run_the_original() {
        let _serial = test_support::serial();
        test_support::reset();
        assert_eq!(HotFn::current(add).call((2, 3)), 5);
        let fp: fn(u32, u32) -> u32 = add;
        assert_eq!(HotFn::current(fp).call((4, 5)), 9);
        assert_eq!(HotFn::from_fn_ptr(fp).call((4, 5)), 9);
        let offset = 10;
        assert_eq!(HotFn::current(|x: u32| x + offset).call((1,)), 11);
        assert_eq!(call(|| 7), 7);
    }

    #[test]
    fn no_table_is_not_a_fall_through() {
        let _serial = test_support::serial();
        test_support::reset();
        HotFn::current(add).call((1, 1));
        assert!(!last_call_fell_through());
    }

    #[test]
    fn ptr_address_of_a_fn_pointer_is_its_value() {
        let _serial = test_support::serial();
        test_support::reset();
        let fp: fn(u32, u32) -> u32 = add;
        assert_eq!(HotFn::from_fn_ptr(fp).ptr_address().0, fp as usize as u64);
        // Through `current`, a fn pointer is keyed on its `call_it` like any other `FnMut`.
        assert_eq!(HotFn::current(fp).ptr_address().0, call_it_key_of(&fp));
    }

    #[test]
    #[cfg_attr(
        not(debug_assertions),
        ignore = "dispatch reads the table only in debug builds; run `cargo test -p frust-hotpatch`"
    )]
    fn patched_dispatch_hits_misses_and_resets() {
        let _serial = test_support::serial();
        test_support::reset();

        // Hit: the table maps `add`'s `call_it` to a stand-in with the same signature.
        let key = call_it_key_of(&add);
        let target = patched_call_it_of(&add);
        test_support::install(table(&[(key, target)]));
        assert_eq!(HotFn::current(add).call((3, 4)), 12);
        assert!(!last_call_fell_through());
        assert_eq!(fall_through_count(), 0);
        assert_eq!(HotFn::current(add).ptr_address().0, target);

        // Miss: `mul`'s `call_it` is a different monomorphisation, absent from the table.
        assert_eq!(HotFn::current(mul).call((3, 4)), 12);
        assert!(last_call_fell_through());
        assert_eq!(fall_through_count(), 1);
        assert_eq!(HotFn::current(mul).ptr_address().0, call_it_key_of(&mul));

        // A newer patch resets the miss count.
        test_support::install(table(&[(key, target)]));
        assert_eq!(fall_through_count(), 0);
        assert_eq!(HotFn::current(add).call((2, 5)), 10);
        assert!(!last_call_fell_through());

        test_support::reset();
    }

    #[test]
    #[cfg_attr(
        not(debug_assertions),
        ignore = "dispatch reads the table only in debug builds; run `cargo test -p frust-hotpatch`"
    )]
    fn fn_pointer_dispatch_keys_on_the_pointer_value() {
        let _serial = test_support::serial();
        test_support::reset();

        let fp: fn(u32, u32) -> u32 = add;
        let patched: fn(u32, u32) -> u32 = mul;
        test_support::install(table(&[(fp as usize as u64, patched as usize as u64)]));
        assert_eq!(HotFn::from_fn_ptr(fp).call((3, 4)), 12);
        assert!(!last_call_fell_through());
        // `current` keys on the shared `call_it`, which this table does not map.
        assert_eq!(HotFn::current(fp).call((3, 4)), 7);
        assert!(last_call_fell_through());

        test_support::reset();
    }

    #[test]
    fn try_call_with_ptr_calls_the_given_address() {
        let _serial = test_support::serial();
        test_support::reset();
        let ptr = HotFnPtr(patched_call_it_of(&add));
        // SAFETY: `patched_call_it::<F>` has exactly `call_it`'s signature for `add`'s fn item.
        let out = unsafe { HotFn::current(add).try_call_with_ptr(ptr, (3, 4)) };
        assert_eq!(out, Ok(12));

        let fp: fn(u32, u32) -> u32 = add;
        let patched: fn(u32, u32) -> u32 = mul;
        // SAFETY: `mul` is a function of exactly type `fn(u32, u32) -> u32`.
        let out = unsafe {
            HotFn::from_fn_ptr(fp).try_call_with_ptr(HotFnPtr(patched as usize as u64), (3, 4))
        };
        assert_eq!(out, Ok(12));
    }

    fn decoy() -> usize {
        1
    }

    fn wrong_target() -> usize {
        999
    }

    #[test]
    #[cfg_attr(
        not(debug_assertions),
        ignore = "dispatch reads the table only in debug builds; run `cargo test -p frust-hotpatch`"
    )]
    fn pointer_sized_closure_is_not_a_fn_pointer() {
        let _serial = test_support::serial();
        test_support::reset();

        // A capturing closure exactly as large as a fn pointer, whose captured value is itself a
        // mapped function address: treating its bytes as a fn pointer would jump to `wrong_target`.
        let n: usize = decoy as *const () as usize;
        assert_eq!(
            std::mem::size_of_val(&move || n),
            std::mem::size_of::<fn()>()
        );

        assert_eq!(call(move || n), n);
        assert!(!last_call_fell_through());

        let wrong: fn() -> usize = wrong_target;
        test_support::install(table(&[(n as u64, wrong as usize as u64)]));
        assert_eq!(call(move || n), n);
        assert!(last_call_fell_through());

        test_support::reset();
    }

    #[test]
    fn panics_propagate_through_call() {
        let _serial = test_support::serial();
        let res = std::panic::catch_unwind(|| call(|| -> u32 { panic!("not ours") }));
        assert!(res.is_err());
    }

    #[test]
    fn split_pointer_tag_masks_the_top_byte() {
        let tagged = 0xb400_7fff_1234_5678;
        let (untagged, tag) = split_pointer_tag(tagged);
        assert_eq!(untagged, 0x0000_7fff_1234_5678);
        assert_eq!(tag, 0xb400_0000_0000_0000);
        assert_eq!(untagged | tag, tagged);
    }

    #[test]
    fn tagged_lookup_strips_and_reapplies_the_tag() {
        let map: AddressMap = [(0x0000_7fff_1234_5678, 0x0000_7fff_9999_0000)]
            .into_iter()
            .collect();
        let tagged = 0xb400_7fff_1234_5678;
        assert_eq!(
            resolve(&map, tagged, true),
            Some(0xb400_7fff_9999_0000),
            "the aarch64 Android path looks up untagged and re-tags the target"
        );
        assert_eq!(
            resolve(&map, tagged, false),
            None,
            "untagged path keys verbatim"
        );
        assert_eq!(
            resolve(&map, 0x0000_7fff_1234_5678, true),
            Some(0x0000_7fff_9999_0000)
        );
    }
}
