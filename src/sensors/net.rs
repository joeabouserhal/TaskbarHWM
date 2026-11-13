//! Network throughput: bytes per second received and sent, per physical adapter.
//!
//! Read from the interface table's 64-bit octet counters (the same counters Task Manager graphs), so there is no
//! packet capture and no driver. Only hardware interfaces are listed: VPN tunnels, Hyper-V/WSL switches and the
//! like carry traffic that also crosses a physical adapter, and would be counted twice.
use std::collections::HashMap;
use std::ffi::c_void;
use std::time::Instant;

use windows::Win32::NetworkManagement::IpHelper::{FreeMibTable, GetIfTable2, MIB_IF_TABLE2};
use windows::Win32::NetworkManagement::Ndis::IfOperStatusUp;

/// IF_TYPE_SOFTWARE_LOOPBACK
const IF_TYPE_LOOPBACK: u32 = 24;

/// One physical network adapter and its current speeds.
#[derive(Clone, Debug, Default)]
pub struct NetAdapter {
    /// The adapter's name in Windows ("Ethernet", "Wi-Fi"). Unique, so it doubles as the id the user's choice is saved by.
    pub name: String,
    pub connected: bool,
    /// Bytes per second received.
    pub down_bps: f32,
    /// Bytes per second sent.
    pub up_bps: f32,
}

#[derive(Clone, Debug, Default)]
pub struct NetStats {
    /// Connected adapters first, then by name.
    pub adapters: Vec<NetAdapter>,
}

impl NetStats {
    /// The adapters that count: the ones named in `chosen`, or all of them when none of those exist (or none were chosen).
    pub fn included<'a>(&'a self, chosen: &'a [String]) -> impl Iterator<Item = &'a NetAdapter> + 'a {
        let any_chosen = self.adapters.iter().any(|a| chosen.contains(&a.name));
        self.adapters.iter().filter(move |a| !any_chosen || chosen.contains(&a.name))
    }

    /// (download, upload) in bytes per second, summed over the included adapters.
    pub fn total(&self, chosen: &[String]) -> (f32, f32) {
        self.included(chosen).fold((0.0, 0.0), |(d, u), a| (d + a.down_bps, u + a.up_bps))
    }
}

struct Row {
    name: String,
    connected: bool,
    rx: u64,
    tx: u64,
}

/// Counters per interface, keyed by the interface's LUID.
type Counters = HashMap<u64, Row>;

pub struct NetSampler {
    last: (Instant, Counters),
}

fn text(s: &[u16]) -> String {
    let end = s.iter().position(|&c| c == 0).unwrap_or(s.len());
    String::from_utf16_lossy(&s[..end])
}

fn counters() -> Counters {
    let mut out = Counters::new();
    unsafe {
        let mut table: *mut MIB_IF_TABLE2 = std::ptr::null_mut();
        if GetIfTable2(&mut table).0 != 0 || table.is_null() {
            return out;
        }
        let t = &*table;
        let rows = std::slice::from_raw_parts(t.Table.as_ptr(), t.NumEntries as usize);
        for r in rows {
            let hardware = r.InterfaceAndOperStatusFlags._bitfield & 1 != 0;
            if hardware && r.Type != IF_TYPE_LOOPBACK {
                out.insert(
                    r.InterfaceLuid.Value,
                    Row { name: text(&r.Alias), connected: r.OperStatus == IfOperStatusUp, rx: r.InOctets, tx: r.OutOctets },
                );
            }
        }
        FreeMibTable(table as *const c_void);
    }
    out
}

impl NetSampler {
    pub fn new() -> NetSampler {
        NetSampler { last: (Instant::now(), counters()) }
    }

    /// Per-adapter throughput since the previous call.
    pub fn sample(&mut self) -> NetStats {
        let now = Instant::now();
        let cur = counters();
        let dt = now.duration_since(self.last.0).as_secs_f32();
        let mut adapters: Vec<NetAdapter> = cur
            .iter()
            .map(|(luid, r)| {
                // An adapter that just appeared has no earlier reading to compare with: it reads 0 for one tick.
                let (down, up) = match self.last.1.get(luid) {
                    Some(prev) if dt >= 0.05 => (r.rx.saturating_sub(prev.rx) as f32 / dt, r.tx.saturating_sub(prev.tx) as f32 / dt),
                    _ => (0.0, 0.0),
                };
                NetAdapter { name: r.name.clone(), connected: r.connected, down_bps: down, up_bps: up }
            })
            .collect();
        adapters.sort_by(|a, b| b.connected.cmp(&a.connected).then_with(|| a.name.cmp(&b.name)));
        self.last = (now, cur);
        NetStats { adapters }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn adapter(name: &str, down: f32, up: f32) -> NetAdapter {
        NetAdapter { name: name.to_string(), connected: true, down_bps: down, up_bps: up }
    }

    fn stats() -> NetStats {
        NetStats { adapters: vec![adapter("Ethernet", 100.0, 10.0), adapter("Wi-Fi", 50.0, 5.0)] }
    }

    #[test]
    fn no_choice_totals_every_adapter() {
        assert_eq!(stats().total(&[]), (150.0, 15.0));
    }

    #[test]
    fn a_choice_counts_only_those_adapters() {
        assert_eq!(stats().total(&["Wi-Fi".to_string()]), (50.0, 5.0));
    }

    #[test]
    fn a_choice_of_adapters_that_are_gone_falls_back_to_all() {
        assert_eq!(stats().total(&["Old dock".to_string()]), (150.0, 15.0));
    }
}
