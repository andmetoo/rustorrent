use std::net::{IpAddr, Ipv4Addr};
use std::path::Path;

const MAX_GEOIP_BYTES: usize = 256 * 1024 * 1024;
const MAX_GEOIP_ENTRIES: usize = 2_000_000;

pub struct GeoIpDb {
    entries: Vec<(u32, u32, [u8; 2])>,
}

impl GeoIpDb {
    pub fn load(path: &Path) -> Result<Self, String> {
        let data = crate::read_file_limited(path, MAX_GEOIP_BYTES, false)
            .map_err(|err| format!("geoip load: {err}"))?;
        let text =
            std::str::from_utf8(&data).map_err(|_| "geoip load: invalid UTF-8".to_string())?;
        let mut entries = Vec::new();
        for (line_no, raw) in text.lines().enumerate() {
            let line = raw.trim();
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            let line = line.split_once('#').map(|(left, _)| left).unwrap_or(line);
            let line = line.trim();
            if line.is_empty() {
                continue;
            }
            match parse_entry(line) {
                Some(entry) if entries.len() < MAX_GEOIP_ENTRIES => entries.push(entry),
                Some(_) => return Err("geoip load: too many entries".to_string()),
                None => {
                    return Err(format!("geoip line {}: invalid entry", line_no + 1));
                }
            }
        }
        crate::util::sort_by_key(&mut entries, |(start, _, _)| *start);
        for pair in entries.windows(2) {
            if pair[1].0 <= pair[0].1 {
                return Err("geoip load: overlapping address ranges".to_string());
            }
        }
        Ok(GeoIpDb { entries })
    }

    pub fn lookup(&self, addr: IpAddr) -> Option<&str> {
        let ip_u32 = u32::from(match addr {
            IpAddr::V4(ip) => ip,
            IpAddr::V6(ip) => ip.to_ipv4_mapped()?,
        });
        let idx = self
            .entries
            .partition_point(|(start, _, _)| *start <= ip_u32);
        if idx == 0 {
            return None;
        }
        let (_, end, cc) = &self.entries[idx - 1];
        // Country codes are validated as ASCII when loaded.
        (ip_u32 <= *end).then(|| std::str::from_utf8(cc).unwrap_or("??"))
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }
}

/// `start,end,CC` or `a.b.c.d/prefix,<ignored>,CC`.
fn parse_entry(line: &str) -> Option<(u32, u32, [u8; 2])> {
    let mut parts = line.splitn(3, ',');
    let (first, second, cc) = (
        parts.next()?.trim(),
        parts.next()?.trim(),
        parts.next()?.trim(),
    );
    let cc: [u8; 2] = cc.as_bytes().try_into().ok()?;
    if !cc
        .iter()
        .all(|byte| byte.is_ascii_uppercase() || byte.is_ascii_digit())
    {
        return None;
    }
    let (start, end) = if let Some((ip, prefix)) = first.split_once('/') {
        let value = u32::from(ip.parse::<Ipv4Addr>().ok()?);
        let prefix = prefix.parse::<u32>().ok().filter(|prefix| *prefix <= 32)?;
        let host = u32::MAX.checked_shr(prefix).unwrap_or(0);
        (value & !host, value | host)
    } else {
        let start = u32::from(first.parse::<Ipv4Addr>().ok()?);
        let end = u32::from(second.parse::<Ipv4Addr>().ok()?);
        (start <= end).then_some((start, end))?
    };
    Some((start, end, cc))
}

pub fn country_flag(cc: &str) -> String {
    if cc.len() != 2 {
        return String::new();
    }
    let bytes = cc.as_bytes();
    let a = bytes[0].to_ascii_uppercase();
    let b = bytes[1].to_ascii_uppercase();
    if !a.is_ascii_uppercase() || !b.is_ascii_uppercase() {
        return String::new();
    }
    let c1 = char::from_u32(0x1F1E6 + (a - b'A') as u32).unwrap_or('?');
    let c2 = char::from_u32(0x1F1E6 + (b - b'A') as u32).unwrap_or('?');
    format!("{c1}{c2}")
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
        std::env::temp_dir().join(format!("rustorrent-geoip-{name}-{nanos}.csv"))
    }

    #[test]
    fn lookup_returns_country_code() {
        let path = temp_file("basic");
        fs::write(
            &path,
            "# GeoIP test data\n\
             1.0.0.0,1.0.0.255,AU\n\
             8.8.8.0,8.8.8.255,US\n\
             192.168.0.0,192.168.255.255,XX\n",
        )
        .unwrap();
        let db = GeoIpDb::load(&path).unwrap();
        let _ = fs::remove_file(&path);
        assert_eq!(db.lookup("1.0.0.1".parse().unwrap()), Some("AU"));
        assert_eq!(db.lookup("8.8.8.8".parse().unwrap()), Some("US"));
        assert_eq!(db.lookup("192.168.1.1".parse().unwrap()), Some("XX"));
        assert_eq!(db.lookup("10.0.0.1".parse().unwrap()), None);
    }

    #[test]
    fn lookup_supports_cidr_notation() {
        let path = temp_file("cidr");
        fs::write(&path, "10.0.0.0/8,,JP\n").unwrap();
        let db = GeoIpDb::load(&path).unwrap();
        let _ = fs::remove_file(&path);
        assert_eq!(db.lookup("10.1.2.3".parse().unwrap()), Some("JP"));
        assert_eq!(db.lookup("11.0.0.1".parse().unwrap()), None);
    }

    #[test]
    fn lookup_handles_ipv6_mapped_ipv4() {
        let path = temp_file("v6mapped");
        fs::write(&path, "1.0.0.0,1.0.0.255,AU\n").unwrap();
        let db = GeoIpDb::load(&path).unwrap();
        let _ = fs::remove_file(&path);
        let v6: IpAddr = "::ffff:1.0.0.1".parse().unwrap();
        assert_eq!(db.lookup(v6), Some("AU"));
    }

    #[test]
    fn lookup_returns_none_for_pure_ipv6() {
        let path = temp_file("purev6");
        fs::write(&path, "1.0.0.0,1.0.0.255,AU\n").unwrap();
        let db = GeoIpDb::load(&path).unwrap();
        let _ = fs::remove_file(&path);
        let v6: IpAddr = "2001:db8::1".parse().unwrap();
        assert_eq!(db.lookup(v6), None);
    }

    #[test]
    fn parse_entry_handles_cidr_edges_and_rejects_garbage() {
        assert_eq!(parse_entry("0.0.0.0/0,,ZZ"), Some((0, u32::MAX, *b"ZZ")));
        assert_eq!(
            parse_entry("10.1.2.3/32,,JP"),
            Some((0x0a01_0203, 0x0a01_0203, *b"JP"))
        );
        assert_eq!(parse_entry("10.0.0.0/33,,JP"), None);
        assert_eq!(parse_entry("10.0.0.0/8,,JPN"), None);
        assert_eq!(parse_entry("10.0.0.0/8,,é"), None);
        assert_eq!(parse_entry("1.2.3.4,1.2.3.4"), None);
    }

    #[test]
    fn country_flag_generates_unicode_indicators() {
        assert_eq!(country_flag("US"), "\u{1F1FA}\u{1F1F8}");
        assert_eq!(country_flag("JP"), "\u{1F1EF}\u{1F1F5}");
        assert_eq!(country_flag("X"), "");
    }

    #[test]
    fn rejects_invalid_entries() {
        let path = temp_file("invalid");
        fs::write(&path, "not,valid\n").unwrap();
        assert!(GeoIpDb::load(&path).is_err());
        let _ = fs::remove_file(&path);
    }

    #[test]
    fn rejects_reversed_overlapping_and_invalid_country_ranges() {
        for (name, data) in [
            ("reversed", "8.8.8.10,8.8.8.1,US\n"),
            ("overlap", "8.8.8.0,8.8.8.100,US\n8.8.8.50,8.8.8.200,CA\n"),
            ("country", "8.8.8.0,8.8.8.255,u$\n"),
        ] {
            let path = temp_file(name);
            fs::write(&path, data).unwrap();
            assert!(GeoIpDb::load(&path).is_err());
            let _ = fs::remove_file(path);
        }
    }
}
