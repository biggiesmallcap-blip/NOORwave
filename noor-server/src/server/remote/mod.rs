use crate::db::{Database, remote as remote_db};
use axum::{
    Json, Router,
    extract::{ConnectInfo, DefaultBodyLimit, Path, State, rejection::JsonRejection},
    http::{HeaderMap, StatusCode, header},
    response::{IntoResponse, Response},
    routing::{delete, get, patch, post},
};
use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
use chrono::{DateTime, Utc};
use rand::{TryRngCore, rngs::OsRng};
use serde::{Deserialize, Serialize};
use serde_json::json;
use sha2::{Digest, Sha256};
use std::{
    collections::{HashMap, VecDeque},
    net::{IpAddr, SocketAddr},
    sync::Arc,
    time::{Duration, Instant},
};
use tokio::sync::{Mutex, RwLock, broadcast};
use url::Url;
use uuid::Uuid;

pub mod discovery;
pub mod network;

const TICKET_LIFETIME: Duration = Duration::from_secs(120);
const INVALID_PER_IP: usize = 8;
const INVALID_PROCESS: usize = 60;
const INVALID_WINDOW: Duration = Duration::from_secs(60);
const LIMITER_IDLE: Duration = Duration::from_secs(600);
const LIMITER_CAPACITY: usize = 4096;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Principal {
    SharedPin { generation: u64 },
    PairedDevice { id: String, generation: u64 },
}

#[derive(Debug, Clone, Copy, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum HostControl {
    Desktop,
    Standalone,
    Environment,
    CommandLine,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct RemoteAddress {
    pub id: String,
    pub label: String,
    pub url: String,
    pub kind: String,
    pub recommended: bool,
}

#[derive(Debug, Clone, Copy, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum DiscoveryState {
    Disabled,
    Starting,
    Advertised,
    Unavailable,
}

#[derive(Debug, Clone)]
pub struct RuntimeSnapshot {
    pub configured_host_mode: bool,
    pub effective_host_mode: bool,
    pub bind_address: SocketAddr,
    pub control: HostControl,
}

#[derive(Debug, Clone)]
struct RuntimeFacts {
    configured_host_mode: bool,
    bind_address: SocketAddr,
    control: HostControl,
    remote_assets_available: bool,
    addresses: Vec<RemoteAddress>,
    discovery_state: DiscoveryState,
    discovery_hostname: Option<String>,
    friendly_url: Option<String>,
    discovery_error: Option<String>,
}

impl Default for RuntimeFacts {
    fn default() -> Self {
        Self {
            configured_host_mode: false,
            bind_address: "127.0.0.1:17600".parse().expect("static socket address"),
            control: HostControl::Standalone,
            remote_assets_available: false,
            addresses: Vec::new(),
            discovery_state: DiscoveryState::Disabled,
            discovery_hostname: None,
            friendly_url: None,
            discovery_error: None,
        }
    }
}

impl RuntimeFacts {
    fn effective_host_mode(&self) -> bool {
        !self.bind_address.ip().is_loopback()
    }

    fn offered_origin(&self, address_id: Option<&str>) -> Option<(String, String)> {
        if address_id == Some("friendly") || address_id.is_none() && self.friendly_url.is_some() {
            return self
                .friendly_url
                .as_ref()
                .map(|url| ("friendly".to_string(), url.clone()));
        }
        let selected = match address_id {
            Some(id) => self.addresses.iter().find(|address| address.id == id),
            None => self.addresses.iter().find(|address| address.recommended),
        }?;
        Some((selected.id.clone(), selected.url.clone()))
    }
}

#[derive(Debug, Clone)]
struct CachedDevice {
    id: String,
    name: String,
    paired_at: DateTime<Utc>,
    last_seen_at: Option<DateTime<Utc>>,
    generation: u64,
}

#[derive(Debug)]
struct CredentialState {
    pin: String,
    pin_generation: u64,
    devices: Vec<([u8; 32], CachedDevice)>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TicketState {
    Pending,
    Redeemed,
}

#[derive(Debug)]
struct Ticket {
    id: Uuid,
    secret_hash: [u8; 32],
    pairing_code_hash: [u8; 32],
    deadline: Instant,
    expires_at: DateTime<Utc>,
    address_id: String,
    origin: String,
    state: TicketState,
}

#[derive(Debug, Default)]
struct FailureBucket {
    failures: VecDeque<Instant>,
    last_attempt: Option<Instant>,
}

#[derive(Debug, Default)]
struct InvalidAttemptLimiter {
    by_ip: HashMap<IpAddr, FailureBucket>,
    process: VecDeque<Instant>,
}

impl InvalidAttemptLimiter {
    fn record_failure(&mut self, ip: IpAddr, now: Instant) -> Result<(), u64> {
        self.process
            .retain(|when| now.duration_since(*when) < INVALID_WINDOW);
        self.by_ip.retain(|_, bucket| {
            bucket
                .last_attempt
                .is_some_and(|when| now.duration_since(when) < LIMITER_IDLE)
        });
        if !self.by_ip.contains_key(&ip) && self.by_ip.len() >= LIMITER_CAPACITY {
            let oldest = self
                .by_ip
                .iter()
                .min_by_key(|(_, bucket)| bucket.last_attempt)
                .map(|(ip, _)| *ip);
            if let Some(oldest) = oldest {
                self.by_ip.remove(&oldest);
            }
        }
        let bucket = self.by_ip.entry(ip).or_default();
        bucket
            .failures
            .retain(|when| now.duration_since(*when) < INVALID_WINDOW);
        bucket.last_attempt = Some(now);
        if bucket.failures.len() >= INVALID_PER_IP || self.process.len() >= INVALID_PROCESS {
            return Err(60);
        }
        bucket.failures.push_back(now);
        self.process.push_back(now);
        Ok(())
    }
}

struct RemoteInner {
    db: Database,
    server_id: String,
    hostname: RwLock<String>,
    credentials: RwLock<CredentialState>,
    runtime: RwLock<RuntimeFacts>,
    ticket: Mutex<Option<Ticket>>,
    mutation: Mutex<()>,
    limiter: Mutex<InvalidAttemptLimiter>,
    global_revocation: broadcast::Sender<()>,
    device_revocations: std::sync::Mutex<HashMap<String, broadcast::Sender<()>>>,
    last_seen_writes: std::sync::Mutex<HashMap<String, Instant>>,
    clock: Arc<dyn Clock>,
}

trait Clock: Send + Sync {
    fn monotonic(&self) -> Instant;
    fn utc(&self) -> DateTime<Utc>;
}

struct SystemClock;

impl Clock for SystemClock {
    fn monotonic(&self) -> Instant {
        Instant::now()
    }

    fn utc(&self) -> DateTime<Utc> {
        Utc::now()
    }
}

#[derive(Clone)]
pub struct RemoteService(Arc<RemoteInner>);

#[derive(Debug, thiserror::Error)]
pub enum AuthError {
    #[error("authentication required")]
    Invalid,
    #[error("rate limited")]
    RateLimited { retry_after: u64 },
}

impl RemoteService {
    pub fn new(db: Database, pin: String) -> anyhow::Result<Self> {
        Self::new_with_clock(db, pin, Arc::new(SystemClock))
    }

    fn new_with_clock(db: Database, pin: String, clock: Arc<dyn Clock>) -> anyhow::Result<Self> {
        let config = remote_db::initialize(&db)?;
        let devices = remote_db::load_devices(&db)?
            .into_iter()
            .map(|row| {
                (
                    row.token_hash,
                    CachedDevice {
                        id: row.id,
                        name: row.name,
                        paired_at: row.paired_at,
                        last_seen_at: row.last_seen_at,
                        generation: 1,
                    },
                )
            })
            .collect();
        let (global_revocation, _) = broadcast::channel(64);
        Ok(Self(Arc::new(RemoteInner {
            db,
            server_id: config.server_id,
            hostname: RwLock::new(config.hostname),
            credentials: RwLock::new(CredentialState {
                pin,
                pin_generation: 1,
                devices,
            }),
            runtime: RwLock::new(RuntimeFacts::default()),
            ticket: Mutex::new(None),
            mutation: Mutex::new(()),
            limiter: Mutex::new(InvalidAttemptLimiter::default()),
            global_revocation,
            device_revocations: std::sync::Mutex::new(HashMap::new()),
            last_seen_writes: std::sync::Mutex::new(HashMap::new()),
            clock,
        })))
    }

    pub fn server_id(&self) -> &str {
        &self.0.server_id
    }

    pub async fn hostname(&self) -> String {
        self.0.hostname.read().await.clone()
    }

    pub async fn shared_pin(&self) -> String {
        self.0.credentials.read().await.pin.clone()
    }

    pub async fn set_bound_listener(
        &self,
        bind_address: SocketAddr,
        control: HostControl,
        configured_host_mode: bool,
        remote_assets_available: bool,
    ) {
        let mut runtime = self.0.runtime.write().await;
        runtime.bind_address = bind_address;
        runtime.control = control;
        runtime.configured_host_mode = configured_host_mode;
        runtime.remote_assets_available = remote_assets_available;
        runtime.addresses.clear();
        runtime.discovery_state = if runtime.effective_host_mode() {
            DiscoveryState::Starting
        } else {
            DiscoveryState::Disabled
        };
        runtime.discovery_hostname = None;
        runtime.friendly_url = None;
        runtime.discovery_error = None;
        if !bind_address.ip().is_unspecified() && !bind_address.ip().is_loopback() {
            runtime.addresses.push(RemoteAddress {
                id: "listener".into(),
                label: "Bound address".into(),
                url: format!("http://{bind_address}/remote"),
                kind: "lan".into(),
                recommended: true,
            });
        }
        drop(runtime);
        self.invalidate_ticket().await;
    }

    /// Stage-3 discovery owns address enumeration and confirmed friendly-name
    /// advertisement. This narrow seam lets it publish only verified facts.
    pub async fn replace_network_facts(&self, addresses: Vec<RemoteAddress>) {
        let _mutation = self.0.mutation.lock().await;
        let mut runtime = self.0.runtime.write().await;
        runtime.addresses = addresses;
        let selected_ticket_is_still_offered = {
            let ticket = self.0.ticket.lock().await;
            ticket.as_ref().is_none_or(|ticket| {
                runtime
                    .offered_origin(Some(&ticket.address_id))
                    .is_some_and(|(_, origin)| origin.trim_end_matches("/remote") == ticket.origin)
            })
        };
        drop(runtime);
        if !selected_ticket_is_still_offered {
            *self.0.ticket.lock().await = None;
        }
    }

    pub async fn publish_discovery_state(
        &self,
        state: DiscoveryState,
        hostname: Option<String>,
        friendly_url: Option<String>,
        error: Option<String>,
    ) {
        let _mutation = self.0.mutation.lock().await;
        let mut runtime = self.0.runtime.write().await;
        runtime.discovery_state = state;
        runtime.discovery_hostname = hostname;
        runtime.friendly_url = friendly_url;
        runtime.discovery_error = error;
        let selected_ticket_is_still_offered = {
            let ticket = self.0.ticket.lock().await;
            ticket.as_ref().is_none_or(|ticket| {
                runtime
                    .offered_origin(Some(&ticket.address_id))
                    .is_some_and(|(_, origin)| origin.trim_end_matches("/remote") == ticket.origin)
            })
        };
        drop(runtime);
        if !selected_ticket_is_still_offered {
            *self.0.ticket.lock().await = None;
        }
    }

    pub async fn persist_effective_hostname(&self, hostname: &str) -> anyhow::Result<()> {
        remote_db::persist_hostname(&self.0.db, hostname)?;
        *self.0.hostname.write().await = hostname.to_string();
        Ok(())
    }

    pub async fn invalidate_ticket(&self) {
        *self.0.ticket.lock().await = None;
    }

    pub async fn runtime_snapshot(&self) -> RuntimeSnapshot {
        let runtime = self.0.runtime.read().await;
        RuntimeSnapshot {
            configured_host_mode: runtime.configured_host_mode,
            effective_host_mode: runtime.effective_host_mode(),
            bind_address: runtime.bind_address,
            control: runtime.control,
        }
    }

    pub async fn set_configured_host_mode(&self, configured: bool) {
        self.0.runtime.write().await.configured_host_mode = configured;
    }

    pub async fn authenticate(
        &self,
        token: Option<&str>,
        source_ip: IpAddr,
    ) -> Result<Principal, AuthError> {
        let _mutation = self.0.mutation.lock().await;
        let token = token.ok_or(AuthError::Invalid)?;
        let digest = hash_secret(token);
        let credential = {
            let credentials = self.0.credentials.read().await;
            if fixed_time_eq(token.as_bytes(), credentials.pin.as_bytes()) {
                Some(Principal::SharedPin {
                    generation: credentials.pin_generation,
                })
            } else {
                credentials.devices.iter().find_map(|(candidate, device)| {
                    fixed_time_eq(&digest, candidate).then(|| Principal::PairedDevice {
                        id: device.id.clone(),
                        generation: device.generation,
                    })
                })
            }
        };
        if let Some(principal) = credential {
            if let Principal::PairedDevice { id, .. } = &principal {
                self.touch_last_seen(id).await;
            }
            return Ok(principal);
        }
        self.record_invalid(normalize_ip(source_ip)).await?;
        Err(AuthError::Invalid)
    }

    async fn record_invalid(&self, source_ip: IpAddr) -> Result<(), AuthError> {
        self.0
            .limiter
            .lock()
            .await
            .record_failure(source_ip, Instant::now())
            .map_err(|retry_after| AuthError::RateLimited { retry_after })
    }

    async fn touch_last_seen(&self, id: &str) {
        let should_write = {
            let mut writes = self
                .0
                .last_seen_writes
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            let now = self.0.clock.monotonic();
            if writes
                .get(id)
                .is_some_and(|previous| now.duration_since(*previous) < Duration::from_secs(60))
            {
                false
            } else {
                writes.insert(id.to_string(), now);
                true
            }
        };
        if should_write {
            let now = self.0.clock.utc();
            if remote_db::touch_last_seen(&self.0.db, id, now).is_ok()
                && let Some((_, device)) = self
                    .0
                    .credentials
                    .write()
                    .await
                    .devices
                    .iter_mut()
                    .find(|(_, device)| device.id == id)
            {
                device.last_seen_at = Some(now);
            }
        }
    }

    pub fn subscribe_revocation(
        &self,
        principal: &Principal,
    ) -> (broadcast::Receiver<()>, Option<broadcast::Receiver<()>>) {
        let global = self.0.global_revocation.subscribe();
        let device = match principal {
            Principal::SharedPin { .. } => None,
            Principal::PairedDevice { id, .. } => {
                let mut channels = self
                    .0
                    .device_revocations
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner());
                let sender = channels.entry(id.clone()).or_insert_with(|| {
                    let (sender, _) = broadcast::channel(16);
                    sender
                });
                Some(sender.subscribe())
            }
        };
        (global, device)
    }

    pub async fn subscribe_if_current(
        &self,
        principal: &Principal,
    ) -> Option<(broadcast::Receiver<()>, Option<broadcast::Receiver<()>>)> {
        let _mutation = self.0.mutation.lock().await;
        if !self.principal_is_current(principal).await {
            return None;
        }
        Some(self.subscribe_revocation(principal))
    }

    pub async fn principal_is_current(&self, principal: &Principal) -> bool {
        let credentials = self.0.credentials.read().await;
        match principal {
            Principal::SharedPin { generation } => credentials.pin_generation == *generation,
            Principal::PairedDevice { id, generation } => credentials
                .devices
                .iter()
                .any(|(_, device)| device.id == *id && device.generation == *generation),
        }
    }
}

fn fixed_time_eq(left: &[u8], right: &[u8]) -> bool {
    let mut difference = left.len() ^ right.len();
    let longest = left.len().max(right.len());
    for index in 0..longest {
        difference |= usize::from(*left.get(index).unwrap_or(&0) ^ *right.get(index).unwrap_or(&0));
    }
    difference == 0
}

fn hash_secret(secret: &str) -> [u8; 32] {
    Sha256::digest(secret.as_bytes()).into()
}

fn random_secret(prefix: &str) -> anyhow::Result<String> {
    let mut bytes = [0_u8; 32];
    OsRng
        .try_fill_bytes(&mut bytes)
        .map_err(|error| anyhow::anyhow!("OS random source unavailable: {error}"))?;
    Ok(format!("{prefix}{}", URL_SAFE_NO_PAD.encode(bytes)))
}

fn random_pairing_code(excluded_pin: &str) -> anyhow::Result<String> {
    for _ in 0..8 {
        let candidate = random_pin()?;
        if candidate != excluded_pin {
            return Ok(candidate);
        }
    }
    Err(anyhow::anyhow!(
        "unable to create a pairing code distinct from the master PIN"
    ))
}

fn normalize_pairing_code(value: &str) -> Option<String> {
    let normalized: String = value
        .chars()
        .filter(|character| !character.is_ascii_whitespace() && *character != '-')
        .flat_map(char::to_uppercase)
        .collect();
    (normalized.len() == 6
        && normalized
            .bytes()
            .all(|character| character.is_ascii_digit()))
    .then_some(normalized)
}

fn random_pin() -> anyhow::Result<String> {
    let mut bytes = [0_u8; 4];
    OsRng
        .try_fill_bytes(&mut bytes)
        .map_err(|error| anyhow::anyhow!("OS random source unavailable: {error}"))?;
    Ok(format!("{:06}", u32::from_le_bytes(bytes) % 1_000_000))
}

#[derive(Debug, Serialize)]
struct RemoteErrorBody<'a> {
    error: &'a str,
    message: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    retry_after_seconds: Option<u64>,
}

pub(super) fn remote_error(
    status: StatusCode,
    code: &'static str,
    message: &'static str,
) -> Response {
    (
        status,
        [(header::CACHE_CONTROL, "no-store")],
        Json(RemoteErrorBody {
            error: code,
            message,
            retry_after_seconds: None,
        }),
    )
        .into_response()
}

pub(super) fn rate_error(retry_after: u64) -> Response {
    let mut response = (
        StatusCode::TOO_MANY_REQUESTS,
        Json(RemoteErrorBody {
            error: "RATE_LIMITED",
            message: "Too many invalid attempts. Try again later.",
            retry_after_seconds: Some(retry_after),
        }),
    )
        .into_response();
    response.headers_mut().insert(
        header::RETRY_AFTER,
        retry_after.to_string().parse().expect("valid retry header"),
    );
    response
        .headers_mut()
        .insert(header::CACHE_CONTROL, "no-store".parse().unwrap());
    response
}

fn invalid_json(rejection: JsonRejection) -> Response {
    let status = match rejection.status() {
        StatusCode::PAYLOAD_TOO_LARGE => StatusCode::PAYLOAD_TOO_LARGE,
        StatusCode::UNSUPPORTED_MEDIA_TYPE => StatusCode::UNSUPPORTED_MEDIA_TYPE,
        _ => StatusCode::BAD_REQUEST,
    };
    remote_error(
        status,
        "INVALID_REQUEST",
        "The request body must be valid JSON with the required fields.",
    )
}

#[derive(Debug, Serialize)]
struct RemoteIdentity {
    server_id: String,
    name: &'static str,
    protocol: u8,
    pairing_available: bool,
}

async fn info_handler(State(remote): State<RemoteService>) -> impl IntoResponse {
    let runtime = remote.0.runtime.read().await;
    (
        [(header::CACHE_CONTROL, "no-store")],
        Json(RemoteIdentity {
            server_id: remote.server_id().to_string(),
            name: "NOORwave",
            protocol: 1,
            pairing_available: runtime.effective_host_mode()
                && runtime.remote_assets_available
                && (runtime.friendly_url.is_some() || !runtime.addresses.is_empty()),
        }),
    )
}

#[derive(Debug, Deserialize)]
struct CreatePairingRequest {
    address_id: Option<String>,
}

#[derive(Debug, Serialize)]
struct PairingTicketResponse {
    id: String,
    pairing_url: String,
    pairing_code: String,
    expires_at: String,
    expires_in_seconds: u64,
}

async fn create_ticket_handler(
    State(remote): State<RemoteService>,
    request: Result<Json<CreatePairingRequest>, JsonRejection>,
) -> Response {
    let request = match request {
        Ok(Json(request)) => request,
        Err(rejection) => return invalid_json(rejection),
    };
    let _mutation = remote.0.mutation.lock().await;
    let runtime = remote.0.runtime.read().await;
    if !runtime.effective_host_mode() || !runtime.remote_assets_available {
        return remote_error(
            StatusCode::CONFLICT,
            "REMOTE_NOT_READY",
            "Phone remote hosting is not ready.",
        );
    }
    let Some((address_id, remote_url)) = runtime.offered_origin(request.address_id.as_deref())
    else {
        return remote_error(
            StatusCode::CONFLICT,
            "ADDRESS_UNAVAILABLE",
            "That phone address is no longer available.",
        );
    };
    drop(runtime);
    let credentials = remote.0.credentials.read().await;
    let count = credentials.devices.len();
    let master_pin = credentials.pin.clone();
    drop(credentials);
    if count >= remote_db::MAX_REMOTE_DEVICES as usize {
        return remote_error(
            StatusCode::CONFLICT,
            "DEVICE_LIMIT_REACHED",
            "The paired-device limit has been reached.",
        );
    }
    let secret = match random_secret("nrt_") {
        Ok(value) => value,
        Err(_) => {
            return remote_error(
                StatusCode::INTERNAL_SERVER_ERROR,
                "INTERNAL_ERROR",
                "Unable to create a pairing ticket.",
            );
        }
    };
    let pairing_code = match random_pairing_code(&master_pin) {
        Ok(value) => value,
        Err(_) => {
            return remote_error(
                StatusCode::INTERNAL_SERVER_ERROR,
                "INTERNAL_ERROR",
                "Unable to create a pairing ticket.",
            );
        }
    };
    let id = Uuid::new_v4();
    let now = remote.0.clock.monotonic();
    let expires_at =
        remote.0.clock.utc() + chrono::Duration::seconds(TICKET_LIFETIME.as_secs() as i64);
    let origin = remote_url.trim_end_matches("/remote").to_string();
    let pairing_url = format!("{}/remote#pair={secret}", origin.trim_end_matches('/'));
    *remote.0.ticket.lock().await = Some(Ticket {
        id,
        secret_hash: hash_secret(&secret),
        pairing_code_hash: hash_secret(&pairing_code),
        deadline: now + TICKET_LIFETIME,
        expires_at,
        address_id,
        origin,
        state: TicketState::Pending,
    });
    (
        StatusCode::CREATED,
        [(header::CACHE_CONTROL, "no-store")],
        Json(PairingTicketResponse {
            id: id.to_string(),
            pairing_url,
            pairing_code,
            expires_at: expires_at.to_rfc3339(),
            expires_in_seconds: 120,
        }),
    )
        .into_response()
}

async fn cancel_ticket_handler(
    State(remote): State<RemoteService>,
    Path(id): Path<String>,
) -> Response {
    let Ok(id) = Uuid::parse_str(&id) else {
        return remote_error(
            StatusCode::BAD_REQUEST,
            "INVALID_REQUEST",
            "Malformed ticket ID.",
        );
    };
    let _mutation = remote.0.mutation.lock().await;
    let mut ticket = remote.0.ticket.lock().await;
    if ticket
        .as_ref()
        .is_some_and(|ticket| ticket.id == id && ticket.state == TicketState::Pending)
    {
        *ticket = None;
    }
    StatusCode::NO_CONTENT.into_response()
}

#[derive(Debug, Deserialize)]
struct RedeemPairingRequest {
    ticket: String,
    device_name: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct RemoteDevice {
    id: String,
    name: String,
    paired_at: String,
    last_seen_at: Option<String>,
}

impl From<&CachedDevice> for RemoteDevice {
    fn from(device: &CachedDevice) -> Self {
        Self {
            id: device.id.clone(),
            name: device.name.clone(),
            paired_at: device.paired_at.to_rfc3339(),
            last_seen_at: device.last_seen_at.map(|value| value.to_rfc3339()),
        }
    }
}

#[derive(Debug, Serialize)]
struct PairingResponse {
    token: String,
    token_type: &'static str,
    server_id: String,
    device: RemoteDevice,
}

fn validated_name(value: Option<String>) -> Result<String, ()> {
    let name = value
        .unwrap_or_else(|| "Phone remote".into())
        .trim()
        .to_string();
    let count = name.chars().count();
    if !(1..=64).contains(&count) || name.chars().any(char::is_control) {
        return Err(());
    }
    Ok(name)
}

fn origin_of_remote_url(url: &str) -> Option<&str> {
    url.strip_suffix("/remote")
}

async fn validate_public_pair_request(remote: &RemoteService, headers: &HeaderMap) -> bool {
    if headers
        .get("sec-fetch-site")
        .and_then(|value| value.to_str().ok())
        .is_some_and(|value| value.eq_ignore_ascii_case("cross-site"))
    {
        return false;
    }
    let runtime = remote.0.runtime.read().await;
    let origins: Vec<&str> = runtime
        .addresses
        .iter()
        .filter_map(|address| origin_of_remote_url(&address.url))
        .chain(
            runtime
                .friendly_url
                .as_deref()
                .and_then(origin_of_remote_url),
        )
        .collect();
    let host_matches = headers
        .get(header::HOST)
        .and_then(|value| value.to_str().ok())
        .is_some_and(|host| {
            origins.iter().any(|origin| {
                let Some((scheme, _)) = origin.split_once("://") else {
                    return false;
                };
                normalized_http_origin(&format!("{scheme}://{host}"))
                    == normalized_http_origin(origin)
            })
        });
    if !host_matches {
        return false;
    }
    if let Some(origin) = headers
        .get(header::ORIGIN)
        .and_then(|value| value.to_str().ok())
    {
        let normalized = normalized_http_origin(origin);
        return normalized.as_ref().is_some_and(|candidate| {
            origins
                .iter()
                .any(|origin| normalized_http_origin(origin).as_ref() == Some(candidate))
        }) || super::is_trusted_local_origin_str(origin);
    }
    if let Some(referer) = headers
        .get(header::REFERER)
        .and_then(|value| value.to_str().ok())
    {
        return super::origin_from_url(referer).is_some_and(|referer_origin| {
            let normalized = normalized_http_origin(&referer_origin);
            normalized.as_ref().is_some_and(|candidate| {
                origins
                    .iter()
                    .any(|origin| normalized_http_origin(origin).as_ref() == Some(candidate))
            }) || super::is_trusted_local_origin_str(&referer_origin)
        });
    }
    true
}

fn normalized_http_origin(raw: &str) -> Option<String> {
    let parsed = Url::parse(raw.trim()).ok()?;
    if !matches!(parsed.scheme(), "http" | "https")
        || !parsed.username().is_empty()
        || parsed.password().is_some()
        || parsed.host().is_none()
        || parsed.path() != "/"
        || parsed.query().is_some()
        || parsed.fragment().is_some()
    {
        return None;
    }
    Some(parsed.origin().ascii_serialization())
}

fn request_matches_ticket_origin(headers: &HeaderMap, ticket_origin: &str) -> bool {
    let Some(ticket_origin) = normalized_http_origin(ticket_origin) else {
        return false;
    };
    let Some(host) = headers
        .get(header::HOST)
        .and_then(|value| value.to_str().ok())
    else {
        return false;
    };
    let scheme = ticket_origin.split_once("://").map(|(scheme, _)| scheme);
    let Some(host_origin) =
        scheme.and_then(|scheme| normalized_http_origin(&format!("{scheme}://{host}")))
    else {
        return false;
    };
    if host_origin != ticket_origin {
        return false;
    }
    headers
        .get(header::ORIGIN)
        .map(|value| {
            value
                .to_str()
                .ok()
                .and_then(normalized_http_origin)
                .is_some_and(|origin| origin == ticket_origin)
        })
        .unwrap_or(true)
}

async fn redeem_handler(
    State(remote): State<RemoteService>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    request: Result<Json<RedeemPairingRequest>, JsonRejection>,
) -> Response {
    let request = match request {
        Ok(Json(request)) => request,
        Err(rejection) => return invalid_json(rejection),
    };
    if !validate_public_pair_request(&remote, &headers).await {
        return remote_error(
            StatusCode::FORBIDDEN,
            "FORBIDDEN",
            "Pairing is not allowed from this origin.",
        );
    }
    let Ok(name) = validated_name(request.device_name) else {
        return remote_error(
            StatusCode::BAD_REQUEST,
            "INVALID_REQUEST",
            "Device name must be 1 to 64 characters without control characters.",
        );
    };
    if request.ticket.len() > 128 {
        return remote_error(
            StatusCode::BAD_REQUEST,
            "INVALID_REQUEST",
            "Invalid pairing request.",
        );
    }
    let pairing_code = normalize_pairing_code(&request.ticket);
    let supplied_hash = hash_secret(pairing_code.as_deref().unwrap_or(&request.ticket));
    let _mutation = remote.0.mutation.lock().await;
    let runtime = remote.0.runtime.read().await;
    let mut ticket_guard = remote.0.ticket.lock().await;
    let authority_matches = pairing_code.is_some()
        || ticket_guard
            .as_ref()
            .is_none_or(|ticket| request_matches_ticket_origin(&headers, &ticket.origin));
    if !authority_matches {
        drop(ticket_guard);
        drop(runtime);
        return remote_error(
            StatusCode::FORBIDDEN,
            "FORBIDDEN",
            "Pairing is not allowed from this origin.",
        );
    }
    let valid = ticket_guard.as_ref().is_some_and(|ticket| {
        let current_origin = runtime
            .offered_origin(Some(&ticket.address_id))
            .and_then(|(_, url)| origin_of_remote_url(&url).map(str::to_string));
        ticket.state == TicketState::Pending
            && remote.0.clock.monotonic() < ticket.deadline
            && (fixed_time_eq(&supplied_hash, &ticket.secret_hash)
                || fixed_time_eq(&supplied_hash, &ticket.pairing_code_hash))
            && runtime.effective_host_mode()
            && runtime.remote_assets_available
            && current_origin.as_deref() == Some(ticket.origin.as_str())
    });
    drop(runtime);
    if !valid {
        drop(ticket_guard);
        return match remote.record_invalid(normalize_ip(peer.ip())).await {
            Ok(()) | Err(AuthError::Invalid) => remote_error(
                StatusCode::UNAUTHORIZED,
                "PAIRING_INVALID",
                "The pairing link expired or was already used.",
            ),
            Err(AuthError::RateLimited { retry_after }) => rate_error(retry_after),
        };
    }

    let token = match random_secret("nrp_") {
        Ok(value) => value,
        Err(_) => {
            return remote_error(
                StatusCode::INTERNAL_SERVER_ERROR,
                "INTERNAL_ERROR",
                "Unable to create a device credential.",
            );
        }
    };
    let token_hash = hash_secret(&token);
    let row = remote_db::RemoteDeviceRow {
        id: Uuid::new_v4().to_string(),
        name: name.clone(),
        token_hash,
        paired_at: remote.0.clock.utc(),
        last_seen_at: None,
    };
    match remote_db::create_device(&remote.0.db, &row) {
        Ok(()) => {}
        Err(remote_db::CreateDeviceError::Capacity) => {
            return remote_error(
                StatusCode::CONFLICT,
                "DEVICE_LIMIT_REACHED",
                "The paired-device limit has been reached.",
            );
        }
        Err(remote_db::CreateDeviceError::Storage(_)) => {
            return remote_error(
                StatusCode::SERVICE_UNAVAILABLE,
                "STORAGE_UNAVAILABLE",
                "Pairing storage is temporarily unavailable.",
            );
        }
    }
    let cached = CachedDevice {
        id: row.id,
        name,
        paired_at: row.paired_at,
        last_seen_at: None,
        generation: 1,
    };
    remote
        .0
        .credentials
        .write()
        .await
        .devices
        .push((token_hash, cached.clone()));
    if let Some(ticket) = ticket_guard.as_mut() {
        ticket.state = TicketState::Redeemed;
    }
    drop(ticket_guard);
    (
        StatusCode::CREATED,
        [(header::CACHE_CONTROL, "no-store")],
        Json(PairingResponse {
            token,
            token_type: "Bearer",
            server_id: remote.server_id().to_string(),
            device: RemoteDevice::from(&cached),
        }),
    )
        .into_response()
}

fn normalize_ip(ip: IpAddr) -> IpAddr {
    match ip {
        IpAddr::V6(ip) => ip
            .to_ipv4_mapped()
            .map(IpAddr::V4)
            .unwrap_or(IpAddr::V6(ip)),
        other => other,
    }
}

#[derive(Debug, Serialize)]
struct RemoteStatus {
    server_id: String,
    control: HostControl,
    configured_host_mode: bool,
    effective_host_mode: bool,
    restart_required: bool,
    state: &'static str,
    bind_address: String,
    port: u16,
    discovery: serde_json::Value,
    addresses: Vec<RemoteAddress>,
    remote_assets_available: bool,
    phone_reachability: &'static str,
    ticket: Option<serde_json::Value>,
    diagnostics: Vec<serde_json::Value>,
}

async fn status_handler(State(remote): State<RemoteService>) -> impl IntoResponse {
    let runtime = remote.0.runtime.read().await.clone();
    let now = remote.0.clock.monotonic();
    let ticket = remote.0.ticket.lock().await.as_ref().map(|ticket| {
        json!({
            "id": ticket.id.to_string(),
            "state": if ticket.state == TicketState::Redeemed { "redeemed" } else if now >= ticket.deadline { "expired" } else { "pending" },
            "expires_at": ticket.expires_at.to_rfc3339(),
        })
    });
    let effective = runtime.effective_host_mode();
    let mut diagnostics = Vec::new();
    if !effective {
        diagnostics
            .push(json!({"code": "LOCAL_ONLY", "message": "Phone remote hosting is disabled."}));
    } else if runtime.addresses.is_empty() && runtime.friendly_url.is_none() {
        diagnostics.push(json!({"code": "NO_USABLE_ADDRESS", "message": "No usable phone address is available yet."}));
    }
    if !runtime.remote_assets_available {
        diagnostics.push(json!({"code": "REMOTE_ASSETS_MISSING", "message": "The bundled remote assets are unavailable."}));
    }
    if effective && runtime.discovery_state == DiscoveryState::Starting {
        diagnostics
            .push(json!({"code": "DISCOVERY_STARTING", "message": "Local discovery is starting."}));
    } else if effective && runtime.discovery_state == DiscoveryState::Unavailable {
        diagnostics.push(json!({
            "code": "DISCOVERY_UNAVAILABLE",
            "message": runtime.discovery_error.as_deref().unwrap_or("Local discovery is unavailable; use a direct IP address.")
        }));
    }
    let ready = effective
        && runtime.remote_assets_available
        && (runtime.friendly_url.is_some() || !runtime.addresses.is_empty());
    (
        [(header::CACHE_CONTROL, "no-store")],
        Json(RemoteStatus {
            server_id: remote.server_id().to_string(),
            control: runtime.control,
            configured_host_mode: runtime.configured_host_mode,
            effective_host_mode: effective,
            restart_required: runtime.configured_host_mode != effective,
            state: if ready {
                "running"
            } else if effective {
                "unavailable"
            } else {
                "disabled"
            },
            bind_address: runtime.bind_address.to_string(),
            port: runtime.bind_address.port(),
            discovery: json!({
                "state": runtime.discovery_state,
                "hostname": runtime.discovery_hostname,
                "friendly_url": runtime.friendly_url,
            }),
            addresses: runtime.addresses,
            remote_assets_available: runtime.remote_assets_available,
            phone_reachability: "unverified",
            ticket,
            diagnostics,
        }),
    )
}

async fn list_devices_handler(State(remote): State<RemoteService>) -> impl IntoResponse {
    let mut devices: Vec<RemoteDevice> = remote
        .0
        .credentials
        .read()
        .await
        .devices
        .iter()
        .map(|(_, device)| RemoteDevice::from(device))
        .collect();
    devices.sort_by(|left, right| right.paired_at.cmp(&left.paired_at));
    (
        [(header::CACHE_CONTROL, "no-store")],
        Json(json!({"devices": devices})),
    )
}

#[derive(Debug, Deserialize)]
struct RenameDeviceRequest {
    name: String,
}

async fn rename_device_handler(
    State(remote): State<RemoteService>,
    Path(id): Path<String>,
    request: Result<Json<RenameDeviceRequest>, JsonRejection>,
) -> Response {
    let request = match request {
        Ok(Json(request)) => request,
        Err(rejection) => return invalid_json(rejection),
    };
    if Uuid::parse_str(&id).is_err() {
        return remote_error(
            StatusCode::BAD_REQUEST,
            "INVALID_REQUEST",
            "Malformed device ID.",
        );
    }
    let Ok(name) = validated_name(Some(request.name)) else {
        return remote_error(
            StatusCode::BAD_REQUEST,
            "INVALID_REQUEST",
            "Device name must be 1 to 64 characters without control characters.",
        );
    };
    let _mutation = remote.0.mutation.lock().await;
    match remote_db::rename_device(&remote.0.db, &id, &name) {
        Ok(true) => {}
        Ok(false) => return remote_error(StatusCode::NOT_FOUND, "NOT_FOUND", "Device not found."),
        Err(_) => {
            return remote_error(
                StatusCode::SERVICE_UNAVAILABLE,
                "STORAGE_UNAVAILABLE",
                "Device storage is temporarily unavailable.",
            );
        }
    }
    let mut credentials = remote.0.credentials.write().await;
    let Some((_, device)) = credentials
        .devices
        .iter_mut()
        .find(|(_, device)| device.id == id)
    else {
        return remote_error(StatusCode::NOT_FOUND, "NOT_FOUND", "Device not found.");
    };
    device.name = name;
    Json(RemoteDevice::from(&*device)).into_response()
}

async fn revoke_device_handler(
    State(remote): State<RemoteService>,
    Path(id): Path<String>,
) -> Response {
    if Uuid::parse_str(&id).is_err() {
        return remote_error(
            StatusCode::BAD_REQUEST,
            "INVALID_REQUEST",
            "Malformed device ID.",
        );
    }
    let _mutation = remote.0.mutation.lock().await;
    if remote_db::revoke_device(&remote.0.db, &id).is_err() {
        return remote_error(
            StatusCode::SERVICE_UNAVAILABLE,
            "STORAGE_UNAVAILABLE",
            "Device storage is temporarily unavailable.",
        );
    }
    remote
        .0
        .credentials
        .write()
        .await
        .devices
        .retain(|(_, device)| device.id != id);
    if let Some(sender) = remote
        .0
        .device_revocations
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .remove(&id)
    {
        let _ = sender.send(());
    }
    StatusCode::NO_CONTENT.into_response()
}

pub async fn reset_all(remote: &RemoteService) -> Result<(String, usize), ()> {
    let _mutation = remote.0.mutation.lock().await;
    let old_pin = remote.0.credentials.read().await.pin.clone();
    let new_pin = loop {
        let candidate = random_pin().map_err(|_| ())?;
        if candidate != old_pin {
            break candidate;
        }
    };
    let count = remote_db::reset_all(&remote.0.db, &new_pin).map_err(|_| ())?;
    {
        let mut credentials = remote.0.credentials.write().await;
        credentials.pin = new_pin.clone();
        credentials.pin_generation = credentials.pin_generation.wrapping_add(1);
        credentials.devices.clear();
    }
    *remote.0.ticket.lock().await = None;
    let _ = remote.0.global_revocation.send(());
    remote
        .0
        .device_revocations
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .clear();
    Ok((new_pin, count))
}

pub fn public_routes(remote: RemoteService) -> Router {
    Router::new()
        .route("/api/remote/info", get(info_handler))
        .route(
            "/api/remote/pair",
            post(redeem_handler).layer(DefaultBodyLimit::max(2048)),
        )
        .with_state(remote)
        .layer(axum::middleware::from_fn(no_store_remote))
}

pub fn management_routes(remote: RemoteService) -> Router {
    Router::new()
        .route("/api/server/remote", get(status_handler))
        .route(
            "/api/server/remote/pairing",
            post(create_ticket_handler).layer(DefaultBodyLimit::max(2048)),
        )
        .route(
            "/api/server/remote/pairing/{id}",
            delete(cancel_ticket_handler),
        )
        .route("/api/server/remote/devices", get(list_devices_handler))
        .route(
            "/api/server/remote/devices/{id}",
            patch(rename_device_handler)
                .delete(revoke_device_handler)
                .layer(DefaultBodyLimit::max(2048)),
        )
        .with_state(remote)
        .layer(axum::middleware::from_fn(no_store_remote))
}

async fn no_store_remote(
    request: axum::extract::Request,
    next: axum::middleware::Next,
) -> Response {
    let mut response = next.run(request).await;
    response
        .headers_mut()
        .insert(header::CACHE_CONTROL, "no-store".parse().unwrap());
    response
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::schema;

    struct ManualClock {
        monotonic: std::sync::Mutex<Instant>,
        utc: std::sync::Mutex<DateTime<Utc>>,
    }

    impl ManualClock {
        fn new() -> Self {
            Self {
                monotonic: std::sync::Mutex::new(Instant::now()),
                utc: std::sync::Mutex::new(Utc::now()),
            }
        }

        fn advance(&self, duration: Duration) {
            *self.monotonic.lock().unwrap() += duration;
            *self.utc.lock().unwrap() += chrono::Duration::from_std(duration).unwrap();
        }
    }

    impl Clock for ManualClock {
        fn monotonic(&self) -> Instant {
            *self.monotonic.lock().unwrap()
        }

        fn utc(&self) -> DateTime<Utc> {
            *self.utc.lock().unwrap()
        }
    }

    fn service(pin: &str) -> RemoteService {
        let db = Database::open_in_memory().unwrap();
        db.with_conn(schema::run_migrations).unwrap();
        RemoteService::new(db, pin.to_string()).unwrap()
    }

    async fn ready_service_with_clock(pin: &str, clock: Arc<dyn Clock>) -> RemoteService {
        let db = Database::open_in_memory().unwrap();
        db.with_conn(schema::run_migrations).unwrap();
        let remote = RemoteService::new_with_clock(db, pin.to_string(), clock).unwrap();
        remote
            .set_bound_listener(
                "192.168.1.10:17600".parse().unwrap(),
                HostControl::Standalone,
                true,
                true,
            )
            .await;
        remote
    }

    async fn create_ticket(remote: &RemoteService) -> (String, String) {
        create_ticket_for(remote, None).await
    }

    async fn create_ticket_for(
        remote: &RemoteService,
        address_id: Option<&str>,
    ) -> (String, String) {
        let response = create_ticket_handler(
            State(remote.clone()),
            Ok(Json(CreatePairingRequest {
                address_id: address_id.map(str::to_string),
            })),
        )
        .await;
        assert_eq!(response.status(), StatusCode::CREATED);
        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let created: serde_json::Value = serde_json::from_slice(&body).unwrap();
        let secret = created["pairing_url"]
            .as_str()
            .unwrap()
            .split("#pair=")
            .nth(1)
            .unwrap()
            .to_string();
        (created["id"].as_str().unwrap().to_string(), secret)
    }

    fn pair_headers() -> HeaderMap {
        let mut headers = HeaderMap::new();
        headers.insert(header::HOST, "192.168.1.10:17600".parse().unwrap());
        headers.insert(header::ORIGIN, "http://192.168.1.10:17600".parse().unwrap());
        headers
    }

    #[tokio::test]
    async fn public_pairing_accepts_a_trusted_loopback_development_origin() {
        let remote = ready_service_with_clock("123456", Arc::new(SystemClock)).await;
        let mut headers = HeaderMap::new();
        headers.insert(header::HOST, "192.168.1.10:17600".parse().unwrap());
        headers.insert(
            header::ORIGIN,
            format!("http://127.0.0.1:{}", super::super::noor_dev_port())
                .parse()
                .unwrap(),
        );
        assert!(validate_public_pair_request(&remote, &headers).await);
    }

    #[tokio::test]
    async fn temporary_code_pairs_an_existing_pwa_from_another_offered_origin_once() {
        let remote = service_with_friendly_and_direct_origins().await;
        let response = create_ticket_handler(
            State(remote.clone()),
            Ok(Json(CreatePairingRequest {
                address_id: Some("listener".into()),
            })),
        )
        .await;
        assert_eq!(response.status(), StatusCode::CREATED);
        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let created: serde_json::Value = serde_json::from_slice(&body).unwrap();
        let code = created["pairing_code"].as_str().unwrap();
        assert_eq!(code.len(), 6);
        assert!(code.bytes().all(|value| value.is_ascii_digit()));

        let entered = format!("{} {}", &code[..3], &code[3..]);
        let mut pwa_headers = HeaderMap::new();
        pwa_headers.insert(header::HOST, "noorwave.local:17600".parse().unwrap());
        pwa_headers.insert(
            header::ORIGIN,
            "http://noorwave.local:17600".parse().unwrap(),
        );
        assert_eq!(
            redeem_with_headers(&remote, entered.clone(), "iPhone PWA", pwa_headers.clone())
                .await
                .status(),
            StatusCode::CREATED
        );
        assert_eq!(
            redeem_with_headers(&remote, entered, "Second use", pwa_headers)
                .await
                .status(),
            StatusCode::UNAUTHORIZED
        );
    }

    async fn redeem(remote: &RemoteService, secret: String, name: &str) -> Response {
        redeem_with_headers(remote, secret, name, pair_headers()).await
    }

    async fn redeem_with_headers(
        remote: &RemoteService,
        secret: String,
        name: &str,
        headers: HeaderMap,
    ) -> Response {
        redeem_handler(
            State(remote.clone()),
            ConnectInfo("192.168.1.20:5000".parse().unwrap()),
            headers,
            Ok(Json(RedeemPairingRequest {
                ticket: secret,
                device_name: Some(name.into()),
            })),
        )
        .await
    }

    async fn service_with_friendly_and_direct_origins() -> RemoteService {
        let remote = ready_service_with_clock("123456", Arc::new(SystemClock)).await;
        remote
            .publish_discovery_state(
                DiscoveryState::Advertised,
                Some("noorwave.local".into()),
                Some("http://noorwave.local:17600/remote".into()),
                None,
            )
            .await;
        remote
    }

    async fn paired_token(remote: &RemoteService, name: &str) -> (String, String) {
        let (_, secret) = create_ticket(remote).await;
        let response = redeem(remote, secret, name).await;
        assert_eq!(response.status(), StatusCode::CREATED);
        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let value: serde_json::Value = serde_json::from_slice(&body).unwrap();
        (
            value["device"]["id"].as_str().unwrap().to_string(),
            value["token"].as_str().unwrap().to_string(),
        )
    }

    #[tokio::test]
    async fn ticket_is_single_use_under_concurrent_redemption() {
        let remote = service("123456");
        remote
            .set_bound_listener(
                "192.168.1.10:17600".parse().unwrap(),
                HostControl::Standalone,
                true,
                true,
            )
            .await;
        let (_, secret) = create_ticket(&remote).await;
        let request = || RedeemPairingRequest {
            ticket: secret.clone(),
            device_name: Some("Phone".into()),
        };
        let first = redeem_handler(
            State(remote.clone()),
            ConnectInfo("192.168.1.20:5000".parse().unwrap()),
            pair_headers(),
            Ok(Json(request())),
        );
        let second = redeem_handler(
            State(remote.clone()),
            ConnectInfo("192.168.1.21:5000".parse().unwrap()),
            pair_headers(),
            Ok(Json(request())),
        );
        let (first, second) = tokio::join!(first, second);
        let statuses = [first.status(), second.status()];
        assert!(statuses.contains(&StatusCode::CREATED));
        assert!(statuses.contains(&StatusCode::UNAUTHORIZED));
        assert_eq!(remote_db::load_devices(&remote.0.db).unwrap().len(), 1);
    }

    #[tokio::test]
    async fn friendly_ticket_rejects_direct_ip_authority_without_consuming_it() {
        let remote = service_with_friendly_and_direct_origins().await;
        let (_, secret) = create_ticket_for(&remote, Some("friendly")).await;

        let wrong =
            redeem_with_headers(&remote, secret.clone(), "Wrong origin", pair_headers()).await;
        assert_eq!(wrong.status(), StatusCode::FORBIDDEN);
        assert!(remote_db::load_devices(&remote.0.db).unwrap().is_empty());

        let mut intended = HeaderMap::new();
        intended.insert(header::HOST, "NOORWAVE.local:17600".parse().unwrap());
        intended.insert(
            header::ORIGIN,
            "http://noorwave.local:17600/".parse().unwrap(),
        );
        assert_eq!(
            redeem_with_headers(&remote, secret, "Friendly phone", intended)
                .await
                .status(),
            StatusCode::CREATED
        );
        assert_eq!(remote_db::load_devices(&remote.0.db).unwrap().len(), 1);
    }

    #[tokio::test]
    async fn direct_ip_ticket_rejects_friendly_authority_and_headerless_host_mismatch() {
        let remote = service_with_friendly_and_direct_origins().await;
        let (_, secret) = create_ticket_for(&remote, Some("listener")).await;
        let mut friendly = HeaderMap::new();
        friendly.insert(header::HOST, "noorwave.local:17600".parse().unwrap());
        friendly.insert(
            header::ORIGIN,
            "http://noorwave.local:17600".parse().unwrap(),
        );
        assert_eq!(
            redeem_with_headers(&remote, secret.clone(), "Wrong friendly", friendly)
                .await
                .status(),
            StatusCode::FORBIDDEN
        );

        let mut wrong_native = HeaderMap::new();
        wrong_native.insert(header::HOST, "noorwave.local:17600".parse().unwrap());
        assert_eq!(
            redeem_with_headers(&remote, secret.clone(), "Wrong native", wrong_native)
                .await
                .status(),
            StatusCode::FORBIDDEN
        );
        assert!(remote_db::load_devices(&remote.0.db).unwrap().is_empty());

        let mut intended_native = HeaderMap::new();
        intended_native.insert(header::HOST, "192.168.1.10:17600".parse().unwrap());
        assert_eq!(
            redeem_with_headers(&remote, secret, "Native phone", intended_native)
                .await
                .status(),
            StatusCode::CREATED
        );
    }

    #[tokio::test]
    async fn invalid_attempt_limit_does_not_block_a_valid_credential() {
        let remote = service("123456");
        let ip: IpAddr = "192.168.1.20".parse().unwrap();
        for _ in 0..8 {
            assert!(matches!(
                remote.authenticate(Some("wrong"), ip).await,
                Err(AuthError::Invalid)
            ));
        }
        assert!(matches!(
            remote.authenticate(Some("wrong"), ip).await,
            Err(AuthError::RateLimited { .. })
        ));
        assert!(matches!(
            remote.authenticate(Some("123456"), ip).await,
            Ok(Principal::SharedPin { .. })
        ));
    }

    #[tokio::test]
    async fn ticket_expires_at_exact_monotonic_boundary_and_refresh_invalidates_predecessor() {
        let clock = Arc::new(ManualClock::new());
        let remote = ready_service_with_clock("123456", clock.clone()).await;
        let (_, first) = create_ticket(&remote).await;
        let (_, second) = create_ticket(&remote).await;
        assert_eq!(
            redeem(&remote, first, "Old QR").await.status(),
            StatusCode::UNAUTHORIZED
        );
        clock.advance(TICKET_LIFETIME);
        assert_eq!(
            redeem(&remote, second, "Expired QR").await.status(),
            StatusCode::UNAUTHORIZED
        );
        assert!(remote_db::load_devices(&remote.0.db).unwrap().is_empty());
    }

    #[tokio::test]
    async fn unchanged_network_refresh_preserves_ticket_but_removed_origin_invalidates_it() {
        let remote = ready_service_with_clock("123456", Arc::new(SystemClock)).await;
        let (_, _) = create_ticket(&remote).await;
        let unchanged = remote.0.runtime.read().await.addresses.clone();

        remote.replace_network_facts(unchanged).await;
        assert!(remote.0.ticket.lock().await.is_some());

        remote.replace_network_facts(Vec::new()).await;
        assert!(remote.0.ticket.lock().await.is_none());
    }

    #[tokio::test]
    async fn unchanged_address_refresh_preserves_a_friendly_hostname_ticket() {
        let remote = ready_service_with_clock("123456", Arc::new(SystemClock)).await;
        remote
            .publish_discovery_state(
                DiscoveryState::Advertised,
                Some("noorwave.local".into()),
                Some("http://noorwave.local:17600/remote".into()),
                None,
            )
            .await;
        let response = create_ticket_handler(
            State(remote.clone()),
            Ok(Json(CreatePairingRequest {
                address_id: Some("friendly".into()),
            })),
        )
        .await;
        assert_eq!(response.status(), StatusCode::CREATED);
        let unchanged = remote.0.runtime.read().await.addresses.clone();

        remote.replace_network_facts(unchanged).await;

        assert!(remote.0.ticket.lock().await.is_some());
        assert_eq!(
            remote.0.runtime.read().await.friendly_url.as_deref(),
            Some("http://noorwave.local:17600/remote")
        );
    }

    #[tokio::test]
    async fn status_separates_direct_ip_readiness_from_discovery_confirmation() {
        let remote = service("123456");
        remote
            .set_bound_listener(
                "0.0.0.0:43123".parse().unwrap(),
                HostControl::Desktop,
                true,
                true,
            )
            .await;
        let starting = status_handler(State(remote.clone())).await.into_response();
        let starting_body = axum::body::to_bytes(starting.into_body(), usize::MAX)
            .await
            .unwrap();
        let starting: serde_json::Value = serde_json::from_slice(&starting_body).unwrap();
        assert_eq!(starting["state"], "unavailable");
        assert_eq!(starting["discovery"]["state"], "starting");
        assert!(starting["addresses"].as_array().unwrap().is_empty());

        remote
            .replace_network_facts(vec![RemoteAddress {
                id: "wifi:192.168.1.24".into(),
                label: "Wi-Fi".into(),
                url: "http://192.168.1.24:43123/remote".into(),
                kind: "lan".into(),
                recommended: true,
            }])
            .await;
        remote
            .publish_discovery_state(
                DiscoveryState::Unavailable,
                Some("noorwave.local".into()),
                None,
                Some("Local discovery is unavailable; use a direct IP address.".into()),
            )
            .await;
        let fallback = status_handler(State(remote)).await.into_response();
        let fallback_body = axum::body::to_bytes(fallback.into_body(), usize::MAX)
            .await
            .unwrap();
        let fallback: serde_json::Value = serde_json::from_slice(&fallback_body).unwrap();
        assert_eq!(fallback["state"], "running");
        assert_eq!(fallback["port"], 43123);
        assert_eq!(fallback["discovery"]["state"], "unavailable");
        assert!(fallback["discovery"]["friendly_url"].is_null());
        assert_eq!(
            fallback["addresses"][0]["url"],
            "http://192.168.1.24:43123/remote"
        );
    }

    #[tokio::test]
    async fn storage_failure_returns_no_token_and_keeps_ticket_redeemable() {
        let remote = ready_service_with_clock("123456", Arc::new(SystemClock)).await;
        let (_, secret) = create_ticket(&remote).await;
        remote
            .0
            .db
            .with_conn(|conn| {
                conn.execute_batch("DROP TABLE remote_devices;")?;
                Ok(())
            })
            .unwrap();
        let failed = redeem(&remote, secret.clone(), "Retry phone").await;
        assert_eq!(failed.status(), StatusCode::SERVICE_UNAVAILABLE);
        let failed_body = axum::body::to_bytes(failed.into_body(), usize::MAX)
            .await
            .unwrap();
        assert!(!String::from_utf8_lossy(&failed_body).contains("nrp_"));

        remote
            .0
            .db
            .with_conn(|conn| {
                conn.execute_batch(
                    "CREATE TABLE remote_devices (
                        id TEXT PRIMARY KEY NOT NULL,
                        name TEXT NOT NULL,
                        token_hash BLOB NOT NULL CHECK(length(token_hash) = 32),
                        paired_at TEXT NOT NULL,
                        last_seen_at TEXT
                     );
                     CREATE UNIQUE INDEX idx_remote_devices_token_hash ON remote_devices(token_hash);",
                )?;
                Ok(())
            })
            .unwrap();
        assert_eq!(
            redeem(&remote, secret, "Retry phone").await.status(),
            StatusCode::CREATED
        );
    }

    #[tokio::test]
    async fn reset_revokes_pin_devices_ticket_and_registered_sessions() {
        let remote = ready_service_with_clock("123456", Arc::new(SystemClock)).await;
        let (_, ticket) = create_ticket(&remote).await;
        let paired = redeem(&remote, ticket, "Phone").await;
        let body = axum::body::to_bytes(paired.into_body(), usize::MAX)
            .await
            .unwrap();
        let value: serde_json::Value = serde_json::from_slice(&body).unwrap();
        let device_token = value["token"].as_str().unwrap().to_string();
        let device_principal = remote
            .authenticate(Some(&device_token), "192.168.1.20".parse().unwrap())
            .await
            .unwrap();
        let (mut global_revocation, _) = remote.subscribe_revocation(&device_principal);
        let (_, pending_ticket) = create_ticket(&remote).await;

        let (new_pin, count) = reset_all(&remote).await.unwrap();

        assert_eq!(count, 1);
        assert_ne!(new_pin, "123456");
        assert!(
            remote
                .authenticate(Some("123456"), "192.168.1.20".parse().unwrap())
                .await
                .is_err()
        );
        assert!(
            remote
                .authenticate(Some(&device_token), "192.168.1.20".parse().unwrap())
                .await
                .is_err()
        );
        assert!(matches!(
            remote
                .authenticate(Some(&new_pin), "192.168.1.20".parse().unwrap())
                .await,
            Ok(Principal::SharedPin { .. })
        ));
        assert_eq!(
            redeem(&remote, pending_ticket, "After reset")
                .await
                .status(),
            StatusCode::UNAUTHORIZED
        );
        tokio::time::timeout(Duration::from_secs(1), global_revocation.recv())
            .await
            .expect("registered session was revoked")
            .unwrap();
    }

    #[tokio::test]
    async fn reset_racing_redemption_leaves_no_post_reset_credential() {
        let remote = ready_service_with_clock("123456", Arc::new(SystemClock)).await;
        let (_, ticket) = create_ticket(&remote).await;

        // Hold the serialization point until both independently scheduled
        // operations are ready to contend for it. This exercises the real
        // reset/redemption boundary instead of polling two immediately-ready
        // futures in a deterministic join order.
        let mutation = remote.0.mutation.lock().await;
        let start = Arc::new(tokio::sync::Barrier::new(3));
        let redeem_remote = remote.clone();
        let redeem_start = start.clone();
        let redemption = tokio::spawn(async move {
            redeem_start.wait().await;
            redeem(&redeem_remote, ticket, "Racing phone").await
        });
        let reset_remote = remote.clone();
        let reset_start = start.clone();
        let reset = tokio::spawn(async move {
            reset_start.wait().await;
            reset_all(&reset_remote).await
        });
        start.wait().await;
        tokio::time::sleep(Duration::from_millis(10)).await;
        drop(mutation);
        let redemption = redemption.await.unwrap();
        let (new_pin, _) = reset.await.unwrap().unwrap();

        assert!(matches!(
            redemption.status(),
            StatusCode::CREATED | StatusCode::UNAUTHORIZED
        ));
        if redemption.status() == StatusCode::CREATED {
            let body = axum::body::to_bytes(redemption.into_body(), usize::MAX)
                .await
                .unwrap();
            let value: serde_json::Value = serde_json::from_slice(&body).unwrap();
            let token = value["token"].as_str().unwrap();
            assert!(
                remote
                    .authenticate(Some(token), "192.168.1.20".parse().unwrap())
                    .await
                    .is_err()
            );
        }
        assert!(remote_db::load_devices(&remote.0.db).unwrap().is_empty());
        assert!(matches!(
            remote
                .authenticate(Some(&new_pin), "192.168.1.20".parse().unwrap())
                .await,
            Ok(Principal::SharedPin { .. })
        ));
    }

    #[tokio::test]
    async fn reset_racing_socket_registration_cannot_leave_a_live_old_principal() {
        let remote = ready_service_with_clock("123456", Arc::new(SystemClock)).await;
        let (_, token) = paired_token(&remote, "Socket race").await;
        let principal = remote
            .authenticate(Some(&token), "192.168.1.20".parse().unwrap())
            .await
            .unwrap();

        let mutation = remote.0.mutation.lock().await;
        let start = Arc::new(tokio::sync::Barrier::new(3));
        let registration_remote = remote.clone();
        let registration_principal = principal.clone();
        let registration_start = start.clone();
        let registration = tokio::spawn(async move {
            registration_start.wait().await;
            registration_remote
                .subscribe_if_current(&registration_principal)
                .await
        });
        let reset_remote = remote.clone();
        let reset_start = start.clone();
        let reset = tokio::spawn(async move {
            reset_start.wait().await;
            reset_all(&reset_remote).await
        });
        start.wait().await;
        tokio::time::sleep(Duration::from_millis(10)).await;
        drop(mutation);
        let registration = registration.await.unwrap();
        reset.await.unwrap().unwrap();

        assert!(!remote.principal_is_current(&principal).await);
        if let Some((mut global, _)) = registration {
            tokio::time::timeout(Duration::from_secs(1), global.recv())
                .await
                .expect("racing socket registration was revoked")
                .unwrap();
        }
    }

    #[tokio::test]
    async fn revoking_one_device_invalidates_only_that_principal_and_signal() {
        let remote = ready_service_with_clock("123456", Arc::new(SystemClock)).await;
        let (first_id, first_token) = paired_token(&remote, "First").await;
        let (_, second_token) = paired_token(&remote, "Second").await;
        let first_principal = remote
            .authenticate(Some(&first_token), "192.168.1.20".parse().unwrap())
            .await
            .unwrap();
        let (_, mut first_revocation) = remote.subscribe_revocation(&first_principal);

        assert_eq!(
            revoke_device_handler(State(remote.clone()), Path(first_id))
                .await
                .status(),
            StatusCode::NO_CONTENT
        );
        assert!(
            remote
                .authenticate(Some(&first_token), "192.168.1.20".parse().unwrap())
                .await
                .is_err()
        );
        assert!(matches!(
            remote
                .authenticate(Some(&second_token), "192.168.1.21".parse().unwrap())
                .await,
            Ok(Principal::PairedDevice { .. })
        ));
        tokio::time::timeout(
            Duration::from_secs(1),
            first_revocation.as_mut().unwrap().recv(),
        )
        .await
        .expect("device revocation signal")
        .unwrap();
    }

    #[tokio::test]
    async fn paired_last_seen_persists_at_most_once_per_minute() {
        let clock = Arc::new(ManualClock::new());
        let remote = ready_service_with_clock("123456", clock.clone()).await;
        let (id, token) = paired_token(&remote, "Seen phone").await;
        remote
            .authenticate(Some(&token), "192.168.1.20".parse().unwrap())
            .await
            .unwrap();
        let first = remote_db::load_devices(&remote.0.db)
            .unwrap()
            .into_iter()
            .find(|device| device.id == id)
            .unwrap()
            .last_seen_at
            .unwrap();
        clock.advance(Duration::from_secs(30));
        remote
            .authenticate(Some(&token), "192.168.1.20".parse().unwrap())
            .await
            .unwrap();
        let unchanged = remote_db::load_devices(&remote.0.db)
            .unwrap()
            .into_iter()
            .find(|device| device.id == id)
            .unwrap()
            .last_seen_at
            .unwrap();
        assert_eq!(unchanged, first);
        clock.advance(Duration::from_secs(30));
        remote
            .authenticate(Some(&token), "192.168.1.20".parse().unwrap())
            .await
            .unwrap();
        let updated = remote_db::load_devices(&remote.0.db)
            .unwrap()
            .into_iter()
            .find(|device| device.id == id)
            .unwrap()
            .last_seen_at
            .unwrap();
        assert!(updated > first);
    }

    #[tokio::test]
    async fn paired_credentials_survive_service_restart_but_tickets_do_not() {
        let remote = ready_service_with_clock("123456", Arc::new(SystemClock)).await;
        let (_, token) = paired_token(&remote, "Persistent phone").await;
        let (_, _pending_secret) = create_ticket(&remote).await;
        let restarted = RemoteService::new(remote.0.db.clone(), "123456".into()).unwrap();

        assert!(matches!(
            restarted
                .authenticate(Some(&token), "192.168.1.20".parse().unwrap())
                .await,
            Ok(Principal::PairedDevice { .. })
        ));
        assert!(restarted.0.ticket.lock().await.is_none());
        assert_eq!(restarted.server_id(), remote.server_id());
    }

    #[tokio::test]
    async fn active_device_limit_rejects_ticket_creation_without_replacing_state() {
        let remote = ready_service_with_clock("123456", Arc::new(SystemClock)).await;
        for index in 0..remote_db::MAX_REMOTE_DEVICES {
            let token = format!("nrp_test_{index}");
            let row = remote_db::RemoteDeviceRow {
                id: Uuid::new_v4().to_string(),
                name: format!("Phone {index}"),
                token_hash: hash_secret(&token),
                paired_at: Utc::now(),
                last_seen_at: None,
            };
            remote_db::create_device(&remote.0.db, &row).unwrap();
            remote.0.credentials.write().await.devices.push((
                row.token_hash,
                CachedDevice {
                    id: row.id,
                    name: row.name,
                    paired_at: row.paired_at,
                    last_seen_at: None,
                    generation: 1,
                },
            ));
        }
        let response = create_ticket_handler(
            State(remote.clone()),
            Ok(Json(CreatePairingRequest { address_id: None })),
        )
        .await;
        assert_eq!(response.status(), StatusCode::CONFLICT);
        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        assert_eq!(
            serde_json::from_slice::<serde_json::Value>(&body).unwrap()["error"],
            "DEVICE_LIMIT_REACHED"
        );
        assert!(remote.0.ticket.lock().await.is_none());
    }

    #[test]
    fn limiter_storage_is_capped_even_during_process_wide_throttling() {
        let mut limiter = InvalidAttemptLimiter::default();
        let now = Instant::now();
        let mut first_process_limit = None;
        for value in 0..5000_u32 {
            let ip = IpAddr::V4(std::net::Ipv4Addr::from(value));
            if limiter.record_failure(ip, now).is_err() && first_process_limit.is_none() {
                first_process_limit = Some(value);
            }
        }
        assert_eq!(first_process_limit, Some(INVALID_PROCESS as u32));
        assert!(limiter.by_ip.len() <= LIMITER_CAPACITY);
    }

    #[test]
    fn device_names_follow_scalar_and_control_character_limits() {
        assert!(validated_name(Some(" Phone ".into())).is_ok());
        assert!(validated_name(Some("".into())).is_err());
        assert!(validated_name(Some("x".repeat(65))).is_err());
        assert!(validated_name(Some("bad\nname".into())).is_err());
    }
}
