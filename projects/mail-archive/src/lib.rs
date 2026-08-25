use flate2::{write::GzEncoder, Compression};
use mailparse::{parse_mail, MailHeaderMap, ParsedMail};
use rusqlite::{params, Connection, OpenFlags, OptionalExtension};
use std::cmp::Ordering;
use std::collections::{BTreeMap, HashMap, HashSet};
use std::fs::{self, File, OpenOptions};
use std::io::{self, Read, Seek, SeekFrom, Write};
use std::ops::Bound;
use std::path::{Component, Path, PathBuf};
use std::time::{Duration, Instant};
use tantivy::collector::TopDocs;
use tantivy::indexer::{IndexWriterOptions, NoMergePolicy};
use tantivy::query::{AllQuery, BooleanQuery, Query, QueryParser, RangeQuery, TermQuery};
use tantivy::schema::{
    Field, IndexRecordOption, Schema, TextFieldIndexing, TextOptions, Value, FAST, INDEXED, STORED,
};
use tantivy::{doc, Index, IndexReader, IndexWriter, Order, TantivyDocument, Term};

pub mod app_config;
mod attachment_text;
pub mod delivery_report;
pub mod gmail;
pub mod html_preview;
pub mod html_remote_evidence;
pub mod i18n;
pub mod imap;

pub use attachment_text::{
    discover_providers, providers_for_mime, selected_provider, AttachmentTextStats, BackendKind,
    ExtractionProvider, ProviderAvailability, ProviderId, ProviderSelection,
};
pub use delivery_report::{
    analyze_delivery_report, DeliveryReportAnalysis, DeliveryReportKind, DsnMessageFields,
    DsnRecipient, DsnReport, MdnDisposition, MdnReport,
};

pub const DEFAULT_SEED: u64 = 0x4d_41_49_4c_41_52_43;
const FRAME_MAGIC: &[u8; 8] = b"MAARC001";
const FRAME_HEADER_BYTES: u64 = 32;

#[derive(Clone, Debug)]
pub struct Attachment {
    pub filename: String,
    pub mime: String,
    pub bytes: usize,
    pub hash: String,
}

#[derive(Clone, Debug)]
pub struct Message {
    pub id: u64,
    pub message_id: String,
    pub timestamp: i64,
    pub sender: String,
    pub recipients: Vec<String>,
    pub subject: String,
    pub text_body: String,
    pub html_body: Option<String>,
    pub account: String,
    pub folder: String,
    pub thread: String,
    pub attachments: Vec<Attachment>,
    pub raw: Vec<u8>,
}

#[derive(Clone, Copy, Debug)]
pub struct CorpusConfig {
    pub messages: u64,
    pub seed: u64,
    pub profile: CorpusProfile,
    pub attachment_rate: u32,
    pub duplicate_rate: u32,
    pub max_attachment_bytes: usize,
    pub measure_compression: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CorpusProfile {
    Light,
    Personal,
    Heavy,
}

impl CorpusProfile {
    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "light" => Some(Self::Light),
            "personal" => Some(Self::Personal),
            "heavy" => Some(Self::Heavy),
            _ => None,
        }
    }

    pub const fn name(self) -> &'static str {
        match self {
            Self::Light => "light",
            Self::Personal => "personal",
            Self::Heavy => "heavy",
        }
    }

    pub const fn defaults(self) -> (u32, u32, usize) {
        match self {
            Self::Light => (3, 20, 64 * 1024),
            Self::Personal => (30, 55, 1024 * 1024),
            Self::Heavy => (65, 35, 8 * 1024 * 1024),
        }
    }
}

#[derive(Clone, Debug)]
pub struct DatasetStats {
    pub messages: u64,
    pub bytes: u64,
    pub min_bytes: usize,
    pub p90_bytes: usize,
    pub p99_bytes: usize,
    pub max_bytes: usize,
    pub median_bytes: usize,
    pub mean_bytes: u64,
    pub mime_text_bytes: u64,
    pub compressed_bytes: u64,
    pub zstd_bytes: u64,
    pub text_compressed_bytes: u64,
    pub attachment_compressed_bytes: u64,
    pub text_zstd_bytes: u64,
    pub attachment_zstd_bytes: u64,
    pub attachments: u64,
    pub unique_attachment_hashes: usize,
    pub attachment_bytes: u64,
    pub unique_attachment_bytes: u64,
    pub duplicate_attachment_objects: u64,
    pub duplicate_attachment_bytes: u64,
    pub duplicate_size_p50: usize,
    pub duplicate_size_p90: usize,
    pub duplicate_size_max: usize,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CasVariant {
    Inline,
    Exact,
    Decoded,
    Hybrid { threshold: usize },
}

impl CasVariant {
    pub const fn name(self) -> &'static str {
        match self {
            Self::Inline => "inline",
            Self::Exact => "cas-exact",
            Self::Decoded => "cas-decoded",
            Self::Hybrid { .. } => "hybrid",
        }
    }
}

#[derive(Clone, Debug, Default)]
pub struct CasStats {
    pub variant: String,
    pub messages: u64,
    pub input_bytes: u64,
    pub physical_bytes: u64,
    pub message_store_bytes: u64,
    pub blob_bytes: u64,
    pub manifest_bytes: u64,
    pub blobs: u64,
    pub unique_blob_bytes: u64,
    pub externalized_objects: u64,
    pub hashed_bytes: u64,
    pub hash_us: u128,
    pub import_us: u128,
    pub reconstruction_us: u128,
    pub random_access_us: u128,
    pub max_blob_bytes: usize,
}

#[derive(Clone, Debug)]
enum CasPiece {
    Inline(Vec<u8>),
    Blob(String),
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ArchiveLocation {
    pub segment: String,
    pub offset: u64,
    pub frame_bytes: u64,
}

/// Result of checking one catalogue record against its RAW frame.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum RecordInventoryStatus {
    /// The catalogue coordinates, frame identity, length and format checksum
    /// all agree, and the payload was read successfully. The checksum is the
    /// archive format's validation field, not a cryptographic integrity claim.
    AvailableValidated,
    /// The referenced segment or the referenced bytes are no longer present
    /// on disk. This status does not assign blame to either source.
    PhysicallyMissing,
    /// The available bytes do not validate as the catalogue's frame, or the
    /// catalogue coordinate itself is invalid. The evidence is insufficient
    /// to attribute the inconsistency to RAW or to SQLite.
    Inconsistent { reason: String },
}

/// Read-only per-record result returned by [`inventory_records`].
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RecordInventory {
    pub doc_id: i64,
    pub location: Option<ArchiveLocation>,
    pub status: RecordInventoryStatus,
}

#[derive(Clone, Debug)]
pub struct SearchHit {
    pub doc_id: u64,
    pub score: f32,
}

#[derive(Clone, Debug)]
pub struct GmailSearchResult {
    pub doc_id: u64,
    pub score: f32,
    pub timestamp: i64,
    pub source_account: String,
    pub archive_message_id: String,
    pub sender: String,
    pub recipients: String,
    pub subject: String,
    pub snippet: String,
    pub attachment_count: u64,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum AttachmentFilter {
    #[default]
    All,
    With,
    Without,
}

/// Structured search contract shared by the CLI, controller and future UIs.
/// Date bounds are Unix milliseconds; `date_to` is exclusive.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SearchRequest {
    pub text: String,
    pub from: Option<String>,
    pub to: Option<String>,
    pub date_from: Option<i64>,
    pub date_to: Option<i64>,
    pub attachment: AttachmentFilter,
    pub attachment_mime: Option<String>,
    pub labels: Vec<String>,
    pub limit: usize,
}

impl Default for SearchRequest {
    fn default() -> Self {
        Self {
            text: String::new(),
            from: None,
            to: None,
            date_from: None,
            date_to: None,
            attachment: AttachmentFilter::All,
            attachment_mime: None,
            labels: Vec::new(),
            limit: 50,
        }
    }
}

impl SearchRequest {
    pub fn has_filters(&self) -> bool {
        self.from.is_some()
            || self.to.is_some()
            || self.date_from.is_some()
            || self.date_to.is_some()
            || self.attachment != AttachmentFilter::All
            || self.attachment_mime.is_some()
            || !self.labels.is_empty()
    }
}

#[derive(Clone, Debug, Default)]
pub struct GmailIndexStats {
    pub examined: u64,
    pub indexed: u64,
    pub skipped: u64,
    pub removed: u64,
    pub parse_failures: u64,
    pub read_us: u128,
    pub parse_us: u128,
    pub index_us: u128,
    pub open_us: u128,
    pub first_query_us: u128,
    pub index_bytes: u64,
    pub segments_before_commit: u64,
    pub segments_after_commit: u64,
    pub segments_after_index: u64,
    pub attachment_encountered: u64,
    pub attachment_supported: u64,
    pub attachment_extracted: u64,
    pub attachment_unsupported: u64,
    pub attachment_extraction_failures: u64,
    pub attachment_decoded_bytes: u64,
    pub attachment_extracted_bytes: u64,
    pub attachment_extracted_chars: u64,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct GmailIndexWriterConfig {
    pub memory_budget_bytes: usize,
    /// `None` preserves Tantivy's hardware-dependent `Index::writer` choice.
    pub worker_threads: Option<usize>,
    pub merge_threads: usize,
    pub no_merge_policy: bool,
}

impl Default for GmailIndexWriterConfig {
    fn default() -> Self {
        Self {
            memory_budget_bytes: 50_000_000,
            worker_threads: None,
            merge_threads: 4,
            no_merge_policy: false,
        }
    }
}

#[derive(Clone, Debug)]
pub struct GmailWorkloadStats {
    pub name: String,
    pub queries: usize,
    pub latency: LatencyStats,
    pub zero_results: usize,
}

#[derive(Clone, Debug)]
pub struct GmailParsedMessage {
    pub subject: String,
    pub sender: String,
    pub recipients: String,
    pub body: String,
    pub labels: Vec<String>,
    pub attachment_types: Vec<String>,
    pub attachment_count: u64,
    pub attachment_text: String,
    pub attachment_text_stats: attachment_text::AttachmentTextStats,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AttachmentInfo {
    pub id: u32,
    pub filename: Option<String>,
    pub mime: String,
    pub decoded_bytes: u64,
    pub content_id: Option<String>,
    pub inline: bool,
}

#[derive(Clone, Debug)]
pub(crate) struct AttachmentPayload {
    pub info: AttachmentInfo,
    pub bytes: Vec<u8>,
    pub decoded_text: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MimeResourceInfo {
    pub id: u32,
    pub mime: String,
    pub content_id: String,
    pub decoded_bytes: u64,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HtmlResource {
    pub mime: String,
    pub content_id: String,
    pub bytes: Vec<u8>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HtmlDocument {
    pub html: String,
    pub resources: Vec<HtmlResource>,
}

fn mime_filename(part: &ParsedMail<'_>) -> Option<String> {
    part.get_content_disposition()
        .params
        .get("filename")
        .cloned()
        .or_else(|| part.ctype.params.get("name").cloned())
        .filter(|value| !value.trim().is_empty())
}

fn mime_content_id(part: &ParsedMail<'_>) -> Option<String> {
    part.headers
        .get_first_value("Content-ID")
        .map(|value| {
            value
                .trim()
                .trim_start_matches('<')
                .trim_end_matches('>')
                .to_string()
        })
        .filter(|value| !value.is_empty())
}

fn mime_part_bytes(part: &ParsedMail<'_>) -> io::Result<Vec<u8>> {
    part.get_body_raw()
        .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error.to_string()))
}

fn walk_mime_parts(
    part: &ParsedMail<'_>,
    next_id: &mut u32,
    attachments: &mut Vec<AttachmentInfo>,
    resources: &mut Vec<MimeResourceInfo>,
    mut payloads: Option<&mut Vec<AttachmentPayload>>,
) -> io::Result<()> {
    if !part.subparts.is_empty() {
        for child in &part.subparts {
            walk_mime_parts(
                child,
                next_id,
                attachments,
                resources,
                payloads.as_deref_mut(),
            )?;
        }
        return Ok(());
    }
    let disposition = part.get_content_disposition();
    let filename = mime_filename(part);
    let content_id = mime_content_id(part);
    let inline = matches!(disposition.disposition, mailparse::DispositionType::Inline);
    let bytes = mime_part_bytes(part)?;
    let id = *next_id;
    *next_id += 1;
    if let Some(content_id) = content_id.clone() {
        resources.push(MimeResourceInfo {
            id,
            mime: part.ctype.mimetype.to_ascii_lowercase(),
            content_id,
            decoded_bytes: bytes.len() as u64,
        });
    }
    // Inline CID resources are retained in the resource map but are not shown
    // as ordinary downloadable attachments. A strict attachment remains
    // downloadable even if a sender also supplied a Content-ID.
    let downloadable = matches!(
        disposition.disposition,
        mailparse::DispositionType::Attachment
    ) || (filename.is_some() && !(inline && content_id.is_some()));
    if downloadable {
        let info = AttachmentInfo {
            id,
            filename,
            mime: part.ctype.mimetype.to_ascii_lowercase(),
            decoded_bytes: bytes.len() as u64,
            content_id,
            inline,
        };
        attachments.push(info.clone());
        if let Some(payloads) = payloads {
            let decoded_text = if info.mime.starts_with("text/") && bytes.len() <= 64 * 1024 * 1024
            {
                part.get_body().ok()
            } else {
                None
            };
            payloads.push(AttachmentPayload {
                info,
                bytes,
                decoded_text,
            });
        }
    }
    Ok(())
}

pub fn list_attachments(root: &Path, doc_id: u64) -> io::Result<Vec<AttachmentInfo>> {
    let raw = read_archived_raw(root, doc_id)?;
    let parsed = parse_mail(&raw)
        .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error.to_string()))?;
    let mut attachments = Vec::new();
    let mut resources = Vec::new();
    walk_mime_parts(&parsed, &mut 0, &mut attachments, &mut resources, None)?;
    Ok(attachments)
}

pub fn list_mime_resources(root: &Path, doc_id: u64) -> io::Result<Vec<MimeResourceInfo>> {
    let raw = read_archived_raw(root, doc_id)?;
    let parsed = parse_mail(&raw)
        .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error.to_string()))?;
    let mut attachments = Vec::new();
    let mut resources = Vec::new();
    walk_mime_parts(&parsed, &mut 0, &mut attachments, &mut resources, None)?;
    Ok(resources)
}

pub(crate) fn attachment_payloads(parsed: &ParsedMail<'_>) -> io::Result<Vec<AttachmentPayload>> {
    let mut attachments = Vec::new();
    let mut resources = Vec::new();
    let mut payloads = Vec::new();
    walk_mime_parts(
        parsed,
        &mut 0,
        &mut attachments,
        &mut resources,
        Some(&mut payloads),
    )?;
    Ok(payloads)
}

fn collect_html_document(
    part: &ParsedMail<'_>,
    html: &mut Option<String>,
    resources: &mut Vec<HtmlResource>,
) -> io::Result<()> {
    if !part.subparts.is_empty() {
        for child in &part.subparts {
            collect_html_document(child, html, resources)?;
        }
        return Ok(());
    }
    let disposition = part.get_content_disposition();
    let filename = mime_filename(part);
    let content_id = mime_content_id(part);
    let inline = matches!(disposition.disposition, mailparse::DispositionType::Inline);
    let downloadable = matches!(
        disposition.disposition,
        mailparse::DispositionType::Attachment
    ) || (filename.is_some() && !(inline && content_id.is_some()));
    let mime = part.ctype.mimetype.to_ascii_lowercase();
    if mime == "text/html" && !downloadable {
        *html = Some(
            part.get_body()
                .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error.to_string()))?,
        );
    } else if let Some(content_id) = content_id {
        resources.push(HtmlResource {
            mime,
            content_id,
            bytes: mime_part_bytes(part)?,
        });
    }
    Ok(())
}

pub fn read_html_document(root: &Path, doc_id: u64) -> io::Result<Option<HtmlDocument>> {
    let raw = read_archived_raw(root, doc_id)?;
    let parsed = parse_mail(&raw)
        .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error.to_string()))?;
    let mut html = None;
    let mut resources = Vec::new();
    collect_html_document(&parsed, &mut html, &mut resources)?;
    Ok(html.map(|html| HtmlDocument { html, resources }))
}

pub fn read_attachment(root: &Path, doc_id: u64, attachment_id: u32) -> io::Result<Vec<u8>> {
    let raw = read_archived_raw(root, doc_id)?;
    let parsed = parse_mail(&raw)
        .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error.to_string()))?;
    let mut next_id = 0;
    fn find(part: &ParsedMail<'_>, next_id: &mut u32, wanted: u32) -> io::Result<Option<Vec<u8>>> {
        if !part.subparts.is_empty() {
            for child in &part.subparts {
                if let Some(value) = find(child, next_id, wanted)? {
                    return Ok(Some(value));
                }
            }
            return Ok(None);
        }
        let disposition = part.get_content_disposition();
        let filename = mime_filename(part);
        let content_id = mime_content_id(part);
        let inline = matches!(disposition.disposition, mailparse::DispositionType::Inline);
        let downloadable = matches!(
            disposition.disposition,
            mailparse::DispositionType::Attachment
        ) || (filename.is_some() && !(inline && content_id.is_some()));
        let id = *next_id;
        *next_id += 1;
        if downloadable && id == wanted {
            return mime_part_bytes(part).map(Some);
        }
        Ok(None)
    }
    find(&parsed, &mut next_id, attachment_id)?
        .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "attachment not found"))
}

pub struct GmailSearchIndex {
    index: Index,
    reader: IndexReader,
    fields: TantivyFields,
    catalog: Connection,
}

impl GmailSearchIndex {
    pub fn open(root: &Path) -> io::Result<Self> {
        let (index, fields) = open_or_create_gmail_tantivy(root).map_err(io::Error::other)?;
        let reader = index.reader().map_err(io::Error::other)?;
        create_metadata(&root.join("metadata.sqlite")).map_err(sqlite_io)?;
        let catalog = Connection::open(root.join("metadata.sqlite")).map_err(sqlite_io)?;
        let source_rows: i64 = catalog
            .query_row(
                "SELECT (SELECT COUNT(*) FROM gmail_messages WHERE source_state='present') + (SELECT COUNT(*) FROM imap_messages WHERE source_state='present')",
                [],
                |row| row.get(0),
            )
            .unwrap_or(0);
        if reader.searcher().num_docs() == 0 && source_rows > 0 {
            drop(reader);
            drop(catalog);
            drop(index);
            let stats = index_gmail_archive(root)?;
            if stats.indexed == 0 && stats.parse_failures > 0 {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    "derived Tantivy index could not be rebuilt",
                ));
            }
            return Self::open(root);
        }
        Ok(Self {
            index,
            reader,
            fields,
            catalog,
        })
    }

    pub fn search(&self, query: &str, limit: usize) -> io::Result<Vec<GmailSearchResult>> {
        let mut request = SearchRequest {
            text: query.to_string(),
            limit,
            ..SearchRequest::default()
        };
        // Keep the historical CLI syntax as a compatibility wrapper. New code
        // should construct SearchRequest directly.
        let (text, from, to, date_from, date_to) = legacy_request_parts(query);
        request.text = text;
        request.from = from;
        request.to = to;
        request.date_from = date_from;
        request.date_to = date_to;
        self.search_request(&request)
    }

    pub fn search_request(&self, request: &SearchRequest) -> io::Result<Vec<GmailSearchResult>> {
        if request.limit == 0 || (request.text.trim().is_empty() && !request.has_filters()) {
            return Ok(Vec::new());
        }
        let searcher = self.reader.searcher();
        let parsed = structured_query(&self.index, self.fields, request)
            .map_err(|error| io::Error::new(io::ErrorKind::InvalidInput, error))?;
        let top_docs: Vec<(f32, tantivy::DocAddress)> = if request.text.trim().is_empty() {
            searcher
                .search(
                    &parsed,
                    &TopDocs::with_limit(request.limit)
                        .order_by_fast_field::<i64>("timestamp", Order::Desc),
                )
                .map_err(io::Error::other)?
                .into_iter()
                .map(|(_, address)| (0.0, address))
                .collect()
        } else {
            searcher
                .search(
                    &parsed,
                    &TopDocs::with_limit(request.limit).order_by_score(),
                )
                .map_err(io::Error::other)?
        };
        let mut results = Vec::new();
        for (score, address) in top_docs {
            let document: TantivyDocument = searcher.doc(address).map_err(io::Error::other)?;
            let doc_id = document
                .get_first(self.fields.doc_id)
                .and_then(|value| value.as_u64())
                .unwrap_or_default();
            let timestamp = document
                .get_first(self.fields.timestamp)
                .and_then(|value| value.as_i64())
                .unwrap_or_default();
            let source_account = document
                .get_first(self.fields.account)
                .and_then(|value| value.as_str())
                .unwrap_or_default()
                .to_string();
            let sender = document
                .get_first(self.fields.sender)
                .and_then(|value| value.as_str())
                .unwrap_or_default()
                .to_string();
            let recipients = document
                .get_first(self.fields.recipients)
                .and_then(|value| value.as_str())
                .unwrap_or_default()
                .to_string();
            let subject = document
                .get_first(self.fields.subject)
                .and_then(|value| value.as_str())
                .unwrap_or_default()
                .to_string();
            let snippet = document
                .get_first(self.fields.body)
                .and_then(|value| value.as_str())
                .unwrap_or_default()
                .chars()
                .take(180)
                .collect();
            let attachment_count = document
                .get_first(self.fields.attachment_count)
                .and_then(|value| value.as_u64())
                .unwrap_or_default();
            let archive_message_id = self
                .catalog
                .query_row(
                    "SELECT message_id FROM messages WHERE doc_id=?1",
                    [doc_id as i64],
                    |row| row.get(0),
                )
                .map_err(sqlite_io)?;
            results.push(GmailSearchResult {
                doc_id,
                score,
                timestamp,
                source_account,
                archive_message_id,
                sender,
                recipients,
                subject,
                snippet,
                attachment_count,
            });
        }
        Ok(results)
    }

    pub fn reload(&self) -> io::Result<()> {
        self.reader.reload().map_err(io::Error::other)
    }
}

#[derive(Clone, Debug, Default)]
pub struct ArchiveSummary {
    pub messages: u64,
    pub archive_bytes: u64,
    pub segments: u64,
    pub catalog_bytes: u64,
    pub index_bytes: u64,
    pub index_present: bool,
}

pub fn archive_summary(root: &Path) -> io::Result<ArchiveSummary> {
    let catalog_path = root.join("metadata.sqlite");
    let messages = if catalog_path.exists() {
        let connection = Connection::open(&catalog_path).map_err(sqlite_io)?;
        connection
            .query_row("SELECT COUNT(*) FROM messages", [], |row| {
                row.get::<_, i64>(0)
            })
            .map_err(sqlite_io)? as u64
    } else {
        0
    };
    let archive_root = root.join("archive");
    let mut segments = 0;
    if archive_root.exists() {
        for entry in fs::read_dir(&archive_root)? {
            let path = entry?.path();
            if path.extension().and_then(|value| value.to_str()) == Some("arc") {
                segments += 1;
            }
        }
    }
    let index_root = gmail_index_dir(root);
    Ok(ArchiveSummary {
        messages,
        archive_bytes: directory_bytes(&archive_root)?,
        segments,
        catalog_bytes: catalog_path
            .metadata()
            .map(|value| value.len())
            .unwrap_or(0),
        index_bytes: directory_bytes(&index_root)?,
        index_present: index_root.join("meta.json").exists(),
    })
}

pub fn available_gmail_labels(root: &Path) -> io::Result<Vec<String>> {
    let catalog = Connection::open(root.join("metadata.sqlite")).map_err(sqlite_io)?;
    let mut statement = catalog
        .prepare("SELECT label_ids FROM gmail_messages WHERE source_state='present'")
        .map_err(sqlite_io)?;
    let rows = statement
        .query_map([], |row| row.get::<_, String>(0))
        .map_err(sqlite_io)?;
    let mut labels = HashSet::new();
    for row in rows {
        let value = row.map_err(sqlite_io)?;
        for label in labels_for_index(&value) {
            labels.insert(label);
        }
    }
    let mut labels: Vec<_> = labels.into_iter().collect();
    labels.sort();
    Ok(labels)
}

#[derive(Clone, Debug)]
pub struct ParsedMessage {
    pub id: u64,
    pub timestamp: i64,
    pub sender: String,
    pub recipients: String,
    pub subject: String,
    pub body: String,
    pub folder: String,
    pub account: String,
    pub raw_bytes: usize,
}

#[derive(Clone, Debug, Default)]
pub struct PipelineStats {
    pub messages: u64,
    pub read_us: u128,
    pub parse_us: u128,
}

#[derive(Clone, Debug)]
pub struct LatencyStats {
    pub count: usize,
    pub p50_us: u128,
    pub p95_us: u128,
    pub p99_us: u128,
    pub max_us: u128,
}

#[derive(Clone, Debug)]
pub struct BenchmarkReport {
    pub archive: DatasetStats,
    pub archive_bytes: u64,
    pub sqlite_bytes: u64,
    pub tantivy_bytes: u64,
    pub import_ms: u128,
    pub sqlite_index_ms: u128,
    pub tantivy_index_ms: u128,
    pub sqlite_latency: LatencyStats,
    pub tantivy_latency: LatencyStats,
    pub sqlite_hot_latency: Option<LatencyStats>,
    pub tantivy_hot_latency: Option<LatencyStats>,
}

#[derive(Clone, Copy)]
struct Rng(u64);

impl Rng {
    fn new(seed: u64) -> Self {
        Self(seed ^ 0x9e37_79b9_7f4a_7c15)
    }
    fn next(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x >> 12;
        x ^= x << 25;
        x ^= x >> 27;
        self.0 = x;
        x.wrapping_mul(0x2545_f491_4f6c_dd1d)
    }
    fn range(&mut self, upper: usize) -> usize {
        (self.next() as usize) % upper
    }
    fn chance(&mut self, percent: u32) -> bool {
        (self.next() % 100) < percent as u64
    }
}

const NAMES: &[&str] = &[
    "alice",
    "bob",
    "carol",
    "david",
    "erin",
    "françois",
    "günter",
    "花子",
    "lucía",
    "маша",
];
const DOMAINS: &[&str] = &[
    "example.net",
    "archive.test",
    "research.example",
    "mail.local",
];
const COMMON_TERMS: &[&str] = &[
    "meeting",
    "project",
    "invoice",
    "travel",
    "family",
    "release",
    "schedule",
    "document",
    "archive",
    "important",
];
const RARE_TERMS: &[&str] = &[
    "quartz",
    "mycelium",
    "wavelength",
    "archipelago",
    "cristallisation",
    "Übertragung",
    "稀少",
    "ñandú",
];
const LANGUAGES: &[(&str, &str, &str)] = &[
    (
        "en",
        "The project archive contains a useful summary and the next schedule.",
        "Please review the attached document before the meeting.",
    ),
    (
        "fr",
        "Le projet contient un résumé utile et le calendrier de la prochaine étape.",
        "Merci de vérifier le document joint avant la réunion.",
    ),
    (
        "de",
        "Das Projektarchiv enthält eine Zusammenfassung und den nächsten Termin.",
        "Bitte prüfen Sie das angehängte Dokument vor dem Treffen.",
    ),
    (
        "es",
        "El archivo del proyecto contiene un resumen y el próximo calendario.",
        "Revise el documento adjunto antes de la reunión.",
    ),
    (
        "ja",
        "プロジェクトのアーカイブには要約と次の予定が含まれています。",
        "会議の前に添付文書を確認してください。",
    ),
];

pub fn generate_message(config: CorpusConfig, id: u64) -> Message {
    let mut rng = Rng::new(config.seed.wrapping_add(id.wrapping_mul(0x9e37_79b9)));
    let sender_name = NAMES[rng.range(NAMES.len())];
    let sender = format!("{sender_name}@{}", DOMAINS[rng.range(DOMAINS.len())]);
    let mut recipients = Vec::new();
    let recipient_count = 1 + rng.range(3);
    for _ in 0..recipient_count {
        let name = NAMES[rng.range(NAMES.len())];
        let address = format!("{name}@{}", DOMAINS[rng.range(DOMAINS.len())]);
        if !recipients.contains(&address) {
            recipients.push(address);
        }
    }
    let account = format!("account-{}", rng.range(4));
    let folder = ["Inbox", "Sent", "Archive", "Projects", "Receipts"][rng.range(5)].to_string();
    let thread = format!("thread-{:05}", id / (1 + rng.range(12) as u64));
    let (language, standard, closing) = LANGUAGES[rng.range(LANGUAGES.len())];
    let term = if rng.chance(8) {
        RARE_TERMS[rng.range(RARE_TERMS.len())]
    } else {
        COMMON_TERMS[rng.range(COMMON_TERMS.len())]
    };
    let subject = format!("{} — {} #{:06}", term, language, id % 1000);
    let repeat_count = body_repeat_count(config.profile, &mut rng);
    let mut text_body = format!("{standard}\n\n{closing}\n\nThread: {thread}. ");
    for _ in 0..repeat_count {
        text_body.push_str(term);
        text_body.push(' ');
    }
    let html_body = if rng.chance(65) {
        Some(format!(
            "<html><body><p>{standard}</p><p>{closing}</p><p>{term}</p></body></html>"
        ))
    } else {
        None
    };
    let mut attachments = Vec::new();
    if config.attachment_rate > 0 && rng.chance(config.attachment_rate) {
        let category = rng.range(100);
        let (bytes, mime, key, compressible) = if category < 15 {
            (
                5 * 1024,
                "image/png",
                format!("logo-{}", rng.range(8)),
                false,
            )
        } else if category < 45 && rng.chance(config.duplicate_rate) {
            (
                variable_attachment_size(config.profile, &mut rng, config.max_attachment_bytes),
                "application/pdf",
                format!("shared-document-{}", rng.range(128)),
                false,
            )
        } else if category < 60 && rng.chance(config.duplicate_rate) {
            (
                variable_attachment_size(config.profile, &mut rng, config.max_attachment_bytes),
                "application/vnd.openxmlformats-officedocument.wordprocessingml.document",
                format!("forwarded-document-{}", rng.range(64)),
                true,
            )
        } else if category < 78 {
            (
                variable_attachment_size(config.profile, &mut rng, config.max_attachment_bytes),
                "image/jpeg",
                format!("unique-photo-{id}"),
                false,
            )
        } else if category < 90 {
            (
                variable_attachment_size(config.profile, &mut rng, config.max_attachment_bytes),
                "application/zip",
                format!("compressed-{id}"),
                false,
            )
        } else {
            (
                variable_attachment_size(config.profile, &mut rng, config.max_attachment_bytes),
                "text/csv",
                format!("compressible-{id}"),
                true,
            )
        };
        attachments.push(Attachment {
            filename: format!("{term}-{id}.dat"),
            mime: mime.into(),
            bytes,
            hash: format!("{}:{}", key, if compressible { "text" } else { "blob" }),
        });
    }
    let timestamp = 1_577_836_800i64 + (id as i64 * 86_400 / 3) + rng.range(86_400) as i64;
    let message_id = format!("<msg-{id:012x}-{language}@archive.test>");
    let mut raw = Vec::new();
    write_raw_line(&mut raw, &format!("Message-ID: {message_id}"));
    write_raw_line(&mut raw, &format!("Date-Unix: {timestamp}"));
    write_raw_line(&mut raw, &format!("From: {sender}"));
    write_raw_line(&mut raw, &format!("To: {}", recipients.join(", ")));
    write_raw_line(&mut raw, &format!("Subject: {subject}"));
    write_raw_line(&mut raw, &format!("X-Account: {account}"));
    write_raw_line(&mut raw, &format!("X-Folder: {folder}"));
    write_raw_line(&mut raw, "MIME-Version: 1.0");
    write_raw_line(&mut raw, "Content-Type: multipart/alternative");
    raw.extend_from_slice(b"\r\n");
    raw.extend_from_slice(text_body.as_bytes());
    raw.extend_from_slice(b"\r\n\n");
    if let Some(html) = &html_body {
        raw.extend_from_slice(html.as_bytes());
        raw.extend_from_slice(b"\r\n");
    }
    for attachment in &mut attachments {
        let key = attachment.hash.clone();
        let compressible =
            attachment.mime == "text/csv" || attachment.mime.contains("wordprocessingml");
        let mut payload_rng = Rng::new(config.seed ^ fnv64(key.as_bytes()));
        let mut payload = Vec::with_capacity(attachment.bytes);
        for _ in 0..attachment.bytes {
            payload.push(if compressible {
                b'A' + (payload_rng.next() % 8) as u8
            } else {
                payload_rng.next() as u8
            });
        }
        attachment.hash = format!("{:016x}", fnv64(&payload));
        raw.extend_from_slice(
            format!(
                "\r\n--attachment; filename={}; hash={}\r\n",
                attachment.filename, attachment.hash
            )
            .as_bytes(),
        );
        raw.extend_from_slice(&payload);
    }
    Message {
        id,
        message_id,
        timestamp,
        sender,
        recipients,
        subject,
        text_body,
        html_body,
        account,
        folder,
        thread,
        attachments,
        raw,
    }
}

fn body_repeat_count(profile: CorpusProfile, rng: &mut Rng) -> usize {
    let roll = rng.range(10_000);
    let (small, medium, large) = match profile {
        CorpusProfile::Light => (8_300, 1_500, 190),
        CorpusProfile::Personal => (7_200, 2_200, 500),
        CorpusProfile::Heavy => (6_000, 2_600, 1_000),
    };
    if roll < small {
        1 + rng.range(3)
    } else if roll < small + medium {
        20 + rng.range(80)
    } else if roll < small + medium + large {
        250 + rng.range(750)
    } else {
        4_000 + rng.range(20_000)
    }
}

fn variable_attachment_size(profile: CorpusProfile, rng: &mut Rng, max_bytes: usize) -> usize {
    let base: usize = match profile {
        CorpusProfile::Light => 512,
        CorpusProfile::Personal => 32 * 1024,
        CorpusProfile::Heavy => 256 * 1024,
    };
    let multiplier = match rng.range(100) {
        0..=59 => 1,
        60..=84 => 4,
        85..=96 => 32,
        _ => 256,
    };
    base.saturating_mul(multiplier).min(max_bytes.max(512))
}

fn write_raw_line(raw: &mut Vec<u8>, line: &str) {
    raw.extend_from_slice(line.as_bytes());
    raw.extend_from_slice(b"\r\n");
}

fn fnv64(bytes: &[u8]) -> u64 {
    let mut hash = 0xcbf2_9ce4_8422_2325u64;
    for byte in bytes {
        hash ^= *byte as u64;
        hash = hash.wrapping_mul(0x1000_0000_01b3);
    }
    hash
}

fn percentile_size(values: &[usize], fraction: f64) -> usize {
    values
        .get(((values.len().saturating_sub(1)) as f64 * fraction).round() as usize)
        .copied()
        .unwrap_or(0)
}

fn duplicate_sizes(
    counts: &std::collections::HashMap<String, u64>,
    sizes: &std::collections::HashMap<String, u64>,
) -> Vec<usize> {
    let mut result = Vec::new();
    for (hash, count) in counts {
        if *count > 1 {
            if let Some(size) = sizes.get(hash) {
                for _ in 1..*count {
                    result.push(*size as usize);
                }
            }
        }
    }
    result.sort_unstable();
    result
}

fn compressed_parts(raw: &[u8]) -> (u64, u64, u64, u64) {
    let split = raw
        .windows(b"\r\n--attachment;".len())
        .position(|window| window == b"\r\n--attachment;")
        .unwrap_or(raw.len());
    let (text, attachments) = raw.split_at(split);
    let gzip = |bytes: &[u8]| {
        let mut encoder = GzEncoder::new(Vec::new(), Compression::default());
        encoder.write_all(bytes).expect("gzip write");
        encoder.finish().expect("gzip finish").len() as u64
    };
    (
        gzip(text),
        gzip(attachments),
        zstd::stream::encode_all(text, 3)
            .expect("zstd text encode")
            .len() as u64,
        zstd::stream::encode_all(attachments, 3)
            .expect("zstd attachment encode")
            .len() as u64,
    )
}

pub fn corpus_stats(config: CorpusConfig) -> DatasetStats {
    let mut sizes = Vec::with_capacity(config.messages.min(1_000_000) as usize);
    let mut bytes = 0u64;
    let mut attachments = 0u64;
    let mut hashes = std::collections::HashSet::new();
    let mut attachment_bytes = 0u64;
    let mut unique_attachment_sizes = std::collections::HashMap::new();
    let mut attachment_counts = std::collections::HashMap::new();
    let mut compressed_bytes = 0u64;
    let mut zstd_bytes = 0u64;
    let mut text_compressed_bytes = 0u64;
    let mut attachment_compressed_bytes = 0u64;
    let mut text_zstd_bytes = 0u64;
    let mut attachment_zstd_bytes = 0u64;
    let mut mime_text_bytes = 0u64;
    for id in 0..config.messages {
        let message = generate_message(config, id);
        bytes += message.raw.len() as u64;
        sizes.push(message.raw.len());
        attachments += message.attachments.len() as u64;
        let message_attachment_bytes: u64 = message
            .attachments
            .iter()
            .map(|attachment| attachment.bytes as u64)
            .sum();
        mime_text_bytes += message.raw.len() as u64 - message_attachment_bytes;
        for attachment in &message.attachments {
            hashes.insert(attachment.hash.clone());
            attachment_bytes += attachment.bytes as u64;
            *attachment_counts
                .entry(attachment.hash.clone())
                .or_insert(0) += 1;
            unique_attachment_sizes
                .entry(attachment.hash.clone())
                .or_insert(attachment.bytes as u64);
        }
        if config.measure_compression {
            let (text_gzip, attachment_gzip, text_zstd, attachment_zstd) =
                compressed_parts(&message.raw);
            text_compressed_bytes += text_gzip;
            attachment_compressed_bytes += attachment_gzip;
            text_zstd_bytes += text_zstd;
            attachment_zstd_bytes += attachment_zstd;
            compressed_bytes += text_gzip + attachment_gzip;
            zstd_bytes += text_zstd + attachment_zstd;
        }
    }
    sizes.sort_unstable();
    let duplicate_sizes = duplicate_sizes(&attachment_counts, &unique_attachment_sizes);
    let unique_attachment_bytes: u64 = unique_attachment_sizes.values().sum();
    DatasetStats {
        messages: config.messages,
        bytes,
        min_bytes: sizes.first().copied().unwrap_or(0),
        p90_bytes: percentile_size(&sizes, 0.90),
        p99_bytes: percentile_size(&sizes, 0.99),
        max_bytes: sizes.last().copied().unwrap_or(0),
        median_bytes: sizes.get(sizes.len() / 2).copied().unwrap_or(0),
        mean_bytes: bytes / config.messages.max(1),
        mime_text_bytes,
        compressed_bytes,
        zstd_bytes,
        text_compressed_bytes,
        attachment_compressed_bytes,
        text_zstd_bytes,
        attachment_zstd_bytes,
        attachments,
        unique_attachment_hashes: hashes.len(),
        attachment_bytes,
        unique_attachment_bytes,
        duplicate_attachment_objects: attachment_counts
            .values()
            .map(|count| count.saturating_sub(1))
            .sum(),
        duplicate_attachment_bytes: attachment_bytes - unique_attachment_bytes,
        duplicate_size_p50: percentile_size(&duplicate_sizes, 0.50),
        duplicate_size_p90: percentile_size(&duplicate_sizes, 0.90),
        duplicate_size_max: duplicate_sizes.last().copied().unwrap_or(0),
    }
}

fn attachment_ranges(raw: &[u8]) -> Vec<(usize, usize, usize)> {
    let marker = b"\r\n--attachment;";
    let starts: Vec<usize> = raw
        .windows(marker.len())
        .enumerate()
        .filter_map(|(index, window)| (window == marker).then_some(index))
        .collect();
    starts
        .iter()
        .enumerate()
        .filter_map(|(index, marker_start)| {
            let header_end = raw[*marker_start + marker.len()..]
                .windows(2)
                .position(|window| window == b"\r\n")
                .map(|offset| marker_start + marker.len() + offset + 2)?;
            let payload_end = starts.get(index + 1).copied().unwrap_or(raw.len());
            Some((*marker_start, header_end, payload_end))
        })
        .collect()
}

fn cas_hash(payload: &[u8], decoded: bool) -> String {
    // The current synthetic MIME payload is already decoded. Keeping the
    // flag explicit makes the equality of exact/decoded results observable,
    // and leaves the decoder boundary ready for real MIME fixtures.
    let _ = decoded;
    blake3::hash(payload).to_hex().to_string()
}

fn externalize_message(
    raw: &[u8],
    variant: CasVariant,
    blobs: &mut std::collections::HashMap<String, Vec<u8>>,
) -> (Vec<u8>, Vec<CasPiece>, u64, u64, usize) {
    if variant == CasVariant::Inline {
        return (raw.to_vec(), vec![CasPiece::Inline(raw.to_vec())], 0, 0, 0);
    }
    let decoded = variant == CasVariant::Decoded;
    let threshold = match variant {
        CasVariant::Hybrid { threshold } => threshold,
        _ => 0,
    };
    let ranges = attachment_ranges(raw);
    let mut stored = Vec::with_capacity(raw.len());
    let mut pieces = Vec::new();
    let mut cursor = 0;
    let mut externalized = 0;
    let mut hashed = 0;
    let mut max_blob = 0;
    for (marker_start, payload_start, payload_end) in ranges {
        stored.extend_from_slice(&raw[cursor..payload_start]);
        pieces.push(CasPiece::Inline(raw[cursor..payload_start].to_vec()));
        let payload = &raw[payload_start..payload_end];
        hashed += payload.len() as u64;
        let should_externalize = payload.len() >= threshold;
        if should_externalize {
            let hash = cas_hash(payload, decoded);
            blobs
                .entry(hash.clone())
                .or_insert_with(|| payload.to_vec());
            let reference = format!("CAS-REF:{hash}:{}\r\n", payload.len());
            stored.extend_from_slice(reference.as_bytes());
            pieces.push(CasPiece::Blob(hash));
            externalized += 1;
            max_blob = max_blob.max(payload.len());
        } else {
            stored.extend_from_slice(payload);
            pieces.push(CasPiece::Inline(payload.to_vec()));
        }
        cursor = payload_end;
        let _ = marker_start;
    }
    stored.extend_from_slice(&raw[cursor..]);
    pieces.push(CasPiece::Inline(raw[cursor..].to_vec()));
    (stored, pieces, externalized, hashed, max_blob)
}

fn reconstruct_pieces(
    pieces: &[CasPiece],
    blobs: &std::collections::HashMap<String, Vec<u8>>,
) -> Vec<u8> {
    let mut result = Vec::new();
    for piece in pieces {
        match piece {
            CasPiece::Inline(bytes) => result.extend_from_slice(bytes),
            CasPiece::Blob(hash) => result.extend_from_slice(blobs.get(hash).expect("CAS blob")),
        }
    }
    result
}

pub fn run_cas(root: &Path, config: CorpusConfig, variant: CasVariant) -> io::Result<CasStats> {
    if root.exists() {
        fs::remove_dir_all(root)?;
    }
    fs::create_dir_all(root)?;
    let messages_path = root.join("messages.cas");
    let manifest_path = root.join("manifest.tsv");
    let mut messages_file = File::create(&messages_path)?;
    let mut blob_segment = 0u64;
    let mut blob_offset = 0u64;
    let mut blobs_file = File::create(root.join("blobs-000000.cas"))?;
    let mut manifest = File::create(&manifest_path)?;
    let started = Instant::now();
    let mut stats = CasStats {
        variant: match variant {
            CasVariant::Hybrid { threshold } => format!("hybrid-{threshold}"),
            _ => variant.name().into(),
        },
        messages: config.messages,
        ..Default::default()
    };
    let mut blobs = std::collections::HashMap::new();
    let mut first_sample: Option<(Vec<u8>, Vec<CasPiece>)> = None;
    for id in 0..config.messages {
        let message = generate_message(config, id);
        stats.input_bytes += message.raw.len() as u64;
        let hash_started = Instant::now();
        let (stored, pieces, externalized, hashed, max_blob) =
            externalize_message(&message.raw, variant, &mut blobs);
        stats.hash_us += hash_started.elapsed().as_micros();
        stats.externalized_objects += externalized;
        stats.hashed_bytes += hashed;
        stats.max_blob_bytes = stats.max_blob_bytes.max(max_blob);
        if first_sample.is_none() {
            first_sample = Some((message.raw.clone(), pieces.clone()));
        }
        messages_file.write_all(&(stored.len() as u64).to_le_bytes())?;
        messages_file.write_all(&[0u8; 24])?;
        messages_file.write_all(&stored)?;
    }
    for (hash, bytes) in &blobs {
        if blob_offset > 0 && blob_offset + 8 + bytes.len() as u64 > 64 * 1024 * 1024 {
            blobs_file.sync_all()?;
            blob_segment += 1;
            blob_offset = 0;
            blobs_file = File::create(root.join(format!("blobs-{blob_segment:06}.cas")))?;
        }
        blobs_file.write_all(&(bytes.len() as u64).to_le_bytes())?;
        blobs_file.write_all(bytes)?;
        blob_offset += 8 + bytes.len() as u64;
        writeln!(manifest, "{hash}\t{}", bytes.len())?;
    }
    manifest.sync_all()?;
    blobs_file.sync_all()?;
    messages_file.sync_all()?;
    stats.import_us = started.elapsed().as_micros();
    stats.blobs = blobs.len() as u64;
    stats.unique_blob_bytes = blobs.values().map(|value| value.len() as u64).sum();
    stats.message_store_bytes = fs::metadata(&messages_path)?.len();
    stats.blob_bytes = fs::read_dir(root)?
        .filter_map(Result::ok)
        .filter(|entry| entry.file_name().to_string_lossy().starts_with("blobs-"))
        .filter_map(|entry| fs::metadata(entry.path()).ok())
        .map(|metadata| metadata.len())
        .sum();
    stats.manifest_bytes = fs::metadata(&manifest_path)?.len();
    stats.physical_bytes = directory_bytes(root)?;
    if let Some((original, pieces)) = first_sample {
        let started = Instant::now();
        assert_eq!(original, reconstruct_pieces(&pieces, &blobs));
        stats.reconstruction_us = started.elapsed().as_micros();
    }
    let started = Instant::now();
    let mut file = File::open(&messages_path)?;
    let mut header = [0u8; 32];
    file.read_exact(&mut header)?;
    let stored_len = u64::from_le_bytes(header[..8].try_into().unwrap()) as usize;
    let mut stored = vec![0u8; stored_len];
    file.read_exact(&mut stored)?;
    stats.random_access_us = started.elapsed().as_micros();
    Ok(stats)
}

pub struct ArchiveWriter {
    root: PathBuf,
    segment_bytes: u64,
    file: File,
    segment_name: String,
    offset: u64,
    segment_number: u64,
}

impl ArchiveWriter {
    pub fn open(root: &Path, segment_bytes: u64) -> io::Result<Self> {
        fs::create_dir_all(root)?;
        let numbers = fs::read_dir(root)?
            .filter_map(Result::ok)
            .filter_map(|entry| {
                entry
                    .file_name()
                    .to_str()
                    .and_then(|name| name.strip_prefix("segment-"))
                    .and_then(|n| n.strip_suffix(".arc"))
                    .and_then(|n| n.parse::<u64>().ok())
            });
        let segment_number = numbers.max().unwrap_or(0);
        let segment_name = format!("segment-{segment_number:06}.arc");
        let path = root.join(&segment_name);
        let mut file = OpenOptions::new()
            .create(true)
            .read(true)
            .append(true)
            .open(&path)?;
        let offset = file.seek(SeekFrom::End(0))?;
        Ok(Self {
            root: root.to_path_buf(),
            segment_bytes: segment_bytes.max(1024),
            file,
            segment_name,
            offset,
            segment_number,
        })
    }

    pub fn append(&mut self, message: &Message) -> io::Result<ArchiveLocation> {
        self.append_raw(message.id, &message.raw)
    }

    pub fn append_raw(&mut self, id: u64, raw: &[u8]) -> io::Result<ArchiveLocation> {
        let frame_bytes = 8 + 8 + 8 + 8 + raw.len() as u64;
        if self.offset > 0 && self.offset + frame_bytes > self.segment_bytes {
            self.file.sync_all()?;
            self.segment_number += 1;
            self.segment_name = format!("segment-{:06}.arc", self.segment_number);
            self.file = OpenOptions::new()
                .create(true)
                .read(true)
                .append(true)
                .open(self.root.join(&self.segment_name))?;
            self.offset = self.file.seek(SeekFrom::End(0))?;
        }
        let start = self.offset;
        self.file.write_all(FRAME_MAGIC)?;
        self.file.write_all(&id.to_le_bytes())?;
        self.file.write_all(&(raw.len() as u64).to_le_bytes())?;
        self.file.write_all(&fnv64(raw).to_le_bytes())?;
        self.file.write_all(raw)?;
        self.offset += frame_bytes;
        Ok(ArchiveLocation {
            segment: self.segment_name.clone(),
            offset: start,
            frame_bytes,
        })
    }

    pub fn sync(&mut self) -> io::Result<()> {
        self.file.sync_all()
    }
}

pub fn recover_segments(root: &Path) -> io::Result<(u64, u64)> {
    let mut recovered = 0;
    let mut truncated = 0;
    let mut paths: Vec<_> = fs::read_dir(root)?
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| path.extension().and_then(|e| e.to_str()) == Some("arc"))
        .collect();
    paths.sort();
    for path in paths {
        let mut file = OpenOptions::new().read(true).write(true).open(&path)?;
        let mut valid_end = 0u64;
        loop {
            let mut header = [0u8; 32];
            file.seek(SeekFrom::Start(valid_end))?;
            match file.read_exact(&mut header) {
                Ok(()) => {}
                Err(error) if error.kind() == io::ErrorKind::UnexpectedEof => break,
                Err(error) => return Err(error),
            }
            if &header[..8] != FRAME_MAGIC {
                break;
            }
            let id = u64::from_le_bytes(header[8..16].try_into().unwrap());
            let len = u64::from_le_bytes(header[16..24].try_into().unwrap());
            let checksum = u64::from_le_bytes(header[24..32].try_into().unwrap());
            if len > 512 * 1024 * 1024 {
                break;
            }
            let mut body = vec![0u8; len as usize];
            if file.read_exact(&mut body).is_err() || fnv64(&body) != checksum {
                break;
            }
            let _ = id;
            valid_end += 32 + len;
            recovered += 1;
        }
        let actual = file.metadata()?.len();
        if valid_end < actual {
            file.set_len(valid_end)?;
            truncated += actual - valid_end;
        }
    }
    Ok((recovered, truncated))
}

fn archive_segment_path(root: &Path, segment_name: &str) -> io::Result<PathBuf> {
    let segment = Path::new(segment_name);
    if segment.is_absolute()
        || segment
            .components()
            .any(|component| !matches!(component, Component::Normal(_)))
        || !segment_name.starts_with("segment-")
        || !segment_name.ends_with(".arc")
        || segment_name
            .strip_prefix("segment-")
            .and_then(|value| value.strip_suffix(".arc"))
            .and_then(|value| value.parse::<u64>().ok())
            .is_none()
    {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "invalid archive segment coordinate",
        ));
    }
    Ok(root.join(segment))
}

fn inventory_inconsistent(
    doc_id: i64,
    location: Option<ArchiveLocation>,
    reason: impl Into<String>,
) -> RecordInventory {
    RecordInventory {
        doc_id,
        location,
        status: RecordInventoryStatus::Inconsistent {
            reason: reason.into(),
        },
    }
}

/// Check every record named by the SQLite catalogue independently.
///
/// The catalogue is opened read-only and this function never repairs, truncates,
/// creates or migrates either input. A missing segment/byte range is reported
/// separately from an invalid frame. Frame validation itself is delegated to
/// [`read_record`], the authoritative A1 reader.
pub fn inventory_records(root: &Path) -> io::Result<Vec<RecordInventory>> {
    let catalog = Connection::open_with_flags(
        root.join("metadata.sqlite"),
        OpenFlags::SQLITE_OPEN_READ_ONLY,
    )
    .map_err(sqlite_io)?;
    let mut statement = catalog
        .prepare(
            "SELECT doc_id, segment, archive_offset, frame_bytes FROM messages ORDER BY doc_id",
        )
        .map_err(sqlite_io)?;
    let rows = statement
        .query_map([], |row| {
            Ok((
                // messages.doc_id is INTEGER PRIMARY KEY, so decode it
                // independently before handling the coordinate columns.
                row.get::<_, i64>(0)?,
                row.get::<_, String>(1).map_err(|error| error.to_string()),
                row.get::<_, i64>(2).map_err(|error| error.to_string()),
                row.get::<_, i64>(3).map_err(|error| error.to_string()),
            ))
        })
        .map_err(sqlite_io)?;
    let mut inventory = Vec::new();
    for row in rows {
        let (doc_id, segment, offset, frame_bytes) = match row {
            Ok(value) => value,
            // A row whose primary key cannot be decoded cannot be represented
            // as a RecordInventory without inventing an identifier.
            Err(error) => return Err(sqlite_io(error)),
        };
        let segment = match segment {
            Ok(segment) => segment,
            Err(error) => {
                inventory.push(inventory_inconsistent(
                    doc_id,
                    None,
                    format!("catalog segment is invalid: {error}"),
                ));
                continue;
            }
        };
        let offset = match offset {
            Ok(offset) => offset,
            Err(error) => {
                inventory.push(inventory_inconsistent(
                    doc_id,
                    Some(ArchiveLocation {
                        segment,
                        offset: 0,
                        frame_bytes: 0,
                    }),
                    format!("catalog offset is invalid: {error}"),
                ));
                continue;
            }
        };
        let frame_bytes = match frame_bytes {
            Ok(frame_bytes) => frame_bytes,
            Err(error) => {
                inventory.push(inventory_inconsistent(
                    doc_id,
                    Some(ArchiveLocation {
                        segment,
                        offset: 0,
                        frame_bytes: 0,
                    }),
                    format!("catalog frame length is invalid: {error}"),
                ));
                continue;
            }
        };
        let location = ArchiveLocation {
            segment,
            offset: u64::try_from(offset).unwrap_or(0),
            frame_bytes: u64::try_from(frame_bytes).unwrap_or(0),
        };
        if doc_id < 0 || offset < 0 || frame_bytes < 0 {
            inventory.push(inventory_inconsistent(
                doc_id,
                Some(location),
                "negative catalogue record or archive coordinate",
            ));
            continue;
        }
        let path = match archive_segment_path(&root.join("archive"), &location.segment) {
            Ok(path) => path,
            Err(error) => {
                inventory.push(inventory_inconsistent(
                    doc_id,
                    Some(location),
                    error.to_string(),
                ));
                continue;
            }
        };
        let file_len = match fs::metadata(&path) {
            Ok(metadata) => metadata.len(),
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                inventory.push(RecordInventory {
                    doc_id,
                    location: Some(location),
                    status: RecordInventoryStatus::PhysicallyMissing,
                });
                continue;
            }
            Err(error) => {
                inventory.push(inventory_inconsistent(
                    doc_id,
                    Some(location),
                    error.to_string(),
                ));
                continue;
            }
        };
        let Some(coordinate_end) = location.offset.checked_add(location.frame_bytes) else {
            inventory.push(inventory_inconsistent(
                doc_id,
                Some(location),
                "archive coordinate overflow",
            ));
            continue;
        };
        if coordinate_end > file_len {
            inventory.push(RecordInventory {
                doc_id,
                location: Some(location),
                status: RecordInventoryStatus::PhysicallyMissing,
            });
            continue;
        }
        if location.frame_bytes < FRAME_HEADER_BYTES {
            inventory.push(inventory_inconsistent(
                doc_id,
                Some(location),
                "catalog frame length is smaller than the frame header",
            ));
            continue;
        }
        match read_record(&root.join("archive"), &location) {
            Ok((record_id, _)) if record_id == doc_id as u64 => inventory.push(RecordInventory {
                doc_id,
                location: Some(location),
                status: RecordInventoryStatus::AvailableValidated,
            }),
            Ok((record_id, _)) => inventory.push(inventory_inconsistent(
                doc_id,
                Some(location),
                format!("catalog/frame id mismatch: frame contains {record_id}"),
            )),
            Err(error) => inventory.push(inventory_inconsistent(
                doc_id,
                Some(location),
                error.to_string(),
            )),
        }
    }
    Ok(inventory)
}

pub fn read_record(root: &Path, location: &ArchiveLocation) -> io::Result<(u64, Vec<u8>)> {
    let path = archive_segment_path(root, &location.segment)?;
    if location.frame_bytes < FRAME_HEADER_BYTES {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "invalid archive frame length coordinate",
        ));
    }

    let file_len = fs::metadata(&path)?.len();
    let coordinate_end = location
        .offset
        .checked_add(location.frame_bytes)
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "archive coordinate overflow"))?;
    if location.offset > file_len || coordinate_end > file_len {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "archive coordinate is outside the segment",
        ));
    }

    let mut file = File::open(path)?;
    file.seek(SeekFrom::Start(location.offset))?;
    let mut header = [0u8; 32];
    file.read_exact(&mut header)?;
    if &header[..8] != FRAME_MAGIC {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "archive frame magic mismatch",
        ));
    }
    let id = u64::from_le_bytes(header[8..16].try_into().unwrap());
    let len = u64::from_le_bytes(header[16..24].try_into().unwrap());
    let checksum = u64::from_le_bytes(header[24..32].try_into().unwrap());
    let available_body_bytes = file_len - location.offset - FRAME_HEADER_BYTES;
    if len > available_body_bytes {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "archive frame body exceeds the segment",
        ));
    }
    let expected_frame_bytes = FRAME_HEADER_BYTES.checked_add(len).ok_or_else(|| {
        io::Error::new(io::ErrorKind::InvalidData, "archive frame length overflow")
    })?;
    if location.frame_bytes != expected_frame_bytes {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "archive frame length does not match catalog coordinate",
        ));
    }
    let body_len = usize::try_from(len).map_err(|_| {
        io::Error::new(
            io::ErrorKind::InvalidData,
            "archive frame body length does not fit in memory",
        )
    })?;
    let mut body = Vec::new();
    body.try_reserve_exact(body_len)
        .map_err(|error| io::Error::other(format!("archive frame allocation failed: {error}")))?;
    body.resize(body_len, 0);
    file.read_exact(&mut body)?;
    if fnv64(&body) != checksum {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "archive frame checksum mismatch",
        ));
    }
    Ok((id, body))
}

pub fn parse_archived_message(id: u64, raw: &[u8]) -> io::Result<ParsedMessage> {
    let text = String::from_utf8_lossy(raw);
    let (headers, body) = text.split_once("\r\n\r\n").unwrap_or((text.as_ref(), ""));
    let mut values = BTreeMap::new();
    for line in headers.lines() {
        if let Some((key, value)) = line.split_once(':') {
            values.insert(key.to_ascii_lowercase(), value.trim().to_string());
        }
    }
    let required = |key: &str| {
        values.get(key).cloned().ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::InvalidData,
                format!("missing archived header {key}"),
            )
        })
    };
    let body = body
        .split("\r\n--attachment;")
        .next()
        .unwrap_or(body)
        .trim()
        .to_string();
    Ok(ParsedMessage {
        id,
        timestamp: required("date-unix")?
            .parse()
            .map_err(|_| io::Error::new(io::ErrorKind::InvalidData, "invalid Date-Unix"))?,
        sender: required("from")?,
        recipients: required("to")?,
        subject: required("subject")?,
        body,
        folder: required("x-folder")?,
        account: required("x-account")?,
        raw_bytes: raw.len(),
    })
}

fn html_text(value: &str) -> String {
    let mut output = String::with_capacity(value.len());
    let mut in_tag = false;
    let mut in_ignored = false;
    let mut tag = String::new();
    for character in value.chars() {
        if character == '<' {
            in_tag = true;
            tag.clear();
            continue;
        }
        if in_tag {
            if character == '>' {
                let lowered = tag.trim().to_ascii_lowercase();
                if lowered.starts_with("script") || lowered.starts_with("style") {
                    in_ignored = true;
                } else if lowered.starts_with("/script") || lowered.starts_with("/style") {
                    in_ignored = false;
                }
                if lowered.starts_with("br")
                    || lowered.starts_with("/p")
                    || lowered.starts_with("/div")
                {
                    output.push('\n');
                }
                in_tag = false;
            } else if tag.len() < 64 {
                tag.push(character);
            }
            continue;
        }
        if !in_ignored {
            output.push(character);
        }
    }
    hide_plain_urls(
        &output
            .replace("&nbsp;", " ")
            .replace("&amp;", "&")
            .replace("&lt;", "<")
            .replace("&gt;", ">")
            .split_whitespace()
            .collect::<Vec<_>>()
            .join(" "),
    )
}

/// The first UI is deliberately text-only. Hide raw URLs in the derived
/// display text until the application has a safe, explicit link treatment.
/// The archived RAW and the search index are not affected.
fn hide_plain_urls(value: &str) -> String {
    value
        .split_whitespace()
        .map(|word| {
            if word.starts_with("http://")
                || word.starts_with("https://")
                || word.starts_with("www.")
            {
                "[lien externe]"
            } else {
                word
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

fn collect_mime_text(
    part: &ParsedMail<'_>,
    text: &mut String,
    attachment_types: &mut Vec<String>,
    attachment_count: &mut u64,
) -> Result<(), mailparse::MailParseError> {
    let disposition = part.get_content_disposition();
    let filename = mime_filename(part);
    let content_id = mime_content_id(part);
    let inline = matches!(disposition.disposition, mailparse::DispositionType::Inline);
    let is_attachment = matches!(
        disposition.disposition,
        mailparse::DispositionType::Attachment
    ) || (filename.is_some() && !(inline && content_id.is_some()));
    if is_attachment && part.subparts.is_empty() {
        *attachment_count += 1;
        attachment_types.push(part.ctype.mimetype.to_ascii_lowercase());
    }
    if !part.subparts.is_empty() {
        for child in &part.subparts {
            collect_mime_text(child, text, attachment_types, attachment_count)?;
        }
        return Ok(());
    }
    let mime = part.ctype.mimetype.to_ascii_lowercase();
    if mime == "text/plain" {
        text.push_str(&part.get_body()?);
        text.push('\n');
    } else if mime == "text/html" && !is_attachment {
        text.push_str(&html_text(&part.get_body()?));
        text.push('\n');
    }
    Ok(())
}

pub fn parse_gmail_message(
    raw: &[u8],
    labels: Vec<String>,
) -> Result<GmailParsedMessage, mailparse::MailParseError> {
    let parsed = parse_mail(raw)?;
    let mut body = String::new();
    let mut attachment_types = Vec::new();
    let mut attachment_count = 0;
    collect_mime_text(
        &parsed,
        &mut body,
        &mut attachment_types,
        &mut attachment_count,
    )?;
    let (attachment_text, attachment_text_stats) = if attachment_count > 0 {
        attachment_text::extract_attachment_texts(&parsed).unwrap_or_else(|_| {
            (
                String::new(),
                AttachmentTextStats {
                    encountered: attachment_count,
                    failures: attachment_count,
                    ..Default::default()
                },
            )
        })
    } else {
        (String::new(), AttachmentTextStats::default())
    };
    let header = |name: &str| parsed.headers.get_first_value(name).unwrap_or_default();
    Ok(GmailParsedMessage {
        subject: header("Subject"),
        sender: header("From"),
        recipients: [header("To"), header("Cc"), header("Bcc")]
            .into_iter()
            .filter(|value| !value.is_empty())
            .collect::<Vec<_>>()
            .join(" "),
        body,
        labels,
        attachment_types,
        attachment_count,
        attachment_text,
        attachment_text_stats,
    })
}

pub fn for_each_archived_message<F>(
    root: &Path,
    start_id: u64,
    end_id: u64,
    mut callback: F,
) -> io::Result<PipelineStats>
where
    F: FnMut(&ParsedMessage) -> io::Result<()>,
{
    let metadata = Connection::open(root.join("metadata.sqlite")).map_err(sqlite_io)?;
    let mut statement = metadata
        .prepare("SELECT doc_id, segment, archive_offset, frame_bytes FROM messages WHERE doc_id >= ?1 AND doc_id < ?2 ORDER BY doc_id")
        .map_err(sqlite_io)?;
    let rows = statement
        .query_map(params![start_id as i64, end_id as i64], |row| {
            Ok((
                row.get::<_, i64>(0)? as u64,
                ArchiveLocation {
                    segment: row.get(1)?,
                    offset: row.get::<_, i64>(2)? as u64,
                    frame_bytes: row.get::<_, i64>(3)? as u64,
                },
            ))
        })
        .map_err(sqlite_io)?;
    let mut stats = PipelineStats::default();
    for row in rows {
        let (id, location) = row.map_err(sqlite_io)?;
        let read_started = Instant::now();
        let (record_id, raw) = read_record(&root.join("archive"), &location)?;
        stats.read_us += read_started.elapsed().as_micros();
        if record_id != id {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "catalog/archive id mismatch",
            ));
        }
        let parse_started = Instant::now();
        let message = parse_archived_message(id, &raw)?;
        stats.parse_us += parse_started.elapsed().as_micros();
        callback(&message)?;
        stats.messages += 1;
    }
    Ok(stats)
}

pub fn create_metadata(path: &Path) -> rusqlite::Result<Connection> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).ok();
    }
    let connection = Connection::open(path)?;
    connection.execute_batch("PRAGMA journal_mode=DELETE; PRAGMA synchronous=NORMAL; CREATE TABLE IF NOT EXISTS messages (doc_id INTEGER PRIMARY KEY, message_id TEXT NOT NULL UNIQUE, timestamp INTEGER NOT NULL, sender TEXT NOT NULL, recipients TEXT NOT NULL, subject TEXT NOT NULL, account TEXT NOT NULL, folder TEXT NOT NULL, thread TEXT NOT NULL, segment TEXT NOT NULL, archive_offset INTEGER NOT NULL, frame_bytes INTEGER NOT NULL); CREATE TABLE IF NOT EXISTS attachments (doc_id INTEGER NOT NULL, filename TEXT NOT NULL, mime TEXT NOT NULL, bytes INTEGER NOT NULL, content_hash TEXT NOT NULL, PRIMARY KEY(doc_id, filename)); CREATE INDEX IF NOT EXISTS messages_timestamp ON messages(timestamp); CREATE INDEX IF NOT EXISTS messages_sender ON messages(sender); CREATE INDEX IF NOT EXISTS messages_folder ON messages(folder); CREATE TABLE IF NOT EXISTS gmail_state (source_account TEXT PRIMARY KEY, history_id TEXT NOT NULL, complete INTEGER NOT NULL DEFAULT 0); CREATE TABLE IF NOT EXISTS gmail_messages (source_account TEXT NOT NULL, gmail_message_id TEXT NOT NULL, doc_id INTEGER NOT NULL UNIQUE, thread_id TEXT NOT NULL, label_ids TEXT NOT NULL, internal_date_ms INTEGER, message_history_id TEXT, source_state TEXT NOT NULL, first_seen_unix INTEGER NOT NULL, last_seen_unix INTEGER NOT NULL, PRIMARY KEY(source_account, gmail_message_id)); CREATE INDEX IF NOT EXISTS gmail_messages_state ON gmail_messages(source_account, source_state);")?;
    connection.execute_batch("CREATE TABLE IF NOT EXISTS imap_messages (source_account TEXT NOT NULL, mailbox TEXT NOT NULL, uid_validity INTEGER NOT NULL, uid INTEGER NOT NULL, doc_id INTEGER NOT NULL UNIQUE, flags TEXT NOT NULL, internal_date TEXT, internal_date_ms INTEGER, rfc822_size INTEGER, source_state TEXT NOT NULL, first_seen_unix INTEGER NOT NULL, last_seen_unix INTEGER NOT NULL, PRIMARY KEY(source_account, mailbox, uid_validity, uid)); CREATE INDEX IF NOT EXISTS imap_messages_state ON imap_messages(source_account, source_state); CREATE TABLE IF NOT EXISTS imap_scan_state (source_account TEXT NOT NULL, mailbox TEXT NOT NULL, uid_validity INTEGER NOT NULL, scanned_through_uid INTEGER NOT NULL, last_uid_next INTEGER NOT NULL, updated_unix INTEGER NOT NULL, PRIMARY KEY(source_account, mailbox, uid_validity)); CREATE TABLE IF NOT EXISTS imap_mailboxes (source_account TEXT NOT NULL, mailbox TEXT NOT NULL, delimiter TEXT, attributes TEXT NOT NULL, special_use TEXT NOT NULL, selectable INTEGER NOT NULL, last_seen_unix INTEGER NOT NULL, PRIMARY KEY(source_account, mailbox));")?;
    Ok(connection)
}

pub fn insert_metadata(
    connection: &Connection,
    message: &Message,
    location: &ArchiveLocation,
) -> rusqlite::Result<()> {
    connection.execute(
        "INSERT INTO messages VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)",
        params![
            message.id as i64,
            message.message_id,
            message.timestamp,
            message.sender,
            message.recipients.join(","),
            message.subject,
            message.account,
            message.folder,
            message.thread,
            location.segment,
            location.offset as i64,
            location.frame_bytes as i64
        ],
    )?;
    for attachment in &message.attachments {
        connection.execute(
            "INSERT INTO attachments VALUES (?1, ?2, ?3, ?4, ?5)",
            params![
                message.id as i64,
                attachment.filename,
                attachment.mime,
                attachment.bytes as i64,
                attachment.hash
            ],
        )?;
    }
    Ok(())
}

pub fn next_doc_id(connection: &Connection) -> rusqlite::Result<i64> {
    connection.query_row(
        "SELECT COALESCE(MAX(doc_id), -1) + 1 FROM messages",
        [],
        |row| row.get(0),
    )
}

pub fn gmail_message_exists(
    connection: &Connection,
    source_account: &str,
    gmail_id: &str,
) -> rusqlite::Result<bool> {
    connection.query_row(
        "SELECT EXISTS(SELECT 1 FROM gmail_messages WHERE source_account=?1 AND gmail_message_id=?2)",
        params![source_account, gmail_id],
        |row| row.get(0),
    )
}

#[allow(clippy::too_many_arguments)]
pub fn insert_gmail_metadata(
    connection: &Connection,
    source_account: &str,
    gmail_id: &str,
    doc_id: i64,
    thread_id: &str,
    label_ids_json: &str,
    internal_date_ms: Option<i64>,
    message_history_id: Option<&str>,
    location: &ArchiveLocation,
) -> rusqlite::Result<()> {
    connection.execute(
        "INSERT INTO messages(doc_id,message_id,timestamp,sender,recipients,subject,account,folder,thread,segment,archive_offset,frame_bytes) VALUES (?1,?2,0,'','','',?3,'','',?4,?5,?6)",
        params![doc_id, format!("gmail:{source_account}:{gmail_id}"), source_account, location.segment, location.offset as i64, location.frame_bytes as i64],
    )?;
    let now = chrono_like_now();
    connection.execute(
        "INSERT INTO gmail_messages(source_account,gmail_message_id,doc_id,thread_id,label_ids,internal_date_ms,message_history_id,source_state,first_seen_unix,last_seen_unix) VALUES (?1,?2,?3,?4,?5,?6,?7,'present',?8,?8)",
        params![source_account, gmail_id, doc_id, thread_id, label_ids_json, internal_date_ms, message_history_id, now],
    )?;
    Ok(())
}

pub fn imap_message_exists(
    connection: &Connection,
    source_account: &str,
    mailbox: &str,
    uid_validity: u32,
    uid: u32,
) -> rusqlite::Result<bool> {
    connection.query_row(
        "SELECT EXISTS(SELECT 1 FROM imap_messages WHERE source_account=?1 AND mailbox=?2 AND uid_validity=?3 AND uid=?4)",
        params![source_account, mailbox, uid_validity as i64, uid as i64],
        |row| row.get(0),
    )
}

pub fn imap_scan_state(
    connection: &Connection,
    source_account: &str,
    mailbox: &str,
) -> rusqlite::Result<Option<(u32, u32)>> {
    connection
        .query_row(
            "SELECT uid_validity, scanned_through_uid FROM imap_scan_state WHERE source_account=?1 AND mailbox=?2",
            params![source_account, mailbox],
            |row| Ok((row.get::<_, i64>(0)? as u32, row.get::<_, i64>(1)? as u32)),
        )
        .optional()
}

pub fn imap_known_uid_validity(
    connection: &Connection,
    source_account: &str,
    mailbox: &str,
) -> rusqlite::Result<Option<u32>> {
    let state = connection
        .query_row(
            "SELECT uid_validity FROM imap_scan_state WHERE source_account=?1 AND mailbox=?2 LIMIT 1",
            params![source_account, mailbox],
            |row| row.get::<_, i64>(0),
        )
        .optional()?;
    if state.is_some() {
        return Ok(state.map(|value| value as u32));
    }
    connection
        .query_row(
            "SELECT uid_validity FROM imap_messages WHERE source_account=?1 AND mailbox=?2 LIMIT 1",
            params![source_account, mailbox],
            |row| row.get::<_, i64>(0),
        )
        .optional()
        .map(|value| value.map(|value| value as u32))
}

pub fn upsert_imap_scan_state(
    connection: &Connection,
    source_account: &str,
    mailbox: &str,
    uid_validity: u32,
    scanned_through_uid: u32,
    last_uid_next: u32,
) -> rusqlite::Result<()> {
    connection.execute(
        "INSERT INTO imap_scan_state(source_account,mailbox,uid_validity,scanned_through_uid,last_uid_next,updated_unix) VALUES (?1,?2,?3,?4,?5,?6) ON CONFLICT(source_account,mailbox,uid_validity) DO UPDATE SET scanned_through_uid=excluded.scanned_through_uid,last_uid_next=excluded.last_uid_next,updated_unix=excluded.updated_unix",
        params![
            source_account,
            mailbox,
            uid_validity as i64,
            scanned_through_uid as i64,
            last_uid_next as i64,
            chrono_like_now()
        ],
    )?;
    Ok(())
}

pub fn upsert_imap_mailbox(
    connection: &Connection,
    source_account: &str,
    mailbox: &str,
    delimiter: Option<&str>,
    attributes_json: &str,
    special_use_json: &str,
    selectable: bool,
) -> rusqlite::Result<()> {
    connection.execute(
        "INSERT INTO imap_mailboxes(source_account,mailbox,delimiter,attributes,special_use,selectable,last_seen_unix) VALUES (?1,?2,?3,?4,?5,?6,?7) ON CONFLICT(source_account,mailbox) DO UPDATE SET delimiter=excluded.delimiter,attributes=excluded.attributes,special_use=excluded.special_use,selectable=excluded.selectable,last_seen_unix=excluded.last_seen_unix",
        params![
            source_account,
            mailbox,
            delimiter,
            attributes_json,
            special_use_json,
            selectable as i64,
            chrono_like_now()
        ],
    )?;
    Ok(())
}

#[allow(clippy::too_many_arguments)]
pub fn insert_imap_metadata(
    connection: &Connection,
    source_account: &str,
    mailbox: &str,
    uid_validity: u32,
    uid: u32,
    flags_json: &str,
    internal_date: Option<&str>,
    internal_date_ms: Option<i64>,
    rfc822_size: Option<u32>,
    doc_id: i64,
    location: &ArchiveLocation,
) -> rusqlite::Result<()> {
    let message_id = format!("imap:{source_account}:{mailbox}:{uid_validity}:{uid}");
    connection.execute(
        "INSERT INTO messages(doc_id,message_id,timestamp,sender,recipients,subject,account,folder,thread,segment,archive_offset,frame_bytes) VALUES (?1,?2,?3,'','','',?4,?5,'',?6,?7,?8)",
        params![
            doc_id,
            message_id,
            internal_date_ms.unwrap_or(0),
            source_account,
            mailbox,
            location.segment,
            location.offset as i64,
            location.frame_bytes as i64
        ],
    )?;
    connection.execute(
        "INSERT INTO imap_messages(source_account,mailbox,uid_validity,uid,doc_id,flags,internal_date,internal_date_ms,rfc822_size,source_state,first_seen_unix,last_seen_unix) VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,'present',?10,?10)",
        params![
            source_account,
            mailbox,
            uid_validity as i64,
            uid as i64,
            doc_id,
            flags_json,
            internal_date,
            internal_date_ms,
            rfc822_size.map(i64::from),
            chrono_like_now()
        ],
    )?;
    Ok(())
}

pub fn update_gmail_labels(
    connection: &Connection,
    source_account: &str,
    gmail_id: &str,
    labels_json: &str,
) -> rusqlite::Result<()> {
    connection.execute(
        "UPDATE gmail_messages SET label_ids=?3,last_seen_unix=?4,source_state='present' WHERE source_account=?1 AND gmail_message_id=?2",
        params![source_account, gmail_id, labels_json, chrono_like_now()],
    )?;
    Ok(())
}

pub fn repair_gmail_metadata(
    connection: &Connection,
    source_account: &str,
    gmail_id: &str,
    thread_id: &str,
    labels_json: &str,
    internal_date_ms: Option<i64>,
    message_history_id: Option<&str>,
) -> rusqlite::Result<()> {
    connection.execute(
        "UPDATE gmail_messages SET thread_id=?3,label_ids=?4,internal_date_ms=?5,message_history_id=?6,source_state='present',last_seen_unix=?7 WHERE source_account=?1 AND gmail_message_id=?2",
        params![source_account, gmail_id, thread_id, labels_json, internal_date_ms, message_history_id, chrono_like_now()],
    )?;
    Ok(())
}

pub fn mark_gmail_deleted(
    connection: &Connection,
    source_account: &str,
    gmail_id: &str,
) -> rusqlite::Result<()> {
    connection.execute(
        "UPDATE gmail_messages SET source_state='deleted',last_seen_unix=?3 WHERE source_account=?1 AND gmail_message_id=?2",
        params![source_account, gmail_id, chrono_like_now()],
    )?;
    Ok(())
}

pub fn mark_gmail_missing_from_full_sync(
    connection: &Connection,
    source_account: &str,
    seen_ids: &std::collections::HashSet<String>,
) -> rusqlite::Result<u64> {
    let mut statement = connection.prepare(
        "SELECT gmail_message_id FROM gmail_messages WHERE source_account=?1 AND source_state='present'",
    )?;
    let ids = statement
        .query_map(params![source_account], |row| row.get::<_, String>(0))?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    let mut changed = 0;
    for gmail_id in ids {
        if !seen_ids.contains(&gmail_id) {
            changed += connection.execute(
                "UPDATE gmail_messages SET source_state='deleted',last_seen_unix=?3 WHERE source_account=?1 AND gmail_message_id=?2 AND source_state='present'",
                params![source_account, gmail_id, chrono_like_now()],
            )? as u64;
        }
    }
    Ok(changed)
}

pub fn gmail_state(
    connection: &Connection,
    source_account: &str,
) -> rusqlite::Result<Option<(String, bool)>> {
    let mut statement = connection
        .prepare("SELECT history_id,complete FROM gmail_state WHERE source_account=?1")?;
    let mut rows = statement.query(params![source_account])?;
    rows.next()?
        .map(|row| Ok((row.get(0)?, row.get::<_, i64>(1)? != 0)))
        .transpose()
}

pub fn set_gmail_state(
    connection: &Connection,
    source_account: &str,
    history_id: &str,
    complete: bool,
) -> rusqlite::Result<()> {
    connection.execute(
        "INSERT INTO gmail_state(source_account,history_id,complete) VALUES (?1,?2,?3) ON CONFLICT(source_account) DO UPDATE SET history_id=excluded.history_id,complete=excluded.complete",
        params![source_account, history_id, complete as i64],
    )?;
    Ok(())
}

fn chrono_like_now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_secs() as i64)
        .unwrap_or(0)
}

pub fn create_sqlite_fts(path: &Path) -> rusqlite::Result<Connection> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).ok();
    }
    let connection = Connection::open(path)?;
    connection.execute_batch("PRAGMA journal_mode=DELETE; PRAGMA synchronous=NORMAL; CREATE VIRTUAL TABLE IF NOT EXISTS docs USING fts5(doc_id UNINDEXED, sender, recipients, subject, body, folder, account, tokenize='unicode61'); CREATE TABLE IF NOT EXISTS attrs (doc_id INTEGER PRIMARY KEY, timestamp INTEGER NOT NULL, sender TEXT NOT NULL, folder TEXT NOT NULL, account TEXT NOT NULL);")?;
    Ok(connection)
}

pub fn index_sqlite(connection: &mut Connection, config: CorpusConfig) -> rusqlite::Result<()> {
    index_sqlite_range(connection, config, 0, config.messages)
}

pub fn index_sqlite_range(
    connection: &mut Connection,
    config: CorpusConfig,
    start: u64,
    count: u64,
) -> rusqlite::Result<()> {
    let transaction = connection.transaction()?;
    {
        let mut statement =
            transaction.prepare("INSERT INTO docs VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)")?;
        for id in start..(start + count).min(config.messages) {
            let message = generate_message(config, id);
            transaction.execute(
                "INSERT INTO attrs VALUES (?1, ?2, ?3, ?4, ?5)",
                params![
                    message.id as i64,
                    message.timestamp,
                    message.sender,
                    message.folder,
                    message.account
                ],
            )?;
            statement.execute(params![
                message.id as i64,
                message.sender,
                message.recipients.join(" "),
                message.subject,
                format!(
                    "{} {}",
                    message.text_body,
                    message.html_body.unwrap_or_default()
                ),
                message.folder,
                message.account
            ])?;
        }
    }
    transaction.commit()
}

pub fn index_sqlite_archive(
    connection: &mut Connection,
    archive_root: &Path,
    start_id: u64,
    end_id: u64,
) -> io::Result<PipelineStats> {
    let transaction = connection.transaction().map_err(sqlite_io)?;
    let mut statement = transaction
        .prepare("INSERT INTO docs VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)")
        .map_err(sqlite_io)?;
    let mut stats = PipelineStats::default();
    let stream_stats = for_each_archived_message(archive_root, start_id, end_id, |message| {
        transaction
            .execute(
                "INSERT INTO attrs VALUES (?1, ?2, ?3, ?4, ?5)",
                params![
                    message.id as i64,
                    message.timestamp,
                    message.sender,
                    message.folder,
                    message.account
                ],
            )
            .map_err(sqlite_io)?;
        statement
            .execute(params![
                message.id as i64,
                message.sender,
                message.recipients,
                message.subject,
                message.body,
                message.folder,
                message.account
            ])
            .map_err(sqlite_io)?;
        Ok(())
    })?;
    stats.messages = stream_stats.messages;
    stats.read_us = stream_stats.read_us;
    stats.parse_us = stream_stats.parse_us;
    drop(statement);
    transaction.commit().map_err(sqlite_io)?;
    Ok(stats)
}

pub fn create_tantivy(path: &Path) -> tantivy::Result<(Index, TantivyFields)> {
    fs::create_dir_all(path)
        .map_err(|error| tantivy::TantivyError::SystemError(error.to_string()))?;
    let mut schema_builder = Schema::builder();
    let text_options = TextOptions::default()
        .set_indexing_options(
            TextFieldIndexing::default()
                .set_tokenizer("default")
                .set_index_option(IndexRecordOption::WithFreqsAndPositions),
        )
        .set_stored();
    let raw_options = TextOptions::default()
        .set_indexing_options(
            TextFieldIndexing::default()
                .set_tokenizer("raw")
                .set_index_option(IndexRecordOption::Basic),
        )
        .set_stored();
    let fields = TantivyFields {
        doc_id: schema_builder.add_u64_field("doc_id", INDEXED | STORED),
        timestamp: schema_builder.add_i64_field("timestamp", INDEXED | FAST | STORED),
        sender: schema_builder.add_text_field("sender", text_options.clone()),
        recipients: schema_builder.add_text_field("recipients", text_options.clone()),
        subject: schema_builder.add_text_field("subject", text_options.clone()),
        body: schema_builder.add_text_field("body", text_options.clone()),
        attachment_text: schema_builder.add_text_field("attachment_text", text_options.clone()),
        folder: schema_builder.add_text_field("folder", text_options.clone()),
        account: schema_builder.add_text_field("account", text_options.clone()),
        labels: schema_builder.add_text_field("label", text_options.clone()),
        attachment_types: schema_builder.add_text_field("attachment_type", text_options),
        attachment_count: schema_builder.add_u64_field("attachment_count", INDEXED | STORED),
        has_attachment: schema_builder.add_u64_field("has_attachment", INDEXED | FAST | STORED),
        attachment_mime: schema_builder.add_text_field("attachment_mime", raw_options.clone()),
        attachment_family: schema_builder.add_text_field("attachment_family", raw_options.clone()),
        labels_exact: schema_builder.add_text_field("label_exact", raw_options.clone()),
        sender_filter: schema_builder.add_text_field("sender_filter", raw_options.clone()),
        recipient_filter: schema_builder.add_text_field("recipient_filter", raw_options),
    };
    let schema = schema_builder.build();
    let index = Index::create_in_dir(path, schema)?;
    Ok((index, fields))
}

#[derive(Clone, Copy)]
pub struct TantivyFields {
    pub doc_id: Field,
    pub timestamp: Field,
    pub sender: Field,
    pub recipients: Field,
    pub subject: Field,
    pub body: Field,
    pub attachment_text: Field,
    pub folder: Field,
    pub account: Field,
    pub labels: Field,
    pub attachment_types: Field,
    pub attachment_count: Field,
    pub has_attachment: Field,
    pub attachment_mime: Field,
    pub attachment_family: Field,
    pub labels_exact: Field,
    pub sender_filter: Field,
    pub recipient_filter: Field,
}

pub fn index_tantivy(
    index: &Index,
    fields: TantivyFields,
    config: CorpusConfig,
) -> tantivy::Result<()> {
    index_tantivy_range(index, fields, config, 0, config.messages)
}

pub fn index_tantivy_range(
    index: &Index,
    fields: TantivyFields,
    config: CorpusConfig,
    start: u64,
    count: u64,
) -> tantivy::Result<()> {
    let mut writer: IndexWriter = index.writer(50_000_000)?;
    for id in start..(start + count).min(config.messages) {
        let message = generate_message(config, id);
        writer.add_document(doc!(fields.doc_id => message.id, fields.timestamp => message.timestamp, fields.sender => message.sender, fields.recipients => message.recipients.join(" "), fields.subject => message.subject, fields.body => format!("{} {}", message.text_body, message.html_body.unwrap_or_default()), fields.folder => message.folder, fields.account => message.account))?;
    }
    writer.commit()?;
    Ok(())
}

pub fn index_tantivy_archive(
    index: &Index,
    fields: TantivyFields,
    archive_root: &Path,
    start_id: u64,
    end_id: u64,
) -> io::Result<PipelineStats> {
    let mut writer = index.writer(50_000_000).map_err(io::Error::other)?;
    let stats = for_each_archived_message(archive_root, start_id, end_id, |message| {
        writer
            .add_document(doc!(
                fields.doc_id => message.id,
                fields.timestamp => message.timestamp,
                fields.sender => message.sender.clone(),
                fields.recipients => message.recipients.clone(),
                fields.subject => message.subject.clone(),
                fields.body => message.body.clone(),
                fields.folder => message.folder.clone(),
                fields.account => message.account.clone()
            ))
            .map_err(io::Error::other)?;
        Ok(())
    })?;
    writer.commit().map_err(io::Error::other)?;
    Ok(stats)
}

fn gmail_index_dir(root: &Path) -> PathBuf {
    root.join("derived").join("tantivy")
}

fn gmail_index_state_path(root: &Path) -> PathBuf {
    root.join("derived").join("tantivy-state.sqlite")
}

fn tantivy_fields_from_schema(schema: &Schema) -> tantivy::Result<TantivyFields> {
    let field = |name: &str| {
        schema
            .get_field(name)
            .map_err(|_| tantivy::TantivyError::SystemError(format!("missing field {name}")))
    };
    Ok(TantivyFields {
        doc_id: field("doc_id")?,
        timestamp: field("timestamp")?,
        sender: field("sender")?,
        recipients: field("recipients")?,
        subject: field("subject")?,
        body: field("body")?,
        attachment_text: field("attachment_text")?,
        folder: field("folder")?,
        account: field("account")?,
        labels: field("label")?,
        attachment_types: field("attachment_type")?,
        attachment_count: field("attachment_count")?,
        has_attachment: field("has_attachment")?,
        attachment_mime: field("attachment_mime")?,
        attachment_family: field("attachment_family")?,
        labels_exact: field("label_exact")?,
        sender_filter: field("sender_filter")?,
        recipient_filter: field("recipient_filter")?,
    })
}

fn open_or_create_gmail_tantivy(root: &Path) -> tantivy::Result<(Index, TantivyFields)> {
    let path = gmail_index_dir(root);
    fs::create_dir_all(&path)
        .map_err(|error| tantivy::TantivyError::SystemError(error.to_string()))?;
    if path.join("meta.json").exists() {
        let index = Index::open_in_dir(&path)?;
        match tantivy_fields_from_schema(&index.schema()) {
            Ok(fields) => Ok((index, fields)),
            Err(_) => {
                // Derived-only schema migration. The archive and catalogue
                // remain untouched; the normal indexer reconstructs this tree.
                drop(index);
                fs::remove_dir_all(&path)
                    .map_err(|error| tantivy::TantivyError::SystemError(error.to_string()))?;
                let state = gmail_index_state_path(root);
                if state.exists() {
                    let _ = fs::remove_file(state);
                }
                fs::create_dir_all(&path)
                    .map_err(|error| tantivy::TantivyError::SystemError(error.to_string()))?;
                create_tantivy(&path)
            }
        }
    } else {
        create_tantivy(&path)
    }
}

fn create_tantivy_state(root: &Path) -> rusqlite::Result<Connection> {
    let derived = root.join("derived");
    fs::create_dir_all(&derived).ok();
    let connection = Connection::open(gmail_index_state_path(root))?;
    connection.execute_batch(
        "PRAGMA journal_mode=DELETE;
         CREATE TABLE IF NOT EXISTS indexed_docs(
           doc_id INTEGER PRIMARY KEY,
           segment TEXT NOT NULL,
           archive_offset INTEGER NOT NULL,
           frame_bytes INTEGER NOT NULL,
           labels TEXT NOT NULL,
           source_state TEXT NOT NULL
         );",
    )?;
    Ok(connection)
}

#[derive(Clone, Debug)]
struct GmailCatalogRow {
    doc_id: u64,
    source_account: String,
    labels: String,
    source_state: String,
    timestamp: i64,
    location: ArchiveLocation,
}

fn for_each_gmail_catalog_row<F>(root: &Path, mut visit: F) -> io::Result<()>
where
    F: FnMut(GmailCatalogRow) -> io::Result<()>,
{
    let connection = create_metadata(&root.join("metadata.sqlite")).map_err(sqlite_io)?;
    let mut statement = connection
        .prepare(
            "SELECT doc_id,source_account,labels,source_state,timestamp,segment,archive_offset,frame_bytes
             FROM (
               SELECT g.doc_id AS doc_id,g.source_account AS source_account,g.label_ids AS labels,g.source_state AS source_state,
                      COALESCE(g.internal_date_ms,0) AS timestamp,
                      m.segment AS segment,m.archive_offset AS archive_offset,m.frame_bytes AS frame_bytes
               FROM gmail_messages g JOIN messages m ON m.doc_id=g.doc_id
               UNION ALL
               SELECT i.doc_id AS doc_id,i.source_account AS source_account,'[]' AS labels,i.source_state AS source_state,
                      COALESCE(i.internal_date_ms,0) AS timestamp,
                      m.segment AS segment,m.archive_offset AS archive_offset,m.frame_bytes AS frame_bytes
               FROM imap_messages i JOIN messages m ON m.doc_id=i.doc_id
             )
             ORDER BY doc_id",
        )
        .map_err(sqlite_io)?;
    let rows = statement
        .query_map([], |row| {
            Ok(GmailCatalogRow {
                doc_id: row.get::<_, i64>(0)? as u64,
                source_account: row.get(1)?,
                labels: row.get(2)?,
                source_state: row.get(3)?,
                timestamp: row.get(4)?,
                location: ArchiveLocation {
                    segment: row.get(5)?,
                    offset: row.get::<_, i64>(6)? as u64,
                    frame_bytes: row.get::<_, i64>(7)? as u64,
                },
            })
        })
        .map_err(sqlite_io)?;
    for row in rows {
        visit(row.map_err(sqlite_io)?)?;
    }
    Ok(())
}

fn labels_for_index(value: &str) -> Vec<String> {
    serde_json::from_str(value).unwrap_or_default()
}

fn search_filter_tokens(value: &str) -> impl Iterator<Item = String> + '_ {
    value
        .split(|character: char| !character.is_alphanumeric())
        .filter(|token| !token.is_empty())
        .map(|token| token.to_ascii_lowercase())
}

fn search_filter_values(value: &str) -> Vec<String> {
    let mut values: Vec<String> = search_filter_tokens(value).collect();
    if let (Some(start), Some(end)) = (value.find('<'), value.rfind('>')) {
        if start < end {
            values.push(value[start + 1..end].trim().to_ascii_lowercase());
        }
    }
    values.sort();
    values.dedup();
    values
}

pub fn index_gmail_archive(root: &Path) -> io::Result<GmailIndexStats> {
    index_gmail_archive_with_observer_and_config(root, |_| {}, GmailIndexWriterConfig::default())
}

/// Indexe l'archive et signale des phases de diagnostic au code expérimental.
/// Le callback n'influence pas le pipeline produit et peut rester un no-op.
pub fn index_gmail_archive_with_observer<F>(root: &Path, observe: F) -> io::Result<GmailIndexStats>
where
    F: FnMut(&str),
{
    index_gmail_archive_with_observer_and_config(root, observe, GmailIndexWriterConfig::default())
}

pub fn index_gmail_archive_with_observer_and_config<F>(
    root: &Path,
    mut observe: F,
    writer_config: GmailIndexWriterConfig,
) -> io::Result<GmailIndexStats>
where
    F: FnMut(&str),
{
    let open_started = Instant::now();
    let (index, fields) = open_or_create_gmail_tantivy(root).map_err(io::Error::other)?;
    let mut stats = GmailIndexStats {
        open_us: open_started.elapsed().as_micros(),
        ..Default::default()
    };
    let mut state = create_tantivy_state(root).map_err(sqlite_io)?;
    let mut known = HashMap::new();
    {
        let mut statement = state
            .prepare("SELECT doc_id,segment,archive_offset,frame_bytes,labels,source_state FROM indexed_docs")
            .map_err(sqlite_io)?;
        let rows = statement
            .query_map([], |row| {
                Ok((
                    row.get::<_, i64>(0)? as u64,
                    row.get::<_, String>(1)?,
                    row.get::<_, i64>(2)? as u64,
                    row.get::<_, i64>(3)? as u64,
                    row.get::<_, String>(4)?,
                    row.get::<_, String>(5)?,
                ))
            })
            .map_err(sqlite_io)?;
        for row in rows {
            let row = row.map_err(sqlite_io)?;
            known.insert(row.0, row);
        }
    }
    observe("catalog_stream_opened");
    let track_current = !known.is_empty();
    let mut current = HashSet::with_capacity(if track_current { known.len() } else { 0 });
    let mut writer = if let Some(worker_threads) = writer_config.worker_threads {
        let memory_budget_per_thread = writer_config
            .memory_budget_bytes
            .checked_div(worker_threads.max(1))
            .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "invalid writer threads"))?;
        let options = IndexWriterOptions::builder()
            .memory_budget_per_thread(memory_budget_per_thread)
            .num_worker_threads(worker_threads)
            .num_merge_threads(writer_config.merge_threads)
            .build();
        index
            .writer_with_options(options)
            .map_err(io::Error::other)?
    } else {
        index
            .writer(writer_config.memory_budget_bytes)
            .map_err(io::Error::other)?
    };
    if writer_config.no_merge_policy {
        writer.set_merge_policy(Box::new(NoMergePolicy));
    }
    observe("writer_opened");
    let mut changed = false;
    let state_transaction = state.transaction().map_err(sqlite_io)?;
    let index_started = Instant::now();
    for_each_gmail_catalog_row(root, |row| {
        if track_current {
            current.insert(row.doc_id);
        }
        stats.examined += 1;
        let fingerprint = (
            row.location.segment.as_str(),
            row.location.offset,
            row.location.frame_bytes,
            row.labels.as_str(),
            row.source_state.as_str(),
        );
        let unchanged = known.get(&row.doc_id).is_some_and(|old| {
            (old.1.as_str(), old.2, old.3, old.4.as_str(), old.5.as_str()) == fingerprint
        });
        if unchanged {
            stats.skipped += 1;
            return Ok(());
        }
        writer.delete_term(Term::from_field_u64(fields.doc_id, row.doc_id));
        if row.source_state != "present" {
            state_transaction
                .execute(
                    "DELETE FROM indexed_docs WHERE doc_id=?1",
                    [row.doc_id as i64],
                )
                .map_err(sqlite_io)?;
            stats.removed += 1;
            changed = true;
            return Ok(());
        }
        let read_started = Instant::now();
        let (record_id, raw) = read_record(&root.join("archive"), &row.location)?;
        stats.read_us += read_started.elapsed().as_micros();
        if record_id != row.doc_id {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "catalog/archive id mismatch",
            ));
        }
        let parse_started = Instant::now();
        let parsed = match parse_gmail_message(&raw, labels_for_index(&row.labels)) {
            Ok(parsed) => parsed,
            Err(_) => {
                stats.parse_failures += 1;
                return Ok(());
            }
        };
        stats.parse_us += parse_started.elapsed().as_micros();
        stats.attachment_encountered += parsed.attachment_text_stats.encountered;
        stats.attachment_supported += parsed.attachment_text_stats.supported;
        stats.attachment_extracted += parsed.attachment_text_stats.extracted;
        stats.attachment_unsupported += parsed.attachment_text_stats.unsupported;
        stats.attachment_extraction_failures += parsed.attachment_text_stats.failures;
        stats.attachment_decoded_bytes += parsed.attachment_text_stats.decoded_bytes;
        stats.attachment_extracted_bytes += parsed.attachment_text_stats.extracted_bytes;
        stats.attachment_extracted_chars += parsed.attachment_text_stats.extracted_chars;
        let mut document = doc!(
            fields.doc_id => row.doc_id,
            fields.timestamp => row.timestamp,
            fields.sender => parsed.sender,
            fields.recipients => parsed.recipients,
            fields.subject => parsed.subject,
            fields.body => parsed.body,
            fields.attachment_text => parsed.attachment_text,
            fields.account => row.source_account,
            fields.labels => parsed.labels.join(" "),
            fields.attachment_types => parsed.attachment_types.join(" "),
            fields.attachment_count => parsed.attachment_count,
            fields.has_attachment => u64::from(parsed.attachment_count > 0),
        );
        for token in search_filter_values(&parsed.sender) {
            document.add_text(fields.sender_filter, token);
        }
        for token in search_filter_values(&parsed.recipients) {
            document.add_text(fields.recipient_filter, token);
        }
        for label in &parsed.labels {
            document.add_text(fields.labels_exact, label);
        }
        for mime in &parsed.attachment_types {
            document.add_text(fields.attachment_mime, mime);
            if let Some(family) = mime.split('/').next() {
                document.add_text(fields.attachment_family, family);
            }
        }
        writer.add_document(document).map_err(io::Error::other)?;
        state_transaction
            .execute(
                "INSERT OR REPLACE INTO indexed_docs VALUES (?1,?2,?3,?4,?5,?6)",
                params![
                    row.doc_id as i64,
                    row.location.segment,
                    row.location.offset as i64,
                    row.location.frame_bytes as i64,
                    row.labels,
                    row.source_state
                ],
            )
            .map_err(sqlite_io)?;
        stats.indexed += 1;
        changed = true;
        if stats.examined.is_multiple_of(100_000) {
            observe("indexing_100k_boundary");
        }
        Ok(())
    })?;
    if track_current {
        for doc_id in known.keys() {
            if !current.contains(doc_id) {
                writer.delete_term(Term::from_field_u64(fields.doc_id, *doc_id));
                state_transaction
                    .execute("DELETE FROM indexed_docs WHERE doc_id=?1", [*doc_id as i64])
                    .map_err(sqlite_io)?;
                stats.removed += 1;
                changed = true;
            }
        }
    }
    if changed {
        observe("before_commit");
        stats.segments_before_commit = index
            .load_metas()
            .map(|meta| meta.segments.len() as u64)
            .unwrap_or(0);
        writer.commit().map_err(io::Error::other)?;
        observe("after_commit");
        stats.segments_after_commit = index
            .load_metas()
            .map(|meta| meta.segments.len() as u64)
            .unwrap_or(0);
    }
    state_transaction.commit().map_err(sqlite_io)?;
    drop(writer);
    stats.segments_after_index = index
        .load_metas()
        .map(|meta| meta.segments.len() as u64)
        .unwrap_or(0);
    stats.index_us = index_started.elapsed().as_micros();
    stats.index_bytes = directory_bytes(gmail_index_dir(root))?;
    Ok(stats)
}

fn parse_date_token(value: &str) -> Option<i64> {
    let mut parts = value.split('-');
    let year: i64 = parts.next()?.parse().ok()?;
    let month: i64 = parts.next()?.parse().ok()?;
    let day: i64 = parts.next()?.parse().ok()?;
    if parts.next().is_some() || !(1..=12).contains(&month) || !(1..=31).contains(&day) {
        return None;
    }
    let adjusted_year = year - i64::from(month <= 2);
    let era = if adjusted_year >= 0 {
        adjusted_year
    } else {
        adjusted_year - 399
    } / 400;
    let year_of_era = adjusted_year - era * 400;
    let month_prime = month + if month > 2 { -3 } else { 9 };
    let day_of_year = (153 * month_prime + 2) / 5 + day - 1;
    let day_of_era = year_of_era * 365 + year_of_era / 4 - year_of_era / 100 + day_of_year;
    Some((era * 146097 + day_of_era - 719468) * 86_400)
}

pub fn parse_search_date_ms(value: &str) -> Option<i64> {
    parse_date_token(value.trim()).map(|seconds| seconds * 1000)
}

fn legacy_request_parts(
    query: &str,
) -> (
    String,
    Option<String>,
    Option<String>,
    Option<i64>,
    Option<i64>,
) {
    let mut text = Vec::new();
    let mut from = None;
    let mut to = None;
    let mut date_from = None;
    let mut date_to = None;
    for token in query.split_whitespace() {
        if let Some(value) = token.strip_prefix("after:") {
            date_from = parse_date_token(value).map(|seconds| seconds * 1000);
        } else if let Some(value) = token.strip_prefix("before:") {
            date_to = parse_date_token(value).map(|seconds| seconds * 1000);
        } else if let Some(value) = token.strip_prefix("from:") {
            from = Some(value.to_string());
        } else if let Some(value) = token.strip_prefix("to:") {
            to = Some(value.to_string());
        } else {
            text.push(token.to_string());
        }
    }
    (text.join(" "), from, to, date_from, date_to)
}

fn field_filter_query(
    _index: &Index,
    field: Field,
    value: &str,
) -> tantivy::Result<Option<Box<dyn Query>>> {
    let normalized = value
        .trim()
        .trim_matches('<')
        .trim_matches('>')
        .to_ascii_lowercase();
    if normalized.contains('@') {
        return Ok(Some(exact_term(field, &normalized)));
    }
    let queries = value
        .split(|character: char| !character.is_alphanumeric())
        .filter(|token| !token.is_empty())
        .map(|token| exact_term(field, &token.to_ascii_lowercase()))
        .collect::<Vec<_>>();
    Ok(match queries.len() {
        0 => None,
        1 => Some(queries.into_iter().next().unwrap()),
        _ => Some(Box::new(BooleanQuery::intersection(queries))),
    })
}

fn exact_term(field: Field, value: &str) -> Box<dyn Query> {
    Box::new(TermQuery::new(
        Term::from_field_text(field, value),
        IndexRecordOption::Basic,
    ))
}

fn exact_u64_term(field: Field, value: u64) -> Box<dyn Query> {
    Box::new(TermQuery::new(
        Term::from_field_u64(field, value),
        IndexRecordOption::Basic,
    ))
}

fn structured_query(
    index: &Index,
    fields: TantivyFields,
    request: &SearchRequest,
) -> tantivy::Result<Box<dyn Query>> {
    let mut queries = Vec::<Box<dyn Query>>::new();
    if !request.text.trim().is_empty() {
        let mut parser = QueryParser::for_index(
            index,
            vec![
                fields.sender,
                fields.recipients,
                fields.subject,
                fields.body,
                fields.account,
                fields.labels,
                fields.attachment_types,
                fields.attachment_text,
            ],
        );
        // Attachment text is useful evidence, but a long document must not
        // outrank a concise match in the message itself by accident.
        parser.set_field_boost(fields.attachment_text, 0.7);
        queries.push(parser.parse_query(&request.text)?);
    }
    if let Some(value) = request.from.as_deref() {
        if let Some(query) = field_filter_query(index, fields.sender_filter, value)? {
            queries.push(query);
        }
    }
    if let Some(value) = request.to.as_deref() {
        if let Some(query) = field_filter_query(index, fields.recipient_filter, value)? {
            queries.push(query);
        }
    }
    if request.date_from.is_some() || request.date_to.is_some() {
        queries.push(Box::new(RangeQuery::new(
            request.date_from.map_or(Bound::Unbounded, |value| {
                Bound::Included(Term::from_field_i64(fields.timestamp, value))
            }),
            request.date_to.map_or(Bound::Unbounded, |value| {
                Bound::Excluded(Term::from_field_i64(fields.timestamp, value))
            }),
        )));
    }
    match request.attachment {
        AttachmentFilter::All => {}
        AttachmentFilter::With => queries.push(exact_u64_term(fields.has_attachment, 1)),
        AttachmentFilter::Without => queries.push(exact_u64_term(fields.has_attachment, 0)),
    }
    if let Some(value) = request.attachment_mime.as_deref() {
        let value = value.trim().to_ascii_lowercase();
        if let Some(family) = value.strip_suffix("/*") {
            queries.push(exact_term(fields.attachment_family, family));
        } else if !value.is_empty() {
            queries.push(exact_term(fields.attachment_mime, &value));
        }
    }
    for label in &request.labels {
        if !label.trim().is_empty() {
            queries.push(exact_term(fields.labels_exact, label.trim()));
        }
    }
    Ok(match queries.len() {
        0 => Box::new(AllQuery),
        1 => queries.pop().unwrap(),
        _ => Box::new(BooleanQuery::intersection(queries)),
    })
}

pub fn search_gmail_archive(
    root: &Path,
    query: &str,
    limit: usize,
) -> io::Result<Vec<GmailSearchResult>> {
    GmailSearchIndex::open(root)?.search(query, limit)
}

pub fn read_archived_raw(root: &Path, doc_id: u64) -> io::Result<Vec<u8>> {
    let catalog = Connection::open(root.join("metadata.sqlite")).map_err(sqlite_io)?;
    let catalog_id = i64::try_from(doc_id).map_err(|_| {
        io::Error::new(
            io::ErrorKind::InvalidInput,
            "archive record id does not fit in the catalog",
        )
    })?;
    let (segment, offset, frame_bytes) = catalog
        .query_row(
            "SELECT segment,archive_offset,frame_bytes FROM messages WHERE doc_id=?1",
            [catalog_id],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, i64>(1)?,
                    row.get::<_, i64>(2)?,
                ))
            },
        )
        .map_err(sqlite_io)?;
    let location = ArchiveLocation {
        segment,
        offset: u64::try_from(offset)
            .map_err(|_| io::Error::new(io::ErrorKind::InvalidData, "negative archive offset"))?,
        frame_bytes: u64::try_from(frame_bytes).map_err(|_| {
            io::Error::new(io::ErrorKind::InvalidData, "negative archive frame length")
        })?,
    };
    let (record_id, raw) = read_record(&root.join("archive"), &location)?;
    if record_id != doc_id {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "catalog/archive id mismatch",
        ));
    }
    Ok(raw)
}

pub fn export_message_eml(root: &Path, doc_id: u64, destination: &Path) -> io::Result<()> {
    let raw = read_archived_raw(root, doc_id)?;
    fs::write(destination, raw)
}

pub fn sqlite_search(connection: &Connection, query: &str) -> rusqlite::Result<Vec<SearchHit>> {
    let mut statement = connection.prepare(
        "SELECT doc_id, bm25(docs) FROM docs WHERE docs MATCH ?1 ORDER BY rank LIMIT 20",
    )?;
    let rows = statement.query_map([query], |row| {
        Ok(SearchHit {
            doc_id: row.get::<_, i64>(0)? as u64,
            score: row.get::<_, f64>(1)? as f32,
        })
    })?;
    rows.collect()
}

pub fn tantivy_search(
    index: &Index,
    reader: &IndexReader,
    fields: TantivyFields,
    query: &str,
) -> tantivy::Result<Vec<SearchHit>> {
    let searcher = reader.searcher();
    let parser = QueryParser::for_index(
        index,
        vec![
            fields.sender,
            fields.recipients,
            fields.subject,
            fields.body,
            fields.folder,
            fields.account,
        ],
    );
    let parsed = parser.parse_query(query)?;
    let top_docs = searcher.search(&parsed, &TopDocs::with_limit(20).order_by_score())?;
    top_docs
        .into_iter()
        .map(|(score, address)| {
            let doc: TantivyDocument = searcher.doc(address)?;
            let id = doc
                .get_first(fields.doc_id)
                .and_then(|value| value.as_u64())
                .unwrap_or_default();
            Ok(SearchHit { doc_id: id, score })
        })
        .collect()
}

pub fn sqlite_date_count(connection: &Connection, start: i64, end: i64) -> rusqlite::Result<u64> {
    connection.query_row(
        "SELECT COUNT(*) FROM attrs WHERE timestamp BETWEEN ?1 AND ?2",
        params![start, end],
        |row| row.get::<_, i64>(0).map(|value| value as u64),
    )
}

pub fn sqlite_text_date_search(
    connection: &Connection,
    text: &str,
    start: i64,
    end: i64,
) -> rusqlite::Result<Vec<SearchHit>> {
    let mut statement = connection.prepare(
        "SELECT docs.doc_id, bm25(docs) FROM docs JOIN attrs ON attrs.doc_id = docs.doc_id WHERE docs MATCH ?1 AND attrs.timestamp BETWEEN ?2 AND ?3 ORDER BY rank LIMIT 20",
    )?;
    let rows = statement.query_map(params![text, start, end], |row| {
        Ok(SearchHit {
            doc_id: row.get::<_, i64>(0)? as u64,
            score: row.get::<_, f64>(1)? as f32,
        })
    })?;
    rows.collect()
}

pub fn tantivy_date_search(
    reader: &IndexReader,
    fields: TantivyFields,
    start: i64,
    end: i64,
) -> tantivy::Result<Vec<SearchHit>> {
    let searcher = reader.searcher();
    let query = RangeQuery::new(
        Bound::Included(Term::from_field_i64(fields.timestamp, start)),
        Bound::Included(Term::from_field_i64(fields.timestamp, end)),
    );
    let top_docs = searcher.search(&query, &TopDocs::with_limit(20).order_by_score())?;
    top_docs
        .into_iter()
        .map(|(score, address)| {
            let doc: TantivyDocument = searcher.doc(address)?;
            let id = doc
                .get_first(fields.doc_id)
                .and_then(|value| value.as_u64())
                .unwrap_or_default();
            Ok(SearchHit { doc_id: id, score })
        })
        .collect()
}

pub fn tantivy_text_date_search(
    index: &Index,
    reader: &IndexReader,
    fields: TantivyFields,
    text: &str,
    start: i64,
    end: i64,
) -> tantivy::Result<Vec<SearchHit>> {
    let parser = QueryParser::for_index(
        index,
        vec![
            fields.sender,
            fields.recipients,
            fields.subject,
            fields.body,
            fields.folder,
            fields.account,
        ],
    );
    let text_query = parser.parse_query(text)?;
    let date_query = RangeQuery::new(
        Bound::Included(Term::from_field_i64(fields.timestamp, start)),
        Bound::Included(Term::from_field_i64(fields.timestamp, end)),
    );
    let query = BooleanQuery::intersection(vec![text_query, Box::new(date_query)]);
    let searcher = reader.searcher();
    let top_docs = searcher.search(&query, &TopDocs::with_limit(20).order_by_score())?;
    top_docs
        .into_iter()
        .map(|(score, address)| {
            let doc: TantivyDocument = searcher.doc(address)?;
            let id = doc
                .get_first(fields.doc_id)
                .and_then(|value| value.as_u64())
                .unwrap_or_default();
            Ok(SearchHit { doc_id: id, score })
        })
        .collect()
}

pub fn latency_stats(mut values: Vec<Duration>) -> LatencyStats {
    values.sort_unstable();
    let micros = |duration: Duration| duration.as_micros();
    let percentile = |fraction: f64| -> u128 {
        values[((values.len().saturating_sub(1)) as f64 * fraction).round() as usize].as_micros()
    };
    LatencyStats {
        count: values.len(),
        p50_us: percentile(0.50),
        p95_us: percentile(0.95),
        p99_us: percentile(0.99),
        max_us: values.last().map(|value| micros(*value)).unwrap_or(0),
    }
}

pub fn run_queries<F>(queries: &[String], mut search: F) -> LatencyStats
where
    F: FnMut(&str),
{
    let mut durations = Vec::with_capacity(queries.len());
    for query in queries {
        let started = Instant::now();
        search(query);
        durations.push(started.elapsed());
    }
    latency_stats(durations)
}

pub fn benchmark_queries() -> Vec<String> {
    [
        "meeting",
        "quartz",
        "meeting archive",
        "\"project archive\"",
        "subject:invoice",
        "sender:alice",
        "recipients:alice",
        "account:account-1",
        "Übertragung",
        "稀少",
        "folder:Inbox",
        "release schedule",
        "archive",
    ]
    .into_iter()
    .map(String::from)
    .collect()
}

pub fn benchmark_workloads() -> Vec<(&'static str, Vec<String>)> {
    vec![
        ("rare", vec!["quartz".into()]),
        ("frequent", vec!["archive".into()]),
        ("multi_term", vec!["meeting archive".into()]),
        ("exact_phrase", vec!["\"project archive\"".into()]),
        ("sender", vec!["sender:alice".into()]),
        ("recipient", vec!["recipients:alice".into()]),
        ("text_sender", vec!["alice meeting".into()]),
        ("no_result", vec!["zzzz-never-generated".into()]),
        ("many_candidates", vec!["folder:Inbox".into()]),
    ]
}

pub fn build_archive(
    root: &Path,
    config: CorpusConfig,
    segment_bytes: u64,
) -> io::Result<(DatasetStats, u64)> {
    fs::create_dir_all(root)?;
    let archive_root = root.join("archive");
    let metadata_path = root.join("metadata.sqlite");
    let mut writer = ArchiveWriter::open(&archive_root, segment_bytes)?;
    let mut metadata = create_metadata(&metadata_path).map_err(sqlite_io)?;
    let transaction = metadata.transaction().map_err(sqlite_io)?;
    let mut sizes = Vec::with_capacity(config.messages.min(1_000_000) as usize);
    let mut bytes = 0u64;
    let mut attachments = 0u64;
    let mut hashes = std::collections::HashSet::new();
    let mut attachment_bytes = 0u64;
    let mut unique_attachment_sizes = std::collections::HashMap::new();
    let mut attachment_counts = std::collections::HashMap::new();
    let mut compressed_bytes = 0u64;
    let mut zstd_bytes = 0u64;
    let mut text_compressed_bytes = 0u64;
    let mut attachment_compressed_bytes = 0u64;
    let mut text_zstd_bytes = 0u64;
    let mut attachment_zstd_bytes = 0u64;
    let mut mime_text_bytes = 0u64;
    for id in 0..config.messages {
        let message = generate_message(config, id);
        let location = writer.append(&message)?;
        insert_metadata(&transaction, &message, &location).map_err(sqlite_io)?;
        bytes += message.raw.len() as u64;
        sizes.push(message.raw.len());
        attachments += message.attachments.len() as u64;
        let message_attachment_bytes: u64 = message
            .attachments
            .iter()
            .map(|attachment| attachment.bytes as u64)
            .sum();
        mime_text_bytes += message.raw.len() as u64 - message_attachment_bytes;
        for attachment in &message.attachments {
            hashes.insert(attachment.hash.clone());
            attachment_bytes += attachment.bytes as u64;
            *attachment_counts
                .entry(attachment.hash.clone())
                .or_insert(0) += 1;
            unique_attachment_sizes
                .entry(attachment.hash.clone())
                .or_insert(attachment.bytes as u64);
        }
        if config.measure_compression {
            let (text_gzip, attachment_gzip, text_zstd, attachment_zstd) =
                compressed_parts(&message.raw);
            text_compressed_bytes += text_gzip;
            attachment_compressed_bytes += attachment_gzip;
            text_zstd_bytes += text_zstd;
            attachment_zstd_bytes += attachment_zstd;
            compressed_bytes += text_gzip + attachment_gzip;
            zstd_bytes += text_zstd + attachment_zstd;
        }
    }
    transaction.commit().map_err(sqlite_io)?;
    writer.sync()?;
    sizes.sort_unstable();
    let duplicate_sizes = duplicate_sizes(&attachment_counts, &unique_attachment_sizes);
    let unique_attachment_bytes: u64 = unique_attachment_sizes.values().sum();
    let stats = DatasetStats {
        messages: config.messages,
        bytes,
        min_bytes: sizes.first().copied().unwrap_or(0),
        p90_bytes: percentile_size(&sizes, 0.90),
        p99_bytes: percentile_size(&sizes, 0.99),
        max_bytes: sizes.last().copied().unwrap_or(0),
        median_bytes: sizes.get(sizes.len() / 2).copied().unwrap_or(0),
        mean_bytes: bytes / config.messages.max(1),
        mime_text_bytes,
        compressed_bytes,
        zstd_bytes,
        text_compressed_bytes,
        attachment_compressed_bytes,
        text_zstd_bytes,
        attachment_zstd_bytes,
        attachments,
        unique_attachment_hashes: hashes.len(),
        attachment_bytes,
        unique_attachment_bytes,
        duplicate_attachment_objects: attachment_counts
            .values()
            .map(|count| count.saturating_sub(1))
            .sum(),
        duplicate_attachment_bytes: attachment_bytes - unique_attachment_bytes,
        duplicate_size_p50: percentile_size(&duplicate_sizes, 0.50),
        duplicate_size_p90: percentile_size(&duplicate_sizes, 0.90),
        duplicate_size_max: duplicate_sizes.last().copied().unwrap_or(0),
    };
    let archive_bytes = directory_bytes(root.join("archive"))?;
    Ok((stats, archive_bytes))
}

fn sqlite_io(error: rusqlite::Error) -> io::Error {
    io::Error::other(error)
}

pub fn directory_bytes(root: impl AsRef<Path>) -> io::Result<u64> {
    let mut total = 0;
    if !root.as_ref().exists() {
        return Ok(0);
    }
    for entry in fs::read_dir(root)? {
        let path = entry?.path();
        let metadata = fs::metadata(&path)?;
        total += if metadata.is_dir() {
            directory_bytes(path)?
        } else {
            metadata.len()
        };
    }
    Ok(total)
}

pub fn write_manifest(path: &Path, config: CorpusConfig, stats: &DatasetStats) -> io::Result<()> {
    let mut file = File::create(path)?;
    writeln!(file, "seed={}\nprofile={}\nmessages={}\nattachment_rate={}\nduplicate_rate={}\nmax_attachment_bytes={}\nbytes={}\nmean_bytes={}\nmin_bytes={}\nmedian_bytes={}\np90_bytes={}\np99_bytes={}\nmax_bytes={}\nmime_text_bytes={}\ncompressed_bytes={}\nzstd_bytes={}\ntext_compressed_bytes={}\nattachment_compressed_bytes={}\ntext_zstd_bytes={}\nattachment_zstd_bytes={}\nattachments={}\nunique_attachment_hashes={}\nattachment_bytes={}\nunique_attachment_bytes={}\nduplicate_attachment_objects={}\nduplicate_attachment_bytes={}", config.seed, config.profile.name(), stats.messages, config.attachment_rate, config.duplicate_rate, config.max_attachment_bytes, stats.bytes, stats.mean_bytes, stats.min_bytes, stats.median_bytes, stats.p90_bytes, stats.p99_bytes, stats.max_bytes, stats.mime_text_bytes, stats.compressed_bytes, stats.zstd_bytes, stats.text_compressed_bytes, stats.attachment_compressed_bytes, stats.text_zstd_bytes, stats.attachment_zstd_bytes, stats.attachments, stats.unique_attachment_hashes, stats.attachment_bytes, stats.unique_attachment_bytes, stats.duplicate_attachment_objects, stats.duplicate_attachment_bytes)
}

pub fn map_to_lines(values: &BTreeMap<&str, String>) -> String {
    values
        .iter()
        .map(|(key, value)| format!("{key}={value}"))
        .collect::<Vec<_>>()
        .join("\n")
}

#[allow(dead_code)]
fn _ordering_is_total(a: f32, b: f32) -> Ordering {
    a.partial_cmp(&b).unwrap_or(Ordering::Equal)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn raw_read_test_root(label: &str) -> PathBuf {
        let nonce = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        std::env::temp_dir().join(format!(
            "atlas-raw-read-{label}-{}-{nonce}",
            std::process::id()
        ))
    }

    #[test]
    fn corpus_is_deterministic_from_seed_and_id() {
        let config = CorpusConfig {
            messages: 10,
            seed: 7,
            profile: CorpusProfile::Personal,
            attachment_rate: 100,
            duplicate_rate: 50,
            max_attachment_bytes: 4096,
            measure_compression: false,
        };
        let left = generate_message(config, 3);
        let right = generate_message(config, 3);
        assert_eq!(left.message_id, right.message_id);
        assert_eq!(left.raw, right.raw);
        assert_eq!(left.attachments[0].hash, right.attachments[0].hash);
    }

    #[test]
    fn gmail_mime_parser_extracts_text_and_attachment_metadata_without_html_noise() {
        let raw = b"From: sender@example.test\r\nTo: recipient@example.test\r\nSubject: Test\r\nContent-Type: multipart/mixed; boundary=outer\r\n\r\n--outer\r\nContent-Type: text/html\r\n\r\n<p>Hello <b>world</b></p><script>secret()</script>\r\n--outer\r\nContent-Type: application/pdf\r\nContent-Disposition: attachment; filename=report.pdf\r\nContent-Transfer-Encoding: base64\r\n\r\nSGVsbG8=\r\n--outer--\r\n";
        let parsed = parse_gmail_message(raw, vec!["INBOX".into()]).unwrap();
        assert!(parsed.body.contains("Hello world"));
        assert!(!parsed.body.contains("secret"));
        assert_eq!(parsed.sender, "sender@example.test");
        assert_eq!(parsed.recipients, "recipient@example.test");
        assert_eq!(parsed.attachment_count, 1);
        assert_eq!(parsed.attachment_types, vec!["application/pdf"]);
        assert_eq!(parsed.labels, vec!["INBOX"]);
    }

    #[test]
    fn attachment_api_lists_downloadables_and_cid_resources_without_changing_raw() {
        let root =
            std::env::temp_dir().join(format!("mail-attachment-test-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(root.join("archive")).unwrap();
        create_metadata(&root.join("metadata.sqlite")).unwrap();
        let raw = b"From: test@example.test\r\nSubject: attachments\r\nContent-Type: multipart/mixed; boundary=outer\r\n\r\n--outer\r\nContent-Type: text/plain\r\n\r\nHello\r\n--outer\r\nContent-Type: application/pdf\r\nContent-Disposition: attachment; filename=report.pdf\r\nContent-Transfer-Encoding: base64\r\n\r\nJVBERi0xLjQ=\r\n--outer\r\nContent-Type: image/png\r\nContent-ID: <logo@example.test>\r\nContent-Disposition: inline\r\nContent-Transfer-Encoding: base64\r\n\r\niVBORw==\r\n--outer\r\nContent-Type: application/octet-stream\r\nContent-Disposition: attachment\r\nContent-Transfer-Encoding: base64\r\n\r\nAQID\r\n--outer\r\nContent-Type: text/plain; name*=UTF-8''r%C3%A9sum%C3%A9.txt\r\nContent-Disposition: attachment; filename*=UTF-8''r%C3%A9sum%C3%A9.txt\r\nContent-Transfer-Encoding: quoted-printable\r\n\r\ncaf=C3=A9\r\n--outer\r\nContent-Type: text/plain\r\nContent-Disposition: attachment; filename=../evil.txt\r\n\r\nblocked-name\r\n--outer--\r\n";
        let mut writer = ArchiveWriter::open(&root.join("archive"), 4096).unwrap();
        let message = Message {
            id: 7,
            message_id: "fixture-attachment".into(),
            timestamp: 0,
            sender: "test@example.test".into(),
            recipients: vec!["recipient@example.test".into()],
            subject: "attachments".into(),
            text_body: "Hello".into(),
            html_body: None,
            account: "fixture".into(),
            folder: "Inbox".into(),
            thread: "thread".into(),
            attachments: Vec::new(),
            raw: raw.to_vec(),
        };
        let location = writer.append(&message).unwrap();
        writer.sync().unwrap();
        let connection = create_metadata(&root.join("metadata.sqlite")).unwrap();
        insert_metadata(&connection, &message, &location).unwrap();
        let attachments = list_attachments(&root, 7).unwrap();
        assert_eq!(attachments.len(), 4);
        assert_eq!(attachments[0].mime, "application/pdf");
        assert_eq!(attachments[0].decoded_bytes, 8);
        assert!(attachments
            .iter()
            .any(|item| item.filename.as_deref() == Some("résumé.txt")));
        assert_eq!(
            read_attachment(&root, 7, attachments[0].id).unwrap(),
            b"%PDF-1.4"
        );
        let resources = list_mime_resources(&root, 7).unwrap();
        assert_eq!(resources.len(), 1);
        assert_eq!(resources[0].content_id, "logo@example.test");
        assert_eq!(resources[0].mime, "image/png");
        assert_eq!(read_archived_raw(&root, 7).unwrap(), raw);
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn html_display_hides_raw_urls_without_changing_plain_text() {
        assert_eq!(
            html_text("<p>Read https://example.test/path and www.example.test.</p>"),
            "Read [lien externe] and [lien externe]"
        );
        assert_eq!(html_text("<p>plain words</p>"), "plain words");
    }

    #[test]
    fn gmail_date_tokens_are_converted_to_unix_milliseconds() {
        assert_eq!(parse_date_token("1970-01-01"), Some(0));
        assert_eq!(parse_date_token("2000-01-01"), Some(946684800));
    }

    #[test]
    fn cas_ranges_match_synthetic_attachment_payloads() {
        let config = CorpusConfig {
            messages: 100,
            seed: 42,
            profile: CorpusProfile::Personal,
            attachment_rate: 100,
            duplicate_rate: 55,
            max_attachment_bytes: 1024 * 1024,
            measure_compression: false,
        };
        for id in 0..config.messages {
            let message = generate_message(config, id);
            let ranges = attachment_ranges(&message.raw);
            assert_eq!(ranges.len(), message.attachments.len());
            for ((_, start, end), attachment) in ranges.iter().zip(&message.attachments) {
                assert_eq!(end - start, attachment.bytes);
            }
        }
    }

    #[test]
    fn cas_variants_reconstruct_the_original_bytes() {
        let config = CorpusConfig {
            messages: 50,
            seed: 42,
            profile: CorpusProfile::Personal,
            attachment_rate: 100,
            duplicate_rate: 55,
            max_attachment_bytes: 64 * 1024,
            measure_compression: false,
        };
        for variant in [
            CasVariant::Exact,
            CasVariant::Decoded,
            CasVariant::Hybrid { threshold: 4096 },
        ] {
            let mut blobs = std::collections::HashMap::new();
            for id in 0..config.messages {
                let message = generate_message(config, id);
                let (stored, pieces, _, _, _) =
                    externalize_message(&message.raw, variant, &mut blobs);
                assert!(!stored.is_empty());
                assert_eq!(message.raw, reconstruct_pieces(&pieces, &blobs));
            }
        }
    }

    #[test]
    fn cas_handles_mime_shaped_fixture_without_changing_exact_bytes() {
        let raw = b"From: sender@example.test\r\nSubject: folded\r\n\tname\r\n\r\n--outer\r\n--inner\r\nContent-Disposition: attachment; filename*=UTF-8''rapport.pdf\r\nContent-Transfer-Encoding: base64\r\n\r\nSGVsbG8=\r\n--inner--\r\n--outer--\r\n";
        let mut blobs = std::collections::HashMap::new();
        let (_, pieces, _, _, _) = externalize_message(raw, CasVariant::Exact, &mut blobs);
        assert_eq!(
            raw.as_slice(),
            reconstruct_pieces(&pieces, &blobs).as_slice()
        );
    }

    #[test]
    fn cas_tolerates_an_unreferenced_orphan_blob() {
        let raw = b"header\r\n\r\n--attachment; filename=x\r\nbody";
        let mut blobs = std::collections::HashMap::new();
        let (_, pieces, _, _, _) = externalize_message(raw, CasVariant::Exact, &mut blobs);
        blobs.insert("orphan".into(), b"not referenced".to_vec());
        assert_eq!(
            raw.as_slice(),
            reconstruct_pieces(&pieces, &blobs).as_slice()
        );
    }

    #[test]
    fn archive_location_reads_one_message_without_scanning_segments() {
        let root = std::env::temp_dir().join(format!("mail-archive-test-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        let archive = root.join("archive");
        let mut writer = ArchiveWriter::open(&archive, 4096).unwrap();
        let config = CorpusConfig {
            messages: 1,
            seed: 9,
            profile: CorpusProfile::Light,
            attachment_rate: 0,
            duplicate_rate: 0,
            max_attachment_bytes: 4096,
            measure_compression: false,
        };
        let message = generate_message(config, 0);
        let location = writer.append(&message).unwrap();
        writer.sync().unwrap();
        let (id, raw) = read_record(&archive, &location).unwrap();
        assert_eq!(id, message.id);
        assert_eq!(raw, message.raw);
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn authoritative_read_rejects_a_catalog_location_to_another_valid_frame() {
        let root = raw_read_test_root("identity");
        let archive = root.join("archive");
        fs::create_dir_all(&archive).unwrap();
        let first_raw = b"first raw";
        let second_raw = b"second raw";
        let mut writer = ArchiveWriter::open(&archive, 4096).unwrap();
        let first_location = writer.append_raw(1, first_raw).unwrap();
        let second_location = writer.append_raw(2, second_raw).unwrap();
        writer.sync().unwrap();
        let catalog = create_metadata(&root.join("metadata.sqlite")).unwrap();
        for (id, raw) in [(1, first_raw.as_slice()), (2, second_raw.as_slice())] {
            let message = Message {
                id,
                message_id: format!("fixture-{id}"),
                timestamp: 0,
                sender: "sender@example.test".into(),
                recipients: Vec::new(),
                subject: String::new(),
                text_body: String::new(),
                html_body: None,
                account: "fixture".into(),
                folder: "Inbox".into(),
                thread: "thread".into(),
                attachments: Vec::new(),
                raw: raw.to_vec(),
            };
            let location = if id == 1 {
                &first_location
            } else {
                &second_location
            };
            insert_metadata(&catalog, &message, location).unwrap();
        }
        catalog
            .execute(
                "UPDATE messages SET segment=?1, archive_offset=?2, frame_bytes=?3 WHERE doc_id=1",
                params![
                    second_location.segment,
                    second_location.offset as i64,
                    second_location.frame_bytes as i64
                ],
            )
            .unwrap();
        drop(catalog);
        drop(writer);

        let error = read_archived_raw(&root, 1).unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::InvalidData);
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn record_read_rejects_inconsistent_coordinates() {
        let root = raw_read_test_root("coordinates");
        let archive = root.join("archive");
        fs::create_dir_all(&archive).unwrap();
        let mut writer = ArchiveWriter::open(&archive, 4096).unwrap();
        let location = writer.append_raw(7, b"coordinate fixture").unwrap();
        writer.sync().unwrap();
        drop(writer);

        let mut wrong_frame_bytes = location.clone();
        wrong_frame_bytes.frame_bytes -= 1;
        assert_eq!(
            read_record(&archive, &wrong_frame_bytes)
                .unwrap_err()
                .kind(),
            io::ErrorKind::InvalidData
        );

        let mut outside = location.clone();
        outside.offset = u64::MAX;
        assert_eq!(
            read_record(&archive, &outside).unwrap_err().kind(),
            io::ErrorKind::InvalidData
        );

        let mut invalid_segment = location;
        invalid_segment.segment = "../segment-000000.arc".into();
        assert_eq!(
            read_record(&archive, &invalid_segment).unwrap_err().kind(),
            io::ErrorKind::InvalidData
        );
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn record_read_rejects_a_stored_length_before_allocating_it() {
        let root = raw_read_test_root("length");
        let archive = root.join("archive");
        fs::create_dir_all(&archive).unwrap();
        let mut writer = ArchiveWriter::open(&archive, 4096).unwrap();
        let location = writer.append_raw(9, b"bounded fixture").unwrap();
        writer.sync().unwrap();
        drop(writer);

        let path = archive.join(&location.segment);
        let mut file = OpenOptions::new().write(true).open(path).unwrap();
        file.seek(SeekFrom::Start(location.offset + 16)).unwrap();
        file.write_all(&u64::MAX.to_le_bytes()).unwrap();
        drop(file);

        let error = read_record(&archive, &location).unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::InvalidData);
        let _ = fs::remove_dir_all(root);
    }

    fn inventory_fixture(label: &str) -> (PathBuf, Vec<ArchiveLocation>, Vec<Vec<u8>>) {
        let root = raw_read_test_root(label);
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(root.join("archive")).unwrap();
        let catalog = create_metadata(&root.join("metadata.sqlite")).unwrap();
        let mut writer = ArchiveWriter::open(&root.join("archive"), 64 * 1024).unwrap();
        let raws = vec![
            b"frame one payload".to_vec(),
            b"frame two payload".to_vec(),
            b"frame three payload".to_vec(),
        ];
        let mut locations = Vec::new();
        for (id, raw) in raws.iter().enumerate() {
            let location = writer.append_raw(id as u64, raw).unwrap();
            let message = Message {
                id: id as u64,
                message_id: format!("inventory-{id}"),
                timestamp: 0,
                sender: String::new(),
                recipients: Vec::new(),
                subject: String::new(),
                text_body: String::new(),
                html_body: None,
                account: String::new(),
                folder: String::new(),
                thread: String::new(),
                attachments: Vec::new(),
                raw: raw.clone(),
            };
            insert_metadata(&catalog, &message, &location).unwrap();
            locations.push(location);
        }
        writer.sync().unwrap();
        drop(catalog);
        drop(writer);
        (root, locations, raws)
    }

    #[test]
    fn inventory_keeps_neighbors_available_for_each_central_frame_corruption() {
        for (label, field_offset) in [
            ("magic", 0usize),
            ("id", 8),
            ("length", 16),
            ("checksum", 24),
            ("payload", 32),
        ] {
            let (root, locations, _) = inventory_fixture(label);
            let segment_path = root.join("archive").join(&locations[1].segment);
            let before_segment = fs::read(&segment_path).unwrap();
            let sqlite_paths = [
                root.join("metadata.sqlite"),
                root.join("metadata.sqlite-wal"),
                root.join("metadata.sqlite-shm"),
            ];
            let before_sqlite = sqlite_paths
                .iter()
                .map(|path| fs::read(path).ok())
                .collect::<Vec<_>>();
            let mut segment = before_segment.clone();
            segment[locations[1].offset as usize + field_offset] ^= 1;
            fs::write(&segment_path, &segment).unwrap();

            let result = inventory_records(&root).unwrap();
            assert!(matches!(
                result[0].status,
                RecordInventoryStatus::AvailableValidated
            ));
            assert!(matches!(
                result[1].status,
                RecordInventoryStatus::Inconsistent { .. }
            ));
            assert!(matches!(
                result[2].status,
                RecordInventoryStatus::AvailableValidated
            ));
            let after_sqlite = sqlite_paths
                .iter()
                .map(|path| fs::read(path).ok())
                .collect::<Vec<_>>();
            assert_eq!(after_sqlite, before_sqlite);
            assert_eq!(fs::read(&segment_path).unwrap(), segment);
            let _ = fs::remove_dir_all(root);
        }
    }

    #[test]
    fn inventory_reports_missing_segment_without_hiding_other_records() {
        let (root, locations, _) = inventory_fixture("missing-segment");
        let original_segment = root.join("archive").join(&locations[0].segment);
        let missing_segment = "segment-000001.arc";
        fs::copy(
            &original_segment,
            root.join("archive").join(missing_segment),
        )
        .unwrap();
        let catalog = Connection::open(root.join("metadata.sqlite")).unwrap();
        catalog
            .execute(
                "UPDATE messages SET segment=?1, archive_offset=0 WHERE doc_id=0",
                params![missing_segment],
            )
            .unwrap();
        drop(catalog);
        fs::remove_file(root.join("archive").join(missing_segment)).unwrap();
        let result = inventory_records(&root).unwrap();
        assert!(matches!(
            result[0].status,
            RecordInventoryStatus::PhysicallyMissing
        ));
        assert!(matches!(
            result[1].status,
            RecordInventoryStatus::AvailableValidated
        ));
        assert!(matches!(
            result[2].status,
            RecordInventoryStatus::AvailableValidated
        ));
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn inventory_continues_after_invalid_catalogue_coordinate_and_truncated_tail() {
        let (root, locations, _) = inventory_fixture("invalid-coordinate");
        let catalog = Connection::open(root.join("metadata.sqlite")).unwrap();
        catalog
            .execute("UPDATE messages SET archive_offset=-1 WHERE doc_id=1", [])
            .unwrap();
        drop(catalog);
        let result = inventory_records(&root).unwrap();
        assert!(matches!(
            result[0].status,
            RecordInventoryStatus::AvailableValidated
        ));
        assert!(matches!(
            result[1].status,
            RecordInventoryStatus::Inconsistent { .. }
        ));
        assert_eq!(result[1].doc_id, 1);
        assert!(matches!(
            result[2].status,
            RecordInventoryStatus::AvailableValidated
        ));
        let catalog = Connection::open(root.join("metadata.sqlite")).unwrap();
        catalog
            .execute("UPDATE messages SET segment=123 WHERE doc_id=1", [])
            .unwrap();
        drop(catalog);
        let result = inventory_records(&root).unwrap();
        assert_eq!(result[1].doc_id, 1);
        assert!(matches!(
            result[1].status,
            RecordInventoryStatus::Inconsistent { .. }
        ));
        let segment_path = root.join("archive").join(&locations[0].segment);
        let mut segment = fs::read(&segment_path).unwrap();
        segment.truncate(locations[2].offset as usize);
        fs::write(&segment_path, segment).unwrap();
        let result = inventory_records(&root).unwrap();
        assert!(matches!(
            result[0].status,
            RecordInventoryStatus::AvailableValidated
        ));
        assert!(matches!(
            result[1].status,
            RecordInventoryStatus::Inconsistent { .. }
        ));
        assert!(matches!(
            result[2].status,
            RecordInventoryStatus::PhysicallyMissing
        ));
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn gmail_index_updates_incrementally_after_archive_append() {
        let root =
            std::env::temp_dir().join(format!("mail-index-incremental-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        let catalog = create_metadata(&root.join("metadata.sqlite")).unwrap();
        let mut writer = ArchiveWriter::open(&root.join("archive"), 4096).unwrap();
        let raw = |word: &str| {
            format!(
                "From: fixture@example.test\r\nTo: reader@example.test\r\nSubject: {word}\r\n\r\n{word}"
            )
            .into_bytes()
        };
        let first = raw("alpha");
        let first_location = writer.append_raw(0, &first).unwrap();
        insert_gmail_metadata(
            &catalog,
            "fixture-account",
            "gmail-0",
            0,
            "thread-0",
            "[\"INBOX\"]",
            Some(1),
            Some("1"),
            &first_location,
        )
        .unwrap();
        writer.sync().unwrap();
        index_gmail_archive(&root).unwrap();
        assert_eq!(
            GmailSearchIndex::open(&root)
                .unwrap()
                .search("alpha", 10)
                .unwrap()
                .len(),
            1
        );

        let second = raw("beta");
        let second_location = writer.append_raw(1, &second).unwrap();
        insert_gmail_metadata(
            &catalog,
            "fixture-account",
            "gmail-1",
            1,
            "thread-1",
            "[\"INBOX\"]",
            Some(2),
            Some("2"),
            &second_location,
        )
        .unwrap();
        writer.sync().unwrap();
        let stats = index_gmail_archive(&root).unwrap();
        assert_eq!(stats.indexed, 1);
        assert_eq!(
            GmailSearchIndex::open(&root)
                .unwrap()
                .search("beta", 10)
                .unwrap()
                .len(),
            1
        );
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn search_finds_a_message_using_attachment_text_only() {
        let root = std::env::temp_dir().join(format!(
            "mail-attachment-text-search-{}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&root);
        let catalog = create_metadata(&root.join("metadata.sqlite")).unwrap();
        let mut writer = ArchiveWriter::open(&root.join("archive"), 4096).unwrap();
        let raw = b"From: fixture@example.test\r\nTo: reader@example.test\r\nSubject: hello\r\nContent-Type: multipart/mixed; boundary=part\r\n\r\n--part\r\nContent-Type: text/plain\r\n\r\nhello\r\n--part\r\nContent-Type: text/plain; charset=iso-8859-1\r\nContent-Disposition: attachment; filename=note.txt\r\n\r\ncaf\xe9 phrase-secrete-947\r\n--part--\r\n";
        let location = writer.append_raw(0, raw).unwrap();
        insert_gmail_metadata(
            &catalog,
            "fixture-account",
            "gmail-attachment-text",
            0,
            "thread-0",
            "[\"INBOX\"]",
            Some(1),
            Some("1"),
            &location,
        )
        .unwrap();
        writer.sync().unwrap();
        index_gmail_archive(&root).unwrap();
        let results = GmailSearchIndex::open(&root)
            .unwrap()
            .search("phrase-secrete-947", 10)
            .unwrap();
        assert_eq!(
            results
                .iter()
                .map(|result| result.doc_id)
                .collect::<Vec<_>>(),
            vec![0]
        );
        assert_eq!(
            GmailSearchIndex::open(&root)
                .unwrap()
                .search("café", 10)
                .unwrap()
                .iter()
                .map(|result| result.doc_id)
                .collect::<Vec<_>>(),
            vec![0]
        );
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn search_finds_a_message_using_pdf_attachment_text_when_provider_exists() {
        if std::process::Command::new("pdftotext")
            .arg("-v")
            .output()
            .is_err()
        {
            return;
        }
        use base64::Engine;
        let root =
            std::env::temp_dir().join(format!("mail-pdf-text-search-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        let catalog = create_metadata(&root.join("metadata.sqlite")).unwrap();
        let mut writer = ArchiveWriter::open(&root.join("archive"), 4096).unwrap();
        let encoded = base64::engine::general_purpose::STANDARD
            .encode(attachment_text::test_pdf_fixture("phrase-secrete-947"));
        let raw = format!(
            "From: fixture@example.test\r\nTo: reader@example.test\r\nSubject: hello\r\nContent-Type: multipart/mixed; boundary=part\r\n\r\n--part\r\nContent-Type: text/plain\r\n\r\nhello\r\n--part\r\nContent-Type: application/pdf\r\nContent-Disposition: attachment; filename=note.pdf\r\nContent-Transfer-Encoding: base64\r\n\r\n{encoded}\r\n--part--\r\n"
        );
        let location = writer.append_raw(0, raw.as_bytes()).unwrap();
        insert_gmail_metadata(
            &catalog,
            "fixture-account",
            "gmail-pdf-text",
            0,
            "thread-0",
            "[\"INBOX\"]",
            Some(1),
            Some("1"),
            &location,
        )
        .unwrap();
        writer.sync().unwrap();
        index_gmail_archive(&root).unwrap();
        let results = GmailSearchIndex::open(&root)
            .unwrap()
            .search("phrase-secrete-947", 10)
            .unwrap();
        assert_eq!(
            results
                .iter()
                .map(|result| result.doc_id)
                .collect::<Vec<_>>(),
            vec![0]
        );
        let _ = fs::remove_dir_all(&root);
    }

    #[cfg(windows)]
    #[test]
    fn search_finds_a_message_using_docx_attachment_text_when_provider_exists() {
        use base64::Engine;
        let Some(fixture) = std::env::var_os("MEMORIA_IFILTER_DOCX_FIXTURE") else {
            return;
        };
        let fixture = PathBuf::from(fixture);
        if !fixture.is_file() {
            return;
        }
        let provider = selected_provider(
            "application/vnd.openxmlformats-officedocument.wordprocessingml.document",
            &ProviderSelection::Automatic,
        )
        .expect("Windows IFilter DOCX provider should be available");
        assert_eq!(provider.id.as_str(), "windows-ifilter");
        let encoded = base64::engine::general_purpose::STANDARD.encode(fs::read(fixture).unwrap());
        let raw = format!(
            "From: fixture@example.test\r\nTo: reader@example.test\r\nSubject: hello\r\nContent-Type: multipart/mixed; boundary=part\r\n\r\n--part\r\nContent-Type: text/plain\r\n\r\nhello\r\n--part\r\nContent-Type: application/vnd.openxmlformats-officedocument.wordprocessingml.document\r\nContent-Disposition: attachment; filename=note.docx\r\nContent-Transfer-Encoding: base64\r\n\r\n{encoded}\r\n--part--\r\n"
        );
        let root =
            std::env::temp_dir().join(format!("mail-docx-text-search-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        let catalog = create_metadata(&root.join("metadata.sqlite")).unwrap();
        let mut writer = ArchiveWriter::open(&root.join("archive"), 4096).unwrap();
        let location = writer.append_raw(0, raw.as_bytes()).unwrap();
        insert_gmail_metadata(
            &catalog,
            "fixture-account",
            "fixture-docx-text",
            0,
            "thread-0",
            "[\"INBOX\"]",
            Some(1),
            Some("1"),
            &location,
        )
        .unwrap();
        writer.sync().unwrap();
        index_gmail_archive(&root).unwrap();
        let results = GmailSearchIndex::open(&root)
            .unwrap()
            .search("memoria-word-automation-fixture-947", 10)
            .unwrap();
        assert_eq!(
            results
                .iter()
                .map(|result| result.doc_id)
                .collect::<Vec<_>>(),
            vec![0]
        );
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn structured_search_combines_filters_without_post_filter_false_negatives() {
        let root =
            std::env::temp_dir().join(format!("mail-structured-search-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        let catalog = create_metadata(&root.join("metadata.sqlite")).unwrap();
        let mut writer = ArchiveWriter::open(&root.join("archive"), 4096).unwrap();
        let messages = [
            (
                "From: Alice Example <alice@example.test>\r\nTo: reader@example.test\r\nSubject: Invoice alpha\r\nDate: Thu, 01 Jan 1970 00:00:01 +0000\r\n\r\nalpha",
                "[\"INBOX\",\"STARRED\"]",
                1_000,
            ),
            (
                "From: Bob Example <bob@example.test>\r\nTo: reader@example.test\r\nSubject: Invoice beta\r\nDate: Thu, 01 Jan 1970 00:00:02 +0000\r\nContent-Type: multipart/mixed; boundary=part\r\n\r\n--part\r\nContent-Type: text/plain\r\n\r\nbeta\r\n--part\r\nContent-Type: application/pdf\r\nContent-Disposition: attachment; filename=report.pdf\r\nContent-Transfer-Encoding: base64\r\n\r\nJVBERi0=\r\n--part--\r\n",
                "[\"INBOX\",\"WORK\"]",
                2_000,
            ),
            (
                "From: Alice Example <alice@example.test>\r\nTo: other@example.test\r\nSubject: Invoice gamma\r\nDate: Thu, 01 Jan 1970 00:03:00 +0000\r\nContent-Type: multipart/related; boundary=part\r\n\r\n--part\r\nContent-Type: text/html\r\n\r\n<img src=\"cid:logo@example.test\">\r\n--part\r\nContent-Type: image/png\r\nContent-ID: <logo@example.test>\r\nContent-Disposition: inline\r\n\r\nPNG\r\n--part--\r\n",
                "[\"SENT\"]",
                3_000,
            ),
        ];
        for (id, (raw, labels, timestamp)) in messages.into_iter().enumerate() {
            let raw = raw.as_bytes();
            let location = writer.append_raw(id as u64, raw).unwrap();
            insert_gmail_metadata(
                &catalog,
                "fixture-account",
                &format!("gmail-{id}"),
                id as i64,
                &format!("thread-{id}"),
                labels,
                Some(timestamp),
                Some(&format!("{id}")),
                &location,
            )
            .unwrap();
        }
        writer.sync().unwrap();
        // The deliberately truncated PDF is provider-dependent: pdftotext
        // reports a failure on Linux, while Windows IFilter may accept it and
        // return no text. This test targets structured-search results, not
        // provider-specific extraction diagnostics.
        index_gmail_archive(&root).unwrap();
        let index = GmailSearchIndex::open(&root).unwrap();
        let result = index
            .search_request(&SearchRequest {
                text: "invoice".into(),
                from: Some("alice".into()),
                date_to: Some(2_500),
                ..SearchRequest {
                    limit: 10,
                    ..Default::default()
                }
            })
            .unwrap();
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].doc_id, 0);
        assert_eq!(
            index
                .search_request(&SearchRequest {
                    from: Some("alice@example.test".into()),
                    limit: 10,
                    ..Default::default()
                })
                .unwrap()
                .iter()
                .map(|item| item.doc_id)
                .collect::<Vec<_>>(),
            vec![2, 0]
        );
        assert_eq!(
            index
                .search_request(&SearchRequest {
                    attachment: AttachmentFilter::With,
                    attachment_mime: Some("application/pdf".into()),
                    limit: 10,
                    ..Default::default()
                })
                .unwrap()
                .iter()
                .map(|item| item.doc_id)
                .collect::<Vec<_>>(),
            vec![1]
        );
        assert!(!index
            .search_request(&SearchRequest {
                attachment: AttachmentFilter::With,
                limit: 10,
                ..Default::default()
            })
            .unwrap()
            .is_empty());
        assert_eq!(
            index
                .search_request(&SearchRequest {
                    attachment: AttachmentFilter::Without,
                    limit: 10,
                    ..Default::default()
                })
                .unwrap()
                .iter()
                .map(|item| item.doc_id)
                .collect::<Vec<_>>(),
            vec![2, 0]
        );
        assert_eq!(
            index
                .search_request(&SearchRequest {
                    labels: vec!["WORK".into()],
                    limit: 10,
                    ..Default::default()
                })
                .unwrap()
                .first()
                .map(|item| item.doc_id),
            Some(1)
        );
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn export_message_eml_is_byte_exact_for_realistic_mime() {
        let root = std::env::temp_dir().join(format!(
            "mail-export-eml-{}-{}",
            std::process::id(),
            Instant::now().elapsed().as_nanos()
        ));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(root.join("archive")).unwrap();
        create_metadata(&root.join("metadata.sqlite")).unwrap();
        let raw = b"From: sender@example.test\r\nTo: recipient@example.test\r\nSubject: Caf\xc3\xa9\r\nContent-Type: multipart/mixed; boundary=outer\r\nX-Raw: \xc3\xa9\r\n\r\n--outer\r\nContent-Type: text/plain; charset=utf-8\r\n\r\nBonjour \xc3\xa9\r\n--outer\r\nContent-Type: application/octet-stream\r\nContent-Disposition: attachment; filename=piece.bin\r\nContent-Transfer-Encoding: base64\r\n\r\nAQIDBA==\r\n--outer--\r\n";
        let message = Message {
            id: 42,
            message_id: "eml-fixture".into(),
            timestamp: 0,
            sender: "sender@example.test".into(),
            recipients: vec!["recipient@example.test".into()],
            subject: "Café".into(),
            text_body: "Bonjour é".into(),
            html_body: None,
            account: "fixture".into(),
            folder: "Inbox".into(),
            thread: "thread".into(),
            attachments: Vec::new(),
            raw: raw.to_vec(),
        };
        let mut writer = ArchiveWriter::open(&root.join("archive"), 4096).unwrap();
        let location = writer.append(&message).unwrap();
        writer.sync().unwrap();
        let catalog = create_metadata(&root.join("metadata.sqlite")).unwrap();
        insert_metadata(&catalog, &message, &location).unwrap();
        drop(catalog);
        drop(writer);
        let destination = root.join("export.eml");
        export_message_eml(&root, 42, &destination).unwrap();
        let exported = fs::read(destination).unwrap();
        assert_eq!(exported, raw);
        assert!(mailparse::parse_mail(&exported).is_ok());
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn export_message_eml_reports_destination_errors() {
        let root = std::env::temp_dir().join(format!(
            "mail-export-error-{}-{}",
            std::process::id(),
            Instant::now().elapsed().as_nanos()
        ));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(root.join("archive")).unwrap();
        create_metadata(&root.join("metadata.sqlite")).unwrap();
        let raw = b"From: sender@example.test\r\n\r\nbody";
        let message = Message {
            id: 7,
            message_id: "eml-error-fixture".into(),
            timestamp: 0,
            sender: "sender@example.test".into(),
            recipients: Vec::new(),
            subject: String::new(),
            text_body: "body".into(),
            html_body: None,
            account: "fixture".into(),
            folder: "Inbox".into(),
            thread: "thread".into(),
            attachments: Vec::new(),
            raw: raw.to_vec(),
        };
        let mut writer = ArchiveWriter::open(&root.join("archive"), 4096).unwrap();
        let location = writer.append(&message).unwrap();
        writer.sync().unwrap();
        let catalog = create_metadata(&root.join("metadata.sqlite")).unwrap();
        insert_metadata(&catalog, &message, &location).unwrap();
        drop(catalog);
        drop(writer);
        let error =
            export_message_eml(&root, 7, &root.join("missing").join("export.eml")).unwrap_err();
        assert!(matches!(
            error.kind(),
            io::ErrorKind::NotFound | io::ErrorKind::PermissionDenied
        ));
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn export_message_eml_can_replace_an_existing_destination() {
        let root = std::env::temp_dir().join(format!(
            "mail-export-existing-{}-{}",
            std::process::id(),
            Instant::now().elapsed().as_nanos()
        ));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(root.join("archive")).unwrap();
        let catalog = create_metadata(&root.join("metadata.sqlite")).unwrap();
        let raw = b"From: sender@example.test\r\n\r\nbody";
        let message = Message {
            id: 8,
            message_id: "eml-existing-fixture".into(),
            timestamp: 0,
            sender: "sender@example.test".into(),
            recipients: Vec::new(),
            subject: String::new(),
            text_body: "body".into(),
            html_body: None,
            account: "fixture".into(),
            folder: "Inbox".into(),
            thread: "thread".into(),
            attachments: Vec::new(),
            raw: raw.to_vec(),
        };
        let mut writer = ArchiveWriter::open(&root.join("archive"), 4096).unwrap();
        let location = writer.append(&message).unwrap();
        writer.sync().unwrap();
        insert_metadata(&catalog, &message, &location).unwrap();
        drop(writer);
        drop(catalog);
        let destination = root.join("existing.eml");
        fs::write(&destination, b"keep").unwrap();
        export_message_eml(&root, 8, &destination).unwrap();
        assert_eq!(fs::read(destination).unwrap(), raw);
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn real_archive_structured_search_smoke_is_offline_only() {
        let root = Path::new(".local/gmail-real-20260820");
        if !root.join("metadata.sqlite").exists() {
            return;
        }
        let index = GmailSearchIndex::open(root).unwrap();
        let _ = index
            .search_request(&SearchRequest {
                attachment: AttachmentFilter::With,
                attachment_mime: Some("image/*".into()),
                limit: 10,
                ..Default::default()
            })
            .unwrap();
        let labels = available_gmail_labels(root).unwrap();
        if let Some(label) = labels.first() {
            let _ = index
                .search_request(&SearchRequest {
                    labels: vec![label.clone()],
                    limit: 10,
                    ..Default::default()
                })
                .unwrap();
        }
    }

    #[test]
    fn recovery_truncates_an_incomplete_tail() {
        let root =
            std::env::temp_dir().join(format!("mail-archive-recovery-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        let mut writer = ArchiveWriter::open(&root, 4096).unwrap();
        let config = CorpusConfig {
            messages: 1,
            seed: 11,
            profile: CorpusProfile::Light,
            attachment_rate: 0,
            duplicate_rate: 0,
            max_attachment_bytes: 4096,
            measure_compression: false,
        };
        writer.append(&generate_message(config, 0)).unwrap();
        writer.sync().unwrap();
        drop(writer);
        let path = root.join("segment-000000.arc");
        let length = fs::metadata(&path).unwrap().len();
        let file = OpenOptions::new().write(true).open(&path).unwrap();
        file.set_len(length + 9).unwrap();
        drop(file);
        let (frames, truncated) = recover_segments(&root).unwrap();
        assert_eq!(frames, 1);
        assert_eq!(truncated, 9);
        let _ = fs::remove_dir_all(&root);
    }
}
