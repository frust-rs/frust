//! Pure `https` map-link builders on top of [`UrlLauncher::open_external`].
//!
//! [`Maps`] builds an `https` URL for the platform's native maps service
//! (Apple Maps on iOS/macOS, Google Maps elsewhere) and hands it to
//! [`UrlLauncher::open_external`], so the `url` module's validator runs on
//! every launch. No platform API is touched here and the validator is not
//! relaxed: every produced URL is `https`, authority-only host, with the
//! query percent-encoded.
//!
//! Coordinates are range-checked (latitude `-90..=90`, longitude
//! `-180..=180`) and formatted with at most six decimals; NaN/infinite or
//! out-of-range values return [`UrlLauncherError::InvalidUrl`]. Free-text
//! queries and labels are percent-encoded: every byte outside RFC 3986
//! `unreserved` (`A-Za-z0-9-._~`) becomes `%XX`, so spaces, `&`, `#` and
//! non-ASCII text cannot break the URL structure.

use crate::{UrlLauncher, UrlLauncherError};

/// Whether this build targets an Apple platform (Apple Maps URLs).
const APPLE: bool = cfg!(any(target_os = "ios", target_os = "macos"));

/// Map-link entry point; carries no state.
pub struct Maps;

impl Maps {
    /// Open turn-by-turn directions to `lat`,`lng` in the default handler.
    ///
    /// # Errors
    /// [`UrlLauncherError::InvalidUrl`] for non-finite or out-of-range
    /// coordinates; otherwise as [`UrlLauncher::open_external`].
    pub fn directions(lat: f64, lng: f64) -> Result<(), UrlLauncherError> {
        UrlLauncher::open_external(&Self::directions_url(lat, lng)?)
    }

    /// Open directions to a free-text address or place `query`.
    ///
    /// # Errors
    /// [`UrlLauncherError::InvalidUrl`] for an empty/blank query; otherwise
    /// as [`UrlLauncher::open_external`].
    pub fn directions_to(query: &str) -> Result<(), UrlLauncherError> {
        UrlLauncher::open_external(&Self::directions_to_url(query)?)
    }

    /// Show `lat`,`lng` on a map, with `label` where the service supports it
    /// (Apple Maps only; Google Maps search URLs take no label).
    ///
    /// # Errors
    /// As [`Self::directions`].
    pub fn show(lat: f64, lng: f64, label: Option<&str>) -> Result<(), UrlLauncherError> {
        UrlLauncher::open_external(&Self::show_url(lat, lng, label)?)
    }

    /// The directions URL for coordinates, without opening it.
    ///
    /// # Errors
    /// [`UrlLauncherError::InvalidUrl`] for non-finite or out-of-range input.
    pub fn directions_url(lat: f64, lng: f64) -> Result<String, UrlLauncherError> {
        let coords = coords(lat, lng)?;
        Ok(if APPLE {
            format!("https://maps.apple.com/?daddr={coords}")
        } else {
            format!("https://www.google.com/maps/dir/?api=1&destination={coords}")
        })
    }

    /// The directions URL for a free-text query, without opening it.
    ///
    /// # Errors
    /// [`UrlLauncherError::InvalidUrl`] for an empty/blank query.
    pub fn directions_to_url(query: &str) -> Result<String, UrlLauncherError> {
        if query.trim().is_empty() {
            return Err(UrlLauncherError::InvalidUrl);
        }
        let q = encode(query);
        Ok(if APPLE {
            format!("https://maps.apple.com/?q={q}")
        } else {
            format!("https://www.google.com/maps/dir/?api=1&destination={q}")
        })
    }

    /// The show-location URL, without opening it.
    ///
    /// # Errors
    /// [`UrlLauncherError::InvalidUrl`] for non-finite or out-of-range input.
    pub fn show_url(lat: f64, lng: f64, label: Option<&str>) -> Result<String, UrlLauncherError> {
        let coords = coords(lat, lng)?;
        Ok(if APPLE {
            match label.filter(|l| !l.trim().is_empty()) {
                Some(label) => format!("https://maps.apple.com/?ll={coords}&q={}", encode(label)),
                None => format!("https://maps.apple.com/?ll={coords}"),
            }
        } else {
            format!("https://www.google.com/maps/search/?api=1&query={coords}")
        })
    }
}

/// Range-check and format `lat,lng` (up to six decimals, trailing zeros trimmed).
fn coords(lat: f64, lng: f64) -> Result<String, UrlLauncherError> {
    if !lat.is_finite() || !lng.is_finite() || lat.abs() > 90.0 || lng.abs() > 180.0 {
        return Err(UrlLauncherError::InvalidUrl);
    }
    Ok(format!("{},{}", fmt_degrees(lat), fmt_degrees(lng)))
}

fn fmt_degrees(value: f64) -> String {
    let text = format!("{value:.6}");
    let text = text.trim_end_matches('0').trim_end_matches('.');
    if text == "-0" || text.is_empty() {
        "0".to_string()
    } else {
        text.to_string()
    }
}

/// Percent-encode every byte outside RFC 3986 `unreserved`.
fn encode(text: &str) -> String {
    const HEX: &[u8; 16] = b"0123456789ABCDEF";
    let mut out = String::with_capacity(text.len());
    for &byte in text.as_bytes() {
        if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'.' | b'_' | b'~') {
            out.push(byte as char);
        } else {
            out.push('%');
            out.push(HEX[usize::from(byte >> 4)] as char);
            out.push(HEX[usize::from(byte & 0x0F)] as char);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::url::validate;

    #[test]
    fn directions_url_exact() {
        let url = Maps::directions_url(51.5007, -0.1246).unwrap();
        if APPLE {
            assert_eq!(url, "https://maps.apple.com/?daddr=51.5007,-0.1246");
        } else {
            assert_eq!(
                url,
                "https://www.google.com/maps/dir/?api=1&destination=51.5007,-0.1246"
            );
        }
    }

    #[test]
    fn directions_to_url_exact_and_encoded() {
        let url = Maps::directions_to_url("1 Main St & 2nd, #5").unwrap();
        let q = "1%20Main%20St%20%26%202nd%2C%20%235";
        if APPLE {
            assert_eq!(url, format!("https://maps.apple.com/?q={q}"));
        } else {
            assert_eq!(
                url,
                format!("https://www.google.com/maps/dir/?api=1&destination={q}")
            );
        }
    }

    #[test]
    fn show_url_exact() {
        let plain = Maps::show_url(10.0, 20.5, None).unwrap();
        let labelled = Maps::show_url(10.0, 20.5, Some("The Café")).unwrap();
        if APPLE {
            assert_eq!(plain, "https://maps.apple.com/?ll=10,20.5");
            assert_eq!(
                labelled,
                "https://maps.apple.com/?ll=10,20.5&q=The%20Caf%C3%A9"
            );
        } else {
            let want = "https://www.google.com/maps/search/?api=1&query=10,20.5";
            assert_eq!(plain, want);
            assert_eq!(labelled, want);
        }
    }

    #[test]
    fn every_produced_url_passes_the_validator() {
        let urls = [
            Maps::directions_url(-33.8688, 151.2093).unwrap(),
            Maps::directions_url(0.0, 0.0).unwrap(),
            Maps::directions_to_url("Zürich HB / Bahnhofplatz?").unwrap(),
            Maps::show_url(48.8584, 2.2945, None).unwrap(),
            Maps::show_url(48.8584, 2.2945, Some("Tour Eiffel & co")).unwrap(),
        ];
        for url in urls {
            assert!(validate(&url).is_ok(), "{url}");
        }
    }

    #[test]
    fn rejects_bad_coordinates_and_queries() {
        for (lat, lng) in [
            (f64::NAN, 0.0),
            (0.0, f64::INFINITY),
            (f64::NEG_INFINITY, 0.0),
            (90.1, 0.0),
            (0.0, -180.1),
        ] {
            assert!(matches!(
                Maps::directions_url(lat, lng),
                Err(UrlLauncherError::InvalidUrl)
            ));
            assert!(matches!(
                Maps::show_url(lat, lng, None),
                Err(UrlLauncherError::InvalidUrl)
            ));
            assert!(matches!(
                Maps::directions(lat, lng),
                Err(UrlLauncherError::InvalidUrl)
            ));
        }
        assert!(matches!(
            Maps::directions_to(" "),
            Err(UrlLauncherError::InvalidUrl)
        ));
    }

    #[test]
    fn coordinate_formatting() {
        assert_eq!(coords(1.123_456_789, -2.0).unwrap(), "1.123457,-2");
        assert_eq!(coords(-0.0, 0.0).unwrap(), "0,0");
        assert_eq!(coords(90.0, -180.0).unwrap(), "90,-180");
    }
}
