use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use kicad_ipc_rs::{
    BoardStackup, DocumentSpecifier, DocumentType, ItemBoundingBox, KiCadClientBlocking as InnerClient,
    KiCadError, PcbItem, PcbVia, VersionInfo,
};

#[derive(Debug)]
pub struct KiCadClientBlocking {
    inner: InnerClient,
    preferred_board_filename: Mutex<Option<String>>,
}

#[derive(Clone, Debug)]
struct InstanceClient {
    client: InnerClient,
    pcb_documents: Vec<DocumentSpecifier>,
}

impl KiCadClientBlocking {
    pub fn connect() -> Result<Self, KiCadError> {
        Ok(Self {
            inner: InnerClient::connect()?,
            preferred_board_filename: Mutex::new(None),
        })
    }

    /// Sets a preferred PCB document filename for board-scoped operations.
    ///
    /// When multiple PCB documents are open, callers can set this so app-level
    /// board workflows know which document is intended.
    pub fn set_preferred_board_filename(
        &self,
        board_filename: Option<String>,
    ) -> Result<(), KiCadError> {
        let mut guard = self
            .preferred_board_filename
            .lock()
            .map_err(|_| KiCadError::InternalPoisoned)?;
        *guard = board_filename
            .map(|value| value.trim().to_string())
            .filter(|value| !value.is_empty());
        Ok(())
    }

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

        // If KiCad exposes one explicit socket, we are in a single-instance context.
        !std::env::var("KICAD_API_SOCKET")
            .ok()
            .map(|value| !value.trim().is_empty())
            .unwrap_or(false)
    }

    fn preferred_board_filename(&self) -> Option<String> {
        self.preferred_board_filename
            .lock()
            .ok()
            .and_then(|guard| (*guard).clone())
    }

    fn parse_socket_uri_to_path(socket_uri: &str) -> Option<PathBuf> {
        const PREFIX: &str = "nng+ipc://";
        socket_uri
            .strip_prefix(PREFIX)
            .map(PathBuf::from)
            .filter(|path| !path.as_os_str().is_empty())
    }

    fn discover_socket_paths(&self) -> Vec<PathBuf> {
        let mut sockets = BTreeSet::new();

        if let Some(current) = Self::parse_socket_uri_to_path(self.inner.socket_uri()) {
            sockets.insert(current);
        }

        let socket_dir = std::env::temp_dir().join("kicad");
        if let Ok(entries) = std::fs::read_dir(socket_dir) {
            for entry in entries.flatten() {
                let path = entry.path();
                if !path.is_file() {
                    continue;
                }

                if !Self::looks_like_kicad_socket_path(&path) {
                    continue;
                }

                sockets.insert(path);
            }
        }

        sockets.into_iter().collect()
    }

    fn looks_like_kicad_socket_path(path: &Path) -> bool {
        let file_name = match path.file_name().and_then(|value| value.to_str()) {
            Some(name) => name.to_ascii_lowercase(),
            None => return false,
        };

        file_name.starts_with("api") && file_name.contains("sock")
    }

    fn list_instance_clients_for_pcb(&self) -> Vec<InstanceClient> {
        let mut clients = Vec::new();

        if let Ok(pcb_documents) = self.inner.get_open_documents(DocumentType::Pcb) {
            clients.push(InstanceClient {
                client: self.inner.clone(),
                pcb_documents,
            });
        }

        if !Self::should_scan_all_instances() {
            return clients;
        }

        for socket_path in self.discover_socket_paths() {
            let socket = socket_path.to_string_lossy().to_string();
            let maybe_client = InnerClient::builder().socket_path(socket).connect();
            let client = match maybe_client {
                Ok(client) => client,
                Err(_) => continue,
            };

            let docs = match client.get_open_documents(DocumentType::Pcb) {
                Ok(docs) => docs,
                Err(_) => continue,
            };

            let already_known = clients.iter().any(|instance| {
                instance
                    .pcb_documents
                    .iter()
                    .filter_map(|doc| doc.board_filename.as_ref())
                    .eq(docs.iter().filter_map(|doc| doc.board_filename.as_ref()))
            });

            if !already_known {
                clients.push(InstanceClient {
                    client,
                    pcb_documents: docs,
                });
            }
        }

        clients
    }

    fn selected_client_for_board_ops(&self) -> InnerClient {
        let preferred = self.preferred_board_filename();
        if preferred.is_none() {
            return self.inner.clone();
        }

        let preferred = preferred.unwrap_or_default();
        if preferred.is_empty() {
            return self.inner.clone();
        }

        let instances = self.list_instance_clients_for_pcb();
        if let Some(instance) = instances.into_iter().find(|instance| {
            instance
                .pcb_documents
                .iter()
                .filter_map(|doc| doc.board_filename.as_ref())
                .any(|board| board == &preferred)
        }) {
            return instance.client;
        }

        self.inner.clone()
    }

    pub fn get_open_documents(
        &self,
        document_type: DocumentType,
    ) -> Result<Vec<DocumentSpecifier>, KiCadError> {
        if document_type != DocumentType::Pcb || !Self::should_scan_all_instances() {
            return self.inner.get_open_documents(document_type);
        }

        let instances = self.list_instance_clients_for_pcb();
        if instances.is_empty() {
            return self.inner.get_open_documents(document_type);
        }

        let mut merged = Vec::<DocumentSpecifier>::new();
        let mut seen = BTreeSet::<String>::new();

        for instance in instances {
            for doc in instance.pcb_documents {
                let key = format!(
                    "{}|{}|{}",
                    doc.board_filename.clone().unwrap_or_default(),
                    doc.project.name.clone().unwrap_or_default(),
                    doc.project
                        .path
                        .as_ref()
                        .map(|path| path.to_string_lossy().to_string())
                        .unwrap_or_default(),
                );
                if seen.insert(key) {
                    merged.push(doc);
                }
            }
        }

        Ok(merged)
    }

    pub fn get_version(&self) -> Result<VersionInfo, KiCadError> {
        self.inner.get_version()
    }

    pub fn has_open_board(&self) -> Result<bool, KiCadError> {
        let docs = self.get_open_documents(DocumentType::Pcb)?;
        Ok(!docs.is_empty())
    }

    pub fn get_vias(&self) -> Result<Vec<PcbVia>, KiCadError> {
        self.selected_client_for_board_ops().get_vias()
    }

    pub fn get_item_bounding_boxes(
        &self,
        item_ids: Vec<String>,
        include_child_text: bool,
    ) -> Result<Vec<ItemBoundingBox>, KiCadError> {
        self.selected_client_for_board_ops()
            .get_item_bounding_boxes(item_ids, include_child_text)
    }

    pub fn get_board_stackup(&self) -> Result<BoardStackup, KiCadError> {
        self.selected_client_for_board_ops().get_board_stackup()
    }

    pub fn get_items_by_type_codes(&self, type_codes: Vec<i32>) -> Result<Vec<PcbItem>, KiCadError> {
        self.selected_client_for_board_ops()
            .get_items_by_type_codes(type_codes)
    }
}
