//! Accessibility: the accesskit Android adapter this handle attaches to the
//! host `FrustSurfaceView`, the two UI-thread-shared channels its handlers talk
//! to the frame loop through, and the handle's attach/drain/publish arms.

use std::collections::VecDeque;
use std::sync::{Arc, Mutex};

use accesskit_android::InjectingAdapter;
use accesskit_android::jni as ak_jni;
use frust_core::SemanticsUpdate;
use frust_core::accesskit::{
    Action, ActionHandler, ActionRequest, ActivationHandler, NodeId, Tree, TreeId, TreeUpdate,
};

use super::AndroidAppHandle;
use super::input::under_root_owner;

/// Per-handle accessibility state: the injecting accesskit Android
/// adapter plus the two UI-thread-shared channels its handlers use to talk to
/// the frame loop.
///
/// # Threading model (verified against `accesskit_android` 0.7.5)
///
/// Every accesskit callback into this shell — `request_initial_tree` (via
/// `AccessibilityNodeProvider.createAccessibilityNodeInfo`/`findFocus`) and
/// `do_action` (via `performAction`) — is dispatched by the Android
/// accessibility framework on the app's **main (UI) thread**, the very thread
/// the Choreographer frame loop and JNI touch/IME entry points already run on.
/// So there is no true concurrency: the two `Mutex`es below carry only the
/// `Send` bound the `ActionHandler`/`ActivationHandler` traits require (both are
/// `Send + 'static` and owned by the adapter), never real contention. Crucially,
/// neither handler ever touches the [`AndroidAppHandle`]: `do_action` only
/// enqueues, and the `&mut self` frame pass drains that queue and calls
/// [`AppTree::perform_accessibility_action`](frust_shell_common::AppTree::perform_accessibility_action)
/// itself — so an assistive-tech action can never alias the handle mid-frame.
pub(super) struct AndroidA11y {
    /// The accesskit_android injecting adapter. On construction it attaches a
    /// `View.AccessibilityDelegate` + `OnHoverListener` to the host
    /// `FrustSurfaceView` (posted to the UI thread), and it pushes
    /// `TreeUpdate`s via [`InjectingAdapter::update_if_active`]. Every push is a
    /// cheap no-op until a screen reader activates the tree.
    adapter: InjectingAdapter,
    /// Actions requested by assistive tech: pushed by the adapter's
    /// `ActionHandler` ([`ForgeActionHandler::do_action`]) and drained by the
    /// frame loop ([`AndroidAppHandle::apply_pending_accessibility_actions`]).
    pending_actions: Arc<Mutex<VecDeque<(NodeId, Action)>>>,
    /// The latest full accessibility tree, published by the frame loop and read
    /// by the adapter's `ActivationHandler`
    /// ([`ForgeActivationHandler::request_initial_tree`]) the moment a screen
    /// reader activates, so it sees real content instead of the adapter's
    /// placeholder window. `None` until the first post-layout semantics pass.
    tree_snapshot: Arc<Mutex<Option<TreeUpdate>>>,
    /// The semantics generation last assembled and pushed, so the frame loop
    /// skips reassembling+re-pushing an unchanged tree (via
    /// [`AppTree::semantics_if_changed`](frust_shell_common::AppTree::semantics_if_changed)).
    last_pushed_gen: u64,
}

impl AndroidA11y {
    /// Build the adapter and its shared channels, attaching accessibility to
    /// `host` (the `FrustSurfaceView`). Contains no `unsafe`: the raw-pointer
    /// bridge from this shell's `jni` 0.22 to accesskit_android's `jni` 0.21
    /// lives at the FFI boundary in [`crate::jni_glue::native_init_accessibility`];
    /// this method receives already-bridged references.
    ///
    /// [`InjectingAdapter::new`] can panic on a JNI error (e.g. a missing
    /// `dev.accesskit.android.Delegate` class); the caller's `guard` wrapper
    /// catches it, leaving the handle with no a11y rather than failing.
    fn new(env: &mut ak_jni::JNIEnv, host: &ak_jni::objects::JObject) -> Self {
        let pending_actions: Arc<Mutex<VecDeque<(NodeId, Action)>>> =
            Arc::new(Mutex::new(VecDeque::new()));
        let tree_snapshot: Arc<Mutex<Option<TreeUpdate>>> = Arc::new(Mutex::new(None));
        let adapter = InjectingAdapter::new(
            env,
            host,
            ForgeActivationHandler {
                tree_snapshot: tree_snapshot.clone(),
            },
            ForgeActionHandler {
                pending_actions: pending_actions.clone(),
            },
        );
        Self {
            adapter,
            pending_actions,
            tree_snapshot,
            last_pushed_gen: 0,
        }
    }
}

/// accesskit `ActivationHandler`: hands a newly-activated screen reader the
/// latest full tree the frame loop published (or `None` before the first
/// semantics pass, in which case the adapter serves its own placeholder until
/// the next `update_if_active`). Reads the shared snapshot slot only — never the
/// [`AndroidAppHandle`]. Runs on the Android UI thread.
struct ForgeActivationHandler {
    tree_snapshot: Arc<Mutex<Option<TreeUpdate>>>,
}

impl ActivationHandler for ForgeActivationHandler {
    fn request_initial_tree(&mut self) -> Option<TreeUpdate> {
        self.tree_snapshot.lock().ok().and_then(|slot| slot.clone())
    }
}

/// accesskit `ActionHandler`: records a requested action for the frame loop to
/// apply against the retained tree. It only enqueues — being `Send + 'static`
/// and owned by the adapter, it has no access to the [`AndroidAppHandle`], so it
/// cannot alias the `&mut self` frame pass that drains it. Runs on the Android
/// UI thread, the same thread as that frame pass.
struct ForgeActionHandler {
    pending_actions: Arc<Mutex<VecDeque<(NodeId, Action)>>>,
}

impl ActionHandler for ForgeActionHandler {
    fn do_action(&mut self, request: ActionRequest) {
        if let Ok(mut queue) = self.pending_actions.lock() {
            queue.push_back((request.target_node, request.action));
        }
    }
}

/// Assemble an accesskit [`TreeUpdate`] from a [`SemanticsUpdate`].
///
/// v1 always publishes the whole tree (`RenderRoot::semantics` recomputes it in
/// full): every node is a "new or changed" entry, `tree` names the root, and
/// `focus` is the focused node or the root fallback
/// ([`SemanticsUpdate::focus_id`]). `tree_id` is always [`TreeId::ROOT`] — this
/// shell drives a single, non-subtree accessibility tree.
fn tree_update_from_semantics(update: &SemanticsUpdate) -> TreeUpdate {
    TreeUpdate {
        nodes: update.nodes.clone(),
        tree: Some(Tree::new(update.root)),
        tree_id: TreeId::ROOT,
        focus: update.focus_id(),
    }
}

impl AndroidAppHandle {
    /// Attach the accesskit Android adapter to the host `FrustSurfaceView`
    /// (`nativeInitAccessibility`).
    ///
    /// `env`/`host` are the bridged (this shell's `jni` 0.22 → accesskit_android's
    /// `jni` 0.21) references built at the FFI boundary in
    /// [`crate::jni_glue::native_init_accessibility`]. Idempotent: a second call
    /// (e.g. a spurious re-init) is ignored, so the adapter — which panics if the
    /// host already has an accessibility delegate — is only ever created once per
    /// handle. Any panic constructing the adapter (a JNI hiccup, a missing
    /// `Delegate` class) unwinds into the caller's `guard`, leaving `self.a11y`
    /// as it was (`None`): accessibility is best-effort and never blocks the app.
    pub(crate) fn attach_accessibility(
        &mut self,
        env: &mut ak_jni::JNIEnv,
        host: &ak_jni::objects::JObject,
    ) {
        if self.a11y.is_some() {
            return;
        }
        self.a11y = Some(AndroidA11y::new(env, host));
    }

    /// Drain and apply any accessibility actions assistive tech queued since the
    /// last frame, routing each to
    /// [`AppTree::perform_accessibility_action`](frust_shell_common::AppTree::perform_accessibility_action).
    ///
    /// Drained into a local `Vec` first so the shared queue lock is released
    /// before `self.app` is touched — this both keeps the lock hold-time
    /// minimal and sidesteps borrowing `self.a11y` across the `&mut self.app`
    /// calls. The `needs_redraw` each action returns is implicit here: the
    /// continuous Choreographer loop already reconsiders the frame every tick.
    ///
    /// Returns whether any action was applied this call, which [`Self::frame`]
    /// feeds into `FrameInputs::a11y_action_performed` so an assistive-tech
    /// action forces the frame to run (the state it mutated must be reflected).
    pub(super) fn apply_pending_accessibility_actions(&mut self) -> bool {
        let drained: Vec<(NodeId, Action)> = match self.a11y.as_ref() {
            Some(a11y) => match a11y.pending_actions.lock() {
                Ok(mut queue) => queue.drain(..).collect(),
                Err(_) => Vec::new(),
            },
            None => return false,
        };
        let performed = !drained.is_empty();
        let app = &mut self.app;
        // An accessibility action is routed through synthesized pointer events,
        // landing in the very same handlers a real tap would — so it needs the
        // ambient `Owner` just as much (see [`under_root_owner`]).
        under_root_owner(|| {
            for (node_id, action) in drained {
                let _ = app.perform_accessibility_action(node_id.0, action);
            }
        });
        performed
    }

    /// Publish the current accessibility tree to the adapter, if it
    /// changed since the last push. Must run **after** [`Self::frame`]'s layout
    /// so node bounds are valid.
    ///
    /// Gated twice over: the [`AppTree::semantics_if_changed`](frust_shell_common::AppTree::semantics_if_changed)
    /// generation check skips reassembling an unchanged tree here, and the
    /// adapter's own `update_if_active` is a cheap no-op until a screen reader
    /// activates. The assembled tree is also snapshotted for a late-activating
    /// screen reader's `request_initial_tree`, so it sees real content rather
    /// than the adapter's placeholder window.
    pub(super) fn publish_semantics(&mut self) {
        // Cheap generation gate first (immutable `a11y` borrow, released before
        // the `&mut self.app` call below).
        let last_gen = match self.a11y.as_ref() {
            Some(a11y) => a11y.last_pushed_gen,
            None => return,
        };
        let Some(update) = self.app.semantics_if_changed(last_gen) else {
            return; // tree unchanged since the last push
        };
        let current_gen = self.app.semantics_generation();
        let tree_update = tree_update_from_semantics(&update);
        if let Some(a11y) = self.a11y.as_mut() {
            if let Ok(mut slot) = a11y.tree_snapshot.lock() {
                *slot = Some(tree_update.clone());
            }
            a11y.adapter.update_if_active(move || tree_update);
            a11y.last_pushed_gen = current_gen;
        }
    }
}

/// Unit tests for the pure accessibility-tree assembly.
///
/// This module is inside the `#[cfg(target_os = "android")]` `app` module, so it
/// only compiles/runs for the Android target — the assembly references
/// `frust_core`/`accesskit` types, all of which are Android-gated dependencies
/// of this crate by deliberate design (see `Cargo.toml`), so it cannot be a host
/// test the way [`crate::ffi_support`]'s pure helpers are — worse, the
/// documented Android compile gate (`cargo check --target aarch64-linux-android
/// -p frust`) never builds test cfg either, so nothing below is even
/// type-checked without an explicit `--all-targets`.
#[cfg(test)]
mod tests {
    use super::tree_update_from_semantics;
    use frust_core::SemanticsUpdate;
    use frust_core::accesskit::{Node, NodeId, Role, Tree, TreeId};

    #[test]
    fn tree_update_carries_nodes_root_and_focused_node() {
        let nodes = vec![
            (NodeId(1), Node::new(Role::Window)),
            (NodeId(5), Node::new(Role::Button)),
        ];
        let update = SemanticsUpdate {
            nodes: nodes.clone(),
            root: NodeId(1),
            focus: Some(NodeId(5)),
        };
        let tree_update = tree_update_from_semantics(&update);
        assert_eq!(tree_update.nodes, nodes);
        assert_eq!(tree_update.tree, Some(Tree::new(NodeId(1))));
        assert_eq!(tree_update.tree_id, TreeId::ROOT);
        // A focused node is reported directly.
        assert_eq!(tree_update.focus, NodeId(5));
    }

    #[test]
    fn tree_update_focus_falls_back_to_root_when_unfocused() {
        let update = SemanticsUpdate {
            nodes: vec![(NodeId(1), Node::new(Role::Window))],
            root: NodeId(1),
            focus: None,
        };
        let tree_update = tree_update_from_semantics(&update);
        // accesskit requires a non-optional focus target; the root is the
        // conventional fallback (`SemanticsUpdate::focus_id`).
        assert_eq!(tree_update.focus, NodeId(1));
    }
}
