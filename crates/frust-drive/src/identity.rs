//! Resolves the app identity a build must produce: the Android application
//! id or iOS bundle identifier, and the display name when `frust.toml`
//! states one, for a project, a platform and an optional flavor declared in
//! `[flavors]`.
//!
//! Pure: no I/O. Explicit `[android]`/`[ios]` identifiers, derived ones and
//! flavor-suffixed ones go through the same validators
//! `android_run::project` and `ios_run::project` use.

use thiserror::Error;

use crate::manifest::{FlavorError, Manifest};

/// The platform an identity is resolved for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IdentityPlatform {
    Android,
    Ios,
}

impl IdentityPlatform {
    /// How errors name this platform's identifier.
    fn noun(self) -> &'static str {
        match self {
            Self::Android => "Android application id",
            Self::Ios => "iOS bundle identifier",
        }
    }

    fn validate(self, id: &str) -> Result<(), String> {
        match self {
            Self::Android => crate::android_id::validate(id).map_err(|e| e.to_string()),
            Self::Ios => crate::ios_id::validate(id).map_err(|e| e.to_string()),
        }
    }
}

/// What a build must be identified as.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AppIdentity {
    /// Android application id or iOS bundle identifier the build produces.
    pub identifier: String,
    /// Whether `frust.toml` states [`identifier`](Self::identifier) for this
    /// build — an explicit `[android]`/`[ios] identifier`, or a selected
    /// declared flavor with an `app-id-suffix` — so frust applies and checks
    /// it. False for a purely derived identifier.
    pub identifier_stated: bool,
    /// The display name frust applies and checks; `None` when `frust.toml`
    /// states no `[app] display-name` and the selected flavor (if any) no
    /// `app-name-suffix`, so the platform files' own label stands.
    pub display_name: Option<String>,
    /// The requested flavor, if any.
    pub flavor: Option<String>,
    /// Whether `flavor` is declared in `[flavors]`. False for a native
    /// flavor the caller must check itself.
    pub flavor_declared: bool,
}

/// Why an identity could not be resolved.
#[derive(Debug, Error, PartialEq, Eq)]
pub enum IdentityError {
    #[error("invalid `[android] identifier` `{id}`: {reason}")]
    InvalidExplicitAndroid { id: String, reason: String },
    #[error("invalid `[ios] identifier` `{id}`: {reason}")]
    InvalidExplicitIos { id: String, reason: String },
    #[error("derived Android application id `{id}` (from `org`/`name`) is invalid: {reason}")]
    InvalidDerivedAndroid { id: String, reason: String },
    #[error("derived iOS bundle identifier `{id}` (from `org`/`name`) is invalid: {reason}")]
    InvalidDerivedIos { id: String, reason: String },
    /// The selected `[flavors.<name>]` is malformed.
    #[error(transparent)]
    InvalidFlavor(#[from] FlavorError),
    /// The identifier with the flavor's `app-id-suffix` appended fails the
    /// platform's own validator.
    #[error(
        "the {noun} `{id}` that flavor `{flavor}` resolves (its `app-id-suffix` appended) is \
         invalid: {reason}"
    )]
    InvalidFlavorIdentifier {
        noun: &'static str,
        flavor: String,
        id: String,
        reason: String,
    },
}

/// Resolves the identity for `platform` and optional `flavor`. Only the
/// selected flavor's `[flavors]` entry is validated.
pub fn resolve(
    manifest: &Manifest,
    platform: IdentityPlatform,
    flavor: Option<&str>,
) -> Result<AppIdentity, IdentityError> {
    let explicit_id = match platform {
        IdentityPlatform::Android => manifest
            .android
            .as_ref()
            .and_then(|a| a.identifier.as_ref()),
        IdentityPlatform::Ios => manifest.ios.as_ref().and_then(|i| i.identifier.as_ref()),
    };

    let (mut identifier, mut identifier_stated) = match explicit_id {
        Some(id) => {
            platform.validate(id).map_err(|reason| {
                let id = id.clone();
                match platform {
                    IdentityPlatform::Android => {
                        IdentityError::InvalidExplicitAndroid { id, reason }
                    }
                    IdentityPlatform::Ios => IdentityError::InvalidExplicitIos { id, reason },
                }
            })?;
            (id.clone(), true)
        }
        None => {
            let derived = match platform {
                IdentityPlatform::Android => {
                    crate::android_id::derive(&manifest.app.org, &manifest.app.name)
                }
                IdentityPlatform::Ios => {
                    crate::ios_id::derive(&manifest.app.org, &manifest.app.name)
                }
            };
            platform.validate(&derived).map_err(|reason| {
                let id = derived.clone();
                match platform {
                    IdentityPlatform::Android => {
                        IdentityError::InvalidDerivedAndroid { id, reason }
                    }
                    IdentityPlatform::Ios => IdentityError::InvalidDerivedIos { id, reason },
                }
            })?;
            (derived, false)
        }
    };

    let mut display_name = manifest.app.display_name.clone();
    let declared = match flavor {
        Some(name) => manifest.flavor(name)?,
        None => None,
    };

    if let (Some(name), Some(declared)) = (flavor, &declared) {
        if let Some(suffix) = &declared.app_id_suffix {
            identifier = with_suffix(platform, name, &identifier, suffix)?;
            identifier_stated = true;
        }
        if let Some(suffix) = &declared.app_name_suffix {
            display_name
                .get_or_insert_with(|| default_display_name(manifest))
                .push_str(suffix);
        }
    }

    Ok(AppIdentity {
        identifier,
        identifier_stated,
        display_name,
        flavor: flavor.map(str::to_string),
        flavor_declared: declared.is_some(),
    })
}

/// `identifier` + `suffix`, run through `platform`'s validator: the
/// identifier a build of `flavor` would install must pass the same rules as
/// any other, whatever the suffix grammar admits.
fn with_suffix(
    platform: IdentityPlatform,
    flavor: &str,
    identifier: &str,
    suffix: &str,
) -> Result<String, IdentityError> {
    let id = format!("{identifier}{suffix}");
    platform
        .validate(&id)
        .map_err(|reason| IdentityError::InvalidFlavorIdentifier {
            noun: platform.noun(),
            flavor: flavor.to_string(),
            id: id.clone(),
            reason,
        })?;
    Ok(id)
}

/// The name a build shows with no override: `[app] display-name`, else
/// `[app] name` in title case — what `frust create` renders into the
/// platform files and what an `app-name-suffix` alone is appended to.
pub fn default_display_name(manifest: &Manifest) -> String {
    manifest
        .app
        .display_name
        .clone()
        .unwrap_or_else(|| crate::scaffold::title_case(&manifest.app.name))
}

/// The error for a `--flavor` declared both natively (`native` names that
/// declaration) and as `[flavors.<flavor>]`, raised before the build tool
/// runs: frust cannot tell which of the two identities the author meant.
pub fn flavor_declared_twice(flavor: &str, native: &str) -> String {
    format!(
        "flavor `{flavor}` is declared twice: {native}, and as `[flavors.{flavor}]` in frust.toml. \
         Keep one of the two declarations."
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::manifest;

    fn m(extra: &str) -> Manifest {
        manifest::parse(&format!(
            "[app]\nname = \"my_app\"\norg = \"dev.f0x\"\n{extra}"
        ))
        .unwrap()
    }

    #[test]
    fn derivation_when_nothing_is_set() {
        let man = m("");
        let a = resolve(&man, IdentityPlatform::Android, None).unwrap();
        assert_eq!(a.identifier, crate::android_id::derive("dev.f0x", "my_app"));
        assert_eq!(a.identifier, "dev.f0x.my_app");
        assert!(!a.identifier_stated && !a.flavor_declared);
        assert_eq!((a.display_name, a.flavor), (None, None));
        let i = resolve(&man, IdentityPlatform::Ios, None).unwrap();
        assert_eq!(i.identifier, "dev.f0x.myApp");
        assert!(!i.identifier_stated);
        assert_eq!(i.display_name, None);
    }

    #[test]
    fn explicit_identifier_per_platform() {
        let man =
            m("\n[android]\nidentifier = \"com.a.droid\"\n\n[ios]\nidentifier = \"com.a.ios\"\n");
        let a = resolve(&man, IdentityPlatform::Android, None).unwrap();
        assert_eq!(
            (a.identifier.as_str(), a.identifier_stated),
            ("com.a.droid", true)
        );
        let i = resolve(&man, IdentityPlatform::Ios, None).unwrap();
        assert_eq!(
            (i.identifier.as_str(), i.identifier_stated),
            ("com.a.ios", true)
        );
        // A stated identifier states no name.
        assert_eq!((a.display_name, i.display_name), (None, None));
    }

    /// The name frust applies: absent without `display-name` or a selected
    /// name suffix; `display-name` alone; `display-name` + suffix; a suffix
    /// alone on the title-cased project name.
    #[test]
    fn the_display_name_is_set_only_when_frust_toml_states_one() {
        let flavors = "\n[flavors.dev]\napp-name-suffix = \" Dev\"\n\n[flavors.qa]\n";
        let plain = m(flavors);
        let named = manifest::parse(&format!(
            "[app]\nname = \"my_app\"\norg = \"dev.f0x\"\ndisplay-name = \"Shop\"\n{flavors}"
        ))
        .unwrap();
        for platform in [IdentityPlatform::Android, IdentityPlatform::Ios] {
            let name =
                |man: &Manifest, flavor| resolve(man, platform, flavor).unwrap().display_name;
            assert_eq!(name(&plain, None), None);
            assert_eq!(name(&plain, Some("qa")), None);
            assert_eq!(name(&plain, Some("native")), None);
            assert_eq!(name(&plain, Some("dev")).as_deref(), Some("My App Dev"));
            assert_eq!(name(&named, None).as_deref(), Some("Shop"));
            assert_eq!(name(&named, Some("qa")).as_deref(), Some("Shop"));
            assert_eq!(name(&named, Some("dev")).as_deref(), Some("Shop Dev"));
        }
        assert_eq!(default_display_name(&plain), "My App");
        assert_eq!(default_display_name(&named), "Shop");
    }

    #[test]
    fn the_default_display_name_is_the_scaffold_title_case() {
        let man = m("");
        assert_eq!(default_display_name(&man), "My App");
        // Pinned against the scaffold's rendered `title_case_name`.
        let vars = crate::scaffold::DesignSystemContext {
            name: "my_app".into(),
            frust_version: "0.1.0".into(),
            frust: crate::scaffold::FrustDependency::Path("/p".into()),
        }
        .render_vars();
        assert_eq!(default_display_name(&man), vars["title_case_name"]);
        let p = manifest::parse("[app]\nname = \"playground\"\norg = \"dev.f0x\"\n").unwrap();
        assert_eq!(default_display_name(&p), "Playground");
    }

    #[test]
    fn flavor_with_both_one_or_no_suffix() {
        let man = m(
            "\n[flavors.dev]\napp-id-suffix = \".dev\"\napp-name-suffix = \" Dev\"\n\n\
             [flavors.idonly]\napp-id-suffix = \".qa\"\n\n\
             [flavors.nameonly]\napp-name-suffix = \" B\"\n\n[flavors.bare]\n",
        );
        let d = resolve(&man, IdentityPlatform::Android, Some("dev")).unwrap();
        assert_eq!(d.identifier, "dev.f0x.my_app.dev");
        assert_eq!(d.display_name.as_deref(), Some("My App Dev"));
        assert!(d.identifier_stated && d.flavor_declared);
        assert_eq!(d.flavor.as_deref(), Some("dev"));
        let i = resolve(&man, IdentityPlatform::Ios, Some("dev")).unwrap();
        assert_eq!(i.identifier, "dev.f0x.myApp.dev");

        let o = resolve(&man, IdentityPlatform::Android, Some("idonly")).unwrap();
        assert_eq!(
            (o.identifier.as_str(), o.display_name.as_deref()),
            ("dev.f0x.my_app.qa", None)
        );
        assert!(o.identifier_stated);
        let n = resolve(&man, IdentityPlatform::Android, Some("nameonly")).unwrap();
        assert_eq!(
            (n.identifier.as_str(), n.display_name.as_deref()),
            ("dev.f0x.my_app", Some("My App B"))
        );
        assert!(!n.identifier_stated && n.flavor_declared);
        let b = resolve(&man, IdentityPlatform::Android, Some("bare")).unwrap();
        assert_eq!(
            (b.identifier.as_str(), b.display_name.as_deref()),
            ("dev.f0x.my_app", None)
        );
        assert!(!b.identifier_stated && b.flavor_declared);
    }

    #[test]
    fn undeclared_flavor_keeps_the_base_identity() {
        let man = m("\n[ios]\nidentifier = \"com.a.ios\"\n");
        let i = resolve(&man, IdentityPlatform::Ios, Some("native")).unwrap();
        assert_eq!(i.identifier, "com.a.ios");
        assert!(i.identifier_stated);
        assert_eq!(i.flavor.as_deref(), Some("native"));
        assert!(!i.flavor_declared);
        let a = resolve(&man, IdentityPlatform::Android, Some("native")).unwrap();
        assert!(!a.identifier_stated && !a.flavor_declared);
    }

    /// Only the selected flavor is judged, with the manifest's own message;
    /// the composed identifier is validated and names the flavor.
    #[test]
    fn flavor_errors_surface_only_for_the_selected_flavor() {
        let man = m("\n[flavors.Dev]\n\n[flavors.ok]\napp-id-suffix = \".ok\"\n");
        resolve(&man, IdentityPlatform::Android, None).unwrap();
        resolve(&man, IdentityPlatform::Android, Some("ok")).unwrap();
        let err = resolve(&man, IdentityPlatform::Android, Some("Dev")).unwrap_err();
        assert!(matches!(err, IdentityError::InvalidFlavor(_)), "{err:?}");
        assert!(
            err.to_string().contains("invalid flavor name `Dev`"),
            "{err}"
        );

        // The composed identifier goes through the platform validator: a
        // suffix the platform grammar rejects fails naming the flavor.
        assert_eq!(
            with_suffix(IdentityPlatform::Ios, "dev", "com.a", ".dev").unwrap(),
            "com.a.dev"
        );
        for (platform, suffix, noun) in [
            (IdentityPlatform::Ios, ".a_b", "iOS bundle identifier"),
            (IdentityPlatform::Android, ".1a", "Android application id"),
        ] {
            let err = with_suffix(platform, "big", "com.a", suffix).unwrap_err();
            assert!(
                matches!(err, IdentityError::InvalidFlavorIdentifier { .. }),
                "{err:?}"
            );
            let message = err.to_string();
            assert!(message.contains("flavor `big`"), "{message}");
            assert!(message.contains(noun), "{message}");
            assert!(message.contains(&format!("`com.a{suffix}`")), "{message}");
        }
    }

    #[test]
    fn a_double_declared_flavor_error_names_both_declarations() {
        let message = flavor_declared_twice("dev", "the Xcode scheme `Dev`");
        assert!(message.contains("flavor `dev`"), "{message}");
        assert!(message.contains("the Xcode scheme `Dev`"), "{message}");
        assert!(
            message.contains("`[flavors.dev]` in frust.toml"),
            "{message}"
        );
    }

    #[test]
    fn invalid_identifiers_are_typed_errors() {
        let man = m("\n[android]\nidentifier = \"bad\"\n\n[ios]\nidentifier = \"a.b_c\"\n");
        assert!(matches!(
            resolve(&man, IdentityPlatform::Android, None),
            Err(IdentityError::InvalidExplicitAndroid { .. })
        ));
        assert!(matches!(
            resolve(&man, IdentityPlatform::Ios, None),
            Err(IdentityError::InvalidExplicitIos { .. })
        ));
        let bad_org = manifest::parse("[app]\nname = \"x\"\norg = \"1a.b\"\n").unwrap();
        assert!(matches!(
            resolve(&bad_org, IdentityPlatform::Android, None),
            Err(IdentityError::InvalidDerivedAndroid { .. })
        ));
        let bad_org = manifest::parse("[app]\nname = \"x\"\norg = \"a b\"\n").unwrap();
        assert!(matches!(
            resolve(&bad_org, IdentityPlatform::Ios, None),
            Err(IdentityError::InvalidDerivedIos { .. })
        ));
    }
}
