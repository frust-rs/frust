import AVFoundation
import CFrustCamera
import FrustEmbedding
import UIKit

// `frust-camera`'s iOS platform-view factory — the preview slot an app
// feeds into `frust::platform_view(session.preview_view_type())`
// (`docs/ARCHITECTURE.md`'s Module Structure, `platform_view` row).
//
// Bare `@objc` runtime name (`CameraPreviewFactory`, no package prefix —
// `docs/CODE_STANDARDS.md`'s platform-view factory `viewType` LAW) IS the
// `viewType` string `plugins/camera/src/apple.rs`'s `PREVIEW_VIEW_TYPE`
// constant hard-codes; `FrustEmbedding`'s `FrustViewHost` resolves it via
// `NSClassFromString`, matching Android's fully-qualified-class-name
// counterpart.
//
// Unlike a typical `platform_view` consumer, this factory does no camera
// work of its own: `plugins/camera/src/apple.rs` (task 07) drives the
// `AVCaptureSession` entirely from Rust via `objc2-av-foundation` — there is
// no Swift-side capture/permission code anywhere in this package. This type
// only bridges the already-open session's native pointer
// (`frust_camera_session_handle`, `CFrustCamera`'s C export — task 08's
// two-package spike) into an `AVCaptureVideoPreviewLayer`.
@objc(CameraPreviewFactory)
public final class CameraPreviewFactory: NSObject, FrustPlatformViewFactory {
    public override init() {
        super.init()
    }

    /// Parse `{"session": N}` (`CameraSession::params_json`'s payload),
    /// resolve the native session, and attach a fresh preview layer to it.
    /// Called on the main thread; must not block (protocol doc) — resolving
    /// the handle and attaching a layer is synchronous, non-blocking work.
    public func createView(paramsJson: String) -> UIView {
        let view = CameraPreviewView()
        attach(paramsJson: paramsJson, to: view)
        return view
    }

    /// A params-only change (e.g. a session id change after a lens flip) —
    /// re-attach the SAME preview slot's layer to the new session rather
    /// than tearing the slot down and recreating it.
    public func updateParams(_ view: UIView, paramsJson: String) {
        guard let preview = view as? CameraPreviewView else { return }
        attach(paramsJson: paramsJson, to: preview)
    }

    /// Detach the preview layer from its session. The underlying
    /// `AVCaptureSession` stays open — `plugins/camera`'s A6 decision
    /// (matching the Flutter `camera` plugin's preview-vs-session lifetime
    /// split): only `CameraSession::close` (Rust-side) actually tears the
    /// camera session down, so navigating away from a preview slot and back
    /// re-attaches to the same still-running session instead of paying a
    /// re-open cost.
    public func disposeView(_ view: UIView) {
        (view as? CameraPreviewView)?.detach()
    }

    /// Shared `createView`/`updateParams` body: parse the session id out of
    /// `paramsJson`, resolve its native `AVCaptureSession *` via
    /// `frust_camera_session_handle`, and attach it to `view`. A missing/
    /// unparsable `session` field or a NULL handle (session not open, or
    /// already closed elsewhere) is a silent no-op — `view`'s preview layer
    /// simply keeps whatever session (or none) it already had, never a
    /// crash on a malformed/stale params payload.
    private func attach(paramsJson: String, to view: CameraPreviewView) {
        guard
            let data = paramsJson.data(using: .utf8),
            let root = (try? JSONSerialization.jsonObject(with: data)) as? [String: Any],
            let sessionId = (root["session"] as? NSNumber)?.int32Value,
            let handle = frust_camera_session_handle(sessionId)
        else { return }

        // `frust_camera_session_handle`'s documented contract (frust_camera.h):
        // ownership stays with the Rust-side session — bridge unretained.
        let session = Unmanaged<AVCaptureSession>.fromOpaque(handle).takeUnretainedValue()
        view.attach(session: session)
    }
}

/// The plain `UIView` a `CameraPreviewFactory` slot hosts: a live
/// `AVCaptureVideoPreviewLayer` (`videoGravity` fixed to aspect-fill,
/// matching `frust`'s other media-preview widgets' cover-fit default) added
/// as a sublayer and kept sized to the view's bounds. `FrustViewHost` (in
/// `FrustEmbedding`) owns this view's frame/clip/visibility entirely — this
/// type never sets its own frame (the factory protocol's documented
/// contract).
private final class CameraPreviewView: UIView {
    private var previewLayer: AVCaptureVideoPreviewLayer?

    /// Attach (or re-attach, on a session change) the preview to `session`.
    /// `AVCaptureVideoPreviewLayer.session` is settable after construction,
    /// so a re-attach reuses the existing layer rather than recreating it —
    /// `AVCaptureVideoPreviewLayer(session:)` (the standard AVFoundation
    /// preview-layer constructor) is only called once, on first attach.
    func attach(session: AVCaptureSession) {
        if let previewLayer {
            previewLayer.session = session
        } else {
            let previewLayer = AVCaptureVideoPreviewLayer(session: session)
            previewLayer.videoGravity = .resizeAspectFill
            previewLayer.frame = bounds
            layer.addSublayer(previewLayer)
            self.previewLayer = previewLayer
        }
    }

    /// Stop observing the current session (`CameraPreviewFactory.disposeView`'s
    /// A6 contract — the session itself is untouched).
    func detach() {
        previewLayer?.session = nil
    }

    override func layoutSubviews() {
        super.layoutSubviews()
        previewLayer?.frame = bounds
    }
}
