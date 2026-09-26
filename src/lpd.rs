use std::collections::HashMap;
use std::net::{Ipv4Addr, Ipv6Addr, SocketAddr, UdpSocket};
use std::sync::mpsc;
use std::thread;
use std::time::{Duration, Instant};

const LPD_ADDR_V4: &str = "239.192.152.143:6771";
const LPD_ADDR_V6: &str = "[ff15::efc0:988f]:6771";
const LPD_HOST_V4: &str = LPD_ADDR_V4;
const LPD_HOST_V6: &str = LPD_ADDR_V6;
const MAX_INFOHASHES_PER_MESSAGE: usize = 64;
const LPD_INTERVAL: Duration = Duration::from_secs(30);

#[derive(Clone)]
pub struct Lpd {
    cmd_tx: mpsc::Sender<Command>,
}

enum Command {
    AddTorrent {
        info_hash: [u8; 20],
        port: u16,
        peers_tx: mpsc::Sender<Vec<SocketAddr>>,
    },
    RemoveTorrent {
        info_hash: [u8; 20],
    },
}

struct Entry {
    peers_tx: mpsc::Sender<Vec<SocketAddr>>,
    port: u16,
    last_announce: Option<Instant>,
}

pub fn start() -> Lpd {
    let (cmd_tx, cmd_rx) = mpsc::channel();
    thread::spawn(move || {
        lpd_thread(cmd_rx);
    });
    Lpd { cmd_tx }
}

pub fn disabled() -> Lpd {
    let (cmd_tx, cmd_rx) = mpsc::channel();
    drop(cmd_rx);
    Lpd { cmd_tx }
}

impl Lpd {
    pub fn add_torrent(
        &self,
        info_hash: [u8; 20],
        port: u16,
        peers_tx: mpsc::Sender<Vec<SocketAddr>>,
    ) {
        let _ = self.cmd_tx.send(Command::AddTorrent {
            info_hash,
            port,
            peers_tx,
        });
    }

    pub fn remove_torrent(&self, info_hash: [u8; 20]) {
        let _ = self.cmd_tx.send(Command::RemoveTorrent { info_hash });
    }
}

fn lpd_thread(cmd_rx: mpsc::Receiver<Command>) {
    let socket = match UdpSocket::bind("0.0.0.0:6771") {
        Ok(socket) => socket,
        Err(_) => return,
    };
    let _ = socket.set_read_timeout(Some(Duration::from_millis(200)));
    let _ = socket.join_multicast_v4(
        &Ipv4Addr::new(239, 192, 152, 143),
        &Ipv4Addr::new(0, 0, 0, 0),
    );

    // IPv6 multicast socket (BEP 14)
    let socket6 = UdpSocket::bind("[::]:6771").ok();
    if let Some(ref s6) = socket6 {
        let _ = s6.set_read_timeout(Some(Duration::from_millis(200)));
        let group = Ipv6Addr::new(0xff15, 0, 0, 0, 0, 0, 0xefc0, 0x988f);
        let _ = s6.join_multicast_v6(&group, 0);
    }

    // BEP 14: a random cookie lets us recognise (and ignore) our own
    // multicast announcements, which are looped back to us.
    let cookie = format!("{:016x}", crate::system_entropy_u64());
    let mut entries: HashMap<[u8; 20], Entry> = HashMap::new();
    let mut buf = [0u8; 1500];
    loop {
        loop {
            match cmd_rx.try_recv() {
                Ok(Command::AddTorrent {
                    info_hash,
                    port,
                    peers_tx,
                }) => {
                    entries.insert(
                        info_hash,
                        Entry {
                            peers_tx,
                            port,
                            last_announce: None,
                        },
                    );
                }
                Ok(Command::RemoveTorrent { info_hash }) => {
                    entries.remove(&info_hash);
                }
                Err(mpsc::TryRecvError::Empty) => break,
                Err(mpsc::TryRecvError::Disconnected) => return,
            };
        }

        let now = Instant::now();
        for (info_hash, entry) in entries.iter_mut() {
            if entry
                .last_announce
                .is_none_or(|last| now.duration_since(last) >= LPD_INTERVAL)
            {
                let msg = build_search_message(LPD_HOST_V4, info_hash, entry.port, &cookie);
                let _ = socket.send_to(&msg, LPD_ADDR_V4);
                if let Some(ref s6) = socket6 {
                    let msg = build_search_message(LPD_HOST_V6, info_hash, entry.port, &cookie);
                    let _ = s6.send_to(&msg, LPD_ADDR_V6);
                }
                entry.last_announce = Some(now);
            }
        }

        for socket in std::iter::once(&socket).chain(socket6.as_ref()) {
            let Ok((n, from)) = socket.recv_from(&mut buf) else {
                continue;
            };
            let Some(search) = parse_search_message(&buf[..n]) else {
                continue;
            };
            if search.cookie == Some(cookie.as_str()) {
                continue;
            }
            // Keep the sender's address (including an IPv6 scope) and
            // substitute its announced listening port.
            let mut peer = from;
            peer.set_port(search.port);
            for info_hash in &search.info_hashes {
                if let Some(entry) = entries.get(info_hash) {
                    let _ = entry.peers_tx.send(vec![peer]);
                }
            }
        }
    }
}

fn build_search_message(host: &str, info_hash: &[u8; 20], port: u16, cookie: &str) -> Vec<u8> {
    let mut out = format!("BT-SEARCH * HTTP/1.1\r\nHost: {host}\r\nPort: {port}\r\nInfohash: ");
    for byte in info_hash {
        out.push(char::from(b"0123456789abcdef"[usize::from(byte >> 4)]));
        out.push(char::from(b"0123456789abcdef"[usize::from(byte & 15)]));
    }
    out.push_str("\r\ncookie: ");
    out.push_str(cookie);
    out.push_str("\r\n\r\n\r\n");
    out.into_bytes()
}

struct Search<'a> {
    port: u16,
    info_hashes: Vec<[u8; 20]>,
    cookie: Option<&'a str>,
}

/// Parses a BEP 14 announcement, which may list several infohashes.
fn parse_search_message(data: &[u8]) -> Option<Search<'_>> {
    let text = std::str::from_utf8(data).ok()?;
    if text.contains('\0') {
        return None;
    }
    let mut lines = text.split("\r\n");
    if lines.next()? != "BT-SEARCH * HTTP/1.1" {
        return None;
    }
    let mut info_hashes = Vec::new();
    let mut port = None;
    let mut cookie = None;
    for line in lines {
        let Some((name, value)) = line.split_once(':') else {
            continue;
        };
        let (name, value) = (name.trim(), value.trim());
        if name.eq_ignore_ascii_case("Infohash") {
            if let Some(hash) = decode_hex_20(value) {
                if info_hashes.len() < MAX_INFOHASHES_PER_MESSAGE && !info_hashes.contains(&hash) {
                    info_hashes.push(hash);
                }
            }
        } else if name.eq_ignore_ascii_case("Port") {
            port = value.parse::<u16>().ok().filter(|port| *port != 0);
        } else if name.eq_ignore_ascii_case("cookie") {
            cookie = Some(value);
        }
    }
    if info_hashes.is_empty() {
        return None;
    }
    Some(Search {
        port: port?,
        info_hashes,
        cookie,
    })
}

fn decode_hex_20(value: &str) -> Option<[u8; 20]> {
    let bytes = value.as_bytes();
    if bytes.len() != 40 {
        return None;
    }
    let mut out = [0u8; 20];
    for (idx, chunk) in bytes.as_chunks::<2>().0.iter().enumerate() {
        let hi = (chunk[0] as char).to_digit(16)? as u8;
        let lo = (chunk[1] as char).to_digit(16)? as u8;
        out[idx] = (hi << 4) | lo;
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    const HASH: &str = "0123456789abcdef0123456789abcdef01234567";

    #[test]
    fn search_message_roundtrip() {
        let info_hash = [0xABu8; 20];
        let msg = build_search_message(LPD_HOST_V6, &info_hash, 6881, "c00kie");
        assert!(msg.starts_with(b"BT-SEARCH * HTTP/1.1\r\nHost: [ff15::efc0:988f]:6771\r\n"));
        let parsed = parse_search_message(&msg).unwrap();
        assert_eq!(parsed.info_hashes, vec![info_hash]);
        assert_eq!(parsed.port, 6881);
        assert_eq!(parsed.cookie, Some("c00kie"));
    }

    #[test]
    fn search_message_may_carry_several_infohashes() {
        let msg = format!(
            "BT-SEARCH * HTTP/1.1\r\nHost: 239.192.152.143:6771\r\nPort: 51413\r\n\
             Infohash: {HASH}\r\nInfohash: {}\r\nInfohash: {HASH}\r\n\r\n\r\n",
            "ab".repeat(20)
        );
        let parsed = parse_search_message(msg.as_bytes()).unwrap();
        assert_eq!(parsed.info_hashes.len(), 2);
        assert_eq!(parsed.info_hashes[1], [0xAB; 20]);
        assert_eq!(parsed.cookie, None);
    }

    #[test]
    fn parse_search_message_requires_infohash_and_port() {
        assert!(parse_search_message(b"Port: 6881\r\n\r\n").is_none());
        assert!(parse_search_message(b"Infohash: 0123\r\n\r\n").is_none());
        assert!(parse_search_message(b"Infohash: nothex\r\nPort: 6881\r\n").is_none());
        let wrong_method = format!("HTTP/1.1 200 OK\r\nInfohash: {HASH}\r\nPort: 6881\r\n\r\n");
        assert!(parse_search_message(wrong_method.as_bytes()).is_none());
        let port_zero = format!("BT-SEARCH * HTTP/1.1\r\nInfohash: {HASH}\r\nPort: 0\r\n\r\n");
        assert!(parse_search_message(port_zero.as_bytes()).is_none());
    }

    #[test]
    fn decode_hex_20_accepts_uppercase_and_rejects_bad_len() {
        let parsed = decode_hex_20("0123456789ABCDEF0123456789ABCDEF01234567").unwrap();
        assert_eq!(parsed, decode_hex_20(HASH).unwrap());
        assert!(decode_hex_20("abcd").is_none());
    }
}
