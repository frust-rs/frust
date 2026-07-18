//! Profile feature domain (wave-2 task 06) — an editable profile with a
//! save/validate use case whose failure path surfaces like the roster's.
//!
//! [`ProfileController`] embeds a [`ControllerCore`] by composition (the
//! `templates/AGENTS.md` controller rule, same as
//! [`TeamController`](crate::features::team::presentation)) and drives the
//! [`SaveProfile`] use case. Validation lives in the use case (blank name /
//! malformed email → [`TeamFailure::Validation`]), never in the controller or
//! the view — exactly the layering
//! [`UpdateMember`](crate::features::team::domain::use_cases::UpdateMember)
//! establishes. A rejected save routes its `TeamFailure` to the controller's
//! failure sink, which the screen surfaces as a dismissible banner (the same
//! `use_failure_listener`-into-a-signal pattern `TeamScreen` uses) — the visible
//! failure path the plan promises.
//!
//! The avatar reuses the example's existing `assets/logo.png` (decoded once,
//! the notes decode-once contract) rather than shipping a second PNG — no new
//! asset file is added.

use clean_signals::{ControllerCore, FailureSink, RunOptions, UseCase};
use forgekit::{ImageSource, RwSignal, Update};

use crate::failure::TeamFailure;

/// The embedded avatar bytes — reuses the example's existing logo asset (no new
/// file). Decoded exactly once, in [`ProfileController::new`].
const AVATAR_PNG: &[u8] = include_bytes!("../assets/logo.png");

/// The editable text fields of a profile — the validated payload a save
/// produces. Separate from [`Profile`] (which also carries the non-editable
/// avatar) so the use case's output stays a plain, comparable value.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProfileFields {
    pub name: String,
    pub role: String,
    pub email: String,
}

/// A person's profile: the editable [`ProfileFields`] plus the decode-once
/// avatar handle re-passed into `Image` every frame.
#[derive(Clone)]
pub struct Profile {
    pub name: String,
    pub role: String,
    pub email: String,
    /// The avatar (an `Arc`-backed decoded handle; cloning is a cheap pointer
    /// bump, never a re-decode).
    pub avatar: ImageSource,
}

/// Parameters for [`SaveProfile`]: the three edit-buffer strings.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SaveProfileParams {
    pub name: String,
    pub role: String,
    pub email: String,
}

/// Whether `email` is a plausible address: a single `@` with a non-empty local
/// part and a dotted, non-empty domain. Deliberately lightweight — a demo
/// validator, not an RFC 5322 parser; its only job is to make the failure path
/// reachable with obviously-bad input.
fn is_plausible_email(email: &str) -> bool {
    let mut parts = email.split('@');
    let (Some(local), Some(domain), None) = (parts.next(), parts.next(), parts.next()) else {
        return false;
    };
    !local.is_empty() && domain.contains('.') && !domain.starts_with('.') && !domain.ends_with('.')
}

/// Saves a profile, validating the name and email before it would ever reach a
/// backend (there is none in this demo — the validated fields are the output).
/// Rejects a blank name or a malformed email with [`TeamFailure::Validation`].
pub struct SaveProfile;

#[cfg_attr(not(target_arch = "wasm32"), clean_signals::async_trait)]
#[cfg_attr(target_arch = "wasm32", clean_signals::async_trait(?Send))]
impl UseCase for SaveProfile {
    type Params = SaveProfileParams;
    type Output = ProfileFields;
    type Failure = TeamFailure;

    async fn execute(&self, params: SaveProfileParams) -> Result<ProfileFields, TeamFailure> {
        let name = params.name.trim();
        if name.is_empty() {
            return Err(TeamFailure::Validation("Name cannot be empty.".to_string()));
        }
        let email = params.email.trim();
        if !is_plausible_email(email) {
            return Err(TeamFailure::Validation(
                "Enter a valid email address.".to_string(),
            ));
        }
        Ok(ProfileFields {
            name: name.to_string(),
            role: params.role.trim().to_string(),
            email: email.to_string(),
        })
    }
}

/// View model for the profile screen.
pub struct ProfileController {
    core: ControllerCore<TeamFailure>,
    save_profile: SaveProfile,
    /// The last successfully-saved profile — the screen seeds its edit buffers
    /// from it and renders its committed values. A plain `RwSignal<Profile>`
    /// (not an `AsyncState`): a save is fast and local, with no loading state.
    pub profile: RwSignal<Profile>,
}

impl ProfileController {
    /// Build the controller, decoding the avatar exactly once and seeding a
    /// starter profile.
    pub fn new() -> Self {
        let avatar = ImageSource::decode(AVATAR_PNG).expect("the embedded avatar PNG is valid");
        let profile = RwSignal::new(Profile {
            name: "Ada Lovelace".to_string(),
            role: "Staff Engineer".to_string(),
            email: "ada@forgekit.dev".to_string(),
            avatar,
        });
        Self {
            core: ControllerCore::new(),
            save_profile: SaveProfile,
            profile,
        }
    }

    /// Validate + save the edit buffers. On success, patches the committed
    /// [`Self::profile`] in place (preserving the avatar); on a validation
    /// failure, `ControllerCore::run` routes the `TeamFailure` to the failure
    /// sink → the screen's banner (the caller only reacts to success).
    pub async fn save(&self, name: String, role: String, email: String) {
        let result = self
            .core
            .run(
                &self.save_profile,
                SaveProfileParams { name, role, email },
                RunOptions::default(),
            )
            .await;

        if let Ok(fields) = result {
            self.profile.try_update(|p| {
                p.name = fields.name;
                p.role = fields.role;
                p.email = fields.email;
            });
        }
    }

    /// The controller's failure sink — the screen subscribes once to surface
    /// validation failures as a banner.
    pub fn failures(&self) -> &FailureSink<TeamFailure> {
        self.core.failures()
    }
}

impl Default for ProfileController {
    fn default() -> Self {
        Self::new()
    }
}

impl AsRef<ControllerCore<TeamFailure>> for ProfileController {
    fn as_ref(&self) -> &ControllerCore<TeamFailure> {
        &self.core
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use forgekit::GetUntracked;

    #[test]
    fn plausible_email_accepts_and_rejects() {
        assert!(is_plausible_email("ada@forgekit.dev"));
        assert!(is_plausible_email("a.b@team.co.uk"));
        assert!(!is_plausible_email("no-at-sign"));
        assert!(!is_plausible_email("@forgekit.dev"));
        assert!(!is_plausible_email("ada@nodot"));
        assert!(!is_plausible_email("ada@@forgekit.dev"));
        assert!(!is_plausible_email("ada@.dev"));
    }

    #[tokio::test]
    async fn happy_path_trims_all_fields() {
        let saved = SaveProfile
            .execute(SaveProfileParams {
                name: "  Grace Hopper  ".to_string(),
                role: "  Rear Admiral  ".to_string(),
                email: "  grace@navy.mil  ".to_string(),
            })
            .await
            .unwrap();
        assert_eq!(
            saved,
            ProfileFields {
                name: "Grace Hopper".to_string(),
                role: "Rear Admiral".to_string(),
                email: "grace@navy.mil".to_string(),
            }
        );
    }

    #[tokio::test]
    async fn blank_name_is_a_validation_failure() {
        let result = SaveProfile
            .execute(SaveProfileParams {
                name: "   ".to_string(),
                role: "Engineer".to_string(),
                email: "e@team.dev".to_string(),
            })
            .await;
        assert_eq!(
            result,
            Err(TeamFailure::Validation("Name cannot be empty.".to_string()))
        );
    }

    #[tokio::test]
    async fn malformed_email_is_a_validation_failure() {
        let result = SaveProfile
            .execute(SaveProfileParams {
                name: "Ada".to_string(),
                role: "Engineer".to_string(),
                email: "not-an-email".to_string(),
            })
            .await;
        assert_eq!(
            result,
            Err(TeamFailure::Validation(
                "Enter a valid email address.".to_string()
            ))
        );
    }

    #[tokio::test]
    async fn save_patches_the_committed_profile_and_keeps_the_avatar() {
        let controller = ProfileController::new();
        let original_avatar = controller.profile.get_untracked().avatar;

        controller
            .save(
                "Katherine Johnson".to_string(),
                "Mathematician".to_string(),
                "kj@nasa.gov".to_string(),
            )
            .await;

        let updated = controller.profile.get_untracked();
        assert_eq!(updated.name, "Katherine Johnson");
        assert_eq!(updated.role, "Mathematician");
        assert_eq!(updated.email, "kj@nasa.gov");
        assert!(
            updated.avatar.same(&original_avatar),
            "the avatar handle is preserved across a save (no re-decode)"
        );

        controller.core.dispose();
    }

    #[tokio::test]
    async fn rejected_save_leaves_the_committed_profile_untouched() {
        let controller = ProfileController::new();
        let before = controller.profile.get_untracked();

        controller
            .save(
                String::new(),
                "Engineer".to_string(),
                "bad-email".to_string(),
            )
            .await;

        let after = controller.profile.get_untracked();
        assert_eq!(after.name, before.name);
        assert_eq!(after.email, before.email);

        controller.core.dispose();
    }
}
