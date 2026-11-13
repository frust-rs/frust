//! Resolves the app identity a build must produce: the Android application
//! id or iOS bundle identifier plus the display name, for a project, a
//! platform and an optional flavor declared in `[flavors]`.
//!
//! Pure: no I/O. Explicit `[android]`/`[ios]` identifiers and derived ones
//! go through the same validators `android_run::project` and
//! `ios_run::project` use.

use thiserror::Error;

use crate::manifest::Manifest;

/// The platform an identity is resolved for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IdentityPlatform {
    Android,
    Ios,
}

/// What a build must be identified as.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AppIdentity {
    /// Android application id or iOS bundle identifier.
    pub identifier: String,
    /// The launcher / home-screen display name.
    pub display_name: String,
    /// Whether the identifier (or name) is pinned by `frust.toml` rather
    /// than purely derived: an explicit platform identifier, or a declared
    /// flavor.
    pub explicit: bool,
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
}

/// Resolves the identity for `platform` and optional `flavor`.
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
    let validate = |id: &str| -> Result<(), String> {
        match platform {
            IdentityPlatform::Android => crate::android_id::validate(id).map_err(|e| e.to_string()),
            IdentityPlatform::Ios => crate::ios_id::validate(id).map_err(|e| e.to_string()),
        }
    };

    let (mut identifier, mut explicit) = match explicit_id {
        Some(id) => {
            validate(id).map_err(|reason| match platform {
                IdentityPlatform::Android => IdentityError::InvalidExplicitAndroid {
                    id: id.clone(),
                    reason,
                },
                IdentityPlatform::Ios => IdentityError::InvalidExplicitIos {
                    id: id.clone(),
                    reason,
                },
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
            validate(&derived).map_err(|reason| match platform {
                IdentityPlatform::Android => IdentityError::InvalidDerivedAndroid {
                    id: derived.clone(),
                    reason,
                },
                IdentityPlatform::Ios => IdentityError::InvalidDerivedIos {
                    id: derived.clone(),
                    reason,
                },
            })?;
            (derived, false)
        }
    };

    let mut display_name = crate::scaffold::title_case(&manifest.app.name);
    let mut flavor_declared = false;

    if let Some(declared) = flavor.and_then(|f| manifest.flavors.get(f)) {
        if let Some(suffix) = &declared.app_id_suffix {
            identifier.push_str(suffix);
        }
        if let Some(suffix) = &declared.app_name_suffix {
            display_name.push_str(suffix);
        }
        explicit = true;
        flavor_declared = true;
    }

    Ok(AppIdentity {
        identifier,
        display_name,
        explicit,
        flavor: flavor.map(str::to_string),
        flavor_declared,
    })
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
        assert!(!a.explicit && !a.flavor_declared);
        assert_eq!(a.flavor, None);
        let i = resolve(&man, IdentityPlatform::Ios, None).unwrap();
        assert_eq!(i.identifier, "dev.f0x.myApp");
        assert!(!i.explicit);
    }

    #[test]
    fn explicit_identifier_per_platform() {
        let man =
            m("\n[android]\nidentifier = \"com.a.droid\"\n\n[ios]\nidentifier = \"com.a.ios\"\n");
        let a = resolve(&man, IdentityPlatform::Android, None).unwrap();
        assert_eq!((a.identifier.as_str(), a.explicit), ("com.a.droid", true));
        let i = resolve(&man, IdentityPlatform::Ios, None).unwrap();
        assert_eq!((i.identifier.as_str(), i.explicit), ("com.a.ios", true));
    }

    #[test]
    fn display_name_is_the_scaffold_title_case() {
        let man = m("");
        let a = resolve(&man, IdentityPlatform::Android, None).unwrap();
        assert_eq!(a.display_name, "My App");
        // Pinned against the scaffold's rendered `title_case_name`.
        let vars = crate::scaffold::DesignSystemContext {
            name: "my_app".into(),
            frust_version: "0.1.0".into(),
            frust: crate::scaffold::FrustDependency::Path("/p".into()),
        }
        .render_vars();
        assert_eq!(a.display_name, vars["title_case_name"]);
        let p = manifest::parse("[app]\nname = \"playground\"\norg = \"dev.f0x\"\n").unwrap();
        let a = resolve(&p, IdentityPlatform::Ios, None).unwrap();
        assert_eq!(a.display_name, "Playground");
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
        assert_eq!(d.display_name, "My App Dev");
        assert!(d.explicit && d.flavor_declared);
        assert_eq!(d.flavor.as_deref(), Some("dev"));
        let i = resolve(&man, IdentityPlatform::Ios, Some("dev")).unwrap();
        assert_eq!(i.identifier, "dev.f0x.myApp.dev");

        let o = resolve(&man, IdentityPlatform::Android, Some("idonly")).unwrap();
        assert_eq!(
            (o.identifier.as_str(), o.display_name.as_str()),
            ("dev.f0x.my_app.qa", "My App")
        );
        let n = resolve(&man, IdentityPlatform::Android, Some("nameonly")).unwrap();
        assert_eq!(
            (n.identifier.as_str(), n.display_name.as_str()),
            ("dev.f0x.my_app", "My App B")
        );
        assert!(n.explicit && n.flavor_declared);
        let b = resolve(&man, IdentityPlatform::Android, Some("bare")).unwrap();
        assert_eq!(
            (b.identifier.as_str(), b.display_name.as_str()),
            ("dev.f0x.my_app", "My App")
        );
        assert!(b.explicit && b.flavor_declared);
    }

    #[test]
    fn undeclared_flavor_keeps_the_base_identity() {
        let man = m("\n[ios]\nidentifier = \"com.a.ios\"\n");
        let i = resolve(&man, IdentityPlatform::Ios, Some("native")).unwrap();
        assert_eq!(i.identifier, "com.a.ios");
        assert!(i.explicit);
        assert_eq!(i.flavor.as_deref(), Some("native"));
        assert!(!i.flavor_declared);
        let a = resolve(&man, IdentityPlatform::Android, Some("native")).unwrap();
        assert!(!a.explicit && !a.flavor_declared);
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
