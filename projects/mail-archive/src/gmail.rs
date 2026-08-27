use base64::engine::general_purpose::{URL_SAFE, URL_SAFE_NO_PAD};
use base64::Engine;
use mailparse::{body::Body, parse_mail, DispositionType, MailHeaderMap};
use reqwest::blocking::Client;
use rusqlite::{Connection, OptionalExtension};
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::fs;
use std::io::{Read, Write};
use std::net::TcpListener;
use std::path::Path;
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use url::form_urlencoded;

pub const GMAIL_READONLY_SCOPE: &str = "https://www.googleapis.com/auth/gmail.readonly";

const IMPORT_BATCH_RECORD_LIMIT: usize = 64;
const IMPORT_BATCH_BYTES_LIMIT: u64 = 8 * 1024 * 1024;

#[derive(Debug)]
pub enum GmailError {
    Config(String),
    Http(u16),
    HistoryExpired,
    Json(String),
    Io(String),
    Other(String),
}

impl std::fmt::Display for GmailError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Config(value) => write!(formatter, "configuration: {value}"),
            Self::Http(status) => write!(formatter, "Gmail HTTP status {status}"),
            Self::HistoryExpired => write!(formatter, "Gmail history expired; full sync required"),
            Self::Json(value) => write!(formatter, "invalid Gmail response: {value}"),
            Self::Io(value) => write!(formatter, "I/O error: {value}"),
            Self::Other(value) => write!(formatter, "Gmail error: {value}"),
        }
    }
}

impl std::error::Error for GmailError {}

#[derive(Clone, Debug, Default)]
pub struct SyncStats {
    pub examined: u64,
    pub total: Option<u64>,
    pub new_messages: u64,
    pub label_changes: u64,
    pub deletions: u64,
    pub network_bytes: u64,
    pub archive_bytes_added: u64,
    pub duration_ms: u128,
    pub full_sync: bool,
    pub mime_messages: u64,
    pub mime_parse_failures: u64,
    pub attachments: u64,
    pub attachment_encoded_bytes: u64,
    pub attachment_decoded_bytes: u64,
    pub attachment_unique_encoded_bytes: u64,
    pub attachment_unique_decoded_bytes: u64,
    pub attachment_unique_encoded_objects: u64,
    pub attachment_unique_decoded_objects: u64,
    pub attachment_encoded_over_64k_bytes: u64,
    pub attachment_candidate_bytes_decoded: u64,
    pub attachment_candidate_bytes_encoded: u64,
    pub attachment_candidate_unique_encoded_bytes: u64,
    pub attachment_candidate_unique_decoded_bytes: u64,
    pub attachment_candidate_unique_encoded_objects: u64,
    pub attachment_candidate_unique_decoded_objects: u64,
    pub attachment_candidate_encoded_over_64k_bytes: u64,
    pub attachment_candidate_encoded_over_64k_objects: u64,
    pub attachment_candidate_encoded_over_64k_unique_bytes: u64,
    pub attachment_candidate_encoded_over_64k_unique_objects: u64,
    encoded_hashes: std::collections::HashMap<String, u64>,
    decoded_hashes: std::collections::HashMap<String, u64>,
}

#[derive(Clone, Debug, Default)]
pub struct SyncProgress {
    pub examined: u64,
    pub total: Option<u64>,
    pub new_messages: u64,
    pub label_changes: u64,
    pub deletions: u64,
    pub network_bytes: u64,
    pub archive_bytes_added: u64,
    pub full_sync: bool,
}

fn progress_snapshot(stats: &SyncStats, total: Option<u64>) -> SyncProgress {
    SyncProgress {
        examined: stats.examined,
        total,
        new_messages: stats.new_messages,
        label_changes: stats.label_changes,
        deletions: stats.deletions,
        network_bytes: stats.network_bytes,
        archive_bytes_added: stats.archive_bytes_added,
        full_sync: stats.full_sync,
    }
}

fn progress_snapshot_with_batch(
    stats: &SyncStats,
    total: Option<u64>,
    staged: &[crate::GmailBatchRecord],
) -> SyncProgress {
    let mut snapshot = progress_snapshot(stats, total);
    snapshot.new_messages += staged.len() as u64;
    snapshot.archive_bytes_added += staged
        .iter()
        .map(crate::GmailBatchRecord::frame_bytes)
        .sum::<u64>();
    snapshot
}

#[cfg(test)]
#[derive(Clone, Debug, Default, PartialEq, Eq)]
struct DuplicationMetrics {
    total_objects: u64,
    unique_objects: u64,
    total_bytes: u64,
    unique_bytes: u64,
}

#[cfg(test)]
#[derive(Default)]
struct DuplicationAccumulator {
    total_objects: u64,
    total_bytes: u64,
    unique_sizes: std::collections::HashMap<String, u64>,
}

#[cfg(test)]
impl DuplicationAccumulator {
    fn add(&mut self, bytes: &[u8]) {
        self.total_objects += 1;
        self.total_bytes += bytes.len() as u64;
        self.unique_sizes
            .entry(blake3::hash(bytes).to_hex().to_string())
            .or_insert(bytes.len() as u64);
    }

    fn metrics(&self) -> DuplicationMetrics {
        DuplicationMetrics {
            total_objects: self.total_objects,
            unique_objects: self.unique_sizes.len() as u64,
            total_bytes: self.total_bytes,
            unique_bytes: self.unique_sizes.values().sum(),
        }
    }
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ListedMessage {
    pub id: String,
    #[serde(default)]
    pub thread_id: String,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RawMessage {
    pub id: String,
    #[serde(default)]
    pub thread_id: String,
    #[serde(default)]
    pub label_ids: Vec<String>,
    #[serde(default)]
    pub history_id: Option<String>,
    #[serde(default)]
    pub internal_date: Option<String>,
    pub raw: String,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MetadataMessage {
    pub id: String,
    #[serde(default)]
    pub thread_id: String,
    #[serde(default)]
    pub label_ids: Vec<String>,
    #[serde(default)]
    pub history_id: Option<String>,
    #[serde(default)]
    pub internal_date: Option<String>,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Profile {
    pub history_id: String,
    #[serde(default)]
    pub email_address: Option<String>,
}

#[derive(Clone, Debug, Default)]
pub struct OfflineMimeReport {
    pub messages: u64,
    pub raw_sizes: Vec<usize>,
    pub multipart_messages: u64,
    pub parts: u64,
    pub leaves: u64,
    pub attachments: u64,
    pub attachment_bytes: u64,
    pub attachment_encoded_bytes: u64,
    pub attachment_unique_encoded_bytes: u64,
    pub attachment_unique_decoded_bytes: u64,
    pub attachment_unique_encoded_objects: u64,
    pub attachment_unique_decoded_objects: u64,
    pub attachment_encoded_over_64k_bytes: u64,
    pub attachment_candidate_bytes_decoded: u64,
    pub attachment_candidate_bytes_encoded: u64,
    pub attachment_candidate_unique_encoded_bytes: u64,
    pub attachment_candidate_unique_decoded_bytes: u64,
    pub attachment_candidate_unique_encoded_objects: u64,
    pub attachment_candidate_unique_decoded_objects: u64,
    pub attachment_candidate_encoded_over_64k_bytes: u64,
    pub attachment_candidate_encoded_over_64k_objects: u64,
    pub attachment_candidate_encoded_over_64k_unique_bytes: u64,
    pub attachment_candidate_encoded_over_64k_unique_objects: u64,
    pub inline: u64,
    pub inline_bytes: u64,
    pub filename_or_name: u64,
    pub filename_or_name_bytes: u64,
    pub content_id: u64,
    pub content_id_bytes: u64,
    pub image_parts: u64,
    pub image_bytes: u64,
    pub pdf_parts: u64,
    pub pdf_bytes: u64,
    pub zip_parts: u64,
    pub zip_bytes: u64,
    pub office_parts: u64,
    pub office_bytes: u64,
    pub other_application_parts: u64,
    pub other_application_bytes: u64,
    pub checksum_verified_frames: u64,
    pub physical_archive_bytes: u64,
    pub segment_files: u64,
}

impl OfflineMimeReport {
    pub fn percentile(&self, fraction: f64) -> usize {
        let index = ((self.raw_sizes.len().saturating_sub(1)) as f64 * fraction).round() as usize;
        self.raw_sizes.get(index).copied().unwrap_or(0)
    }
    pub fn max_raw(&self) -> usize {
        self.raw_sizes.iter().copied().max().unwrap_or(0)
    }
}

pub fn analyze_archived_mime(root: &Path) -> Result<OfflineMimeReport, GmailError> {
    let connection = crate::open_catalogue(&root.join("metadata.sqlite"))
        .map_err(|error| GmailError::Other(error.to_string()))?;
    let mut statement = connection
        .prepare("SELECT doc_id FROM messages ORDER BY doc_id")
        .map_err(|error| GmailError::Other(error.to_string()))?;
    let rows = statement
        .query_map([], |row| row.get::<_, i64>(0))
        .map_err(|error| GmailError::Other(error.to_string()))?;
    let doc_ids: Vec<u64> = rows
        .map(|row| {
            row.map(|doc_id| doc_id as u64)
                .map_err(|error| GmailError::Other(error.to_string()))
        })
        .collect::<Result<_, _>>()?;
    let mut report = OfflineMimeReport::default();
    let mut encoded_hashes = std::collections::HashMap::new();
    let mut decoded_hashes = std::collections::HashMap::new();
    let mut candidate_encoded_hashes = std::collections::HashMap::new();
    let mut candidate_decoded_hashes = std::collections::HashMap::new();
    let mut candidate_over_64k_hashes = std::collections::HashMap::new();
    for doc_id in doc_ids {
        let raw = crate::read_catalogue_raw(&root.join("archive"), &connection, doc_id)
            .map_err(|error| GmailError::Io(error.to_string()))?;
        report.messages += 1;
        report.raw_sizes.push(raw.len());
        report.checksum_verified_frames += 1;
        let parsed = match parse_mail(&raw) {
            Ok(value) => value,
            Err(_) => continue,
        };
        if parsed.ctype.mimetype.starts_with("multipart/") {
            report.multipart_messages += 1;
        }
        for part in parsed.parts() {
            report.parts += 1;
            if !part.subparts.is_empty() {
                continue;
            }
            report.leaves += 1;
            let bytes = part
                .get_body_raw()
                .map(|value| value.len() as u64)
                .unwrap_or(0);
            let disposition = part.get_content_disposition();
            let has_name = disposition.params.contains_key("filename")
                || part.ctype.params.contains_key("name");
            match part.get_content_disposition().disposition {
                DispositionType::Attachment => {
                    report.attachments += 1;
                    report.attachment_bytes += bytes;
                    let encoded = match part.get_body_encoded() {
                        Body::Base64(body) | Body::QuotedPrintable(body) => body.get_raw(),
                        Body::SevenBit(body) | Body::EightBit(body) => body.get_raw(),
                        Body::Binary(body) => body.get_raw(),
                    };
                    report.attachment_encoded_bytes += encoded.len() as u64;
                    if encoded.len() >= 64 * 1024 {
                        report.attachment_encoded_over_64k_bytes += encoded.len() as u64;
                    }
                    encoded_hashes
                        .entry(blake3::hash(encoded).to_hex().to_string())
                        .or_insert(encoded.len() as u64);
                    decoded_hashes
                        .entry(
                            blake3::hash(&part.get_body_raw().unwrap_or_default())
                                .to_hex()
                                .to_string(),
                        )
                        .or_insert(bytes);
                }
                DispositionType::Inline => {
                    report.inline += 1;
                    report.inline_bytes += bytes;
                }
                _ => {}
            }
            if has_name {
                report.filename_or_name += 1;
                report.filename_or_name_bytes += bytes;
            }
            if has_name || matches!(disposition.disposition, DispositionType::Attachment) {
                let encoded = match part.get_body_encoded() {
                    Body::Base64(body) | Body::QuotedPrintable(body) => body.get_raw(),
                    Body::SevenBit(body) | Body::EightBit(body) => body.get_raw(),
                    Body::Binary(body) => body.get_raw(),
                };
                report.attachment_candidate_bytes_decoded += bytes;
                report.attachment_candidate_bytes_encoded += encoded.len() as u64;
                candidate_encoded_hashes
                    .entry(blake3::hash(encoded).to_hex().to_string())
                    .or_insert(encoded.len() as u64);
                candidate_decoded_hashes
                    .entry(
                        blake3::hash(&part.get_body_raw().unwrap_or_default())
                            .to_hex()
                            .to_string(),
                    )
                    .or_insert(bytes);
                if encoded.len() >= 64 * 1024 {
                    report.attachment_candidate_encoded_over_64k_objects += 1;
                    report.attachment_candidate_encoded_over_64k_bytes += encoded.len() as u64;
                    candidate_over_64k_hashes
                        .entry(blake3::hash(encoded).to_hex().to_string())
                        .or_insert(encoded.len() as u64);
                }
            }
            if part.headers.get_first_value("content-id").is_some() {
                report.content_id += 1;
                report.content_id_bytes += bytes;
            }
            let mime = part.ctype.mimetype.as_str();
            if mime.starts_with("image/") {
                report.image_parts += 1;
                report.image_bytes += bytes;
            } else if mime == "application/pdf" {
                report.pdf_parts += 1;
                report.pdf_bytes += bytes;
            } else if mime == "application/zip" || mime == "application/x-zip-compressed" {
                report.zip_parts += 1;
                report.zip_bytes += bytes;
            } else if is_office_mime(mime) {
                report.office_parts += 1;
                report.office_bytes += bytes;
            } else if mime.starts_with("application/") {
                report.other_application_parts += 1;
                report.other_application_bytes += bytes;
            }
        }
    }
    report.attachment_unique_encoded_bytes = encoded_hashes.values().sum();
    report.attachment_unique_decoded_bytes = decoded_hashes.values().sum();
    report.attachment_unique_encoded_objects = encoded_hashes.len() as u64;
    report.attachment_unique_decoded_objects = decoded_hashes.len() as u64;
    report.attachment_candidate_unique_encoded_bytes = candidate_encoded_hashes.values().sum();
    report.attachment_candidate_unique_decoded_bytes = candidate_decoded_hashes.values().sum();
    report.attachment_candidate_unique_encoded_objects = candidate_encoded_hashes.len() as u64;
    report.attachment_candidate_unique_decoded_objects = candidate_decoded_hashes.len() as u64;
    report.attachment_candidate_encoded_over_64k_unique_bytes =
        candidate_over_64k_hashes.values().sum();
    report.attachment_candidate_encoded_over_64k_unique_objects =
        candidate_over_64k_hashes.len() as u64;
    report.raw_sizes.sort_unstable();
    for entry in
        fs::read_dir(root.join("archive")).map_err(|error| GmailError::Io(error.to_string()))?
    {
        let entry = entry.map_err(|error| GmailError::Io(error.to_string()))?;
        if entry
            .file_type()
            .map_err(|error| GmailError::Io(error.to_string()))?
            .is_file()
        {
            report.segment_files += 1;
            report.physical_archive_bytes += entry
                .metadata()
                .map_err(|error| GmailError::Io(error.to_string()))?
                .len();
        }
    }
    Ok(report)
}

fn is_office_mime(mime: &str) -> bool {
    mime.contains("msword")
        || mime.contains("ms-excel")
        || mime.contains("ms-powerpoint")
        || mime.contains("openxmlformats-officedocument")
        || mime.contains("oasis.opendocument")
}

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ListPage {
    #[serde(default)]
    pub messages: Vec<ListedMessage>,
    #[serde(default)]
    pub next_page_token: Option<String>,
}

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HistoryPage {
    #[serde(default)]
    pub history: Vec<HistoryRecord>,
    #[serde(default)]
    pub next_page_token: Option<String>,
    #[serde(default)]
    pub history_id: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct HistoryRecord {
    pub id: String,
    #[serde(default)]
    pub messages_added: Vec<HistoryMessageAdded>,
    #[serde(default)]
    pub messages_deleted: Vec<HistoryMessageDeleted>,
    #[serde(default)]
    pub labels_added: Vec<HistoryLabels>,
    #[serde(default)]
    pub labels_removed: Vec<HistoryLabels>,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HistoryMessageAdded {
    pub message: ListedMessage,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HistoryMessageDeleted {
    pub message: ListedMessage,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HistoryLabels {
    pub message: ListedMessage,
    #[serde(default)]
    pub label_ids: Vec<String>,
}

pub trait GmailTransport {
    fn list(
        &mut self,
        page_token: Option<&str>,
        query: Option<&str>,
    ) -> Result<ListPage, GmailError>;
    fn get_raw(&mut self, id: &str) -> Result<RawMessage, GmailError>;
    fn get_metadata(&mut self, id: &str) -> Result<MetadataMessage, GmailError>;
    fn profile(&mut self) -> Result<Profile, GmailError>;
    fn history(
        &mut self,
        start_history_id: &str,
        page_token: Option<&str>,
    ) -> Result<HistoryPage, GmailError>;
}

#[derive(Clone, Debug, Deserialize)]
struct InstalledCredentials {
    installed: InstalledClient,
}

#[derive(Clone, Debug, Deserialize)]
struct InstalledClient {
    client_id: String,
    client_secret: String,
    #[serde(default)]
    auth_uri: Option<String>,
    #[serde(default)]
    token_uri: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
struct StoredToken {
    refresh_token: String,
    #[serde(default)]
    access_token: Option<String>,
    #[serde(default)]
    expires_at: Option<u64>,
    #[serde(default)]
    scope: Option<String>,
}

pub struct HttpGmail {
    client: Client,
    access_token: String,
}

impl HttpGmail {
    pub fn authenticate(credentials_path: &Path, token_dir: &Path) -> Result<Self, GmailError> {
        let credentials = read_credentials(credentials_path)?;
        fs::create_dir_all(token_dir).map_err(|error| GmailError::Io(error.to_string()))?;
        let token_path = token_dir.join("gmail-token.json");
        let token = if token_path.exists() {
            let bytes = fs::read(&token_path).map_err(|error| GmailError::Io(error.to_string()))?;
            serde_json::from_slice::<StoredToken>(&bytes)
                .map_err(|error| GmailError::Json(error.to_string()))?
        } else {
            let token = interactive_authorization(&credentials)?;
            let encoded = serde_json::to_vec_pretty(&token)
                .map_err(|error| GmailError::Json(error.to_string()))?;
            fs::write(&token_path, encoded).map_err(|error| GmailError::Io(error.to_string()))?;
            token
        };
        if !token
            .scope
            .as_deref()
            .unwrap_or_default()
            .split_whitespace()
            .any(|value| value == GMAIL_READONLY_SCOPE)
        {
            return Err(GmailError::Config(
                "stored OAuth token does not grant gmail.readonly; authorize again".into(),
            ));
        }
        let access_token = if token.expires_at.unwrap_or(0) > now_seconds() + 60 {
            token
                .access_token
                .ok_or_else(|| GmailError::Config("token has no access token".into()))?
        } else {
            let refresh = token.refresh_token.clone();
            refresh_token(&credentials, &refresh, &token_path, token)?
        };
        Ok(Self {
            client: Client::builder()
                .build()
                .map_err(|error| GmailError::Other(error.to_string()))?,
            access_token,
        })
    }

    fn get_json<T: for<'de> Deserialize<'de>>(
        &self,
        url: &str,
        query: &[(&str, &str)],
    ) -> Result<T, GmailError> {
        let response = self
            .client
            .get(url)
            .bearer_auth(&self.access_token)
            .query(query)
            .send()
            .map_err(|error| GmailError::Other(error.to_string()))?;
        let status = response.status();
        if !status.is_success() {
            return Err(if status.as_u16() == 404 && url.contains("/history") {
                GmailError::HistoryExpired
            } else {
                GmailError::Http(status.as_u16())
            });
        }
        response
            .json()
            .map_err(|error| GmailError::Json(error.to_string()))
    }
}

impl GmailTransport for HttpGmail {
    fn list(
        &mut self,
        page_token: Option<&str>,
        query: Option<&str>,
    ) -> Result<ListPage, GmailError> {
        let mut params = vec![("maxResults", "500")];
        if let Some(value) = page_token {
            params.push(("pageToken", value));
        }
        if let Some(value) = query {
            params.push(("q", value));
        }
        self.get_json(
            "https://gmail.googleapis.com/gmail/v1/users/me/messages",
            &params,
        )
    }

    fn get_raw(&mut self, id: &str) -> Result<RawMessage, GmailError> {
        self.get_json(
            &format!("https://gmail.googleapis.com/gmail/v1/users/me/messages/{id}"),
            &[("format", "RAW")],
        )
    }

    fn get_metadata(&mut self, id: &str) -> Result<MetadataMessage, GmailError> {
        self.get_json(
            &format!("https://gmail.googleapis.com/gmail/v1/users/me/messages/{id}"),
            &[("format", "METADATA")],
        )
    }

    fn profile(&mut self) -> Result<Profile, GmailError> {
        self.get_json(
            "https://gmail.googleapis.com/gmail/v1/users/me/profile",
            &[],
        )
    }

    fn history(
        &mut self,
        start_history_id: &str,
        page_token: Option<&str>,
    ) -> Result<HistoryPage, GmailError> {
        let mut params = vec![
            ("startHistoryId", start_history_id),
            ("historyTypes", "messageAdded"),
            ("historyTypes", "messageDeleted"),
            ("historyTypes", "labelAdded"),
            ("historyTypes", "labelRemoved"),
        ];
        if let Some(value) = page_token {
            params.push(("pageToken", value));
        }
        self.get_json(
            "https://gmail.googleapis.com/gmail/v1/users/me/history",
            &params,
        )
    }
}

fn read_credentials(path: &Path) -> Result<InstalledClient, GmailError> {
    if !path.exists() {
        return Err(GmailError::Config(format!(
            "credentials file not found: {}",
            path.display()
        )));
    }
    let bytes = fs::read(path).map_err(|error| GmailError::Io(error.to_string()))?;
    serde_json::from_slice::<InstalledCredentials>(&bytes)
        .map(|value| value.installed)
        .map_err(|error| GmailError::Json(error.to_string()))
}

fn interactive_authorization(credentials: &InstalledClient) -> Result<StoredToken, GmailError> {
    let listener =
        TcpListener::bind("127.0.0.1:0").map_err(|error| GmailError::Io(error.to_string()))?;
    let redirect = format!(
        "http://127.0.0.1:{}/",
        listener
            .local_addr()
            .map_err(|error| GmailError::Io(error.to_string()))?
            .port()
    );
    let state = format!("{}-{}", now_seconds(), std::process::id());
    let mut query = form_urlencoded::Serializer::new(String::new());
    query.append_pair("client_id", &credentials.client_id);
    query.append_pair("redirect_uri", &redirect);
    query.append_pair("response_type", "code");
    query.append_pair("scope", GMAIL_READONLY_SCOPE);
    query.append_pair("access_type", "offline");
    query.append_pair("prompt", "consent");
    query.append_pair("state", &state);
    let url = format!(
        "{}?{}",
        credentials
            .auth_uri
            .as_deref()
            .unwrap_or("https://accounts.google.com/o/oauth2/v2/auth"),
        query.finish()
    );
    eprintln!(
        "Open this URL in a browser to authorize the local read-only Gmail experiment:\n{url}"
    );
    let _ = webbrowser::open(&url);
    listener
        .set_nonblocking(true)
        .map_err(|error| GmailError::Io(error.to_string()))?;
    let deadline = std::time::Instant::now() + Duration::from_secs(300);
    let (mut stream, _) = loop {
        match listener.accept() {
            Ok(value) => break value,
            Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                if std::time::Instant::now() >= deadline {
                    return Err(GmailError::Io(
                        "OAuth callback timed out after 300 seconds".into(),
                    ));
                }
                std::thread::sleep(Duration::from_millis(100));
            }
            Err(error) => return Err(GmailError::Io(error.to_string())),
        }
    };
    stream
        .set_read_timeout(Some(Duration::from_secs(10)))
        .map_err(|error| GmailError::Io(error.to_string()))?;
    let mut request_bytes = Vec::with_capacity(1024);
    let mut buffer = [0u8; 1024];
    loop {
        let count = stream
            .read(&mut buffer)
            .map_err(|error| GmailError::Io(error.to_string()))?;
        if count == 0 {
            break;
        }
        request_bytes.extend_from_slice(&buffer[..count]);
        if request_bytes.windows(4).any(|window| window == b"\r\n\r\n") {
            break;
        }
        if request_bytes.len() > 8192 {
            return Err(GmailError::Other("OAuth callback request too large".into()));
        }
    }
    let request = String::from_utf8_lossy(&request_bytes);
    let target = request
        .lines()
        .next()
        .and_then(|line| line.split_whitespace().nth(1))
        .ok_or_else(|| GmailError::Other("invalid OAuth callback".into()))?;
    let callback = url::Url::parse(&format!("http://localhost{target}"))
        .map_err(|error| GmailError::Other(error.to_string()))?;
    let values: std::collections::HashMap<_, _> = callback.query_pairs().into_owned().collect();
    let _ = stream.write_all(b"HTTP/1.1 200 OK\r\nContent-Type: text/plain\r\n\r\nAuthorization received. You can close this window.\r\n");
    if values.get("state") != Some(&state) {
        return Err(GmailError::Other("OAuth state mismatch".into()));
    }
    let code = values
        .get("code")
        .ok_or_else(|| GmailError::Other("OAuth authorization denied".into()))?;
    let response = Client::new()
        .post(
            credentials
                .token_uri
                .as_deref()
                .unwrap_or("https://oauth2.googleapis.com/token"),
        )
        .form(&[
            ("code", code.as_str()),
            ("client_id", credentials.client_id.as_str()),
            ("client_secret", credentials.client_secret.as_str()),
            ("redirect_uri", redirect.as_str()),
            ("grant_type", "authorization_code"),
        ])
        .send()
        .map_err(|error| GmailError::Other(error.to_string()))?;
    if !response.status().is_success() {
        return Err(GmailError::Http(response.status().as_u16()));
    }
    let token: TokenResponse = response
        .json()
        .map_err(|error| GmailError::Json(error.to_string()))?;
    if token
        .scope
        .as_deref()
        .map(|scope| {
            !scope
                .split_whitespace()
                .any(|value| value == GMAIL_READONLY_SCOPE)
        })
        .unwrap_or(true)
    {
        return Err(GmailError::Config(
            "OAuth token does not grant gmail.readonly".into(),
        ));
    }
    Ok(StoredToken {
        refresh_token: token
            .refresh_token
            .ok_or_else(|| GmailError::Config("OAuth response has no refresh token".into()))?,
        access_token: Some(token.access_token),
        expires_at: Some(now_seconds() + token.expires_in as u64),
        scope: token.scope,
    })
}

#[derive(Debug, Deserialize)]
struct TokenResponse {
    access_token: String,
    expires_in: u32,
    refresh_token: Option<String>,
    scope: Option<String>,
}

fn refresh_token(
    credentials: &InstalledClient,
    refresh: &str,
    path: &Path,
    previous: StoredToken,
) -> Result<String, GmailError> {
    let response = Client::new()
        .post(
            credentials
                .token_uri
                .as_deref()
                .unwrap_or("https://oauth2.googleapis.com/token"),
        )
        .form(&[
            ("client_id", credentials.client_id.as_str()),
            ("client_secret", credentials.client_secret.as_str()),
            ("refresh_token", refresh),
            ("grant_type", "refresh_token"),
        ])
        .send()
        .map_err(|error| GmailError::Other(error.to_string()))?;
    if !response.status().is_success() {
        return Err(GmailError::Http(response.status().as_u16()));
    }
    let token: TokenResponse = response
        .json()
        .map_err(|error| GmailError::Json(error.to_string()))?;
    if let Some(scope) = token.scope.as_deref() {
        if !scope
            .split_whitespace()
            .any(|value| value == GMAIL_READONLY_SCOPE)
        {
            return Err(GmailError::Config(
                "refreshed OAuth token does not grant gmail.readonly".into(),
            ));
        }
    }
    if !previous
        .scope
        .as_deref()
        .unwrap_or_default()
        .split_whitespace()
        .any(|value| value == GMAIL_READONLY_SCOPE)
    {
        return Err(GmailError::Config(
            "stored OAuth token does not grant gmail.readonly; authorize again".into(),
        ));
    }
    let stored = StoredToken {
        refresh_token: previous.refresh_token,
        access_token: Some(token.access_token.clone()),
        expires_at: Some(now_seconds() + token.expires_in as u64),
        scope: previous.scope,
    };
    fs::write(
        path,
        serde_json::to_vec_pretty(&stored).map_err(|error| GmailError::Json(error.to_string()))?,
    )
    .map_err(|error| GmailError::Io(error.to_string()))?;
    Ok(token.access_token)
}

fn now_seconds() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or(Duration::ZERO)
        .as_secs()
}

pub fn decode_raw(raw: &str) -> Result<Vec<u8>, GmailError> {
    URL_SAFE_NO_PAD
        .decode(raw.as_bytes())
        .or_else(|_| URL_SAFE.decode(raw.as_bytes()))
        .map_err(|error| GmailError::Other(format!("invalid Gmail RAW encoding: {error}")))
}

pub fn sync_account<T: GmailTransport>(
    root: &Path,
    source_account: &str,
    transport: &mut T,
    query: Option<&str>,
    max_messages: Option<u64>,
) -> Result<SyncStats, GmailError> {
    sync_account_with_progress(root, source_account, transport, query, max_messages, |_| {})
}

pub fn sync_account_with_progress<T: GmailTransport, P: FnMut(&SyncProgress)>(
    root: &Path,
    source_account: &str,
    transport: &mut T,
    query: Option<&str>,
    max_messages: Option<u64>,
    mut progress: P,
) -> Result<SyncStats, GmailError> {
    let started = std::time::Instant::now();
    fs::create_dir_all(root).map_err(|error| GmailError::Io(error.to_string()))?;
    let metadata_path = root.join("metadata.sqlite");
    let mut connection = crate::create_metadata(&metadata_path)
        .map_err(|error| GmailError::Other(error.to_string()))?;
    let state = crate::gmail_state(&connection, source_account)
        .map_err(|error| GmailError::Other(error.to_string()))?;
    let mut stats = SyncStats::default();
    if query.is_some() || max_messages.is_some() {
        stats.full_sync = true;
        full_sync(
            root,
            &mut connection,
            source_account,
            transport,
            query,
            max_messages,
            &mut stats,
            &mut progress,
        )?;
        stats.duration_ms = started.elapsed().as_millis();
        return Ok(stats);
    }
    if let Some((history_id, complete)) = state {
        if !complete {
            stats.full_sync = true;
            full_sync(
                root,
                &mut connection,
                source_account,
                transport,
                query,
                max_messages,
                &mut stats,
                &mut progress,
            )
        } else {
            stats.full_sync = false;
            match incremental_sync(
                root,
                &mut connection,
                source_account,
                transport,
                &history_id,
                complete,
                &mut stats,
                &mut progress,
            ) {
                Err(GmailError::HistoryExpired) => {
                    stats.full_sync = true;
                    full_sync(
                        root,
                        &mut connection,
                        source_account,
                        transport,
                        query,
                        max_messages,
                        &mut stats,
                        &mut progress,
                    )
                }
                other => other,
            }
        }
    } else {
        stats.full_sync = true;
        full_sync(
            root,
            &mut connection,
            source_account,
            transport,
            query,
            max_messages,
            &mut stats,
            &mut progress,
        )
    }?;
    stats.duration_ms = started.elapsed().as_millis();
    Ok(stats)
}

#[allow(clippy::too_many_arguments)]
fn full_sync<T: GmailTransport>(
    root: &Path,
    connection: &mut crate::CatalogueConnection,
    source: &str,
    transport: &mut T,
    query: Option<&str>,
    max: Option<u64>,
    stats: &mut SyncStats,
    progress: &mut dyn FnMut(&SyncProgress),
) -> Result<(), GmailError> {
    let mut writer = crate::ArchiveWriter::open_for_catalogue(
        &root.join("archive"),
        64 * 1024 * 1024,
        connection,
    )
    .map_err(|error| GmailError::Io(error.to_string()))?;
    let mut page = None;
    let mut listed_messages = Vec::new();
    let reconcile = max.is_none() && query.is_none();
    let complete = reconcile;
    let mut fence = None;
    loop {
        let response = transport.list(page.as_deref(), query)?;
        if fence.is_none() {
            fence = if let Some(first) = response.messages.first() {
                let metadata = transport.get_metadata(&first.id)?;
                Some(
                    metadata
                        .history_id
                        .filter(|value| !value.is_empty())
                        .ok_or_else(|| {
                            GmailError::Other(
                                "first listed Gmail message has no valid historyId".into(),
                            )
                        })?,
                )
            } else {
                None
            };
        }
        let next_page = response.next_page_token.clone();
        for listed in response.messages {
            if max
                .map(|limit| listed_messages.len() as u64 >= limit)
                .unwrap_or(false)
            {
                break;
            }
            listed_messages.push(listed);
        }
        if max
            .map(|limit| listed_messages.len() as u64 >= limit)
            .unwrap_or(false)
        {
            break;
        }
        page = next_page;
        if page.is_none() {
            break;
        }
    }
    let total = Some(listed_messages.len() as u64);
    stats.total = total;
    let mut seen = HashSet::new();
    let mut next_doc_id =
        crate::next_doc_id(connection).map_err(|error| GmailError::Other(error.to_string()))?;
    let mut staged = Vec::new();
    let mut pending_ids = HashSet::new();
    for listed in listed_messages {
        stats.examined += 1;
        seen.insert(listed.id.clone());
        if pending_ids.contains(&listed.id) {
            continue;
        }
        if gmail_identity_is_valid(root, connection, source, &listed.id)? {
            if reconcile {
                let metadata = transport.get_metadata(&listed.id)?;
                crate::repair_gmail_metadata(
                    connection,
                    source,
                    &metadata.id,
                    &metadata.thread_id,
                    &serde_json::to_string(&metadata.label_ids)
                        .map_err(|error| GmailError::Json(error.to_string()))?,
                    metadata
                        .internal_date
                        .as_deref()
                        .and_then(|value| value.parse().ok()),
                    metadata.history_id.as_deref(),
                )
                .map_err(|error| GmailError::Other(error.to_string()))?;
                progress(&progress_snapshot(stats, total));
            }
            continue;
        }
        ensure_gmail_message_id_available(connection, source, &listed.id)?;
        let raw = transport.get_raw(&listed.id)?;
        import_raw(
            &mut writer,
            source,
            &raw,
            stats,
            &mut next_doc_id,
            &mut staged,
            &mut pending_ids,
        )?;
        if batch_full(&staged) {
            flush_batch(
                connection,
                &mut writer,
                &mut staged,
                &mut pending_ids,
                stats,
            )?;
        }
        progress(&progress_snapshot_with_batch(stats, total, &staged));
    }
    flush_batch_if_needed(
        connection,
        &mut writer,
        &mut staged,
        &mut pending_ids,
        stats,
    )?;
    if complete {
        stats.deletions += crate::mark_gmail_missing_from_full_sync(connection, source, &seen)
            .map_err(|error| GmailError::Other(error.to_string()))?;
    }
    if complete {
        if let Some(history) = fence {
            crate::set_gmail_state(connection, source, &history, true)
                .map_err(|error| GmailError::Other(error.to_string()))?;
        }
    }
    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn incremental_sync<T: GmailTransport>(
    root: &Path,
    connection: &mut crate::CatalogueConnection,
    source: &str,
    transport: &mut T,
    start: &str,
    complete: bool,
    stats: &mut SyncStats,
    progress: &mut dyn FnMut(&SyncProgress),
) -> Result<(), GmailError> {
    let mut page = None;
    let mut writer = crate::ArchiveWriter::open_for_catalogue(
        &root.join("archive"),
        64 * 1024 * 1024,
        connection,
    )
    .map_err(|error| GmailError::Io(error.to_string()))?;
    let mut next_doc_id =
        crate::next_doc_id(connection).map_err(|error| GmailError::Other(error.to_string()))?;
    let mut staged = Vec::new();
    let mut pending_ids = HashSet::new();
    let mut terminal_history = None;
    loop {
        let response = transport.history(start, page.as_deref())?;
        let response_history_id = response.history_id.clone();
        let next_page = response.next_page_token.clone();
        for record in response.history {
            for added in record.messages_added {
                stats.examined += 1;
                if pending_ids.contains(&added.message.id) {
                    continue;
                }
                if gmail_identity_is_valid(root, connection, source, &added.message.id)? {
                    continue;
                }
                ensure_gmail_message_id_available(connection, source, &added.message.id)?;
                let raw = transport.get_raw(&added.message.id)?;
                import_raw(
                    &mut writer,
                    source,
                    &raw,
                    stats,
                    &mut next_doc_id,
                    &mut staged,
                    &mut pending_ids,
                )?;
                if batch_full(&staged) {
                    flush_batch(
                        connection,
                        &mut writer,
                        &mut staged,
                        &mut pending_ids,
                        stats,
                    )?;
                }
                progress(&progress_snapshot_with_batch(stats, None, &staged));
            }
            for deleted in record.messages_deleted {
                stats.examined += 1;
                crate::mark_gmail_deleted(connection, source, &deleted.message.id)
                    .map_err(|error| GmailError::Other(error.to_string()))?;
                stats.deletions += 1;
            }
            for label in record.labels_added.into_iter().chain(record.labels_removed) {
                stats.examined += 1;
                flush_batch_if_needed(
                    connection,
                    &mut writer,
                    &mut staged,
                    &mut pending_ids,
                    stats,
                )?;
                if !gmail_identity_is_valid(root, connection, source, &label.message.id)? {
                    return Err(GmailError::Other(format!(
                        "label event references unknown Gmail identity {}",
                        label.message.id
                    )));
                }
                let raw = transport.get_raw(&label.message.id)?;
                crate::update_gmail_labels(
                    connection,
                    source,
                    &raw.id,
                    &serde_json::to_string(&raw.label_ids).unwrap_or_else(|_| "[]".into()),
                )
                .map_err(|error| GmailError::Other(error.to_string()))?;
                stats.label_changes += 1;
                progress(&progress_snapshot(stats, None));
            }
        }
        if page.is_none() {
            terminal_history = response_history_id;
        }
        page = next_page;
        if page.is_none() {
            break;
        }
    }
    flush_batch_if_needed(
        connection,
        &mut writer,
        &mut staged,
        &mut pending_ids,
        stats,
    )?;
    let history = terminal_history
        .filter(|value| !value.is_empty())
        .ok_or_else(|| {
            GmailError::Other("terminal Gmail history response has no historyId".into())
        })?;
    crate::set_gmail_state(connection, source, &history, complete)
        .map_err(|error| GmailError::Other(error.to_string()))?;
    Ok(())
}

fn canonical_gmail_message_id(source: &str, gmail_id: &str) -> String {
    format!("gmail:{source}:{gmail_id}")
}

fn gmail_identity_is_valid(
    root: &Path,
    connection: &Connection,
    source: &str,
    gmail_id: &str,
) -> Result<bool, GmailError> {
    let doc_id = connection
        .query_row(
            "SELECT doc_id FROM gmail_messages WHERE source_account=?1 AND gmail_message_id=?2",
            rusqlite::params![source, gmail_id],
            |row| row.get::<_, i64>(0),
        )
        .optional()
        .map_err(|error| GmailError::Other(error.to_string()))?;
    let Some(doc_id) = doc_id else {
        return Ok(false);
    };
    crate::validate_catalog_record(
        &root.join("archive"),
        connection,
        doc_id,
        &canonical_gmail_message_id(source, gmail_id),
    )
    .map_err(|error| {
        GmailError::Other(format!(
            "known Gmail identity {source}/{gmail_id} is not valid locally: {error}"
        ))
    })?;
    Ok(true)
}

fn ensure_gmail_message_id_available(
    connection: &Connection,
    source: &str,
    gmail_id: &str,
) -> Result<(), GmailError> {
    let canonical = canonical_gmail_message_id(source, gmail_id);
    let exists = connection
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM messages WHERE message_id=?1)",
            [canonical.as_str()],
            |row| row.get::<_, bool>(0),
        )
        .map_err(|error| GmailError::Other(error.to_string()))?;
    if exists {
        return Err(GmailError::Other(format!(
            "historical messages row already uses Gmail identity {source}/{gmail_id} without source metadata"
        )));
    }
    Ok(())
}

fn import_raw(
    writer: &mut crate::ArchiveWriter,
    source: &str,
    raw: &RawMessage,
    stats: &mut SyncStats,
    next_doc_id: &mut i64,
    staged: &mut Vec<crate::GmailBatchRecord>,
    pending_ids: &mut HashSet<String>,
) -> Result<(), GmailError> {
    let bytes = decode_raw(&raw.raw)?;
    analyze_mime(&bytes, stats);
    let doc_id = *next_doc_id;
    *next_doc_id = (*next_doc_id)
        .checked_add(1)
        .ok_or_else(|| GmailError::Other("document ID overflow".into()))?;
    let location = writer
        .append_raw(doc_id as u64, &bytes)
        .map_err(|error| GmailError::Io(error.to_string()))?;
    let labels = serde_json::to_string(&raw.label_ids)
        .map_err(|error| GmailError::Json(error.to_string()))?;
    let date = raw
        .internal_date
        .as_deref()
        .and_then(|value| value.parse::<i64>().ok());
    pending_ids.insert(raw.id.clone());
    staged.push(crate::GmailBatchRecord::new(
        source.into(),
        raw.id.clone(),
        doc_id,
        raw.thread_id.clone(),
        labels,
        date,
        raw.history_id.clone(),
        location,
    ));
    stats.network_bytes += raw.raw.len() as u64;
    Ok(())
}

fn batch_full(batch: &[crate::GmailBatchRecord]) -> bool {
    batch.len() >= IMPORT_BATCH_RECORD_LIMIT
        || batch
            .iter()
            .map(crate::GmailBatchRecord::frame_bytes)
            .sum::<u64>()
            >= IMPORT_BATCH_BYTES_LIMIT
}

fn flush_batch_if_needed(
    connection: &crate::CatalogueConnection,
    writer: &mut crate::ArchiveWriter,
    staged: &mut Vec<crate::GmailBatchRecord>,
    pending_ids: &mut HashSet<String>,
    stats: &mut SyncStats,
) -> Result<(), GmailError> {
    if staged.is_empty() {
        return Ok(());
    }
    flush_batch(connection, writer, staged, pending_ids, stats)
}

fn flush_batch(
    connection: &crate::CatalogueConnection,
    writer: &mut crate::ArchiveWriter,
    staged: &mut Vec<crate::GmailBatchRecord>,
    pending_ids: &mut HashSet<String>,
    stats: &mut SyncStats,
) -> Result<(), GmailError> {
    let durable = writer
        .durable_barrier()
        .map_err(|error| GmailError::Io(error.to_string()))?;
    let records = staged.len() as u64;
    let frame_bytes = staged
        .iter()
        .map(crate::GmailBatchRecord::frame_bytes)
        .sum::<u64>();
    crate::publish_gmail_batch(connection, staged, &durable)
        .map_err(|error| GmailError::Other(error.to_string()))?;
    stats.new_messages += records;
    stats.archive_bytes_added += frame_bytes;
    staged.clear();
    pending_ids.clear();
    Ok(())
}

fn analyze_mime(bytes: &[u8], stats: &mut SyncStats) {
    let parsed = match parse_mail(bytes) {
        Ok(value) => value,
        Err(_) => {
            stats.mime_parse_failures += 1;
            return;
        }
    };
    stats.mime_messages += 1;
    for part in parsed.parts() {
        let disposition = part.get_content_disposition();
        let is_attachment = matches!(disposition.disposition, DispositionType::Attachment)
            || disposition.params.contains_key("filename")
            || part.ctype.params.contains_key("name");
        if !is_attachment || !part.subparts.is_empty() {
            continue;
        }
        let encoded = match part.get_body_encoded() {
            Body::Base64(body) | Body::QuotedPrintable(body) => body.get_raw(),
            Body::SevenBit(body) => body.get_raw(),
            Body::EightBit(body) => body.get_raw(),
            Body::Binary(body) => body.get_raw(),
        };
        let decoded = match part.get_body_raw() {
            Ok(value) => value,
            Err(_) => continue,
        };
        stats.attachments += 1;
        stats.attachment_encoded_bytes += encoded.len() as u64;
        stats.attachment_decoded_bytes += decoded.len() as u64;
        if encoded.len() >= 64 * 1024 {
            stats.attachment_encoded_over_64k_bytes += encoded.len() as u64;
        }
        stats
            .encoded_hashes
            .entry(blake3::hash(encoded).to_hex().to_string())
            .or_insert(encoded.len() as u64);
        stats
            .decoded_hashes
            .entry(blake3::hash(&decoded).to_hex().to_string())
            .or_insert(decoded.len() as u64);
        stats.attachment_unique_encoded_bytes = stats.encoded_hashes.values().sum();
        stats.attachment_unique_decoded_bytes = stats.decoded_hashes.values().sum();
        stats.attachment_unique_encoded_objects = stats.encoded_hashes.len() as u64;
        stats.attachment_unique_decoded_objects = stats.decoded_hashes.len() as u64;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Fixture {
        pages: Vec<ListPage>,
        messages: std::collections::HashMap<String, RawMessage>,
        history: Vec<HistoryPage>,
        expire_history: bool,
        raw_calls: u64,
        metadata_calls: u64,
    }
    impl GmailTransport for Fixture {
        fn list(
            &mut self,
            page: Option<&str>,
            _query: Option<&str>,
        ) -> Result<ListPage, GmailError> {
            Ok(self
                .pages
                .get(page.and_then(|v| v.parse().ok()).unwrap_or(0))
                .cloned()
                .unwrap_or_default())
        }
        fn get_raw(&mut self, id: &str) -> Result<RawMessage, GmailError> {
            self.raw_calls += 1;
            self.messages
                .get(id)
                .cloned()
                .ok_or_else(|| GmailError::Other("fixture message missing".into()))
        }
        fn get_metadata(&mut self, id: &str) -> Result<MetadataMessage, GmailError> {
            self.metadata_calls += 1;
            let raw = self
                .messages
                .get(id)
                .cloned()
                .ok_or_else(|| GmailError::Other("fixture message missing".into()))?;
            Ok(MetadataMessage {
                id: raw.id,
                thread_id: raw.thread_id,
                label_ids: vec!["UPDATED".into()],
                history_id: raw.history_id,
                internal_date: raw.internal_date,
            })
        }
        fn profile(&mut self) -> Result<Profile, GmailError> {
            Ok(Profile {
                history_id: "13".into(),
                email_address: None,
            })
        }
        fn history(&mut self, _start: &str, page: Option<&str>) -> Result<HistoryPage, GmailError> {
            if self.expire_history {
                return Err(GmailError::HistoryExpired);
            }
            Ok(self
                .history
                .get(page.and_then(|v| v.parse().ok()).unwrap_or(0))
                .cloned()
                .unwrap_or_default())
        }
    }

    #[test]
    fn raw_base64url_roundtrip_is_binary_safe() {
        assert_eq!(decode_raw("SGVsbG8").unwrap(), b"Hello");
    }

    #[test]
    fn duplication_math_uses_one_size_per_distinct_hash() {
        let a = b"A";
        let b = b"BB";
        let c = b"CCC";
        let payloads: [&[u8]; 5] = [a, a, b, c, c];
        let mut accumulator = DuplicationAccumulator::default();
        for payload in payloads {
            accumulator.add(payload);
        }
        assert_eq!(
            accumulator.metrics(),
            DuplicationMetrics {
                total_objects: 5,
                unique_objects: 3,
                total_bytes: 10,
                unique_bytes: 6,
            }
        );
        assert_eq!(10 - 6, 4);
        assert!((1.0_f64 - 3.0 / 5.0 - 0.4).abs() < f64::EPSILON);
        assert!((1.0_f64 - 6.0 / 10.0 - 0.4).abs() < f64::EPSILON);
    }

    #[test]
    fn gmail_camel_case_metadata_is_decoded() {
        let raw: RawMessage = serde_json::from_str(
            r#"{"id":"m1","threadId":"t1","labelIds":["INBOX"],"historyId":"h1","internalDate":"1700000000000","raw":"SGk"}"#,
        )
        .unwrap();
        assert_eq!(raw.thread_id, "t1");
        assert_eq!(raw.label_ids, ["INBOX"]);
        assert_eq!(raw.history_id.as_deref(), Some("h1"));
    }

    #[test]
    fn reconciliation_uses_metadata_for_known_and_raw_for_new_messages() {
        let root =
            std::env::temp_dir().join(format!("gmail-reconcile-fixture-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let raw = |id: &str| RawMessage {
            id: id.into(),
            thread_id: format!("thread-{id}"),
            label_ids: vec!["INBOX".into()],
            history_id: Some("20".into()),
            internal_date: Some("1700000000000".into()),
            raw: "RnJvbTogZml4dHVyZUBleGFtcGxlLnRlc3QNCg0KSGVsbG8=".into(),
        };
        let messages = (0..1010)
            .map(|id| (format!("m{id}"), raw(&format!("m{id}"))))
            .collect();
        let listed = (0..1010)
            .map(|id| ListedMessage {
                id: format!("m{id}"),
                thread_id: format!("thread-m{id}"),
            })
            .collect();
        let mut transport = Fixture {
            pages: vec![ListPage {
                messages: listed,
                next_page_token: None,
            }],
            messages,
            history: vec![HistoryPage::default()],
            expire_history: false,
            raw_calls: 0,
            metadata_calls: 0,
        };
        let first =
            sync_account(&root, "fixture-account", &mut transport, None, Some(1000)).unwrap();
        assert_eq!(first.new_messages, 1000);
        transport.raw_calls = 0;
        transport.metadata_calls = 0;
        let second = sync_account(&root, "fixture-account", &mut transport, None, None).unwrap();
        assert_eq!(second.new_messages, 10);
        assert_eq!(transport.raw_calls, 10);
        assert_eq!(transport.metadata_calls, 1001);
        let connection = crate::create_metadata(&root.join("metadata.sqlite")).unwrap();
        assert_eq!(
            connection
                .query_row(
                    "SELECT label_ids FROM gmail_messages WHERE gmail_message_id='m0'",
                    [],
                    |row| row.get::<_, String>(0)
                )
                .unwrap(),
            "[\"UPDATED\"]"
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn sync_is_paginated_and_idempotent() {
        let root = std::env::temp_dir().join(format!("gmail-sync-fixture-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let raw = |id: &str, history: &str| RawMessage {
            id: id.into(),
            thread_id: format!("thread-{id}"),
            label_ids: vec!["INBOX".into()],
            history_id: Some(history.into()),
            internal_date: Some("1700000000000".into()),
            raw: "RnJvbTogZml4dHVyZUBleGFtcGxlLnRlc3QNCg0KSGVsbG8=".into(),
        };
        let mut transport = Fixture {
            pages: vec![
                ListPage {
                    messages: vec![ListedMessage {
                        id: "a".into(),
                        thread_id: "ta".into(),
                    }],
                    next_page_token: Some("1".into()),
                },
                ListPage {
                    messages: vec![ListedMessage {
                        id: "b".into(),
                        thread_id: "tb".into(),
                    }],
                    next_page_token: None,
                },
            ],
            messages: [("a".into(), raw("a", "10")), ("b".into(), raw("b", "11"))]
                .into_iter()
                .collect(),
            history: vec![HistoryPage {
                history_id: Some("12".into()),
                ..Default::default()
            }],
            expire_history: false,
            raw_calls: 0,
            metadata_calls: 0,
        };
        let first = sync_account(&root, "fixture-account", &mut transport, None, None).unwrap();
        assert_eq!(first.new_messages, 2);
        let connection = crate::create_metadata(&root.join("metadata.sqlite")).unwrap();
        assert_eq!(
            crate::gmail_state(&connection, "fixture-account").unwrap(),
            Some(("10".into(), true))
        );
        drop(connection);
        let second = sync_account(&root, "fixture-account", &mut transport, None, None).unwrap();
        assert_eq!(second.new_messages, 0);
        assert_eq!(second.examined, 0);
        transport.expire_history = true;
        let fallback = sync_account(&root, "fixture-account", &mut transport, None, None).unwrap();
        assert!(fallback.full_sync);
        assert_eq!(fallback.new_messages, 0);
        let connection = crate::create_metadata(&root.join("metadata.sqlite")).unwrap();
        assert_eq!(
            connection
                .query_row("SELECT COUNT(*) FROM gmail_messages", [], |row| row
                    .get::<_, i64>(0))
                .unwrap(),
            2
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn bounded_gmail_sync_preserves_a_complete_state() {
        let root = std::env::temp_dir().join(format!("gmail-bounded-state-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let connection = crate::create_metadata(&root.join("metadata.sqlite")).unwrap();
        crate::set_gmail_state(&connection, "fixture-account", "7", true).unwrap();
        drop(connection);
        let message = RawMessage {
            id: "bounded".into(),
            thread_id: "thread-bounded".into(),
            label_ids: vec!["INBOX".into()],
            history_id: Some("8".into()),
            internal_date: None,
            raw: "RnJvbTogYm91bmRlZEBleGFtcGxlLnRlc3QNCg0KYm91bmRlZA==".into(),
        };
        let mut transport = Fixture {
            pages: vec![ListPage {
                messages: vec![ListedMessage {
                    id: message.id.clone(),
                    thread_id: message.thread_id.clone(),
                }],
                next_page_token: None,
            }],
            messages: [(message.id.clone(), message)].into_iter().collect(),
            history: vec![HistoryPage::default()],
            expire_history: false,
            raw_calls: 0,
            metadata_calls: 0,
        };
        sync_account(
            &root,
            "fixture-account",
            &mut transport,
            Some("from:bounded"),
            None,
        )
        .unwrap();
        let connection = crate::create_metadata(&root.join("metadata.sqlite")).unwrap();
        assert_eq!(
            crate::gmail_state(&connection, "fixture-account").unwrap(),
            Some(("7".into(), true))
        );
        drop(connection);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn empty_full_sync_does_not_publish_profile_fence() {
        let root = std::env::temp_dir().join(format!("gmail-empty-fence-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let message = RawMessage {
            id: "after-empty".into(),
            thread_id: "thread-after-empty".into(),
            label_ids: vec!["INBOX".into()],
            history_id: Some("21".into()),
            internal_date: None,
            raw: "RnJvbTogYWZ0ZXItZW1wdHlAZXhhbXBsZS50ZXN0DQoNCmFmdGVyLWVtcHR5".into(),
        };
        let mut transport = Fixture {
            pages: vec![ListPage::default()],
            messages: std::collections::HashMap::new(),
            history: vec![HistoryPage::default()],
            expire_history: false,
            raw_calls: 0,
            metadata_calls: 0,
        };
        sync_account(&root, "fixture-account", &mut transport, None, None).unwrap();
        let connection = crate::create_metadata(&root.join("metadata.sqlite")).unwrap();
        assert_eq!(
            crate::gmail_state(&connection, "fixture-account").unwrap(),
            None
        );
        drop(connection);
        transport.pages = vec![ListPage {
            messages: vec![ListedMessage {
                id: message.id.clone(),
                thread_id: message.thread_id.clone(),
            }],
            next_page_token: None,
        }];
        transport.messages.insert(message.id.clone(), message);
        let stats = sync_account(&root, "fixture-account", &mut transport, None, None).unwrap();
        assert_eq!(stats.new_messages, 1);
        let connection = crate::create_metadata(&root.join("metadata.sqlite")).unwrap();
        assert_eq!(
            crate::gmail_state(&connection, "fixture-account").unwrap(),
            Some(("21".into(), true))
        );
        drop(connection);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn incremental_gmail_requires_terminal_history_id() {
        let root =
            std::env::temp_dir().join(format!("gmail-terminal-history-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let connection = crate::create_metadata(&root.join("metadata.sqlite")).unwrap();
        crate::set_gmail_state(&connection, "fixture-account", "7", true).unwrap();
        drop(connection);
        let mut transport = Fixture {
            pages: vec![ListPage::default()],
            messages: std::collections::HashMap::new(),
            history: vec![HistoryPage::default()],
            expire_history: false,
            raw_calls: 0,
            metadata_calls: 0,
        };
        assert!(sync_account(&root, "fixture-account", &mut transport, None, None).is_err());
        let connection = crate::create_metadata(&root.join("metadata.sqlite")).unwrap();
        assert_eq!(
            crate::gmail_state(&connection, "fixture-account").unwrap(),
            Some(("7".into(), true))
        );
        drop(connection);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn known_gmail_identity_with_invalid_raw_fails_closed() {
        let root = std::env::temp_dir().join(format!("gmail-known-invalid-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let connection = crate::create_metadata(&root.join("metadata.sqlite")).unwrap();
        let mut writer = crate::ArchiveWriter::open(&root.join("archive"), 4096).unwrap();
        let raw = b"From: known@example.test\r\n\r\nknown\r\n";
        writer.append_raw(0, raw).unwrap();
        let durable = writer.durable_barrier().unwrap();
        crate::insert_gmail_metadata(
            &connection,
            "fixture-account",
            "known",
            0,
            "thread-known",
            "[]",
            None,
            None,
            &durable.entries()[0],
        )
        .unwrap();
        crate::set_gmail_state(&connection, "fixture-account", "7", true).unwrap();
        drop(writer);
        drop(connection);
        let path = root.join("archive/segment-000000.arc");
        let mut bytes = std::fs::read(&path).unwrap();
        bytes[0] ^= 1;
        std::fs::write(&path, bytes).unwrap();
        let message = RawMessage {
            id: "known".into(),
            thread_id: "thread-known".into(),
            label_ids: vec![],
            history_id: Some("8".into()),
            internal_date: None,
            raw: URL_SAFE_NO_PAD.encode(raw),
        };
        let mut transport = Fixture {
            pages: vec![ListPage::default()],
            messages: [(message.id.clone(), message)].into_iter().collect(),
            history: vec![HistoryPage {
                history: vec![HistoryRecord {
                    id: "8".into(),
                    messages_added: vec![HistoryMessageAdded {
                        message: ListedMessage {
                            id: "known".into(),
                            thread_id: "thread-known".into(),
                        },
                    }],
                    ..Default::default()
                }],
                history_id: Some("8".into()),
                ..Default::default()
            }],
            expire_history: false,
            raw_calls: 0,
            metadata_calls: 0,
        };
        assert!(sync_account(&root, "fixture-account", &mut transport, None, None).is_err());
        let connection = crate::create_metadata(&root.join("metadata.sqlite")).unwrap();
        assert_eq!(
            crate::gmail_state(&connection, "fixture-account").unwrap(),
            Some(("7".into(), true))
        );
        drop(connection);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn duplicate_gmail_id_before_flush_creates_one_raw_and_identity() {
        let root =
            std::env::temp_dir().join(format!("gmail-duplicate-fixture-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let message = RawMessage {
            id: "duplicate".into(),
            thread_id: "thread-duplicate".into(),
            label_ids: vec!["INBOX".into()],
            history_id: Some("10".into()),
            internal_date: Some("1700000000000".into()),
            raw: "RnJvbTogZml4dHVyZUBleGFtcGxlLnRlc3QNCg0KSGVsbG8=".into(),
        };
        let mut transport = Fixture {
            pages: vec![ListPage {
                messages: vec![
                    ListedMessage {
                        id: message.id.clone(),
                        thread_id: message.thread_id.clone(),
                    },
                    ListedMessage {
                        id: message.id.clone(),
                        thread_id: message.thread_id.clone(),
                    },
                ],
                next_page_token: None,
            }],
            messages: [(message.id.clone(), message)].into_iter().collect(),
            history: vec![HistoryPage::default()],
            expire_history: false,
            raw_calls: 0,
            metadata_calls: 0,
        };
        let stats = sync_account(&root, "fixture-account", &mut transport, None, None).unwrap();
        assert_eq!(stats.new_messages, 1);
        assert_eq!(transport.raw_calls, 1);
        let connection = crate::create_metadata(&root.join("metadata.sqlite")).unwrap();
        assert_eq!(
            connection
                .query_row("SELECT COUNT(*) FROM messages", [], |row| row
                    .get::<_, i64>(0))
                .unwrap(),
            1
        );
        assert_eq!(
            connection
                .query_row("SELECT COUNT(*) FROM gmail_messages", [], |row| row
                    .get::<_, i64>(0))
                .unwrap(),
            1
        );
        drop(connection);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn sync_progress_reports_new_messages_without_exposing_content() {
        let root =
            std::env::temp_dir().join(format!("gmail-progress-fixture-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let message = RawMessage {
            id: "progress-1".into(),
            thread_id: "thread-progress".into(),
            label_ids: vec!["INBOX".into()],
            history_id: Some("2".into()),
            internal_date: Some("1700000000000".into()),
            raw: "RnJvbTogZml4dHVyZUBleGFtcGxlLnRlc3QNCg0KSGVsbG8=".into(),
        };
        let mut transport = Fixture {
            pages: vec![ListPage {
                messages: vec![ListedMessage {
                    id: message.id.clone(),
                    thread_id: message.thread_id.clone(),
                }],
                next_page_token: None,
            }],
            messages: [(message.id.clone(), message)].into_iter().collect(),
            history: vec![HistoryPage::default()],
            expire_history: false,
            raw_calls: 0,
            metadata_calls: 0,
        };
        let mut progress = Vec::new();
        let stats = sync_account_with_progress(
            &root,
            "fixture-account",
            &mut transport,
            None,
            None,
            |snapshot| progress.push(snapshot.clone()),
        )
        .unwrap();
        assert_eq!(stats.new_messages, 1);
        assert_eq!(stats.total, Some(1));
        assert_eq!(progress.last().map(|value| value.examined), Some(1));
        assert_eq!(progress.last().and_then(|value| value.total), Some(1));
        assert_eq!(progress.last().map(|value| value.new_messages), Some(1));
        assert!(progress.iter().all(|value| value.archive_bytes_added > 0));
        let _ = std::fs::remove_dir_all(&root);
    }
}
