//! libp2p transport and session layer for Sceadu.
//!
//! M1c scope: one TCP + Noise connection, signed manifest exchange, and an
//! echo RPC over a libp2p request/response stream.

use std::collections::{HashMap, HashSet};
use std::net::{Ipv4Addr, Ipv6Addr};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::Engine;
use futures::StreamExt;
use libp2p::identity::Keypair;
use libp2p::kad::store::MemoryStore;
use libp2p::kad::{
    self, GetProvidersOk, GetRecordOk, QueryId, QueryResult, Quorum, Record, RecordKey,
};
use libp2p::multiaddr::Protocol;
use libp2p::request_response::{
    self, Message, OutboundRequestId, ProtocolSupport, ResponseChannel,
};
use libp2p::swarm::{NetworkBehaviour, StreamProtocol, Swarm, SwarmEvent};
use libp2p::SwarmBuilder;
pub use libp2p::{Multiaddr, PeerId};
use rand_core::{OsRng, RngCore};
use sceadu_core::{AgentId, Capability, Identity, Manifest, ManifestBody};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};
use tokio::sync::{mpsc, oneshot};
use tokio::task::JoinHandle;

pub mod invite;
pub use invite::InviteTicket;

const PROTOCOL: &str = "/sceadu/rpc/1.0.0";
const MANIFEST_TTL_SECONDS: u64 = 3600;
const QUARANTINE_FAILURES: u32 = 3;
const QUARANTINE_DURATION: Duration = Duration::from_secs(60);
const MAX_TASK_INPUT_BYTES: usize = 64 * 1024;
const MAX_TASK_ID_LEN: usize = 128;

#[derive(NetworkBehaviour)]
struct Behaviour {
    rpc: request_response::json::Behaviour<RpcRequest, RpcResponse>,
    kad: libp2p::kad::Behaviour<MemoryStore>,
}

/// Errors returned by the network layer.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("core error: {0}")]
    Core(#[from] sceadu_core::Error),
    #[error("identity error: {0}")]
    Identity(String),
    #[error("transport error: {0}")]
    Transport(String),
    #[error("behaviour error: {0}")]
    Behaviour(String),
    #[error("listen error: {0}")]
    Listen(String),
    #[error("dial error: {0}")]
    Dial(String),
    #[error("request error: {0}")]
    Request(String),
    #[error("peer mismatch: expected {expected}, got {actual}")]
    PeerMismatch { expected: String, actual: String },
    #[error("DHT error: {0}")]
    Dht(String),
    #[error("serialization error: {0}")]
    Serialization(String),
    #[error("invite error: {0}")]
    Invite(String),
    #[error("unsupported operation: {0}")]
    Unsupported(String),
    #[error("node task stopped")]
    Closed,
}

/// Result type used by the network layer.
pub type Result<T> = std::result::Result<T, Error>;

#[derive(Debug, Clone, Serialize, Deserialize)]
enum RpcRequest {
    Manifest(Box<Manifest>),
    Echo {
        message: String,
    },
    Neighbors {
        capability: Option<String>,
        max: u32,
        nonce: u64,
    },
    TaskStart {
        task_id: String,
        capability: String,
        input: Value,
        deadline_at: u64,
    },
    TaskReady {
        task_id: String,
    },
    TaskEventPush {
        task_id: String,
        sequence: u64,
        event: TaskEventKind,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
enum RpcResponse {
    Manifest(Box<Manifest>),
    Echo {
        message: String,
    },
    Neighbors {
        records: Vec<ContactRecord>,
        nonce: u64,
        policy: String,
    },
    TaskAccepted {
        task_id: String,
    },
    TaskReadyAck {
        task_id: String,
    },
    TaskEventAck {
        task_id: String,
        sequence: u64,
    },
    Error {
        message: String,
    },
}

/// A signed manifest observed during discovery.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ContactRecord {
    pub manifest: Manifest,
    pub observed_at: u64,
}

/// How a node answers neighbor queries.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum NeighborPolicy {
    /// Return verified manifests from the local peer cache.
    #[default]
    Open,
    /// Return no manifests.
    Stealth,
}

impl NeighborPolicy {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Open => "open",
            Self::Stealth => "stealth",
        }
    }
}

/// How a node chooses a network path.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum ConnectivityPolicy {
    /// Try direct TCP, then future fallbacks.
    #[default]
    Auto,
    /// Use only direct TCP addresses.
    Direct,
    /// Require a relay path.
    Relay,
    /// Require an anonymizing transport (Tor or I2P).
    Privacy,
}

impl ConnectivityPolicy {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Auto => "auto",
            Self::Direct => "direct",
            Self::Relay => "relay",
            Self::Privacy => "privacy",
        }
    }
}

/// Transport names used in status and future manifests.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TransportKind {
    Tcp,
    Relay,
    Tor,
    #[serde(rename = "i2p")]
    I2P,
}

impl TransportKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Tcp => "tcp",
            Self::Relay => "relay",
            Self::Tor => "tor",
            Self::I2P => "i2p",
        }
    }
}

/// Locally observed reachability classification.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Reachability {
    Loopback,
    Private,
    PublicUnverified,
    PublicVerified,
    Relay,
    Tor,
    I2P,
    Unknown,
}

impl Reachability {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Loopback => "loopback",
            Self::Private => "private",
            Self::PublicUnverified => "public_unverified",
            Self::PublicVerified => "public_verified",
            Self::Relay => "relay",
            Self::Tor => "tor",
            Self::I2P => "i2p",
            Self::Unknown => "unknown",
        }
    }
}

/// One endpoint known to the local address book.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EndpointRecord {
    pub addr: String,
    pub transport: TransportKind,
    pub reachability: Reachability,
    pub source: String,
    pub verified: bool,
}

/// Availability of a transport in this build.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TransportStatus {
    pub transport: TransportKind,
    pub available: bool,
    pub note: String,
}

/// A snapshot of local connectivity facts for an agent.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NetworkStatus {
    pub agent_id: String,
    pub peer_id: String,
    pub listen: Option<String>,
    pub policy: ConnectivityPolicy,
    pub reachability: Reachability,
    pub transports: Vec<TransportStatus>,
    pub endpoints: Vec<EndpointRecord>,
}

/// Node behavior configuration.
#[derive(Debug, Clone, Copy, Default)]
pub struct NodeConfig {
    pub neighbor_policy: NeighborPolicy,
    pub connectivity_policy: ConnectivityPolicy,
}

/// One event in a remote task stream.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TaskEventKind {
    Started {
        capability: String,
    },
    Progress {
        message: String,
        fraction: Option<f64>,
    },
    Result {
        output: Value,
    },
    Failed {
        message: String,
    },
    Done,
}

/// A sequenced task event sent by a remote agent.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TaskEvent {
    pub task_id: String,
    pub sequence: u64,
    pub event: TaskEventKind,
}

/// A handle to an in-flight task. Events arrive in sequence order.
pub struct TaskHandle {
    pub task_id: String,
    events: mpsc::Receiver<TaskEvent>,
}

impl TaskHandle {
    pub async fn next(&mut self) -> Option<TaskEvent> {
        self.events.recv().await
    }
}

type PendingNeighbors = (
    u64,
    u32,
    Option<String>,
    oneshot::Sender<Result<Vec<ContactRecord>>>,
);

#[derive(Default)]
struct SourceStats {
    failures: u32,
    quarantined_until: Option<Instant>,
}

struct ActiveTask {
    events: mpsc::Sender<TaskEvent>,
    last_sequence: u64,
}

struct PendingAsk {
    task_id: String,
    events: mpsc::Sender<TaskEvent>,
    receiver: Option<mpsc::Receiver<TaskEvent>>,
    reply: oneshot::Sender<Result<TaskHandle>>,
}

struct IncomingTask {
    peer: PeerId,
    capability: String,
    input: Value,
    deadline_at: u64,
}

#[derive(Default)]
struct AddressBook {
    endpoints: HashMap<String, EndpointRecord>,
}

impl AddressBook {
    fn upsert(&mut self, record: EndpointRecord) {
        self.endpoints.insert(record.addr.clone(), record);
    }

    fn records(&self) -> Vec<EndpointRecord> {
        let mut records: Vec<EndpointRecord> = self.endpoints.values().cloned().collect();
        records.sort_by(|left, right| left.addr.cmp(&right.addr));
        records
    }
}

#[derive(Default)]
struct NodeState {
    sources: HashMap<PeerId, SourceStats>,
    address_book: AddressBook,
    pending_dial_endpoints: HashMap<PeerId, String>,
    connectivity_policy: ConnectivityPolicy,
    pending_asks: HashMap<OutboundRequestId, PendingAsk>,
    active_tasks: HashMap<String, ActiveTask>,
    incoming_tasks: HashMap<String, IncomingTask>,
    manifests: HashMap<PeerId, Manifest>,
    pending_dials: HashMap<PeerId, oneshot::Sender<Result<PeerId>>>,
    pending_manifests: HashMap<OutboundRequestId, oneshot::Sender<Result<Manifest>>>,
    pending_echoes: HashMap<OutboundRequestId, oneshot::Sender<Result<String>>>,
    pending_neighbors: HashMap<OutboundRequestId, PendingNeighbors>,
    pending_dht: HashMap<QueryId, DhtPending>,
}

enum DhtPending {
    Publish {
        reply: oneshot::Sender<Result<()>>,
    },
    Lookup {
        agent_id: AgentId,
        found: Option<Box<Manifest>>,
        reply: oneshot::Sender<Result<Manifest>>,
    },
    Bootstrap {
        reply: oneshot::Sender<Result<()>>,
    },
    Provide {
        reply: oneshot::Sender<Result<()>>,
    },
    Providers {
        collected: HashSet<PeerId>,
        reply: oneshot::Sender<Result<Vec<PeerId>>>,
    },
}

enum Command {
    Dial {
        addr: Multiaddr,
        reply: oneshot::Sender<Result<PeerId>>,
    },
    Manifest {
        peer: PeerId,
        reply: oneshot::Sender<Result<Manifest>>,
    },
    Echo {
        peer: PeerId,
        message: String,
        reply: oneshot::Sender<Result<String>>,
    },
    Neighbors {
        peer: PeerId,
        capability: Option<String>,
        max: u32,
        nonce: u64,
        reply: oneshot::Sender<Result<Vec<ContactRecord>>>,
    },
    Publish {
        reply: oneshot::Sender<Result<()>>,
    },
    Lookup {
        agent_id: AgentId,
        reply: oneshot::Sender<Result<Manifest>>,
    },
    AddAddress {
        peer: PeerId,
        addr: Multiaddr,
        reply: oneshot::Sender<Result<()>>,
    },
    Bootstrap {
        reply: oneshot::Sender<Result<()>>,
    },
    Provide {
        capability: String,
        reply: oneshot::Sender<Result<()>>,
    },
    Providers {
        capability: String,
        reply: oneshot::Sender<Result<Vec<PeerId>>>,
    },
    DialPeer {
        peer: PeerId,
        reply: oneshot::Sender<Result<PeerId>>,
    },
    Status {
        reply: oneshot::Sender<NetworkStatus>,
    },
    Ask {
        peer: PeerId,
        capability: String,
        input: Value,
        deadline_at: u64,
        reply: oneshot::Sender<Result<TaskHandle>>,
    },
    Shutdown,
}

/// A running Sceadu node. Commands are serialized through the task that owns
/// the libp2p swarm.
pub struct NodeHandle {
    peer_id: PeerId,
    listen_addr: Option<Multiaddr>,
    agent_id: AgentId,
    commands: mpsc::Sender<Command>,
    task: JoinHandle<()>,
}

impl NodeHandle {
    pub fn peer_id(&self) -> PeerId {
        self.peer_id
    }

    pub fn listen_addr(&self) -> Option<&Multiaddr> {
        self.listen_addr.as_ref()
    }

    pub fn agent_id(&self) -> &AgentId {
        &self.agent_id
    }

    pub async fn dial(&self, addr: Multiaddr) -> Result<PeerId> {
        let (reply, receiver) = oneshot::channel();
        self.commands
            .send(Command::Dial { addr, reply })
            .await
            .map_err(|_| Error::Closed)?;
        receiver.await.map_err(|_| Error::Closed)?
    }

    pub async fn request_manifest(&self, peer: PeerId) -> Result<Manifest> {
        let (reply, receiver) = oneshot::channel();
        self.commands
            .send(Command::Manifest { peer, reply })
            .await
            .map_err(|_| Error::Closed)?;
        receiver.await.map_err(|_| Error::Closed)?
    }

    pub async fn echo(&self, peer: PeerId, message: impl Into<String>) -> Result<String> {
        let (reply, receiver) = oneshot::channel();
        self.commands
            .send(Command::Echo {
                peer,
                message: message.into(),
                reply,
            })
            .await
            .map_err(|_| Error::Closed)?;
        receiver.await.map_err(|_| Error::Closed)?
    }

    pub async fn neighbors(
        &self,
        peer: PeerId,
        capability: Option<String>,
        max: u32,
    ) -> Result<Vec<ContactRecord>> {
        let nonce = now_unix();
        let (reply, receiver) = oneshot::channel();
        self.commands
            .send(Command::Neighbors {
                peer,
                capability,
                max,
                nonce,
                reply,
            })
            .await
            .map_err(|_| Error::Closed)?;
        receiver.await.map_err(|_| Error::Closed)?
    }

    /// Query multiple introducers and merge their validated contacts.
    ///
    /// A failed introducer is skipped rather than failing the whole discovery
    /// result. Records are de-duplicated by AgentID and capped globally.
    pub async fn discover(
        &self,
        introducers: &[PeerId],
        capability: Option<String>,
        per_introducer: u32,
        max_total: u32,
    ) -> Result<Vec<ContactRecord>> {
        let limit = max_total.clamp(1, 128) as usize;
        let mut seen = HashSet::new();
        let mut records = Vec::new();

        for introducer in introducers {
            if records.len() >= limit {
                break;
            }
            let Ok(batch) = self
                .neighbors(*introducer, capability.clone(), per_introducer)
                .await
            else {
                continue;
            };
            for record in batch {
                if records.len() >= limit {
                    break;
                }
                if seen.insert(record.manifest.body.agent_id.to_string()) {
                    records.push(record);
                }
            }
        }

        Ok(records)
    }

    pub async fn publish(&self) -> Result<()> {
        let (reply, receiver) = oneshot::channel();
        self.commands
            .send(Command::Publish { reply })
            .await
            .map_err(|_| Error::Closed)?;
        receiver.await.map_err(|_| Error::Closed)?
    }

    pub async fn lookup(&self, agent_id: AgentId) -> Result<Manifest> {
        let (reply, receiver) = oneshot::channel();
        self.commands
            .send(Command::Lookup { agent_id, reply })
            .await
            .map_err(|_| Error::Closed)?;
        receiver.await.map_err(|_| Error::Closed)?
    }

    pub async fn add_address(&self, peer: PeerId, addr: Multiaddr) -> Result<()> {
        let (reply, receiver) = oneshot::channel();
        self.commands
            .send(Command::AddAddress { peer, addr, reply })
            .await
            .map_err(|_| Error::Closed)?;
        receiver.await.map_err(|_| Error::Closed)?
    }

    pub async fn bootstrap(&self) -> Result<()> {
        let (reply, receiver) = oneshot::channel();
        self.commands
            .send(Command::Bootstrap { reply })
            .await
            .map_err(|_| Error::Closed)?;
        receiver.await.map_err(|_| Error::Closed)?
    }

    pub async fn provide(&self, capability: impl Into<String>) -> Result<()> {
        let (reply, receiver) = oneshot::channel();
        self.commands
            .send(Command::Provide {
                capability: capability.into(),
                reply,
            })
            .await
            .map_err(|_| Error::Closed)?;
        receiver.await.map_err(|_| Error::Closed)?
    }

    pub async fn providers(&self, capability: impl Into<String>) -> Result<Vec<PeerId>> {
        let (reply, receiver) = oneshot::channel();
        self.commands
            .send(Command::Providers {
                capability: capability.into(),
                reply,
            })
            .await
            .map_err(|_| Error::Closed)?;
        receiver.await.map_err(|_| Error::Closed)?
    }

    pub async fn network_status(&self) -> Result<NetworkStatus> {
        let (reply, receiver) = oneshot::channel();
        self.commands
            .send(Command::Status { reply })
            .await
            .map_err(|_| Error::Closed)?;
        receiver.await.map_err(|_| Error::Closed)
    }

    pub async fn dial_peer(&self, peer: PeerId) -> Result<PeerId> {
        let (reply, receiver) = oneshot::channel();
        self.commands
            .send(Command::DialPeer { peer, reply })
            .await
            .map_err(|_| Error::Closed)?;
        receiver.await.map_err(|_| Error::Closed)?
    }

    /// Start a task and return a handle that yields streaming events.
    pub async fn ask(
        &self,
        peer: PeerId,
        capability: impl Into<String>,
        input: Value,
        timeout: Duration,
    ) -> Result<TaskHandle> {
        let deadline_at = now_unix().saturating_add(timeout.as_secs().max(1));
        let (reply, receiver) = oneshot::channel();
        self.commands
            .send(Command::Ask {
                peer,
                capability: capability.into(),
                input,
                deadline_at,
                reply,
            })
            .await
            .map_err(|_| Error::Closed)?;
        receiver.await.map_err(|_| Error::Closed)?
    }

    pub async fn shutdown(self) {
        let _ = self.commands.send(Command::Shutdown).await;
        let _ = self.task.await;
    }
}

/// Start a node with the default configuration.
pub async fn spawn(
    identity: &Identity,
    listen: Option<Multiaddr>,
) -> Result<(NodeHandle, Option<Multiaddr>)> {
    spawn_with_config(identity, listen, NodeConfig::default()).await
}

/// Start a node with an explicit configuration.
///
/// Returns the handle and the actual listen address (with `/p2p/<peer-id>`).
pub async fn spawn_with_config(
    identity: &Identity,
    listen: Option<Multiaddr>,
    config: NodeConfig,
) -> Result<(NodeHandle, Option<Multiaddr>)> {
    if listen.is_some()
        && matches!(
            config.connectivity_policy,
            ConnectivityPolicy::Relay | ConnectivityPolicy::Privacy
        )
    {
        return Err(Error::Unsupported(
            "relay/privacy transports are not implemented; disable the cleartext listener"
                .to_owned(),
        ));
    }
    let mut seed = identity.to_seed();
    let keypair = Keypair::ed25519_from_bytes(&mut seed.expose()[..])
        .map_err(|error| Error::Identity(error.to_string()))?;
    let peer_id = keypair.public().to_peer_id();

    let mut swarm = SwarmBuilder::with_existing_identity(keypair)
        .with_tokio()
        .with_tcp(
            libp2p::tcp::Config::default(),
            libp2p::noise::Config::new,
            libp2p::yamux::Config::default,
        )
        .map_err(|error| Error::Transport(error.to_string()))?
        .with_behaviour(|key| {
            let peer_id = key.public().to_peer_id();
            let mut kad = libp2p::kad::Behaviour::new(peer_id, MemoryStore::new(peer_id));
            kad.set_mode(Some(libp2p::kad::Mode::Server));
            Behaviour {
                rpc: rpc_behaviour(),
                kad,
            }
        })
        .map_err(|error| Error::Behaviour(error.to_string()))?
        .with_swarm_config(|config| config.with_idle_connection_timeout(Duration::from_secs(60)))
        .build();

    let listen_addr = if let Some(listen) = listen {
        swarm
            .listen_on(listen)
            .map_err(|error| Error::Listen(error.to_string()))?;
        let address = tokio::time::timeout(Duration::from_secs(5), wait_for_listen(&mut swarm))
            .await
            .map_err(|_| Error::Listen("timed out waiting for a listen address".to_owned()))??;
        Some(address.with(Protocol::P2p(peer_id)))
    } else {
        None
    };

    let mut body = ManifestBody::new(identity.agent_id(), 1, MANIFEST_TTL_SECONDS);
    body.capabilities.push(Capability::new("task.echo"));
    if let Some(address) = &listen_addr {
        body.endpoints.push(address.to_string());
    }
    body.protocols.push(PROTOCOL.to_owned());
    let local_manifest = Manifest::sign(body, identity)?;

    let (commands, receiver) = mpsc::channel(32);
    let task = tokio::spawn(run_node(swarm, local_manifest.clone(), receiver, config));

    let handle = NodeHandle {
        peer_id,
        listen_addr: listen_addr.clone(),
        agent_id: local_manifest.body.agent_id.clone(),
        commands,
        task,
    };

    Ok((handle, listen_addr))
}

fn rpc_behaviour() -> request_response::json::Behaviour<RpcRequest, RpcResponse> {
    request_response::json::Behaviour::new(
        [(StreamProtocol::new(PROTOCOL), ProtocolSupport::Full)],
        request_response::Config::default(),
    )
}

async fn wait_for_listen(swarm: &mut Swarm<Behaviour>) -> Result<Multiaddr> {
    loop {
        if let SwarmEvent::NewListenAddr { address, .. } = swarm.select_next_some().await {
            return Ok(address);
        }
    }
}

async fn run_node(
    mut swarm: Swarm<Behaviour>,
    local_manifest: Manifest,
    mut commands: mpsc::Receiver<Command>,
    config: NodeConfig,
) {
    let mut state = NodeState {
        connectivity_policy: config.connectivity_policy,
        ..Default::default()
    };

    loop {
        tokio::select! {
            event = swarm.select_next_some() => {
                handle_event(&mut swarm, event, &local_manifest, &mut state, config);
            }
            command = commands.recv() => {
                match command {
                    Some(command) => {
                        if !handle_command(&mut swarm, command, &local_manifest, &mut state) {
                            break;
                        }
                    }
                    None => break,
                }
            }
        }
    }
}

fn handle_event(
    swarm: &mut Swarm<Behaviour>,
    event: SwarmEvent<BehaviourEvent>,
    local_manifest: &Manifest,
    state: &mut NodeState,
    config: NodeConfig,
) {
    match event {
        SwarmEvent::ConnectionEstablished { peer_id, .. } => {
            if let Some(addr) = state.pending_dial_endpoints.remove(&peer_id) {
                if let Some(mut record) = state.address_book.endpoints.get(&addr).cloned() {
                    record.verified = true;
                    if record.reachability == Reachability::PublicUnverified {
                        record.reachability = Reachability::PublicVerified;
                    }
                    state.address_book.upsert(record);
                }
            }
            if let Some(reply) = state.pending_dials.remove(&peer_id) {
                let _ = reply.send(Ok(peer_id));
            }
        }
        SwarmEvent::OutgoingConnectionError { peer_id, error, .. } => {
            let message = format!("dial failed: {error}");
            if let Some(peer) = peer_id {
                if let Some(reply) = state.pending_dials.remove(&peer) {
                    let _ = reply.send(Err(Error::Dial(message)));
                }
            } else {
                for (_, reply) in state.pending_dials.drain() {
                    let _ = reply.send(Err(Error::Dial(message.clone())));
                }
            }
        }
        SwarmEvent::ConnectionClosed { peer_id, .. } => {
            state.manifests.remove(&peer_id);
        }
        SwarmEvent::Behaviour(event) => match event {
            BehaviourEvent::Rpc(event) => {
                handle_rpc_event(swarm, event, local_manifest, state, config)
            }
            BehaviourEvent::Kad(event) => handle_kad_event(event, state),
        },
        _ => {}
    }
}

fn handle_rpc_event(
    swarm: &mut Swarm<Behaviour>,
    event: request_response::Event<RpcRequest, RpcResponse>,
    local_manifest: &Manifest,
    state: &mut NodeState,
    config: NodeConfig,
) {
    match event {
        request_response::Event::Message { peer, message, .. } => match message {
            Message::Request {
                request, channel, ..
            } => respond_to_request(swarm, peer, request, channel, local_manifest, state, config),
            Message::Response {
                request_id,
                response,
            } => handle_response(swarm, peer, request_id, response, state),
        },
        request_response::Event::OutboundFailure {
            peer,
            request_id,
            error,
            ..
        } => {
            let message = format!("request failed: {error}");
            if let Some(reply) = state.pending_manifests.remove(&request_id) {
                let _ = reply.send(Err(Error::Request(message.clone())));
            }
            if let Some(reply) = state.pending_echoes.remove(&request_id) {
                let _ = reply.send(Err(Error::Request(message.clone())));
            }
            if let Some((_, _, _, reply)) = state.pending_neighbors.remove(&request_id) {
                record_source_failure(state, &peer);
                let _ = reply.send(Err(Error::Request(message)));
            }
        }
        _ => {}
    }
}

fn task_error(message: impl Into<String>) -> RpcResponse {
    RpcResponse::Error {
        message: message.into(),
    }
}

fn handle_task_start(
    peer: PeerId,
    task_id: String,
    capability: String,
    input: Value,
    deadline_at: u64,
    local_manifest: &Manifest,
    state: &mut NodeState,
) -> RpcResponse {
    if let Err(error) = ensure_direct_allowed(state.connectivity_policy) {
        return task_error(error.to_string());
    }
    if !state.manifests.contains_key(&peer) {
        return task_error("manifest exchange required");
    }
    if task_id.is_empty() || task_id.len() > MAX_TASK_ID_LEN {
        return task_error("invalid task id");
    }
    if state.incoming_tasks.contains_key(&task_id) {
        return task_error("task id already active");
    }
    if deadline_at <= now_unix() {
        return task_error("task deadline has passed");
    }
    if !local_manifest
        .body
        .capabilities
        .iter()
        .any(|advertised| advertised.id == capability)
    {
        return task_error("capability not advertised");
    }
    let input_size = serde_json::to_vec(&input)
        .map(|bytes| bytes.len())
        .unwrap_or(usize::MAX);
    if input_size > MAX_TASK_INPUT_BYTES {
        return task_error("task input too large");
    }

    state.incoming_tasks.insert(
        task_id.clone(),
        IncomingTask {
            peer,
            capability,
            input,
            deadline_at,
        },
    );
    RpcResponse::TaskAccepted { task_id }
}

fn handle_task_ready(
    swarm: &mut Swarm<Behaviour>,
    peer: PeerId,
    task_id: String,
    state: &mut NodeState,
) -> RpcResponse {
    let Some(task) = state.incoming_tasks.remove(&task_id) else {
        return task_error("unknown task");
    };
    if task.peer != peer {
        return task_error("task peer mismatch");
    }

    if task.deadline_at <= now_unix() {
        send_task_event(
            swarm,
            peer,
            &task_id,
            1,
            TaskEventKind::Failed {
                message: "task deadline expired".to_owned(),
            },
        );
        send_task_event(swarm, peer, &task_id, 2, TaskEventKind::Done);
        return RpcResponse::TaskReadyAck { task_id };
    }

    match task.capability.as_str() {
        "task.echo" => {
            send_task_event(
                swarm,
                peer,
                &task_id,
                1,
                TaskEventKind::Started {
                    capability: task.capability.clone(),
                },
            );
            send_task_event(
                swarm,
                peer,
                &task_id,
                2,
                TaskEventKind::Progress {
                    message: "processing".to_owned(),
                    fraction: Some(0.5),
                },
            );
            send_task_event(
                swarm,
                peer,
                &task_id,
                3,
                TaskEventKind::Result {
                    output: serde_json::json!({ "echo": task.input }),
                },
            );
            send_task_event(swarm, peer, &task_id, 4, TaskEventKind::Done);
        }
        _ => {
            send_task_event(
                swarm,
                peer,
                &task_id,
                1,
                TaskEventKind::Failed {
                    message: "no local handler for capability".to_owned(),
                },
            );
            send_task_event(swarm, peer, &task_id, 2, TaskEventKind::Done);
        }
    }

    RpcResponse::TaskReadyAck { task_id }
}

fn handle_task_event(
    state: &mut NodeState,
    task_id: String,
    sequence: u64,
    event: TaskEventKind,
) -> RpcResponse {
    if !state.active_tasks.contains_key(&task_id) {
        return task_error("unknown task");
    }

    let terminal = matches!(&event, TaskEventKind::Done);
    if let Some(active) = state.active_tasks.get_mut(&task_id) {
        if sequence <= active.last_sequence {
            return RpcResponse::TaskEventAck { task_id, sequence };
        }
        active.last_sequence = sequence;
        let _ = active.events.try_send(TaskEvent {
            task_id: task_id.clone(),
            sequence,
            event,
        });
    }
    if terminal {
        state.active_tasks.remove(&task_id);
    }

    RpcResponse::TaskEventAck { task_id, sequence }
}

fn send_task_event(
    swarm: &mut Swarm<Behaviour>,
    peer: PeerId,
    task_id: &str,
    sequence: u64,
    event: TaskEventKind,
) {
    let _ = swarm.behaviour_mut().rpc.send_request(
        &peer,
        RpcRequest::TaskEventPush {
            task_id: task_id.to_owned(),
            sequence,
            event,
        },
    );
}

fn new_task_id() -> String {
    let mut bytes = [0u8; 16];
    OsRng.fill_bytes(&mut bytes);
    URL_SAFE_NO_PAD.encode(bytes)
}

fn respond_to_request(
    swarm: &mut Swarm<Behaviour>,
    peer: PeerId,
    request: RpcRequest,
    channel: ResponseChannel<RpcResponse>,
    local_manifest: &Manifest,
    state: &mut NodeState,
    config: NodeConfig,
) {
    let response = match request {
        RpcRequest::Manifest(manifest) => match verify_manifest_for_peer(&manifest, &peer) {
            Ok(()) => {
                state.manifests.insert(peer, *manifest);
                RpcResponse::Manifest(Box::new(local_manifest.clone()))
            }
            Err(error) => RpcResponse::Error {
                message: error.to_string(),
            },
        },
        RpcRequest::Echo { message } => {
            if state.manifests.contains_key(&peer) {
                RpcResponse::Echo { message }
            } else {
                RpcResponse::Error {
                    message: "manifest exchange required".to_owned(),
                }
            }
        }
        RpcRequest::Neighbors {
            capability,
            max,
            nonce,
        } => {
            let records = answer_neighbors(
                state,
                &peer,
                capability.as_deref(),
                max,
                config.neighbor_policy,
            );
            RpcResponse::Neighbors {
                records,
                nonce,
                policy: config.neighbor_policy.as_str().to_owned(),
            }
        }
        RpcRequest::TaskStart {
            task_id,
            capability,
            input,
            deadline_at,
        } => handle_task_start(
            peer,
            task_id,
            capability,
            input,
            deadline_at,
            local_manifest,
            state,
        ),
        RpcRequest::TaskReady { task_id } => handle_task_ready(swarm, peer, task_id, state),
        RpcRequest::TaskEventPush {
            task_id,
            sequence,
            event,
        } => handle_task_event(state, task_id, sequence, event),
    };

    let _ = swarm.behaviour_mut().rpc.send_response(channel, response);
}

fn handle_response(
    swarm: &mut Swarm<Behaviour>,
    peer: PeerId,
    request_id: OutboundRequestId,
    response: RpcResponse,
    state: &mut NodeState,
) {
    if let Some(pending) = state.pending_asks.remove(&request_id) {
        let PendingAsk {
            task_id: expected_task_id,
            events,
            receiver,
            reply,
        } = pending;

        let result = match response {
            RpcResponse::TaskAccepted { task_id } if task_id == expected_task_id => {
                let Some(receiver) = receiver else {
                    let _ = reply.send(Err(Error::Request(
                        "task handle receiver already consumed".to_owned(),
                    )));
                    return;
                };
                state.active_tasks.insert(
                    task_id.clone(),
                    ActiveTask {
                        events,
                        last_sequence: 0,
                    },
                );
                let _ = swarm.behaviour_mut().rpc.send_request(
                    &peer,
                    RpcRequest::TaskReady {
                        task_id: task_id.clone(),
                    },
                );
                Ok(TaskHandle {
                    task_id,
                    events: receiver,
                })
            }
            RpcResponse::Error { message } => Err(Error::Request(message)),
            _ => Err(Error::Request("unexpected task response".to_owned())),
        };
        let _ = reply.send(result);
        return;
    }

    if let Some(reply) = state.pending_manifests.remove(&request_id) {
        let result = match response {
            RpcResponse::Manifest(manifest) => {
                let manifest = *manifest;
                match verify_manifest_for_peer(&manifest, &peer) {
                    Ok(()) => {
                        state.manifests.insert(peer, manifest.clone());
                        Ok(manifest)
                    }
                    Err(error) => Err(error),
                }
            }
            RpcResponse::Error { message } => Err(Error::Request(message)),
            RpcResponse::Echo { .. }
            | RpcResponse::Neighbors { .. }
            | RpcResponse::TaskAccepted { .. }
            | RpcResponse::TaskReadyAck { .. }
            | RpcResponse::TaskEventAck { .. } => {
                Err(Error::Request("unexpected manifest response".to_owned()))
            }
        };
        let _ = reply.send(result);
        return;
    }

    if let Some(reply) = state.pending_echoes.remove(&request_id) {
        let result = match response {
            RpcResponse::Echo { message } => Ok(message),
            RpcResponse::Error { message } => Err(Error::Request(message)),
            RpcResponse::Manifest(_)
            | RpcResponse::Neighbors { .. }
            | RpcResponse::TaskAccepted { .. }
            | RpcResponse::TaskReadyAck { .. }
            | RpcResponse::TaskEventAck { .. } => {
                Err(Error::Request("unexpected echo response".to_owned()))
            }
        };
        let _ = reply.send(result);
        return;
    }

    if let Some((expected_nonce, max, capability, reply)) =
        state.pending_neighbors.remove(&request_id)
    {
        let result = match response {
            RpcResponse::Neighbors { records, nonce, .. } if nonce == expected_nonce => {
                let total = records.len();
                let valid = validate_contacts(records, capability.as_deref(), max);
                if valid.len() == total {
                    record_source_success(state, &peer);
                } else {
                    record_source_failure(state, &peer);
                }
                Ok(valid)
            }
            RpcResponse::Neighbors { .. } => {
                record_source_failure(state, &peer);
                Err(Error::Request(
                    "neighbor response nonce mismatch".to_owned(),
                ))
            }
            RpcResponse::Error { message } => {
                record_source_failure(state, &peer);
                Err(Error::Request(message))
            }
            _ => {
                record_source_failure(state, &peer);
                Err(Error::Request("unexpected neighbor response".to_owned()))
            }
        };
        let _ = reply.send(result);
    }
}

fn ensure_direct_allowed(policy: ConnectivityPolicy) -> Result<()> {
    match policy {
        ConnectivityPolicy::Auto | ConnectivityPolicy::Direct => Ok(()),
        ConnectivityPolicy::Relay => Err(Error::Unsupported(
            "relay transport is not implemented; direct dial is disabled by policy".to_owned(),
        )),
        ConnectivityPolicy::Privacy => Err(Error::Unsupported(
            "Tor/I2P transport is not implemented; direct dial is disabled by policy".to_owned(),
        )),
    }
}

fn classify_multiaddr(addr: &Multiaddr) -> Reachability {
    for protocol in addr.iter() {
        match protocol {
            Protocol::Ip4(ip) => return classify_ipv4(ip),
            Protocol::Ip6(ip) => return classify_ipv6(ip),
            _ => {}
        }
    }
    Reachability::Unknown
}

fn classify_ipv4(ip: Ipv4Addr) -> Reachability {
    if ip.is_loopback() {
        Reachability::Loopback
    } else if ip.is_unspecified() {
        Reachability::Unknown
    } else if ip.is_private() {
        Reachability::Private
    } else {
        Reachability::PublicUnverified
    }
}

fn classify_ipv6(ip: Ipv6Addr) -> Reachability {
    if ip.is_loopback() {
        Reachability::Loopback
    } else if ip.is_unspecified() {
        Reachability::Unknown
    } else if ip.is_unique_local() || ip.is_unicast_link_local() {
        Reachability::Private
    } else {
        Reachability::PublicUnverified
    }
}

fn endpoint_record(addr: &Multiaddr, source: &str, verified: bool) -> EndpointRecord {
    EndpointRecord {
        addr: addr.to_string(),
        transport: TransportKind::Tcp,
        reachability: classify_multiaddr(addr),
        source: source.to_owned(),
        verified,
    }
}

fn transport_statuses() -> Vec<TransportStatus> {
    vec![
        TransportStatus {
            transport: TransportKind::Tcp,
            available: true,
            note: "direct TCP + Noise".to_owned(),
        },
        TransportStatus {
            transport: TransportKind::Relay,
            available: false,
            note: "planned for M4b".to_owned(),
        },
        TransportStatus {
            transport: TransportKind::Tor,
            available: false,
            note: "planned for M4c".to_owned(),
        },
        TransportStatus {
            transport: TransportKind::I2P,
            available: false,
            note: "planned for M4c".to_owned(),
        },
    ]
}

fn build_network_status(
    peer_id: &PeerId,
    local_manifest: &Manifest,
    state: &NodeState,
) -> NetworkStatus {
    let listen = local_manifest.body.endpoints.first().cloned();
    let reachability = listen
        .as_deref()
        .and_then(|value| value.parse::<Multiaddr>().ok())
        .map(|addr| classify_multiaddr(&addr))
        .unwrap_or(Reachability::Unknown);

    let mut endpoints = state.address_book.records();
    if let Some(addr) = &listen {
        if !endpoints.iter().any(|record| &record.addr == addr) {
            if let Ok(parsed) = addr.parse::<Multiaddr>() {
                endpoints.push(endpoint_record(&parsed, "listen", false));
            }
        }
    }

    NetworkStatus {
        agent_id: local_manifest.body.agent_id.to_string(),
        peer_id: peer_id.to_string(),
        listen,
        policy: state.connectivity_policy,
        reachability,
        transports: transport_statuses(),
        endpoints,
    }
}

fn handle_command(
    swarm: &mut Swarm<Behaviour>,
    command: Command,
    local_manifest: &Manifest,
    state: &mut NodeState,
) -> bool {
    match command {
        Command::Dial { addr, reply } => {
            if let Err(error) = ensure_direct_allowed(state.connectivity_policy) {
                let _ = reply.send(Err(error));
                return true;
            }
            let peer = match peer_from_multiaddr(&addr) {
                Some(peer) => peer,
                None => {
                    let _ = reply.send(Err(Error::Dial(
                        "multiaddr must include /p2p/<peer-id>".to_owned(),
                    )));
                    return true;
                }
            };

            let endpoint = endpoint_record(&addr, "dial", false);
            if swarm.is_connected(&peer) {
                let mut verified = endpoint;
                verified.verified = true;
                state.address_book.upsert(verified);
                let _ = reply.send(Ok(peer));
                return true;
            }

            match swarm.dial(addr) {
                Ok(()) => {
                    state
                        .pending_dial_endpoints
                        .insert(peer, endpoint.addr.clone());
                    state.address_book.upsert(endpoint);
                    state.pending_dials.insert(peer, reply);
                }
                Err(error) => {
                    let _ = reply.send(Err(Error::Dial(error.to_string())));
                }
            }
            true
        }
        Command::Manifest { peer, reply } => {
            if let Err(error) = ensure_direct_allowed(state.connectivity_policy) {
                let _ = reply.send(Err(error));
                return true;
            }
            let request_id = swarm.behaviour_mut().rpc.send_request(
                &peer,
                RpcRequest::Manifest(Box::new(local_manifest.clone())),
            );
            state.pending_manifests.insert(request_id, reply);
            true
        }
        Command::Echo {
            peer,
            message,
            reply,
        } => {
            if let Err(error) = ensure_direct_allowed(state.connectivity_policy) {
                let _ = reply.send(Err(error));
                return true;
            }
            let request_id = swarm
                .behaviour_mut()
                .rpc
                .send_request(&peer, RpcRequest::Echo { message });
            state.pending_echoes.insert(request_id, reply);
            true
        }
        Command::Neighbors {
            peer,
            capability,
            max,
            nonce,
            reply,
        } => {
            if let Err(error) = ensure_direct_allowed(state.connectivity_policy) {
                let _ = reply.send(Err(error));
                return true;
            }
            if source_is_quarantined(state, &peer) {
                let _ = reply.send(Err(Error::Request("introducer is quarantined".to_owned())));
                return true;
            }
            let request_id = swarm.behaviour_mut().rpc.send_request(
                &peer,
                RpcRequest::Neighbors {
                    capability: capability.clone(),
                    max,
                    nonce,
                },
            );
            state
                .pending_neighbors
                .insert(request_id, (nonce, max, capability, reply));
            true
        }
        Command::Publish { reply } => {
            let bytes = match serde_json::to_vec(local_manifest) {
                Ok(bytes) => bytes,
                Err(error) => {
                    let _ = reply.send(Err(Error::Serialization(error.to_string())));
                    return true;
                }
            };
            let mut record = Record::new(manifest_key(&local_manifest.body.agent_id), bytes);
            record.expires = Some(Instant::now() + Duration::from_secs(MANIFEST_TTL_SECONDS));
            let query = match swarm.behaviour_mut().kad.put_record(record, Quorum::One) {
                Ok(query) => query,
                Err(error) => {
                    let _ = reply.send(Err(Error::Dht(error.to_string())));
                    return true;
                }
            };
            state
                .pending_dht
                .insert(query, DhtPending::Publish { reply });
            true
        }
        Command::Lookup { agent_id, reply } => {
            let query = swarm
                .behaviour_mut()
                .kad
                .get_record(manifest_key(&agent_id));
            state.pending_dht.insert(
                query,
                DhtPending::Lookup {
                    agent_id,
                    found: None,
                    reply,
                },
            );
            true
        }
        Command::AddAddress { peer, addr, reply } => {
            state
                .address_book
                .upsert(endpoint_record(&addr, "manual", false));
            let _ = swarm.behaviour_mut().kad.add_address(&peer, addr);
            let _ = reply.send(Ok(()));
            true
        }
        Command::Bootstrap { reply } => {
            let query = match swarm.behaviour_mut().kad.bootstrap() {
                Ok(query) => query,
                Err(error) => {
                    let _ = reply.send(Err(Error::Dht(error.to_string())));
                    return true;
                }
            };
            state
                .pending_dht
                .insert(query, DhtPending::Bootstrap { reply });
            true
        }
        Command::Provide { capability, reply } => {
            let query = match swarm
                .behaviour_mut()
                .kad
                .start_providing(capability_key(&capability))
            {
                Ok(query) => query,
                Err(error) => {
                    let _ = reply.send(Err(Error::Dht(error.to_string())));
                    return true;
                }
            };
            state
                .pending_dht
                .insert(query, DhtPending::Provide { reply });
            true
        }
        Command::Providers { capability, reply } => {
            let query = swarm
                .behaviour_mut()
                .kad
                .get_providers(capability_key(&capability));
            state.pending_dht.insert(
                query,
                DhtPending::Providers {
                    collected: HashSet::new(),
                    reply,
                },
            );
            true
        }
        Command::DialPeer { peer, reply } => {
            if let Err(error) = ensure_direct_allowed(state.connectivity_policy) {
                let _ = reply.send(Err(error));
                return true;
            }
            if swarm.is_connected(&peer) {
                let _ = reply.send(Ok(peer));
                return true;
            }
            match swarm.dial(peer) {
                Ok(()) => {
                    state.pending_dials.insert(peer, reply);
                }
                Err(error) => {
                    let _ = reply.send(Err(Error::Dial(error.to_string())));
                }
            }
            true
        }
        Command::Ask {
            peer,
            capability,
            input,
            deadline_at,
            reply,
        } => {
            if let Err(error) = ensure_direct_allowed(state.connectivity_policy) {
                let _ = reply.send(Err(error));
                return true;
            }
            let (events, receiver) = mpsc::channel(32);
            let task_id = new_task_id();
            let request_id = swarm.behaviour_mut().rpc.send_request(
                &peer,
                RpcRequest::TaskStart {
                    task_id: task_id.clone(),
                    capability,
                    input,
                    deadline_at,
                },
            );
            state.pending_asks.insert(
                request_id,
                PendingAsk {
                    task_id,
                    events,
                    receiver: Some(receiver),
                    reply,
                },
            );
            true
        }
        Command::Status { reply } => {
            let status = build_network_status(swarm.local_peer_id(), local_manifest, state);
            let _ = reply.send(status);
            true
        }
        Command::Shutdown => false,
    }
}

fn handle_kad_event(event: kad::Event, state: &mut NodeState) {
    let kad::Event::OutboundQueryProgressed {
        id, result, step, ..
    } = event
    else {
        return;
    };

    if let Some(DhtPending::Lookup {
        agent_id, found, ..
    }) = state.pending_dht.get_mut(&id)
    {
        update_found(found, agent_id, &result);
    }

    if let Some(DhtPending::Providers { collected, .. }) = state.pending_dht.get_mut(&id) {
        if let QueryResult::GetProviders(Ok(GetProvidersOk::FoundProviders { providers, .. })) =
            &result
        {
            collected.extend(providers.iter().cloned());
        }
    }

    if !step.last {
        return;
    }

    let Some(pending) = state.pending_dht.remove(&id) else {
        return;
    };

    match pending {
        DhtPending::Publish { reply } => {
            let outcome = match result {
                QueryResult::PutRecord(Ok(_)) => Ok(()),
                QueryResult::PutRecord(Err(error)) => Err(Error::Dht(error.to_string())),
                other => Err(Error::Dht(format!("unexpected DHT result: {other:?}"))),
            };
            let _ = reply.send(outcome);
        }
        DhtPending::Lookup { found, reply, .. } => {
            let outcome = match found {
                Some(manifest) => Ok(*manifest),
                None => Err(Error::Dht("manifest not found in DHT".to_owned())),
            };
            let _ = reply.send(outcome);
        }
        DhtPending::Bootstrap { reply } => {
            let outcome = match result {
                QueryResult::Bootstrap(Ok(_)) => Ok(()),
                QueryResult::Bootstrap(Err(error)) => Err(Error::Dht(error.to_string())),
                other => Err(Error::Dht(format!("unexpected DHT result: {other:?}"))),
            };
            let _ = reply.send(outcome);
        }
        DhtPending::Provide { reply } => {
            let outcome = match result {
                QueryResult::StartProviding(Ok(_)) => Ok(()),
                QueryResult::StartProviding(Err(error)) => Err(Error::Dht(error.to_string())),
                other => Err(Error::Dht(format!("unexpected DHT result: {other:?}"))),
            };
            let _ = reply.send(outcome);
        }
        DhtPending::Providers { collected, reply } => {
            let mut peers: Vec<PeerId> = collected.into_iter().collect();
            peers.sort_by_key(|peer| peer.to_string());
            let _ = reply.send(Ok(peers));
        }
    }
}

fn update_found(found: &mut Option<Box<Manifest>>, agent_id: &AgentId, result: &QueryResult) {
    if found.is_some() {
        return;
    }
    let QueryResult::GetRecord(Ok(GetRecordOk::FoundRecord(peer_record))) = result else {
        return;
    };
    let Ok(manifest) = serde_json::from_slice::<Manifest>(&peer_record.record.value) else {
        return;
    };
    if &manifest.body.agent_id == agent_id && manifest.verify().is_ok() {
        *found = Some(Box::new(manifest));
    }
}

fn manifest_key(agent_id: &AgentId) -> RecordKey {
    let digest = Sha256::digest(format!("aan/id/{agent_id}").as_bytes());
    RecordKey::new(&digest)
}

fn capability_key(capability: &str) -> RecordKey {
    let normalized = capability.trim().to_ascii_lowercase();
    let digest = Sha256::digest(format!("aan/cap/{normalized}").as_bytes());
    RecordKey::new(&digest)
}

fn record_source_success(state: &mut NodeState, peer: &PeerId) {
    let stats = state.sources.entry(*peer).or_default();
    stats.failures = 0;
    stats.quarantined_until = None;
}

fn record_source_failure(state: &mut NodeState, peer: &PeerId) {
    let stats = state.sources.entry(*peer).or_default();
    stats.failures = stats.failures.saturating_add(1);
    if stats.failures >= QUARANTINE_FAILURES {
        stats.quarantined_until = Some(Instant::now() + QUARANTINE_DURATION);
    }
}

fn source_is_quarantined(state: &NodeState, peer: &PeerId) -> bool {
    state
        .sources
        .get(peer)
        .and_then(|stats| stats.quarantined_until)
        .is_some_and(|until| until > Instant::now())
}

fn answer_neighbors(
    state: &NodeState,
    requester: &PeerId,
    capability: Option<&str>,
    max: u32,
    policy: NeighborPolicy,
) -> Vec<ContactRecord> {
    if policy == NeighborPolicy::Stealth {
        return Vec::new();
    }

    let limit = max.clamp(1, 32) as usize;
    let observed_at = now_unix();
    state
        .manifests
        .iter()
        .filter(|(peer, _)| *peer != requester)
        .filter(|(_, manifest)| {
            capability.is_none_or(|cap| {
                manifest
                    .body
                    .capabilities
                    .iter()
                    .any(|capability| capability.id == cap)
            })
        })
        .take(limit)
        .map(|(_, manifest)| ContactRecord {
            manifest: manifest.clone(),
            observed_at,
        })
        .collect()
}

fn validate_contacts(
    records: Vec<ContactRecord>,
    capability: Option<&str>,
    max: u32,
) -> Vec<ContactRecord> {
    let limit = max.clamp(1, 32) as usize;
    let mut seen = HashSet::new();
    let mut valid = Vec::new();

    for record in records {
        if valid.len() >= limit {
            break;
        }
        if record.manifest.verify().is_err() {
            continue;
        }
        if let Some(cap) = capability {
            if !record
                .manifest
                .body
                .capabilities
                .iter()
                .any(|capability| capability.id == cap)
            {
                continue;
            }
        }
        if seen.insert(record.manifest.body.agent_id.to_string()) {
            valid.push(record);
        }
    }

    valid
}

fn now_unix() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .unwrap_or(0)
}

fn verify_manifest_for_peer(manifest: &Manifest, peer: &PeerId) -> Result<()> {
    manifest.verify()?;
    let expected = peer_id_for_agent_id(&manifest.body.agent_id)?;
    if &expected != peer {
        return Err(Error::PeerMismatch {
            expected: expected.to_string(),
            actual: peer.to_string(),
        });
    }
    Ok(())
}

fn peer_id_for_agent_id(agent_id: &AgentId) -> Result<PeerId> {
    let public = libp2p::identity::ed25519::PublicKey::try_from_bytes(agent_id.as_bytes())
        .map_err(|error| Error::Identity(error.to_string()))?;
    Ok(PeerId::from_public_key(&libp2p::identity::PublicKey::from(
        public,
    )))
}

fn peer_from_multiaddr(address: &Multiaddr) -> Option<PeerId> {
    address.iter().find_map(|protocol| match protocol {
        Protocol::P2p(peer) => Some(peer),
        _ => None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn manifest_peer_id_matches_agent_id() {
        let identity = Identity::generate();
        let peer = peer_id_for_agent_id(&identity.agent_id()).unwrap();
        let keypair = {
            let mut seed = identity.to_seed();
            Keypair::ed25519_from_bytes(&mut seed.expose()[..]).unwrap()
        };
        assert_eq!(peer, keypair.public().to_peer_id());
    }

    #[test]
    fn rejects_manifest_from_a_different_peer() {
        let signer = Identity::generate();
        let other = Identity::generate();
        let mut body = ManifestBody::new(signer.agent_id(), 1, MANIFEST_TTL_SECONDS);
        body.capabilities.push(Capability::new("task.echo"));
        let manifest = Manifest::sign(body, &signer).unwrap();
        let peer = peer_id_for_agent_id(&other.agent_id()).unwrap();

        assert!(matches!(
            verify_manifest_for_peer(&manifest, &peer),
            Err(Error::PeerMismatch { .. })
        ));
    }

    fn signed_manifest(identity: &Identity, capability: &str) -> Manifest {
        let mut body = ManifestBody::new(identity.agent_id(), 1, MANIFEST_TTL_SECONDS);
        body.capabilities.push(Capability::new(capability));
        Manifest::sign(body, identity).unwrap()
    }

    fn contact(manifest: Manifest) -> ContactRecord {
        ContactRecord {
            manifest,
            observed_at: 0,
        }
    }

    #[test]
    fn filters_forged_expired_and_wrong_capability() {
        let identity = Identity::generate();
        let valid = contact(signed_manifest(&identity, "task.echo"));

        let mut forged_manifest = signed_manifest(&identity, "task.echo");
        forged_manifest.body.capabilities[0].id = "task.other".to_owned();
        let forged = contact(forged_manifest);

        let expired_identity = Identity::generate();
        let mut expired_body =
            ManifestBody::new(expired_identity.agent_id(), 1, MANIFEST_TTL_SECONDS);
        expired_body.issued_at = 1;
        expired_body.expires_at = 2;
        expired_body.capabilities.push(Capability::new("task.echo"));
        let expired = contact(Manifest::sign(expired_body, &expired_identity).unwrap());

        let wrong_identity = Identity::generate();
        let wrong_capability = contact(signed_manifest(&wrong_identity, "task.other"));

        let filtered = validate_contacts(
            vec![valid, forged, expired, wrong_capability],
            Some("task.echo"),
            8,
        );

        assert_eq!(filtered.len(), 1);
        assert_eq!(filtered[0].manifest.body.agent_id, identity.agent_id());
    }

    #[test]
    fn deduplicates_contacts() {
        let identity = Identity::generate();
        let duplicate = contact(signed_manifest(&identity, "task.echo"));

        let filtered = validate_contacts(vec![duplicate.clone(), duplicate], Some("task.echo"), 8);

        assert_eq!(filtered.len(), 1);
    }

    #[test]
    fn classifies_loopback_private_and_public_endpoints() {
        assert_eq!(
            classify_multiaddr(&"/ip4/127.0.0.1/tcp/4001".parse().unwrap()),
            Reachability::Loopback
        );
        assert_eq!(
            classify_multiaddr(&"/ip4/192.168.1.10/tcp/4001".parse().unwrap()),
            Reachability::Private
        );
        assert_eq!(
            classify_multiaddr(&"/ip4/203.0.113.10/tcp/4001".parse().unwrap()),
            Reachability::PublicUnverified
        );
        assert_eq!(
            classify_multiaddr(&"/ip6/::1/tcp/4001".parse().unwrap()),
            Reachability::Loopback
        );
    }

    #[test]
    fn privacy_and_relay_policies_fail_closed() {
        assert!(ensure_direct_allowed(ConnectivityPolicy::Auto).is_ok());
        assert!(ensure_direct_allowed(ConnectivityPolicy::Direct).is_ok());
        assert!(ensure_direct_allowed(ConnectivityPolicy::Relay).is_err());
        assert!(ensure_direct_allowed(ConnectivityPolicy::Privacy).is_err());
    }

    #[test]
    fn quarantines_repeated_source_failures() {
        let mut state = NodeState::default();
        let peer = peer_id_for_agent_id(&Identity::generate().agent_id()).unwrap();

        for _ in 0..QUARANTINE_FAILURES {
            record_source_failure(&mut state, &peer);
        }
        assert!(source_is_quarantined(&state, &peer));

        record_source_success(&mut state, &peer);
        assert!(!source_is_quarantined(&state, &peer));
    }

    #[test]
    fn respects_the_requested_record_limit() {
        let mut records = Vec::new();
        for _ in 0..3 {
            records.push(contact(signed_manifest(&Identity::generate(), "task.echo")));
        }

        assert_eq!(validate_contacts(records, Some("task.echo"), 2).len(), 2);
    }
}
