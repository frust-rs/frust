//! The static plugin registry (v1) — seventeen entries mirroring `plugins/`:
//! `shared-preferences` (dependency only), `secure-storage` (dependency plus
//! an optional `biometric-gate` feature wiring in the plugin's own Android
//! library module and the iOS plist key its README documents),
//! `clean-signals-frust` (dependency; `clean-signals` itself is git+rev-pinned
//! to its public repo, so no sibling checkout is required), `camera`
//! (dependency, an app-side plist key, the plugin's
//! own Android library module, its own iOS Swift package — the first
//! registry entry to use [`Contribution::SwiftPackageRef`] — and an app-crate
//! export shim a device gate proved necessary, see this module's `CAMERA_BASE`
//! constant's doc comment for the full rationale), `native-widgets`
//! (dependency plus the plugin's own Android library
//! module — and **nothing** on iOS, which is not an omission: that arm ships
//! zero Swift by design), `clipboard` (dependency only — plain text
//! clipboard access needs no manifest permission, plist key, Gradle module,
//! or Swift package on either mobile platform), `haptics` (dependency
//! plus the `android.permission.VIBRATE` manifest permission — the first
//! registry entry to use [`Contribution::ManifestPermission`] rather than a
//! Gradle module for its Android addition, since the plugin's Android
//! backend is plain JNI with no Kotlin helper class to carry the permission
//! inside a module manifest; see `HAPTICS_BASE`'s doc comment), `url-launcher`
//! (dependency only — the launch half of an RFC 8252 OAuth round trip over a
//! platform's external browser; the second registry entry, after `haptics`,
//! whose Android side needs no `<uses-permission>` at all — `startActivity`
//! with `ACTION_VIEW` needs none on API 30+, and no `<queries>` element
//! either, since it targets an implicit intent the OS itself resolves — see
//! `URL_LAUNCHER_BASE`'s doc comment), `auth-session`
//! (dependency plus the plugin's own Android library module — OAuth round trip
//! completion half, mirroring `url-launcher`'s launch half; Chrome Custom Tabs
//! on Android / ASWebAuthenticationSession on iOS — the second registry entry
//! to pair a bare CargoDep with just a GradleModule on Android, like
//! `native-widgets` and `video-player` — no manifest permission or plist key,
//! since Custom Tabs carry no permission, the `<queries>` element comes from
//! the module's own manifest via the manifest merger, and ASWebAuthenticationSession
//! needs no plist key; desktop reports NoHandler — see `AUTH_SESSION_BASE`'s doc
//! comment), `iap`
//! (dependency, the plugin's own Android library module, and its own iOS
//! Swift package — no plist key and no app-crate macro; see `IAP_BASE`'s doc
//! comment for why), `database` (dependency only — pure-Rust plugin, no
//! OS-side integration), and `i18n` (dependency, a seeded starter
//! `locales/en/main.ftl` — the first registry entry to use
//! [`Contribution::ScaffoldFile`] — and the `frust_i18n::locales!` app-crate
//! macro invocation; see `I18N_BASE`'s doc comment for the ordering
//! constraint between the two), `glyph`/`material`/`cupertino`
//! (dependency only — the three built-in design-system catalogs, which left
//! the `frust` facade's Cargo-feature graph and now each ship as their own
//! sibling plugin crate; adding one contributes only the Cargo dependency —
//! calling `frust_<name>::install()` in `app!(setup = {..})` to make it the
//! app's active theme is left to the app, the same way `frust create`'s own
//! scaffold does it), and `shadcn` (dependency only, the same shape as the
//! three built-ins above — the tier's first *external-origin* catalog,
//! ported from shadcn/ui rather than authored in this repo), and
//! `video-player` (dependency plus the plugin's own Android library
//! module — the exact `native-widgets` two-contribution shape; no plist
//! key, no manifest permission, no Swift package, no app-crate macro, and
//! no contribution at all on macOS, whose factory self-registers into
//! `frust_plugin::desktop` at first use — see `VIDEO_PLAYER_BASE`'s doc
//! comment for the full accounting).

use super::{Contribution, FeatureSpec, PluginSpec};

/// The `biometric-gate` feature's two contributions — the exact Android/iOS
/// additions `plugins/secure-storage/README.md` §2 documents.
///
/// Android is a **single** contribution: the plugin's own
/// `com.android.library` module. `FrustBiometric.kt` lives inside it (no file
/// is copied into the app), its `AndroidManifest.xml` carries the
/// `USE_BIOMETRIC` permission for the manifest merger to fold in, and its
/// `consumerProguardFiles` carries the R8 keep rule — so the app's manifest
/// and `proguard-rules.pro` are never touched, and nothing can drift from the
/// plugin it came from. iOS still needs a real [`Contribution::PlistEntry`]:
/// Apple requires the usage-description string in the *app's* own
/// `Info.plist`.
const SECURE_STORAGE_BIOMETRIC: &[Contribution] = &[
    Contribution::PlistEntry {
        key: "NSFaceIDUsageDescription",
        value: "Unlock your stored credentials.",
        // No `--` here: an XML comment may not contain a double hyphen.
        comment: "Face ID / Touch ID usage description, added only when the \
                  frust-secure-storage biometric gate is used.",
    },
    Contribution::GradleModule {
        gradle_name: ":frust-secure-storage",
        rel_path: "plugins/secure-storage/platform/android",
    },
];

/// secure-storage's single optional feature.
const SECURE_STORAGE_FEATURES: &[FeatureSpec] = &[FeatureSpec {
    id: "biometric-gate",
    summary: "Per-store Face ID / Touch ID / BiometricPrompt gate: adds the \
              NSFaceIDUsageDescription plist key and includes the plugin's \
              `:frust-secure-storage` Android library module (which carries \
              the FrustBiometric helper, the USE_BIOMETRIC permission, and \
              the R8 keep rule).",
    contributions: SECURE_STORAGE_BIOMETRIC,
}];

const SHARED_PREFERENCES: PluginSpec = PluginSpec {
    id: "shared-preferences",
    summary: "Synchronous, thread-safe key-value store (plaintext).",
    crate_dir: "shared-preferences",
    base: &[Contribution::CargoDep {
        name: "frust-shared-preferences",
    }],
    optional_features: &[],
    requires_sibling: None,
};

const SECURE_STORAGE: PluginSpec = PluginSpec {
    id: "secure-storage",
    summary: "Encrypted key-value store (Keychain / Keystore / keyring) with an \
              optional per-store biometric gate.",
    crate_dir: "secure-storage",
    base: &[Contribution::CargoDep {
        name: "frust-secure-storage",
    }],
    optional_features: SECURE_STORAGE_FEATURES,
    requires_sibling: None,
};

const CLEAN_SIGNALS_FRUST: PluginSpec = PluginSpec {
    id: "clean-signals-frust",
    summary: "Clean-architecture facade binding the clean-signals core to frust.",
    crate_dir: "clean-signals-frust",
    base: &[Contribution::CargoDep {
        name: "clean-signals-frust",
    }],
    optional_features: &[],
    requires_sibling: None,
};

/// `camera`'s base contributions, unwidened since first landing. No optional
/// features in v1, and no
/// `ManifestPermission`: `android.permission.CAMERA` rides the plugin's own
/// `AndroidManifest.xml`, manifest-merged like secure-storage's
/// `USE_BIOMETRIC` above — only the plist key stays app-side, since Apple
/// requires the usage string in the app's own `Info.plist`.
const CAMERA_BASE: &[Contribution] = &[
    Contribution::CargoDep {
        name: "frust-camera",
    },
    Contribution::PlistEntry {
        key: "NSCameraUsageDescription",
        value: "Take photos and record video.",
        // No `--` here: an XML comment may not contain a double hyphen.
        comment: "Camera usage description, required by Apple in the app's \
                  own Info.plist — no plugin module can supply it, unlike \
                  the GradleModule/SwiftPackageRef contributions below.",
    },
    Contribution::GradleModule {
        gradle_name: ":frust-camera",
        rel_path: "plugins/camera/platform/android",
    },
    // The first registry use of this variant (the embedding work defined the
    // multi-package shape without building it — see
    // `plugins/camera/platform/ios`'s own `Package.swift` doc comment).
    // `rel_path` points at the package
    // directory itself (no nested `FrustCamera/` subdirectory — that is
    // just the package/product *name*), mirroring `GradleModule`'s
    // `rel_path` convention above.
    Contribution::SwiftPackageRef {
        package_name: "FrustCamera",
        rel_path: "plugins/camera/platform/ios",
    },
    // Added after a device gate measured the failure this prevents: without
    // it a freshly scaffolded app that added the camera
    // plugin does **not link on iOS** —
    // `Undefined symbols: _frust_camera_session_handle`.
    //
    // That export lives in the plugin crate and is called only from Swift, so
    // nothing in Rust references it and the release profile's `lto = "fat"`
    // internalizes it away before the Swift side links. The catalog never hit
    // this because its own page calls the camera API heavily; an app that
    // merely *added* the plugin — i.e. every Add Plugin user — does not.
    // `apple.rs`'s `ios_exports!` doc says to invoke it "only if the device
    // gate shows the direct export missing". The gate showed exactly that.
    Contribution::AppCrateMacro {
        invocation: "frust_camera::ios_exports!();",
        cfg: Some("target_vendor = \"apple\""),
        comment: "Keeps frust-camera's Swift-called C export alive through \
                  release LTO (see frust_camera::ios_exports).",
    },
];

const CAMERA: PluginSpec = PluginSpec {
    id: "camera",
    summary: "Camera preview (a platform view), still capture, and a YUV/BGRA \
              image stream (CameraX / AVFoundation).",
    crate_dir: "camera",
    base: CAMERA_BASE,
    optional_features: &[],
    requires_sibling: None,
};

/// `native-widgets`' base contributions — deliberately just **two**, and the
/// short list is the interesting part.
///
/// The Android half is one [`Contribution::GradleModule`]: the plugin's own
/// `com.android.library` module carries both of its Kotlin classes
/// (`dev.frust.nativewidgets.FrustNativeControlFactory` and
/// `FrustNativeListener`) and its two R8 keep rules in its own
/// `consumer-rules.pro`, so nothing is copied into the app and nothing can
/// drift from the plugin it came from — the same shape secure-storage and
/// camera already use. It needs **no** [`Contribution::ManifestPermission`]:
/// hosting an `android.widget` view requires no permission at all.
///
/// The iOS half is **empty on purpose, not by omission**. There is no
/// [`Contribution::SwiftPackageRef`] and no `.swift` file anywhere in this
/// plugin: its platform-view factory is a Rust `objc2` `define_class!` type
/// registered straight into the Objective-C runtime and resolved by
/// `NSClassFromString`, already proven out by the plugin's own iOS runtime
/// factory. And there is no
/// [`Contribution::PlistEntry`] because nothing here touches a
/// privacy-gated API. So an iOS app needs exactly the Cargo dependency.
const NATIVE_WIDGETS_BASE: &[Contribution] = &[
    Contribution::CargoDep {
        name: "frust-native-widgets",
    },
    Contribution::GradleModule {
        gradle_name: ":frust-native-widgets",
        rel_path: "plugins/native-widgets/platform/android",
    },
];

const NATIVE_WIDGETS: PluginSpec = PluginSpec {
    id: "native-widgets",
    summary: "Real OS controls (button, label, switch, slider, progress, \
              image) hosted as platform views and driven from Rust \
              (android.widget / UIKit).",
    crate_dir: "native-widgets",
    base: NATIVE_WIDGETS_BASE,
    optional_features: &[],
    requires_sibling: None,
};

/// `clipboard`'s base contribution — a single Cargo dependency, and nothing
/// else. Plain-text clipboard read/write needs no Android manifest
/// permission, no iOS plist key, no Gradle module (the Android backend is
/// plain JNI against framework classes, no app-defined helper class), and no
/// Swift package (the iOS backend is plain `objc2-ui-kit` against
/// `UIPasteboard`) — see `plugins/clipboard/README.md`.
const CLIPBOARD: PluginSpec = PluginSpec {
    id: "clipboard",
    summary: "Synchronous plain-text clipboard access (ClipboardManager / \
              UIPasteboard / arboard).",
    crate_dir: "clipboard",
    base: &[Contribution::CargoDep {
        name: "frust-clipboard",
    }],
    optional_features: &[],
    requires_sibling: None,
};

/// `haptics`' base contributions — a Cargo dependency plus one
/// [`Contribution::ManifestPermission`], the app's own `android.permission.VIBRATE`
/// `<uses-permission>` line.
///
/// This is the registry's **first** use of [`Contribution::ManifestPermission`]
/// rather than [`Contribution::GradleModule`] for an Android addition — every
/// prior permission-carrying entry (`secure-storage`'s `USE_BIOMETRIC`,
/// `camera`'s implicit `CAMERA` permission) rides inside the plugin's own
/// Gradle module manifest, folded in by the manifest merger so the app's
/// manifest is never touched. `haptics` has no such module: like `clipboard`,
/// its Android backend is plain JNI against framework classes
/// (`android.os.Vibrator`/`VibrationEffect`) with no app-defined Kotlin helper
/// class to carry a permission inside — see
/// `plugins/haptics/src/android.rs`'s module doc. `VIBRATE` is a **normal**
/// permission (granted automatically at install, no runtime prompt), so an
/// app-manifest edit here carries none of the runtime-permission-flow
/// complexity a dangerous permission would.
const HAPTICS_BASE: &[Contribution] = &[
    Contribution::CargoDep {
        name: "frust-haptics",
    },
    Contribution::ManifestPermission {
        permission: "android.permission.VIBRATE",
    },
];

const HAPTICS: PluginSpec = PluginSpec {
    id: "haptics",
    summary: "Minimal haptic feedback (selection tick, impact, success/warning/error) \
              over Vibrator/VibrationEffect and UIKit's feedback generators.",
    crate_dir: "haptics",
    base: HAPTICS_BASE,
    optional_features: &[],
    requires_sibling: None,
};

/// `url-launcher`'s single base contribution — a Cargo dependency and
/// nothing else, the first platform plugin (one with real Android/iOS/desktop
/// backends, unlike the pure-Rust `database`/`i18n`/design-system entries) to
/// need a **bare** `CargoDep` with no accompanying permission, plist key,
/// Gradle module, or Swift package.
///
/// Opening an absolute `http`/`https` URL in the platform's external browser
/// needs none of those: Android's `startActivity` with an implicit
/// `ACTION_VIEW` intent needs no `<uses-permission>` (unlike `haptics`'
/// `VIBRATE`, itself a *normal*, install-time-granted permission) and no
/// `<queries>` element either, because targeting an implicit intent the OS
/// itself resolves is exempt from the package-visibility filter added in API
/// 30 — see `plugins/url-launcher/src/android.rs`'s module doc. iOS's
/// `UIApplication.shared.open(_:)` needs no `Info.plist` key for an
/// `http`/`https` URL (only `LSApplicationQueriesSchemes` gates
/// `canOpenURL:` on a *custom* scheme, and this plugin never calls that).
/// Desktop's `xdg-open`/`open`/`ShellExecuteW` shell out to the OS URL
/// handler directly, no linked library or bundle entry required. Like
/// `haptics`, there is no Kotlin helper class or Swift package: both mobile
/// backends are plain JNI/objc2 against framework APIs.
const URL_LAUNCHER_BASE: &[Contribution] = &[Contribution::CargoDep {
    name: "frust-url-launcher",
}];

const URL_LAUNCHER: PluginSpec = PluginSpec {
    id: "url-launcher",
    summary: "Open an absolute http/https URL in the platform's default external browser \
              (Android ACTION_VIEW, iOS openURL:, desktop xdg-open/open/ShellExecuteW) — \
              the launch half of an RFC 8252 OAuth round trip; completion returns through \
              deep links.",
    crate_dir: "url-launcher",
    base: URL_LAUNCHER_BASE,
    optional_features: &[],
    requires_sibling: None,
};

/// `auth-session`'s base contributions — a Cargo dependency plus the plugin's own
/// Android library module, the `native-widgets`/`video-player` two-contribution
/// shape (dependency + one Gradle module).
///
/// No [`Contribution::ManifestPermission`]: Chrome Custom Tabs carries no permission —
/// the Android `startActivity` implied by the API needs none (unlike `haptics`'
/// `VIBRATE`, itself a *normal* permission). No [`Contribution::PlistEntry`]: Apple's
/// `ASWebAuthenticationSession` needs no plist key for `http`/`https` callbacks
/// (unlike `camera`'s usage-description string, which the app's `Info.plist` must
/// carry). No [`Contribution::SwiftPackageRef`] and no [`Contribution::AppCrateMacro`]:
/// the iOS factory is a Rust `objc2` `define_class!` type (like `native-widgets` and
/// `video-player`'s Apple arms), so there is nothing to link from Swift and nothing
/// for release LTO to strip. The `<queries>` element for the CustomTabsService comes
/// from the module's own manifest via the manifest merger — the app's manifest is
/// never touched. Desktop backends report `NoHandler` (no platform API available).
const AUTH_SESSION_BASE: &[Contribution] = &[
    Contribution::CargoDep {
        name: "frust-auth-session",
    },
    Contribution::GradleModule {
        gradle_name: ":frust-auth-session",
        rel_path: "plugins/auth-session/platform/android",
    },
];

const AUTH_SESSION: PluginSpec = PluginSpec {
    id: "auth-session",
    summary: "OAuth round trip in the platform auth user agent — ASWebAuthenticationSession \
              (iOS/macOS, in-process callback) or Chrome Custom Tabs (Android, Gradle module) — \
              resolving Callback(url)/Cancelled with an ephemeral mode; Linux/Windows report \
              NoHandler.",
    crate_dir: "auth-session",
    base: AUTH_SESSION_BASE,
    optional_features: &[],
    requires_sibling: None,
};

/// `iap`'s base contributions — a Cargo dependency plus its own Android
/// library module and its own iOS Swift package, the `camera`/`native-widgets`
/// shape (a plugin's Kotlin/Swift never copied into the app).
///
/// No `ManifestPermission`: `com.android.vending.BILLING` arrives transitively
/// from the `openiap-google` Play Billing AAR, folded into the app by the
/// manifest merger exactly like `secure-storage`'s `USE_BIOMETRIC` and
/// `camera`'s implicit `CAMERA` permission — proven by grepping a generated
/// app's merged manifest. No `PlistEntry`: unlike camera's usage-description
/// string, StoreKit needs no app-side privacy key. No `AppCrateMacro`: an app
/// calls `frust_iap::Iap` explicitly, so there is no dead-code-elimination
/// hazard for release LTO to strip (camera's export shim exists only because
/// nothing in Rust calls the symbol it protects).
const IAP_BASE: &[Contribution] = &[
    Contribution::CargoDep { name: "frust-iap" },
    Contribution::GradleModule {
        gradle_name: ":frust-iap",
        rel_path: "plugins/iap/platform/android",
    },
    Contribution::SwiftPackageRef {
        package_name: "FrustIap",
        rel_path: "plugins/iap/platform/ios",
    },
];

const IAP: PluginSpec = PluginSpec {
    id: "iap",
    summary: "In-app purchases and subscriptions over Play Billing / StoreKit 2, \
              speaking the OpenIAP wire protocol.",
    crate_dir: "iap",
    base: IAP_BASE,
    optional_features: &[],
    requires_sibling: None,
};

/// `video-player`'s base contributions — the exact two-contribution shape
/// [`NATIVE_WIDGETS_BASE`] uses: a Cargo dependency plus the plugin's own
/// Android library module, and nothing else.
///
/// No [`Contribution::PlistEntry`]: video playback needs no usage-description
/// string in the app's own Info.plist, unlike camera's
/// `NSCameraUsageDescription`. No [`Contribution::ManifestPermission`]:
/// `android.permission.INTERNET` rides the plugin's own Android library
/// module manifest, folded in by the manifest merger exactly like
/// secure-storage's `USE_BIOMETRIC` above — the app's own manifest is never
/// touched. No [`Contribution::SwiftPackageRef`] and no
/// [`Contribution::AppCrateMacro`]: the Apple factories (iOS and macOS) are
/// pure-Rust `objc2` `define_class!` classes — native-widgets' iOS precedent,
/// widened to macOS — so there is nothing to link from Swift and nothing for
/// release LTO to strip (unlike camera's Swift-called C export; see
/// `CAMERA_BASE`'s doc comment). And macOS needs **no contribution at all**:
/// its view factory registers itself into `frust_plugin::desktop` the first
/// time a video view is opened, the same lazy-registration shape the iOS
/// factory uses.
const VIDEO_PLAYER_BASE: &[Contribution] = &[
    Contribution::CargoDep {
        name: "frust-video-player",
    },
    Contribution::GradleModule {
        gradle_name: ":frust-video-player",
        rel_path: "plugins/video-player/platform/android",
    },
];

const VIDEO_PLAYER: PluginSpec = PluginSpec {
    id: "video-player",
    summary: "Video playback (Media3 ExoPlayer / AVPlayer) with the video \
              surface as a platform view; local, asset, http(s) and HLS \
              sources; Android, iOS, macOS.",
    crate_dir: "video-player",
    base: VIDEO_PLAYER_BASE,
    optional_features: &[],
    requires_sibling: None,
};

/// `database`'s single optional feature — enables the Turso engine (a pure-Rust
/// SQLite rewrite) as an alternative to SQLite. This is the first zero-OS-side
/// optional feature: a pure Cargo-feature flip with no Gradle module, plist key,
/// manifest permission, Swift package, or app-crate macro contribution.
///
/// See `plugins/database/README.md` for the binary-size cost and a comparison
/// with SQLite. Engine selection is deferred until connection time (the API is
/// engine-agnostic), so adding this feature does not lock an app to a single
/// choice.
const DATABASE_TURSO: &[Contribution] = &[Contribution::CargoFeature {
    name: "frust-database",
    feature: "engine-turso",
}];

/// database's single optional feature.
const DATABASE_FEATURES: &[FeatureSpec] = &[FeatureSpec {
    id: "engine-turso",
    summary: "Adds the Turso (Rust SQLite rewrite) engine alongside SQLite. \
              Significant binary-size cost (see plugin README); enables future \
              vector-search/cloud-sync capabilities. Engine chosen at open time.",
    contributions: DATABASE_TURSO,
}];

const DATABASE: PluginSpec = PluginSpec {
    id: "database",
    summary: "Embedded SQL database (SQLite via rusqlite; engine-agnostic API).",
    crate_dir: "database",
    base: &[Contribution::CargoDep {
        name: "frust-database",
    }],
    optional_features: DATABASE_FEATURES,
    requires_sibling: None,
};

/// `i18n`'s base contributions — a Cargo dependency, a starter locale file,
/// and a macro invocation to load the locale bundles at compile time.
///
/// [`Contribution::ScaffoldFile`] must come **first**: the starter locale file
/// is created before the [`Contribution::AppCrateMacro`] invocation references
/// it, and the macro's expansion really does walk the `locales/` directory at
/// compile time — a missing fallback tree is a compile error, so the seeded
/// file is what keeps a freshly-wired scaffold building. [`Contribution::CargoDep`] adds
/// the dependency. [`Contribution::AppCrateMacro`] invokes the compile-time
/// `frust_i18n::locales!` macro, which validates and loads the bundle
/// directory — the entry point into the plugin's API.
///
/// No Android manifest permission, plist key, Gradle module, or Swift package
/// is needed: this is a pure-Rust plugin with no OS-side integration (see
/// `plugins/i18n/README.md`).
const I18N_BASE: &[Contribution] = &[
    Contribution::ScaffoldFile {
        rel_path: "locales/en/main.ftl",
        contents: "# English locale — add sibling directories (de/, fr/, ...) with the same file names.\n\
                   # Syntax: https://projectfluent.org/fluent/guide/\n\
                   hello = Hello, { $name }!\n",
        comment: "Starter English Fluent locale file",
    },
    Contribution::CargoDep { name: "frust-i18n" },
    Contribution::AppCrateMacro {
        invocation: "frust_i18n::locales!(\"locales\");",
        cfg: None,
        comment: "Load and compile Fluent locale bundles (see plugins/i18n/README.md).",
    },
];

const I18N: PluginSpec = PluginSpec {
    id: "i18n",
    summary: "Fluent Project-based internationalization/localization: locale-aware message \
              resolution and system-locale detection.",
    crate_dir: "i18n",
    base: I18N_BASE,
    optional_features: &[],
    requires_sibling: None,
};

/// `glyph`'s single base contribution — a Cargo dependency and nothing else.
/// Like `database`/`i18n`, this is a pure Cargo-graph addition with no
/// OS-side integration: the registry only wires the dependency in, exactly
/// the shape `frust create`'s own scaffold uses. Making the design system
/// *active* still needs an explicit `frust_glyph::install()` call in the
/// app's own `app!(setup = {..})` block — the registry cannot add that call
/// for the caller, since it doesn't know which design system (if any) the
/// app already has installed there.
const GLYPH: PluginSpec = PluginSpec {
    id: "glyph",
    summary: "Glyph catalog; needs frust_glyph::install() in app! setup \
              (widgets + tokens + bundled fonts).",
    crate_dir: "glyph",
    base: &[Contribution::CargoDep {
        name: "frust-glyph",
    }],
    optional_features: &[],
    requires_sibling: None,
};

/// `material`'s single base contribution — see [`GLYPH`]'s doc comment for
/// the shape and the same install-call caveat.
const MATERIAL: PluginSpec = PluginSpec {
    id: "material",
    summary: "Material 3 catalog; needs frust_material::install() in app! \
              setup (widgets + tokens).",
    crate_dir: "material",
    base: &[Contribution::CargoDep {
        name: "frust-material",
    }],
    optional_features: &[],
    requires_sibling: None,
};

/// `cupertino`'s single base contribution — see [`GLYPH`]'s doc comment for
/// the shape and the same install-call caveat.
const CUPERTINO: PluginSpec = PluginSpec {
    id: "cupertino",
    summary: "Cupertino catalog; needs frust_cupertino::install() in app! \
              setup (widgets + tokens).",
    crate_dir: "cupertino",
    base: &[Contribution::CargoDep {
        name: "frust-cupertino",
    }],
    optional_features: &[],
    requires_sibling: None,
};

/// `shadcn`'s single base contribution — see [`GLYPH`]'s doc comment for the
/// shape and the same install-call caveat. The tier's first *external-origin*
/// catalog (ported from shadcn/ui rather than authored here — see
/// `plugins/shadcn/src/lib.rs`), but the same pure-Cargo-dependency shape as
/// `glyph`/`material`/`cupertino`: no OS-side integration, and making it the
/// app's active theme is still a manual `frust_shadcn::install()` call in
/// `app!(setup = {..})` the registry cannot add on the caller's behalf (it
/// has no way to know where that block should go, or whether one is already
/// installing another design system there).
const SHADCN: PluginSpec = PluginSpec {
    id: "shadcn",
    summary: "shadcn/ui catalog; needs frust_shadcn::install() in app! setup \
              (48 components + tokens + bundled Inter/JetBrains Mono fonts).",
    crate_dir: "shadcn",
    base: &[Contribution::CargoDep {
        name: "frust-shadcn",
    }],
    optional_features: &[],
    requires_sibling: None,
};

/// The v1 static plugin registry (Vec-factory convention). A caller (the CLI
/// or the TUI Add Plugin dialog) enumerates this to drive selection without
/// hardcoding plugin ids.
pub fn known_plugins() -> Vec<PluginSpec> {
    vec![
        SHARED_PREFERENCES,
        SECURE_STORAGE,
        CLEAN_SIGNALS_FRUST,
        CAMERA,
        NATIVE_WIDGETS,
        CLIPBOARD,
        HAPTICS,
        URL_LAUNCHER,
        AUTH_SESSION,
        IAP,
        VIDEO_PLAYER,
        DATABASE,
        I18N,
        GLYPH,
        MATERIAL,
        CUPERTINO,
        SHADCN,
    ]
}

/// Look up a registry entry by id.
pub(crate) fn find_plugin(id: &str) -> Option<PluginSpec> {
    known_plugins().into_iter().find(|p| p.id == id)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::plugin::{AddOutcome, add_plugin};
    use crate::scaffold::{self, TemplateContext};
    use std::collections::BTreeMap;
    use std::fs;
    use std::path::{Path, PathBuf};
    use std::sync::atomic::{AtomicU32, Ordering};

    #[test]
    fn registry_lists_the_seventeen_v1_plugins() {
        let ids: Vec<&str> = known_plugins().iter().map(|p| p.id).collect();
        assert_eq!(
            ids,
            vec![
                "shared-preferences",
                "secure-storage",
                "clean-signals-frust",
                "camera",
                "native-widgets",
                "clipboard",
                "haptics",
                "url-launcher",
                "auth-session",
                "iap",
                "video-player",
                "database",
                "i18n",
                "glyph",
                "material",
                "cupertino",
                "shadcn",
            ]
        );
    }

    /// The `glyph`/`material`/`cupertino`/`shadcn` entries are each exactly
    /// one `CargoDep` and nothing else — no manifest permission, plist key,
    /// Gradle module, or Swift package, since these are pure-Rust design
    /// systems built on `frust::authoring` alone (see `GLYPH`'s doc
    /// comment) — `shadcn` included, despite being the tier's first
    /// external-origin catalog: its registry shape is identical.
    ///
    /// `url-launcher` joins the same assertion despite being a real platform
    /// plugin, not a design system: it is the first such plugin whose base
    /// is a bare `CargoDep` with no permission, plist key, Gradle module, or
    /// Swift package at all (see `URL_LAUNCHER_BASE`'s doc comment) — the
    /// same single-contribution shape this test already checks, so it is
    /// grouped here rather than duplicated into its own test.
    #[test]
    fn design_systems_and_url_launcher_are_each_a_cargo_dep_and_nothing_else() {
        for (id, crate_name) in [
            ("glyph", "frust-glyph"),
            ("material", "frust-material"),
            ("cupertino", "frust-cupertino"),
            ("shadcn", "frust-shadcn"),
            ("url-launcher", "frust-url-launcher"),
        ] {
            let spec = find_plugin(id).unwrap();
            assert_eq!(spec.crate_dir, id);
            assert!(spec.optional_features.is_empty());
            assert_eq!(spec.requires_sibling, None);

            assert_eq!(spec.base.len(), 1, "{:?}", spec.base);
            assert!(
                matches!(
                    spec.base[0],
                    Contribution::CargoDep { name } if name == crate_name
                ),
                "{:?}",
                spec.base
            );
        }
    }

    /// The `haptics` entry is exactly a `CargoDep` plus one
    /// `ManifestPermission` — no optional features, no sibling, and (unlike
    /// every other permission-carrying entry) no `GradleModule`, since this
    /// plugin's Android backend is plain JNI with no Kotlin helper class to
    /// carry the permission inside (see `HAPTICS_BASE`'s own doc comment).
    #[test]
    fn haptics_is_a_cargo_dep_plus_one_manifest_permission_and_nothing_else() {
        let spec = find_plugin("haptics").unwrap();
        assert_eq!(spec.crate_dir, "haptics");
        assert!(spec.optional_features.is_empty());
        assert_eq!(spec.requires_sibling, None);

        assert_eq!(spec.base.len(), 2, "{:?}", spec.base);
        assert!(matches!(
            spec.base[0],
            Contribution::CargoDep {
                name: "frust-haptics"
            }
        ));
        assert!(matches!(
            spec.base[1],
            Contribution::ManifestPermission {
                permission: "android.permission.VIBRATE"
            }
        ));
    }

    /// The `clipboard` entry is exactly one `CargoDep` — no manifest
    /// permission, plist key, Gradle module, or Swift package (see
    /// `CLIPBOARD`'s own doc comment for why none apply).
    #[test]
    fn clipboard_is_a_cargo_dep_and_nothing_else() {
        let spec = find_plugin("clipboard").unwrap();
        assert_eq!(spec.crate_dir, "clipboard");
        assert!(spec.optional_features.is_empty());
        assert_eq!(spec.requires_sibling, None);

        assert_eq!(spec.base.len(), 1, "{:?}", spec.base);
        assert!(matches!(
            spec.base[0],
            Contribution::CargoDep {
                name: "frust-clipboard"
            }
        ));
    }

    /// The `database` entry is exactly one `CargoDep` — no manifest
    /// permission, plist key, Gradle module, or Swift package, since this is
    /// a pure-Rust plugin with no OS-side integration. It now carries one
    /// optional feature (`engine-turso`), the first zero-OS-side feature: a
    /// pure Cargo-feature flip.
    #[test]
    fn database_is_a_cargo_dep_and_nothing_else() {
        let spec = find_plugin("database").unwrap();
        assert_eq!(spec.crate_dir, "database");
        assert_eq!(spec.optional_features.len(), 1);
        assert_eq!(spec.requires_sibling, None);

        assert_eq!(spec.base.len(), 1, "{:?}", spec.base);
        assert!(matches!(
            spec.base[0],
            Contribution::CargoDep {
                name: "frust-database"
            }
        ));
    }

    /// The `database` entry's `engine-turso` feature is exactly one
    /// `CargoFeature` — no manifest permission, plist key, Gradle module,
    /// Swift package, or app-crate macro. It's the registry's first pure
    /// Cargo-feature flip with zero OS-side integration (see `DATABASE_TURSO`'s
    /// own doc comment for why).
    #[test]
    fn database_engine_turso_is_a_cargo_feature_and_nothing_else() {
        let spec = find_plugin("database").unwrap();
        let feature = spec
            .optional_features
            .iter()
            .find(|f| f.id == "engine-turso")
            .expect("engine-turso feature");

        assert_eq!(
            feature.contributions.len(),
            1,
            "{:?}",
            feature.contributions
        );
        assert!(matches!(
            feature.contributions[0],
            Contribution::CargoFeature {
                name: "frust-database",
                feature: "engine-turso"
            }
        ));
    }

    /// The `native-widgets` entry is exactly a Cargo dependency plus the
    /// plugin's own Android library module — and the **absences** are the
    /// assertion that matters (see `NATIVE_WIDGETS_BASE`'s doc): no
    /// `SwiftPackageRef`, because that arm ships zero Swift by design, and no
    /// `PlistEntry`/`ManifestPermission`, because hosting an OS control needs
    /// neither. A later widening would have to come here and say why.
    #[test]
    fn native_widgets_is_a_cargo_dep_plus_one_gradle_module_and_nothing_else() {
        let spec = find_plugin("native-widgets").unwrap();
        assert_eq!(spec.crate_dir, "native-widgets");
        assert!(spec.optional_features.is_empty());
        assert_eq!(spec.requires_sibling, None);

        assert_eq!(spec.base.len(), 2, "{:?}", spec.base);
        assert!(matches!(
            spec.base[0],
            Contribution::CargoDep {
                name: "frust-native-widgets"
            }
        ));
        assert!(matches!(
            spec.base[1],
            Contribution::GradleModule {
                gradle_name: ":frust-native-widgets",
                rel_path: "plugins/native-widgets/platform/android",
            }
        ));
        assert!(
            !spec
                .base
                .iter()
                .any(|c| matches!(c, Contribution::SwiftPackageRef { .. })),
            "native-widgets ships no Swift — its iOS factory is a Rust \
             define_class! type resolved by NSClassFromString"
        );
    }

    /// The `video-player` entry mirrors `native-widgets`' exact
    /// two-contribution shape (see `VIDEO_PLAYER_BASE`'s doc): a Cargo
    /// dependency plus the plugin's own Android library module, and the
    /// same absences matter here too — no `SwiftPackageRef` (the Apple
    /// factories are pure-Rust `define_class!` types), no
    /// `PlistEntry`/`ManifestPermission` (no usage-description string is
    /// needed, and `INTERNET` rides the plugin's own Android manifest), and
    /// no `AppCrateMacro` (nothing here needs an LTO-survival shim).
    #[test]
    fn video_player_is_a_cargo_dep_plus_one_gradle_module_and_nothing_else() {
        let spec = find_plugin("video-player").unwrap();
        assert_eq!(spec.crate_dir, "video-player");
        assert!(spec.optional_features.is_empty());
        assert_eq!(spec.requires_sibling, None);

        assert_eq!(spec.base.len(), 2, "{:?}", spec.base);
        assert!(matches!(
            spec.base[0],
            Contribution::CargoDep {
                name: "frust-video-player"
            }
        ));
        assert!(matches!(
            spec.base[1],
            Contribution::GradleModule {
                gradle_name: ":frust-video-player",
                rel_path: "plugins/video-player/platform/android",
            }
        ));
        assert!(
            !spec
                .base
                .iter()
                .any(|c| matches!(c, Contribution::SwiftPackageRef { .. })),
            "video-player ships no Swift — its iOS and macOS factories are \
             Rust define_class! types"
        );
    }

    /// The `camera` entry's base contributions, exactly [`CAMERA_BASE`]'s own
    /// list — no more, no fewer, in application order, and no optional
    /// features / sibling requirement.
    #[test]
    fn camera_base_contributions_match_the_final_accounting() {
        let spec = find_plugin("camera").unwrap();
        assert_eq!(spec.crate_dir, "camera");
        assert!(spec.optional_features.is_empty());
        assert_eq!(spec.requires_sibling, None);

        assert_eq!(spec.base.len(), 5);
        assert!(matches!(
            spec.base[0],
            Contribution::CargoDep {
                name: "frust-camera"
            }
        ));
        assert!(matches!(
            spec.base[1],
            Contribution::PlistEntry {
                key: "NSCameraUsageDescription",
                ..
            }
        ));
        assert!(matches!(
            spec.base[2],
            Contribution::GradleModule {
                gradle_name: ":frust-camera",
                rel_path: "plugins/camera/platform/android",
            }
        ));
        assert!(matches!(
            spec.base[3],
            Contribution::SwiftPackageRef {
                package_name: "FrustCamera",
                rel_path: "plugins/camera/platform/ios",
            }
        ));
        // The device-gate addition (see `CAMERA_BASE`'s doc comment). Pinned
        // by exact invocation because this string IS the idempotence key,
        // and because it is what keeps a fresh
        // scaffold's iOS link from failing on `_frust_camera_session_handle`.
        assert!(matches!(
            spec.base[4],
            Contribution::AppCrateMacro {
                invocation: "frust_camera::ios_exports!();",
                cfg: Some("target_vendor = \"apple\""),
                ..
            }
        ));
    }

    /// The `iap` entry's base contributions, exactly [`IAP_BASE`]'s own list
    /// — a Cargo dependency, the plugin's own Android library module, and its
    /// own iOS Swift package, no more and no fewer, in application order —
    /// and no optional features / sibling requirement.
    #[test]
    fn iap_base_contributions_match_the_final_accounting() {
        let spec = find_plugin("iap").unwrap();
        assert_eq!(spec.crate_dir, "iap");
        assert!(spec.optional_features.is_empty());
        assert_eq!(spec.requires_sibling, None);

        assert_eq!(spec.base.len(), 3);
        assert!(matches!(
            spec.base[0],
            Contribution::CargoDep { name: "frust-iap" }
        ));
        assert!(matches!(
            spec.base[1],
            Contribution::GradleModule {
                gradle_name: ":frust-iap",
                rel_path: "plugins/iap/platform/android",
            }
        ));
        assert!(matches!(
            spec.base[2],
            Contribution::SwiftPackageRef {
                package_name: "FrustIap",
                rel_path: "plugins/iap/platform/ios",
            }
        ));
        assert!(
            !spec
                .base
                .iter()
                .any(|c| matches!(c, Contribution::ManifestPermission { .. })),
            "iap must add no ManifestPermission — com.android.vending.BILLING \
             arrives transitively from the openiap-google Play Billing AAR"
        );
    }

    /// The `SwiftPackageRef` counterpart of
    /// `secure_storage_gradle_module_path_exists_in_this_checkout` below — a
    /// typo in `rel_path` would otherwise surface only as a missing
    /// `Package.swift` in a generated Xcode project, far from here.
    #[test]
    fn camera_swift_package_ref_path_exists_in_this_checkout() {
        let repo_root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
        let mut found = false;
        for plugin in known_plugins() {
            for contribution in plugin.base.iter().chain(
                plugin
                    .optional_features
                    .iter()
                    .flat_map(|f| f.contributions.iter()),
            ) {
                if let Contribution::SwiftPackageRef { rel_path, .. } = contribution {
                    found = true;
                    let dir = repo_root.join(rel_path);
                    assert!(
                        dir.join("Package.swift").is_file(),
                        "`{rel_path}` must be a Swift package directory (a \
                         `Package.swift` file) in this checkout"
                    );
                }
            }
        }
        assert!(found, "expected at least one SwiftPackageRef contribution");
    }

    #[test]
    fn secure_storage_biometric_bundles_the_two_readme_contributions() {
        let spec = find_plugin("secure-storage").unwrap();
        let feature = spec
            .optional_features
            .iter()
            .find(|f| f.id == "biometric-gate")
            .expect("biometric-gate feature");
        // plist key + the plugin's own Gradle module. The Android permission,
        // Kotlin helper and R8 keep rule all ride inside the module now.
        assert_eq!(feature.contributions.len(), 2);
        let has_module = feature.contributions.iter().any(|c| {
            matches!(
                c,
                Contribution::GradleModule { gradle_name, rel_path }
                    if *gradle_name == ":frust-secure-storage"
                        && *rel_path == "plugins/secure-storage/platform/android"
            )
        });
        assert!(
            has_module,
            "biometric-gate must include the plugin's Android library module"
        );
    }

    /// The `rel_path` above is a repo-root-relative directory that must
    /// actually exist in this checkout — a typo would otherwise surface only
    /// as a Gradle failure in a generated app, far from here.
    #[test]
    fn secure_storage_gradle_module_path_exists_in_this_checkout() {
        let repo_root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
        for plugin in known_plugins() {
            for contribution in plugin.base.iter().chain(
                plugin
                    .optional_features
                    .iter()
                    .flat_map(|f| f.contributions.iter()),
            ) {
                if let Contribution::GradleModule { rel_path, .. } = contribution {
                    let dir = repo_root.join(rel_path);
                    assert!(
                        dir.join("build.gradle.kts").is_file(),
                        "`{rel_path}` must be a Gradle module directory in this checkout"
                    );
                }
            }
        }
    }

    #[test]
    fn clean_signals_frust_declares_no_sibling_requirement() {
        // clean-signals is git+rev-pinned to its public repo; the
        // plugin no longer needs a `../clean-signals-rs` sibling checkout.
        let spec = find_plugin("clean-signals-frust").unwrap();
        assert_eq!(spec.requires_sibling, None);
    }

    #[test]
    fn unknown_id_is_none() {
        assert!(find_plugin("nope").is_none());
    }

    // -------------------------------------------------------------------
    // End-to-end idempotence, driven through the real registry entry — a
    // fresh scaffold, `add_plugin(.., "camera", ..)`, applied twice.
    // `apply.rs` itself owns per-contribution unit tests against a
    // stand-in `PKG_REL` (its `SwiftPackageRef` section's own comment);
    // these instead exercise the *actual* `camera` registry entry end to
    // end, the four contributions together.
    // -------------------------------------------------------------------

    fn unique_temp_dir(tag: &str) -> PathBuf {
        static COUNTER: AtomicU32 = AtomicU32::new(0);
        let n = COUNTER.fetch_add(1, Ordering::Relaxed);
        let dir = std::env::temp_dir().join(format!(
            "frust-drive-registry-test-{tag}-{}-{n}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&dir);
        dir
    }

    /// A scaffold context whose `frust_path` deliberately does NOT exist on
    /// disk, mirroring `apply.rs`'s own `test_context` — the module
    /// directory / package directory need not exist for `add_plugin` to
    /// succeed (that's Gradle/Xcode's problem at build time).
    fn test_context() -> TemplateContext {
        TemplateContext {
            project_name: "my_app".into(),
            title_case_name: "My App".into(),
            org: "dev.f0x".into(),
            description: "A new Frust application.".into(),
            frust_version: "0.1.0".into(),
            frust_path: "/nonexistent/frust/checkout".into(),
            deeplink_scheme: None,
            deeplink_host: None,
        }
    }

    fn scaffold_project(tag: &str) -> PathBuf {
        let dest = unique_temp_dir(tag);
        scaffold::generate(&dest, &test_context(), None, false, None).unwrap();
        dest
    }

    fn snapshot_tree(root: &Path) -> BTreeMap<String, Vec<u8>> {
        fn walk(dir: &Path, root: &Path, out: &mut BTreeMap<String, Vec<u8>>) {
            for entry in fs::read_dir(dir).unwrap() {
                let path = entry.unwrap().path();
                if path.is_dir() {
                    walk(&path, root, out);
                } else {
                    let rel = path
                        .strip_prefix(root)
                        .unwrap()
                        .to_string_lossy()
                        .replace('\\', "/");
                    out.insert(rel, fs::read(&path).unwrap());
                }
            }
        }
        let mut out = BTreeMap::new();
        walk(root, root, &mut out);
        out
    }

    /// No duplicate/dangling/orphaned pbxproj object id and balanced
    /// `Begin`/`End` section markers — a lightweight stand-in for the
    /// scaffold test suite's own `pbx_*` structural checks (private to that
    /// module's tests, and to `apply.rs`'s own, so each of the three test
    /// modules that touches a pbxproj carries its own copy of this check —
    /// `apply.rs`'s `assert_pbxproj_well_formed` doc comment notes the same
    /// duplication against `scaffold/mod.rs`'s).
    fn assert_pbxproj_well_formed(pbxproj: &str) {
        assert_eq!(
            pbxproj.matches("/* Begin ").count(),
            pbxproj.matches("/* End ").count(),
            "unbalanced PBX sections:\n{pbxproj}"
        );
        let mut defined = Vec::new();
        for line in pbxproj.lines() {
            let Some(rest) = line.strip_prefix("\t\t") else {
                continue;
            };
            if rest.starts_with('\t') || !rest.contains(" = {") {
                continue;
            }
            let id = rest.split_whitespace().next().unwrap_or_default();
            if id.len() == 24 && id.chars().all(|c| c.is_ascii_hexdigit()) {
                defined.push(id.to_string());
            }
        }
        let mut unique = defined.clone();
        unique.sort_unstable();
        unique.dedup();
        assert_eq!(
            unique.len(),
            defined.len(),
            "duplicate PBX object id(s):\n{pbxproj}"
        );
    }

    #[test]
    fn camera_add_plugin_applies_every_contribution_and_reapply_is_idempotent() {
        let root = scaffold_project("camera-idempotence");

        let first = add_plugin(&root, "camera", &[]).unwrap();
        assert_eq!(first.plugin_id, "camera");
        assert_eq!(first.items.len(), 5, "{first:?}");
        assert!(
            first.items.iter().all(|i| i.outcome == AddOutcome::Applied),
            "{first:?}"
        );

        // The Cargo dependency, Info.plist key, Gradle module wiring, Swift
        // package reference and app-crate export shim all actually landed.
        let cargo = fs::read_to_string(root.join("Cargo.toml")).unwrap();
        assert!(cargo.contains("frust-camera"), "{cargo}");
        let plist = fs::read_to_string(root.join("ios/Runner/Info.plist")).unwrap();
        assert!(plist.contains("NSCameraUsageDescription"), "{plist}");
        let settings = fs::read_to_string(root.join("android/settings.gradle.kts")).unwrap();
        assert!(
            settings.contains("include(\":frust-camera\")"),
            "{settings}"
        );
        let app_build = fs::read_to_string(root.join("android/app/build.gradle.kts")).unwrap();
        assert!(
            app_build.contains("implementation(project(\":frust-camera\"))"),
            "{app_build}"
        );
        let pbxproj =
            fs::read_to_string(root.join("ios/Runner.xcodeproj/project.pbxproj")).unwrap();
        assert!(pbxproj.contains("FrustCamera"), "{pbxproj}");
        assert_pbxproj_well_formed(&pbxproj);
        let lib_rs = fs::read_to_string(root.join("src/lib.rs")).unwrap();
        assert!(lib_rs.contains("frust_camera::ios_exports!();"), "{lib_rs}");
        assert!(
            lib_rs.contains("#[cfg(target_vendor = \"apple\")]"),
            "{lib_rs}"
        );

        let after_first = snapshot_tree(&root);
        let second = add_plugin(&root, "camera", &[]).unwrap();
        assert_eq!(second.items.len(), 5);
        assert!(
            second
                .items
                .iter()
                .all(|i| i.outcome == AddOutcome::AlreadyPresent),
            "{second:?}"
        );
        assert_eq!(
            after_first,
            snapshot_tree(&root),
            "a second apply must leave a byte-identical tree"
        );

        let _ = fs::remove_dir_all(&root);
    }

    /// The `native-widgets` counterpart of the camera end-to-end case above:
    /// a fresh scaffold, `add_plugin(.., "native-widgets", ..)` applied
    /// twice. The second apply must report `AlreadyPresent` for both
    /// contributions and leave a byte-identical tree — the idempotence
    /// contract `docs/CODE_STANDARDS.md`'s Plugin Conventions requires of
    /// every generated-project mutation.
    #[test]
    fn native_widgets_add_plugin_applies_both_contributions_and_reapply_is_idempotent() {
        let root = scaffold_project("native-widgets-idempotence");

        let first = add_plugin(&root, "native-widgets", &[]).unwrap();
        assert_eq!(first.plugin_id, "native-widgets");
        assert_eq!(first.items.len(), 2, "{first:?}");
        assert!(
            first.items.iter().all(|i| i.outcome == AddOutcome::Applied),
            "{first:?}"
        );

        let cargo = fs::read_to_string(root.join("Cargo.toml")).unwrap();
        assert!(cargo.contains("frust-native-widgets"), "{cargo}");
        let settings = fs::read_to_string(root.join("android/settings.gradle.kts")).unwrap();
        assert!(
            settings.contains("include(\":frust-native-widgets\")"),
            "{settings}"
        );
        assert!(
            settings.contains("rootDir.resolve(\"../build/android/frust-native-widgets\")"),
            "{settings}"
        );
        let app_build = fs::read_to_string(root.join("android/app/build.gradle.kts")).unwrap();
        assert!(
            app_build.contains("implementation(project(\":frust-native-widgets\"))"),
            "{app_build}"
        );

        // The iOS side must be untouched: no Swift package reference, no
        // plist key. A regression here would mean the entry grew a
        // contribution its own registry doc says it does not have.
        let pbxproj =
            fs::read_to_string(root.join("ios/Runner.xcodeproj/project.pbxproj")).unwrap();
        assert!(
            !pbxproj.contains("FrustNativeWidgets"),
            "native-widgets must add no Swift package reference"
        );
        assert_pbxproj_well_formed(&pbxproj);

        let after_first = snapshot_tree(&root);
        let second = add_plugin(&root, "native-widgets", &[]).unwrap();
        assert_eq!(second.items.len(), 2);
        assert!(
            second
                .items
                .iter()
                .all(|i| i.outcome == AddOutcome::AlreadyPresent),
            "{second:?}"
        );
        assert_eq!(
            after_first,
            snapshot_tree(&root),
            "a second apply must leave a byte-identical tree"
        );

        let _ = fs::remove_dir_all(&root);
    }

    /// The `haptics` counterpart of the camera/native-widgets end-to-end
    /// cases above, and the registry's first exercise of
    /// `Contribution::ManifestPermission` end to end (`HAPTICS_BASE`'s doc
    /// comment): a fresh scaffold, `add_plugin(.., "haptics", ..)` applied
    /// twice. The second apply must report `AlreadyPresent` for both
    /// contributions and leave a byte-identical tree.
    #[test]
    fn haptics_add_plugin_applies_both_contributions_and_reapply_is_idempotent() {
        let root = scaffold_project("haptics-idempotence");

        let first = add_plugin(&root, "haptics", &[]).unwrap();
        assert_eq!(first.plugin_id, "haptics");
        assert_eq!(first.items.len(), 2, "{first:?}");
        assert!(
            first.items.iter().all(|i| i.outcome == AddOutcome::Applied),
            "{first:?}"
        );

        let cargo = fs::read_to_string(root.join("Cargo.toml")).unwrap();
        assert!(cargo.contains("frust-haptics"), "{cargo}");
        let manifest =
            fs::read_to_string(root.join("android/app/src/main/AndroidManifest.xml")).unwrap();
        assert!(
            manifest.contains("android.permission.VIBRATE"),
            "{manifest}"
        );

        // No Gradle module, no Swift package, no plist key — this entry's
        // own doc says the Android addition rides a manifest permission
        // alone (`HAPTICS_BASE`'s comment).
        let settings = fs::read_to_string(root.join("android/settings.gradle.kts")).unwrap();
        assert!(
            !settings.contains("frust-haptics"),
            "haptics must add no Gradle module include"
        );
        let pbxproj =
            fs::read_to_string(root.join("ios/Runner.xcodeproj/project.pbxproj")).unwrap();
        assert!(
            !pbxproj.contains("FrustHaptics"),
            "haptics must add no Swift package reference"
        );
        assert_pbxproj_well_formed(&pbxproj);

        let after_first = snapshot_tree(&root);
        let second = add_plugin(&root, "haptics", &[]).unwrap();
        assert_eq!(second.items.len(), 2);
        assert!(
            second
                .items
                .iter()
                .all(|i| i.outcome == AddOutcome::AlreadyPresent),
            "{second:?}"
        );
        assert_eq!(
            after_first,
            snapshot_tree(&root),
            "a second apply must leave a byte-identical tree"
        );

        let _ = fs::remove_dir_all(&root);
    }

    /// The `iap` counterpart of the camera/native-widgets/haptics end-to-end
    /// cases above — the registry's second `GradleModule` +
    /// `SwiftPackageRef` pairing, and the first with neither a `PlistEntry`
    /// nor an `AppCrateMacro` (`IAP_BASE`'s doc comment says why). A fresh
    /// scaffold, `add_plugin(.., "iap", ..)` applied twice: the app's
    /// Cargo.toml gains the dependency, `settings.gradle.kts` gains the
    /// include trio, the app's `build.gradle.kts` gains the project
    /// dependency, and `project.pbxproj` gains the `FrustIap` package
    /// reference — the second apply must report every item
    /// `AlreadyPresent` and leave a byte-identical tree.
    #[test]
    fn iap_add_plugin_applies_every_contribution_and_reapply_is_idempotent() {
        let root = scaffold_project("iap-idempotence");

        let pre_plist = fs::read_to_string(root.join("ios/Runner/Info.plist")).unwrap();
        let pre_lib_rs = fs::read_to_string(root.join("src/lib.rs")).unwrap();

        let first = add_plugin(&root, "iap", &[]).unwrap();
        assert_eq!(first.plugin_id, "iap");
        assert_eq!(first.items.len(), 3, "{first:?}");
        assert!(
            first.items.iter().all(|i| i.outcome == AddOutcome::Applied),
            "{first:?}"
        );

        // The Cargo dependency, Gradle module wiring and Swift package
        // reference all actually landed.
        let cargo = fs::read_to_string(root.join("Cargo.toml")).unwrap();
        assert!(cargo.contains("frust-iap"), "{cargo}");
        let settings = fs::read_to_string(root.join("android/settings.gradle.kts")).unwrap();
        assert!(settings.contains("include(\":frust-iap\")"), "{settings}");
        assert!(
            settings.contains("rootDir.resolve(\"../build/android/frust-iap\")"),
            "{settings}"
        );
        let app_build = fs::read_to_string(root.join("android/app/build.gradle.kts")).unwrap();
        assert!(
            app_build.contains("implementation(project(\":frust-iap\"))"),
            "{app_build}"
        );
        let pbxproj =
            fs::read_to_string(root.join("ios/Runner.xcodeproj/project.pbxproj")).unwrap();
        assert!(pbxproj.contains("FrustIap"), "{pbxproj}");
        assert_pbxproj_well_formed(&pbxproj);

        // No Info.plist key and no app-crate macro invocation — the
        // absences `IAP_BASE`'s own doc comment states.
        let plist = fs::read_to_string(root.join("ios/Runner/Info.plist")).unwrap();
        assert_eq!(plist, pre_plist, "iap must add no Info.plist key");
        let lib_rs = fs::read_to_string(root.join("src/lib.rs")).unwrap();
        assert_eq!(
            lib_rs, pre_lib_rs,
            "iap must add no app-crate macro invocation"
        );

        let after_first = snapshot_tree(&root);
        let second = add_plugin(&root, "iap", &[]).unwrap();
        assert_eq!(second.items.len(), 3);
        assert!(
            second
                .items
                .iter()
                .all(|i| i.outcome == AddOutcome::AlreadyPresent),
            "{second:?}"
        );
        assert_eq!(
            after_first,
            snapshot_tree(&root),
            "a second apply must leave a byte-identical tree"
        );

        let _ = fs::remove_dir_all(&root);
    }

    /// The `database` plugin with the `engine-turso` feature applied: a fresh
    /// scaffold, `add_plugin(.., "database", &["engine-turso"])` applied twice.
    /// The first apply reports two items (`Applied`: the base CargoDep and the
    /// optional CargoFeature), and the app's Cargo.toml gains
    /// `frust-database = { path = ..., features = ["engine-turso"] }`. The
    /// second apply must report every item `AlreadyPresent` and leave a
    /// byte-identical tree — the idempotence contract.
    ///
    /// This is the registry's first zero-OS-side optional feature, exercising
    /// the pure Cargo-feature flip with no platform-side machinery.
    #[test]
    fn database_add_plugin_with_engine_turso_feature_and_reapply_is_idempotent() {
        let root = scaffold_project("database-engine-turso-idempotence");

        let first = add_plugin(&root, "database", &["engine-turso"]).unwrap();
        assert_eq!(first.plugin_id, "database");
        assert_eq!(first.items.len(), 2, "{first:?}");
        assert!(
            first.items.iter().all(|i| i.outcome == AddOutcome::Applied),
            "{first:?}"
        );

        // The Cargo dependency with the feature enabled actually landed.
        let cargo = fs::read_to_string(root.join("Cargo.toml")).unwrap();
        assert!(cargo.contains("frust-database"), "{cargo}");
        assert!(
            cargo.contains("features = [\"engine-turso\"]"),
            "Cargo.toml must contain the engine-turso feature: {cargo}"
        );

        // No Android manifest permission, iOS plist key, Gradle module, or
        // Swift package — the absences this pure-Rust plugin's own registry
        // entry states (and the zero-OS-side feature reinforces).
        let manifest =
            fs::read_to_string(root.join("android/app/src/main/AndroidManifest.xml")).unwrap();
        assert!(
            !manifest.contains("database"),
            "database must add no manifest permission"
        );
        let plist = fs::read_to_string(root.join("ios/Runner/Info.plist")).unwrap();
        assert!(
            !plist.contains("database"),
            "database must add no plist key"
        );
        let settings = fs::read_to_string(root.join("android/settings.gradle.kts")).unwrap();
        assert!(
            !settings.contains("database"),
            "database must add no Gradle module include"
        );
        let pbxproj =
            fs::read_to_string(root.join("ios/Runner.xcodeproj/project.pbxproj")).unwrap();
        assert!(
            !pbxproj.contains("FrustDatabase"),
            "database must add no Swift package reference"
        );
        assert_pbxproj_well_formed(&pbxproj);

        let after_first = snapshot_tree(&root);
        let second = add_plugin(&root, "database", &["engine-turso"]).unwrap();
        assert_eq!(second.items.len(), 2);
        assert!(
            second
                .items
                .iter()
                .all(|i| i.outcome == AddOutcome::AlreadyPresent),
            "{second:?}"
        );
        assert_eq!(
            after_first,
            snapshot_tree(&root),
            "a second apply must leave a byte-identical tree"
        );

        let _ = fs::remove_dir_all(&root);
    }

    /// The `i18n` entry's base contributions, exactly [`I18N_BASE`]'s own list
    /// — a starter Fluent locale file, a Cargo dependency, and a macro
    /// invocation, no more and no fewer, in application order (scaffolded file
    /// first, then the dependency, then the macro), and no optional features /
    /// sibling requirement.
    #[test]
    fn i18n_base_contributions_match_the_final_accounting() {
        let spec = find_plugin("i18n").unwrap();
        assert_eq!(spec.crate_dir, "i18n");
        assert!(spec.optional_features.is_empty());
        assert_eq!(spec.requires_sibling, None);

        assert_eq!(spec.base.len(), 3);
        assert!(matches!(
            spec.base[0],
            Contribution::ScaffoldFile {
                rel_path: "locales/en/main.ftl",
                ..
            }
        ));
        assert!(matches!(
            spec.base[1],
            Contribution::CargoDep { name: "frust-i18n" }
        ));
        assert!(matches!(
            spec.base[2],
            Contribution::AppCrateMacro {
                invocation: "frust_i18n::locales!(\"locales\");",
                cfg: None,
                ..
            }
        ));
    }

    /// The `i18n` plugin end to end: a fresh scaffold, `add_plugin(..,
    /// "i18n", ..)` applied twice. The first apply reports three items
    /// (`Applied`: ScaffoldFile, CargoDep, AppCrateMacro), and the app gains
    /// the starter `locales/en/main.ftl` file, the Cargo dependency, and the
    /// macro invocation. The second apply must report every item
    /// `AlreadyPresent` and leave a byte-identical tree — the idempotence
    /// contract.
    ///
    /// This is the registry's first exercise of [`Contribution::ScaffoldFile`]
    /// end to end, and the first pure-Rust plugin after `database` to add an
    /// `AppCrateMacro` without OS-side machinery.
    #[test]
    fn i18n_add_plugin_applies_every_contribution_and_reapply_is_idempotent() {
        let root = scaffold_project("i18n-idempotence");

        let first = add_plugin(&root, "i18n", &[]).unwrap();
        assert_eq!(first.plugin_id, "i18n");
        assert_eq!(first.items.len(), 3, "{first:?}");
        assert!(
            first.items.iter().all(|i| i.outcome == AddOutcome::Applied),
            "{first:?}"
        );

        // The Cargo dependency, starter locale file, and macro invocation all
        // actually landed.
        let cargo = fs::read_to_string(root.join("Cargo.toml")).unwrap();
        assert!(cargo.contains("frust-i18n"), "{cargo}");
        let locale_file = fs::read_to_string(root.join("locales/en/main.ftl")).unwrap();
        assert!(
            locale_file.contains("hello = Hello, { $name }!"),
            "{locale_file}"
        );
        let lib_rs = fs::read_to_string(root.join("src/lib.rs")).unwrap();
        assert!(
            lib_rs.contains("frust_i18n::locales!(\"locales\");"),
            "{lib_rs}"
        );

        // No Android manifest permission, iOS plist key, Gradle module, or
        // Swift package — the absences this pure-Rust plugin's own registry
        // entry states (`I18N_BASE`'s doc comment).
        let manifest =
            fs::read_to_string(root.join("android/app/src/main/AndroidManifest.xml")).unwrap();
        assert!(
            !manifest.contains("i18n"),
            "i18n must add no manifest permission"
        );
        let plist = fs::read_to_string(root.join("ios/Runner/Info.plist")).unwrap();
        assert!(!plist.contains("i18n"), "i18n must add no plist key");
        let settings = fs::read_to_string(root.join("android/settings.gradle.kts")).unwrap();
        assert!(
            !settings.contains("i18n"),
            "i18n must add no Gradle module include"
        );
        let pbxproj =
            fs::read_to_string(root.join("ios/Runner.xcodeproj/project.pbxproj")).unwrap();
        assert!(
            !pbxproj.contains("Frusti18n"),
            "i18n must add no Swift package reference"
        );
        assert_pbxproj_well_formed(&pbxproj);

        let after_first = snapshot_tree(&root);
        let second = add_plugin(&root, "i18n", &[]).unwrap();
        assert_eq!(second.items.len(), 3);
        assert!(
            second
                .items
                .iter()
                .all(|i| i.outcome == AddOutcome::AlreadyPresent),
            "{second:?}"
        );
        assert_eq!(
            after_first,
            snapshot_tree(&root),
            "a second apply must leave a byte-identical tree"
        );

        let _ = fs::remove_dir_all(&root);
    }

    /// The half-applied completion case for `camera` specifically: delete
    /// the Gradle include this entry's `GradleModule` contribution
    /// added, then re-apply — the missing half must complete (report
    /// `Applied`), not skip on the app-side dependency line it still finds
    /// present, and the surviving half must not be duplicated.
    #[test]
    fn camera_half_applied_gradle_include_completes_on_reapply() {
        let root = scaffold_project("camera-half-applied");
        add_plugin(&root, "camera", &[]).unwrap();

        let settings_path = root.join("android/settings.gradle.kts");
        let wired = fs::read_to_string(&settings_path).unwrap();
        assert!(wired.contains("include(\":frust-camera\")"), "{wired}");

        // Strip exactly the block `GradleModule` appended after the
        // `// frust:plugin-includes` anchor (that anchor is the last line
        // of a freshly scaffolded `settings.gradle.kts`, so the addition is
        // everything after it).
        let anchor = "// frust:plugin-includes";
        let anchor_at = wired.find(anchor).expect("plugin-includes anchor");
        let cut_at = anchor_at + anchor.len();
        let (before, added) = wired.split_at(cut_at);
        assert!(
            added.contains(":frust-camera"),
            "expected the camera GradleModule addition after the anchor: {added}"
        );
        fs::write(&settings_path, before).unwrap();

        let app_build_before = fs::read(root.join("android/app/build.gradle.kts")).unwrap();

        let report = add_plugin(&root, "camera", &[]).unwrap();
        let item = report
            .items
            .iter()
            .find(|i| i.description.contains(":frust-camera"))
            .expect("a Gradle module line item");
        assert_eq!(
            item.outcome,
            AddOutcome::Applied,
            "a half-applied module must complete and report Applied: {report:?}"
        );

        let repaired = fs::read_to_string(&settings_path).unwrap();
        assert_eq!(
            repaired.matches("include(\":frust-camera\")").count(),
            1,
            "the include line must be restored exactly once:\n{repaired}"
        );
        assert_eq!(
            repaired.matches("if (path == \":frust-camera\")").count(),
            1,
            "the build-dir redirect must be restored exactly once, not duplicated:\n{repaired}"
        );

        // The app-side dependency half was already wired and must not be
        // touched/duplicated.
        assert_eq!(
            fs::read(root.join("android/app/build.gradle.kts")).unwrap(),
            app_build_before,
            "the already-wired app dependency half must be left untouched"
        );

        let _ = fs::remove_dir_all(&root);
    }
}
