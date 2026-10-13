//! `frust-qr`: a scannable QR code view for Frust apps.
//!
//! A facade-tier, platform-free plugin: [`qr_code`] encodes its payload once
//! (via the `qrcode` crate) and paints the modules through the baseline
//! `canvas` widget as one rectangle per run of dark modules.
//!
//! ```no_run
//! use frust::{Component, View};
//! use frust_qr::{EccLevel, qr_code};
//!
//! #[derive(Default)]
//! struct Ticket;
//!
//! impl Component for Ticket {
//!     type State = ();
//!     fn init(&self) {}
//!     fn build(&self, _state: &mut ()) -> impl View<()> {
//!         qr_code("https://example.com/booking/42")
//!             .ecc(EccLevel::Q)
//!             .size(220.0)
//!             .semantic_label("Booking confirmation QR code")
//!     }
//! }
//! ```

mod encode;
mod widget;

pub use encode::{QrError, QrMatrix, dark_runs, encode};
pub use widget::{QrCodeView, QrCodeWidget, qr_code};

/// Error-correction level: how much of the symbol may be damaged and still scan.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum EccLevel {
    /// About 7% recovery.
    L,
    /// About 15% recovery (default).
    #[default]
    M,
    /// About 25% recovery.
    Q,
    /// About 30% recovery.
    H,
}
