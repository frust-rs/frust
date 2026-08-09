//! Pure parsing for `/proc/net/dev`'s per-interface byte counters — shared by
//! [`super::desktop`] (reads the file directly) and [`super::android`] (reads
//! it via `adb shell cat`). Both callers read this table **without any
//! per-process scoping** — see the crate-level [`super`] module doc's
//! namespace-wide/device-wide note; this module only parses, it doesn't
//! claim a scope.

use super::MetricsError;

/// One interface's cumulative counters, as `/proc/net/dev` reports them.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NetIfaceCounts {
    pub iface: String,
    pub rx_bytes: u64,
    pub tx_bytes: u64,
}

/// Parses `/proc/net/dev`'s two-line header followed by one row per
/// interface (`<iface>: <8 receive fields> <8 transmit fields>`), returning
/// every interface found. `rx_bytes`/`tx_bytes` are each row's first field
/// (receive) and 9th field (transmit) — `/proc/net/dev`'s fixed column order
/// is `bytes packets errs drop fifo frame compressed multicast` per
/// direction.
pub fn parse_proc_net_dev(contents: &str) -> Result<Vec<NetIfaceCounts>, MetricsError> {
    let mut out = Vec::new();
    for line in contents.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with("Inter-") || line.starts_with("face") {
            continue;
        }
        let (iface, rest) = line.split_once(':').ok_or_else(|| {
            MetricsError::parse("proc/net/dev", format!("no `:` separator in line `{line}`"))
        })?;
        let fields: Vec<&str> = rest.split_whitespace().collect();
        let rx_bytes = field_u64(&fields, 0, "rx_bytes")?;
        let tx_bytes = field_u64(&fields, 8, "tx_bytes")?;
        out.push(NetIfaceCounts {
            iface: iface.trim().to_string(),
            rx_bytes,
            tx_bytes,
        });
    }
    Ok(out)
}

fn field_u64(fields: &[&str], index: usize, name: &'static str) -> Result<u64, MetricsError> {
    let raw = fields
        .get(index)
        .ok_or_else(|| MetricsError::parse("proc/net/dev", format!("missing {name}")))?;
    raw.parse::<u64>()
        .map_err(|e| MetricsError::parse("proc/net/dev", format!("{name} `{raw}`: {e}")))
}

/// Sums every interface except loopback (`lo`) — the standard convention for
/// "real" network traffic, since `lo` traffic never leaves the host and would
/// otherwise inflate every reading with intra-host chatter.
pub fn aggregate_excluding_loopback(ifaces: &[NetIfaceCounts]) -> (u64, u64) {
    ifaces
        .iter()
        .filter(|i| i.iface != "lo")
        .fold((0u64, 0u64), |(rx, tx), i| {
            (rx + i.rx_bytes, tx + i.tx_bytes)
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Real-shaped `/proc/net/dev` content: header, loopback, one real
    /// interface.
    const NET_DEV: &str = "\
Inter-|   Receive                                                |  Transmit
 face |bytes    packets errs drop fifo frame compressed multicast|bytes    packets errs drop fifo colls carrier compressed
    lo: 1296327    5169    0    0    0     0          0         0  1296327    5169    0    0    0     0       0          0
  eth0: 25200103   34842    0    0    0     0          0         0  4241926   28312    0    0    0     0       0          0
 wlan0:       0       0    0    0    0     0          0         0        0       0    0    0    0     0       0          0
";

    #[test]
    fn parses_every_interface_row() {
        let ifaces = parse_proc_net_dev(NET_DEV).unwrap();
        assert_eq!(
            ifaces,
            vec![
                NetIfaceCounts {
                    iface: "lo".to_string(),
                    rx_bytes: 1296327,
                    tx_bytes: 1296327
                },
                NetIfaceCounts {
                    iface: "eth0".to_string(),
                    rx_bytes: 25200103,
                    tx_bytes: 4241926
                },
                NetIfaceCounts {
                    iface: "wlan0".to_string(),
                    rx_bytes: 0,
                    tx_bytes: 0
                },
            ]
        );
    }

    #[test]
    fn aggregate_excludes_loopback() {
        let ifaces = parse_proc_net_dev(NET_DEV).unwrap();
        let (rx, tx) = aggregate_excluding_loopback(&ifaces);
        assert_eq!(rx, 25200103);
        assert_eq!(tx, 4241926);
    }

    #[test]
    fn malformed_row_without_colon_errs() {
        assert!(parse_proc_net_dev("garbage line with no colon").is_err());
    }

    #[test]
    fn empty_content_yields_no_interfaces() {
        assert_eq!(parse_proc_net_dev("").unwrap(), Vec::new());
    }
}
