//! The iOS scroll mirror (decision C5): the body's [`ScrollState`] drives the
//! navigation bar's scroll-edge transition and the iOS 26 tab-bar minimize as
//! a native scroll view would. [`SCROLL_MIRROR`] picks the mode at compile time.
//!
//! - `ScrollMirror::HiddenScrollView`: each page owns a `UIScrollView` at alpha
//!   0 (never `hidden`, so UIKit still treats it as content), with no user
//!   interaction, scroll-to-top or inset adjustment (offset 0 is the top
//!   edge), at origin zero behind the host view. Its frame height, content
//!   height and offset come from [`ScrollState::mirror_geometry`], and
//!   `setContentScrollView:forEdge:` names it for the top and bottom edges.
//!   While the spec carries a state, a tab bar controller answering
//!   `setTabBarMinimizeBehavior:` (iOS 26) minimizes on scroll down.
//! - `ScrollMirror::ManualAppearance`: no scroll view and no minimize; the
//!   navigation bar's `scrollEdgeAppearance` is its `standardAppearance` past
//!   the edge and a transparent one at it ([`past_scroll_edge`]).
//!
//! Apple documents that a controller finds the scroll view to observe by
//! walking its hierarchy and that `setContentScrollView:forEdge:` names one;
//! whether an offset-driven, invisible one is honoured is not, so which mode
//! ships is decided on device. Every call runs inside the chrome's queued
//! main-queue apply.

use objc2::rc::Retained;
use objc2::runtime::NSObjectProtocol;
use objc2::{MainThreadMarker, sel};
use objc2_core_foundation::{CGFloat, CGPoint, CGRect, CGSize};
use objc2_ui_kit::{
    NSDirectionalRectEdge, UINavigationBarAppearance, UINavigationController, UIScrollView,
    UIScrollViewContentInsetAdjustmentBehavior, UITabBarController, UITabBarMinimizeBehavior,
    UIViewController,
};

use crate::chrome::{SCROLL_MIRROR, ScrollMirror, ScrollState, past_scroll_edge};

/// One page's mirror, per [`SCROLL_MIRROR`].
pub(super) enum PageMirror {
    Hidden(Retained<UIScrollView>),
    /// Whether the bar shows its standard appearance; `None` until applied.
    Manual(Option<bool>),
}

impl PageMirror {
    /// The mirror for `page`, its scroll view (hidden mode) behind the rest.
    pub(super) fn new(mtm: MainThreadMarker, page: &UIViewController) -> Self {
        match SCROLL_MIRROR {
            ScrollMirror::ManualAppearance => Self::Manual(None),
            ScrollMirror::HiddenScrollView => {
                let mirror = UIScrollView::new(mtm);
                mirror.setAlpha(0.0);
                mirror.setUserInteractionEnabled(false);
                mirror.setScrollsToTop(false);
                mirror.setShowsVerticalScrollIndicator(false);
                mirror.setShowsHorizontalScrollIndicator(false);
                mirror.setContentInsetAdjustmentBehavior(
                    UIScrollViewContentInsetAdjustmentBehavior::Never,
                );
                if let Some(view) = page.view() {
                    mirror.setFrame(view.bounds());
                    view.insertSubview_atIndex(&mirror, 0);
                }
                let edges = NSDirectionalRectEdge::Top | NSDirectionalRectEdge::Bottom;
                page.setContentScrollView_forEdge(Some(&mirror), edges);
                Self::Hidden(mirror)
            }
        }
    }

    /// Mirror `state` (`None`: at the top) onto `nav`'s bar.
    pub(super) fn apply(
        &mut self,
        mtm: MainThreadMarker,
        nav: &UINavigationController,
        state: Option<ScrollState>,
    ) {
        match self {
            Self::Hidden(mirror) => {
                let geometry = state.unwrap_or_default().mirror_geometry();
                let page = mirror
                    .superview()
                    .map_or(CGSize::new(0.0, 0.0), |page| page.bounds().size);
                // No laid-out viewport yet: fill the page, as at creation.
                let height = if geometry.viewport > 0.0 {
                    geometry.viewport as CGFloat
                } else {
                    page.height
                };
                let origin = CGPoint::new(0.0, 0.0);
                mirror.setFrame(CGRect::new(origin, CGSize::new(page.width, height)));
                let content = CGSize::new(page.width, geometry.content_height as CGFloat);
                mirror.setContentSize(content);
                mirror.setContentOffset(CGPoint::new(0.0, geometry.offset as CGFloat));
            }
            Self::Manual(shown) => {
                let offset = state.map_or(0.0, |state| state.offset);
                let past = past_scroll_edge(shown.unwrap_or(false), offset);
                if *shown == Some(past) {
                    return;
                }
                *shown = Some(past);
                let bar = nav.navigationBar();
                let edge = if past {
                    bar.standardAppearance()
                } else {
                    let transparent = UINavigationBarAppearance::new(mtm);
                    transparent.configureWithTransparentBackground();
                    transparent
                };
                bar.setScrollEdgeAppearance(Some(&edge));
            }
        }
    }
}

/// Minimize the tab bar on scroll down while the spec carries a state, in
/// hidden mode, where UIKit has it (`setTabBarMinimizeBehavior:` is iOS 26).
pub(super) fn apply_minimize(tabs: &UITabBarController, state: Option<ScrollState>) {
    if SCROLL_MIRROR != ScrollMirror::HiddenScrollView
        || !tabs.respondsToSelector(sel!(setTabBarMinimizeBehavior:))
    {
        return;
    }
    let behavior = if state.is_some() {
        UITabBarMinimizeBehavior::OnScrollDown
    } else {
        UITabBarMinimizeBehavior::Automatic
    };
    if tabs.tabBarMinimizeBehavior() != behavior {
        tabs.setTabBarMinimizeBehavior(behavior);
    }
}
