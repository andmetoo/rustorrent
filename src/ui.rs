use std::collections::{HashMap, HashSet};
#[cfg(unix)]
use std::fs::File;
use std::io::{Read, Write};
use std::net::{IpAddr, TcpListener, TcpStream};
use std::process::Command;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{mpsc, Arc, Mutex, MutexGuard, OnceLock};
use std::thread;
use std::time::{Duration, Instant};

use crate::is_paused;

#[derive(Debug)]
pub enum UiCommand {
    AddTorrent {
        data: Vec<u8>,
        download_dir: String,
        preallocate: bool,
        options: AddOptions,
        reply: mpsc::Sender<UiCommandResult>,
    },
    AddMagnet {
        magnet: String,
        download_dir: String,
        preallocate: bool,
        options: AddOptions,
        reply: mpsc::Sender<UiCommandResult>,
    },
    PauseTorrent {
        torrent_id: u64,
        reply: mpsc::Sender<UiCommandResult>,
    },
    ResumeTorrent {
        torrent_id: u64,
        reply: mpsc::Sender<UiCommandResult>,
    },
    StopTorrent {
        torrent_id: u64,
        reply: mpsc::Sender<UiCommandResult>,
    },
    ArchiveTorrent {
        torrent_id: u64,
        reply: mpsc::Sender<UiCommandResult>,
    },
    DeleteTorrent {
        torrent_id: u64,
        remove_data: bool,
        reply: mpsc::Sender<UiCommandResult>,
    },
    SetFilePriority {
        torrent_id: u64,
        file_index: usize,
        priority: u8,
        reply: mpsc::Sender<UiCommandResult>,
    },
    SetRateLimits {
        download_limit_bps: u64,
        upload_limit_bps: u64,
        reply: mpsc::Sender<UiCommandResult>,
    },
    RecheckTorrent {
        torrent_id: u64,
        reply: mpsc::Sender<UiCommandResult>,
    },
    SetSeedRatio {
        ratio: f64,
        reply: mpsc::Sender<UiCommandResult>,
    },
    SetPeerProfile {
        profile: String,
        reply: mpsc::Sender<UiCommandResult>,
    },
    SetLabel {
        torrent_id: u64,
        label: String,
        reply: mpsc::Sender<UiCommandResult>,
    },
    AddTracker {
        torrent_id: u64,
        url: String,
        reply: mpsc::Sender<UiCommandResult>,
    },
    RemoveTracker {
        torrent_id: u64,
        url: String,
        reply: mpsc::Sender<UiCommandResult>,
    },
    RenameFile {
        torrent_id: u64,
        file_index: usize,
        new_name: String,
        reply: mpsc::Sender<UiCommandResult>,
    },
    AddRssFeed {
        url: String,
        interval: u64,
        reply: mpsc::Sender<UiCommandResult>,
    },
    RemoveRssFeed {
        url: String,
        reply: mpsc::Sender<UiCommandResult>,
    },
    AddRssRule {
        name: String,
        feed_url: String,
        pattern: String,
        reply: mpsc::Sender<UiCommandResult>,
    },
    RemoveRssRule {
        name: String,
        reply: mpsc::Sender<UiCommandResult>,
    },
}

#[derive(Debug, Clone, Default)]
pub struct AddOptions {
    pub paused: bool,
    pub skip_files: Vec<usize>,
}

fn add_options(form: &[(String, String)]) -> Result<AddOptions, String> {
    let skip = query_value(form, "skip").unwrap_or("");
    let mut skip_files = Vec::new();
    if !skip.is_empty() {
        for value in skip.split(',') {
            if skip_files.len() >= 4096 {
                return Err("too many selected files".to_string());
            }
            skip_files.push(
                value
                    .parse::<usize>()
                    .map_err(|_| "invalid file selection".to_string())?,
            );
        }
        skip_files.sort_unstable();
        skip_files.dedup();
    }
    Ok(AddOptions {
        paused: query_value(form, "paused").is_some_and(parse_bool),
        skip_files,
    })
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UiCommandSuccess {
    Ok,
    TorrentAdded { torrent_id: u64 },
}

pub type UiCommandResult = Result<UiCommandSuccess, String>;

#[derive(Debug, Default, Clone)]
pub struct UiFile {
    pub path: String,
    pub length: u64,
    pub completed: u64,
    pub priority: u8,
}

#[derive(Debug, Default, Clone)]
pub struct UiState {
    pub name: String,
    pub info_hash: String,
    pub download_dir: String,
    pub total_pieces: usize,
    pub completed_pieces: usize,
    pub total_bytes: u64,
    pub completed_bytes: u64,
    pub downloaded_bytes: u64,
    pub uploaded_bytes: u64,
    pub tracker_peers: usize,
    pub active_peers: usize,
    pub interested_peers: usize,
    pub status: String,
    pub last_error: String,
    pub preallocate: bool,
    pub paused: bool,
    pub download_rate_bps: f64,
    pub upload_rate_bps: f64,
    pub upload_requests_served: u64,
    pub eta_secs: u64,
    pub incoming_port: u16,
    pub natpmp_status: String,
    pub upnp_status: String,
    pub files: Vec<UiFile>,
    pub queue_len: usize,
    pub last_added: String,
    pub torrents: Vec<UiTorrent>,
    pub deleted_torrents: HashSet<u64>,
    pub current_id: Option<u64>,
    pub peer_connected: u64,
    pub peer_disconnected: u64,
    pub disk_read_ms_avg: f64,
    pub disk_write_ms_avg: f64,
    pub session_downloaded_bytes: u64,
    pub session_uploaded_bytes: u64,
    pub global_download_limit_bps: u64,
    pub global_upload_limit_bps: u64,
    pub seed_ratio: f64,
    pub peer_profile: String,
    pub peer_profile_global_limit: usize,
    pub peer_profile_torrent_limit: usize,
    pub peer_profile_numwant: u32,
    pub proxy_label: String,
    pub download_history_bps: Vec<f64>,
    pub upload_history_bps: Vec<f64>,
}

#[derive(Debug, Default, Clone)]
pub struct UiTorrent {
    pub id: u64,
    pub name: String,
    pub info_hash: String,
    pub download_dir: String,
    pub preallocate: bool,
    pub status: String,
    pub total_bytes: u64,
    pub completed_bytes: u64,
    pub downloaded_bytes: u64,
    pub uploaded_bytes: u64,
    pub total_pieces: usize,
    pub completed_pieces: usize,
    pub download_rate_bps: f64,
    pub upload_rate_bps: f64,
    pub eta_secs: u64,
    pub tracker_peers: usize,
    pub active_peers: usize,
    pub interested_peers: usize,
    pub paused: bool,
    pub last_error: String,
    pub upload_requests_served: u64,
    pub files: Vec<UiFile>,
    pub label: String,
    pub trackers: Vec<String>,
    pub peer_country_counts: Vec<(String, u32)>,
    pub meta_version: u8,
}

fn lock_state(state: &Arc<Mutex<UiState>>) -> MutexGuard<'_, UiState> {
    match state.lock() {
        Ok(guard) => guard,
        Err(poisoned) => {
            let mut guard = poisoned.into_inner();
            guard.last_error = "ui state lock poisoned; recovered".to_string();
            guard
        }
    }
}

const API_TOKEN_HEADER: &str = "x-rustorrent-token";
const UI_OWNER_SECRET_ENV: &str = "RUSTORRENT_UI_OWNER_SECRET";
const UI_OWNER_SECRET_HEX_LEN: usize = 64;
static UI_API_TOKEN: OnceLock<String> = OnceLock::new();
static UI_OWNER_SECRET: OnceLock<String> = OnceLock::new();
static API_TOKEN_RATE: OnceLock<Mutex<HashMap<IpAddr, (u32, Instant)>>> = OnceLock::new();
static SSE_ACTIVE_CONNECTIONS: AtomicUsize = AtomicUsize::new(0);

const API_TOKEN_RATE_MAX: u32 = 180;
const API_TOKEN_RATE_WINDOW: Duration = Duration::from_secs(60);
const SSE_MAX_ACTIVE_CONNECTIONS: usize = 24;

pub(crate) fn api_token() -> &'static str {
    UI_API_TOKEN.get_or_init(generate_api_token).as_str()
}

fn generate_api_token() -> String {
    let mut bytes = [0u8; 16];
    if let Err(err) = fill_secure_random(&mut bytes) {
        eprintln!("fatal: failed to generate secure API token: {err}");
        std::process::abort();
    }
    hex_bytes(&bytes)
}

fn valid_ui_owner_secret(value: &str) -> bool {
    value.len() == UI_OWNER_SECRET_HEX_LEN && value.bytes().all(|byte| byte.is_ascii_hexdigit())
}

fn read_ui_owner_secret() -> std::io::Result<String> {
    match std::env::var(UI_OWNER_SECRET_ENV) {
        Ok(value) if valid_ui_owner_secret(&value) => Ok(value),
        Ok(_) => Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            format!(
                "{UI_OWNER_SECRET_ENV} must be a {UI_OWNER_SECRET_HEX_LEN}-character hexadecimal secret"
            ),
        )),
        Err(std::env::VarError::NotPresent) => Ok(String::new()),
        Err(std::env::VarError::NotUnicode(_)) => Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            format!("{UI_OWNER_SECRET_ENV} must be valid UTF-8"),
        )),
    }
}

fn configure_ui_owner_secret() -> std::io::Result<()> {
    let secret = read_ui_owner_secret()?;
    if let Some(existing) = UI_OWNER_SECRET.get() {
        if existing == &secret {
            return Ok(());
        }
        return Err(std::io::Error::new(
            std::io::ErrorKind::AlreadyExists,
            "UI owner secret was already configured with a different value",
        ));
    }
    let _ = UI_OWNER_SECRET.set(secret);
    Ok(())
}

fn ui_owner_secret() -> &'static str {
    UI_OWNER_SECRET
        .get_or_init(|| read_ui_owner_secret().unwrap_or_default())
        .as_str()
}

#[cfg(unix)]
fn fill_secure_random(bytes: &mut [u8]) -> std::io::Result<()> {
    if let Ok(mut file) = File::open("/dev/urandom") {
        if file.read_exact(bytes).is_ok() {
            return Ok(());
        }
    }
    Err(std::io::Error::other(
        "operating-system random source unavailable",
    ))
}

#[cfg(windows)]
fn fill_secure_random(bytes: &mut [u8]) -> std::io::Result<()> {
    #[link(name = "bcrypt")]
    extern "system" {
        fn BCryptGenRandom(
            algorithm: *mut std::ffi::c_void,
            buffer: *mut u8,
            length: u32,
            flags: u32,
        ) -> i32;
    }
    const BCRYPT_USE_SYSTEM_PREFERRED_RNG: u32 = 0x0000_0002;
    let length = u32::try_from(bytes.len()).map_err(|_| {
        std::io::Error::new(std::io::ErrorKind::InvalidInput, "random request too large")
    })?;
    // SAFETY: `bytes` is writable for `length` bytes and a null algorithm handle is
    // required when requesting the system-preferred RNG.
    let status = unsafe {
        BCryptGenRandom(
            std::ptr::null_mut(),
            bytes.as_mut_ptr(),
            length,
            BCRYPT_USE_SYSTEM_PREFERRED_RNG,
        )
    };
    if status >= 0 {
        Ok(())
    } else {
        Err(std::io::Error::other(format!(
            "BCryptGenRandom failed with status {status:#x}"
        )))
    }
}

#[cfg(not(any(unix, windows)))]
fn fill_secure_random(_bytes: &mut [u8]) -> std::io::Result<()> {
    Err(std::io::Error::new(
        std::io::ErrorKind::Unsupported,
        "no secure random implementation for this platform",
    ))
}

fn hex_bytes(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        out.push_str(&format!("{byte:02x}"));
    }
    out
}

struct UiConnectionGuard {
    active: Arc<AtomicUsize>,
}

struct SseConnectionGuard;

impl Drop for SseConnectionGuard {
    fn drop(&mut self) {
        SSE_ACTIVE_CONNECTIONS.fetch_sub(1, Ordering::SeqCst);
    }
}

fn try_acquire_sse_connection_slot() -> Option<SseConnectionGuard> {
    loop {
        let current = SSE_ACTIVE_CONNECTIONS.load(Ordering::SeqCst);
        if current >= SSE_MAX_ACTIVE_CONNECTIONS {
            return None;
        }
        if SSE_ACTIVE_CONNECTIONS
            .compare_exchange(current, current + 1, Ordering::SeqCst, Ordering::SeqCst)
            .is_ok()
        {
            return Some(SseConnectionGuard);
        }
    }
}

impl Drop for UiConnectionGuard {
    fn drop(&mut self) {
        self.active.fetch_sub(1, Ordering::SeqCst);
    }
}

fn try_acquire_ui_connection_slot(active: &Arc<AtomicUsize>) -> Option<UiConnectionGuard> {
    loop {
        let current = active.load(Ordering::SeqCst);
        if current >= UI_MAX_ACTIVE_CONNECTIONS {
            return None;
        }
        if active
            .compare_exchange(current, current + 1, Ordering::SeqCst, Ordering::SeqCst)
            .is_ok()
        {
            return Some(UiConnectionGuard {
                active: Arc::clone(active),
            });
        }
    }
}

pub fn start(
    addr: String,
    state: Arc<Mutex<UiState>>,
    cmd_tx: Option<mpsc::Sender<UiCommand>>,
) -> std::io::Result<std::net::SocketAddr> {
    configure_ui_owner_secret()?;
    let listener = TcpListener::bind(&addr)?;
    let local_addr = listener.local_addr()?;
    let active_connections = Arc::new(AtomicUsize::new(0));
    thread::spawn(move || {
        for stream in listener.incoming().flatten() {
            let _ = stream.set_write_timeout(Some(UI_WRITE_TIMEOUT));
            let Some(slot_guard) = try_acquire_ui_connection_slot(&active_connections) else {
                let _ = send_api_error_with_status(stream, 503, "ui busy");
                continue;
            };
            let state = state.clone();
            let cmd_tx = cmd_tx.clone();
            thread::spawn(move || {
                let _slot_guard = slot_guard;
                let _ = handle_connection(stream, state, cmd_tx);
            });
        }
    });
    Ok(local_addr)
}

fn handle_connection(
    mut stream: TcpStream,
    state: Arc<Mutex<UiState>>,
    cmd_tx: Option<mpsc::Sender<UiCommand>>,
) -> std::io::Result<()> {
    stream.set_read_timeout(Some(UI_READ_TIMEOUT))?;
    stream.set_write_timeout(Some(UI_WRITE_TIMEOUT))?;
    let mut pending = match read_request_head(&mut stream) {
        Ok(request) => request,
        Err(_) => {
            return send_api_error(stream, "bad request");
        }
    };
    // The API token is intentionally delivered to the local UI. Restricting
    // Host to localhost or an IP literal prevents a hostile DNS name that has
    // rebound to this listener from reading that token and issuing same-origin
    // mutations through the victim's browser.
    if !request_has_safe_host(&pending.request) {
        return send_api_error_with_status(stream, 403, "forbidden host");
    }
    let (path, query) = split_path_query(&pending.request.path);

    if pending.request.method == "POST" {
        if let Err(err) = authorize_mutating_request(&pending.request) {
            return send_api_error_with_status(stream, 403, &err);
        }
        let Some(body_limit) = post_body_limit(&path) else {
            return send_api_error_with_status(stream, 404, "unknown endpoint");
        };
        if finish_request_body(&mut stream, &mut pending, body_limit).is_err() {
            return send_api_error(stream, "bad request");
        }
    }
    let request = pending.request;

    if request.method == "GET" && path == "/api-token" {
        if let Ok(peer) = stream.peer_addr() {
            if !check_api_token_rate(peer.ip()) {
                return send_api_error_with_status(stream, 429, "too many requests");
            }
        }
        return send_api_token(stream);
    }
    if request.method == "HEAD" && path == "/api-token" {
        return send_head(stream, "application/json");
    }
    if request.method == "GET" && path == "/events" {
        let Some(sse_guard) = try_acquire_sse_connection_slot() else {
            return send_api_error_with_status(stream, 503, "too many event streams");
        };
        let _sse_guard = sse_guard;
        return handle_sse(stream, state);
    }
    if request.method == "HEAD" && path == "/events" {
        return send_head(stream, "text/event-stream");
    }

    if request.method == "POST" {
        if path == "/torrent/open-folder" {
            if let Err(err) = handle_open_folder(&query, &state) {
                return send_api_error(stream, &err);
            }
            return send_api_ok(stream);
        }
        if path == "/torrent/pause"
            || path == "/torrent/resume"
            || path == "/torrent/stop"
            || path == "/torrent/archive"
            || path == "/torrent/delete"
        {
            if let Err(err) = handle_torrent_action(&path, &query, &cmd_tx) {
                update_error(&state, &err);
                return send_api_error(stream, &err);
            }
            return send_api_ok(stream);
        }
        if path == "/add-torrent" {
            let torrent_id = match handle_add_torrent(&request, &query, &state, &cmd_tx) {
                Ok(torrent_id) => torrent_id,
                Err(err) => {
                    update_error(&state, &err);
                    return send_api_error(stream, &err);
                }
            };
            return send_api_ok_with_torrent_id(stream, torrent_id);
        }
        if path == "/add-magnet" {
            let torrent_id = match handle_add_magnet(&request, &state, &cmd_tx) {
                Ok(torrent_id) => torrent_id,
                Err(err) => {
                    update_error(&state, &err);
                    return send_api_error(stream, &err);
                }
            };
            return send_api_ok_with_torrent_id(stream, torrent_id);
        }
        if path == "/select-download-dir" {
            match handle_select_download_dir() {
                Ok(Some(path)) => return send_api_ok_with_path(stream, &path),
                Ok(None) => return send_api_ok_with_path(stream, ""),
                Err(err) => return send_api_error(stream, &err),
            }
        }
        if path == "/file-priority" {
            if let Err(err) = handle_file_priority(&request, &state, &cmd_tx) {
                update_error(&state, &err);
                return send_api_error(stream, &err);
            }
            return send_api_ok(stream);
        }
        if path == "/rename-file" {
            if let Err(err) = handle_rename_file(&request, &state, &cmd_tx) {
                update_error(&state, &err);
                return send_api_error(stream, &err);
            }
            return send_api_ok(stream);
        }
        if path == "/rate-limits" {
            if let Err(err) = handle_rate_limits(&request, &state, &cmd_tx) {
                update_error(&state, &err);
                return send_api_error(stream, &err);
            }
            return send_api_ok(stream);
        }
        if path == "/torrent/recheck" {
            if let Err(err) = handle_torrent_recheck(&query, &cmd_tx) {
                update_error(&state, &err);
                return send_api_error(stream, &err);
            }
            return send_api_ok(stream);
        }
        if path == "/settings/seed-ratio" {
            if let Err(err) = handle_set_seed_ratio(&request, &cmd_tx) {
                update_error(&state, &err);
                return send_api_error(stream, &err);
            }
            return send_api_ok(stream);
        }
        if path == "/settings/peer-profile" {
            if let Err(err) = handle_set_peer_profile(&request, &cmd_tx) {
                update_error(&state, &err);
                return send_api_error(stream, &err);
            }
            return send_api_ok(stream);
        }
        if path == "/torrent/set-label" {
            if let Err(err) = handle_set_label(&request, &state, &cmd_tx) {
                update_error(&state, &err);
                return send_api_error(stream, &err);
            }
            return send_api_ok(stream);
        }
        if path == "/torrent/add-tracker" {
            if let Err(err) = handle_add_tracker(&request, &cmd_tx) {
                update_error(&state, &err);
                return send_api_error(stream, &err);
            }
            return send_api_ok(stream);
        }
        if path == "/torrent/remove-tracker" {
            if let Err(err) = handle_remove_tracker(&request, &cmd_tx) {
                update_error(&state, &err);
                return send_api_error(stream, &err);
            }
            return send_api_ok(stream);
        }
        if path == "/rss/add-feed" {
            if let Err(err) = handle_rss_add_feed(&request, &cmd_tx) {
                update_error(&state, &err);
                return send_api_error(stream, &err);
            }
            return send_api_ok(stream);
        }
        if path == "/rss/remove-feed" {
            if let Err(err) = handle_rss_remove_feed(&request, &cmd_tx) {
                update_error(&state, &err);
                return send_api_error(stream, &err);
            }
            return send_api_ok(stream);
        }
        if path == "/rss/add-rule" {
            if let Err(err) = handle_rss_add_rule(&request, &cmd_tx) {
                update_error(&state, &err);
                return send_api_error(stream, &err);
            }
            return send_api_ok(stream);
        }
        if path == "/rss/remove-rule" {
            if let Err(err) = handle_rss_remove_rule(&request, &cmd_tx) {
                update_error(&state, &err);
                return send_api_error(stream, &err);
            }
            return send_api_ok(stream);
        }
        if path == "/search/install-url" {
            if let Err(err) = handle_search_install_url(&request) {
                update_error(&state, &err);
                return send_api_error(stream, &err);
            }
            return send_api_ok(stream);
        }
        if path == "/search/install-plugin" {
            if let Err(err) = handle_search_install_plugin(&request, &query) {
                update_error(&state, &err);
                return send_api_error(stream, &err);
            }
            return send_api_ok(stream);
        }
        if path == "/search/remove-plugin" {
            if let Err(err) = handle_search_remove_plugin(&request) {
                update_error(&state, &err);
                return send_api_error(stream, &err);
            }
            return send_api_ok(stream);
        }
        if path == "/search/run" {
            if let Err(err) = handle_search_run(&request) {
                update_error(&state, &err);
                return send_api_error(stream, &err);
            }
            return send_api_ok(stream);
        }
        if path == "/search/add-result" {
            let torrent_id = match handle_search_add_result(&request, &state, &cmd_tx) {
                Ok(torrent_id) => torrent_id,
                Err(err) => {
                    update_error(&state, &err);
                    return send_api_error(stream, &err);
                }
            };
            return send_api_ok_with_torrent_id(stream, torrent_id);
        }
        return send_api_error_with_status(stream, 404, "unknown endpoint");
    }

    if request.method == "HEAD" {
        let content_type = match path.as_str() {
            "/" | "/index.html" => "text/html; charset=utf-8",
            "/status" | "/search/status" | "/search/catalog" | "/rss/status" => "application/json",
            _ => return send_api_error_with_status(stream, 404, "unknown endpoint"),
        };
        return send_head(stream, content_type);
    }

    if path == "/search/status" {
        let body = crate::search::status_json();
        return send_json_body(stream, 200, &body);
    }
    if path == "/search/catalog" {
        let refresh = query_value(&query, "refresh")
            .map(parse_bool)
            .unwrap_or(false);
        if refresh && !has_valid_api_token(&request) {
            return send_api_error_with_status(stream, 403, "missing or invalid api token");
        }
        let body = crate::search::catalog_json(refresh);
        return send_json_body(stream, 200, &body);
    }
    if path == "/rss/status" {
        let body = rss_status_json();
        return send_json_body(stream, 200, &body);
    }

    if path != "/status" && path != "/" && path != "/index.html" {
        return send_api_error_with_status(stream, 404, "unknown endpoint");
    }

    let mut guard = lock_state(&state);
    guard.paused = is_paused();
    let (content_type, body) = if path == "/status" {
        ("application/json", status_json(&guard))
    } else {
        ("text/html; charset=utf-8", status_html(&guard))
    };

    let response = format!(
        "HTTP/1.1 200 OK\r\nContent-Type: {content_type}\r\nCache-Control: no-store, no-cache, must-revalidate\r\nPragma: no-cache\r\nExpires: 0\r\n{SECURITY_HEADERS}Content-Length: {}\r\nConnection: close\r\n\r\n",
        body.len()
    );
    stream.write_all(response.as_bytes())?;
    stream.write_all(body.as_bytes())?;
    Ok(())
}

const MAX_REQUEST_BODY_BYTES: usize = crate::MAX_TORRENT_BYTES;
const MAX_FORM_BODY_BYTES: usize = 64 * 1024;
const MAX_PLUGIN_BODY_BYTES: usize = 512 * 1024;
const MAX_HEADER_BYTES: usize = 16 * 1024;
const MAX_CHUNK_LINE_BYTES: usize = 1024;
const MAX_RATE_LIMIT_KBPS: u64 = 102_400;
const MAX_LABEL_BYTES: usize = 128;
const MAX_USER_URL_BYTES: usize = 2_048;
const MAX_RSS_RULE_BYTES: usize = 512;
const COMMAND_WAIT_TIMEOUT: Duration = Duration::from_secs(15);
const UI_MAX_ACTIVE_CONNECTIONS: usize = 64;
const UI_READ_TIMEOUT: Duration = Duration::from_secs(1);
const UI_WRITE_TIMEOUT: Duration = Duration::from_secs(5);
const REQUEST_HEADER_TIMEOUT: Duration = Duration::from_secs(3);
const REQUEST_BODY_TIMEOUT: Duration = Duration::from_secs(15);
const SECURITY_HEADERS: &str = concat!(
    "X-Content-Type-Options: nosniff\r\n",
    "X-Frame-Options: DENY\r\n",
    "Referrer-Policy: no-referrer\r\n",
    "Cross-Origin-Resource-Policy: same-origin\r\n",
    "Permissions-Policy: camera=(), microphone=(), geolocation=()\r\n",
    "Content-Security-Policy: default-src 'self'; base-uri 'none'; object-src 'none'; ",
    "frame-ancestors 'none'; img-src 'self' data:; connect-src 'self'; ",
    "style-src 'self' 'unsafe-inline'; script-src 'self' 'unsafe-inline'\r\n",
);

fn post_body_limit(path: &str) -> Option<usize> {
    match path {
        "/add-torrent" => Some(crate::MAX_TORRENT_BYTES),
        "/search/install-plugin" => Some(MAX_PLUGIN_BODY_BYTES),
        "/add-magnet"
        | "/file-priority"
        | "/rename-file"
        | "/rate-limits"
        | "/settings/seed-ratio"
        | "/settings/peer-profile"
        | "/torrent/set-label"
        | "/torrent/add-tracker"
        | "/torrent/remove-tracker"
        | "/rss/add-feed"
        | "/rss/remove-feed"
        | "/rss/add-rule"
        | "/rss/remove-rule"
        | "/search/install-url"
        | "/search/remove-plugin"
        | "/search/run"
        | "/search/add-result" => Some(MAX_FORM_BODY_BYTES),
        "/torrent/open-folder"
        | "/torrent/pause"
        | "/torrent/resume"
        | "/torrent/stop"
        | "/torrent/archive"
        | "/torrent/delete"
        | "/select-download-dir"
        | "/torrent/recheck" => Some(0),
        _ => None,
    }
}

struct HttpRequest {
    method: String,
    path: String,
    headers: Vec<(String, String)>,
    body: Vec<u8>,
}

#[derive(Clone, Copy)]
enum RequestBodyFraming {
    Fixed(usize),
    Chunked,
}

struct PendingHttpRequest {
    request: HttpRequest,
    buffered_body: Vec<u8>,
    framing: RequestBodyFraming,
}

impl HttpRequest {
    fn header_value(&self, name: &str) -> Option<&str> {
        let name = name.to_ascii_lowercase();
        self.headers
            .iter()
            .find(|(key, _)| key == &name)
            .map(|(_, value)| value.as_str())
    }
}

fn read_request_head(stream: &mut TcpStream) -> std::io::Result<PendingHttpRequest> {
    let mut buffer = Vec::with_capacity(1024);
    let mut header_end = None;
    let header_deadline = Instant::now() + REQUEST_HEADER_TIMEOUT;
    loop {
        if Instant::now() >= header_deadline {
            return Err(std::io::Error::new(
                std::io::ErrorKind::TimedOut,
                "request header timeout",
            ));
        }
        let mut chunk = [0u8; 1024];
        let n = match stream.read(&mut chunk) {
            Ok(n) => n,
            Err(err) if is_retryable_io_error(&err) => continue,
            Err(err) => return Err(err),
        };
        if n == 0 {
            break;
        }
        buffer.extend_from_slice(&chunk[..n]);
        if let Some(pos) = find_header_end(&buffer) {
            if pos + 4 > MAX_HEADER_BYTES {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::InvalidData,
                    "request headers too large",
                ));
            }
            header_end = Some(pos);
            break;
        }
        if buffer.len() > MAX_HEADER_BYTES {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "request headers too large",
            ));
        }
    }

    let header_end = header_end
        .ok_or_else(|| std::io::Error::new(std::io::ErrorKind::InvalidData, "invalid request"))?;
    let header_str = std::str::from_utf8(&buffer[..header_end]).map_err(|_| {
        std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "request headers are not utf-8",
        )
    })?;
    let mut lines = header_str.split("\r\n");
    let request_line = lines
        .next()
        .ok_or_else(|| std::io::Error::new(std::io::ErrorKind::InvalidData, "invalid request"))?;
    let mut parts = request_line.split_whitespace();
    let method = parts
        .next()
        .ok_or_else(|| std::io::Error::new(std::io::ErrorKind::InvalidData, "invalid method"))?
        .to_string();
    let path = parts
        .next()
        .ok_or_else(|| std::io::Error::new(std::io::ErrorKind::InvalidData, "invalid path"))?
        .to_string();
    let version = parts.next().ok_or_else(|| {
        std::io::Error::new(std::io::ErrorKind::InvalidData, "invalid http version")
    })?;
    if parts.next().is_some() || !matches!(version, "HTTP/1.0" | "HTTP/1.1") {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "invalid request line",
        ));
    }
    if !matches!(method.as_str(), "GET" | "HEAD" | "POST") {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "unsupported method",
        ));
    }
    if !path.starts_with('/') {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "invalid path",
        ));
    }

    if path.bytes().any(|byte| byte.is_ascii_control()) {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "invalid path",
        ));
    }

    let mut content_length = None;
    let mut transfer_encoding = None;
    let mut host_seen = false;
    let mut headers = Vec::new();
    for line in lines {
        let (name, value) = line.split_once(':').ok_or_else(|| {
            std::io::Error::new(std::io::ErrorKind::InvalidData, "malformed request header")
        })?;
        if name.is_empty() || !name.bytes().all(is_http_token_byte) {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "invalid request header name",
            ));
        }
        let header_name = name.to_ascii_lowercase();
        let header_value = value.trim_matches([' ', '\t']);
        if header_value
            .bytes()
            .any(|byte| byte.is_ascii_control() && byte != b'\t')
        {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "invalid request header value",
            ));
        }
        if header_name == "content-length" {
            if content_length.is_some() {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::InvalidData,
                    "duplicate content length",
                ));
            }
            content_length = Some(header_value.parse::<usize>().map_err(|_| {
                std::io::Error::new(std::io::ErrorKind::InvalidData, "invalid content length")
            })?);
        }
        if header_name == "transfer-encoding" {
            if transfer_encoding.is_some() {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::InvalidData,
                    "duplicate transfer encoding",
                ));
            }
            transfer_encoding = Some(header_value.to_string());
        }
        if header_name == "host" {
            if host_seen {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::InvalidData,
                    "duplicate host",
                ));
            }
            host_seen = true;
        }
        headers.push((header_name, header_value.to_string()));
    }
    if version == "HTTP/1.1" && !host_seen {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "missing host",
        ));
    }
    if content_length.is_some() && transfer_encoding.is_some() {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "ambiguous request body framing",
        ));
    }
    let chunked_body = match transfer_encoding.as_deref() {
        None => false,
        Some(value) if value.eq_ignore_ascii_case("chunked") => true,
        Some(_) => {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "unsupported transfer encoding",
            ));
        }
    };
    let content_length = content_length.unwrap_or(0);
    if content_length > MAX_REQUEST_BODY_BYTES {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "request body too large",
        ));
    }

    let mut buffered_body = buffer[header_end + 4..].to_vec();
    if !chunked_body && buffered_body.len() > content_length {
        buffered_body.truncate(content_length);
    }
    Ok(PendingHttpRequest {
        request: HttpRequest {
            method,
            path,
            headers,
            body: Vec::new(),
        },
        buffered_body,
        framing: if chunked_body {
            RequestBodyFraming::Chunked
        } else {
            RequestBodyFraming::Fixed(content_length)
        },
    })
}

fn finish_request_body(
    stream: &mut TcpStream,
    pending: &mut PendingHttpRequest,
    limit: usize,
) -> std::io::Result<()> {
    match pending.framing {
        RequestBodyFraming::Chunked => {
            pending.request.body =
                read_chunked_body(stream, std::mem::take(&mut pending.buffered_body), limit)?;
            Ok(())
        }
        RequestBodyFraming::Fixed(content_length) => {
            if content_length > limit {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::InvalidData,
                    "request body too large",
                ));
            }
            let mut body = std::mem::take(&mut pending.buffered_body);
            if body.len() > content_length {
                body.truncate(content_length);
            }
            let body_deadline = Instant::now() + REQUEST_BODY_TIMEOUT;
            while body.len() < content_length {
                if Instant::now() >= body_deadline {
                    return Err(std::io::Error::new(
                        std::io::ErrorKind::TimedOut,
                        "request body timeout",
                    ));
                }
                let mut chunk = [0u8; 1024];
                let n = match stream.read(&mut chunk) {
                    Ok(n) => n,
                    Err(err) if is_retryable_io_error(&err) => continue,
                    Err(err) => return Err(err),
                };
                if n == 0 {
                    break;
                }
                body.extend_from_slice(&chunk[..n]);
                if body.len() > content_length {
                    body.truncate(content_length);
                    break;
                }
            }
            if body.len() < content_length {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::UnexpectedEof,
                    "request body truncated",
                ));
            }

            pending.request.body = body;
            Ok(())
        }
    }
}

fn is_http_token_byte(byte: u8) -> bool {
    byte.is_ascii_alphanumeric()
        || matches!(
            byte,
            b'!' | b'#'
                | b'$'
                | b'%'
                | b'&'
                | b'\''
                | b'*'
                | b'+'
                | b'-'
                | b'.'
                | b'^'
                | b'_'
                | b'`'
                | b'|'
                | b'~'
        )
}

fn read_chunked_body(
    stream: &mut TcpStream,
    mut encoded: Vec<u8>,
    limit: usize,
) -> std::io::Result<Vec<u8>> {
    let mut decoded = Vec::new();
    let mut cursor = 0usize;
    let body_deadline = Instant::now() + REQUEST_BODY_TIMEOUT;

    loop {
        let line_end = loop {
            if let Some(relative) = find_crlf(&encoded[cursor..]) {
                if relative > MAX_CHUNK_LINE_BYTES {
                    return Err(std::io::Error::new(
                        std::io::ErrorKind::InvalidData,
                        "chunk header too large",
                    ));
                }
                break cursor + relative;
            }
            if encoded.len().saturating_sub(cursor) > MAX_CHUNK_LINE_BYTES {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::InvalidData,
                    "chunk header too large",
                ));
            }
            read_more_body(stream, &mut encoded, body_deadline, limit)?;
        };

        let size = parse_chunk_size(&encoded[cursor..line_end])?;
        cursor = line_end + 2;

        if size == 0 {
            loop {
                if encoded.len() >= cursor + 2 && &encoded[cursor..cursor + 2] == b"\r\n" {
                    return Ok(decoded);
                }
                if find_header_end(&encoded[cursor..]).is_some() {
                    return Ok(decoded);
                }
                read_more_body(stream, &mut encoded, body_deadline, limit)?;
            }
        }

        let next_len = decoded.len().checked_add(size).ok_or_else(|| {
            std::io::Error::new(std::io::ErrorKind::InvalidData, "request body too large")
        })?;
        if next_len > limit {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "request body too large",
            ));
        }

        let chunk_end = cursor.checked_add(size).ok_or_else(|| {
            std::io::Error::new(std::io::ErrorKind::InvalidData, "request body too large")
        })?;
        let framed_end = chunk_end.checked_add(2).ok_or_else(|| {
            std::io::Error::new(std::io::ErrorKind::InvalidData, "request body too large")
        })?;
        while encoded.len() < framed_end {
            read_more_body(stream, &mut encoded, body_deadline, limit)?;
        }
        decoded.extend_from_slice(&encoded[cursor..chunk_end]);
        cursor = chunk_end;
        if encoded.get(cursor..cursor + 2) != Some(b"\r\n") {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "invalid chunk terminator",
            ));
        }
        cursor += 2;
    }
}

fn read_more_body(
    stream: &mut TcpStream,
    buffer: &mut Vec<u8>,
    deadline: Instant,
    limit: usize,
) -> std::io::Result<()> {
    if Instant::now() >= deadline {
        return Err(std::io::Error::new(
            std::io::ErrorKind::TimedOut,
            "request body timeout",
        ));
    }
    let mut chunk = [0u8; 1024];
    let n = match stream.read(&mut chunk) {
        Ok(n) => n,
        Err(err) if is_retryable_io_error(&err) => return Ok(()),
        Err(err) => return Err(err),
    };
    if n == 0 {
        return Err(std::io::Error::new(
            std::io::ErrorKind::UnexpectedEof,
            "request body truncated",
        ));
    }
    buffer.extend_from_slice(&chunk[..n]);
    if buffer.len() > MAX_HEADER_BYTES + limit.saturating_mul(2) {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "request body too large",
        ));
    }
    Ok(())
}

fn find_crlf(data: &[u8]) -> Option<usize> {
    data.windows(2).position(|window| window == b"\r\n")
}

fn parse_chunk_size(line: &[u8]) -> std::io::Result<usize> {
    let mut size_part = line.split(|byte| *byte == b';').next().unwrap_or(&[]);
    while size_part.first().is_some_and(u8::is_ascii_whitespace) {
        size_part = &size_part[1..];
    }
    while size_part.last().is_some_and(u8::is_ascii_whitespace) {
        size_part = &size_part[..size_part.len() - 1];
    }
    if size_part.is_empty() {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "invalid chunk size",
        ));
    }
    if !size_part.iter().all(u8::is_ascii_hexdigit) {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "invalid chunk size",
        ));
    }
    let text = std::str::from_utf8(size_part)
        .map_err(|_| std::io::Error::new(std::io::ErrorKind::InvalidData, "invalid chunk size"))?;
    usize::from_str_radix(text, 16)
        .map_err(|_| std::io::Error::new(std::io::ErrorKind::InvalidData, "invalid chunk size"))
}

fn is_retryable_io_error(err: &std::io::Error) -> bool {
    matches!(
        err.kind(),
        std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
    )
}

fn find_header_end(data: &[u8]) -> Option<usize> {
    data.windows(4).position(|window| window == b"\r\n\r\n")
}

fn split_path_query(path: &str) -> (String, Vec<(String, String)>) {
    match path.split_once('?') {
        Some((path, query)) => (path.to_string(), parse_query_pairs(query)),
        None => (path.to_string(), Vec::new()),
    }
}

fn dispatch_command(
    cmd_tx: &Option<mpsc::Sender<UiCommand>>,
    build: impl FnOnce(mpsc::Sender<UiCommandResult>) -> UiCommand,
) -> Result<UiCommandSuccess, String> {
    let Some(tx) = cmd_tx.as_ref() else {
        return Err("ui command channel closed".to_string());
    };
    let (reply_tx, reply_rx) = mpsc::channel::<UiCommandResult>();
    tx.send(build(reply_tx))
        .map_err(|_| "ui command channel closed".to_string())?;
    match reply_rx.recv_timeout(COMMAND_WAIT_TIMEOUT) {
        Ok(result) => result,
        Err(mpsc::RecvTimeoutError::Timeout) => Err("ui command timeout".to_string()),
        Err(mpsc::RecvTimeoutError::Disconnected) => {
            Err("ui command response channel closed".to_string())
        }
    }
}

fn dispatch_command_ok(
    cmd_tx: &Option<mpsc::Sender<UiCommand>>,
    build: impl FnOnce(mpsc::Sender<UiCommandResult>) -> UiCommand,
) -> Result<(), String> {
    let _ = dispatch_command(cmd_tx, build)?;
    Ok(())
}

fn handle_add_torrent(
    request: &HttpRequest,
    query: &[(String, String)],
    state: &Arc<Mutex<UiState>>,
    cmd_tx: &Option<mpsc::Sender<UiCommand>>,
) -> Result<u64, String> {
    if request.body.is_empty() {
        return Err("empty torrent upload".to_string());
    }
    let download_dir = query_value(query, "dir").unwrap_or("").to_string();
    let preallocate = query_value(query, "prealloc")
        .map(parse_bool)
        .unwrap_or(false);

    let options = add_options(query)?;
    let command_result = dispatch_command(cmd_tx, |reply| UiCommand::AddTorrent {
        data: request.body.clone(),
        download_dir,
        preallocate,
        options,
        reply,
    })?;
    let torrent_id = match command_result {
        UiCommandSuccess::TorrentAdded { torrent_id } => torrent_id,
        UiCommandSuccess::Ok => {
            return Err("ui command response missing torrent id".to_string());
        }
    };

    if let Ok(mut guard) = state.lock() {
        guard.last_added = "torrent upload".to_string();
        guard.status = "queued".to_string();
    }
    Ok(torrent_id)
}

fn handle_add_magnet(
    request: &HttpRequest,
    state: &Arc<Mutex<UiState>>,
    cmd_tx: &Option<mpsc::Sender<UiCommand>>,
) -> Result<u64, String> {
    let body_str = String::from_utf8_lossy(&request.body);
    let form = parse_query_pairs(&body_str);
    let magnet = query_value(&form, "magnet").unwrap_or("").to_string();
    if magnet.trim().is_empty() {
        return Err("magnet link is empty".to_string());
    }
    let download_dir = query_value(&form, "dir").unwrap_or("").to_string();
    let preallocate = query_value(&form, "prealloc")
        .map(parse_bool)
        .unwrap_or(false);

    let options = add_options(&form)?;
    let command_result = dispatch_command(cmd_tx, |reply| UiCommand::AddMagnet {
        magnet,
        download_dir,
        preallocate,
        options,
        reply,
    })?;
    let torrent_id = match command_result {
        UiCommandSuccess::TorrentAdded { torrent_id } => torrent_id,
        UiCommandSuccess::Ok => {
            return Err("ui command response missing torrent id".to_string());
        }
    };

    if let Ok(mut guard) = state.lock() {
        guard.last_added = "magnet link".to_string();
        guard.status = "queued".to_string();
    }
    Ok(torrent_id)
}

fn handle_file_priority(
    request: &HttpRequest,
    state: &Arc<Mutex<UiState>>,
    cmd_tx: &Option<mpsc::Sender<UiCommand>>,
) -> Result<(), String> {
    let body_str = String::from_utf8_lossy(&request.body);
    let form = parse_query_pairs(&body_str);
    let index = query_value(&form, "index")
        .ok_or_else(|| "missing index".to_string())?
        .parse::<usize>()
        .map_err(|_| "invalid index".to_string())?;
    let priority = query_value(&form, "priority")
        .ok_or_else(|| "missing priority".to_string())?
        .parse::<u8>()
        .map_err(|_| "invalid priority".to_string())?;
    if priority > 3 {
        return Err("invalid priority (expected 0 through 3)".to_string());
    }
    let torrent_id = query_value(&form, "id")
        .and_then(|value| value.parse::<u64>().ok())
        .ok_or_else(|| "missing torrent id".to_string())?;

    dispatch_command_ok(cmd_tx, |reply| UiCommand::SetFilePriority {
        torrent_id,
        file_index: index,
        priority,
        reply,
    })?;

    let mut guard = lock_state(state);
    if let Some(torrent) = guard
        .torrents
        .iter_mut()
        .find(|torrent| torrent.id == torrent_id)
    {
        if let Some(file) = torrent.files.get_mut(index) {
            file.priority = priority;
        }
    }
    if guard.current_id == Some(torrent_id) {
        if let Some(file) = guard.files.get_mut(index) {
            file.priority = priority;
        }
    }
    Ok(())
}

fn handle_rename_file(
    request: &HttpRequest,
    state: &Arc<Mutex<UiState>>,
    cmd_tx: &Option<mpsc::Sender<UiCommand>>,
) -> Result<(), String> {
    let body_str = String::from_utf8_lossy(&request.body);
    let form = parse_query_pairs(&body_str);
    let index = query_value(&form, "index")
        .ok_or_else(|| "missing index".to_string())?
        .parse::<usize>()
        .map_err(|_| "invalid index".to_string())?;
    let new_name = query_value(&form, "name")
        .ok_or_else(|| "missing name".to_string())?
        .to_string();
    if new_name.is_empty()
        || new_name.len() > 255
        || new_name.contains('/')
        || new_name.contains('\\')
        || new_name.contains('\0')
        || new_name.chars().any(char::is_control)
        || new_name == "."
        || new_name == ".."
    {
        return Err("invalid file name".to_string());
    }
    let torrent_id = query_value(&form, "id")
        .and_then(|value| value.parse::<u64>().ok())
        .ok_or_else(|| "missing torrent id".to_string())?;

    dispatch_command_ok(cmd_tx, |reply| UiCommand::RenameFile {
        torrent_id,
        file_index: index,
        new_name: new_name.clone(),
        reply,
    })?;

    let mut guard = lock_state(state);
    if let Some(torrent) = guard
        .torrents
        .iter_mut()
        .find(|torrent| torrent.id == torrent_id)
    {
        if let Some(file) = torrent.files.get_mut(index) {
            if let Some(pos) = file.path.rfind('/') {
                file.path = format!("{}/{}", &file.path[..pos], new_name);
            } else {
                file.path = new_name.clone();
            }
        }
    }
    if guard.current_id == Some(torrent_id) {
        if let Some(file) = guard.files.get_mut(index) {
            if let Some(pos) = file.path.rfind('/') {
                file.path = format!("{}/{}", &file.path[..pos], new_name);
            } else {
                file.path = new_name;
            }
        }
    }
    Ok(())
}

fn handle_rss_add_feed(
    request: &HttpRequest,
    cmd_tx: &Option<mpsc::Sender<UiCommand>>,
) -> Result<(), String> {
    let body_str = String::from_utf8_lossy(&request.body);
    let form = parse_query_pairs(&body_str);
    let url = query_value(&form, "url")
        .ok_or_else(|| "missing url".to_string())?
        .to_string();
    if url.is_empty() {
        return Err("empty url".to_string());
    }
    if url.len() > MAX_USER_URL_BYTES
        || !(url.starts_with("http://") || url.starts_with("https://"))
    {
        return Err("invalid feed url (expected http:// or https://)".to_string());
    }
    let interval = query_value(&form, "interval")
        .and_then(|v| v.parse::<u64>().ok())
        .unwrap_or(900);
    if !(60..=7 * 24 * 60 * 60).contains(&interval) {
        return Err("invalid feed interval (expected 60 to 604800 seconds)".to_string());
    }
    dispatch_command_ok(cmd_tx, |reply| UiCommand::AddRssFeed {
        url: url.clone(),
        interval,
        reply,
    })
}

fn handle_rss_remove_feed(
    request: &HttpRequest,
    cmd_tx: &Option<mpsc::Sender<UiCommand>>,
) -> Result<(), String> {
    let body_str = String::from_utf8_lossy(&request.body);
    let form = parse_query_pairs(&body_str);
    let url = query_value(&form, "url")
        .ok_or_else(|| "missing url".to_string())?
        .to_string();
    dispatch_command_ok(cmd_tx, |reply| UiCommand::RemoveRssFeed {
        url: url.clone(),
        reply,
    })
}

fn handle_rss_add_rule(
    request: &HttpRequest,
    cmd_tx: &Option<mpsc::Sender<UiCommand>>,
) -> Result<(), String> {
    let body_str = String::from_utf8_lossy(&request.body);
    let form = parse_query_pairs(&body_str);
    let name = query_value(&form, "name")
        .ok_or_else(|| "missing name".to_string())?
        .to_string();
    let feed_url = query_value(&form, "feed_url").unwrap_or("").to_string();
    let pattern = query_value(&form, "pattern")
        .ok_or_else(|| "missing pattern".to_string())?
        .to_string();
    if name.trim().is_empty() || name.len() > 128 {
        return Err("invalid rule name".to_string());
    }
    if pattern.trim().is_empty() || pattern.len() > MAX_RSS_RULE_BYTES {
        return Err("invalid rule pattern".to_string());
    }
    if feed_url.len() > MAX_USER_URL_BYTES {
        return Err("invalid rule feed url".to_string());
    }
    dispatch_command_ok(cmd_tx, |reply| UiCommand::AddRssRule {
        name: name.clone(),
        feed_url: feed_url.clone(),
        pattern: pattern.clone(),
        reply,
    })
}

fn handle_rss_remove_rule(
    request: &HttpRequest,
    cmd_tx: &Option<mpsc::Sender<UiCommand>>,
) -> Result<(), String> {
    let body_str = String::from_utf8_lossy(&request.body);
    let form = parse_query_pairs(&body_str);
    let name = query_value(&form, "name")
        .ok_or_else(|| "missing name".to_string())?
        .to_string();
    dispatch_command_ok(cmd_tx, |reply| UiCommand::RemoveRssRule {
        name: name.clone(),
        reply,
    })
}

fn rss_status_json() -> String {
    use crate::RSS_STATE;

    let lock = match RSS_STATE.get() {
        Some(lock) => lock,
        None => return "{\"feeds\":[],\"rules\":[]}".to_string(),
    };
    let state = match lock.lock() {
        Ok(guard) => guard,
        Err(_) => return "{\"feeds\":[],\"rules\":[]}".to_string(),
    };
    let mut out = String::from("{\"feeds\":[");
    for (i, feed) in state.feeds.iter().enumerate() {
        if i > 0 {
            out.push(',');
        }
        out.push_str(&format!(
            "{{\"url\":\"{}\",\"title\":\"{}\",\"items\":{},\"last_poll\":{},\"interval\":{}}}",
            escape_json(&feed.url),
            escape_json(&feed.title),
            feed.items.len(),
            feed.last_poll,
            feed.poll_interval_secs,
        ));
    }
    out.push_str("],\"rules\":[");
    for (i, rule) in state.rules.iter().enumerate() {
        if i > 0 {
            out.push(',');
        }
        out.push_str(&format!(
            "{{\"name\":\"{}\",\"feed_url\":\"{}\",\"pattern\":\"{}\"}}",
            escape_json(&rule.name),
            escape_json(&rule.feed_url),
            escape_json(&rule.pattern),
        ));
    }
    out.push_str("]}");
    out
}

fn handle_search_install_url(request: &HttpRequest) -> Result<(), String> {
    let body_str = String::from_utf8_lossy(&request.body);
    let form = parse_query_pairs(&body_str);
    let url = query_value(&form, "url")
        .ok_or_else(|| "missing url".to_string())?
        .trim()
        .to_string();
    if url.is_empty() {
        return Err("empty url".to_string());
    }
    let _ = crate::search::install_plugin_from_url(&url)?;
    Ok(())
}

fn handle_search_install_plugin(
    request: &HttpRequest,
    query: &[(String, String)],
) -> Result<(), String> {
    if request.body.is_empty() {
        return Err("empty plugin upload".to_string());
    }
    let filename = query_value(query, "filename")
        .ok_or_else(|| "missing filename".to_string())?
        .to_string();
    let _ = crate::search::install_plugin_from_bytes(&filename, &request.body)?;
    Ok(())
}

fn handle_search_remove_plugin(request: &HttpRequest) -> Result<(), String> {
    let body_str = String::from_utf8_lossy(&request.body);
    let form = parse_query_pairs(&body_str);
    let module = query_value(&form, "module")
        .ok_or_else(|| "missing module".to_string())?
        .trim()
        .to_string();
    if module.is_empty() {
        return Err("empty module".to_string());
    }
    crate::search::remove_plugin(&module)
}

fn handle_search_run(request: &HttpRequest) -> Result<(), String> {
    let body_str = String::from_utf8_lossy(&request.body);
    let form = parse_query_pairs(&body_str);
    let query = query_value(&form, "query")
        .ok_or_else(|| "missing query".to_string())?
        .trim()
        .to_string();
    let category = query_value(&form, "category").unwrap_or("all").to_string();
    let engines = query_value(&form, "engines")
        .unwrap_or("")
        .split(',')
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
        .collect::<Vec<_>>();
    crate::search::start_search(&query, &category, &engines)
}

fn handle_search_add_result(
    request: &HttpRequest,
    state: &Arc<Mutex<UiState>>,
    cmd_tx: &Option<mpsc::Sender<UiCommand>>,
) -> Result<u64, String> {
    let body_str = String::from_utf8_lossy(&request.body);
    let form = parse_query_pairs(&body_str);
    let index = query_value(&form, "index")
        .ok_or_else(|| "missing index".to_string())?
        .parse::<u64>()
        .map_err(|_| "invalid index".to_string())?;
    let download_dir = query_value(&form, "dir").unwrap_or("").to_string();
    let preallocate = query_value(&form, "prealloc")
        .map(parse_bool)
        .unwrap_or(false);

    let result = crate::search::resolve_result(index)?;
    let command_result = match result {
        crate::search::SearchDownload::Magnet(magnet) => {
            dispatch_command(cmd_tx, |reply| UiCommand::AddMagnet {
                magnet,
                download_dir,
                preallocate,
                options: AddOptions::default(),
                reply,
            })?
        }
        crate::search::SearchDownload::TorrentBytes(data) => {
            dispatch_command(cmd_tx, |reply| UiCommand::AddTorrent {
                data,
                download_dir,
                preallocate,
                options: AddOptions::default(),
                reply,
            })?
        }
    };
    let torrent_id = match command_result {
        UiCommandSuccess::TorrentAdded { torrent_id } => torrent_id,
        UiCommandSuccess::Ok => {
            return Err("ui command response missing torrent id".to_string());
        }
    };

    if let Ok(mut guard) = state.lock() {
        guard.last_added = "search result".to_string();
        guard.status = "queued".to_string();
    }
    Ok(torrent_id)
}

fn handle_rate_limits(
    request: &HttpRequest,
    state: &Arc<Mutex<UiState>>,
    cmd_tx: &Option<mpsc::Sender<UiCommand>>,
) -> Result<(), String> {
    let body_str = String::from_utf8_lossy(&request.body);
    let form = parse_query_pairs(&body_str);
    let download_kbps = query_value(&form, "download_kbps")
        .ok_or_else(|| "missing download_kbps".to_string())?
        .parse::<u64>()
        .map_err(|_| "invalid download_kbps".to_string())?;
    let upload_kbps = query_value(&form, "upload_kbps")
        .ok_or_else(|| "missing upload_kbps".to_string())?
        .parse::<u64>()
        .map_err(|_| "invalid upload_kbps".to_string())?;
    if download_kbps > MAX_RATE_LIMIT_KBPS || upload_kbps > MAX_RATE_LIMIT_KBPS {
        return Err(format!(
            "rate limit exceeds maximum of {MAX_RATE_LIMIT_KBPS} KiB/s"
        ));
    }
    let download_limit_bps = download_kbps.saturating_mul(1024);
    let upload_limit_bps = upload_kbps.saturating_mul(1024);

    dispatch_command_ok(cmd_tx, |reply| UiCommand::SetRateLimits {
        download_limit_bps,
        upload_limit_bps,
        reply,
    })?;

    let mut guard = lock_state(state);
    guard.global_download_limit_bps = download_limit_bps;
    guard.global_upload_limit_bps = upload_limit_bps;
    Ok(())
}

fn handle_torrent_recheck(
    query: &[(String, String)],
    cmd_tx: &Option<mpsc::Sender<UiCommand>>,
) -> Result<(), String> {
    let torrent_id = query_value(query, "id")
        .and_then(|value| value.parse::<u64>().ok())
        .ok_or_else(|| "missing torrent id".to_string())?;
    dispatch_command_ok(cmd_tx, |reply| UiCommand::RecheckTorrent {
        torrent_id,
        reply,
    })
}

fn handle_set_seed_ratio(
    request: &HttpRequest,
    cmd_tx: &Option<mpsc::Sender<UiCommand>>,
) -> Result<(), String> {
    let body_str = String::from_utf8_lossy(&request.body);
    let form = parse_query_pairs(&body_str);
    let ratio = query_value(&form, "ratio")
        .ok_or_else(|| "missing ratio".to_string())?
        .parse::<f64>()
        .map_err(|_| "invalid ratio".to_string())?;
    if !ratio.is_finite() || !(0.0..=10.0).contains(&ratio) {
        return Err("ratio must be finite and between 0 and 10".to_string());
    }
    dispatch_command_ok(cmd_tx, |reply| UiCommand::SetSeedRatio { ratio, reply })
}

fn handle_set_peer_profile(
    request: &HttpRequest,
    cmd_tx: &Option<mpsc::Sender<UiCommand>>,
) -> Result<(), String> {
    let body_str = String::from_utf8_lossy(&request.body);
    let form = parse_query_pairs(&body_str);
    let profile = query_value(&form, "profile")
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| "missing profile".to_string())?
        .to_string();
    if !matches!(profile.as_str(), "conservative" | "balanced" | "aggressive") {
        return Err("invalid peer profile".to_string());
    }
    dispatch_command_ok(cmd_tx, |reply| UiCommand::SetPeerProfile { profile, reply })
}

fn handle_set_label(
    request: &HttpRequest,
    state: &Arc<Mutex<UiState>>,
    cmd_tx: &Option<mpsc::Sender<UiCommand>>,
) -> Result<(), String> {
    let body_str = String::from_utf8_lossy(&request.body);
    let form = parse_query_pairs(&body_str);
    let torrent_id = query_value(&form, "id")
        .and_then(|v| v.parse::<u64>().ok())
        .ok_or_else(|| "missing torrent id".to_string())?;
    let label = query_value(&form, "label").unwrap_or("").to_string();
    if label.len() > MAX_LABEL_BYTES || label.chars().any(char::is_control) {
        return Err("invalid label".to_string());
    }

    dispatch_command_ok(cmd_tx, |reply| UiCommand::SetLabel {
        torrent_id,
        label: label.clone(),
        reply,
    })?;

    let mut guard = lock_state(state);
    if let Some(torrent) = guard.torrents.iter_mut().find(|t| t.id == torrent_id) {
        torrent.label = label;
    }
    Ok(())
}

fn handle_add_tracker(
    request: &HttpRequest,
    cmd_tx: &Option<mpsc::Sender<UiCommand>>,
) -> Result<(), String> {
    let body_str = String::from_utf8_lossy(&request.body);
    let form = parse_query_pairs(&body_str);
    let torrent_id = query_value(&form, "id")
        .and_then(|v| v.parse::<u64>().ok())
        .ok_or_else(|| "missing torrent id".to_string())?;
    let url = query_value(&form, "url")
        .ok_or_else(|| "missing tracker url".to_string())?
        .to_string();
    if url.trim().is_empty() {
        return Err("tracker url is empty".to_string());
    }
    dispatch_command_ok(cmd_tx, |reply| UiCommand::AddTracker {
        torrent_id,
        url,
        reply,
    })
}

fn handle_remove_tracker(
    request: &HttpRequest,
    cmd_tx: &Option<mpsc::Sender<UiCommand>>,
) -> Result<(), String> {
    let body_str = String::from_utf8_lossy(&request.body);
    let form = parse_query_pairs(&body_str);
    let torrent_id = query_value(&form, "id")
        .and_then(|v| v.parse::<u64>().ok())
        .ok_or_else(|| "missing torrent id".to_string())?;
    let url = query_value(&form, "url")
        .ok_or_else(|| "missing tracker url".to_string())?
        .to_string();
    dispatch_command_ok(cmd_tx, |reply| UiCommand::RemoveTracker {
        torrent_id,
        url,
        reply,
    })
}

fn handle_select_download_dir() -> Result<Option<String>, String> {
    select_download_dir()
}

#[cfg(target_os = "macos")]
fn select_download_dir() -> Result<Option<String>, String> {
    let output = Command::new("osascript")
        .args([
            "-e",
            "set chosenFolder to POSIX path of (choose folder with prompt \"Select download folder\")",
            "-e",
            "return chosenFolder",
        ])
        .output()
        .map_err(|err| format!("failed to launch folder picker: {err}"))?;

    let stderr = String::from_utf8_lossy(&output.stderr).to_string();
    let lower = stderr.to_ascii_lowercase();
    if lower.contains("user canceled") || lower.contains("user cancelled") || lower.contains("-128")
    {
        return Ok(None);
    }
    if !output.status.success() {
        let detail = stderr.trim();
        if detail.is_empty() {
            return Err("failed to open folder picker".to_string());
        }
        return Err(format!("failed to open folder picker: {detail}"));
    }

    let path = String::from_utf8_lossy(&output.stdout).trim().to_string();
    if path.is_empty() {
        return Err("folder picker returned empty path".to_string());
    }
    Ok(Some(path))
}

#[cfg(not(target_os = "macos"))]
fn select_download_dir() -> Result<Option<String>, String> {
    Err("folder picker not available on this platform".to_string())
}

fn handle_torrent_action(
    path: &str,
    query: &[(String, String)],
    cmd_tx: &Option<mpsc::Sender<UiCommand>>,
) -> Result<(), String> {
    let torrent_id = query_value(query, "id")
        .and_then(|value| value.parse::<u64>().ok())
        .ok_or_else(|| "missing torrent id".to_string())?;
    enum TorrentActionKind {
        Pause,
        Resume,
        Stop,
        Archive,
        Delete { remove_data: bool },
    }
    let action = match path {
        "/torrent/pause" => TorrentActionKind::Pause,
        "/torrent/resume" => TorrentActionKind::Resume,
        "/torrent/stop" => TorrentActionKind::Stop,
        "/torrent/archive" => TorrentActionKind::Archive,
        "/torrent/delete" => TorrentActionKind::Delete {
            remove_data: query_value(query, "data").map(parse_bool).unwrap_or(false),
        },
        _ => return Err("unknown action".to_string()),
    };
    dispatch_command_ok(cmd_tx, |reply| match action {
        TorrentActionKind::Pause => UiCommand::PauseTorrent { torrent_id, reply },
        TorrentActionKind::Resume => UiCommand::ResumeTorrent { torrent_id, reply },
        TorrentActionKind::Stop => UiCommand::StopTorrent { torrent_id, reply },
        TorrentActionKind::Archive => UiCommand::ArchiveTorrent { torrent_id, reply },
        TorrentActionKind::Delete { remove_data } => UiCommand::DeleteTorrent {
            torrent_id,
            remove_data,
            reply,
        },
    })
}

fn handle_open_folder(
    query: &[(String, String)],
    state: &Arc<Mutex<UiState>>,
) -> Result<(), String> {
    let torrent_id = query_value(query, "id")
        .and_then(|value| value.parse::<u64>().ok())
        .ok_or_else(|| "missing torrent id".to_string())?;
    let guard = lock_state(state);
    let torrent = guard
        .torrents
        .iter()
        .find(|t| t.id == torrent_id)
        .ok_or_else(|| "torrent not found".to_string())?;
    let dir = torrent.download_dir.clone();
    drop(guard);
    if dir.is_empty() {
        return Err("no download directory".to_string());
    }
    if !std::path::Path::new(&dir).exists() {
        return Err("directory does not exist".to_string());
    }
    #[cfg(target_os = "macos")]
    {
        Command::new("open")
            .arg(&dir)
            .spawn()
            .map_err(|err| format!("open failed: {err}"))?;
    }
    #[cfg(target_os = "linux")]
    {
        Command::new("xdg-open")
            .arg(&dir)
            .spawn()
            .map_err(|err| format!("open failed: {err}"))?;
    }
    #[cfg(target_os = "windows")]
    {
        Command::new("explorer")
            .arg(&dir)
            .spawn()
            .map_err(|err| format!("open failed: {err}"))?;
    }
    Ok(())
}

fn query_value<'a>(pairs: &'a [(String, String)], key: &str) -> Option<&'a str> {
    pairs
        .iter()
        .find(|(k, _)| k == key)
        .map(|(_, v)| v.as_str())
}

fn parse_query_pairs(query: &str) -> Vec<(String, String)> {
    let mut out = Vec::new();
    for pair in query.split('&') {
        if pair.is_empty() {
            continue;
        }
        let (key, value) = match pair.split_once('=') {
            Some((key, value)) => (key, value),
            None => (pair, ""),
        };
        out.push((percent_decode(key), percent_decode(value)));
    }
    out
}

fn percent_decode(input: &str) -> String {
    let bytes = input.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut idx = 0;
    while idx < bytes.len() {
        match bytes[idx] {
            b'%' if idx + 2 < bytes.len() => {
                let hi = bytes[idx + 1] as char;
                let lo = bytes[idx + 2] as char;
                if let (Some(hi), Some(lo)) = (hi.to_digit(16), lo.to_digit(16)) {
                    out.push((hi * 16 + lo) as u8);
                    idx += 3;
                    continue;
                }
            }
            b'+' => {
                out.push(b' ');
                idx += 1;
                continue;
            }
            _ => {}
        }
        out.push(bytes[idx]);
        idx += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

fn parse_bool(value: &str) -> bool {
    matches!(
        value.trim().to_ascii_lowercase().as_str(),
        "1" | "true" | "yes" | "on"
    )
}

fn extract_origin_host(origin: &str) -> Option<String> {
    let rest = origin
        .trim()
        .strip_prefix("http://")
        .or_else(|| origin.trim().strip_prefix("https://"))?;
    let host = rest.split('/').next()?.trim().to_ascii_lowercase();
    if host.is_empty() {
        None
    } else {
        Some(host)
    }
}

fn authority_host(authority: &str) -> Option<&str> {
    let authority = authority.trim();
    if authority.is_empty() || authority.contains(['/', '\\', '@', '?', '#']) {
        return None;
    }
    if let Some(rest) = authority.strip_prefix('[') {
        let (host, tail) = rest.split_once(']')?;
        if !tail.is_empty()
            && (!tail.starts_with(':')
                || tail[1..].is_empty()
                || tail[1..]
                    .parse::<u16>()
                    .ok()
                    .filter(|port| *port > 0)
                    .is_none())
        {
            return None;
        }
        return (!host.is_empty()).then_some(host);
    }
    if authority.contains(['[', ']']) || authority.matches(':').count() > 1 {
        return None;
    }
    match authority.rsplit_once(':') {
        Some((host, port)) => (!host.is_empty()
            && port.parse::<u16>().ok().filter(|port| *port > 0).is_some())
        .then_some(host),
        None => Some(authority),
    }
}

fn request_has_safe_host(request: &HttpRequest) -> bool {
    let Some(host) = request.header_value("host").and_then(authority_host) else {
        // HTTP/1.0 clients may omit Host, but the browser UI and API require it
        // so accepting a hostless request provides no useful compatibility.
        return false;
    };
    host.eq_ignore_ascii_case("localhost")
        || host
            .parse::<IpAddr>()
            .map(|ip| !ip.is_unspecified())
            .unwrap_or(false)
}

fn request_origin_matches_host(request: &HttpRequest) -> bool {
    let Some(origin) = request.header_value("origin") else {
        return true;
    };
    let Some(origin_host) = extract_origin_host(origin) else {
        return false;
    };
    let request_host = request
        .header_value("host")
        .unwrap_or("")
        .trim()
        .to_ascii_lowercase();
    !request_host.is_empty() && request_host == origin_host
}

fn has_valid_api_token(request: &HttpRequest) -> bool {
    request
        .header_value(API_TOKEN_HEADER)
        .map(|value| constant_time_eq(value.as_bytes(), api_token().as_bytes()))
        .unwrap_or(false)
}

fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    let mut acc = 0u8;
    for (x, y) in a.iter().zip(b.iter()) {
        acc |= x ^ y;
    }
    acc == 0
}

fn authorize_mutating_request(request: &HttpRequest) -> Result<(), String> {
    if !has_valid_api_token(request) {
        return Err("missing or invalid api token".to_string());
    }
    if request.header_value("origin").is_none() {
        return Err("origin header required".to_string());
    }
    if !request_origin_matches_host(request) {
        return Err("forbidden origin".to_string());
    }
    Ok(())
}

fn update_error(state: &Arc<Mutex<UiState>>, message: &str) {
    let mut guard = lock_state(state);
    guard.last_error = message.to_string();
    if guard.status != "downloading" {
        guard.status = "error".to_string();
    }
}

fn reason_phrase(code: u16) -> &'static str {
    match code {
        200 => "OK",
        400 => "Bad Request",
        403 => "Forbidden",
        404 => "Not Found",
        409 => "Conflict",
        429 => "Too Many Requests",
        503 => "Service Unavailable",
        504 => "Gateway Timeout",
        _ => "Error",
    }
}

fn status_for_error(message: &str) -> u16 {
    let lower = message.to_ascii_lowercase();
    if lower.contains("timeout") {
        504
    } else if lower.contains("invalid api token")
        || lower.contains("missing api token")
        || lower.contains("forbidden origin")
        || lower.contains("forbidden host")
        || lower.contains("origin header required")
    {
        403
    } else if lower.contains("unknown torrent") {
        404
    } else if lower.contains("already added") {
        409
    } else if lower.contains("not ready")
        || lower.contains("service unavailable")
        || lower.contains("channel closed")
    {
        503
    } else if lower.contains("invalid")
        || lower.contains("missing")
        || lower.contains("empty")
        || lower.contains("unknown action")
        || lower.contains("bad request")
    {
        400
    } else {
        409
    }
}

fn send_json_body(mut stream: TcpStream, code: u16, body: &str) -> std::io::Result<()> {
    let reason = reason_phrase(code);
    let response = format!(
        "HTTP/1.1 {code} {reason}\r\nContent-Type: application/json\r\nCache-Control: no-store\r\n{SECURITY_HEADERS}Content-Length: {}\r\nConnection: close\r\n\r\n",
        body.len()
    );
    stream.write_all(response.as_bytes())?;
    stream.write_all(body.as_bytes())?;
    Ok(())
}

fn send_api_ok(stream: TcpStream) -> std::io::Result<()> {
    send_json_body(stream, 200, r#"{"ok":true}"#)
}

fn send_api_ok_with_torrent_id(stream: TcpStream, torrent_id: u64) -> std::io::Result<()> {
    let body = format!(r#"{{"ok":true,"torrent_id":{torrent_id}}}"#);
    send_json_body(stream, 200, &body)
}

fn send_api_ok_with_path(stream: TcpStream, path: &str) -> std::io::Result<()> {
    let body = format!(r#"{{"ok":true,"path":"{}"}}"#, escape_json(path));
    send_json_body(stream, 200, &body)
}

fn send_api_error(stream: TcpStream, message: &str) -> std::io::Result<()> {
    send_api_error_with_status(stream, status_for_error(message), message)
}

fn send_api_error_with_status(stream: TcpStream, code: u16, message: &str) -> std::io::Result<()> {
    let body = format!("{{\"ok\":false,\"error\":\"{}\"}}", escape_json(message));
    send_json_body(stream, code, &body)
}

fn check_api_token_rate(ip: IpAddr) -> bool {
    let lock = API_TOKEN_RATE.get_or_init(|| Mutex::new(HashMap::new()));
    let mut map = match lock.lock() {
        Ok(guard) => guard,
        Err(poisoned) => poisoned.into_inner(),
    };
    let now = Instant::now();
    map.retain(|_, (_, ts)| now.duration_since(*ts) < API_TOKEN_RATE_WINDOW);
    let entry = map.entry(ip).or_insert((0, now));
    entry.0 += 1;
    entry.0 <= API_TOKEN_RATE_MAX
}

fn send_head(mut stream: TcpStream, content_type: &str) -> std::io::Result<()> {
    let response = format!(
        "HTTP/1.1 200 OK\r\nContent-Type: {content_type}\r\nCache-Control: no-store, no-cache, must-revalidate\r\nPragma: no-cache\r\nExpires: 0\r\n{SECURITY_HEADERS}Connection: close\r\nContent-Length: 0\r\n\r\n"
    );
    stream.write_all(response.as_bytes())
}

fn send_api_token(stream: TcpStream) -> std::io::Result<()> {
    let body = api_token_json(api_token(), ui_owner_secret());
    send_json_body(stream, 200, &body)
}

fn api_token_json(token: &str, owner_secret: &str) -> String {
    format!(
        r#"{{"token":"{}","owner_secret":"{}"}}"#,
        escape_json(token),
        escape_json(owner_secret)
    )
}

fn handle_sse(mut stream: TcpStream, state: Arc<Mutex<UiState>>) -> std::io::Result<()> {
    let response = format!(
        "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nCache-Control: no-cache\r\n{SECURITY_HEADERS}Connection: keep-alive\r\n\r\n"
    );
    stream.write_all(response.as_bytes())?;
    stream.flush()?;

    let mut last_payload = String::new();
    let mut last_ping = Instant::now();
    loop {
        let payload = {
            let guard = lock_state(&state);
            app_body_html(&guard)
        };
        if payload != last_payload {
            if write_sse_event(&mut stream, "status", &payload).is_err() {
                break;
            }
            last_payload = payload;
            last_ping = Instant::now();
        } else if last_ping.elapsed() > Duration::from_secs(15) {
            if write_sse_comment(&mut stream, "ping").is_err() {
                break;
            }
            last_ping = Instant::now();
        }
        thread::sleep(Duration::from_millis(450));
    }
    Ok(())
}

fn write_sse_event(stream: &mut TcpStream, event: &str, data: &str) -> std::io::Result<()> {
    stream.write_all(format!("event: {event}\n").as_bytes())?;
    for line in data.split('\n') {
        stream.write_all(b"data: ")?;
        stream.write_all(line.as_bytes())?;
        stream.write_all(b"\n")?;
    }
    stream.write_all(b"\n")?;
    stream.flush()
}

fn write_sse_comment(stream: &mut TcpStream, comment: &str) -> std::io::Result<()> {
    stream.write_all(format!(": {comment}\n\n").as_bytes())?;
    stream.flush()
}

fn status_html(state: &UiState) -> String {
    let mut out = String::with_capacity(5200 + state.torrents.len() * 2200);
    out.push_str("<!doctype html><html lang=\"en\"><head><meta charset=\"utf-8\"><meta name=\"viewport\" content=\"width=device-width,initial-scale=1\"><link rel=\"icon\" href=\"data:,\">");
    out.push_str("<title>rustorrent</title>");
    out.push_str(&format!(
        "<meta name=\"rustorrent-api-token\" content=\"{}\">",
        escape_html(api_token())
    ));
    out.push_str("<script>try{var t=localStorage.getItem('rustorrent-theme');if(t!=='light'&&t!=='dark'){t='light';}document.documentElement.setAttribute('data-theme',t);}catch(e){}</script>");
    out.push_str("<style>");
    out.push_str(include_str!("../assets/ui/app.css"));
    out.push_str(include_str!("../assets/ui/beta.css"));
    out.push_str("</style></head><body>");
    out.push_str(r##"<svg xmlns="http://www.w3.org/2000/svg" style="position:absolute;width:0;height:0;overflow:hidden" aria-hidden="true"><defs><symbol id="i-add" viewBox="0 0 24 24"><path d="M12 5v14M5 12h14"/></symbol><symbol id="i-archive" viewBox="0 0 24 24"><path d="M4 8.5h16V19a1 1 0 0 1-1 1H5a1 1 0 0 1-1-1z"/><path d="M3.5 4.5h17v4h-17z"/><path d="M10 12h4"/></symbol><symbol id="i-bolt" viewBox="0 0 24 24"><path d="M13 2.5 4.5 13.5H11l-1 8 8.5-11.5H12z"/></symbol><symbol id="i-check" viewBox="0 0 24 24"><path d="M5 13l4 4L19 7"/></symbol><symbol id="i-check_circle" viewBox="0 0 24 24"><circle cx="12" cy="12" r="9"/><path d="m8.5 12 2.4 2.4L15.6 9"/></symbol><symbol id="i-close" viewBox="0 0 24 24"><path d="M6 6l12 12M6 18 18 6"/></symbol><symbol id="i-cloud_download" viewBox="0 0 24 24"><path d="M7 18a4 4 0 0 1-.6-7.96 5.5 5.5 0 0 1 10.7-1.05A4 4 0 0 1 17.2 18"/><path d="M12 11.5v6.5m0 0-2.4-2.4M12 18l2.4-2.4"/></symbol><symbol id="i-cloud_upload" viewBox="0 0 24 24"><path d="M7 18a4 4 0 0 1-.6-7.96 5.5 5.5 0 0 1 10.7-1.05A4 4 0 0 1 17.2 18"/><path d="M12 18.5V12m0 0-2.4 2.4M12 12l2.4 2.4"/></symbol><symbol id="i-dark_mode" viewBox="0 0 24 24"><path d="M20 14.5A8 8 0 0 1 9.5 4 7 7 0 1 0 20 14.5z"/></symbol><symbol id="i-delete" viewBox="0 0 24 24"><path d="M4 7h16"/><path d="M9 7V5a1 1 0 0 1 1-1h4a1 1 0 0 1 1 1v2"/><path d="M6 7l1 12.1a1 1 0 0 0 1 .9h8a1 1 0 0 0 1-.9L18 7"/><path d="M10 11v5M14 11v5"/></symbol><symbol id="i-description" viewBox="0 0 24 24"><path d="M14 3.5v5h5"/><path d="M14 3.5H6.5a1 1 0 0 0-1 1v15a1 1 0 0 0 1 1h11a1 1 0 0 0 1-1V8.5z"/><path d="M9 13h6M9 16.5h4"/></symbol><symbol id="i-download" viewBox="0 0 24 24"><path d="M12 4v11m0 0-4-4m4 4 4-4"/><path d="M5 20h14"/></symbol><symbol id="i-downloading" viewBox="0 0 24 24"><path d="M12 3.5v8m0 0-3-3m3 3 3-3"/><path d="M5.2 14a7 7 0 0 0 13.6 0"/></symbol><symbol id="i-error" viewBox="0 0 24 24"><circle cx="12" cy="12" r="9"/><path d="M12 7.5v5.5M12 16.5h.01"/></symbol><symbol id="i-extension" viewBox="0 0 24 24"><path d="M9 5.2a2 2 0 1 1 4 0V6h2.8a1 1 0 0 1 1 1v2.8h.7a2 2 0 1 1 0 4h-.7V17a1 1 0 0 1-1 1H13v-.8a2 2 0 1 0-4 0V18H6.2a1 1 0 0 1-1-1v-3h-.7a2 2 0 1 1 0-4h.7V7.2a1 1 0 0 1 1-1H9z"/></symbol><symbol id="i-folder" viewBox="0 0 24 24"><path d="M3.5 6.5a1 1 0 0 1 1-1H10l2 2h7.5a1 1 0 0 1 1 1v9a1 1 0 0 1-1 1h-15a1 1 0 0 1-1-1z"/></symbol><symbol id="i-folder_open" viewBox="0 0 24 24"><path d="M3.5 7a1 1 0 0 1 1-1H10l2 2h7.5a1 1 0 0 1 1 1v1.5H7a1 1 0 0 0-.95.68"/><path d="m3.6 18.5 2.2-7a1 1 0 0 1 .95-.7H21l-2.2 7a1 1 0 0 1-.95.7H4.5a1 1 0 0 1-1-1z"/></symbol><symbol id="i-grid_view" viewBox="0 0 24 24"><rect x="4" y="4" width="6.5" height="6.5" rx="1.4"/><rect x="13.5" y="4" width="6.5" height="6.5" rx="1.4"/><rect x="4" y="13.5" width="6.5" height="6.5" rx="1.4"/><rect x="13.5" y="13.5" width="6.5" height="6.5" rx="1.4"/></symbol><symbol id="i-group" viewBox="0 0 24 24"><circle cx="9" cy="8" r="3"/><path d="M3 19a6 6 0 0 1 12 0"/><path d="M16 5.2a3 3 0 0 1 0 5.6"/><path d="M18 19a6 6 0 0 0-3-5.2"/></symbol><symbol id="i-hub" viewBox="0 0 24 24"><circle cx="12" cy="12" r="2.3"/><circle cx="12" cy="4.2" r="1.8"/><circle cx="12" cy="19.8" r="1.8"/><circle cx="5" cy="8" r="1.8"/><circle cx="19" cy="8" r="1.8"/><path d="M12 6v3.7M10.2 11 6.5 8.9M13.8 11l3.7-2.1M12 14.3v3.7"/></symbol><symbol id="i-label" viewBox="0 0 24 24"><path d="M4 6.5a1 1 0 0 1 1-1h9.3a1 1 0 0 1 .78.37l4.6 5.5a1 1 0 0 1 0 1.26l-4.6 5.5a1 1 0 0 1-.78.37H5a1 1 0 0 1-1-1z"/><circle cx="8" cy="12" r="1.1"/></symbol><symbol id="i-list" viewBox="0 0 24 24"><path d="M8 6h12M8 12h12M8 18h12"/><path d="M3.6 6h.01M3.6 12h.01M3.6 18h.01"/></symbol><symbol id="i-manage_search" viewBox="0 0 24 24"><circle cx="10.5" cy="10.5" r="6"/><path d="m20 20-5.2-5.2"/></symbol><symbol id="i-pause_circle" viewBox="0 0 24 24"><circle cx="12" cy="12" r="9"/><path d="M10 9v6M14 9v6"/></symbol><symbol id="i-queue" viewBox="0 0 24 24"><path d="M4 7h11M4 12h11M4 17h7"/><path d="M18 13v6M15 16h6"/></symbol><symbol id="i-refresh" viewBox="0 0 24 24"><path d="M20.5 12a8.5 8.5 0 1 1-2.5-6"/><path d="M20.5 4v5h-5"/></symbol><symbol id="i-rss_feed" viewBox="0 0 24 24"><circle cx="6" cy="18" r="1.6"/><path d="M4.5 11a8.5 8.5 0 0 1 8.5 8.5"/><path d="M4.5 5a14.5 14.5 0 0 1 14.5 14.5"/></symbol><symbol id="i-schedule" viewBox="0 0 24 24"><circle cx="12" cy="12" r="9"/><path d="M12 7.5v5l3.4 2"/></symbol><symbol id="i-stop" viewBox="0 0 24 24"><rect x="5.5" y="5.5" width="13" height="13" rx="2.5"/></symbol><symbol id="i-swap_vert" viewBox="0 0 24 24"><path d="M7 4.5v15m0 0-3-3m3 3 3-3"/><path d="M17 19.5v-15m0 0-3 3m3-3 3 3"/></symbol><symbol id="i-system_update" viewBox="0 0 24 24"><rect x="7" y="3" width="10" height="18" rx="2"/><path d="M12 7v6m0 0-2.2-2.2M12 13l2.2-2.2"/></symbol><symbol id="i-travel_explore" viewBox="0 0 24 24"><circle cx="10.5" cy="10.5" r="6.5"/><path d="M4 10.5h13"/><path d="M10.5 4a12 12 0 0 1 0 13M10.5 4a12 12 0 0 0 0 13"/><path d="m19.5 19.5-2.4-2.4"/></symbol><symbol id="i-unfold_less" viewBox="0 0 24 24"><path d="m8 5.5 4 4 4-4"/><path d="m8 18.5 4-4 4 4"/></symbol><symbol id="i-unfold_more" viewBox="0 0 24 24"><path d="m8 9.5 4-4 4 4"/><path d="m8 14.5 4 4 4-4"/></symbol><symbol id="i-upload" viewBox="0 0 24 24"><path d="M12 20V9m0 0-4 4m4-4 4 4"/><path d="M5 4.5h14"/></symbol><symbol id="i-upload_file" viewBox="0 0 24 24"><path d="M14 3.5v5h5"/><path d="M14 3.5H6.5a1 1 0 0 0-1 1v15a1 1 0 0 0 1 1h11a1 1 0 0 0 1-1V8.5z"/><path d="M12 18v-5m0 0-2 2m2-2 2 2"/></symbol><symbol id="i-verified" viewBox="0 0 24 24"><path d="M12 3.2 5 6v6c0 4 3 6.7 7 8 4-1.3 7-4 7-8V6z"/><path d="m9 12 2 2 4-4"/></symbol><symbol id="i-play_arrow" viewBox="0 0 24 24"><path fill="currentColor" stroke="none" d="M8 5.6v12.8a1 1 0 0 0 1.52.86l10.5-6.4a1 1 0 0 0 0-1.72L9.52 4.74A1 1 0 0 0 8 5.6z"/></symbol><symbol id="i-pause" viewBox="0 0 24 24"><rect x="7" y="5" width="3.4" height="14" rx="1.1" fill="currentColor" stroke="none"/><rect x="13.6" y="5" width="3.4" height="14" rx="1.1" fill="currentColor" stroke="none"/></symbol><symbol id="i-light_mode" viewBox="0 0 24 24"><circle cx="12" cy="12" r="4"/><path d="M12 2.5v2.2M12 19.3v2.2M4.6 4.6l1.6 1.6M17.8 17.8l1.6 1.6M2.5 12h2.2M19.3 12h2.2M4.6 19.4l1.6-1.6M17.8 6.2l1.6-1.6"/></symbol><symbol id="i-arrow_upward" viewBox="0 0 24 24"><path d="M12 20V5m0 0-6 6m6-6 6 6"/></symbol><symbol id="i-arrow_downward" viewBox="0 0 24 24"><path d="M12 4v15m0 0-6-6m6 6 6-6"/></symbol></defs></svg>"##);
    out.push_str("<div class=\"app\" id=\"appRoot\">");
    out.push_str(&app_body_html(state));
    out.push_str("</div>");
    out.push_str("<script>");
    out.push_str(include_str!("../assets/ui/app.js"));
    out.push_str("</script>");
    out.push_str("</body></html>");
    out
}

fn render_search_panel(out: &mut String) {
    out.push_str("<div class=\"panel\">");
    out.push_str("<div class=\"panel-title\"><svg class=\"material-symbols-rounded\" style=\"font-size:16px;vertical-align:-3px;margin-right:4px\"><use href=\"#i-travel_explore\"></use></svg>Torrent Search</div>");
    out.push_str("<form class=\"search-form\" onsubmit=\"submitSearchQuery(event)\">");
    out.push_str("<input id=\"searchQuery\" class=\"input\" type=\"search\" placeholder=\"Search public torrent plugins\" autocomplete=\"off\">");
    out.push_str("<select id=\"searchCategory\" class=\"input\">");
    out.push_str("<option value=\"all\">All categories</option>");
    out.push_str("<option value=\"anime\">Anime</option>");
    out.push_str("<option value=\"books\">Books</option>");
    out.push_str("<option value=\"games\">Games</option>");
    out.push_str("<option value=\"movies\">Movies</option>");
    out.push_str("<option value=\"music\">Music</option>");
    out.push_str("<option value=\"pictures\">Pictures</option>");
    out.push_str("<option value=\"software\">Software</option>");
    out.push_str("<option value=\"tv\">TV</option>");
    out.push_str("</select>");
    out.push_str(
        "<div id=\"searchPluginError\" class=\"search-alert\" style=\"display:none\"></div>",
    );
    out.push_str("<div id=\"searchSelectionSummary\" class=\"search-selection-summary\">Loading plugin selection...</div>");
    out.push_str(
        "<div id=\"searchPluginWarning\" class=\"search-warning\" style=\"display:none\"></div>",
    );
    out.push_str("<div class=\"search-panel-actions\">");
    out.push_str("<button type=\"submit\" class=\"btn primary\">Search</button>");
    out.push_str("</div>");
    out.push_str("</form>");
    out.push_str(
        "<div class=\"search-note\">Search uses your enabled plugins. Open Plugins from the results header when you want to manage sources.</div>",
    );
    out.push_str("</div>");
}

fn render_search_results_panel(out: &mut String) {
    out.push_str("<div class=\"panel search-results-panel search-main-panel active\" data-search-view=\"results\">");
    out.push_str("<div class=\"panel-head\">");
    out.push_str("<div><div class=\"panel-title\"><svg class=\"material-symbols-rounded\" style=\"font-size:16px;vertical-align:-3px;margin-right:4px\"><use href=\"#i-manage_search\"></use></svg>Search Results</div>");
    out.push_str("<div class=\"small\" id=\"searchStatusText\">Install a plugin and run a search to populate results.</div></div>");
    out.push_str("<div class=\"search-plugin-manager-tools\">");
    out.push_str("<button class=\"btn ghost\" type=\"button\" data-search-view-target=\"plugins\"><svg class=\"material-symbols-rounded\"><use href=\"#i-extension\"></use></svg>Plugins</button>");
    out.push_str("<button class=\"btn ghost panel-toggle\" type=\"button\" onclick=\"loadSearchStatus(true)\"><svg class=\"material-symbols-rounded\"><use href=\"#i-refresh\"></use></svg>Refresh</button>");
    out.push_str("</div>");
    out.push_str("</div>");
    out.push_str("<div id=\"searchResults\" class=\"search-results-grid\"><div class=\"search-results-empty\">No search results yet.</div></div>");
    out.push_str("</div>");
}

fn render_search_plugin_manager(out: &mut String) {
    out.push_str("<div class=\"panel search-plugin-manager search-main-panel\" data-search-view=\"plugins\">");
    out.push_str("<div class=\"search-plugin-manager-head\">");
    out.push_str("<div class=\"panel-title\"><svg class=\"material-symbols-rounded\" style=\"font-size:16px;vertical-align:-3px;margin-right:4px\"><use href=\"#i-extension\"></use></svg>Plugins</div>");
    out.push_str("<div class=\"search-plugin-manager-tools\">");
    out.push_str("<button type=\"button\" class=\"btn ghost\" onclick=\"updateInstalledCatalogPlugins()\"><svg class=\"material-symbols-rounded\"><use href=\"#i-system_update\"></use></svg>Update All</button>");
    out.push_str("<button type=\"button\" class=\"btn ghost\" data-search-view-target=\"results\"><svg class=\"material-symbols-rounded\"><use href=\"#i-close\"></use></svg>Close</button>");
    out.push_str("</div>");
    out.push_str("</div>");
    // Installed section
    out.push_str("<div class=\"rss-section-label\">Installed</div>");
    out.push_str("<div id=\"searchPluginList\" class=\"search-plugin-list\"><div class=\"rss-item\"><span class=\"rss-item-info\">Loading...</span></div></div>");
    // Quick install
    out.push_str("<div style=\"display:flex;gap:6px;margin-top:10px;align-items:center\">");
    out.push_str("<form class=\"rss-form\" style=\"flex:1;margin:0\" onsubmit=\"installSearchPluginUrl(event)\">");
    out.push_str("<input id=\"searchPluginUrl\" class=\"input\" placeholder=\"https://.../plugin.py\" style=\"height:30px;font-size:12px\">");
    out.push_str("</form>");
    out.push_str("<button type=\"button\" class=\"btn primary\" style=\"height:30px;font-size:12px;padding:0 10px\" onclick=\"document.getElementById('searchPluginUrl')&&installSearchPluginUrl()\">Install URL</button>");
    out.push_str("<label class=\"btn ghost search-upload-label\" style=\"height:30px;font-size:12px;padding:0 10px\"><svg class=\"material-symbols-rounded\" style=\"font-size:14px\"><use href=\"#i-upload_file\"></use></svg>Upload .py<input id=\"searchPluginFile\" type=\"file\" accept=\".py\" onchange=\"installSearchPluginFile(event)\"></label>");
    out.push_str("</div>");
    // Community catalog
    out.push_str("<div class=\"rss-section-label\" style=\"margin-top:12px\">Community Catalog <button type=\"button\" class=\"btn ghost\" title=\"Refresh catalog\" aria-label=\"Refresh catalog\" style=\"padding:0 6px;height:20px;font-size:11px;vertical-align:1px\" onclick=\"loadSearchCatalog(true)\"><svg class=\"material-symbols-rounded\" style=\"font-size:13px\" aria-hidden=\"true\"><use href=\"#i-refresh\"></use></svg></button></div>");
    out.push_str(
        "<div id=\"searchCatalogMeta\" class=\"small\">Loading community plugins...</div>",
    );
    out.push_str("<input id=\"searchCatalogFilter\" class=\"input\" type=\"search\" placeholder=\"Filter...\" autocomplete=\"off\" style=\"height:28px;padding:0 8px;font-size:12px;margin-top:4px\">");
    out.push_str("<div id=\"searchCatalog\" class=\"search-catalog-list\"><div class=\"rss-item\"><span class=\"rss-item-info\">Loading...</span></div></div>");
    // Recommended
    out.push_str("<div class=\"rss-section-label\" style=\"margin-top:12px\">Quick Start</div>");
    out.push_str("<div class=\"small\">Popular public plugins for general search.</div>");
    out.push_str("<div id=\"searchRecommended\"><div class=\"rss-item\"><span class=\"rss-item-info\">Loading...</span></div></div>");
    out.push_str("</div>");
}

fn app_body_html(state: &UiState) -> String {
    let mut out = String::with_capacity(4200 + state.torrents.len() * 2000);
    let _last_added = escape_html(&state.last_added);
    let total_torrents = state.torrents.len();
    let download_dir = escape_html(&state.download_dir);
    let total_downloaded_bytes = state.session_downloaded_bytes;
    let total_uploaded_bytes = state.session_uploaded_bytes;
    let total_downloaded = human_bytes(total_downloaded_bytes);
    let total_uploaded = human_bytes(total_uploaded_bytes);
    let download_limit_kbps = state.global_download_limit_bps / 1024;
    let upload_limit_kbps = state.global_upload_limit_bps / 1024;
    let peer_profile = match state.peer_profile.as_str() {
        "conservative" | "aggressive" => state.peer_profile.as_str(),
        _ => "balanced",
    };
    let total_tracker_peers: usize = state.torrents.iter().map(|t| t.tracker_peers).sum();
    let total_active_peers: usize = state.torrents.iter().map(|t| t.active_peers).sum();

    let mut downloading = 0usize;
    let mut complete = 0usize;
    let mut paused = 0usize;
    let mut errored = 0usize;
    let mut queued = 0usize;
    let is_complete_torrent = |torrent: &UiTorrent| -> bool {
        (torrent.total_pieces > 0 && torrent.completed_pieces >= torrent.total_pieces)
            || (torrent.total_bytes > 0 && torrent.completed_bytes >= torrent.total_bytes)
    };
    let bucket_for = |torrent: &UiTorrent| -> &'static str {
        let status = torrent.status.as_str();
        if torrent.paused || matches!(status, "paused" | "stopped" | "stopping") {
            return "paused";
        }
        if status == "queued" {
            return "queued";
        }
        if status.contains("error") || status.contains("failed") {
            return "error";
        }
        if is_complete_torrent(torrent) {
            return "complete";
        }
        "downloading"
    };
    for torrent in &state.torrents {
        match bucket_for(torrent) {
            "downloading" => downloading += 1,
            "complete" => complete += 1,
            "paused" => paused += 1,
            "error" => errored += 1,
            "queued" => queued += 1,
            _ => {}
        }
    }
    let (fleet_state, fleet_signal, fleet_copy) = if errored > 0 {
        (
            "Needs attention",
            "error",
            "One or more torrents reported an error. Open the affected item for details.",
        )
    } else if state.download_rate_bps > 0.0 {
        (
            "Downloading",
            "live",
            "Receiving files from connected peers.",
        )
    } else if state.upload_rate_bps > 0.0 {
        (
            "Seeding now",
            "live",
            "Verified pieces are being served to peers. Upload rate reflects actual peer requests.",
        )
    } else if downloading > 0 {
        (
            "Waiting for peers",
            "",
            "Transfers will begin when a reachable peer has the requested pieces.",
        )
    } else if complete > 0 {
        (
            "Ready to seed",
            "live",
            "Completed torrents stay available. Uploads begin when interested peers request pieces.",
        )
    } else if queued > 0 {
        (
            "Queued",
            "",
            "Torrents are waiting for an active slot or metadata before transfer begins.",
        )
    } else if total_torrents == 0 {
        ("Ready", "", "Your transfers will appear here.")
    } else {
        (
            "Idle",
            "warn",
            "No active peer traffic right now. Trackers and DHT continue looking for peers.",
        )
    };

    out.push_str("<header class=\"appbar\">");
    out.push_str("<div class=\"appbar-main\">");
    out.push_str("<div class=\"brand\">");
    out.push_str(
        "<div class=\"brand-icon\"><svg class=\"material-symbols-rounded\"><use href=\"#i-downloading\"></use></svg></div>",
    );
    out.push_str("<div><div class=\"title\">Rustorrent</div>");
    out.push_str("<div class=\"sub\">Small app. Shared files.</div></div>");
    out.push_str("</div>");
    out.push_str("<div class=\"app-tabs\">");
    out.push_str("<button class=\"tab-btn active\" type=\"button\" data-main-tab-target=\"library\"><svg class=\"material-symbols-rounded\"><use href=\"#i-folder\"></use></svg>Library</button>");
    out.push_str("<button class=\"tab-btn\" type=\"button\" data-main-tab-target=\"search\"><svg class=\"material-symbols-rounded\"><use href=\"#i-travel_explore\"></use></svg>Search</button>");
    out.push_str("</div>");
    out.push_str("</div>");
    out.push_str("<div class=\"app-actions\">");
    out.push_str(&format!(
        "<div class=\"header-rate\" aria-label=\"Download speed\"><span>Download</span><strong>↓ {}</strong></div>", human_rate(state.download_rate_bps)
    ));
    out.push_str(&format!("<div class=\"header-rate\" aria-label=\"Upload speed\"><span>Upload</span><strong>↑ {}</strong></div>", human_rate(state.upload_rate_bps)));
    out.push_str("<div class=\"toolbar\">");
    out.push_str(
        "<button class=\"btn primary\" type=\"button\" onclick=\"openAdd()\"><svg class=\"material-symbols-rounded\"><use href=\"#i-add\"></use></svg>Add Torrent</button>",
    );
    out.push_str(
        "<button class=\"btn icon-btn ghost\" id=\"themeToggle\" type=\"button\" title=\"Toggle theme\"><svg class=\"material-symbols-rounded\"><use href=\"#i-dark_mode\"></use></svg></button>",
    );
    out.push_str("</div>");
    out.push_str("</div>");
    out.push_str("</header>");

    out.push_str("<div class=\"layout workspace active\" data-main-tab=\"library\">");

    out.push_str("<aside class=\"sidebar\">");
    out.push_str("<div class=\"panel\">");
    out.push_str("<div class=\"panel-title\">Library</div>");
    out.push_str(
        "<input id=\"librarySearch\" class=\"input\" type=\"search\" placeholder=\"Search torrents\" autocomplete=\"off\" style=\"margin-top:8px\">",
    );
    out.push_str("<div class=\"nav\">");
    out.push_str(&format!(
        "<button class=\"nav-item active\" type=\"button\" data-filter=\"all\"><span class=\"nav-label\"><svg class=\"material-symbols-rounded\"><use href=\"#i-list\"></use></svg>All</span><span class=\"count\">{}</span></button>",
        state.torrents.len()
    ));
    out.push_str(&format!(
        "<button class=\"nav-item\" type=\"button\" data-filter=\"downloading\"><span class=\"nav-label\"><svg class=\"material-symbols-rounded\"><use href=\"#i-download\"></use></svg>Downloading</span><span class=\"count\">{downloading}</span></button>"
    ));
    out.push_str(&format!(
        "<button class=\"nav-item\" type=\"button\" data-filter=\"complete\"><span class=\"nav-label\"><svg class=\"material-symbols-rounded\"><use href=\"#i-check_circle\"></use></svg>Complete</span><span class=\"count\">{complete}</span></button>"
    ));
    out.push_str(&format!(
        "<button class=\"nav-item\" type=\"button\" data-filter=\"paused\"><span class=\"nav-label\"><svg class=\"material-symbols-rounded\"><use href=\"#i-pause_circle\"></use></svg>Paused</span><span class=\"count\">{paused}</span></button>"
    ));
    out.push_str(&format!(
        "<button class=\"nav-item\" type=\"button\" data-filter=\"queued\"><span class=\"nav-label\"><svg class=\"material-symbols-rounded\"><use href=\"#i-schedule\"></use></svg>Queued</span><span class=\"count\">{queued}</span></button>"
    ));
    out.push_str(&format!(
        "<button class=\"nav-item\" type=\"button\" data-filter=\"error\"><span class=\"nav-label\"><svg class=\"material-symbols-rounded\"><use href=\"#i-error\"></use></svg>Errors</span><span class=\"count\">{errored}</span></button>"
    ));
    // Label filter buttons
    {
        let mut labels: Vec<String> = state
            .torrents
            .iter()
            .filter(|t| !t.label.is_empty())
            .map(|t| t.label.clone())
            .collect();
        labels.sort();
        labels.dedup();
        for lbl in &labels {
            let count = state.torrents.iter().filter(|t| t.label == *lbl).count();
            out.push_str(&format!(
                "<button class=\"nav-item\" type=\"button\" data-filter=\"label:{}\" style=\"margin-top:2px\"><span class=\"nav-label\"><svg class=\"material-symbols-rounded\"><use href=\"#i-label\"></use></svg>{}</span><span class=\"count\">{count}</span></button>",
                escape_html(lbl),
                escape_html(lbl)
            ));
        }
    }
    out.push_str("</div></div>");

    out.push_str(
        "<div class=\"panel transfer-panel\" data-panel=\"transfer\" data-collapsed=\"true\">",
    );
    out.push_str("<div class=\"panel-head\">");
    out.push_str("<div><div class=\"panel-title\">Transfer</div>");
    out.push_str(&format!(
        "<div class=\"small\" style=\"margin-top:6px\">Down {}  Up {}</div>",
        human_rate(state.download_rate_bps),
        human_rate(state.upload_rate_bps)
    ));
    out.push_str("</div>");
    out.push_str("<button class=\"btn ghost panel-toggle\" type=\"button\" data-action=\"toggle-panel\" data-panel=\"transfer\"><svg class=\"material-symbols-rounded\"><use href=\"#i-unfold_more\"></use></svg>Expand</button>");
    out.push_str("</div>");
    out.push_str("<div class=\"transfer-panel-body\">");
    out.push_str(&render_speed_chart(
        &state.download_history_bps,
        &state.upload_history_bps,
    ));
    out.push_str("<div class=\"limit-controls\">");
    out.push_str("<div class=\"limit-row\">");
    out.push_str("<div class=\"limit-label\">Max download</div>");
    out.push_str(&format!(
        "<div class=\"limit-value\" id=\"downloadLimitValue\">{}</div>",
        human_rate(state.global_download_limit_bps as f64)
    ));
    out.push_str("</div>");
    out.push_str(&format!(
        "<input id=\"downloadLimit\" class=\"limit-slider\" type=\"range\" min=\"0\" max=\"102400\" step=\"128\" value=\"{download_limit_kbps}\">"
    ));
    out.push_str("<div class=\"limit-row\" style=\"margin-top:8px\">");
    out.push_str("<div class=\"limit-label\">Max upload</div>");
    out.push_str(&format!(
        "<div class=\"limit-value\" id=\"uploadLimitValue\">{}</div>",
        human_rate(state.global_upload_limit_bps as f64)
    ));
    out.push_str("</div>");
    out.push_str(&format!(
        "<input id=\"uploadLimit\" class=\"limit-slider\" type=\"range\" min=\"0\" max=\"102400\" step=\"64\" value=\"{upload_limit_kbps}\">"
    ));
    out.push_str("</div>");
    out.push_str("<div class=\"limit-group\">");
    out.push_str("<div class=\"limit-label\">Seed Ratio</div>");
    out.push_str(&format!(
        "<div class=\"limit-value\" id=\"seedRatioValue\">{}</div>",
        if state.seed_ratio > 0.0 {
            format!("{:.2}", state.seed_ratio)
        } else {
            "unlimited".to_string()
        }
    ));
    out.push_str("</div>");
    out.push_str(&format!(
        "<input id=\"seedRatio\" class=\"limit-slider\" type=\"range\" min=\"0\" max=\"100\" step=\"1\" value=\"{}\" title=\"0 = unlimited\">",
        (state.seed_ratio * 10.0).round() as u32
    ));
    out.push_str("<div class=\"limit-group\" style=\"margin-top:12px\">");
    out.push_str("<div class=\"limit-label\">Peer Profile</div>");
    out.push_str("<select id=\"peerProfile\" class=\"input\" style=\"margin-top:8px\">");
    for (value, label) in [
        ("conservative", "Conservative"),
        ("balanced", "Balanced"),
        ("aggressive", "Aggressive"),
    ] {
        let selected = if peer_profile == value {
            " selected"
        } else {
            ""
        };
        out.push_str(&format!(
            "<option value=\"{value}\"{selected}>{label}</option>"
        ));
    }
    out.push_str("</select>");
    out.push_str(&format!(
        "<div class=\"small\" id=\"peerProfileSummary\" style=\"margin-top:8px\">{} global / {} per torrent / numwant {}</div>",
        state.peer_profile_global_limit,
        state.peer_profile_torrent_limit,
        state.peer_profile_numwant
    ));
    out.push_str("</div>");
    out.push_str("</div>");
    out.push_str("</div>");

    out.push_str("<details class=\"panel secondary-panel\" id=\"sessionDetails\"><summary>Session &amp; connection</summary>");
    out.push_str("<div class=\"session-stats\">");
    out.push_str(&format!("<div class=\"session-row\"><span>Downloaded</span><span class=\"session-value\">{total_downloaded}</span></div>"));
    out.push_str(&format!("<div class=\"session-row\"><span>Uploaded</span><span class=\"session-value\">{total_uploaded}</span></div>"));
    out.push_str(&format!(
        "<div class=\"session-row\"><span>Peers</span><span class=\"session-value\">{total_active_peers} / {total_tracker_peers}</span></div>"
    ));
    out.push_str(&format!(
        "<div class=\"session-row\"><span>Connections</span><span class=\"session-value\">+{} / -{}</span></div>",
        state.peer_connected, state.peer_disconnected
    ));
    if state.incoming_port > 0 {
        out.push_str(&format!(
            "<div class=\"session-row\"><span>Incoming Port</span><span class=\"session-value\">{}</span></div>",
            state.incoming_port
        ));
        let all_mapping_statuses = [&state.natpmp_status, &state.upnp_status];
        let successful_mapping_statuses = all_mapping_statuses
            .iter()
            .copied()
            .filter(|status| status.starts_with("mapped "))
            .collect::<Vec<_>>();
        let visible_mapping_statuses = if successful_mapping_statuses.is_empty() {
            all_mapping_statuses.as_slice()
        } else {
            successful_mapping_statuses.as_slice()
        };
        let mapping_html = visible_mapping_statuses
            .iter()
            .map(|status| format!("<span>{}</span>", escape_html(status)))
            .collect::<String>();
        out.push_str(&format!(
            "<div class=\"session-row stack\"><span>Port Mapping</span><span class=\"session-value\">{mapping_html}</span></div>"
        ));
    }
    out.push_str(&format!(
        "<div class=\"session-row\"><span>Disk I/O</span><span class=\"session-value\">{:.1}ms / {:.1}ms</span></div>",
        state.disk_read_ms_avg, state.disk_write_ms_avg
    ));
    if !state.proxy_label.is_empty() {
        out.push_str(&format!(
            "<div class=\"session-row\"><span>Proxy</span><span class=\"session-value\">{}</span></div>",
            escape_html(&state.proxy_label)
        ));
    }
    out.push_str("</div>");
    out.push_str("</details>");

    // RSS panel
    out.push_str("<details class=\"panel secondary-panel\" id=\"rssDetails\"><summary>RSS subscriptions</summary>");
    out.push_str("<div class=\"panel-title\"><svg class=\"material-symbols-rounded\" style=\"font-size:16px;vertical-align:-3px;margin-right:4px\"><use href=\"#i-rss_feed\"></use></svg>RSS Feeds</div>");
    out.push_str("<form class=\"rss-form\" onsubmit=\"addRssFeed(event)\">");
    out.push_str("<input id=\"rssUrl\" class=\"input\" placeholder=\"Feed URL\">");
    out.push_str("<button type=\"submit\" class=\"btn primary\">Add</button>");
    out.push_str("</form>");
    {
        use crate::RSS_STATE;
        if let Some(lock) = RSS_STATE.get() {
            if let Ok(rss_state) = lock.lock() {
                if !rss_state.feeds.is_empty() {
                    out.push_str("<div class=\"rss-list\">");
                    for feed in &rss_state.feeds {
                        let title = if feed.title.is_empty() {
                            &feed.url
                        } else {
                            &feed.title
                        };
                        out.push_str(&format!(
                            "<div class=\"rss-item\"><span class=\"rss-item-info\" title=\"{}\">{}</span><span class=\"rss-item-meta\">{} items</span><button class=\"remove-btn\" onclick=\"removeRssFeed('{}')\" title=\"Remove\">\u{00d7}</button></div>",
                            escape_html(&feed.url),
                            escape_html(title),
                            feed.items.len(),
                            escape_html(&escape_js_single_quoted(&feed.url)),
                        ));
                    }
                    out.push_str("</div>");
                }
                // Rules section
                out.push_str("<div class=\"rss-section-label\">Rules</div>");
                if !rss_state.rules.is_empty() {
                    out.push_str("<div class=\"rss-list\">");
                    for rule in &rss_state.rules {
                        out.push_str(&format!(
                            "<div class=\"rss-item\"><span class=\"rss-item-info\">{}: {}</span><button class=\"remove-btn\" onclick=\"removeRssRule('{}')\" title=\"Remove\">\u{00d7}</button></div>",
                            escape_html(&rule.name),
                            escape_html(&rule.pattern),
                            escape_html(&escape_js_single_quoted(&rule.name)),
                        ));
                    }
                    out.push_str("</div>");
                }
                // Add rule form
                out.push_str("<form class=\"rss-form\" onsubmit=\"addRssRule(event)\">");
                out.push_str(
                    "<input id=\"rssRuleName\" class=\"input\" placeholder=\"Rule name\">",
                );
                out.push_str(
                    "<input id=\"rssRulePattern\" class=\"input\" placeholder=\"Pattern\">",
                );
                out.push_str("<button type=\"submit\" class=\"btn primary\">Add</button>");
                out.push_str("</form>");
            }
        }
    }
    out.push_str("</details>");

    out.push_str("</aside>");

    out.push_str("<main class=\"torrent-list\">");
    out.push_str(&format!(
        "<div class=\"library-heading\"><div><h1>Transfers</h1><p class=\"small\">Your files, from first piece to finished download.</p></div><span class=\"library-count\">{total_torrents} in library</span></div>"
    ));
    if !state.torrents.is_empty() {
        out.push_str(&format!(
            "<div class=\"library-summary\"><span class=\"signal-dot {fleet_signal}\"></span><strong>{fleet_state}</strong><span>{}</span></div>",
            escape_html(fleet_copy)
        ));
    }
    out.push_str("<div class=\"filter-empty\" id=\"filterEmpty\" hidden><p>No transfers match this view.</p><button class=\"btn\" type=\"button\" onclick=\"clearLibraryFilters()\">Show all transfers</button></div>");
    if state.torrents.is_empty() {
        out.push_str("<div class=\"panel empty-state\">");
        out.push_str(
            "<svg class=\"material-symbols-rounded\"><use href=\"#i-cloud_download\"></use></svg>",
        );
        out.push_str("<h2>Ready when you are</h2><p>Add a .torrent file or paste a magnet link. Choose where to save it, and we'll take care of the pieces.</p><button class=\"btn primary\" type=\"button\" onclick=\"openAdd()\"><svg class=\"material-symbols-rounded\" aria-hidden=\"true\"><use href=\"#i-add\"></use></svg>Add your first torrent</button><span class=\"small\">You can also drop a .torrent file anywhere.</span>");
        out.push_str("</div>");
    } else {
        for (card_index, torrent) in state.torrents.iter().enumerate() {
            let name = if torrent.name.is_empty() {
                "(unknown)"
            } else {
                &torrent.name
            };
            let name = escape_html(name);
            let status_raw = torrent.status.as_str();
            let bucket = bucket_for(torrent);
            let status_display = if torrent.paused {
                "paused"
            } else if bucket == "complete" {
                "seeding"
            } else if matches!(status_raw, "complete" | "seeding") {
                "downloading"
            } else {
                status_raw
            };
            let status = escape_html(status_display);
            let status_class = format!("status-{bucket}");
            let info_hash = escape_html(&torrent.info_hash);
            let download_dir = escape_html(&torrent.download_dir);
            let total_bytes = human_bytes(torrent.total_bytes);
            let mut completed_bytes = if torrent.completed_bytes > 0 || torrent.total_pieces == 0 {
                torrent.completed_bytes
            } else {
                torrent
                    .total_bytes
                    .saturating_mul(torrent.completed_pieces as u64)
                    / torrent.total_pieces.max(1) as u64
            };
            if is_complete_torrent(torrent) && torrent.total_bytes > 0 {
                completed_bytes = torrent.total_bytes;
            }
            let completed_label = human_bytes(completed_bytes.min(torrent.total_bytes));
            let downloaded_label = human_bytes(torrent.downloaded_bytes);
            let uploaded_label = human_bytes(torrent.uploaded_bytes);
            let ratio = format_ratio(torrent.uploaded_bytes, torrent.downloaded_bytes);
            let speed = human_rate(torrent.download_rate_bps);
            let upload_rate = human_rate(torrent.upload_rate_bps);
            let eta = format_eta_secs(torrent.eta_secs);
            let pct = percent(completed_bytes, torrent.total_bytes);
            let pct_value = (pct as f64 / 100.0).min(100.0);
            let peers = format!(
                "{} connected / {} known",
                torrent.active_peers, torrent.tracker_peers
            );
            let pieces = format!("{}/{}", torrent.completed_pieces, torrent.total_pieces);
            let progress_note = if torrent.paused || matches!(status_raw, "stopped" | "stopping") {
                "Transfer paused"
            } else if bucket == "complete" {
                if torrent.upload_rate_bps > 0.0 {
                    "Sharing with peers"
                } else {
                    "Ready to share when peers request pieces"
                }
            } else if matches!(status_raw, "complete" | "seeding") {
                "Verification pending"
            } else if torrent.active_peers > 0 {
                "Peers connected"
            } else if torrent.tracker_peers > 0 {
                "Finding reachable peers"
            } else {
                "Looking for peers"
            };
            let progress_class =
                if torrent.total_bytes > 0 && completed_bytes >= torrent.total_bytes {
                    "progress good"
                } else {
                    "progress"
                };
            let preallocate = if torrent.preallocate { "true" } else { "false" };
            let is_stopping = status_raw == "stopping";
            let can_resume = torrent.paused || status_raw == "stopped";
            let paused = if can_resume { "true" } else { "false" };
            let pause_label = if can_resume { "Resume" } else { "Pause" };
            let pause_disabled =
                is_stopping || matches!(status_raw, "queued" | "loading" | "fetching metadata");
            let pause_attrs = if is_stopping {
                " disabled title=\"Torrent is stopping\""
            } else if pause_disabled {
                " disabled title=\"Not available while torrent is initializing\""
            } else {
                ""
            };
            let stop_disabled =
                is_stopping || matches!(status_raw, "loading" | "fetching metadata");
            let stop_attrs = if is_stopping {
                " disabled title=\"Torrent is stopping\""
            } else if stop_disabled {
                " disabled title=\"Not available while metadata is loading\""
            } else {
                ""
            };
            let priority_disabled =
                matches!(status_raw, "queued" | "loading" | "fetching metadata");
            let priority_attrs = if priority_disabled {
                " disabled title=\"Priority can be changed after metadata is ready\""
            } else {
                ""
            };

            out.push_str(&format!(
                "<section class=\"panel torrent-card\" style=\"--card-index:{card_index}\" data-status=\"{bucket}\" data-id=\"{id}\" data-info-hash=\"{info_hash}\" data-name=\"{name}\" data-paused=\"{paused}\" data-label=\"{label}\" data-collapsed=\"true\">",
                id = torrent.id,
                card_index = card_index,
                label = escape_html(&torrent.label)
            ));
            out.push_str("<div class=\"torrent-head\">");
            out.push_str("<div>");
            out.push_str(&format!("<div class=\"torrent-title\">{name}</div>"));
            out.push_str(&format!(
                "<div class=\"torrent-sub\"><span class=\"status-pill {status_class}\">{status}</span><span class=\"torrent-size\">{total_bytes}</span></div>"
            ));
            out.push_str("</div>");
            let pause_icon = if can_resume { "play_arrow" } else { "pause" };
            out.push_str("<div class=\"torrent-actions\">");
            out.push_str(&format!(
                "<button class=\"btn ghost\" type=\"button\" data-action=\"toggle-pause\"{pause_attrs}><svg class=\"material-symbols-rounded\"><use href=\"#i-{pause_icon}\"></use></svg>{pause_label}</button>"
            ));
            out.push_str("<button class=\"btn ghost\" type=\"button\" data-action=\"toggle-expand\"><svg class=\"material-symbols-rounded\"><use href=\"#i-unfold_more\"></use></svg>Expand</button>");
            out.push_str("<button class=\"btn ghost\" type=\"button\" data-action=\"open-folder\"><svg class=\"material-symbols-rounded\"><use href=\"#i-folder_open\"></use></svg>Open Folder</button>");
            let stop_label = if is_stopping { "Stopping..." } else { "Stop" };
            out.push_str(&format!(
                "<button class=\"btn ghost\" type=\"button\" data-action=\"stop\"{stop_attrs}><svg class=\"material-symbols-rounded\"><use href=\"#i-stop\"></use></svg>{stop_label}</button>"
            ));
            out.push_str("<button class=\"btn ghost\" type=\"button\" data-action=\"archive\"><svg class=\"material-symbols-rounded\"><use href=\"#i-archive\"></use></svg>Archive</button>");
            out.push_str("<button class=\"btn danger\" type=\"button\" data-action=\"delete\"><svg class=\"material-symbols-rounded\"><use href=\"#i-delete\"></use></svg>Remove</button>");
            out.push_str("</div>");
            out.push_str("</div>");
            if !torrent.last_error.is_empty() && bucket == "error" {
                out.push_str(&format!(
                    "<div class=\"torrent-error\" role=\"alert\">{}</div>",
                    escape_html(&torrent.last_error)
                ));
            }
            out.push_str(&format!(
                "<div class=\"torrent-progress\"><div class=\"torrent-progress-top\"><div class=\"meta\">Progress {completed_label} / {total_bytes} ({:.2}%)</div><div class=\"torrent-progress-note\">{progress_note}</div></div>",
                pct_value
            ));
            out.push_str(&format!(
                "<div class=\"{progress_class}\"><div class=\"fill\" style=\"width:{:.2}%\"></div></div></div>",
                pct_value
            ));
            out.push_str(&format!(
                "<div class=\"torrent-quick\"><span><svg class=\"material-symbols-rounded\"><use href=\"#i-download\"></use></svg>{speed}</span><span><svg class=\"material-symbols-rounded\"><use href=\"#i-upload\"></use></svg>{upload_rate}</span><span><svg class=\"material-symbols-rounded\"><use href=\"#i-group\"></use></svg>{peers}</span><span><svg class=\"material-symbols-rounded\"><use href=\"#i-schedule\"></use></svg>{eta}</span></div>"
            ));
            out.push_str("<div class=\"torrent-stats\">");
            out.push_str(&format!(
                "<div class=\"stat\"><span class=\"k\"><svg class=\"material-symbols-rounded\"><use href=\"#i-download\"></use></svg>Down</span><span class=\"v\">{speed}</span></div>"
            ));
            out.push_str(&format!(
                "<div class=\"stat\"><span class=\"k\"><svg class=\"material-symbols-rounded\"><use href=\"#i-upload\"></use></svg>Up</span><span class=\"v\">{upload_rate}</span></div>"
            ));
            out.push_str(&format!(
                "<div class=\"stat\"><span class=\"k\"><svg class=\"material-symbols-rounded\"><use href=\"#i-swap_vert\"></use></svg>Ratio</span><span class=\"v\">{ratio}</span></div>"
            ));
            out.push_str(&format!(
                "<div class=\"stat\"><span class=\"k\"><svg class=\"material-symbols-rounded\"><use href=\"#i-schedule\"></use></svg>ETA</span><span class=\"v\">{eta}</span></div>"
            ));
            out.push_str(&format!(
                "<div class=\"stat\"><span class=\"k\"><svg class=\"material-symbols-rounded\"><use href=\"#i-group\"></use></svg>Peers</span><span class=\"v\">{peers}</span></div>"
            ));
            out.push_str(&format!(
                "<div class=\"stat\"><span class=\"k\"><svg class=\"material-symbols-rounded\"><use href=\"#i-grid_view\"></use></svg>Pieces</span><span class=\"v\">{pieces}</span></div>"
            ));
            out.push_str("</div>");

            out.push_str("<div class=\"torrent-grid\">");
            out.push_str("<div class=\"detail-actions\">");
            out.push_str(&format!("<button class=\"btn ghost\" type=\"button\" data-action=\"stop\"{stop_attrs}><svg class=\"material-symbols-rounded\"><use href=\"#i-stop\"></use></svg>{stop_label}</button>"));
            out.push_str("<button class=\"btn ghost\" type=\"button\" data-action=\"recheck\"><svg class=\"material-symbols-rounded\"><use href=\"#i-verified\"></use></svg>Recheck</button>");
            out.push_str("</div>");
            out.push_str("<div class=\"subpanel\">");
            out.push_str("<div class=\"panel-title\">General</div>");
            out.push_str("<div class=\"kv\">");
            out.push_str(&format!(
                "<div class=\"k\">Info hash</div><div>{info_hash}</div>"
            ));
            {
                let version_label = match torrent.meta_version {
                    2 => "v2",
                    3 => "Hybrid",
                    _ => "v1",
                };
                out.push_str(&format!(
                    "<div class=\"k\">Version</div><div>{version_label}</div>"
                ));
            }
            out.push_str(&format!(
                "<div class=\"k\">Download dir</div><div>{download_dir}</div>"
            ));
            out.push_str(&format!(
                "<div class=\"k\">Preallocate</div><div>{preallocate}</div>"
            ));
            out.push_str(&format!(
                "<div class=\"k\">Downloaded</div><div>{downloaded_label}</div>"
            ));
            out.push_str(&format!(
                "<div class=\"k\">Uploaded</div><div>{uploaded_label}</div>"
            ));
            out.push_str(&format!("<div class=\"k\">Ratio</div><div>{ratio}</div>"));
            out.push_str(&format!(
                "<div class=\"k\">Label</div><div class=\"inline-form\"><input class=\"label-input input\" type=\"text\" value=\"{}\" placeholder=\"none\"><button class=\"btn\" data-action=\"set-label\">Set</button></div>",
                escape_html(&torrent.label)
            ));
            out.push_str("</div>");
            out.push_str("</div>");

            // Peer countries panel
            if !torrent.peer_country_counts.is_empty() {
                out.push_str("<div class=\"subpanel\">");
                out.push_str("<div class=\"panel-title\">Peers by Country</div>");
                out.push_str("<div class=\"country-tags\">");
                for (cc, count) in &torrent.peer_country_counts {
                    let flag = crate::geoip::country_flag(cc);
                    out.push_str(&format!(
                        "<span class=\"country-tag\">{} {} <b>{count}</b></span>",
                        escape_html(&flag),
                        escape_html(cc),
                    ));
                }
                out.push_str("</div></div>");
            }

            // Trackers panel
            out.push_str("<div class=\"subpanel\">");
            out.push_str("<div class=\"panel-title\">Trackers</div>");
            if !torrent.trackers.is_empty() {
                out.push_str("<div class=\"tracker-list\">");
                for tracker in &torrent.trackers {
                    out.push_str(&format!(
                        "<div class=\"tracker-item\"><span>{}</span><button class=\"remove-btn\" data-action=\"remove-tracker\" data-url=\"{}\" title=\"Remove\">\u{00d7}</button></div>",
                        escape_html(tracker),
                        escape_html(tracker)
                    ));
                }
                out.push_str("</div>");
            }
            out.push_str("<div class=\"inline-form\"><input class=\"tracker-add-input input\" type=\"text\" placeholder=\"https://... or udp://...\"><button class=\"btn\" data-action=\"add-tracker\">Add</button></div>");
            out.push_str("</div>");

            out.push_str("<div class=\"subpanel\">");
            out.push_str("<div class=\"panel-title\">Files</div>");
            if torrent.files.is_empty() {
                out.push_str("<div class=\"small\" style=\"margin-top:8px\">No files.</div>");
            } else {
                out.push_str("<table class=\"table\"><thead><tr><th>File</th><th>Size</th><th>Done</th><th>Progress</th><th>Priority</th></tr></thead><tbody>");
                for (idx, file) in torrent.files.iter().enumerate() {
                    let file_name = escape_html(&file.path);
                    let size = human_bytes(file.length);
                    let done = human_bytes(file.completed);
                    let file_pct = percent(file.completed, file.length);
                    let file_pct_value = (file_pct as f64 / 100.0).min(100.0);
                    let priority = file.priority;
                    let priority_select = format!(
                        "<select class=\"input\" onchange=\"setPriority({id},{idx}, this.value)\"{priority_attrs}><option value=\"0\"{}>Skip</option><option value=\"1\"{}>Low</option><option value=\"2\"{}>Normal</option><option value=\"3\"{}>High</option></select>",
                        if priority == 0 { " selected" } else { "" },
                        if priority == 1 { " selected" } else { "" },
                        if priority == 2 { " selected" } else { "" },
                        if priority == 3 { " selected" } else { "" },
                        priority_attrs = priority_attrs,
                        id = torrent.id,
                        idx = idx
                    );
                    let tid = torrent.id;
                    out.push_str(&format!(
                        "<tr><td class=\"file-cell\" ondblclick=\"startRename({tid},{idx},this)\" title=\"Double-click to rename\">{file_name}</td><td>{size}</td><td>{done}</td><td><div class=\"file-bar\"><div class=\"fill\" style=\"width:{file_pct_value:.2}%\"></div></div></td><td>{priority_select}</td></tr>"
                    ));
                }
                out.push_str("</tbody></table>");
            }
            out.push_str("</div>");
            out.push_str("</div>");
            out.push_str("</section>");
        }
    }
    out.push_str("</main>");

    out.push_str("</div>");
    out.push_str("<div class=\"layout workspace search-layout\" data-main-tab=\"search\">");
    out.push_str("<aside class=\"sidebar search-sidebar\">");
    render_search_panel(&mut out);
    out.push_str("</aside>");
    out.push_str("<main class=\"torrent-list search-main\">");
    render_search_results_panel(&mut out);
    render_search_plugin_manager(&mut out);
    out.push_str("</main>");
    out.push_str("</div>");
    out.push_str("<div id=\"addModal\" class=\"modal\" onclick=\"maybeClose(event)\">");
    out.push_str("<div class=\"modal-card\">");
    out.push_str("<div class=\"modal-head\"><div class=\"modal-title\">Add Torrent</div><button class=\"btn icon-btn ghost\" type=\"button\" title=\"Close\" aria-label=\"Close\" onclick=\"closeAdd()\"><svg class=\"material-symbols-rounded\" aria-hidden=\"true\"><use href=\"#i-close\"></use></svg></button></div>");
    out.push_str("<div class=\"add-grid\">");
    // Drop zone for .torrent file
    out.push_str("<div id=\"dropZone\" class=\"drop-zone\" onclick=\"document.getElementById('torrentFile').click()\">");
    out.push_str("<input id=\"torrentFile\" type=\"file\" accept=\".torrent\" onchange=\"handleTorrentInputChange()\" onclick=\"event.stopPropagation()\">");
    out.push_str(
        "<svg class=\"material-symbols-rounded dz-icon\"><use href=\"#i-upload_file\"></use></svg>",
    );
    out.push_str("<span class=\"dz-text\">Drop .torrent file here or click to browse</span>");
    out.push_str("<span class=\"dz-hint\">.torrent files only</span>");
    out.push_str("<div class=\"dz-file-info\"><svg class=\"material-symbols-rounded dz-icon\"><use href=\"#i-description\"></use></svg><span id=\"dzFileName\" class=\"dz-file-name\"></span><button type=\"button\" class=\"dz-file-remove\" onclick=\"event.stopPropagation();clearTorrentFile()\" title=\"Remove file\"><svg class=\"material-symbols-rounded\" style=\"font-size:18px\"><use href=\"#i-close\"></use></svg></button></div>");
    out.push_str("</div>");
    // Divider
    out.push_str("<div class=\"add-divider\">or paste a magnet link</div>");
    // Magnet input
    out.push_str("<input id=\"magnet\" class=\"input\" type=\"text\" placeholder=\"magnet:?xt=urn:btih:...\" autocomplete=\"off\" oninput=\"handleMagnetInput()\">");
    // Summary & review
    out.push_str("<div id=\"addSummary\" class=\"add-summary\">Select a .torrent file or paste a magnet link.</div>");
    out.push_str("<div id=\"addReview\" class=\"add-review\" style=\"display:none\"></div>");
    // Options section
    out.push_str("<div class=\"add-opts\">");
    out.push_str("<div><div class=\"add-field-label\">Save to</div>");
    out.push_str("<div class=\"add-download-row\">");
    out.push_str(&format!(
        "<input id=\"downloadDir\" class=\"input\" type=\"text\" placeholder=\"Download directory\" value=\"{download_dir}\">"
    ));
    out.push_str("<button class=\"btn ghost\" type=\"button\" title=\"Browse for folder\" aria-label=\"Browse for folder\" onclick=\"chooseDownloadDir()\"><svg class=\"material-symbols-rounded\" style=\"font-size:18px\" aria-hidden=\"true\"><use href=\"#i-folder_open\"></use></svg></button>");
    out.push_str("</div></div>");
    out.push_str(&format!(
        "<div class=\"add-prefs\"><label class=\"add-check\"><input id=\"preallocate\" type=\"checkbox\" {}> Preallocate</label><label class=\"add-check\"><input id=\"startWhenAdded\" type=\"checkbox\" checked> Start immediately</label></div>",
        if state.preallocate { "checked" } else { "" }
    ));
    out.push_str("</div>");
    // Actions
    out.push_str("<div class=\"modal-actions\"><button class=\"btn ghost\" type=\"button\" onclick=\"closeAdd()\">Cancel</button><button class=\"btn primary\" type=\"button\" onclick=\"return submitAdd();\"><svg class=\"material-symbols-rounded\" style=\"font-size:18px\"><use href=\"#i-add\"></use></svg> Add Torrent</button></div>");
    out.push_str("</div>");
    out.push_str("</div>");
    out.push_str("</div>");
    // Page-level drop overlay
    out.push_str("<div id=\"pageDropOverlay\" class=\"page-drop-overlay\"><div class=\"page-drop-overlay-inner\"><svg class=\"material-symbols-rounded\"><use href=\"#i-cloud_upload\"></use></svg><p>Drop to add torrent</p></div></div>");
    out
}

fn status_json(state: &UiState) -> String {
    let overall_percent = percent(state.completed_bytes, state.total_bytes);
    let ratio = ratio_value(state.uploaded_bytes, state.downloaded_bytes);
    let current_id_json = state
        .current_id
        .map(|id| id.to_string())
        .unwrap_or_else(|| "null".to_string());
    let mut files_json = String::new();
    files_json.push('[');
    for (idx, file) in state.files.iter().enumerate() {
        if idx > 0 {
            files_json.push(',');
        }
        let file_percent = percent(file.completed, file.length);
        files_json.push_str(&format!(
            "{{\"path\":\"{}\",\"length\":{},\"completed\":{},\"percent\":{},\"priority\":{}}}",
            escape_json(&file.path),
            file.length,
            file.completed,
            file_percent,
            file.priority
        ));
    }
    files_json.push(']');
    let mut torrents_json = String::new();
    torrents_json.push('[');
    for (idx, torrent) in state.torrents.iter().enumerate() {
        if idx > 0 {
            torrents_json.push(',');
        }
        let percent_done = percent(torrent.completed_bytes, torrent.total_bytes);
        let ratio = ratio_value(torrent.uploaded_bytes, torrent.downloaded_bytes);
        let mut torrent_files_json = String::new();
        torrent_files_json.push('[');
        for (file_idx, file) in torrent.files.iter().enumerate() {
            if file_idx > 0 {
                torrent_files_json.push(',');
            }
            let file_percent = percent(file.completed, file.length);
            torrent_files_json.push_str(&format!(
                "{{\"path\":\"{}\",\"length\":{},\"completed\":{},\"percent\":{},\"priority\":{}}}",
                escape_json(&file.path),
                file.length,
                file.completed,
                file_percent,
                file.priority
            ));
        }
        torrent_files_json.push(']');
        let mut trackers_json = String::from("[");
        for (ti, t) in torrent.trackers.iter().enumerate() {
            if ti > 0 {
                trackers_json.push(',');
            }
            trackers_json.push('"');
            trackers_json.push_str(&escape_json(t));
            trackers_json.push('"');
        }
        trackers_json.push(']');
        let mut countries_json = String::from("[");
        for (ci, (cc, count)) in torrent.peer_country_counts.iter().enumerate() {
            if ci > 0 {
                countries_json.push(',');
            }
            countries_json.push_str(&format!(
                "{{\"code\":\"{}\",\"count\":{}}}",
                escape_json(cc),
                count
            ));
        }
        countries_json.push(']');
        torrents_json.push_str(&format!(
            "{{\"id\":{},\"name\":\"{}\",\"info_hash\":\"{}\",\"download_dir\":\"{}\",\"preallocate\":{},\"status\":\"{}\",\"total_bytes\":{},\"completed_bytes\":{},\"downloaded_bytes\":{},\"uploaded_bytes\":{},\"ratio\":{:.3},\"total_pieces\":{},\"completed_pieces\":{},\"percent\":{},\"download_rate_bps\":{:.2},\"upload_rate_bps\":{:.2},\"eta_secs\":{},\"tracker_peers\":{},\"active_peers\":{},\"interested_peers\":{},\"upload_requests_served\":{},\"paused\":{},\"last_error\":\"{}\",\"label\":\"{}\",\"trackers\":{},\"files\":{},\"peer_countries\":{}}}",
            torrent.id,
            escape_json(&torrent.name),
            escape_json(&torrent.info_hash),
            escape_json(&torrent.download_dir),
            torrent.preallocate,
            escape_json(&torrent.status),
            torrent.total_bytes,
            torrent.completed_bytes,
            torrent.downloaded_bytes,
            torrent.uploaded_bytes,
            ratio,
            torrent.total_pieces,
            torrent.completed_pieces,
            percent_done,
            torrent.download_rate_bps,
            torrent.upload_rate_bps,
            torrent.eta_secs,
            torrent.tracker_peers,
            torrent.active_peers,
            torrent.interested_peers,
            torrent.upload_requests_served,
            torrent.paused,
            escape_json(&torrent.last_error),
            escape_json(&torrent.label),
            trackers_json,
            torrent_files_json,
            countries_json
        ));
    }
    torrents_json.push(']');
    format!(
        "{{\"version\":\"{}\",\"name\":\"{}\",\"info_hash\":\"{}\",\"download_dir\":\"{}\",\"status\":\"{}\",\"last_error\":\"{}\",\"total_pieces\":{},\"completed_pieces\":{},\"total_bytes\":{},\"completed_bytes\":{},\"downloaded_bytes\":{},\"uploaded_bytes\":{},\"ratio\":{:.3},\"percent\":{},\"tracker_peers\":{},\"active_peers\":{},\"interested_peers\":{},\"upload_requests_served\":{},\"preallocate\":{},\"paused\":{},\"download_rate_bps\":{:.2},\"upload_rate_bps\":{:.2},\"eta_secs\":{},\"incoming_port\":{},\"natpmp_status\":\"{}\",\"upnp_status\":\"{}\",\"queue_len\":{},\"last_added\":\"{}\",\"current_id\":{},\"peer_connected\":{},\"peer_disconnected\":{},\"disk_read_ms_avg\":{:.3},\"disk_write_ms_avg\":{:.3},\"session_downloaded_bytes\":{},\"session_uploaded_bytes\":{},\"global_download_limit_bps\":{},\"global_upload_limit_bps\":{},\"seed_ratio\":{:.2},\"files\":{},\"torrents\":{}}}",
        env!("CARGO_PKG_VERSION"),
        escape_json(&state.name),
        escape_json(&state.info_hash),
        escape_json(&state.download_dir),
        escape_json(&state.status),
        escape_json(&state.last_error),
        state.total_pieces,
        state.completed_pieces,
        state.total_bytes,
        state.completed_bytes,
        state.downloaded_bytes,
        state.uploaded_bytes,
        ratio,
        overall_percent,
        state.tracker_peers,
        state.active_peers,
        state.interested_peers,
        state.upload_requests_served,
        state.preallocate,
        state.paused,
        state.download_rate_bps,
        state.upload_rate_bps,
        state.eta_secs,
        state.incoming_port,
        escape_json(&state.natpmp_status),
        escape_json(&state.upnp_status),
        state.queue_len,
        escape_json(&state.last_added),
        current_id_json,
        state.peer_connected,
        state.peer_disconnected,
        state.disk_read_ms_avg,
        state.disk_write_ms_avg,
        state.session_downloaded_bytes,
        state.session_uploaded_bytes,
        state.global_download_limit_bps,
        state.global_upload_limit_bps,
        state.seed_ratio,
        files_json,
        torrents_json
    )
}

fn percent(done: u64, total: u64) -> u64 {
    done.saturating_mul(10_000).checked_div(total).unwrap_or(0)
}

fn ratio_value(uploaded: u64, downloaded: u64) -> f64 {
    if downloaded == 0 {
        0.0
    } else {
        uploaded as f64 / downloaded as f64
    }
}

fn format_ratio(uploaded: u64, downloaded: u64) -> String {
    if downloaded == 0 {
        if uploaded == 0 {
            "0.00".to_string()
        } else {
            "inf".to_string()
        }
    } else {
        format!("{:.2}", uploaded as f64 / downloaded as f64)
    }
}

fn human_bytes(value: u64) -> String {
    const UNITS: [&str; 5] = ["B", "KB", "MB", "GB", "TB"];
    let mut size = value as f64;
    let mut unit = 0;
    while size >= 1024.0 && unit + 1 < UNITS.len() {
        size /= 1024.0;
        unit += 1;
    }
    if unit == 0 {
        format!("{value} {}", UNITS[unit])
    } else {
        format!("{:.2} {}", size, UNITS[unit])
    }
}

fn human_rate(bps: f64) -> String {
    const UNITS: [&str; 5] = ["B/s", "KB/s", "MB/s", "GB/s", "TB/s"];
    if !bps.is_finite() || bps <= 0.0 {
        return "0 B/s".to_string();
    }
    let mut size = bps;
    let mut unit = 0;
    while size >= 1024.0 && unit + 1 < UNITS.len() {
        size /= 1024.0;
        unit += 1;
    }
    if unit == 0 {
        format!("{:.0} {}", size, UNITS[unit])
    } else {
        format!("{:.2} {}", size, UNITS[unit])
    }
}

fn render_speed_chart(download: &[f64], upload: &[f64]) -> String {
    let sample_count = download.len().max(upload.len());
    let latest_down = human_rate(*download.last().unwrap_or(&0.0));
    let latest_up = human_rate(*upload.last().unwrap_or(&0.0));
    if sample_count < 2 {
        return format!(
            "<div class=\"speed-chart\"><div class=\"speed-chart-empty\">Collecting speed samples...</div><div class=\"speed-legend\"><span class=\"item\"><span class=\"swatch down\"></span>Download {latest_down}</span><span class=\"item\"><span class=\"swatch up\"></span>Upload {latest_up}</span></div></div>"
        );
    }

    let width = 720.0;
    let height = 120.0;
    let pad_x = 8.0;
    let pad_y = 10.0;
    let plot_width = width - (pad_x * 2.0);
    let plot_height = height - (pad_y * 2.0);
    let max_rate = download
        .iter()
        .chain(upload.iter())
        .copied()
        .fold(0.0f64, f64::max)
        .max(1.0);

    let mut grid = String::new();
    for idx in 0..=3 {
        let y = pad_y + (plot_height / 3.0) * idx as f64;
        grid.push_str(&format!(
            "<line x1=\"{pad_x:.1}\" y1=\"{y:.1}\" x2=\"{x2:.1}\" y2=\"{y:.1}\" stroke=\"rgba(116,119,127,0.25)\" stroke-width=\"1\" />",
            x2 = width - pad_x
        ));
    }

    let build_points = |values: &[f64]| -> String {
        if values.is_empty() {
            return String::new();
        }
        let start = sample_count.saturating_sub(values.len());
        let denom = sample_count.saturating_sub(1).max(1) as f64;
        let mut out = String::new();
        for (idx, value) in values.iter().enumerate() {
            let x = pad_x + ((start + idx) as f64 / denom) * plot_width;
            let normalized = (*value / max_rate).clamp(0.0, 1.0);
            let y = pad_y + (1.0 - normalized) * plot_height;
            if !out.is_empty() {
                out.push(' ');
            }
            out.push_str(&format!("{x:.1},{y:.1}"));
        }
        out
    };

    let max_label = human_rate(max_rate);
    format!(
        "<div class=\"speed-chart\"><svg viewBox=\"0 0 {width:.0} {height:.0}\" preserveAspectRatio=\"none\" aria-label=\"transfer speed chart\"><text x=\"{label_x:.1}\" y=\"{label_y:.1}\" fill=\"currentColor\" opacity=\"0.65\" font-size=\"10\" text-anchor=\"end\">{max_label}</text>{grid}<polyline fill=\"none\" stroke=\"var(--primary)\" stroke-width=\"2.5\" stroke-linecap=\"round\" stroke-linejoin=\"round\" points=\"{down_points}\" /><polyline fill=\"none\" stroke=\"var(--tertiary)\" stroke-width=\"2.5\" stroke-linecap=\"round\" stroke-linejoin=\"round\" points=\"{up_points}\" /></svg><div class=\"speed-legend\"><span class=\"item\"><span class=\"swatch down\"></span>Download {latest_down}</span><span class=\"item\"><span class=\"swatch up\"></span>Upload {latest_up}</span></div></div>",
        label_x = width - pad_x,
        label_y = pad_y - 2.0,
        down_points = build_points(download),
        up_points = build_points(upload),
    )
}

fn format_eta_secs(secs: u64) -> String {
    if secs == 0 {
        return "--:--".to_string();
    }
    let hours = secs / 3600;
    let minutes = (secs % 3600) / 60;
    let seconds = secs % 60;
    if hours > 0 {
        format!("{:02}:{:02}:{:02}", hours, minutes, seconds)
    } else {
        format!("{:02}:{:02}", minutes, seconds)
    }
}

fn escape_html(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    for ch in input.chars() {
        match ch {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&#39;"),
            _ => out.push(ch),
        }
    }
    out
}

fn escape_js_single_quoted(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    for ch in input.chars() {
        match ch {
            '\\' => out.push_str("\\\\"),
            '\'' => out.push_str("\\'"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\u{2028}' => out.push_str("\\u2028"),
            '\u{2029}' => out.push_str("\\u2029"),
            '<' => out.push_str("\\x3c"),
            '>' => out.push_str("\\x3e"),
            '&' => out.push_str("\\x26"),
            c if c.is_control() => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
    out
}

fn escape_json(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    for ch in input.chars() {
        match ch {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if c.is_control() => out.push_str(&format!("\\u{:04x}", c as u32)),
            _ => out.push(ch),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Read, Write};
    use std::net::{Shutdown, TcpListener, TcpStream};
    use std::sync::{mpsc, Arc, Mutex};
    use std::thread;

    fn torrent_list_is_inside_layout(html: &str) -> bool {
        let mut index = 0usize;
        let mut div_stack: Vec<&'static str> = Vec::new();
        while let Some(start_rel) = html[index..].find('<') {
            let start = index + start_rel;
            let Some(end_rel) = html[start..].find('>') else {
                break;
            };
            let end = start + end_rel + 1;
            let tag = &html[start..end];
            if tag.starts_with("<div") {
                if tag.contains("class=\"layout\"") || tag.contains("class=\"layout ") {
                    div_stack.push("layout");
                } else {
                    div_stack.push("div");
                }
            } else if tag.starts_with("</div") {
                if div_stack.pop().is_none() {
                    return false;
                }
            } else if tag.starts_with("<main") && tag.contains("class=\"torrent-list\"") {
                return div_stack.contains(&"layout");
            }
            index = end;
        }
        false
    }

    fn run_single_request(request_bytes: &[u8], cmd_tx: Option<mpsc::Sender<UiCommand>>) -> String {
        let state = Arc::new(Mutex::new(UiState::default()));
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind test listener");
        let addr = listener.local_addr().expect("listener addr");
        let server_state = Arc::clone(&state);
        let server = thread::spawn(move || {
            let (stream, _) = listener.accept().expect("accept connection");
            handle_connection(stream, server_state, cmd_tx).expect("handle request");
        });

        let mut client = TcpStream::connect(addr).expect("connect test listener");
        client.write_all(request_bytes).expect("write request");
        client.shutdown(Shutdown::Write).expect("shutdown write");
        let mut response = Vec::new();
        client.read_to_end(&mut response).expect("read response");
        server.join().expect("join server");
        String::from_utf8_lossy(&response).into_owned()
    }

    #[test]
    fn status_json_uses_null_for_missing_current_id() {
        let state = UiState::default();
        let json = status_json(&state);
        assert!(json.contains("\"current_id\":null"));
    }

    #[test]
    fn status_json_exposes_seed_upload_diagnostics() {
        let mut state = UiState {
            incoming_port: 6881,
            natpmp_status: "mapped".to_string(),
            upnp_status: "failed: no gateway".to_string(),
            interested_peers: 2,
            upload_requests_served: 7,
            ..Default::default()
        };
        state.torrents.push(UiTorrent {
            id: 1,
            interested_peers: 1,
            upload_requests_served: 3,
            ..Default::default()
        });

        let json = status_json(&state);

        assert!(json.contains("\"incoming_port\":6881"));
        assert!(json.contains("\"natpmp_status\":\"mapped\""));
        assert!(json.contains("\"upnp_status\":\"failed: no gateway\""));
        assert!(json.contains("\"interested_peers\":2"));
        assert!(json.contains("\"upload_requests_served\":7"));
        assert!(json.contains("\"interested_peers\":1"));
        assert!(json.contains("\"upload_requests_served\":3"));
    }

    #[test]
    fn status_for_error_maps_common_cases() {
        assert_eq!(status_for_error("unknown torrent"), 404);
        assert_eq!(status_for_error("torrent already added"), 409);
        assert_eq!(status_for_error("invalid priority"), 400);
        assert_eq!(status_for_error("ui command timeout"), 504);
    }

    #[test]
    fn api_token_json_exposes_the_launcher_owner_secret() {
        let owner_secret = "a1".repeat(32);
        let json = api_token_json("0123456789abcdef0123456789abcdef", &owner_secret);

        assert_eq!(
            json,
            format!(
                "{{\"token\":\"0123456789abcdef0123456789abcdef\",\"owner_secret\":\"{owner_secret}\"}}"
            )
        );
    }

    #[test]
    fn launcher_owner_secret_requires_256_bit_hex() {
        assert!(valid_ui_owner_secret(&"ab".repeat(32)));
        assert!(valid_ui_owner_secret(&"AB".repeat(32)));
        assert!(!valid_ui_owner_secret(&"ab".repeat(31)));
        assert!(!valid_ui_owner_secret(&"ag".repeat(32)));
    }

    #[test]
    fn dns_rebinding_host_cannot_read_api_token() {
        let request = b"GET /api-token HTTP/1.1\r\nHost: attacker.example:8080\r\n\r\n";
        let response = run_single_request(request, None);
        assert!(response.starts_with("HTTP/1.1 403 Forbidden"));
        assert!(response.contains("forbidden host"));
        assert!(!response.contains(api_token()));
    }

    #[test]
    fn dns_rebinding_host_cannot_mutate_with_a_stolen_token() {
        let request = format!(
            "POST /torrent/pause?id=1 HTTP/1.1\r\nHost: attacker.example:8080\r\nOrigin: http://attacker.example:8080\r\nX-Rustorrent-Token: {}\r\nContent-Length: 0\r\n\r\n",
            api_token()
        );
        let response = run_single_request(request.as_bytes(), None);
        assert!(response.starts_with("HTTP/1.1 403 Forbidden"));
        assert!(response.contains("forbidden host"));
    }

    #[test]
    fn safe_host_validation_accepts_ip_literals_and_localhost_only() {
        for host in [
            "127.0.0.1:8080",
            "[::1]:8080",
            "192.168.1.4",
            "localhost:8080",
        ] {
            let request = HttpRequest {
                method: "GET".to_string(),
                path: "/".to_string(),
                headers: vec![("host".to_string(), host.to_string())],
                body: Vec::new(),
            };
            assert!(request_has_safe_host(&request), "host rejected: {host}");
        }
        for host in [
            "attacker.example:8080",
            "0.0.0.0:8080",
            "[::]:8080",
            "user@127.0.0.1",
        ] {
            let request = HttpRequest {
                method: "GET".to_string(),
                path: "/".to_string(),
                headers: vec![("host".to_string(), host.to_string())],
                body: Vec::new(),
            };
            assert!(
                !request_has_safe_host(&request),
                "unsafe host accepted: {host}"
            );
        }
    }

    #[test]
    fn post_without_token_is_forbidden() {
        let request = b"POST /torrent/pause?id=1 HTTP/1.1\r\nHost: 127.0.0.1:19001\r\nContent-Length: 0\r\n\r\n";
        let response = run_single_request(request, None);
        assert!(response.starts_with("HTTP/1.1 403 Forbidden"));
        assert!(response.contains("missing or invalid api token"));
    }

    #[test]
    fn unauthorized_post_is_rejected_before_its_declared_body_is_read() {
        let state = Arc::new(Mutex::new(UiState::default()));
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let server_state = Arc::clone(&state);
        let server = thread::spawn(move || {
            let (stream, _) = listener.accept().unwrap();
            handle_connection(stream, server_state, None).unwrap();
        });

        let mut client = TcpStream::connect(addr).unwrap();
        client
            .set_read_timeout(Some(Duration::from_secs(2)))
            .unwrap();
        let request = format!(
            "POST /add-torrent HTTP/1.1\r\nHost: 127.0.0.1:{}\r\nContent-Length: {MAX_REQUEST_BODY_BYTES}\r\n\r\n",
            addr.port()
        );
        client.write_all(request.as_bytes()).unwrap();
        let mut response = Vec::new();
        client.read_to_end(&mut response).unwrap();
        server.join().unwrap();

        let response = String::from_utf8_lossy(&response);
        assert!(response.starts_with("HTTP/1.1 403 Forbidden"));
        assert!(response.contains("missing or invalid api token"));
    }

    #[test]
    fn endpoint_body_limit_is_checked_before_waiting_for_the_body() {
        let state = Arc::new(Mutex::new(UiState::default()));
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let server_state = Arc::clone(&state);
        let server = thread::spawn(move || {
            let (stream, _) = listener.accept().unwrap();
            handle_connection(stream, server_state, None).unwrap();
        });

        let mut client = TcpStream::connect(addr).unwrap();
        client
            .set_read_timeout(Some(Duration::from_secs(2)))
            .unwrap();
        let host = format!("127.0.0.1:{}", addr.port());
        let request = format!(
            "POST /add-magnet HTTP/1.1\r\nHost: {host}\r\nOrigin: http://{host}\r\nX-Rustorrent-Token: {}\r\nContent-Length: {}\r\n\r\n",
            api_token(),
            MAX_FORM_BODY_BYTES + 1
        );
        client.write_all(request.as_bytes()).unwrap();
        let mut response = Vec::new();
        client.read_to_end(&mut response).unwrap();
        server.join().unwrap();

        let response = String::from_utf8_lossy(&response);
        assert!(response.starts_with("HTTP/1.1 400 Bad Request"));
        assert!(response.contains("bad request"));
    }

    #[test]
    fn post_with_origin_mismatch_is_forbidden() {
        let request = format!(
            "POST /torrent/pause?id=1 HTTP/1.1\r\nHost: 127.0.0.1:19002\r\nOrigin: http://127.0.0.1:19003\r\nX-Rustorrent-Token: {}\r\nContent-Length: 0\r\n\r\n",
            api_token()
        );
        let response = run_single_request(request.as_bytes(), None);
        assert!(response.starts_with("HTTP/1.1 403 Forbidden"));
        assert!(response.contains("forbidden origin"));
    }

    #[test]
    fn post_with_valid_token_but_missing_origin_is_forbidden() {
        let request = format!(
            "POST /torrent/pause?id=1 HTTP/1.1\r\nHost: 127.0.0.1:19002\r\nX-Rustorrent-Token: {}\r\nContent-Length: 0\r\n\r\n",
            api_token()
        );
        let response = run_single_request(request.as_bytes(), None);
        assert!(response.starts_with("HTTP/1.1 403 Forbidden"));
        assert!(response.contains("origin header required"));
    }

    #[test]
    fn add_torrent_returns_torrent_id_in_json() {
        let (cmd_tx, cmd_rx) = mpsc::channel::<UiCommand>();
        let command_thread = thread::spawn(move || {
            let cmd = cmd_rx.recv().expect("receive add command");
            match cmd {
                UiCommand::AddTorrent { reply, .. } => {
                    let _ = reply.send(Ok(UiCommandSuccess::TorrentAdded { torrent_id: 77 }));
                }
                _ => panic!("expected add torrent command"),
            }
        });

        let host = "127.0.0.1:19004";
        let body = b"test";
        let mut request = format!(
            "POST /add-torrent?dir=%2Ftmp&prealloc=0 HTTP/1.1\r\nHost: {host}\r\nOrigin: http://{host}\r\nX-Rustorrent-Token: {}\r\nContent-Type: application/x-bittorrent\r\nContent-Length: {}\r\n\r\n",
            api_token(),
            body.len()
        )
        .into_bytes();
        request.extend_from_slice(body);

        let response = run_single_request(&request, Some(cmd_tx));
        command_thread.join().expect("join command thread");
        assert!(response.starts_with("HTTP/1.1 200 OK"));
        assert!(response.contains("\"ok\":true"));
        assert!(response.contains("\"torrent_id\":77"));
    }

    #[test]
    fn add_torrent_accepts_chunked_upload_body() {
        let (cmd_tx, cmd_rx) = mpsc::channel::<UiCommand>();
        let (captured_tx, captured_rx) = mpsc::channel::<Vec<u8>>();
        let command_thread = thread::spawn(move || {
            let cmd = cmd_rx.recv().expect("receive add command");
            match cmd {
                UiCommand::AddTorrent { data, reply, .. } => {
                    captured_tx.send(data).expect("send captured body");
                    let _ = reply.send(Ok(UiCommandSuccess::TorrentAdded { torrent_id: 88 }));
                }
                _ => panic!("expected add torrent command"),
            }
        });

        let host = "127.0.0.1:19014";
        let request = format!(
            "POST /add-torrent?dir=%2Ftmp&prealloc=0 HTTP/1.1\r\nHost: {host}\r\nOrigin: http://{host}\r\nX-Rustorrent-Token: {}\r\nContent-Type: application/x-bittorrent\r\nTransfer-Encoding: chunked\r\n\r\n4\r\ntest\r\n0\r\n\r\n",
            api_token(),
        );

        let response = run_single_request(request.as_bytes(), Some(cmd_tx));
        command_thread.join().expect("join command thread");
        let captured = captured_rx.recv().expect("receive captured body");
        assert_eq!(captured, b"test");
        assert!(response.starts_with("HTTP/1.1 200 OK"));
        assert!(response.contains("\"ok\":true"));
        assert!(response.contains("\"torrent_id\":88"));
    }

    #[test]
    fn chunk_header_limit_applies_when_the_terminator_arrives_later() {
        let host = "127.0.0.1:19015";
        let mut request = format!(
            "POST /add-torrent HTTP/1.1\r\nHost: {host}\r\nOrigin: http://{host}\r\nX-Rustorrent-Token: {}\r\nTransfer-Encoding: chunked\r\n\r\n0;",
            api_token()
        )
        .into_bytes();
        request.extend(std::iter::repeat_n(b'a', MAX_CHUNK_LINE_BYTES + 1));
        request.extend_from_slice(b"\r\n\r\n");

        let response = run_single_request(&request, None);
        assert!(response.starts_with("HTTP/1.1 400 Bad Request"));
    }

    #[test]
    fn archive_action_dispatches_archive_command() {
        let (cmd_tx, cmd_rx) = mpsc::channel::<UiCommand>();
        let command_thread = thread::spawn(move || {
            let cmd = cmd_rx.recv().expect("receive archive command");
            match cmd {
                UiCommand::ArchiveTorrent { torrent_id, reply } => {
                    assert_eq!(torrent_id, 7);
                    let _ = reply.send(Ok(UiCommandSuccess::Ok));
                }
                _ => panic!("expected archive torrent command"),
            }
        });

        let host = "127.0.0.1:19005";
        let request = format!(
            "POST /torrent/archive?id=7 HTTP/1.1\r\nHost: {host}\r\nOrigin: http://{host}\r\nX-Rustorrent-Token: {}\r\nContent-Length: 0\r\n\r\n",
            api_token()
        );
        let response = run_single_request(request.as_bytes(), Some(cmd_tx));
        command_thread.join().expect("join archive command thread");
        assert!(response.starts_with("HTTP/1.1 200 OK"));
        assert!(response.contains("\"ok\":true"));
    }

    #[test]
    fn peer_profile_setting_dispatches_command() {
        let (cmd_tx, cmd_rx) = mpsc::channel::<UiCommand>();
        let command_thread = thread::spawn(move || {
            let cmd = cmd_rx.recv().expect("receive peer profile command");
            match cmd {
                UiCommand::SetPeerProfile { profile, reply } => {
                    assert_eq!(profile, "aggressive");
                    let _ = reply.send(Ok(UiCommandSuccess::Ok));
                }
                _ => panic!("expected set peer profile command"),
            }
        });

        let host = "127.0.0.1:19006";
        let body = "profile=aggressive";
        let request = format!(
            "POST /settings/peer-profile HTTP/1.1\r\nHost: {host}\r\nOrigin: http://{host}\r\nX-Rustorrent-Token: {}\r\nContent-Type: application/x-www-form-urlencoded\r\nContent-Length: {}\r\n\r\n{}",
            api_token(),
            body.len(),
            body
        );
        let response = run_single_request(request.as_bytes(), Some(cmd_tx));
        command_thread.join().expect("join peer profile command");
        assert!(response.starts_with("HTTP/1.1 200 OK"));
        assert!(response.contains("\"ok\":true"));
    }

    #[test]
    fn split_path_query_and_percent_decode_work() {
        let (path, query) = split_path_query("/add?name=hello+world&x=%2Ftmp&empty=");
        assert_eq!(path, "/add");
        assert_eq!(
            query,
            vec![
                ("name".to_string(), "hello world".to_string()),
                ("x".to_string(), "/tmp".to_string()),
                ("empty".to_string(), "".to_string())
            ]
        );
    }

    #[test]
    fn percent_decode_preserves_utf8_paths_and_names() {
        assert_eq!(percent_decode("Espa%C3%B1a+%F0%9F%9A%80"), "España 🚀");
        assert_eq!(percent_decode("café"), "café");
    }

    #[test]
    fn authorize_mutating_request_allows_valid_token_and_origin() {
        let request = HttpRequest {
            method: "POST".to_string(),
            path: "/torrent/pause?id=1".to_string(),
            headers: vec![
                ("host".to_string(), "127.0.0.1:8080".to_string()),
                ("origin".to_string(), "http://127.0.0.1:8080".to_string()),
                (API_TOKEN_HEADER.to_string(), api_token().to_string()),
            ],
            body: Vec::new(),
        };
        assert!(authorize_mutating_request(&request).is_ok());
    }

    #[test]
    fn parse_bool_and_origin_extraction() {
        assert!(parse_bool("true"));
        assert!(parse_bool("YES"));
        assert!(parse_bool("1"));
        assert!(!parse_bool("0"));
        assert!(!parse_bool("no"));
        assert_eq!(
            extract_origin_host("https://Example.com:8443/path"),
            Some("example.com:8443".to_string())
        );
        assert_eq!(extract_origin_host("invalid"), None);
    }

    #[test]
    fn formatting_helpers_are_stable() {
        assert_eq!(human_bytes(999), "999 B");
        assert_eq!(human_bytes(1024), "1.00 KB");
        assert_eq!(human_rate(0.0), "0 B/s");
        assert_eq!(human_rate(1536.0), "1.50 KB/s");
        assert_eq!(format_eta_secs(0), "--:--");
        assert_eq!(format_eta_secs(61), "01:01");
        assert_eq!(format_eta_secs(3661), "01:01:01");
        assert_eq!(escape_html("<a&\"'>"), "&lt;a&amp;&quot;&#39;&gt;");
        assert_eq!(escape_json("a\"b\\\n"), "a\\\"b\\\\\\n");
    }

    #[test]
    fn desktop_layout_breakpoints_keep_two_columns_until_small_widths() {
        let html = status_html(&UiState::default());
        assert!(html.contains(".app{"));
        assert!(html.contains("max-width:1600px;"));
        assert!(html.contains("height:100svh;"));
        assert!(html.contains(".layout{"));
        assert!(html.contains("flex:1 1 auto;"));
        assert!(html.contains("overflow:hidden;"));
        assert!(html.contains(".sidebar{"));
        assert!(html.contains("flex:0 0 260px;"));
        assert!(html.contains("width:260px;"));
        assert!(html.contains(".torrent-list{"));
        assert!(html.contains("overflow-y:auto;"));
        assert!(html.contains("@media(max-width:900px){"));
        assert!(html.contains(".layout{gap:16px}"));
        assert!(html.contains(".sidebar{flex-basis:240px;width:240px}"));
        assert!(html.contains("@media(max-width:520px){"));
        assert!(html.contains(".layout{flex-direction:column;align-items:stretch;overflow-y:auto;"));
        assert!(html.contains(".sidebar{position:static;flex:0 0 auto;width:100%;"));
        assert!(html.contains(".torrent-list{flex:0 0 auto;overflow:visible;"));
        assert!(html.contains(".appbar-main{width:100%;min-width:0;flex-direction:column;"));
    }

    #[test]
    fn session_port_mapping_uses_stacked_layout_for_long_statuses() {
        let state = UiState {
            incoming_port: 6881,
            natpmp_status: "mapped nat-pmp on port 6881".to_string(),
            upnp_status: "mapped upnp on port 6881".to_string(),
            ..UiState::default()
        };

        let body_html = app_body_html(&state);
        let full_html = status_html(&state);
        assert!(body_html.contains("<div class=\"session-row stack\"><span>Port Mapping</span>"));
        assert!(
            body_html.contains(
                "<span class=\"session-value\"><span>mapped nat-pmp on port 6881</span><span>mapped upnp on port 6881</span></span>"
            )
        );
        assert!(full_html.contains(".session-row.stack{"));
    }

    #[test]
    fn successful_port_mapping_hides_failed_alternative() {
        let state = UiState {
            incoming_port: 6881,
            natpmp_status: "failed nat-pmp on port 6881: unsupported".to_string(),
            upnp_status: "mapped upnp on port 6881".to_string(),
            ..UiState::default()
        };

        let body_html = app_body_html(&state);
        assert!(body_html.contains("<span>mapped upnp on port 6881</span>"));
        assert!(!body_html.contains("failed nat-pmp"));
    }

    #[test]
    fn theme_defaults_to_light_without_saved_preference() {
        let html = status_html(&UiState::default());
        assert!(html.contains("if(t!=='light'&&t!=='dark'){t='light';}"));
        assert!(html.contains("function resolveTheme(){"));
        assert!(html.contains("let theme='light';"));
    }

    #[test]
    fn torrent_list_is_nested_inside_layout_container() {
        let html = app_body_html(&UiState::default());
        assert!(torrent_list_is_inside_layout(&html));
    }

    #[test]
    fn search_ui_renders_separate_main_tab_workspace() {
        let html = app_body_html(&UiState::default());
        assert!(html.contains("data-main-tab-target=\"library\""));
        assert!(html.contains("data-main-tab-target=\"search\""));
        assert!(html.contains("data-main-tab=\"library\""));
        assert!(html.contains("data-main-tab=\"search\""));
    }

    #[test]
    fn search_ui_explains_empty_plugin_state() {
        let html = status_html(&UiState::default());
        assert!(html.contains(
            "No search plugins are installed yet. Open Plugins or Community Catalog to add one."
        ));
    }

    #[test]
    fn seeding_status_does_not_force_full_progress_when_bytes_lag() {
        let mut state = UiState::default();
        state.torrents.push(UiTorrent {
            id: 7,
            name: "ubuntu.iso".to_string(),
            status: "seeding".to_string(),
            total_bytes: 1000,
            completed_bytes: 998,
            total_pieces: 10,
            completed_pieces: 9,
            ..UiTorrent::default()
        });

        let html = app_body_html(&state);
        assert!(html.contains("Progress 998 B / 1000 B (99.80%)"));
    }

    #[test]
    fn stopped_torrent_renders_resume_action() {
        let mut state = UiState::default();
        state.torrents.push(UiTorrent {
            id: 9,
            name: "bugonia".to_string(),
            status: "stopped".to_string(),
            ..UiTorrent::default()
        });

        let html = app_body_html(&state);
        assert!(html.contains("data-paused=\"true\""));
        assert!(html.contains("#i-play_arrow\"></use></svg>Resume"));
    }

    #[test]
    fn stopping_torrent_disables_stop_and_pause_actions() {
        let mut state = UiState::default();
        state.torrents.push(UiTorrent {
            id: 10,
            name: "ubuntu.iso".to_string(),
            status: "stopping".to_string(),
            ..UiTorrent::default()
        });

        let html = app_body_html(&state);
        assert!(html.contains("status-paused\">stopping"));
        assert!(html.contains("Stopping...</button>"));
        assert!(html.contains("title=\"Torrent is stopping\""));
    }

    #[test]
    fn transfer_panel_renders_archive_recheck_and_speed_chart() {
        let mut state = UiState {
            download_history_bps: vec![1024.0, 2048.0, 4096.0],
            upload_history_bps: vec![256.0, 512.0, 768.0],
            peer_profile: "balanced".to_string(),
            peer_profile_global_limit: 200,
            peer_profile_torrent_limit: 30,
            peer_profile_numwant: 200,
            ..UiState::default()
        };
        state.torrents.push(UiTorrent {
            id: 12,
            name: "ubuntu.iso".to_string(),
            status: "downloading".to_string(),
            ..UiTorrent::default()
        });

        let html = app_body_html(&state);
        assert_eq!(html.matches("data-action=\"recheck\"").count(), 1);
        assert!(html.contains("data-action=\"archive\""));
        assert!(html.contains("data-panel=\"transfer\""));
        assert!(html.contains("data-collapsed=\"true\""));
        assert!(html.contains("aria-label=\"transfer speed chart\""));
        assert!(html.contains("data-action=\"toggle-panel\""));
        assert!(html.contains("id=\"peerProfile\""));
        assert!(html.contains("Peer Profile"));
    }

    #[test]
    fn search_shell_keeps_plugin_panel_hidden_by_default_and_includes_toast_assets() {
        let html = status_html(&UiState::default());
        assert!(html.contains(".search-main-panel{display:none}"));
        assert!(
            html.contains(".search-main-panel.active{display:flex;flex-direction:column;gap:12px}")
        );
        assert!(html.contains(".search-results-panel{min-height:0}"));
        assert!(html.contains(".search-plugin-manager{min-height:0}"));
        assert!(!html.contains(".search-results-panel{display:flex"));
        assert!(!html.contains(".search-plugin-manager{display:flex"));
        assert!(html.contains(".toast-stack{"));
        assert!(html.contains("function showToast("));
        assert!(html.contains("function setSearchSort("));
        assert!(html.contains("id=\"searchPluginWarning\""));
    }

    #[test]
    fn scheduled_live_render_defers_while_add_modal_is_open() {
        let html = status_html(&UiState::default());
        assert!(html.contains("function isAddModalOpen()"));
        assert!(html.contains("if(isAddModalOpen()){pendingHtml=nextHtml;return;}"));
    }
}
