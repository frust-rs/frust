//! Just enough JPEG marker parsing to size a progressive frame's coefficient
//! store from the header, because `image` exposes neither progressiveness nor
//! sampling factors.

/// Start of Image.
const SOI: u8 = 0xD8;
/// End of Image.
const EOI: u8 = 0xD9;
/// Standalone marker with no length field (TEM).
const TEM: u8 = 0x01;

/// One frame component's sampling factors.
struct Component {
    h: u64,
    v: u64,
}

/// A parsed SOFn header.
struct Frame {
    progressive: bool,
    width: u64,
    height: u64,
    components: Vec<Component>,
}

/// Bytes of the whole-image `i16` coefficient store zune-jpeg allocates for a
/// progressive frame, or `Some(0)` for any other frame type. `None` when the
/// header is malformed or truncated, which callers treat as no extra bytes
/// (the decoder itself rejects such data).
///
/// Per component: `2 * 64 * mcu_x * mcu_y * h * v`, with MCU counts rounded
/// up over the largest sampling factors (single-component frames use 8x8
/// MCUs). The per-MCU-row scratch is small and deliberately left out.
pub(crate) fn progressive_coefficient_bytes(bytes: &[u8]) -> Option<u64> {
    let frame = read_frame(bytes)?;
    if !frame.progressive {
        return Some(0);
    }
    coefficient_bytes(&frame)
}

fn coefficient_bytes(frame: &Frame) -> Option<u64> {
    let single = frame.components.len() == 1;
    let h_max = frame.components.iter().map(|c| c.h).max()?;
    let v_max = frame.components.iter().map(|c| c.v).max()?;
    let (mcu_w, mcu_h) = if single {
        (8, 8)
    } else {
        (8 * h_max, 8 * v_max)
    };
    let mcus = frame.width.div_ceil(mcu_w) * frame.height.div_ceil(mcu_h);
    let blocks_per_mcu: u64 = if single {
        1
    } else {
        frame.components.iter().map(|c| c.h * c.v).sum()
    };
    Some(2 * 64 * mcus * blocks_per_mcu)
}

/// Walk markers from SOI to the first SOFn and parse it.
fn read_frame(bytes: &[u8]) -> Option<Frame> {
    if bytes.get(..2)? != [0xFF, SOI] {
        return None;
    }
    let mut pos = 2;
    loop {
        let marker = next_marker(bytes, &mut pos)?;
        if marker == EOI {
            return None;
        }
        if is_standalone(marker) {
            continue;
        }
        let len = usize::from(u16::from_be_bytes([*bytes.get(pos)?, *bytes.get(pos + 1)?]));
        if len < 2 {
            return None;
        }
        let payload = bytes.get(pos + 2..pos + len)?;
        if is_sof(marker) {
            return parse_sof(marker, payload);
        }
        pos += len;
    }
}

/// Advance to the next marker code, skipping fill bytes (repeated 0xFF).
fn next_marker(bytes: &[u8], pos: &mut usize) -> Option<u8> {
    if *bytes.get(*pos)? != 0xFF {
        return None;
    }
    while *bytes.get(*pos)? == 0xFF {
        *pos += 1;
    }
    let marker = *bytes.get(*pos)?;
    *pos += 1;
    Some(marker)
}

fn is_standalone(marker: u8) -> bool {
    marker == TEM || marker == SOI || (0xD0..=0xD7).contains(&marker)
}

/// SOF0..SOF15 except DHT (C4), JPG (C8) and DAC (CC).
fn is_sof(marker: u8) -> bool {
    (0xC0..=0xCF).contains(&marker) && !matches!(marker, 0xC4 | 0xC8 | 0xCC)
}

fn parse_sof(marker: u8, payload: &[u8]) -> Option<Frame> {
    let height = u64::from(u16::from_be_bytes([*payload.get(1)?, *payload.get(2)?]));
    let width = u64::from(u16::from_be_bytes([*payload.get(3)?, *payload.get(4)?]));
    let count = usize::from(*payload.get(5)?);
    if height == 0 || width == 0 || count == 0 {
        return None;
    }
    let mut components = Vec::with_capacity(count);
    for i in 0..count {
        let sampling = *payload.get(6 + i * 3 + 1)?;
        let (h, v) = (u64::from(sampling >> 4), u64::from(sampling & 0x0F));
        if h == 0 || v == 0 {
            return None;
        }
        components.push(Component { h, v });
    }
    Some(Frame {
        progressive: matches!(marker, 0xC2 | 0xC6 | 0xCA | 0xCE),
        width,
        height,
        components,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// SOI, optional extra segments, then a SOF with `components` as
    /// `(h, v)` pairs.
    fn header(sof: u8, w: u16, h: u16, components: &[(u8, u8)], extra: &[u8]) -> Vec<u8> {
        let mut out = vec![0xFF, SOI];
        out.extend_from_slice(extra);
        let len = 8 + 3 * components.len() as u16;
        out.extend_from_slice(&[0xFF, sof]);
        out.extend_from_slice(&len.to_be_bytes());
        out.push(8);
        out.extend_from_slice(&h.to_be_bytes());
        out.extend_from_slice(&w.to_be_bytes());
        out.push(components.len() as u8);
        for (i, (hs, vs)) in components.iter().enumerate() {
            out.extend_from_slice(&[i as u8 + 1, (hs << 4) | vs, 0]);
        }
        out
    }

    fn prog(w: u16, h: u16, components: &[(u8, u8)]) -> Option<u64> {
        progressive_coefficient_bytes(&header(0xC2, w, h, components, &[]))
    }

    #[test]
    fn counts_444() {
        assert_eq!(
            prog(8192, 8192, &[(1, 1), (1, 1), (1, 1)]),
            Some(402_653_184)
        );
    }

    #[test]
    fn counts_420() {
        assert_eq!(
            prog(8192, 8192, &[(2, 2), (1, 1), (1, 1)]),
            Some(201_326_592)
        );
    }

    #[test]
    fn counts_cmyk() {
        assert_eq!(
            prog(8192, 8192, &[(1, 1), (1, 1), (1, 1), (1, 1)]),
            Some(536_870_912)
        );
    }

    #[test]
    fn counts_grayscale() {
        assert_eq!(prog(8192, 8192, &[(1, 1)]), Some(134_217_728));
    }

    #[test]
    fn rounds_odd_sizes_up_to_whole_mcus() {
        assert_eq!(prog(9, 9, &[(2, 2), (1, 1), (1, 1)]), Some(768));
    }

    #[test]
    fn baseline_has_no_store() {
        let sof0 = header(0xC0, 8192, 8192, &[(1, 1), (1, 1), (1, 1)], &[]);
        assert_eq!(progressive_coefficient_bytes(&sof0), Some(0));
    }

    #[test]
    fn skips_app_and_com_segments() {
        let extra = [0xFF, 0xE1, 0x00, 0x04, 1, 2, 0xFF, 0xFE, 0x00, 0x03, 9];
        let bytes = header(0xC2, 8192, 8192, &[(1, 1)], &extra);
        assert_eq!(progressive_coefficient_bytes(&bytes), Some(134_217_728));
    }

    #[test]
    fn truncated_sof_is_none() {
        let mut bytes = header(0xC2, 64, 64, &[(1, 1), (1, 1), (1, 1)], &[]);
        bytes.truncate(bytes.len() - 4);
        assert_eq!(progressive_coefficient_bytes(&bytes), None);
    }

    #[test]
    fn no_sof_before_eoi_is_none() {
        assert_eq!(progressive_coefficient_bytes(&[0xFF, SOI, 0xFF, EOI]), None);
    }

    #[test]
    fn degenerate_frames_are_none() {
        assert_eq!(prog(64, 0, &[(1, 1)]), None);
        assert_eq!(prog(64, 64, &[]), None);
        assert_eq!(prog(64, 64, &[(0, 1)]), None);
    }
}
