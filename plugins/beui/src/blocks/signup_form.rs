//! Ports beUI's `signup-form` composed block — the composed registration form:
//! five fields that flag themselves only once left, a length-weighted strength
//! meter, a password reveal and a submit button that carries the whole
//! submission lifecycle.
//!
//! Source: `components/motion/signup-form.tsx` (beUI v2, rev
//! `10c283e433a8f4f0ac0736684d4426ab612b9f55`, retrieved 2026-09-01), registry
//! slug `signup-form` (blocks): *"Composed sign-up form that flags a field only
//! once it is left, then clears the moment it is fixed, with a length-weighted
//! strength meter, password reveal and an animated submit lifecycle."*
//!
//! | class / prop | here |
//! |---|---|
//! | form `max-w-sm rounded-3xl border p-6 gap-5` | [`FORM_MAX_WIDTH`] / [`style::RADIUS_3XL`] / [`FORM_PADDING`] / [`SECTION_GAP`] |
//! | header `gap-1`, title `text-xl font-semibold`, description `text-sm text-muted-foreground` | [`HEADER_GAP`] / [`TITLE_SIZE`] / [`style::TEXT_SM`] |
//! | fields `flex-col gap-1` | [`FIELD_GAP`] |
//! | strength `flex-col gap-1.5 px-1`, bars `h-1 flex-1 gap-1.5 rounded-full` | [`STRENGTH_GAP`] / [`STRENGTH_PADDING_X`] / [`STRENGTH_BAR_HEIGHT`] |
//! | bar track `bg-muted-foreground/20` | `on_surface_variant` at [`STRENGTH_TRACK_ALPHA`] |
//! | bar fill `scaleX(index < strength ? 1 : 0)`, `SPRING_LAYOUT` | a lane per bar on [`crate::tokens::motion::SPRING_LAYOUT`] |
//! | strength label `text-xs text-muted-foreground` | [`style::TEXT_XS`] |
//! | strength enter/exit `opacity, y: -4`, `duration: 0.18` | [`MESSAGE_RISE`] / [`STRENGTH_FADE_MS`] |
//! | terms `flex-col gap-1.5`, error `px-1 text-xs text-destructive` | [`TERMS_GAP`] / [`MESSAGE_PADDING_X`] |
//! | form error `rounded-2xl border-destructive/30 bg-destructive/10 px-3 py-2 text-xs` | [`style::RADIUS_2XL`] / [`FORM_ERROR_BORDER_ALPHA`] / [`FORM_ERROR_PADDING_X`] / [`FORM_ERROR_PADDING_Y`] |
//! | error enter/exit `opacity, y: -4`, `duration: 0.2` | [`MESSAGE_RISE`] / [`MESSAGE_FADE_MS`] |
//! | `<StatefulButton size="lg" className="w-full">` | the wrapped [`button`] at [`ButtonSize::Lg`], full width |
//! | reveal `<Eye>` / `<EyeOff>` in the field's `rightIcon` | [`EYE_BOX`], painted over the field's trailing padding |
//!
//! # Reward early, punish late
//!
//! Upstream computes every error on every change but *shows* a field's error
//! only once that field has been **blurred** (or once a submit has touched them
//! all), and clears it the moment the field becomes valid again. That is the
//! behaviour worth porting, and it needs a blur this widget can observe.
//!
//! It has one: each wrapped field is a [`ChildPod`], and a pod records whether
//! it holds the focus path ([`ChildPod::is_focused`], maintained by the pod's own
//! event routing). This widget latches that flag per field and treats a
//! `true → false` transition as the blur — so leaving a field marks it touched,
//! exactly as upstream's `onBlur` does. A submit touches all five outright.
//!
//! **The touched set is the widget's, not the app's**, because the app cannot
//! see a blur either. It therefore reaches the child field views through
//! `View::rebuild`'s `element`, which is where a rebuilt child's props are
//! assembled — a touch observed in one event pass shows up on the next rebuild,
//! which for both shipped shells is the very next frame.
//!
//! # Validation rules are the caller's
//!
//! [`default_validate`] is upstream's own rule set, ported verbatim (including
//! the deliberately permissive e-mail shape and the eight-character floor), and
//! it is what a bare form uses. [`SignupFormView::validate`] replaces it with a
//! caller-supplied predicate over [`SignupValues`], upstream's `validate` prop.
//! [`password_strength`] is likewise upstream's own NIST-flavoured heuristic,
//! and is public so a caller scoring passwords elsewhere agrees with the meter.
//!
//! # Premise correction: the fields do not cascade in
//!
//! `signup-form.tsx` mounts its fields plainly — there is no `AnimatePresence`,
//! no stagger and no per-field entrance on the field column; the animated parts
//! are the strength meter's presence, the two error messages' presence, the
//! per-field chrome that [`input`](crate::components::input) already carries,
//! and the submit button's state morph. This port matches the source rather
//! than adding an entrance the source does not have.
//!
//! # Degradations
//!
//! - **No leading icons.** Upstream gives each field a lucide `leftIcon`
//!   (`User`, `Mail`, `Lock`); [`input`](crate::components::input) ships no icon
//!   slots, as its own docs record, so the fields carry their labels alone.
//! - **The reveal toggle is painted by this block, not by the field.** With no
//!   `rightIcon` slot the eye sits over the password field's trailing padding
//!   and is hit-tested here, ahead of the pods. It reveals **both** password
//!   fields at once, as upstream's shared `revealPassword` does.
//! - **The strength meter's two middle colours are the catalog's `--warning`.**
//!   Upstream reaches straight for Tailwind's `amber-500`/`amber-400`, which are
//!   not beUI tokens; the port folds both onto the one authored warning hue
//!   rather than vendoring two raw hexes.
//! - **No submission task.** `onSubmit` may be `async` upstream, which is what
//!   drives its internal status. Here [`SignupFormView::status`] is a prop and
//!   [`SignupFormView::on_submit`] reports valid values; the app runs the task.

use std::rc::Rc;
use std::time::Duration;

use frust::authoring::text::{FontWeight, TextStyle};
use frust::authoring::{
    Action, AnyView, BoxConstraints, Brush, BuildCtx, ChangeFlags, ChildPod, Color, EventCtx,
    EventResult, InputEvent, LayoutCtx, PaintCtx, PaintScene, Point, PointerPhase, Rect, Role,
    RoundedRect, SemanticsCtx, Shape, Size, View, Widget, any, build_child, rebuild_child,
    route_event_single, teardown_child, visit_children,
};
use frust::{FrameTime, Theme};

use crate::components::button::{ButtonSize, ButtonState, ButtonVariant, button};
use crate::components::checkbox::checkbox;
use crate::components::input::{self as beui_input, MESSAGE_RISE, input};
use crate::motion::{Presence, Ramp};
use crate::press::{Lane, presses};
use crate::style;
use crate::text::Label;
use crate::tokens::motion::{EASE_OUT, SPRING_LAYOUT};
use crate::tokens::{BeuiTokens, sans_family};

// ---- Ported metrics --------------------------------------------------------

/// The form's width ceiling, in logical px (`max-w-sm`).
pub const FORM_MAX_WIDTH: f64 = 384.0;
/// The form's inner padding, in logical px (`p-6`).
pub const FORM_PADDING: f64 = 24.0;
/// Gap between the form's sections, in logical px (`gap-5`).
pub const SECTION_GAP: f64 = 20.0;
/// Gap inside the header, in logical px (`gap-1`).
pub const HEADER_GAP: f64 = 4.0;
/// The title's type size, in logical px (`text-xl`).
pub const TITLE_SIZE: f64 = 20.0;
/// Gap between two fields, in logical px (`gap-1`).
pub const FIELD_GAP: f64 = 4.0;
/// Gap inside the terms group, in logical px (`gap-1.5`).
pub const TERMS_GAP: f64 = 6.0;
/// Gap between the terms checkbox and its label, in logical px (the row's own
/// `gap-2` — `checkbox` paints no text of its own).
pub const TERMS_LABEL_GAP: f64 = 8.0;
/// Horizontal padding on a message row, in logical px (`px-1`).
pub const MESSAGE_PADDING_X: f64 = 4.0;
/// How long a message's entrance and exit take, in milliseconds
/// (`duration: 0.2`).
pub const MESSAGE_FADE_MS: u64 = 200;

/// Gap inside the strength group, in logical px (`gap-1.5`).
pub const STRENGTH_GAP: f64 = 6.0;
/// The strength group's horizontal padding, in logical px (`px-1`).
pub const STRENGTH_PADDING_X: f64 = 4.0;
/// A strength bar's height, in logical px (`h-1`).
pub const STRENGTH_BAR_HEIGHT: f64 = 4.0;
/// Gap between two strength bars, in logical px (`gap-1.5`).
pub const STRENGTH_BAR_GAP: f64 = 6.0;
/// How many strength bars the meter shows (`[0, 1, 2, 3]`).
pub const STRENGTH_BARS: usize = 4;
/// Alpha the unfilled bar track is painted at (`bg-muted-foreground/20`).
pub const STRENGTH_TRACK_ALPHA: f32 = 0.2;
/// How long the strength group's entrance and exit take, in milliseconds
/// (`duration: 0.18`).
pub const STRENGTH_FADE_MS: u64 = 180;

/// The form-level error's horizontal padding, in logical px (`px-3`).
pub const FORM_ERROR_PADDING_X: f64 = 12.0;
/// The form-level error's vertical padding, in logical px (`py-2`).
pub const FORM_ERROR_PADDING_Y: f64 = 8.0;
/// Alpha of the form-level error's hairline (`border-destructive/30`).
pub const FORM_ERROR_BORDER_ALPHA: f32 = 0.3;

/// The reveal toggle's hit box, in logical px — the field's own trailing
/// padding, squared off around the glyph.
pub const EYE_BOX: f64 = 24.0;
/// The reveal glyph's edge, in logical px (`[&_svg]:h-4 [&_svg]:w-4`).
pub const EYE_ICON: f64 = style::ICON_SIZE;

/// The shortest password [`default_validate`] accepts, and the floor
/// [`password_strength`] scores zero below (`MIN_PASSWORD_LENGTH`).
pub const MIN_PASSWORD_LENGTH: usize = 8;

/// The strength ladder's labels (`STRENGTH_LABELS`).
pub const STRENGTH_LABELS: [&str; 5] = ["Too short", "Weak", "Fair", "Good", "Strong"];

/// The ramp a message's presence runs on.
const MESSAGE_RAMP: Ramp = Ramp::eased(Duration::from_millis(MESSAGE_FADE_MS), EASE_OUT);
/// The ramp the strength group's presence runs on.
const STRENGTH_RAMP: Ramp = Ramp::eased(Duration::from_millis(STRENGTH_FADE_MS), EASE_OUT);

/// Unthemed fallback hairline — the light table's `--border`.
const FALLBACK_BORDER: Color = crate::BEUI_LIGHT.border;
/// Unthemed fallback ink — the light table's `--foreground`.
const FALLBACK_FOREGROUND: Color = crate::BEUI_LIGHT.foreground;
/// Unthemed fallback dim ink — the light table's `--muted-foreground`.
const FALLBACK_MUTED: Color = crate::BEUI_LIGHT.muted_foreground;
/// Unthemed fallback error hue — the light table's `--destructive`.
const FALLBACK_DESTRUCTIVE: Color = crate::BEUI_LIGHT.destructive;

// ---- The value model -------------------------------------------------------

/// The form's five values — upstream's `SignUpValues`.
#[derive(Clone, Default, PartialEq, Eq)]
pub struct SignupValues {
    /// The `name` field.
    pub name: String,
    /// The `email` field.
    pub email: String,
    /// The `password` field.
    pub password: String,
    /// The `confirmPassword` field.
    pub confirm_password: String,
    /// The `terms` checkbox.
    pub terms: bool,
}

impl std::fmt::Debug for SignupValues {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SignupValues")
            .field("name", &self.name)
            .field("email", &self.email)
            .field("password", &"<redacted>")
            .field("confirm_password", &"<redacted>")
            .field("terms", &self.terms)
            .finish()
    }
}

/// Which of the five inputs a value, an error or a touch belongs to.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SignupField {
    /// `name`.
    Name,
    /// `email`.
    Email,
    /// `password`.
    Password,
    /// `confirmPassword`.
    ConfirmPassword,
    /// `terms`.
    Terms,
}

impl SignupField {
    /// The five fields, in the order the form lays them out.
    pub const ALL: [SignupField; 5] = [
        SignupField::Name,
        SignupField::Email,
        SignupField::Password,
        SignupField::ConfirmPassword,
        SignupField::Terms,
    ];

    /// The four *text* fields — the ones backed by a wrapped
    /// [`input`](crate::components::input) pod.
    pub const TEXT: [SignupField; 4] = [
        SignupField::Name,
        SignupField::Email,
        SignupField::Password,
        SignupField::ConfirmPassword,
    ];

    /// This field's index into the touched/pod arrays.
    const fn index(self) -> usize {
        match self {
            SignupField::Name => 0,
            SignupField::Email => 1,
            SignupField::Password => 2,
            SignupField::ConfirmPassword => 3,
            SignupField::Terms => 4,
        }
    }

    /// The field's label (upstream's `label` prop per `<Input>`).
    pub const fn label(self) -> &'static str {
        match self {
            SignupField::Name => "Name",
            SignupField::Email => "Email",
            SignupField::Password => "Password",
            SignupField::ConfirmPassword => "Confirm password",
            SignupField::Terms => "Terms",
        }
    }

    /// The field's placeholder.
    pub const fn placeholder(self) -> &'static str {
        match self {
            SignupField::Name => "Ada Lovelace",
            SignupField::Email => "you@example.com",
            SignupField::Password => "At least 8 characters",
            SignupField::ConfirmPassword => "Re-enter your password",
            SignupField::Terms => "",
        }
    }

    /// Whether this field is masked unless the reveal toggle is on
    /// (`type={revealPassword ? "text" : "password"}`).
    pub const fn is_secret(self) -> bool {
        matches!(self, SignupField::Password | SignupField::ConfirmPassword)
    }

    /// Whether `values` holds anything in this field — upstream's
    /// `Boolean(values[key])`, which for `terms` is the flag itself.
    pub fn is_filled(self, values: &SignupValues) -> bool {
        match self {
            SignupField::Name => !values.name.is_empty(),
            SignupField::Email => !values.email.is_empty(),
            SignupField::Password => !values.password.is_empty(),
            SignupField::ConfirmPassword => !values.confirm_password.is_empty(),
            SignupField::Terms => values.terms,
        }
    }

    /// This field's value in `values`, for the four text fields.
    pub fn text(self, values: &SignupValues) -> &str {
        match self {
            SignupField::Name => &values.name,
            SignupField::Email => &values.email,
            SignupField::Password => &values.password,
            SignupField::ConfirmPassword => &values.confirm_password,
            SignupField::Terms => "",
        }
    }

    /// Write `text` into this field of `values`.
    pub fn set_text(self, values: &mut SignupValues, text: String) {
        match self {
            SignupField::Name => values.name = text,
            SignupField::Email => values.email = text,
            SignupField::Password => values.password = text,
            SignupField::ConfirmPassword => values.confirm_password = text,
            SignupField::Terms => {}
        }
    }
}

/// One message per invalid field — upstream's `SignUpErrors`.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SignupErrors {
    /// Why `name` is invalid.
    pub name: Option<String>,
    /// Why `email` is invalid.
    pub email: Option<String>,
    /// Why `password` is invalid.
    pub password: Option<String>,
    /// Why `confirmPassword` is invalid.
    pub confirm_password: Option<String>,
    /// Why `terms` is invalid.
    pub terms: Option<String>,
}

impl SignupErrors {
    /// The message for `field`, if any.
    pub fn get(&self, field: SignupField) -> Option<&str> {
        match field {
            SignupField::Name => self.name.as_deref(),
            SignupField::Email => self.email.as_deref(),
            SignupField::Password => self.password.as_deref(),
            SignupField::ConfirmPassword => self.confirm_password.as_deref(),
            SignupField::Terms => self.terms.as_deref(),
        }
    }

    /// Whether every field passed — upstream's
    /// `Object.keys(errors).length === 0`.
    pub fn is_empty(&self) -> bool {
        SignupField::ALL
            .into_iter()
            .all(|field| self.get(field).is_none())
    }
}

/// Whether `address` matches upstream's deliberately permissive e-mail shape,
/// `/^[^\s@]+@[^\s@]+\.[^\s@]+$/`.
///
/// Transcribed rather than run through a regex engine, and permissive for
/// upstream's own stated reason: full RFC 5322 matching rejects addresses that
/// deliver fine, and the only real check is sending mail.
pub fn looks_like_email(address: &str) -> bool {
    let forbidden = |c: char| c.is_whitespace() || c == '@';
    let Some((local, rest)) = address.split_once('@') else {
        return false;
    };
    if local.is_empty() || local.chars().any(forbidden) {
        return false;
    }
    // The remainder must itself split on a dot into two non-empty runs that
    // contain no further `@` or whitespace — `[^\s@]+\.[^\s@]+`.
    let Some((domain, tld)) = rest.rsplit_once('.') else {
        return false;
    };
    !domain.is_empty()
        && !tld.is_empty()
        && !domain.chars().any(forbidden)
        && !tld.chars().any(forbidden)
}

/// Score a password `0..=4`, length-weighted — upstream's `passwordStrength`.
///
/// NIST SP 800-63B advises against composition requirements and treats length
/// as the dominant factor, so extra character classes only nudge the score and
/// cannot rescue a short password. A heuristic for feedback, not an entropy
/// estimate; pair it with a breach-list check server-side for anything real.
pub fn password_strength(password: &str) -> u8 {
    if password.chars().count() < MIN_PASSWORD_LENGTH {
        return 0;
    }
    let length = password.chars().count();
    let mut score = 1u8;
    if length >= 12 {
        score += 1;
    }
    if length >= 16 {
        score += 1;
    }
    let classes = [
        password.chars().any(|c| c.is_ascii_lowercase()),
        password.chars().any(|c| c.is_ascii_uppercase()),
        password.chars().any(|c| c.is_ascii_digit()),
        password.chars().any(|c| !c.is_ascii_alphanumeric()),
    ]
    .into_iter()
    .filter(|hit| *hit)
    .count();
    if classes >= 3 {
        score += 1;
    }
    score.min(4)
}

/// The built-in rule set — upstream's `defaultValidate`, message for message.
pub fn default_validate(values: &SignupValues) -> SignupErrors {
    let mut errors = SignupErrors::default();

    if values.name.trim().is_empty() {
        errors.name = Some("Enter your name.".to_string());
    }

    if values.email.trim().is_empty() {
        errors.email = Some("Enter your email.".to_string());
    } else if !looks_like_email(&values.email) {
        errors.email = Some("That doesn't look like an email address.".to_string());
    }

    if values.password.is_empty() {
        errors.password = Some("Choose a password.".to_string());
    } else if values.password.chars().count() < MIN_PASSWORD_LENGTH {
        errors.password = Some(format!("Use at least {MIN_PASSWORD_LENGTH} characters."));
    }

    if values.confirm_password.is_empty() {
        errors.confirm_password = Some("Confirm your password.".to_string());
    } else if values.confirm_password != values.password {
        errors.confirm_password = Some("Passwords don't match.".to_string());
    }

    if !values.terms {
        errors.terms = Some("Accept the terms to continue.".to_string());
    }

    errors
}

/// Where a submission has got to — upstream's `SignUpStatus`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum SignupStatus {
    /// Nothing in flight.
    #[default]
    Idle,
    /// Submitting: every control is inert and the button spins.
    Loading,
    /// Accepted.
    Success,
    /// Rejected.
    Error,
}

impl SignupStatus {
    /// Whether a submission is in flight (`isSubmitting`).
    pub const fn is_submitting(self) -> bool {
        matches!(self, SignupStatus::Loading)
    }

    /// The button state this status drives.
    const fn button_state(self) -> ButtonState {
        match self {
            SignupStatus::Idle => ButtonState::Idle,
            SignupStatus::Loading => ButtonState::Loading,
            SignupStatus::Success => ButtonState::Success,
            SignupStatus::Error => ButtonState::Error,
        }
    }
}

// ---- View ------------------------------------------------------------------

/// A caller-supplied rule set over the form's values.
type Validate = Rc<dyn Fn(&SignupValues) -> SignupErrors>;
/// A view-held values callback.
type OnValues<State> = Rc<dyn Fn(&mut State, SignupValues)>;

/// A declarative beUI sign-up form. See the [module docs](self).
pub struct SignupFormView<State: 'static> {
    values: SignupValues,
    status: SignupStatus,
    title: String,
    description: String,
    submit_label: String,
    terms_label: String,
    error_message: Option<String>,
    strength_meter: bool,
    validate: Option<Validate>,
    on_values_change: OnValues<State>,
    on_submit: Option<OnValues<State>>,
}

/// Create a sign-up form showing `values` and reporting every edit through
/// `on_values_change` — a **controlled** component (see the [module docs](self)).
pub fn signup_form<State: 'static, F: Fn(&mut State, SignupValues) + 'static>(
    values: SignupValues,
    on_values_change: F,
) -> SignupFormView<State> {
    SignupFormView {
        values,
        status: SignupStatus::default(),
        // The component's own prop defaults.
        title: "Create your account".to_string(),
        description: "Start building in under a minute.".to_string(),
        submit_label: "Create account".to_string(),
        terms_label: "I agree to the Terms and Privacy Policy".to_string(),
        error_message: None,
        strength_meter: true,
        validate: None,
        on_values_change: Rc::new(on_values_change),
        on_submit: None,
    }
}

impl<State: 'static> SignupFormView<State> {
    /// Set the submission status (`status`) — see [`SignupStatus`].
    pub fn status(mut self, status: SignupStatus) -> Self {
        self.status = status;
        self
    }

    /// Replace the header copy (`title` / `description`).
    pub fn header(mut self, title: impl Into<String>, description: impl Into<String>) -> Self {
        self.title = title.into();
        self.description = description.into();
        self
    }

    /// Set the submit button's label (`submitLabel`).
    pub fn submit_label(mut self, label: impl Into<String>) -> Self {
        self.submit_label = label.into();
        self
    }

    /// Set the terms row's label.
    pub fn terms_label(mut self, label: impl Into<String>) -> Self {
        self.terms_label = label.into();
        self
    }

    /// Show a form-level failure above the submit button (`errorMessage`).
    pub fn error_message(mut self, message: impl Into<String>) -> Self {
        self.error_message = Some(message.into());
        self
    }

    /// Show or hide the strength meter (`strengthMeter`, default on).
    pub fn strength_meter(mut self, shown: bool) -> Self {
        self.strength_meter = shown;
        self
    }

    /// Replace [`default_validate`] with a caller-supplied rule set
    /// (`validate`).
    pub fn validate<F: Fn(&SignupValues) -> SignupErrors + 'static>(mut self, validate: F) -> Self {
        self.validate = Some(Rc::new(validate));
        self
    }

    /// Report a submit that passed validation, with the values
    /// (`onSubmit`, called *"with valid values only"*).
    pub fn on_submit<F: Fn(&mut State, SignupValues) + 'static>(mut self, on_submit: F) -> Self {
        self.on_submit = Some(Rc::new(on_submit));
        self
    }

    /// The current errors, from the caller's rules or the built-in ones.
    fn errors(&self) -> SignupErrors {
        match &self.validate {
            Some(validate) => validate(&self.values),
            None => default_validate(&self.values),
        }
    }

    /// The strength score of the current password.
    fn strength(&self) -> u8 {
        password_strength(&self.values.password)
    }

    /// Whether the strength group is on screen
    /// (`strengthMeter && values.password.length > 0`).
    fn shows_strength(&self) -> bool {
        self.strength_meter && !self.values.password.is_empty()
    }

    /// One wrapped field, built from the current values, the resolved errors,
    /// and the widget-held touched/reveal state (see the [module docs](self)).
    fn field_view(
        &self,
        field: SignupField,
        errors: &SignupErrors,
        touched: bool,
        reveal: bool,
    ) -> AnyView<State> {
        let values = self.values.clone();
        let on_change = self.on_values_change.clone();
        let shown_error = touched.then(|| errors.get(field)).flatten();
        // `isValid`: the check draws only once a touched field is non-empty and
        // has no error of its own.
        let valid = touched && shown_error.is_none() && field.is_filled(&self.values);

        let mut view =
            input::<State, _>(field.text(&self.values), move |state: &mut State, text| {
                let mut next = values.clone();
                field.set_text(&mut next, text);
                on_change(state, next);
            })
            .label(field.label())
            .placeholder(field.placeholder())
            .reserve_message_line(true)
            .disabled(self.status.is_submitting())
            .password(field.is_secret() && !reveal)
            .success(valid);
        if let Some(message) = shown_error {
            view = view.error(message);
        }
        any(view)
    }

    /// The terms checkbox.
    fn terms_view(&self) -> AnyView<State> {
        let values = self.values.clone();
        let on_change = self.on_values_change.clone();
        any(
            checkbox::<State, _>(self.values.terms, move |state: &mut State, next| {
                let mut values = values.clone();
                values.terms = next;
                on_change(state, values);
            })
            .disabled(self.status.is_submitting())
            .label(self.terms_label.clone()),
        )
    }

    /// The submit button — the stateful variant, so the lifecycle is its own.
    fn submit_view(&self, errors: &SignupErrors) -> AnyView<State> {
        let on_submit = self.on_submit.clone();
        let values = self.values.clone();
        let valid = errors.is_empty();
        any(
            button::<State>(self.submit_label.clone(), move |state: &mut State| {
                // `if (Object.keys(errors).length > 0) return;` — a submit that
                // fails validation touches every field (the widget's own arm)
                // and reports nothing.
                if !valid {
                    return;
                }
                if let Some(on_submit) = &on_submit {
                    on_submit(state, values.clone());
                }
            })
            .variant(ButtonVariant::Stateful)
            .state(self.status.button_state())
            .size(ButtonSize::Lg)
            .loading_label("Creating account")
            .success_label("Account created")
            .error_label("Try again"),
        )
    }
}

// ---- Widget ----------------------------------------------------------------

/// The retained widget for a [`SignupFormView`].
pub struct SignupFormWidget {
    /// One pod per text field, in [`SignupField::TEXT`] order.
    fields: Vec<ChildPod>,
    terms: ChildPod,
    submit: ChildPod,
    title: Label,
    description: Label,
    terms_label: Label,
    /// A measuring run shaped exactly like the wrapped field's own label row —
    /// the only way to know where a field's pill starts, since the pod reports
    /// one box for label + pill + message row.
    field_label: Label,
    terms_error: Label,
    form_error: Label,
    strength_label: Label,
    /// Which fields have been left (or submitted through) — this widget's own
    /// state, see the [module docs](self).
    touched: [bool; 5],
    /// What `touched` was when the child views were last assembled, so a
    /// rebuild diffs the props it actually produced last time.
    touched_at_last_build: [bool; 5],
    /// Whether each field pod held focus on the previous pass, so a
    /// `true → false` transition can be read as a blur.
    was_focused: [bool; 4],
    /// Whether the two secret fields are revealed (`revealPassword`).
    reveal: bool,
    reveal_at_last_build: bool,
    /// Whether the terms row currently has a shown error.
    terms_error_shown: bool,
    /// Whether a form-level error is currently set.
    form_error_shown: bool,
    strength: u8,
    shows_strength: bool,
    /// Whether the strength meter was visible on the previous paint.
    strength_was_visible: bool,
    submitting: bool,
    /// The strength group's presence.
    strength_presence: Presence,
    /// The terms error's presence.
    terms_error_presence: Presence,
    /// The form error's presence.
    form_error_presence: Presence,
    /// One `0 → 1` lane per strength bar (`scaleX`).
    bars: Vec<Lane>,
    /// The reveal toggle's box, resolved by layout.
    eye: Rect,
    /// Whether the reveal toggle is armed by a `Down`.
    eye_armed: bool,
    /// The strength group's and the two message rows' boxes.
    strength_rect: Rect,
    terms_error_rect: Rect,
    form_error_rect: Rect,
    width: f64,
    height: f64,
}

/// The lanes a fresh strength meter starts with.
fn bar_lanes(strength: u8) -> Vec<Lane> {
    (0..STRENGTH_BARS)
        .map(|index| {
            Lane::at_rest(
                Ramp::spring(SPRING_LAYOUT),
                if index < strength as usize { 1.0 } else { 0.0 },
            )
        })
        .collect()
}

impl<State: 'static> View<State> for SignupFormView<State> {
    type Element = SignupFormWidget;

    fn build(&self, ctx: &mut BuildCtx<'_>) -> SignupFormWidget {
        let errors = self.errors();
        let strength = self.strength();
        let shows_strength = self.shows_strength();
        let mut strength_presence = Presence::symmetric(STRENGTH_RAMP);
        strength_presence.set_open(shows_strength);
        settle(&mut strength_presence, STRENGTH_FADE_MS);
        let mut terms_error_presence = Presence::symmetric(MESSAGE_RAMP);
        let mut form_error_presence = Presence::symmetric(MESSAGE_RAMP);
        form_error_presence.set_open(self.error_message.is_some());
        settle(&mut form_error_presence, MESSAGE_FADE_MS);
        terms_error_presence.set_open(false);

        SignupFormWidget {
            fields: SignupField::TEXT
                .into_iter()
                .map(|field| build_child(&self.field_view(field, &errors, false, false), ctx))
                .collect(),
            terms: build_child(&self.terms_view(), ctx),
            submit: build_child(&self.submit_view(&errors), ctx),
            title: Label::new(&self.title),
            description: Label::new(&self.description),
            terms_label: Label::new(&self.terms_label),
            field_label: Label::new(SignupField::Password.label()),
            terms_error: Label::new(errors.terms.clone().unwrap_or_default()),
            form_error: Label::new(self.error_message.clone().unwrap_or_default()),
            strength_label: Label::new(strength_line(strength)),
            touched: [false; 5],
            touched_at_last_build: [false; 5],
            was_focused: [false; 4],
            reveal: false,
            reveal_at_last_build: false,
            terms_error_shown: false,
            form_error_shown: self.error_message.is_some(),
            strength,
            shows_strength,
            strength_was_visible: strength_presence.is_visible(),
            submitting: self.status.is_submitting(),
            strength_presence,
            terms_error_presence,
            form_error_presence,
            bars: bar_lanes(strength),
            eye: Rect::ZERO,
            eye_armed: false,
            strength_rect: Rect::ZERO,
            terms_error_rect: Rect::ZERO,
            form_error_rect: Rect::ZERO,
            width: 0.0,
            height: 0.0,
        }
    }

    fn rebuild(
        &self,
        prev: &Self,
        element: &mut SignupFormWidget,
        ctx: &mut BuildCtx<'_>,
    ) -> ChangeFlags {
        let errors = self.errors();
        let prev_errors = prev.errors();
        let touched = element.touched;
        let previous_touched = element.touched_at_last_build;
        let reveal = element.reveal;
        let previous_reveal = element.reveal_at_last_build;

        let mut flags = ChangeFlags::NONE;
        for (index, field) in SignupField::TEXT.into_iter().enumerate() {
            flags |= rebuild_child(
                &prev.field_view(
                    field,
                    &prev_errors,
                    previous_touched[index],
                    previous_reveal,
                ),
                &self.field_view(field, &errors, touched[index], reveal),
                &mut element.fields[index],
                ctx,
            );
        }
        element.touched_at_last_build = touched;
        element.reveal_at_last_build = reveal;

        flags |= rebuild_child(
            &prev.terms_view(),
            &self.terms_view(),
            &mut element.terms,
            ctx,
        );
        flags |= rebuild_child(
            &prev.submit_view(&prev_errors),
            &self.submit_view(&errors),
            &mut element.submit,
            ctx,
        );

        if element.title.set_content(&self.title)
            | element.description.set_content(&self.description)
            | element.terms_label.set_content(&self.terms_label)
        {
            flags |= ChangeFlags::LAYOUT | ChangeFlags::PAINT;
        }

        // The terms message is shown on the same reward-early rule the fields
        // use, and kept mounted through its exit.
        let terms_error = touched[SignupField::Terms.index()]
            .then(|| errors.get(SignupField::Terms))
            .flatten();
        if let Some(message) = terms_error {
            element.terms_error.set_content(message);
        }
        if element.terms_error_shown != terms_error.is_some() {
            element.terms_error_shown = terms_error.is_some();
            element.terms_error_presence.set_open(terms_error.is_some());
            flags |= ChangeFlags::LAYOUT | ChangeFlags::PAINT;
        }

        if let Some(message) = &self.error_message {
            element.form_error.set_content(message);
        }
        if element.form_error_shown != self.error_message.is_some() {
            element.form_error_shown = self.error_message.is_some();
            element
                .form_error_presence
                .set_open(self.error_message.is_some());
            flags |= ChangeFlags::LAYOUT | ChangeFlags::PAINT;
        }

        let strength = self.strength();
        if element.strength != strength {
            element.strength = strength;
            element.strength_label.set_content(strength_line(strength));
            for (index, bar) in element.bars.iter_mut().enumerate() {
                bar.retarget(if index < strength as usize { 1.0 } else { 0.0 });
            }
            flags |= ChangeFlags::LAYOUT | ChangeFlags::PAINT;
        }
        let shows_strength = self.shows_strength();
        if element.shows_strength != shows_strength {
            element.shows_strength = shows_strength;
            element.strength_presence.set_open(shows_strength);
            flags |= ChangeFlags::LAYOUT | ChangeFlags::PAINT;
        }

        if element.submitting != self.status.is_submitting() {
            element.submitting = self.status.is_submitting();
            if element.submitting {
                // A form disabled mid-press keeps no armed state behind.
                element.eye_armed = false;
            }
            flags |= ChangeFlags::PAINT;
        }

        flags
    }

    fn teardown(&self, element: &mut SignupFormWidget, ctx: &mut BuildCtx<'_>) {
        let errors = self.errors();
        for (index, field) in SignupField::TEXT.into_iter().enumerate() {
            teardown_child(
                &self.field_view(field, &errors, element.touched[index], element.reveal),
                &mut element.fields[index],
                ctx,
            );
        }
        teardown_child(&self.terms_view(), &mut element.terms, ctx);
        teardown_child(&self.submit_view(&errors), &mut element.submit, ctx);
    }
}

/// Drive `presence` past its own ramp, so a form that mounts with a message
/// already showing rests showing it rather than playing an entrance.
fn settle(presence: &mut Presence, ramp_ms: u64) {
    presence.advance(FrameTime::ZERO);
    presence.advance(FrameTime::from_nanos(
        Duration::from_millis(ramp_ms).as_nanos() as u64 + 1,
    ));
}

/// The meter's caption (`Password strength: {STRENGTH_LABELS[strength]}`).
fn strength_line(strength: u8) -> String {
    format!(
        "Password strength: {}",
        STRENGTH_LABELS[(strength as usize).min(STRENGTH_LABELS.len() - 1)]
    )
}

/// The palette the form paints from.
struct FormColors {
    border: Color,
    ink: Color,
    muted: Color,
    destructive: Color,
    destructive_wash: Color,
    warning: Color,
    success: Color,
}

fn resolve_colors(theme: Option<&Theme>) -> FormColors {
    match theme {
        Some(theme) => {
            let scheme = theme.scheme();
            let tokens = BeuiTokens::resolve(Some(theme));
            FormColors {
                border: scheme.outline_variant,
                ink: scheme.on_surface,
                muted: scheme.on_surface_variant,
                destructive: scheme.error,
                destructive_wash: scheme.error_container,
                warning: tokens.warning,
                success: tokens.success,
            }
        }
        None => FormColors {
            border: FALLBACK_BORDER,
            ink: FALLBACK_FOREGROUND,
            muted: FALLBACK_MUTED,
            destructive: FALLBACK_DESTRUCTIVE,
            destructive_wash: style::with_alpha(FALLBACK_DESTRUCTIVE, 0.1),
            warning: BeuiTokens::beui().warning,
            success: BeuiTokens::beui().success,
        },
    }
}

impl FormColors {
    /// The bar colour a score paints in (`STRENGTH_COLORS`), with upstream's
    /// two ambers folded onto the catalog's one warning hue.
    fn strength_hue(&self, strength: u8) -> Color {
        match strength {
            0 | 1 => self.destructive,
            2 | 3 => self.warning,
            _ => self.success,
        }
    }
}

/// `font-semibold` at `size`.
fn semibold(size: f64, color: Color) -> TextStyle {
    TextStyle {
        family: sans_family(),
        weight: FontWeight::SEMI_BOLD,
        size: size as f32,
        color,
        ..TextStyle::default()
    }
}

/// `font-medium` at `size`.
fn medium(size: f64, color: Color) -> TextStyle {
    TextStyle {
        family: sans_family(),
        weight: FontWeight::MEDIUM,
        size: size as f32,
        color,
        ..TextStyle::default()
    }
}

/// A plain run at `size`.
fn plain(size: f64, color: Color) -> TextStyle {
    TextStyle {
        family: sans_family(),
        size: size as f32,
        color,
        ..TextStyle::default()
    }
}

impl SignupFormWidget {
    /// Whether the reveal toggle answers a press at all.
    fn eye_enabled(&self) -> bool {
        !self.submitting
    }

    /// Latch each field pod's focus flag, reporting which fields were just
    /// left. The blur observation the [module docs](self) explain.
    fn observe_blurs(&mut self) -> bool {
        let mut touched_any = false;
        for (index, pod) in self.fields.iter().enumerate() {
            let focused = pod.is_focused();
            if self.was_focused[index] && !focused && !self.touched[index] {
                self.touched[index] = true;
                touched_any = true;
            }
            self.was_focused[index] = focused;
        }
        touched_any
    }

    /// Mark every field touched — upstream's submit-time `setTouched({...})`.
    fn touch_all(&mut self) {
        self.touched = [true; 5];
    }

    /// The strength group's height, at full presence.
    fn strength_height(&self) -> f64 {
        STRENGTH_BAR_HEIGHT + STRENGTH_GAP + self.strength_label.size().height
    }

    /// The terms error row's height, at full presence.
    fn terms_error_height(&self) -> f64 {
        self.terms_error.size().height
    }

    /// The form error row's height, at full presence.
    fn form_error_height(&self) -> f64 {
        self.form_error.size().height + FORM_ERROR_PADDING_Y * 2.0
    }
}

impl Widget for SignupFormWidget {
    fn layout(&mut self, ctx: &mut LayoutCtx, bc: &BoxConstraints) -> Size {
        let theme = Theme::from_layout_ctx(ctx);
        let colors = resolve_colors(theme);

        let available = if bc.max().width.is_finite() {
            bc.max().width
        } else {
            FORM_MAX_WIDTH
        };
        let width = FORM_MAX_WIDTH.min(available);
        self.width = width;
        let content = (width - FORM_PADDING * 2.0).max(1.0);

        let title = self.title.layout(ctx, &semibold(TITLE_SIZE, colors.ink));
        let description = self
            .description
            .layout(ctx, &plain(style::TEXT_SM, colors.muted));
        self.terms_label
            .layout(ctx, &plain(style::TEXT_SM, colors.ink));
        // Shaped in the wrapped field's own label style (`text-sm font-medium`).
        let field_label = self
            .field_label
            .layout(ctx, &medium(style::TEXT_SM, colors.ink));
        self.terms_error
            .layout(ctx, &plain(style::TEXT_XS, colors.destructive));
        self.form_error
            .layout(ctx, &plain(style::TEXT_XS, colors.destructive));
        self.strength_label
            .layout(ctx, &plain(style::TEXT_XS, colors.muted));

        let loose = BoxConstraints::new(Size::ZERO, Size::new(content, f64::INFINITY));

        let mut y = FORM_PADDING;
        y += title.height + HEADER_GAP + description.height;
        y += SECTION_GAP;

        // The four fields, with the strength group tucked under the password.
        for (index, field) in SignupField::TEXT.into_iter().enumerate() {
            let size = self.fields[index].layout_child(ctx, &loose);
            self.fields[index].set_origin(Point::new(FORM_PADDING, y));
            if field == SignupField::Password {
                // The reveal toggle sits in the field's own trailing padding,
                // vertically centred on the pill. The pill starts one label row
                // and one gap below the pod's top — the wrapped field's own
                // layout, measured through `field_label` rather than guessed
                // from the bottom, where the message row's height varies.
                let pill_top = y + field_label.height + beui_input::ROW_GAP;
                self.eye = Rect::from_origin_size(
                    Point::new(
                        FORM_PADDING + content - style::PADDING_X_INPUT - EYE_BOX,
                        pill_top + (style::HEIGHT_INPUT - EYE_BOX) / 2.0,
                    ),
                    Size::new(EYE_BOX, EYE_BOX),
                );
            }
            y += size.height;
            if field == SignupField::Password {
                let height = if self.strength_presence.is_visible() {
                    self.strength_height()
                } else {
                    0.0
                };
                self.strength_rect = Rect::from_origin_size(
                    Point::new(FORM_PADDING + STRENGTH_PADDING_X, y + STRENGTH_GAP),
                    Size::new((content - STRENGTH_PADDING_X * 2.0).max(1.0), height),
                );
                if height > 0.0 {
                    y += STRENGTH_GAP + height;
                }
            }
            y += FIELD_GAP;
        }
        y -= FIELD_GAP;
        y += SECTION_GAP;

        // The terms row: the checkbox pod, its label beside it, and the message
        // row under both.
        let terms = self.terms.layout_child(ctx, &loose);
        self.terms.set_origin(Point::new(FORM_PADDING, y));
        y += terms.height;
        let terms_error_height = if self.terms_error_presence.is_visible() {
            self.terms_error_height()
        } else {
            0.0
        };
        self.terms_error_rect = Rect::from_origin_size(
            Point::new(FORM_PADDING + MESSAGE_PADDING_X, y + TERMS_GAP),
            Size::new(content, terms_error_height),
        );
        if terms_error_height > 0.0 {
            y += TERMS_GAP + terms_error_height;
        }
        y += SECTION_GAP;

        let form_error_height = if self.form_error_presence.is_visible() {
            self.form_error_height()
        } else {
            0.0
        };
        self.form_error_rect = Rect::from_origin_size(
            Point::new(FORM_PADDING, y),
            Size::new(content, form_error_height),
        );
        if form_error_height > 0.0 {
            y += form_error_height + SECTION_GAP;
        }

        // `className="w-full"`: the submit pill spans the content column.
        let submit = self.submit.layout_child(
            ctx,
            &BoxConstraints::tight(Size::new(content, style::HEIGHT_LG)),
        );
        self.submit.set_origin(Point::new(FORM_PADDING, y));
        y += submit.height;
        y += FORM_PADDING;

        self.height = y;
        bc.constrain(Size::new(width, y))
    }

    fn paint(&mut self, ctx: &mut PaintCtx, scene: &mut dyn PaintScene) {
        let now = ctx.frame_time();
        let origin = ctx.origin();
        let (colors, reduce) = {
            let theme = Theme::from_paint_ctx(ctx);
            (
                resolve_colors(theme),
                theme.is_some_and(|t| t.motion.reduce_motion),
            )
        };

        let mut owes_frame = false;
        if reduce {
            self.strength_presence = self.strength_presence.collapsed();
            self.terms_error_presence = self.terms_error_presence.collapsed();
            self.form_error_presence = self.form_error_presence.collapsed();
            for bar in &mut self.bars {
                bar.snap();
            }
        } else {
            for bar in &mut self.bars {
                owes_frame |= bar.advance(now);
            }
        }
        let strength_shown = self.strength_presence.advance(now);
        let terms_shown = self.terms_error_presence.advance(now);
        let form_shown = self.form_error_presence.advance(now);
        owes_frame |= self.strength_presence.is_animating()
            || self.terms_error_presence.is_animating()
            || self.form_error_presence.is_animating();

        // Request layout when the strength meter's visibility state changes
        // (entering or exiting), so the layout is recomputed on the frame it
        // becomes visible and on the frame it settles away.
        let strength_is_visible = self.strength_presence.is_visible();
        if self.strength_was_visible != strength_is_visible {
            ctx.request_layout();
        }
        self.strength_was_visible = strength_is_visible;

        // The card itself.
        let size = Size::new(self.width, self.height);
        let radius = style::resolve_radius(style::RADIUS_3XL, size.width, size.height);
        let half = style::BORDER_WIDTH / 2.0;
        let frame = RoundedRect::from_rect(
            Rect::from_origin_size(Point::ORIGIN, size).inset(-half),
            (radius - half).max(0.0),
        );
        scene.stroke_path(
            origin,
            &Shape::to_path(&frame, style::PATH_TOLERANCE),
            style::BORDER_WIDTH,
            &Brush::Solid(colors.border),
        );

        // The header.
        let header = Point::new(origin.x + FORM_PADDING, origin.y + FORM_PADDING);
        self.title.paint(header, scene);
        self.description.paint(
            Point::new(header.x, header.y + self.title.size().height + HEADER_GAP),
            scene,
        );

        // The fields, and the reveal toggle over the password's trailing edge.
        for pod in &mut self.fields {
            pod.paint_child(ctx, scene);
        }
        if self.eye.height() > 0.0 {
            let centre = Point::new(
                origin.x + self.eye.center().x,
                origin.y + self.eye.center().y,
            );
            let ink = if self.eye_armed {
                colors.ink
            } else {
                colors.muted
            };
            eye_glyph(centre, EYE_ICON, self.reveal, ink, scene);
        }

        // The strength meter.
        if strength_shown > 0.0 {
            self.paint_strength(origin, strength_shown, &colors, scene);
        }

        self.terms.paint_child(ctx, scene);
        let label = Point::new(
            origin.x + self.terms.origin().x + self.terms.size().width + TERMS_LABEL_GAP,
            origin.y
                + self.terms.origin().y
                + (self.terms.size().height - self.terms_label.size().height) / 2.0,
        );
        self.terms_label.paint(label, scene);

        if terms_shown > 0.0 {
            let rise = MESSAGE_RISE * (1.0 - terms_shown);
            let at = Point::new(
                origin.x + self.terms_error_rect.x0,
                origin.y + self.terms_error_rect.y0 - rise,
            );
            let box_size = Size::new(
                self.terms_error_rect.width().max(1.0),
                self.terms_error.size().height.max(1.0),
            );
            scene.push_layer(at, box_size, terms_shown as f32);
            self.terms_error.paint(at, scene);
            scene.pop_layer();
        }

        if form_shown > 0.0 {
            self.paint_form_error(origin, form_shown, &colors, scene);
        }

        self.submit.paint_child(ctx, scene);

        if owes_frame {
            // The strength group and both message rows are height-bearing, so
            // an animating form owes a relayout rather than a bare repaint.
            ctx.request_layout();
        }
    }

    fn event(&mut self, ctx: &mut EventCtx, event: &InputEvent) -> EventResult {
        // The reveal toggle is hit-tested ahead of the pods: it sits over the
        // password field's own trailing padding, so the field would otherwise
        // take the press.
        if let InputEvent::Pointer(p) = event {
            let over_eye = self.eye.height() > 0.0 && self.eye.contains(p.position);
            match p.phase {
                PointerPhase::Down if over_eye && self.eye_enabled() && presses(p) => {
                    self.eye_armed = true;
                    ctx.capture_pointer();
                    ctx.request_redraw();
                    return EventResult::Handled;
                }
                PointerPhase::Up if self.eye_armed => {
                    self.eye_armed = false;
                    if over_eye {
                        self.reveal = !self.reveal;
                    }
                    ctx.request_redraw();
                    return EventResult::Handled;
                }
                PointerPhase::Cancel if self.eye_armed => {
                    // A `Cancel` arm clears internal flags only.
                    self.eye_armed = false;
                    ctx.request_redraw();
                    return EventResult::Handled;
                }
                PointerPhase::Move if self.eye_armed => {
                    ctx.set_cursor(style::ACTIVE_CURSOR);
                    return EventResult::Handled;
                }
                PointerPhase::Move if over_eye && self.eye_enabled() => {
                    ctx.claim_hover();
                    ctx.set_cursor(style::ACTIVE_CURSOR);
                }
                _ => {}
            }
        }

        // The submit pod is routed first so a submit press is seen here before
        // the blur it also causes: pressing the button blurs whichever field
        // held focus, and upstream touches every field on submit anyway.
        let mut result = route_event_single(&mut self.submit, ctx, event);
        if result == EventResult::Handled
            && matches!(
                event,
                InputEvent::Pointer(p) if p.phase == PointerPhase::Up
            )
        {
            self.touch_all();
            ctx.request_redraw();
        }
        if result != EventResult::Handled {
            for index in 0..self.fields.len() {
                if route_event_single(&mut self.fields[index], ctx, event) == EventResult::Handled {
                    result = EventResult::Handled;
                    break;
                }
            }
        }
        if result != EventResult::Handled {
            result = route_event_single(&mut self.terms, ctx, event);
            if result == EventResult::Handled
                && matches!(
                    event,
                    InputEvent::Pointer(p) if p.phase == PointerPhase::Up
                )
            {
                // `onCheckedChange` touches the terms as well as setting them.
                self.touched[SignupField::Terms.index()] = true;
            }
        }

        // Whatever the event was, a field that has just lost focus is now
        // touched — the blur observation, taken after routing so the pods'
        // recorded focus flags are up to date.
        if self.observe_blurs() {
            ctx.request_redraw();
        }
        result
    }

    fn semantics(&self, ctx: &mut SemanticsCtx) {
        ctx.push_container(
            Role::Form,
            |node| {
                node.set_label("Sign up");
                node.add_action(Action::Focus);
            },
            |ctx| {
                for pod in &self.fields {
                    pod.semantics_child(ctx);
                }
                self.terms.semantics_child(ctx);
                self.submit.semantics_child(ctx);
            },
        );
    }

    visit_children!(fields, terms, submit);
}

impl SignupFormWidget {
    /// The four bars and their caption.
    fn paint_strength(
        &self,
        origin: Point,
        shown: f64,
        colors: &FormColors,
        scene: &mut dyn PaintScene,
    ) {
        let rise = MESSAGE_RISE * (1.0 - shown);
        let at = Point::new(
            origin.x + self.strength_rect.x0,
            origin.y + self.strength_rect.y0 - rise,
        );
        let width = self.strength_rect.width();
        scene.push_layer(
            at,
            Size::new(width.max(1.0), self.strength_height().max(1.0)),
            shown as f32,
        );

        let bar_width = ((width - STRENGTH_BAR_GAP * (STRENGTH_BARS - 1) as f64)
            / STRENGTH_BARS as f64)
            .max(1.0);
        let hue = colors.strength_hue(self.strength);
        for index in 0..STRENGTH_BARS {
            let x = at.x + index as f64 * (bar_width + STRENGTH_BAR_GAP);
            scene.fill_rounded_rect(
                Point::new(x, at.y),
                Size::new(bar_width, STRENGTH_BAR_HEIGHT),
                STRENGTH_BAR_HEIGHT / 2.0,
                style::with_alpha(colors.muted, STRENGTH_TRACK_ALPHA),
            );
            // `scaleX` from the left origin, so a filling bar grows rightward.
            let fill = self.bars[index].value().clamp(0.0, 1.0);
            if fill > 0.0 {
                scene.fill_rounded_rect(
                    Point::new(x, at.y),
                    Size::new((bar_width * fill).max(0.5), STRENGTH_BAR_HEIGHT),
                    STRENGTH_BAR_HEIGHT / 2.0,
                    hue,
                );
            }
        }

        self.strength_label.paint(
            Point::new(at.x, at.y + STRENGTH_BAR_HEIGHT + STRENGTH_GAP),
            scene,
        );
        scene.pop_layer();
    }

    /// The form-level failure: a washed, hairlined block above the submit.
    fn paint_form_error(
        &self,
        origin: Point,
        shown: f64,
        colors: &FormColors,
        scene: &mut dyn PaintScene,
    ) {
        let rise = MESSAGE_RISE * (1.0 - shown);
        let at = Point::new(
            origin.x + self.form_error_rect.x0,
            origin.y + self.form_error_rect.y0 - rise,
        );
        let size = Size::new(
            self.form_error_rect.width().max(1.0),
            self.form_error_height(),
        );
        scene.push_layer(at, size, shown as f32);
        scene.fill_rounded_rect(at, size, style::RADIUS_2XL, colors.destructive_wash);
        let half = style::BORDER_WIDTH / 2.0;
        let frame = RoundedRect::from_rect(
            Rect::from_origin_size(Point::ORIGIN, size).inset(-half),
            (style::RADIUS_2XL - half).max(0.0),
        );
        scene.stroke_path(
            at,
            &Shape::to_path(&frame, style::PATH_TOLERANCE),
            style::BORDER_WIDTH,
            &Brush::Solid(style::with_alpha(
                colors.destructive,
                FORM_ERROR_BORDER_ALPHA,
            )),
        );
        self.form_error.paint(
            Point::new(at.x + FORM_ERROR_PADDING_X, at.y + FORM_ERROR_PADDING_Y),
            scene,
        );
        scene.pop_layer();
    }
}

/// Lucide's `Eye` (or `EyeOff`, with the slash), centred on `centre`.
fn eye_glyph(centre: Point, size: f64, off: bool, color: Color, scene: &mut dyn PaintScene) {
    let half = size / 2.0;
    let stroke = 2.0 * size / 24.0;
    // The almond: two quadratic arcs meeting at the corners.
    let mut path = frust::authoring::BezPath::new();
    path.move_to(Point::new(centre.x - half, centre.y));
    path.quad_to(
        Point::new(centre.x, centre.y - half * 0.9),
        Point::new(centre.x + half, centre.y),
    );
    path.quad_to(
        Point::new(centre.x, centre.y + half * 0.9),
        Point::new(centre.x - half, centre.y),
    );
    scene.stroke_path(Point::ZERO, &path, stroke, &Brush::Solid(color));

    let mut pupil = frust::authoring::BezPath::new();
    let r = half * 0.32;
    pupil.move_to(Point::new(centre.x + r, centre.y));
    pupil.quad_to(
        Point::new(centre.x + r, centre.y + r),
        Point::new(centre.x, centre.y + r),
    );
    pupil.quad_to(
        Point::new(centre.x - r, centre.y + r),
        Point::new(centre.x - r, centre.y),
    );
    pupil.quad_to(
        Point::new(centre.x - r, centre.y - r),
        Point::new(centre.x, centre.y - r),
    );
    pupil.quad_to(
        Point::new(centre.x + r, centre.y - r),
        Point::new(centre.x + r, centre.y),
    );
    scene.stroke_path(Point::ZERO, &pupil, stroke, &Brush::Solid(color));

    if off {
        let mut slash = frust::authoring::BezPath::new();
        slash.move_to(Point::new(centre.x - half, centre.y + half));
        slash.line_to(Point::new(centre.x + half, centre.y - half));
        scene.stroke_path(Point::ZERO, &slash, stroke, &Brush::Solid(color));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use frust::authoring::scene::GlyphRun;
    use frust::authoring::text::TextContext;
    use frust::authoring::{BezPath, PointerButton, PointerEvent};
    use frust_core::RenderRoot;
    use std::any::Any;

    /// Records the ops these tests assert on.
    #[derive(Default)]
    struct Recorder {
        rrects: Vec<(Point, Size, f64, Color)>,
        strokes: Vec<(Rect, f64, Color)>,
        glyphs: Vec<Point>,
        layers: Vec<f32>,
    }

    impl PaintScene for Recorder {
        fn fill_rect(&mut self, _o: Point, _s: Size, _c: Color) {}
        fn draw_text(&mut self, _o: Point, _t: &str) {}
        fn fill_rounded_rect(&mut self, o: Point, s: Size, r: f64, c: Color) {
            self.rrects.push((o, s, r, c));
        }
        fn stroke_path(&mut self, origin: Point, path: &BezPath, width: f64, brush: &Brush) {
            let bbox = path.bounding_box() + origin.to_vec2();
            let color = match brush {
                Brush::Solid(c) => *c,
                _ => Color::TRANSPARENT,
            };
            self.strokes.push((bbox, width, color));
        }
        fn draw_glyph_run(&mut self, run: GlyphRun) {
            let t = run.transform.translation();
            self.glyphs.push(Point::new(t.x, t.y));
        }
        fn push_layer(&mut self, _o: Point, _s: Size, alpha: f32) {
            self.layers.push(alpha);
        }
    }

    impl Recorder {
        /// Every strength-bar rect of the pass, by height — the four tracks and
        /// however many fills, interleaved in paint order.
        fn bars(&self) -> Vec<(Point, Size, Color)> {
            self.rrects
                .iter()
                .filter(|(_, s, _, _)| (s.height - STRENGTH_BAR_HEIGHT).abs() < 1e-9)
                .map(|(o, s, _, c)| (*o, *s, *c))
                .collect()
        }

        /// The bar tracks — the ones painted at [`STRENGTH_TRACK_ALPHA`].
        fn bar_tracks(&self) -> Vec<(Point, Size, Color)> {
            self.bars()
                .into_iter()
                .filter(|(_, _, c)| (c.components[3] - STRENGTH_TRACK_ALPHA).abs() < 1e-6)
                .collect()
        }

        /// The bar fills — everything else.
        fn bar_fills(&self) -> Vec<(Point, Size, Color)> {
            self.bars()
                .into_iter()
                .filter(|(_, _, c)| (c.components[3] - STRENGTH_TRACK_ALPHA).abs() >= 1e-6)
                .collect()
        }
    }

    /// What the app state records.
    #[derive(Default)]
    struct App {
        values: SignupValues,
        submitted: Vec<SignupValues>,
    }

    const WINDOW: Size = Size::new(420.0, 1_400.0);

    fn ft_ms(millis: f64) -> FrameTime {
        FrameTime::from_nanos((millis * 1_000_000.0) as u64)
    }

    fn pointer(phase: PointerPhase, at: Point) -> InputEvent {
        InputEvent::Pointer(PointerEvent {
            phase,
            position: at,
            button: PointerButton::Primary,
        })
    }

    fn filled() -> SignupValues {
        SignupValues {
            name: "Ada Lovelace".to_string(),
            email: "ada@example.com".to_string(),
            password: "correct horse".to_string(),
            confirm_password: "correct horse".to_string(),
            terms: true,
        }
    }

    /// A form under a real `RenderRoot` — the only harness that can move focus,
    /// which is what the touched machine is driven by.
    struct Harness {
        root: RenderRoot<App, SignupFormView<App>>,
        state: App,
        tcx: TextContext,
        status: SignupStatus,
        error_message: Option<String>,
        strength_meter: bool,
    }

    impl Harness {
        fn new(values: SignupValues) -> Self {
            let mut h = Harness {
                root: RenderRoot::new(),
                state: App {
                    values,
                    submitted: Vec::new(),
                },
                tcx: TextContext::new(),
                status: SignupStatus::Idle,
                error_message: None,
                strength_meter: true,
            };
            h.root.set_theme(Box::new(crate::theme()));
            h.pass();
            h
        }

        fn reduce_motion(&mut self) {
            let mut theme = crate::theme();
            theme.motion.reduce_motion = true;
            self.root.set_theme(Box::new(theme));
            self.pass();
        }

        fn pass(&mut self) {
            let (status, error, meter) =
                (self.status, self.error_message.clone(), self.strength_meter);
            let mut logic = move |state: &mut App| {
                let mut view =
                    signup_form::<App, _>(state.values.clone(), |s: &mut App, values| {
                        s.values = values;
                    })
                    .status(status)
                    .strength_meter(meter)
                    .on_submit(|s: &mut App, values| s.submitted.push(values));
                if let Some(message) = &error {
                    view = view.error_message(message.clone());
                }
                view
            };
            self.root.rebuild(&mut logic, &mut self.state);
            self.root
                .layout_with_text(WINDOW, &mut self.tcx as &mut dyn Any);
        }

        fn dispatch(&mut self, event: &InputEvent) {
            self.root.event(&mut self.state, event);
            self.pass();
        }

        fn click(&mut self, at: Point) {
            self.dispatch(&pointer(PointerPhase::Down, at));
            self.dispatch(&pointer(PointerPhase::Up, at));
        }

        fn paint_at(&mut self, millis: f64) -> Recorder {
            let mut rec = Recorder::default();
            self.root.paint(&mut rec, ft_ms(millis));
            rec
        }

        fn paint(&mut self) -> Recorder {
            self.paint_at(0.0)
        }

        /// Paint one frame with no rebuild or layout in between — the
        /// production frame shape — and report whether it asked for layout.
        fn paint_only(&mut self, millis: f64) -> bool {
            let mut rec = Recorder::default();
            self.root.paint(&mut rec, ft_ms(millis)).needs_layout
        }

        /// Run layout alone (no rebuild), the way the shell answers a
        /// `request_layout`, and return the laid-out size.
        fn relayout(&mut self) -> Size {
            self.root
                .layout_with_text(WINDOW, &mut self.tcx as &mut dyn Any)
        }
    }

    fn build(view: &SignupFormView<App>) -> SignupFormWidget {
        let mut counter = 0u64;
        View::<App>::build(view, &mut BuildCtx::new(&mut counter))
    }

    fn layout(w: &mut SignupFormWidget) -> Size {
        let mut tcx = TextContext::new();
        let lctx = LayoutCtx::with_text_context(&mut tcx as &mut dyn Any);
        let theme = crate::theme();
        let mut lctx = lctx.with_theme(&theme as &dyn Any);
        w.layout(&mut lctx, &BoxConstraints::loose(WINDOW))
    }

    // ---- The rules ----------------------------------------------------------

    #[test]
    fn the_email_shape_is_upstreams_permissive_one() {
        for good in [
            "ada@example.com",
            "a.b+c@sub.example.co.uk",
            "weird!name@example.io",
        ] {
            assert!(looks_like_email(good), "{good} should pass");
        }
        for bad in [
            "",
            "ada",
            "ada@example",
            "@example.com",
            "ada@.com",
            "ada@example.",
            "ada @example.com",
            "ada@ex ample.com",
            "ada@@example.com",
        ] {
            assert!(!looks_like_email(bad), "{bad} should fail");
        }
    }

    #[test]
    fn password_strength_is_length_weighted_and_capped() {
        // Below the floor scores zero whatever else it contains.
        assert_eq!(password_strength(""), 0);
        assert_eq!(password_strength("Ab1!Ab1"), 0, "seven characters");
        // At the floor, one point; extra classes are a nudge, not a rescue.
        assert_eq!(password_strength("abcdefgh"), 1);
        assert_eq!(password_strength("Abcdef1!"), 2, "8 chars + three classes");
        // Twelve and sixteen characters each add a point.
        assert_eq!(password_strength("abcdefghijkl"), 2);
        assert_eq!(password_strength("abcdefghijklmnop"), 3);
        assert_eq!(password_strength("Abcdefghijklmno1"), 4);
        // And it never exceeds the ladder's top.
        assert_eq!(password_strength("Abcdefghijklmnopqrstuvwxyz1!"), 4);
    }

    #[test]
    fn default_validate_is_upstreams_rule_set_message_for_message() {
        let empty = default_validate(&SignupValues::default());
        assert_eq!(empty.name.as_deref(), Some("Enter your name."));
        assert_eq!(empty.email.as_deref(), Some("Enter your email."));
        assert_eq!(empty.password.as_deref(), Some("Choose a password."));
        assert_eq!(
            empty.confirm_password.as_deref(),
            Some("Confirm your password.")
        );
        assert_eq!(
            empty.terms.as_deref(),
            Some("Accept the terms to continue.")
        );
        assert!(!empty.is_empty());

        let malformed = default_validate(&SignupValues {
            email: "ada".to_string(),
            ..SignupValues::default()
        });
        assert_eq!(
            malformed.email.as_deref(),
            Some("That doesn't look like an email address.")
        );

        let short = default_validate(&SignupValues {
            password: "abc".to_string(),
            confirm_password: "abc".to_string(),
            ..SignupValues::default()
        });
        assert_eq!(
            short.password.as_deref(),
            Some("Use at least 8 characters.")
        );
        assert!(
            short.confirm_password.is_none(),
            "a matching confirmation is valid even when the password is not"
        );

        let mismatched = default_validate(&SignupValues {
            password: "correct horse".to_string(),
            confirm_password: "battery staple".to_string(),
            ..SignupValues::default()
        });
        assert_eq!(
            mismatched.confirm_password.as_deref(),
            Some("Passwords don't match.")
        );

        assert!(default_validate(&filled()).is_empty(), "a full form passes");
    }

    #[test]
    fn a_caller_supplied_rule_set_replaces_the_built_in_one() {
        let view: SignupFormView<App> = signup_form(SignupValues::default(), |_s: &mut App, _v| {})
            .validate(|values| {
                let mut errors = SignupErrors::default();
                if values.name != "ada" {
                    errors.name = Some("only ada".to_string());
                }
                errors
            });
        let errors = view.errors();
        assert_eq!(errors.name.as_deref(), Some("only ada"));
        assert!(
            errors.email.is_none(),
            "the built-in rules are replaced, not merged"
        );
    }

    #[test]
    fn the_field_axis_reads_and_writes_its_own_slot() {
        let mut values = filled();
        assert_eq!(SignupField::Email.text(&values), "ada@example.com");
        SignupField::Email.set_text(&mut values, "grace@example.com".to_string());
        assert_eq!(values.email, "grace@example.com");
        assert!(SignupField::Terms.is_filled(&values));
        values.terms = false;
        assert!(!SignupField::Terms.is_filled(&values));
        assert!(SignupField::Password.is_secret());
        assert!(!SignupField::Name.is_secret());
        assert_eq!(SignupField::TEXT.len(), 4);
        assert_eq!(SignupField::ALL.len(), 5);
    }

    // ---- Reward early, punish late -------------------------------------------

    #[test]
    fn an_untouched_field_shows_no_error_and_a_left_one_does() {
        let view: SignupFormView<App> = signup_form(SignupValues::default(), |_s: &mut App, _v| {});
        let mut w = build(&view);
        layout(&mut w);
        assert_eq!(w.touched, [false; 5], "nothing is touched at mount");

        // A pod that held focus and no longer does is a blur.
        w.was_focused[SignupField::Name.index()] = true;
        assert!(w.observe_blurs());
        assert!(w.touched[SignupField::Name.index()]);
        assert!(
            !w.touched[SignupField::Email.index()],
            "only the field that was left"
        );
        // Re-observing the same state touches nothing further.
        assert!(!w.observe_blurs());
    }

    #[test]
    fn a_submit_touches_every_field_at_once() {
        let view: SignupFormView<App> = signup_form(SignupValues::default(), |_s: &mut App, _v| {});
        let mut w = build(&view);
        w.touch_all();
        assert_eq!(w.touched, [true; 5]);
    }

    #[test]
    fn leaving_a_field_surfaces_its_message_through_the_wrapped_input() {
        // The whole path, end to end: a real focus move blurs the name field,
        // the widget latches the touch, and the next rebuild hands the wrapped
        // `input` its error — which is one more painted run than before.
        let (name, terms) = {
            let mut w = build(&signup_form::<App, _>(
                SignupValues::default(),
                |_s: &mut App, _v| {},
            ));
            layout(&mut w);
            let name = &w.fields[SignupField::Name.index()];
            // The name field's pill sits at the same offset inside its pod as
            // the password field's, which `eye` already resolves.
            let pill_y = w.eye.center().y - w.fields[SignupField::Password.index()].origin().y;
            (
                Point::new(name.origin().x + 40.0, name.origin().y + pill_y),
                Point::new(
                    w.terms.origin().x + 8.0,
                    w.terms.origin().y + w.terms.size().height / 2.0,
                ),
            )
        };

        let mut h = Harness::new(SignupValues::default());
        h.paint_at(0.0);
        let before = h.paint_at(4_000.0).glyphs.len();

        h.click(name);
        assert!(h.root.is_focus_active(), "the field took focus");
        h.click(terms);

        // The message enters on a ramp, so it takes a settled frame to count.
        h.paint_at(4_001.0);
        let after = h.paint_at(8_000.0).glyphs.len();
        assert!(
            after > before,
            "the name field's message appeared once it was left: {before} -> {after}"
        );
    }

    // ---- Layout --------------------------------------------------------------

    #[test]
    fn the_form_caps_at_max_w_sm_and_stacks_its_sections() {
        let view: SignupFormView<App> = signup_form(SignupValues::default(), |_s: &mut App, _v| {});
        let mut w = build(&view);
        let size = layout(&mut w);
        assert_eq!(size.width, FORM_MAX_WIDTH);
        // The four fields descend in order, inside the card's padding.
        let mut previous = FORM_PADDING;
        for pod in &w.fields {
            assert_eq!(pod.origin().x, FORM_PADDING);
            assert!(pod.origin().y > previous);
            previous = pod.origin().y;
        }
        assert!(w.terms.origin().y > previous);
        assert!(w.submit.origin().y > w.terms.origin().y);
        assert!(w.submit.origin().y + w.submit.size().height + FORM_PADDING <= size.height + 1e-6);
        // The submit pill spans the content column.
        assert_eq!(w.submit.size().width, FORM_MAX_WIDTH - FORM_PADDING * 2.0);
    }

    #[test]
    fn the_reveal_toggle_sits_over_the_password_fields_trailing_padding() {
        let view: SignupFormView<App> = signup_form(SignupValues::default(), |_s: &mut App, _v| {});
        let mut w = build(&view);
        layout(&mut w);
        let password = &w.fields[SignupField::Password.index()];
        let field = Rect::from_origin_size(password.origin(), password.size());
        assert!(field.contains(w.eye.center()), "the eye is over the field");
        assert!(
            (w.eye.max_x()
                - (FORM_PADDING + FORM_MAX_WIDTH - FORM_PADDING * 2.0 - style::PADDING_X_INPUT))
                .abs()
                < 1e-6,
            "and inset by the field's own trailing padding"
        );
    }

    #[test]
    fn the_strength_group_takes_height_only_while_a_password_is_typed() {
        let view: SignupFormView<App> = signup_form(SignupValues::default(), |_s: &mut App, _v| {});
        let mut w = build(&view);
        let bare = layout(&mut w);
        assert_eq!(w.strength_rect.height(), 0.0);

        let typed: SignupFormView<App> = signup_form(
            SignupValues {
                password: "abcdefgh".to_string(),
                ..SignupValues::default()
            },
            |_s: &mut App, _v| {},
        );
        let mut w = build(&typed);
        let taller = layout(&mut w);
        assert!(w.strength_rect.height() > 0.0);
        assert!(taller.height > bare.height, "{taller:?} vs {bare:?}");

        // ...and not at all with the meter switched off.
        let off = typed.strength_meter(false);
        let mut w = build(&off);
        layout(&mut w);
        assert_eq!(w.strength_rect.height(), 0.0);
    }

    // ---- Paint ---------------------------------------------------------------

    #[test]
    fn the_meter_fills_one_bar_per_point_of_strength() {
        let mut h = Harness::new(SignupValues {
            password: "abcdefgh".to_string(),
            ..SignupValues::default()
        });
        h.paint_at(0.0);
        let rec = h.paint_at(4_000.0);
        // Four tracks always, plus one fill for the single point scored.
        assert_eq!(rec.bar_tracks().len(), STRENGTH_BARS);
        assert_eq!(rec.bar_fills().len(), 1);
        let theme = crate::theme();
        let destructive = theme.scheme().error;
        assert_eq!(rec.bar_fills()[0].2, destructive, "a weak score is red");

        // A stronger password fills more of them, in the success hue.
        h.state.values.password = "Abcdefghijklmno1".to_string();
        h.pass();
        h.paint_at(4_001.0);
        let rec = h.paint_at(8_000.0);
        assert_eq!(rec.bar_tracks().len(), STRENGTH_BARS);
        assert_eq!(rec.bar_fills().len(), STRENGTH_BARS, "every bar is lit");
        let success = BeuiTokens::resolve(Some(&theme)).success;
        assert_eq!(rec.bar_fills()[0].2, success);
    }

    #[test]
    fn a_bar_glides_rather_than_snapping_when_the_score_climbs() {
        let mut h = Harness::new(SignupValues {
            password: "abcdefgh".to_string(),
            ..SignupValues::default()
        });
        h.paint_at(0.0);
        h.paint_at(4_000.0);
        h.state.values.password = "abcdefghijkl".to_string();
        h.pass();
        h.paint_at(4_001.0);
        let mid = h.paint_at(4_030.0);
        // The second bar is part-way across its own track.
        let track = mid.bar_tracks()[0].1.width;
        let growing = mid
            .bar_fills()
            .iter()
            .any(|(_, size, _)| size.width > 0.0 && size.width < track - 1e-6);
        assert!(growing, "a bar is mid-glide");
    }

    #[test]
    fn the_form_level_error_paints_a_washed_block_above_the_submit() {
        let mut h = Harness::new(filled());
        let before = h.paint().rrects.len();
        h.error_message = Some("That email is already registered.".to_string());
        h.pass();
        h.paint_at(0.0);
        let rec = h.paint_at(4_000.0);
        assert!(rec.rrects.len() > before);
        let theme = crate::theme();
        let wash = theme.scheme().error_container;
        let block = rec
            .rrects
            .iter()
            .find(|(_, _, r, c)| (*r - style::RADIUS_2XL).abs() < 1e-9 && *c == wash)
            .expect("the washed error block");
        // It sits above the submit pill.
        assert!(block.0.y < WINDOW.height);
    }

    #[test]
    fn the_card_is_stroked_once_at_the_hairline_width() {
        let mut h = Harness::new(SignupValues::default());
        let rec = h.paint();
        let theme = crate::theme();
        let border = theme.scheme().outline_variant;
        assert!(
            rec.strokes
                .iter()
                .any(|(_, w, c)| *w == style::BORDER_WIDTH && *c == border),
            "the form's own hairline"
        );
    }

    #[test]
    fn reduced_motion_lands_every_lane_at_once() {
        let mut h = Harness::new(SignupValues {
            password: "abcdefgh".to_string(),
            ..SignupValues::default()
        });
        h.reduce_motion();
        let rec = h.paint_at(0.0);
        assert_eq!(rec.bar_tracks().len(), STRENGTH_BARS);
        assert_eq!(rec.bar_fills().len(), 1);
        assert!(
            (rec.bar_fills()[0].1.width - rec.bar_tracks()[0].1.width).abs() < 1e-6,
            "the fill is already at full width"
        );
    }

    // ---- Presses -------------------------------------------------------------

    #[test]
    fn the_reveal_toggle_flips_both_secret_fields() {
        let view: SignupFormView<App> = signup_form(SignupValues::default(), |_s: &mut App, _v| {});
        let mut w = build(&view);
        layout(&mut w);
        assert!(!w.reveal);

        let mut state = App::default();
        let any_state: &mut dyn Any = &mut state;
        let mut ctx = EventCtx::new(any_state, Point::ZERO, Size::new(w.width, w.height));
        let at = w.eye.center();
        w.event(&mut ctx, &pointer(PointerPhase::Down, at));
        assert!(w.eye_armed);
        w.event(&mut ctx, &pointer(PointerPhase::Up, at));
        assert!(w.reveal, "the toggle flipped");
        assert!(!w.eye_armed);
    }

    #[test]
    fn a_reveal_press_that_wanders_off_reports_nothing() {
        let view: SignupFormView<App> = signup_form(SignupValues::default(), |_s: &mut App, _v| {});
        let mut w = build(&view);
        layout(&mut w);
        let mut state = App::default();
        let any_state: &mut dyn Any = &mut state;
        let mut ctx = EventCtx::new(any_state, Point::ZERO, Size::new(w.width, w.height));
        w.event(&mut ctx, &pointer(PointerPhase::Down, w.eye.center()));
        w.event(&mut ctx, &pointer(PointerPhase::Up, Point::new(2.0, 2.0)));
        assert!(!w.reveal);
        assert!(!w.eye_armed);

        // ...and a cancel disarms without flipping either.
        w.event(&mut ctx, &pointer(PointerPhase::Down, w.eye.center()));
        w.event(&mut ctx, &pointer(PointerPhase::Cancel, w.eye.center()));
        assert!(!w.reveal);
        assert!(!w.eye_armed);
    }

    #[test]
    fn a_submitting_form_refuses_the_reveal_toggle() {
        let view: SignupFormView<App> = signup_form(SignupValues::default(), |_s: &mut App, _v| {})
            .status(SignupStatus::Loading);
        let mut w = build(&view);
        layout(&mut w);
        assert!(!w.eye_enabled());
        let mut state = App::default();
        let any_state: &mut dyn Any = &mut state;
        let mut ctx = EventCtx::new(any_state, Point::ZERO, Size::new(w.width, w.height));
        w.event(&mut ctx, &pointer(PointerPhase::Down, w.eye.center()));
        assert!(!w.eye_armed);
    }

    #[test]
    fn a_submit_reports_only_when_every_rule_passes() {
        let mut h = Harness::new(SignupValues::default());
        let submit_y = {
            let mut w = build(&signup_form::<App, _>(
                SignupValues::default(),
                |_s: &mut App, _v| {},
            ));
            layout(&mut w);
            w.submit.origin().y + w.submit.size().height / 2.0
        };
        let at = Point::new(FORM_MAX_WIDTH / 2.0, submit_y);
        h.click(at);
        assert!(
            h.state.submitted.is_empty(),
            "an empty form reports no submission"
        );

        let mut valid = Harness::new(filled());
        let submit_y = {
            let mut w = build(&signup_form::<App, _>(filled(), |_s: &mut App, _v| {}));
            layout(&mut w);
            w.submit.origin().y + w.submit.size().height / 2.0
        };
        valid.click(Point::new(FORM_MAX_WIDTH / 2.0, submit_y));
        assert_eq!(valid.state.submitted.len(), 1);
        assert_eq!(valid.state.submitted[0], filled());
    }

    #[test]
    fn the_status_axis_drives_the_buttons_own_state() {
        assert!(SignupStatus::Loading.is_submitting());
        assert!(!SignupStatus::Idle.is_submitting());
        assert_eq!(SignupStatus::Idle.button_state(), ButtonState::Idle);
        assert_eq!(SignupStatus::Loading.button_state(), ButtonState::Loading);
        assert_eq!(SignupStatus::Success.button_state(), ButtonState::Success);
        assert_eq!(SignupStatus::Error.button_state(), ButtonState::Error);
    }

    // ---- Strength meter layout with presence -------

    #[test]
    fn strength_meter_height_reserved_only_while_visible() {
        // Without any password, the strength meter is not shown and reserves
        // no height.
        let view: SignupFormView<App> = signup_form(SignupValues::default(), |_s: &mut App, _v| {});
        let mut w = build(&view);
        layout(&mut w);
        assert_eq!(
            w.strength_rect.height(),
            0.0,
            "strength meter reserves no height when not shown"
        );
        assert!(!w.strength_was_visible);

        // With a password typed, the meter is shown and reserves height.
        let view: SignupFormView<App> = signup_form(
            SignupValues {
                password: "abcdefgh".to_string(),
                ..SignupValues::default()
            },
            |_s: &mut App, _v| {},
        );
        let mut w = build(&view);
        layout(&mut w);
        assert!(
            w.strength_rect.height() > 0.0,
            "strength meter reserves height when shown"
        );
    }

    #[test]
    fn strength_meter_layout_requests_on_visibility_change() {
        // Typing the first character opens the strength meter. The layout
        // that follows the rebuild must reserve its height at once (the
        // phase, not a progress sample, decides), the entrance keeps asking
        // for layout while it runs, and a settled meter goes quiet — all on
        // paint-only frames.
        let mut h = Harness::new(SignupValues::default());
        h.paint_at(0.0);
        let empty = h.relayout();
        assert!(!h.paint_only(100.0), "an idle form asks for no layout");

        h.state.values.password = "a".to_string();
        h.pass();
        let entering = h.relayout();
        assert!(
            entering.height > empty.height,
            "the strength group takes its height from the first Entering layout"
        );
        assert!(h.paint_only(101.0), "an entering meter asks for layout");
        assert!(h.paint_only(150.0), "so does every frame of the entrance");
        h.relayout();
        // Well past the entrance and the bars' own springs.
        assert!(
            !h.paint_only(2_000.0),
            "a settled meter asks for no more layout"
        );
    }

    #[test]
    fn strength_meter_layout_requests_on_clearing_password() {
        // Clearing the password sends the meter out. It keeps its height
        // while exiting, asks for layout on the frame the exit settles, and
        // the layout that answers releases the height.
        let mut h = Harness::new(SignupValues {
            password: "abcdefgh".to_string(),
            ..SignupValues::default()
        });
        h.paint_at(0.0);
        h.paint_at(500.0);
        let with_meter = h.relayout();
        assert!(!h.paint_only(500.0), "a resting meter asks for no layout");

        h.state.values.password = String::new();
        h.pass();
        assert!(h.paint_only(501.0), "an exiting meter asks for layout");
        assert!(
            h.paint_only(681.0),
            "the frame the exit settles asks for layout"
        );
        let after = h.relayout();
        assert!(
            after.height < with_meter.height,
            "the strength height is released once the exit settles"
        );
        // Well past the exit and the bars' own springs.
        assert!(
            !h.paint_only(2_000.0),
            "an absent meter asks for no more layout"
        );
    }

    #[test]
    fn signup_values_debug_redacts_passwords() {
        let values = SignupValues {
            name: "Ada Lovelace".to_string(),
            email: "ada@example.com".to_string(),
            password: "secret123".to_string(),
            confirm_password: "secret123".to_string(),
            terms: true,
        };

        let debug_str = format!("{:?}", values);

        // The Debug output should contain the public fields.
        assert!(
            debug_str.contains("Ada Lovelace"),
            "Debug should include name"
        );
        assert!(
            debug_str.contains("ada@example.com"),
            "Debug should include email"
        );
        assert!(debug_str.contains("true"), "Debug should include terms");

        // The Debug output should NOT contain the actual passwords.
        assert!(
            !debug_str.contains("secret123"),
            "Debug should not include password plaintext"
        );

        // The Debug output should contain redaction markers.
        assert!(
            debug_str.contains("<redacted>"),
            "Debug should mark passwords as redacted"
        );
    }
}
