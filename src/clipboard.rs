use std::sync::{Arc, Mutex};

use ironrdp_cliprdr::{
    backend::CliprdrBackend,
    pdu::{
        ClipboardFormat, ClipboardFormatId, ClipboardGeneralCapabilityFlags, FileContentsRequest,
        FileContentsResponse, FormatDataRequest, FormatDataResponse, LockDataId,
    },
};
use ironrdp_core::impl_as_any;
use tracing::debug;

/// Format ID we use when advertising `FileGroupDescriptorW`.
/// Must be in the dynamic range (>= 0xC000).
pub(crate) const FILE_LIST_FORMAT_ID: ClipboardFormatId = ClipboardFormatId(0xC0A0);

/// File data staged for sending via clipboard.
#[derive(Debug)]
pub(crate) struct PendingFileSend {
    pub name: String,
    pub data: Vec<u8>,
}

/// Shared state between the clipboard backend (callback-driven) and the session.
#[derive(Debug)]
pub(crate) struct ClipboardState {
    pub ready: bool,
    pub remote_formats: Vec<ClipboardFormat>,
    pub received_data: Option<String>,
    pub pending_send: Option<String>,
    /// Set when the server requests format data from us.
    pub data_requested: bool,
    /// Set when the CLIPRDR init handshake requests our format list.
    pub format_list_requested: bool,

    // --- File transfer state ---
    /// File staged for sending to the remote.
    pub pending_file_send: Option<PendingFileSend>,
    /// Set when server requests our file list descriptor.
    pub file_list_data_requested: bool,
    /// Pending file contents request from server (SIZE or DATA).
    pub file_contents_request: Option<FileContentsRequest>,
    /// Format ID the remote uses for `FileGroupDescriptorW` (detected from `on_remote_copy`).
    pub remote_file_list_format_id: Option<ClipboardFormatId>,
    /// Received file list from remote (for recv-file).
    pub received_file_list: Option<Vec<ReceivedFileInfo>>,
    /// Received file contents data from remote.
    pub received_file_contents: Option<ReceivedFileContents>,
}

/// Metadata about a file offered by the remote.
#[derive(Debug, Clone)]
pub(crate) struct ReceivedFileInfo {
    pub name: String,
    pub size: Option<u64>,
}

/// Data received from a file contents response.
#[derive(Debug)]
pub(crate) enum ReceivedFileContents {
    Size(u64),
    Data(Vec<u8>),
    Error,
}

impl ClipboardState {
    fn new() -> Self {
        Self {
            ready: false,
            remote_formats: Vec::new(),
            received_data: None,
            pending_send: None,
            data_requested: false,
            format_list_requested: false,
            pending_file_send: None,
            file_list_data_requested: false,
            file_contents_request: None,
            remote_file_list_format_id: None,
            received_file_list: None,
            received_file_contents: None,
        }
    }
}

/// Clipboard backend implementing `CliprdrBackend`.
///
/// All state is behind `Arc<Mutex<>>` so both the backend callbacks (called by
/// the CLIPRDR processor) and the session orchestrator can access it.
#[derive(Debug)]
pub(crate) struct ClipboardBackend {
    state: Arc<Mutex<ClipboardState>>,
}

impl_as_any!(ClipboardBackend);

impl ClipboardBackend {
    pub(crate) fn new() -> Self {
        Self {
            state: Arc::new(Mutex::new(ClipboardState::new())),
        }
    }

    pub(crate) fn state(&self) -> Arc<Mutex<ClipboardState>> {
        Arc::clone(&self.state)
    }
}

impl CliprdrBackend for ClipboardBackend {
    fn temporary_directory(&self) -> &'static str {
        "/tmp/rdpdo-cliprdr"
    }

    fn client_capabilities(&self) -> ClipboardGeneralCapabilityFlags {
        ClipboardGeneralCapabilityFlags::USE_LONG_FORMAT_NAMES
            | ClipboardGeneralCapabilityFlags::STREAM_FILECLIP_ENABLED
            | ClipboardGeneralCapabilityFlags::FILECLIP_NO_FILE_PATHS
            | ClipboardGeneralCapabilityFlags::HUGE_FILE_SUPPORT_ENABLED
    }

    fn on_ready(&mut self) {
        debug!("CLIPRDR backend ready");
        self.state.lock().expect("clipboard lock").ready = true;
    }

    fn on_process_negotiated_capabilities(
        &mut self,
        capabilities: ClipboardGeneralCapabilityFlags,
    ) {
        debug!(?capabilities, "CLIPRDR negotiated capabilities");
    }

    fn on_remote_copy(&mut self, available_formats: &[ClipboardFormat]) {
        debug!(
            count = available_formats.len(),
            "Remote copy: formats available"
        );
        let mut state = self.state.lock().expect("clipboard lock");
        state.remote_formats = available_formats.to_vec();

        // Detect if the remote is offering files (FileGroupDescriptorW format)
        state.remote_file_list_format_id = available_formats.iter().find_map(|f| {
            f.name
                .as_ref()
                .filter(|n| n.value() == "FileGroupDescriptorW")
                .map(|_| f.id)
        });
        if state.remote_file_list_format_id.is_some() {
            debug!("Remote is offering file list via clipboard");
        }
    }

    fn on_format_data_request(&mut self, request: FormatDataRequest) {
        debug!(?request.format, "Server requested format data from us");
        let mut state = self.state.lock().expect("clipboard lock");
        if request.format == FILE_LIST_FORMAT_ID {
            state.file_list_data_requested = true;
        } else {
            state.data_requested = true;
        }
    }

    fn on_format_data_response(&mut self, response: FormatDataResponse<'_>) {
        if response.is_error() {
            debug!("Received error format data response");
            return;
        }

        let mut state = self.state.lock().expect("clipboard lock");

        // If we're expecting a file list response, try to parse it first
        if state.received_file_list.is_none()
            && let Ok(file_list) = response.to_file_list()
        {
            debug!(
                count = file_list.files.len(),
                "Received file list from server"
            );
            let infos: Vec<ReceivedFileInfo> = file_list
                .files
                .iter()
                .map(|fd| ReceivedFileInfo {
                    name: fd.name.clone(),
                    size: fd.file_size,
                })
                .collect();
            state.received_file_list = Some(infos);
            return;
        }

        // Otherwise treat as text
        match response.to_unicode_string() {
            Ok(text) => {
                debug!(len = text.len(), "Received clipboard text from server");
                state.received_data = Some(text);
            }
            Err(e) => {
                debug!(?e, "Failed to decode clipboard unicode string");
            }
        }
    }

    fn on_request_format_list(&mut self) {
        debug!("Format list requested — signalling session to send initiate_copy");
        self.state
            .lock()
            .expect("clipboard lock")
            .format_list_requested = true;
    }

    fn on_file_contents_request(&mut self, request: FileContentsRequest) {
        debug!(
            stream_id = request.stream_id,
            index = request.index,
            ?request.flags,
            position = request.position,
            requested_size = request.requested_size,
            "Server requested file contents from us"
        );
        self.state
            .lock()
            .expect("clipboard lock")
            .file_contents_request = Some(request);
    }

    fn on_file_contents_response(&mut self, response: FileContentsResponse<'_>) {
        let mut state = self.state.lock().expect("clipboard lock");
        if response.data().is_empty() {
            debug!(
                stream_id = response.stream_id(),
                "File contents response: error/empty"
            );
            state.received_file_contents = Some(ReceivedFileContents::Error);
        } else if response.data().len() == 8 {
            // Could be a size response (8 bytes = u64)
            if let Ok(size) = response.data_as_size() {
                debug!(
                    stream_id = response.stream_id(),
                    size, "File contents response: size"
                );
                state.received_file_contents = Some(ReceivedFileContents::Size(size));
                return;
            }
            // If not a valid u64, treat as data
            debug!(
                stream_id = response.stream_id(),
                len = 8,
                "File contents response: data"
            );
            state.received_file_contents =
                Some(ReceivedFileContents::Data(response.data().to_vec()));
        } else {
            debug!(
                stream_id = response.stream_id(),
                len = response.data().len(),
                "File contents response: data"
            );
            state.received_file_contents =
                Some(ReceivedFileContents::Data(response.data().to_vec()));
        }
    }

    fn on_lock(&mut self, _data_id: LockDataId) {}

    fn on_unlock(&mut self, _data_id: LockDataId) {}
}
