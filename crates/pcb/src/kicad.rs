//! The KiCad adapter: connect, discover instances, enumerate open PCBs, and
//! collect a snapshot from a chosen one.
//!
//! # Several instances
//!
//! More than one KiCad can be running at once, each listening on its own IPC
//! socket. Unless a single instance is forced (see [`should_scan_all_instances`]),
//! [`KiCad::enumerate_pcbs`] scans the well-known socket directory, connects to
//! each live instance, and merges their open PCB documents into one deduplicated
//! list. Every [`PcbInfo`] remembers which instance's socket it came from, so
//! [`KiCad::collect_snapshot`] can route the collection back to the right
//! instance instead of guessing by filename.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use kicad_ipc_rs::{DocumentSpecifier, DocumentType, KiCadClientBlocking as Inner};

use crate::error::PcbError;
use crate::snapshot::{self, BoardSnapshot};

/// A PCB document open in a running KiCad instance — the unit of enumeration.
///
/// Cheap and owned: hold onto it, show it in a list, and pass it back to
/// [`KiCad::collect_snapshot`] when the user picks one.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PcbInfo {
    /// Absolute path of the `.kicad_pcb` file, as KiCad reports it.
    pub board_filename: Option<String>,
    /// The owning project's name, if any.
    pub project_name: Option<String>,
    /// The owning project's path, if any.
    pub project_path: Option<PathBuf>,
    /// Filesystem socket of the instance this document belongs to. Internal
    /// routing detail; `None` means "the instance we first connected to".
    socket: Option<String>,
}

impl PcbInfo {
    /// A short, user-facing label: the board file's name **without extension**,
    /// falling back to the project name, then to a generic placeholder.
    pub fn display_name(&self) -> String {
        if let Some(board) = self.board_filename.as_deref().filter(|s| !s.is_empty()) {
            return Path::new(board)
                .file_stem()
                .and_then(|name| name.to_str())
                .map(str::to_string)
                .unwrap_or_else(|| board.to_string());
        }
        self.project_name
            .clone()
            .filter(|name| !name.is_empty())
            .unwrap_or_else(|| "PCB".to_string())
    }
}

/// A handle to KiCad over its IPC API. Owns the connection to the instance it
/// was created against and can reach sibling instances on demand.
#[derive(Debug)]
pub struct KiCad {
    inner: Inner,
}

impl KiCad {
    /// Connects to KiCad, using whatever socket the environment points at.
    pub fn connect() -> Result<Self, PcbError> {
        Ok(Self {
            inner: Inner::connect().map_err(|e| PcbError::Connection(e.to_string()))?,
        })
    }

    /// The connected KiCad's full version string (e.g. `9.0.1`).
    pub fn version(&self) -> Result<String, PcbError> {
        self.inner
            .get_version()
            .map(|version| version.full_version)
            .map_err(|e| PcbError::Connection(e.to_string()))
    }

    /// Lists every open PCB across all reachable KiCad instances, deduplicated
    /// by board file + project. The order is stable: the connected instance's
    /// documents first, then any discovered on other instances.
    pub fn enumerate_pcbs(&self) -> Result<Vec<PcbInfo>, PcbError> {
        let mut out = Vec::new();
        let mut seen = BTreeSet::new();

        for instance in self.list_instances() {
            for doc in instance.docs {
                if seen.insert(dedup_key(&doc)) {
                    out.push(pcb_info_from(doc, instance.socket.clone()));
                }
            }
        }

        Ok(out)
    }

    /// Whether any reachable instance has an open PCB.
    pub fn has_open_pcb(&self) -> Result<bool, PcbError> {
        Ok(!self.enumerate_pcbs()?.is_empty())
    }

    /// Collects a [`BoardSnapshot`] for a specific enumerated PCB, routing the
    /// query to the instance that owns it.
    pub fn collect_snapshot(&self, pcb: &PcbInfo) -> Result<BoardSnapshot, PcbError> {
        let client = self.resolve_client(pcb);
        let mut snapshot = snapshot::collect(&client).map_err(PcbError::Collect)?;
        // The name lives in the enumerated `PcbInfo`, not the geometry collect.
        snapshot.name = pcb.display_name();
        Ok(snapshot)
    }

    /// Convenience for startup: collect the first open PCB, or `None` if no
    /// board is open anywhere.
    pub fn collect_first_snapshot(&self) -> Result<Option<BoardSnapshot>, PcbError> {
        match self.enumerate_pcbs()?.first() {
            Some(pcb) => Ok(Some(self.collect_snapshot(pcb)?)),
            None => Ok(None),
        }
    }

    // -- instance discovery -------------------------------------------------

    /// The instance we connected to, plus (unless single-instance is forced)
    /// every other live instance found on the well-known socket directory.
    fn list_instances(&self) -> Vec<Instance> {
        let current_socket = self.current_socket_path();

        let mut instances = vec![Instance {
            socket: current_socket.clone(),
            docs: self
                .inner
                .get_open_documents(DocumentType::Pcb)
                .unwrap_or_default(),
        }];

        if !should_scan_all_instances() {
            return instances;
        }

        let mut connected: BTreeSet<String> = current_socket.into_iter().collect();

        for socket_path in scan_socket_paths() {
            let socket = socket_path.to_string_lossy().to_string();
            if !connected.insert(socket.clone()) {
                continue;
            }

            let client = match Inner::builder().socket_path(socket.clone()).connect() {
                Ok(client) => client,
                Err(_) => continue,
            };
            let docs = match client.get_open_documents(DocumentType::Pcb) {
                Ok(docs) => docs,
                Err(_) => continue,
            };

            instances.push(Instance {
                socket: Some(socket),
                docs,
            });
        }

        instances
    }

    /// A client pointed at the instance owning `pcb`: the current connection
    /// when the sockets match (or the PCB carries none), else a fresh
    /// connection to that instance, falling back to the current one on failure.
    fn resolve_client(&self, pcb: &PcbInfo) -> Inner {
        let current = self.current_socket_path();
        match &pcb.socket {
            Some(socket) if Some(socket) != current.as_ref() => Inner::builder()
                .socket_path(socket.clone())
                .connect()
                .unwrap_or_else(|_| self.inner.clone()),
            _ => self.inner.clone(),
        }
    }

    /// The current connection's socket as a filesystem path (matching the form
    /// [`scan_socket_paths`] produces), if it can be parsed from the URI.
    fn current_socket_path(&self) -> Option<String> {
        parse_socket_uri_to_path(self.inner.socket_uri())
            .map(|path| path.to_string_lossy().into_owned())
    }
}

/// One reachable KiCad instance and the PCB documents it has open.
struct Instance {
    socket: Option<String>,
    docs: Vec<DocumentSpecifier>,
}

/// Whether to scan for sibling instances. A single instance is assumed when
/// `K2G_KICAD_SINGLE_INSTANCE` is truthy, or when KiCad exposes one explicit
/// socket through `KICAD_API_SOCKET`.
fn should_scan_all_instances() -> bool {
    if std::env::var("K2G_KICAD_SINGLE_INSTANCE")
        .ok()
        .map(|value| {
            let normalized = value.trim().to_ascii_lowercase();
            matches!(normalized.as_str(), "1" | "true" | "yes" | "on")
        })
        .unwrap_or(false)
    {
        return false;
    }

    !std::env::var("KICAD_API_SOCKET")
        .ok()
        .map(|value| !value.trim().is_empty())
        .unwrap_or(false)
}

/// KiCad IPC socket URIs look like `nng+ipc://<path>`; extract `<path>`.
fn parse_socket_uri_to_path(socket_uri: &str) -> Option<PathBuf> {
    const PREFIX: &str = "nng+ipc://";
    socket_uri
        .strip_prefix(PREFIX)
        .map(PathBuf::from)
        .filter(|path| !path.as_os_str().is_empty())
}

/// Socket files that look like KiCad API endpoints, under `<temp>/kicad`.
fn scan_socket_paths() -> Vec<PathBuf> {
    let mut sockets = BTreeSet::new();

    let socket_dir = std::env::temp_dir().join("kicad");
    if let Ok(entries) = std::fs::read_dir(socket_dir) {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_file() && looks_like_kicad_socket_path(&path) {
                sockets.insert(path);
            }
        }
    }

    sockets.into_iter().collect()
}

fn looks_like_kicad_socket_path(path: &Path) -> bool {
    match path.file_name().and_then(|value| value.to_str()) {
        Some(name) => {
            let name = name.to_ascii_lowercase();
            name.starts_with("api") && name.contains("sock")
        }
        None => false,
    }
}

/// Identity of a document for cross-instance deduplication: board file plus
/// project name and path.
fn dedup_key(doc: &DocumentSpecifier) -> String {
    format!(
        "{}|{}|{}",
        doc.board_filename.clone().unwrap_or_default(),
        doc.project.name.clone().unwrap_or_default(),
        doc.project
            .path
            .as_ref()
            .map(|path| path.to_string_lossy().to_string())
            .unwrap_or_default(),
    )
}

fn pcb_info_from(doc: DocumentSpecifier, socket: Option<String>) -> PcbInfo {
    PcbInfo {
        board_filename: doc.board_filename,
        project_name: doc.project.name,
        project_path: doc.project.path,
        socket,
    }
}
