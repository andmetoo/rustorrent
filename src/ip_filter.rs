use std::net::{IpAddr, Ipv4Addr};
use std::path::Path;
use std::str::FromStr;

const MAX_BLOCKLIST_BYTES: usize = 128 * 1024 * 1024;
const MAX_BLOCKLIST_RULES: usize = 2_000_000;

#[derive(Default, Clone)]
pub struct IpFilter {
    v4: Vec<(u32, u32)>,
    v6: Vec<(u128, u128)>,
}

impl IpFilter {
    pub fn from_file(path: &Path) -> Result<Self, String> {
        let data = crate::read_file_limited(path, MAX_BLOCKLIST_BYTES, false)
            .map_err(|err| format!("failed to read blocklist: {err}"))?;
        // Descriptions in P2P/DAT lists are frequently Latin-1; addresses are
        // ASCII, so a lossy decode never changes a rule.
        let text = String::from_utf8_lossy(&data);
        let mut filter = Self::default();
        for (line_no, raw) in text.lines().enumerate() {
            let raw = if line_no == 0 {
                raw.strip_prefix('\u{feff}').unwrap_or(raw)
            } else {
                raw
            };
            let line = raw.trim();
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            let uncommented = line.split_once('#').map_or(line, |(left, _)| left.trim());
            if uncommented.is_empty() {
                continue;
            }
            // Strip an inline comment first; a P2P description may itself
            // contain '#', so fall back to the whole line.
            if let Err(err) = filter.add_rule(uncommented).or_else(|err| {
                if uncommented.len() < line.len() {
                    filter.add_rule(line)
                } else {
                    Err(err)
                }
            }) {
                return Err(format!("blocklist line {}: {err}", line_no + 1));
            }
            if filter.v4.len().saturating_add(filter.v6.len()) > MAX_BLOCKLIST_RULES {
                return Err("blocklist contains too many rules".to_string());
            }
        }
        filter.normalize();
        Ok(filter)
    }

    pub fn is_blocked(&self, addr: IpAddr) -> bool {
        match addr {
            IpAddr::V4(ip) => {
                let value = u32::from(ip);
                contains_v4(&self.v4, value)
            }
            IpAddr::V6(ip) => {
                let value = u128::from(ip);
                contains_v6(&self.v6, value)
                    || ip
                        .to_ipv4_mapped()
                        .is_some_and(|mapped| contains_v4(&self.v4, u32::from(mapped)))
            }
        }
    }

    fn normalize(&mut self) {
        crate::util::sort_by_key(&mut self.v4, |range| range.0);
        crate::util::sort_by_key(&mut self.v6, |range| range.0);
        merge_v4_ranges(&mut self.v4);
        merge_v6_ranges(&mut self.v6);
    }

    /// Accepts plain rules (`ip`, `start-end`, `ip/prefix`), eMule/DAT lines
    /// (`start - end , level , description`; levels above 127 allow the
    /// range, as in eMule and qBittorrent) and PeerGuardian P2P lines
    /// (`description:start-end`). DAT files zero-pad IPv4 octets.
    fn add_rule(&mut self, rule: &str) -> Result<(), &'static str> {
        let mut fields = rule.splitn(3, ',');
        let head = fields.next().unwrap_or(rule);
        if let Some(level) = fields
            .next()
            .and_then(|level| level.trim().parse::<u32>().ok())
        {
            return if level > 127 {
                Ok(())
            } else {
                self.add_plain(head)
            };
        }
        let plain = self.add_plain(rule);
        if plain.is_ok() {
            return plain;
        }
        // P2P: the description may itself contain ':' or '-', so split at
        // the last '-' and then at the last ':' before it.
        if let Some((start, end)) = rule
            .rsplit_once('-')
            .and_then(|(head, end)| Some((head.rsplit_once(':')?.1, end)))
        {
            if let (Some(start), Some(end)) = (parse_ipv4(start), parse_ipv4(end)) {
                self.v4.push(ordered(u32::from(start), u32::from(end)));
                return Ok(());
            }
        }
        plain
    }

    fn add_plain(&mut self, rule: &str) -> Result<(), &'static str> {
        let (start, end) = if let Some((start, end)) = rule.split_once('-') {
            let start = parse_ip(start).ok_or("invalid start ip")?;
            let end = parse_ip(end).ok_or("invalid end ip")?;
            (start, end)
        } else if let Some((base, prefix)) = rule.split_once('/') {
            let base = parse_ip(base).ok_or("invalid cidr ip")?;
            let prefix = prefix
                .trim()
                .parse::<u8>()
                .map_err(|_| "invalid cidr prefix")?;
            let bits = if base.is_ipv4() { 32 } else { 128 };
            if prefix > bits {
                return Err("cidr prefix out of range");
            }
            let value = ip_bits(base);
            // Host bits below the prefix, within the family's width.
            let host = (u128::MAX >> (128 - bits))
                .checked_shr(u32::from(prefix))
                .unwrap_or(0);
            let (start, end) = (value & !host, value | host);
            return self.push(base.is_ipv4(), start, end);
        } else {
            let ip = parse_ip(rule).ok_or("invalid ip")?;
            (ip, ip)
        };
        if start.is_ipv4() != end.is_ipv4() {
            return Err("mixed ip versions");
        }
        let v4 = start.is_ipv4();
        let (start, end) = ordered(ip_bits(start), ip_bits(end));
        self.push(v4, start, end)
    }

    fn push(&mut self, v4: bool, start: u128, end: u128) -> Result<(), &'static str> {
        if v4 {
            self.v4.push((start as u32, end as u32));
        } else {
            self.v6.push((start, end));
        }
        Ok(())
    }
}

fn ordered<T: Ord>(a: T, b: T) -> (T, T) {
    if a <= b {
        (a, b)
    } else {
        (b, a)
    }
}

fn ip_bits(ip: IpAddr) -> u128 {
    match ip {
        IpAddr::V4(ip) => u128::from(u32::from(ip)),
        IpAddr::V6(ip) => u128::from(ip),
    }
}

fn parse_ip(text: &str) -> Option<IpAddr> {
    let text = text.trim();
    IpAddr::from_str(text)
        .ok()
        .or_else(|| parse_ipv4(text).map(IpAddr::V4))
}

/// Dotted-quad IPv4 that also accepts zero-padded octets (`001.002.003.004`),
/// which the standard parser rejects.
fn parse_ipv4(text: &str) -> Option<Ipv4Addr> {
    let mut octets = [0u8; 4];
    let mut parts = text.trim().split('.');
    for octet in &mut octets {
        let part = parts.next()?;
        if part.is_empty() || part.len() > 3 || !part.bytes().all(|b| b.is_ascii_digit()) {
            return None;
        }
        *octet = part.parse().ok()?;
    }
    parts.next().is_none().then_some(Ipv4Addr::from(octets))
}

fn contains_v4(ranges: &[(u32, u32)], value: u32) -> bool {
    let index = ranges.partition_point(|(start, _)| *start <= value);
    index > 0 && value <= ranges[index - 1].1
}

fn contains_v6(ranges: &[(u128, u128)], value: u128) -> bool {
    let index = ranges.partition_point(|(start, _)| *start <= value);
    index > 0 && value <= ranges[index - 1].1
}

fn merge_v4_ranges(ranges: &mut Vec<(u32, u32)>) {
    let mut merged: Vec<(u32, u32)> = Vec::with_capacity(ranges.len());
    for (start, end) in ranges.drain(..) {
        if let Some(last) = merged.last_mut() {
            if start <= last.1.saturating_add(1) {
                last.1 = last.1.max(end);
                continue;
            }
        }
        merged.push((start, end));
    }
    *ranges = merged;
}

fn merge_v6_ranges(ranges: &mut Vec<(u128, u128)>) {
    let mut merged: Vec<(u128, u128)> = Vec::with_capacity(ranges.len());
    for (start, end) in ranges.drain(..) {
        if let Some(last) = merged.last_mut() {
            if start <= last.1.saturating_add(1) {
                last.1 = last.1.max(end);
                continue;
            }
        }
        merged.push((start, end));
    }
    *ranges = merged;
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn temp_file(name: &str) -> std::path::PathBuf {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        std::env::temp_dir().join(format!("rustorrent-ipfilter-{name}-{nanos}.txt"))
    }

    #[test]
    fn supports_single_range_and_cidr_rules() {
        let mut filter = IpFilter::default();
        filter.add_rule("10.0.0.1").unwrap();
        filter.add_rule("10.0.0.10 - 10.0.0.4").unwrap();
        filter.add_rule("192.168.1.0/24").unwrap();
        filter.add_rule("2001:db8::/32").unwrap();

        assert!(filter.is_blocked(IpAddr::V4(Ipv4Addr::new(10, 0, 0, 1))));
        assert!(filter.is_blocked(IpAddr::V4(Ipv4Addr::new(10, 0, 0, 7))));
        assert!(filter.is_blocked(IpAddr::V4(Ipv4Addr::new(192, 168, 1, 200))));
        assert!(filter.is_blocked(IpAddr::V6("2001:db8::1".parse().unwrap())));
        assert!(!filter.is_blocked(IpAddr::V4(Ipv4Addr::new(172, 16, 0, 1))));
    }

    #[test]
    fn cidr_prefix_bounds_are_validated() {
        let mut filter = IpFilter::default();
        assert!(filter.add_rule("1.2.3.4/32").is_ok());
        assert!(filter.add_rule("1.2.3.4/33").is_err());
        assert!(filter.add_rule("2001:db8::1/128").is_ok());
        assert!(filter.add_rule("2001:db8::1/129").is_err());
        assert!(filter.add_rule("0.0.0.0/0").is_ok());
        assert!(filter.add_rule("::/0").is_ok());
        assert_eq!(filter.v4, [(0x0102_0304, 0x0102_0304), (0, u32::MAX)]);
        assert_eq!(filter.v6[1], (0, u128::MAX));
        filter.v4.clear();
        filter.add_rule("10.1.2.3/8").unwrap();
        assert_eq!(filter.v4, [(0x0a00_0000, 0x0aff_ffff)]);
        assert!(filter.add_rule("10.0.0.1-2001:db8::1").is_err());
    }

    #[test]
    fn accepts_p2p_and_emule_dat_blocklists() {
        let path = temp_file("formats");
        let mut data = b"\
Some Org, Inc #3 - east:1.2.3.0-1.2.3.255
Bad-Net:010.000.000.001-10.0.0.9
001.002.004.000 - 001.002.004.255 , 000 , eMule entry
009.009.009.000 - 009.009.009.255 , 200 , allowed by level
"
        .to_vec();
        data.extend_from_slice(b"Latin-1 caf\xe9:5.6.7.8-5.6.7.9\n");
        fs::write(&path, data).unwrap();
        let filter = IpFilter::from_file(&path).unwrap();
        let _ = fs::remove_file(&path);
        for blocked in ["1.2.3.77", "10.0.0.5", "1.2.4.9", "5.6.7.9"] {
            assert!(filter.is_blocked(blocked.parse().unwrap()), "{blocked}");
        }
        for allowed in ["1.2.5.1", "9.9.9.9", "10.0.0.10"] {
            assert!(!filter.is_blocked(allowed.parse().unwrap()), "{allowed}");
        }
    }

    #[test]
    fn zero_padded_octets_are_decimal_and_bounded() {
        assert_eq!(
            parse_ipv4("001.020.003.255"),
            Some(Ipv4Addr::new(1, 20, 3, 255))
        );
        assert_eq!(parse_ipv4("1.2.3.256"), None);
        assert_eq!(parse_ipv4("1.2.3"), None);
        assert_eq!(parse_ipv4("1.2.3.4.5"), None);
        assert_eq!(parse_ipv4("0001.2.3.4"), None);
        assert_eq!(parse_ipv4("1.2.+3.4"), None);
    }

    #[test]
    fn from_file_ignores_comments_and_inline_comments() {
        let path = temp_file("comments");
        fs::write(
            &path,
            "\
# top comment
10.0.0.0/8
192.0.2.1 # inline comment

2001:db8::/32
",
        )
        .unwrap();

        let filter = IpFilter::from_file(&path).unwrap();
        let _ = fs::remove_file(&path);
        assert!(filter.is_blocked(IpAddr::V4(Ipv4Addr::new(10, 10, 10, 10))));
        assert!(filter.is_blocked(IpAddr::V4(Ipv4Addr::new(192, 0, 2, 1))));
        assert!(filter.is_blocked(IpAddr::V6("2001:db8::1234".parse().unwrap())));
    }

    #[test]
    fn from_file_reports_line_numbers_for_errors() {
        let path = temp_file("line-number");
        fs::write(&path, "10.0.0.1\nnot-an-ip\n").unwrap();
        let err = match IpFilter::from_file(&path) {
            Ok(_) => panic!("expected parse failure"),
            Err(err) => err,
        };
        let _ = fs::remove_file(&path);
        assert!(err.contains("line 2"));
    }

    #[test]
    fn ipv4_mapped_ipv6_cannot_bypass_ipv4_rules() {
        let mut filter = IpFilter::default();
        filter.add_rule("192.0.2.0/24").unwrap();
        assert!(filter.is_blocked(IpAddr::V6("::ffff:192.0.2.42".parse().unwrap())));
    }

    #[test]
    fn from_file_accepts_utf8_bom() {
        let path = temp_file("bom");
        fs::write(&path, "\u{feff}203.0.113.7\n").unwrap();
        let filter = IpFilter::from_file(&path).unwrap();
        let _ = fs::remove_file(&path);
        assert!(filter.is_blocked("203.0.113.7".parse().unwrap()));
    }
}
