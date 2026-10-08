//! The typed v1 method set — [`Method::as_str`]/[`Method::from_str`] are the
//! single source of truth for the on-wire `"method"` string each variant
//! corresponds to.

/// A devtools protocol v1 method name, typed. [`crate::Request::method`]/
/// [`crate::Notification::method`] carry the plain `String` on the wire
/// (JSON-RPC has no closed method set, and an unrecognized method must
/// still decode — see `docs/CODE_STANDARDS.md`'s Naming Conventions and this
/// crate's forward-compat testing); a caller matches the decoded string
/// against [`Method::from_str`] to dispatch, treating `None` as "unknown
/// method" (→ `RpcError::METHOD_NOT_FOUND` on a server, or an ignored push
/// on a client).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Method {
    /// Client→server request; result: [`crate::HandshakeInfo`].
    Handshake,
    /// Client→server request; result: [`crate::WidgetTreeDump`].
    WidgetTree,
    /// Client→server request; params: [`crate::WidgetPropsParams`];
    /// result: [`crate::WidgetProps`].
    WidgetProps,
    /// Client→server request; result: [`crate::AckResult`]. Arms the
    /// server to start pushing `FrameStats` notifications.
    FrameStatsSubscribe,
    /// Server→client notification; payload: [`crate::FrameStats`].
    FrameStats,
    /// Client→server request; result: [`crate::MetricsSnapshot`].
    MetricsSnapshot,
    /// Client→server request; params: [`crate::InputTapParams`];
    /// result: [`crate::AckResult`].
    InputTap,
    /// Client→server request; params: [`crate::InputScrollParams`];
    /// result: [`crate::AckResult`].
    InputScroll,
    /// Client→server request; params: [`crate::InputTextParams`];
    /// result: [`crate::AckResult`].
    InputText,
    /// Client→server request; result: [`crate::ScreenshotResult`], or
    /// `RpcError::NOT_SUPPORTED` on a server with no `Screenshot`
    /// capability.
    Screenshot,
    /// Client→server request; result: [`crate::HotpatchInfo`], or
    /// `RpcError::NOT_SUPPORTED` on a server with no `HotPatch` capability.
    HotpatchInfo,
    /// Client→server request; params: [`crate::PatchChunkParams`];
    /// result: [`crate::AckResult`]. Capability-gated like `Screenshot`.
    PatchChunk,
    /// Client→server request; params: [`crate::ApplyPatchParams`];
    /// result: [`crate::PatchOutcome`]. Capability-gated like `Screenshot`.
    /// Applies the `patch_chunk` uploads for `patch_id`, or — on an app that
    /// advertises [`crate::HotpatchInfo::patch_file_hand_off`] — the
    /// host-written file named by [`crate::ApplyPatchParams::file`] instead.
    ApplyPatch,
}

impl Method {
    /// The wire method name — the exact `"method"` field value.
    pub fn as_str(self) -> &'static str {
        match self {
            Method::Handshake => "handshake",
            Method::WidgetTree => "widget_tree",
            Method::WidgetProps => "widget_props",
            Method::FrameStatsSubscribe => "frame_stats_subscribe",
            Method::FrameStats => "frame_stats",
            Method::MetricsSnapshot => "metrics_snapshot",
            Method::InputTap => "input_tap",
            Method::InputScroll => "input_scroll",
            Method::InputText => "input_text",
            Method::Screenshot => "screenshot",
            Method::HotpatchInfo => "hotpatch_info",
            Method::PatchChunk => "patch_chunk",
            Method::ApplyPatch => "apply_patch",
        }
    }

    /// Parses a wire method name into its typed variant. Returns `None` for
    /// any name outside the v1 set — the caller's own dispatch decides what
    /// an unrecognized method means (unknown-method tolerance is a decode
    /// concern for [`crate::Request`]/[`crate::Notification`] themselves,
    /// not this lookup).
    ///
    /// Deliberately an inherent method returning `Option`, not a
    /// `std::str::FromStr` impl: an unrecognized method is an ordinary,
    /// expected case here (any future-protocol or peer-defined method
    /// name), not an error a caller needs `FromStr`'s `Result`/`Err`-type
    /// machinery for.
    #[allow(clippy::should_implement_trait)]
    pub fn from_str(s: &str) -> Option<Method> {
        Some(match s {
            "handshake" => Method::Handshake,
            "widget_tree" => Method::WidgetTree,
            "widget_props" => Method::WidgetProps,
            "frame_stats_subscribe" => Method::FrameStatsSubscribe,
            "frame_stats" => Method::FrameStats,
            "metrics_snapshot" => Method::MetricsSnapshot,
            "input_tap" => Method::InputTap,
            "input_scroll" => Method::InputScroll,
            "input_text" => Method::InputText,
            "screenshot" => Method::Screenshot,
            "hotpatch_info" => Method::HotpatchInfo,
            "patch_chunk" => Method::PatchChunk,
            "apply_patch" => Method::ApplyPatch,
            _ => return None,
        })
    }
}

impl std::fmt::Display for Method {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const ALL: [Method; 13] = [
        Method::Handshake,
        Method::WidgetTree,
        Method::WidgetProps,
        Method::FrameStatsSubscribe,
        Method::FrameStats,
        Method::MetricsSnapshot,
        Method::InputTap,
        Method::InputScroll,
        Method::InputText,
        Method::Screenshot,
        Method::HotpatchInfo,
        Method::PatchChunk,
        Method::ApplyPatch,
    ];

    #[test]
    fn as_str_from_str_round_trips_every_v1_method() {
        for m in ALL {
            assert_eq!(Method::from_str(m.as_str()), Some(m));
        }
    }

    #[test]
    fn hotpatch_methods_use_their_wire_names() {
        assert_eq!(
            Method::from_str("hotpatch_info"),
            Some(Method::HotpatchInfo)
        );
        assert_eq!(Method::from_str("patch_chunk"), Some(Method::PatchChunk));
        assert_eq!(Method::from_str("apply_patch"), Some(Method::ApplyPatch));
    }

    #[test]
    fn from_str_rejects_unknown_method() {
        assert_eq!(Method::from_str("not_a_real_method"), None);
    }

    #[test]
    fn display_matches_as_str() {
        assert_eq!(Method::Handshake.to_string(), "handshake");
    }
}
