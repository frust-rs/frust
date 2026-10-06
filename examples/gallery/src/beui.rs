//! `Beui`-design cases — one per non-index page of the website's
//! `widgets/design-systems/beui/` set, built out of the real `frust-beui`
//! catalog (`plugins/beui`) under [`crate::case::Design::Beui`], which
//! [`crate::theme::theme`] resolves through `frust_beui::theme()`.
//!
//! See the [crate docs](crate) for the registry-wide contract every case here
//! follows (pure `View`, `()` state, no reactive runtime, no wall clock) and
//! [`crate::base`] for the light/dark rule ("the variant has to reach the
//! pixels") this module obeys the same way: [`framed`]/[`framed_in`] size a
//! case's frame without ever filling it, so the recorder's per-variant
//! `surface` clear stays visible around whatever the case paints.
//!
//! # Motion: which frame a preview captures
//!
//! beUI is a *motion* catalog — nearly every component here is a static design
//! plus an animation — so which frame a preview captures matters more than it
//! does for the baseline set.
//!
//! `frust_beui::motion::Presence` (plus every `Ramp`-driven lane built on it)
//! **latches its start time on the first paint that steps it** — deliberately,
//! so a ramp is timed from the frame it first painted rather than from a
//! rebuild several frames earlier. That latch used to be decisive here:
//! `crates/frust-testing/src/frame.rs`'s recorder painted exactly one
//! `rebuild` -> `layout_with_text` -> `paint` pass, so `elapsed` was zero no
//! matter what [`Case::time_ms`] said, an entrance ramp always reported
//! progress 0, and a mid-entrance frame was not expressible at all.
//!
//! It is no longer decisive. The recorder now paints
//! [`Case::warm_frames`] discarded passes before the captured one, and a case
//! opts in simply by pinning a non-zero [`Case::time_ms`] — the warm count is
//! *derived* from that field, so there is no second field to set. The first
//! (discarded) paint latches the ramp at zero and the captured paint sees the
//! whole of `time_ms` as its elapsed, which is exactly the reading a lane
//! needs. Two cases below take it; see *The two cases the warm pass rescues*.
//!
//! Warming buys a case nothing when its component is already at rest on
//! mount, or when what it is missing is a pointer or a live clock rather than
//! elapsed time. Every other case therefore keeps [`Case::DEFAULT_TIME_MS`],
//! and for a reason that is compositional rather than about timing:
//!
//! * `message_bubble`'s `animate_in` is left `false` (its default), which
//!   paints both lanes settled.
//! * `otp_input` stays `OtpStatus::Idle`: the success check and the error shake
//!   are ramps that would record at progress 0.
//! * `code_block` stays `CodeBlockStatus::Complete`: `Streaming` turns a
//!   spinner, which a still frame cannot represent honestly.
//! * `button` uses `ButtonVariant::Base`; `Metallic`/`Magnetic` are a drifting
//!   reflection and a pointer-tracked translation, neither of which exists
//!   without a live clock or a live pointer.
//! * `tilt_card` records flat — its tilt is `PointerTracker`-driven and there
//!   is no pointer in a recorded frame, so flat *is* its rest state.
//!
//! For those, a `time_ms` other than the default would be decoration on this
//! registry rather than information, so none of them sets one.
//!
//! # The two cases the warm pass rescues
//!
//! Both used to record a frame that was honest but nearly empty — exactly what
//! the component paints on its own first frame, never faked, and never a skip
//! the CPU oracle reports. Both now pin a `time_ms` past the end of their own
//! motion, so the poster shows the settled component instead. The timings
//! below are the point each one's slowest lane finishes, not a round number
//! picked for looks.
//!
//! * **`beui/command-palette`, `time_ms: 400`** — was a blank frame: the
//!   palette is a modal whose panel and scrim are both `Presence`-staged, so
//!   both reported progress 0 and the `PNG` was the variant's cleared surface
//!   and nothing else. Its slowest lane is the panel's own
//!   `PALETTE_PANEL_SPRING`, which `Ramp::settle` estimates at ~382ms — well
//!   past `PALETTE_SCRIM_FADE` (180ms) and past the row cascade
//!   (`PALETTE_STAGGER` 18ms a slot into a 160ms `PALETTE_ROW_ENTER`, so
//!   under 250ms for this case's four rows however the group headings are
//!   slotted). 400ms therefore clears all three, and `Ramp::progress` snaps
//!   the spring exactly onto 1.0 rather than leaving it a thousandth short.
//!   The poster now shows the open panel over its scrim.
//! * **`beui/wallet-card`, `time_ms: 1_000`** — was missing its balance
//!   figure: the balance paints as per-grapheme cells on a `Stagger` cascade
//!   whose origin is `WalletCardWidget::privacy_start`, latched the same way,
//!   so every cell sat at reveal 0. (`balance_hidden` does not help — the mask
//!   cells ride the same cascade.) `"$12,480.25"` is ten cells at
//!   `WALLET_ITEM_STAGGER` (35ms) apart over a `SPRING_PANEL` that settles in
//!   ~556ms, so the run ends at ~871ms. 1000ms clears it, and is also an exact
//!   multiple of `WALLET_PULSE_PERIOD` — the unread halo is driven off
//!   absolute frame time and cycles forever rather than settling, so landing
//!   on its period start keeps that dot pixel-identical to the old poster and
//!   leaves the balance as the only thing that changed.
//!
//! # The inverted-chrome ink gap: `dynamic-island`
//!
//! `frust_beui::blocks::dynamic_island`'s shell is `bg-foreground` — *inverted*
//! chrome, near-black under the light scheme and near-white under the dark one
//! — and upstream's `text-background` ink is CSS inheritance a frust child does
//! not get. The catalog documents the same gap for `message_bubble`'s `Solid`
//! variant ("the catalog's bubble publishes no inverted-ink seam"), which is
//! why `frust_beui::agents::chat_app` defaults its user bubble to `Soft`.
//!
//! A case cannot close that gap the way an app does: [`Case::build`] is
//! `fn() -> AnyView<()>` and receives no [`crate::Variant`], so it cannot pick
//! an ink per scheme, and `use_context::<Theme>()` would need the reactive
//! runtime this registry forbids. [`ISLAND_INK`] is the resolution: one fixed
//! mid-neutral that clears roughly 4:1 against *both* shells. Every other case
//! in this module leaves its text at the themed default.

use frust_core::{AnyView, any};
use frust_widgets::{EdgeInsets, Padding, SizedBox, column, container, row, text};
use kurbo::Size;
use peniko::Color;

use frust_beui::agents::chat_app::{ChatMessage, ChatModel, ChatRole, chat_conversation};
use frust_beui::agents::code_block::{CodeSpan, CodeTokenClass, code_block, code_span};
use frust_beui::agents::message_bubble::{
    MessageBubbleAlign, MessageBubbleVariant, message_bubble, message_bubble_group,
};
use frust_beui::agents::prompt_input::prompt_input;
use frust_beui::blocks::command_palette::{command_palette, command_palette_item};
use frust_beui::blocks::dynamic_island::{dynamic_island, dynamic_island_slot};
use frust_beui::blocks::otp_input::otp_input;
use frust_beui::blocks::wallet_card::{wallet_account, wallet_card};
use frust_beui::components::button::{ButtonSize, ButtonTone, button};
use frust_beui::components::checkbox::checkbox;
use frust_beui::components::input::input;
use frust_beui::components::radio::radio;
use frust_beui::components::switch::switch;
use frust_beui::components::tabs::{TabsVariant, tabs, tabs_tab};
use frust_beui::components::tilt_card::tilt_card;

use crate::base::{framed, framed_in};
use crate::case::{Case, Design};

/// The ink every `dynamic-island` slot child paints in — see the module docs'
/// *inverted-chrome ink gap*.
///
/// A fixed mid-neutral, not a token: the island's shell is `foreground`
/// (`#0B0B0B` light, `#F2F2F2` dark) and a case cannot know which it is being
/// recorded against. `#757575` is the neutral whose contrast is closest to
/// balanced across that pair (~4.2:1 against the light shell, ~4.1:1 against
/// the dark one), so the readout stays legible in both previews instead of
/// vanishing into one of them.
const ISLAND_INK: Color = Color::from_rgb8(0x75, 0x75, 0x75);

/// The surface `beui/tilt-card` paints its card in — beUI's `--accent`, taken
/// from the light table.
///
/// `tilt_card` deliberately paints no fill (it tilts, clips and glares a
/// surface the *caller* supplies), and a case cannot resolve a scheme role
/// because [`Case::build`] receives no [`crate::Variant`]. `accent` is the one
/// beUI role whose two tables are near-identical (`#00C5C7` light, `#00DFE1`
/// dark), so pinning the light value is not a lie about either scheme — and it
/// is a swatch smaller than the frame, so the recorder's per-variant `surface`
/// clear still shows around it (see [`crate::base`]'s light/dark rule).
const ACCENT_SURFACE: Color = frust_beui::BEUI_LIGHT.accent;

/// The ink on [`ACCENT_SURFACE`] — beUI's own `--accent-fg`, the ink the design
/// system pairs with `accent` (`#0B0B0B` light, `#151515` dark; the light value
/// reads on both).
const ACCENT_INK: Color = frust_beui::BEUI_LIGHT.accent_fg;

/// A fixed vertical gap, as an `AnyView` a [`Column`] can take.
fn gap_y(height: f64) -> AnyView<()> {
    any(SizedBox(None, Some(height)))
}

/// A fixed horizontal gap, as an `AnyView` a [`Row`] can take.
fn gap_x(width: f64) -> AnyView<()> {
    any(SizedBox(Some(width), None))
}

// ---- components/button -----------------------------------------------------

/// The four [`ButtonTone`]s at rest, in `ButtonVariant::Base`.
///
/// The tones are the page's own axis; the animated variants are excluded for
/// the reason in the module docs (`Magnetic` needs a pointer, `Metallic` a
/// clock, `Stateful` a transition a single frame cannot stage).
fn button_case() -> AnyView<()> {
    framed(
        column()
            .child(
                row()
                    .child(button("Primary", |_: &mut ()| {}).tone(ButtonTone::Primary))
                    .child(gap_x(12.0))
                    .child(button("Secondary", |_: &mut ()| {}).tone(ButtonTone::Secondary)),
            )
            .child(gap_y(14.0))
            .child(
                row()
                    .child(button("Outline", |_: &mut ()| {}).tone(ButtonTone::Outline))
                    .child(gap_x(12.0))
                    .child(button("Ghost", |_: &mut ()| {}).tone(ButtonTone::Ghost)),
            )
            .child(gap_y(14.0))
            .child(
                row()
                    .child(
                        button("Small", |_: &mut ()| {})
                            .tone(ButtonTone::Secondary)
                            .size(ButtonSize::Sm),
                    )
                    .child(gap_x(12.0))
                    .child(
                        button("Large", |_: &mut ()| {})
                            .tone(ButtonTone::Primary)
                            .size(ButtonSize::Lg),
                    ),
            ),
    )
}

// ---- components/input ------------------------------------------------------

/// The labelled field in its three readable states: filled, empty with a
/// placeholder, and invalid with its message row.
fn input_case() -> AnyView<()> {
    framed_in(
        Size::new(360.0, 300.0),
        column()
            .child(
                SizedBox(Some(280.0), None)
                    .child(input("ada@example.com", |_: &mut (), _: String| {}).label("Email")),
            )
            .child(gap_y(14.0))
            .child(
                SizedBox(Some(280.0), None).child(
                    input("", |_: &mut (), _: String| {})
                        .label("Workspace")
                        .placeholder("acme-inc"),
                ),
            )
            .child(gap_y(14.0))
            .child(
                SizedBox(Some(280.0), None).child(
                    input("nope", |_: &mut (), _: String| {})
                        .label("Handle")
                        .error("That handle is taken."),
                ),
            ),
    )
}

// ---- components/form-controls ----------------------------------------------

/// The three selection controls the page groups: `checkbox` (checked and
/// indeterminate), `radio` (a two-item group), and `switch`.
///
/// Each is composed with its own `label`, which is the catalog's accessible
/// name only — the visible caption beside it is a plain themed [`text`], per
/// the components' own module docs.
fn form_controls_case() -> AnyView<()> {
    framed_in(
        Size::new(360.0, 280.0),
        column()
            .child(
                row()
                    .child(checkbox(true, |_: &mut (), _: bool| {}).label("Notifications"))
                    .child(gap_x(10.0))
                    .child(text("Notifications").size(14.0))
                    .child(gap_x(24.0))
                    .child(
                        checkbox(false, |_: &mut (), _: bool| {})
                            .indeterminate(true)
                            .label("Partial"),
                    )
                    .child(gap_x(10.0))
                    .child(text("Partial").size(14.0)),
            )
            .child(gap_y(18.0))
            .child(
                row()
                    .child(radio(true, |_: &mut ()| {}).label("Monthly"))
                    .child(gap_x(10.0))
                    .child(text("Monthly").size(14.0))
                    .child(gap_x(24.0))
                    .child(radio(false, |_: &mut ()| {}).label("Yearly"))
                    .child(gap_x(10.0))
                    .child(text("Yearly").size(14.0)),
            )
            .child(gap_y(18.0))
            .child(
                row()
                    .child(switch(true, |_: &mut (), _: bool| {}).label("Dark mode"))
                    .child(gap_x(10.0))
                    .child(text("Dark mode").size(14.0))
                    .child(gap_x(24.0))
                    .child(
                        switch(false, |_: &mut (), _: bool| {})
                            .disabled(true)
                            .label("Beta features"),
                    )
                    .child(gap_x(10.0))
                    .child(text("Beta features").size(14.0)),
            ),
    )
}

// ---- components/tabs -------------------------------------------------------

/// The pill strip with its active panel — the page's default variant.
fn tabs_case() -> AnyView<()> {
    framed_in(
        Size::new(400.0, 240.0),
        SizedBox(Some(340.0), None).child(
            tabs(
                "overview",
                vec![
                    tabs_tab(
                        "overview",
                        "Overview",
                        column()
                            .child(text("Overview").size(15.0))
                            .child(gap_y(6.0))
                            .child(text("The active panel swaps under the pill.").size(13.0)),
                    ),
                    tabs_tab("activity", "Activity", text("Activity").size(15.0)),
                    tabs_tab("settings", "Settings", text("Settings").size(15.0)),
                ],
                |_: &mut (), _: String| {},
            )
            .variant(TabsVariant::Pill),
        ),
    )
}

// ---- components/tilt-card --------------------------------------------------

/// The tilt card at rest — flat, with its glare and shadow on.
///
/// Rest *is* the honest frame here: the tilt is `PointerTracker`-driven and a
/// recorded frame has no pointer, so there is no engaged angle to freeze.
///
/// The surface is the caller's, not the component's — `tilt_card` paints no
/// fill of its own (its module docs: "a caller supplies that surface
/// explicitly"), so an unfilled child would record as floating text over the
/// glare. It is painted in the beUI [`ACCENT_SURFACE`]/[`ACCENT_INK`] pair
/// rather than a scheme role for the reason those constants document.
fn tilt_card_case() -> AnyView<()> {
    framed_in(
        Size::new(360.0, 280.0),
        SizedBox(Some(248.0), Some(160.0)).child(
            tilt_card(
                container(Padding(
                    EdgeInsets::all(20.0),
                    column()
                        .child(text("Tilt Card").size(18.0).color(ACCENT_INK))
                        .child(gap_y(8.0))
                        .child(
                            text("Leans toward the pointer, with a cursor-tracked glare.")
                                .size(13.0)
                                .color(ACCENT_INK),
                        ),
                ))
                .fill(ACCENT_SURFACE)
                .expand(),
            )
            .glare(true)
            .shadow(true),
        ),
    )
}

// ---- agents/message-bubble -------------------------------------------------

/// A three-turn exchange as a bubble group: the assistant on the leading edge
/// in `Soft`, the person on the trailing edge in `Tint`.
///
/// `Solid` is deliberately absent — it is `bg-foreground text-background`, and
/// the catalog publishes no inverted-ink seam for a bubble's child (see the
/// module docs), so a `Solid` bubble here would paint on-surface ink on an
/// on-surface fill.
fn message_bubble_case() -> AnyView<()> {
    framed_in(
        Size::new(400.0, 260.0),
        SizedBox(Some(320.0), None).child(message_bubble_group(vec![
            any(
                message_bubble(text("How do I freeze a beUI preview?").size(14.0))
                    .variant(MessageBubbleVariant::Tint)
                    .align(MessageBubbleAlign::End),
            ),
            any(message_bubble(
                text("Record it in a state that is already at rest on mount.").size(14.0),
            )
            .variant(MessageBubbleVariant::Soft)
            .align(MessageBubbleAlign::Start)),
            any(message_bubble(text("Got it \u{2014} thanks.").size(14.0))
                .variant(MessageBubbleVariant::Outline)
                .align(MessageBubbleAlign::End)),
        ])),
    )
}

// ---- agents/chat -----------------------------------------------------------

/// The assembled conversation: a settled four-turn transcript plus the composer.
///
/// The [`ChatModel`] is built inside the case (it is a plain value, not a
/// signal) with a finished history and no driver, so nothing is streaming and
/// no clock is owed.
fn chat_case() -> AnyView<()> {
    let model = ChatModel::new().with_history(vec![
        ChatMessage::new("m1", ChatRole::User, "Summarize the release notes."),
        ChatMessage::new(
            "m2",
            ChatRole::Assistant,
            "Three changes landed: the strip pipeline, the beUI port, and the preview registry.",
        ),
        ChatMessage::new("m3", ChatRole::User, "Which one is the biggest?"),
        ChatMessage::new(
            "m4",
            ChatRole::Assistant,
            "The beUI port \u{2014} 81 components across three catalogs.",
        ),
    ]);
    framed_in(
        Size::new(440.0, 396.0),
        SizedBox(Some(376.0), None).child(
            chat_conversation::<()>(&model)
                .placeholder("Ask a follow-up\u{2026}")
                .avatars("You", "AI"),
        ),
    )
}

// ---- agents/code-block -----------------------------------------------------

/// The finished snippet, syntax-coloured through the caller's own
/// [`CodeSpan`] list — nothing here infers syntax.
fn code_block_case() -> AnyView<()> {
    // Each span carries its separating space as a LEADING one. A run's
    // trailing space is dropped by shaping, so `"const "` + `"theme"` would
    // paint as `consttheme`; `"const"` + `" theme"` does not.
    let lines: Vec<Vec<CodeSpan>> = vec![
        vec![
            code_span("const", CodeTokenClass::Keyword),
            code_span(" theme", CodeTokenClass::Plain),
            code_span(" =", CodeTokenClass::Punctuation),
            code_span(" beui", CodeTokenClass::Function),
            code_span("();", CodeTokenClass::Punctuation),
        ],
        vec![code_span(
            "// one frame, already at rest",
            CodeTokenClass::Comment,
        )],
        vec![
            code_span("record", CodeTokenClass::Function),
            code_span("(theme,", CodeTokenClass::Punctuation),
            code_span(" 0", CodeTokenClass::Number),
            code_span(");", CodeTokenClass::Punctuation),
        ],
    ];
    let code = "const theme = beui();\n// one frame, already at rest\nrecord(theme, 0);";
    framed_in(
        Size::new(420.0, 280.0),
        SizedBox(Some(340.0), None).child(
            code_block::<()>(code)
                .colored(lines)
                .language("typescript")
                .filename("preview.ts")
                .line_numbers(true)
                .highlight_lines(vec![3]),
        ),
    )
}

// ---- agents/prompt-input ---------------------------------------------------

/// The composer with a draft in it, so the send affordance reads as enabled.
fn prompt_input_case() -> AnyView<()> {
    framed_in(
        Size::new(400.0, 240.0),
        SizedBox(Some(320.0), None).child(
            prompt_input::<(), _>(
                "Draft a changelog entry for the beUI port",
                |_: &mut (), _: String| {},
            )
            .placeholder("Ask a follow-up\u{2026}")
            .min_rows(2)
            .max_rows(4),
        ),
    )
}

// ---- blocks/otp-input ------------------------------------------------------

/// Six slots, partly filled, in `OtpStatus::Idle`.
///
/// `Success`/`Error` are ramps (the check draw, the row shake) that a
/// single-frame recording would capture at progress 0 — see the module docs.
fn otp_input_case() -> AnyView<()> {
    framed_in(
        Size::new(360.0, 240.0),
        otp_input("428", |_: &mut (), _: String| {})
            .length(6)
            .label("Verification code")
            .hint("Enter the six digits we sent you."),
    )
}

// ---- blocks/dynamic-island -------------------------------------------------

/// The island expanded onto its `music` slot.
///
/// Every child paints in [`ISLAND_INK`] rather than the themed default,
/// because the shell is inverted chrome — see the module docs.
fn dynamic_island_case() -> AnyView<()> {
    let compact = column().child(text("9:41").size(13.0).color(ISLAND_INK));
    let slots = vec![
        dynamic_island_slot(
            "music",
            column()
                .child(text("NOW PLAYING").size(10.0).color(ISLAND_INK))
                .child(gap_y(4.0))
                .child(
                    text("Weightless \u{b7} Marconi Union")
                        .size(13.0)
                        .color(ISLAND_INK),
                ),
        ),
        dynamic_island_slot(
            "call",
            column().child(text("INCOMING CALL").size(10.0).color(ISLAND_INK)),
        ),
    ];
    framed(dynamic_island(Some("music".to_string()), compact, slots).label("Live activity"))
}

// ---- blocks/command-palette ------------------------------------------------

/// The palette, open, over its scrim.
///
/// Recorded at `time_ms: 400`, past every one of its entrance lanes, because
/// the palette is a modal and both its lanes are `Presence`-staged: at rest it
/// records its own progress-0 frame, which is a blank surface. See the module
/// docs' *The two cases the warm pass rescues* for the timing.
///
/// The list is taller than the panel's own `PALETTE_MAX_HEIGHT_FRACTION`
/// allows, so the last row is cut mid-height *by the panel*, not by this
/// case's frame — a command palette clipping its own scrollable list is the
/// component behaving correctly, and the scrim stays visible on all four
/// sides.
fn command_palette_case() -> AnyView<()> {
    let items = vec![
        command_palette_item("New project")
            .group("Actions")
            .hint("\u{2318}N"),
        command_palette_item("Open preview")
            .group("Actions")
            .hint("\u{2318}O"),
        command_palette_item("Toggle theme").group("View"),
        command_palette_item("Go to settings").group("View"),
    ];
    framed_in(
        Size::new(480.0, 360.0),
        command_palette(
            items,
            "",
            |_: &mut (), _: String| {},
            |_: &mut (), _: usize| {},
        )
        .open(true)
        .label("Gallery command palette"),
    )
}

// ---- blocks/wallet-card ----------------------------------------------------

/// The balance card at rest: an account selected, a positive delta, and the
/// action row. No panel is open — the switcher and the search picker share one
/// box and both open on a `Presence`.
///
/// The balance figure rides a per-grapheme `Stagger` cascade whose origin
/// latches on the first paint, so at rest every cell records at reveal 0 and
/// the figure is simply absent. This case pins `time_ms: 1_000` — past the end
/// of that cascade, and on the unread pulse's own period boundary. See the
/// module docs' *The two cases the warm pass rescues*.
fn wallet_card_case() -> AnyView<()> {
    let accounts = vec![
        wallet_account(
            "main",
            "Main Wallet",
            "0x8f3Cb1a29e4D7c6F1B2a3E9d0C4b5A6f7D8e9C0b",
        ),
        wallet_account(
            "trading",
            "Trading",
            "0x1a2B3c4D5e6F7a8B9c0D1e2F3a4B5c6D7e8F9a0B",
        ),
    ];
    framed_in(
        Size::new(400.0, 340.0),
        wallet_card::<()>(accounts, 12_480.25)
            .account_id("main")
            .balance_prefix("$")
            .change(2.4)
            .has_notifications(true),
    )
}

/// This module's slice of the registry [`crate::cases`] concatenates, in the
/// website's own page order.
pub const CASES: &[Case] = &[
    Case {
        slug: "beui/button",
        title: "beUI Button",
        size: Case::DEFAULT_SIZE,
        scale: Case::DEFAULT_SCALE,
        time_ms: Case::DEFAULT_TIME_MS,
        design: Design::Beui,
        build: button_case,
    },
    Case {
        slug: "beui/chat",
        title: "beUI Chat",
        size: Size::new(440.0, 396.0),
        scale: Case::DEFAULT_SCALE,
        time_ms: Case::DEFAULT_TIME_MS,
        design: Design::Beui,
        build: chat_case,
    },
    Case {
        slug: "beui/code-block",
        title: "beUI Code Block",
        size: Size::new(420.0, 280.0),
        scale: Case::DEFAULT_SCALE,
        time_ms: Case::DEFAULT_TIME_MS,
        design: Design::Beui,
        build: code_block_case,
    },
    Case {
        slug: "beui/command-palette",
        title: "beUI Command Palette",
        size: Size::new(480.0, 360.0),
        scale: Case::DEFAULT_SCALE,
        time_ms: 400,
        design: Design::Beui,
        build: command_palette_case,
    },
    Case {
        slug: "beui/dynamic-island",
        title: "beUI Dynamic Island",
        size: Case::DEFAULT_SIZE,
        scale: Case::DEFAULT_SCALE,
        time_ms: Case::DEFAULT_TIME_MS,
        design: Design::Beui,
        build: dynamic_island_case,
    },
    Case {
        slug: "beui/form-controls",
        title: "beUI Form Controls",
        size: Size::new(360.0, 280.0),
        scale: Case::DEFAULT_SCALE,
        time_ms: Case::DEFAULT_TIME_MS,
        design: Design::Beui,
        build: form_controls_case,
    },
    Case {
        slug: "beui/input",
        title: "beUI Input",
        size: Size::new(360.0, 300.0),
        scale: Case::DEFAULT_SCALE,
        time_ms: Case::DEFAULT_TIME_MS,
        design: Design::Beui,
        build: input_case,
    },
    Case {
        slug: "beui/message-bubble",
        title: "beUI Message Bubble",
        size: Size::new(400.0, 260.0),
        scale: Case::DEFAULT_SCALE,
        time_ms: Case::DEFAULT_TIME_MS,
        design: Design::Beui,
        build: message_bubble_case,
    },
    Case {
        slug: "beui/otp-input",
        title: "beUI OTP Input",
        size: Case::DEFAULT_SIZE,
        scale: Case::DEFAULT_SCALE,
        time_ms: Case::DEFAULT_TIME_MS,
        design: Design::Beui,
        build: otp_input_case,
    },
    Case {
        slug: "beui/prompt-input",
        title: "beUI Prompt Input",
        size: Size::new(400.0, 240.0),
        scale: Case::DEFAULT_SCALE,
        time_ms: Case::DEFAULT_TIME_MS,
        design: Design::Beui,
        build: prompt_input_case,
    },
    Case {
        slug: "beui/tabs",
        title: "beUI Tabs",
        size: Size::new(400.0, 240.0),
        scale: Case::DEFAULT_SCALE,
        time_ms: Case::DEFAULT_TIME_MS,
        design: Design::Beui,
        build: tabs_case,
    },
    Case {
        slug: "beui/tilt-card",
        title: "beUI Tilt Card",
        size: Size::new(360.0, 280.0),
        scale: Case::DEFAULT_SCALE,
        time_ms: Case::DEFAULT_TIME_MS,
        design: Design::Beui,
        build: tilt_card_case,
    },
    Case {
        slug: "beui/wallet-card",
        title: "beUI Wallet Card",
        size: Size::new(400.0, 340.0),
        scale: Case::DEFAULT_SCALE,
        time_ms: 1_000,
        design: Design::Beui,
        build: wallet_card_case,
    },
];
