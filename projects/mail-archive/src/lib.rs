use flate2::{write::GzEncoder, Compression};
use fs4::fs_std::FileExt;
use mailparse::{parse_mail, MailHeaderMap, ParsedMail};
use rusqlite::{params, Connection, OpenFlags, OptionalExtension};
use std::cmp::Ordering;
use std::collections::{BTreeMap, HashMap, HashSet};
use std::fs::{self, File, OpenOptions};
use std::io::{self, Read, Seek, SeekFrom, Write};
use std::ops::Bound;
use std::path::{Component, Path, PathBuf};
use std::sync::Arc;
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
pub mod recovery;

pub use attachment_text::{
    discover_providers, providers_for_mime, selected_provider, AttachmentTextStats, BackendKind,
    ExtractionProvider, ProviderAvailability, ProviderId, ProviderSelection,
};
pub use delivery_report::{
    analyze_delivery_report, DeliveryReportAnalysis, DeliveryReportKind, DsnMessageFields,
    DsnRecipient, DsnReport, MdnDisposition, MdnReport,
};

pub const DEFAULT_SEED: u64 = 0x4d_41_49_4c_41_52_43;
pub const MEMORIA_CATALOGUE_APPLICATION_ID: i64 = 0x4d_45_4d_31;
pub const MEMORIA_CATALOGUE_VERSION: i64 = 1;
const FRAME_MAGIC: &[u8; 8] = b"MAARC001";
const FRAME_HEADER_BYTES: u64 = 32;
const CATALOGUE_BATCH_RECORD_LIMIT: usize = 256;
const CATALOGUE_BATCH_BYTES_LIMIT: u64 = 16 * 1024 * 1024;

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

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct ArchiveLocation {
    pub segment: String,
    pub offset: u64,
    pub frame_bytes: u64,
}

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct RawReference {
    pub doc_id: u64,
    pub location: ArchiveLocation,
    pub blake3: [u8; 32],
}

/// Opaque token for one RAW append that has not crossed a durable barrier.
///
/// It deliberately exposes neither archive coordinates nor a content digest.
#[derive(Clone, Debug)]
pub struct PendingRawLocation {
    batch: Arc<RawBatchIdentity>,
    ordinal: usize,
    doc_id: u64,
    frame_bytes: u64,
}

#[derive(Debug)]
struct RawBatchIdentity;

#[derive(Debug)]
struct ArchiveAuthority {
    _lock: Option<ArchiveLock>,
}

pub(crate) struct CatalogueConnection {
    connection: Connection,
    authority: Arc<ArchiveAuthority>,
}

impl std::ops::Deref for CatalogueConnection {
    type Target = Connection;

    fn deref(&self) -> &Self::Target {
        &self.connection
    }
}

impl std::ops::DerefMut for CatalogueConnection {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.connection
    }
}

/// A publishable RAW reference created only by [`ArchiveWriter::durable_barrier`].
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DurableRawLocation {
    reference: RawReference,
}

impl DurableRawLocation {
    pub fn reference(&self) -> &RawReference {
        &self.reference
    }
}

/// Exact set of RAW entries covered by one successful durable barrier.
#[derive(Clone, Debug)]
pub struct DurableRawBatch {
    batch: Arc<RawBatchIdentity>,
    authority: Arc<ArchiveAuthority>,
    entries: Vec<DurableRawLocation>,
    frame_bytes: u64,
}

impl DurableRawBatch {
    pub fn entries(&self) -> &[DurableRawLocation] {
        &self.entries
    }

    pub fn records(&self) -> u64 {
        self.entries.len() as u64
    }

    pub fn frame_bytes(&self) -> u64 {
        self.frame_bytes
    }
}

/// Opaque catalogue/archive pair for the safe public publication API.
pub struct ArchiveSession {
    writer: ArchiveWriter,
    catalogue: CatalogueConnection,
}

impl ArchiveSession {
    pub fn create(root: &Path, segment_bytes: u64) -> io::Result<Self> {
        let authority = acquire_session_authority(root)?;
        let writer = ArchiveWriter::open_with_authority(
            &root.join("archive"),
            segment_bytes,
            Arc::clone(&authority),
        )?;
        let catalogue = create_catalogue_for_authority(&root.join("metadata.sqlite"), authority)
            .map_err(sqlite_io)?;
        Ok(Self { writer, catalogue })
    }

    /// Recreates an archive while holding the same authority used for normal
    /// creation. The rendezvous file is outside the directory being removed.
    pub fn reset(root: &Path, segment_bytes: u64) -> io::Result<Self> {
        let authority = acquire_session_authority(root)?;
        if root.exists() {
            fs::remove_dir_all(root)?;
        }
        let writer = ArchiveWriter::open_with_authority(
            &root.join("archive"),
            segment_bytes,
            Arc::clone(&authority),
        )?;
        let catalogue = create_catalogue_for_authority(&root.join("metadata.sqlite"), authority)
            .map_err(sqlite_io)?;
        Ok(Self { writer, catalogue })
    }

    pub(crate) fn parts_mut(&mut self) -> (&mut ArchiveWriter, &mut CatalogueConnection) {
        (&mut self.writer, &mut self.catalogue)
    }

    #[cfg(test)]
    fn replace_writer_for_test(&mut self) {
        let replacement = ArchiveWriter::open_with_authority(
            &self.writer.root,
            self.writer.segment_bytes,
            Arc::clone(&self.catalogue.authority),
        )
        .unwrap();
        let previous = std::mem::replace(&mut self.writer, replacement);
        drop(previous);
    }

    pub fn writer_mut(&mut self) -> &mut ArchiveWriter {
        &mut self.writer
    }

    pub fn publish_catalogue_batch(
        &self,
        batch: &[CatalogueBatchRecord],
        durable: &DurableRawBatch,
    ) -> rusqlite::Result<()> {
        publish_catalogue_batch(&self.catalogue, batch, durable)
    }

    pub fn publish_gmail_batch(
        &self,
        batch: &[GmailBatchRecord],
        durable: &DurableRawBatch,
    ) -> rusqlite::Result<()> {
        publish_gmail_batch(&self.catalogue, batch, durable)
    }
}

/// Gmail metadata staged before durability; it cannot expose a RAW location.
pub struct GmailBatchRecord {
    source_account: String,
    gmail_id: String,
    doc_id: i64,
    thread_id: String,
    label_ids_json: String,
    internal_date_ms: Option<i64>,
    message_history_id: Option<String>,
    pending: PendingRawLocation,
}

impl GmailBatchRecord {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        source_account: String,
        gmail_id: String,
        doc_id: i64,
        thread_id: String,
        label_ids_json: String,
        internal_date_ms: Option<i64>,
        message_history_id: Option<String>,
        pending: PendingRawLocation,
    ) -> Self {
        Self {
            source_account,
            gmail_id,
            doc_id,
            thread_id,
            label_ids_json,
            internal_date_ms,
            message_history_id,
            pending,
        }
    }

    pub(crate) fn frame_bytes(&self) -> u64 {
        self.pending.frame_bytes
    }
}

pub(crate) struct ImapBatchRecord {
    source_account: String,
    mailbox: String,
    uid_validity: u32,
    uid: u32,
    flags_json: String,
    internal_date: Option<String>,
    internal_date_ms: Option<i64>,
    rfc822_size: Option<u32>,
    doc_id: i64,
    pending: PendingRawLocation,
}

impl ImapBatchRecord {
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn new(
        source_account: String,
        mailbox: String,
        uid_validity: u32,
        uid: u32,
        flags_json: String,
        internal_date: Option<String>,
        internal_date_ms: Option<i64>,
        rfc822_size: Option<u32>,
        doc_id: i64,
        pending: PendingRawLocation,
    ) -> Self {
        Self {
            source_account,
            mailbox,
            uid_validity,
            uid,
            flags_json,
            internal_date,
            internal_date_ms,
            rfc822_size,
            doc_id,
            pending,
        }
    }

    pub(crate) fn frame_bytes(&self) -> u64 {
        self.pending.frame_bytes
    }
}

/// Corpus metadata staged before durability; publication requires its exact durable batch.
pub struct CatalogueBatchRecord {
    message: Message,
    pending: PendingRawLocation,
}

impl CatalogueBatchRecord {
    pub fn new(message: Message, pending: PendingRawLocation) -> Self {
        Self { message, pending }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ArchiveWriterState {
    Ready,
    Poisoned,
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

/// Classification of one physically encountered frame or tail.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum PhysicalFrameStatus {
    CataloguedValidated,
    CataloguedInconsistent,
    OrphanValidated,
    PhysicalCorruption { reason: String },
    IncompleteTail { reason: String },
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PhysicalFrameInventory {
    pub location: ArchiveLocation,
    pub doc_id: Option<u64>,
    pub blake3: Option<[u8; 32]>,
    pub status: PhysicalFrameStatus,
}

/// Read-only physical archive inventory, joined to the authoritative catalogue
/// by the complete frame identity. A doc_id alone is deliberately insufficient.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct PhysicalInventory {
    pub frames: Vec<PhysicalFrameInventory>,
    pub catalogued_records: u64,
    pub validated_catalogued_records: u64,
    pub orphan_valid_frames: u64,
    pub inconsistent_catalogued_records: u64,
    pub catalogued_physically_missing: u64,
    pub physical_corruptions: u64,
    pub incomplete_tails: u64,
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
    pub raw_unavailable: u64,
    pub raw_inconsistent: u64,
    pub partial: bool,
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
    catalog: CatalogueConnection,
}

impl GmailSearchIndex {
    pub fn open(root: &Path) -> io::Result<Self> {
        let (index, fields) = open_or_create_gmail_tantivy(root).map_err(io::Error::other)?;
        let reader = index.reader().map_err(io::Error::other)?;
        let catalog = open_catalogue(&root.join("metadata.sqlite")).map_err(sqlite_io)?;
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
    pub catalogued_records: u64,
    pub validated_catalogued_records: u64,
    pub orphan_valid_frames: u64,
    pub inconsistent_catalogued_records: u64,
    pub catalogued_physically_missing: u64,
    pub physical_corruptions: u64,
    pub incomplete_tails: u64,
    pub archive_bytes: u64,
    pub segments: u64,
    pub catalog_bytes: u64,
    pub index_bytes: u64,
    pub index_present: bool,
}

pub fn archive_summary(root: &Path) -> io::Result<ArchiveSummary> {
    let catalog_path = root.join("metadata.sqlite");
    let catalogue_present = match fs::metadata(&catalog_path) {
        Ok(_) => true,
        Err(error) if error.kind() == io::ErrorKind::NotFound => false,
        Err(error) => return Err(error),
    };
    let messages = if catalogue_present {
        validate_existing_catalogue(&catalog_path).map_err(sqlite_io)?;
        let connection =
            Connection::open_with_flags(&catalog_path, OpenFlags::SQLITE_OPEN_READ_ONLY)
                .map_err(sqlite_io)?;
        connection
            .query_row("SELECT COUNT(*) FROM messages", [], |row| {
                row.get::<_, i64>(0)
            })
            .map_err(sqlite_io)? as u64
    } else {
        0
    };
    let physical = if catalogue_present {
        inventory_physical(root)?
    } else {
        PhysicalInventory::default()
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
        catalogued_records: physical.catalogued_records,
        validated_catalogued_records: physical.validated_catalogued_records,
        orphan_valid_frames: physical.orphan_valid_frames,
        inconsistent_catalogued_records: physical.inconsistent_catalogued_records,
        catalogued_physically_missing: physical.catalogued_physically_missing,
        physical_corruptions: physical.physical_corruptions,
        incomplete_tails: physical.incomplete_tails,
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
    let catalog = open_catalogue(&root.join("metadata.sqlite")).map_err(sqlite_io)?;
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

#[derive(Debug)]
pub struct ArchiveWriter {
    authority: Arc<ArchiveAuthority>,
    root: PathBuf,
    segment_bytes: u64,
    file: File,
    segment_name: String,
    offset: u64,
    segment_number: u64,
    pending_batch: Arc<RawBatchIdentity>,
    pending_references: Vec<RawReference>,
    pending_records: u64,
    pending_frame_bytes: u64,
    file_dirty: bool,
    namespace_sync_pending: bool,
    current_segment_created: bool,
    poisoned: Option<String>,
    #[cfg(test)]
    fault: ArchiveWriterFaultInjection,
}

/// OS-backed exclusive authority for the complete logical archive.
///
/// The file is only a stable rendezvous point; its contents and existence are
/// not the proof of ownership. The OS lock is released when this handle is
/// dropped, including process termination.
#[derive(Debug)]
struct ArchiveLock {
    _file: File,
}

fn acquire_archive_authority(archive_root: &Path) -> io::Result<Arc<ArchiveAuthority>> {
    let canonical_name = archive_root
        .canonicalize()
        .ok()
        .and_then(|path| path.file_name().map(ToOwned::to_owned));
    let requested_name = archive_root.file_name().and_then(|name| name.to_str());
    let lock_path = if requested_name == Some("archive")
        || canonical_name.as_deref().and_then(|name| name.to_str()) == Some("archive")
    {
        session_lock_path(archive_root.parent().ok_or_else(|| {
            io::Error::new(io::ErrorKind::InvalidInput, "archive path has no parent")
        })?)?
    } else {
        archive_lock_path(archive_root)?
    };
    Ok(Arc::new(ArchiveAuthority {
        _lock: Some(ArchiveLock::acquire_at(&lock_path)?),
    }))
}

fn acquire_session_authority(root: &Path) -> io::Result<Arc<ArchiveAuthority>> {
    Ok(Arc::new(ArchiveAuthority {
        _lock: Some(ArchiveLock::acquire_at(&session_lock_path(root)?)?),
    }))
}

impl ArchiveLock {
    fn acquire_at(lock_path: &Path) -> io::Result<Self> {
        let file = OpenOptions::new()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
            .open(lock_path)?;
        match file.try_lock_exclusive()? {
            true => Ok(Self { _file: file }),
            false => Err(io::Error::new(
                io::ErrorKind::AlreadyExists,
                "ArchiveAlreadyLocked: another writer owns this archive",
            )),
        }
    }
}

fn session_lock_path(root: &Path) -> io::Result<PathBuf> {
    let parent = root
        .parent()
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "archive root has no parent"))?;
    fs::create_dir_all(parent)?;
    let canonical_parent = fs::canonicalize(parent)?;
    let canonical_root = fs::canonicalize(root).unwrap_or_else(|_| {
        canonical_parent.join(
            root.file_name()
                .expect("root parent implies a final component"),
        )
    });
    let rendezvous_parent = canonical_root.parent().ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::InvalidInput,
            "canonical archive root has no parent",
        )
    })?;
    let root_name = canonical_root.file_name().ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::InvalidInput,
            "canonical archive root has no name",
        )
    })?;
    Ok(rendezvous_parent.join(format!(
        ".memoria-{}-archive.writer.lock",
        root_name.to_string_lossy()
    )))
}

fn archive_lock_path(archive_root: &Path) -> io::Result<PathBuf> {
    let parent = archive_root
        .parent()
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "archive path has no parent"))?;
    fs::create_dir_all(parent)?;
    let canonical_parent = fs::canonicalize(parent)?;
    let parent_name = parent.file_name().ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::InvalidInput,
            "archive parent has no final component",
        )
    })?;
    let requested_archive_name = archive_root.file_name().ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::InvalidInput,
            "archive path has no final component",
        )
    })?;
    let requested_parent = canonical_parent.clone();
    let canonical_archive = if archive_root.exists() {
        fs::canonicalize(archive_root)?
    } else {
        requested_parent.join(requested_archive_name)
    };
    let canonical_parent = canonical_archive
        .parent()
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "archive has no parent"))?;
    let archive_name = canonical_archive.file_name().ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::InvalidInput,
            "archive has no final component",
        )
    })?;
    Ok(canonical_parent.join(format!(
        ".memoria-{}-{}.writer.lock",
        canonical_parent
            .file_name()
            .unwrap_or(parent_name)
            .to_string_lossy(),
        archive_name.to_string_lossy()
    )))
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum ArchiveWriterWriteComponent {
    Magic,
    Id,
    Length,
    Checksum,
    Payload,
}

#[cfg(test)]
#[derive(Clone, Debug, Default)]
struct ArchiveWriterFaultInjection {
    fail_component: Option<ArchiveWriterWriteComponent>,
    partial_payload: bool,
    fail_sync_remaining: usize,
    fail_namespace_remaining: usize,
    sync_calls: usize,
    namespace_sync_calls: usize,
    namespace_sync_paths: Vec<PathBuf>,
}

impl ArchiveWriter {
    fn open_with_authority(
        root: &Path,
        segment_bytes: u64,
        authority: Arc<ArchiveAuthority>,
    ) -> io::Result<Self> {
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
        let segment_created = !path.exists();
        let mut file = OpenOptions::new()
            .create(true)
            .read(true)
            .append(true)
            .open(&path)?;
        let offset = file.seek(SeekFrom::End(0))?;
        Ok(Self {
            authority,
            root: root.to_path_buf(),
            segment_bytes: segment_bytes.max(1024),
            file,
            segment_name,
            offset,
            segment_number,
            pending_batch: Arc::new(RawBatchIdentity),
            pending_references: Vec::new(),
            pending_records: 0,
            pending_frame_bytes: 0,
            file_dirty: false,
            namespace_sync_pending: false,
            current_segment_created: segment_created,
            poisoned: None,
            #[cfg(test)]
            fault: ArchiveWriterFaultInjection::default(),
        })
    }

    pub fn open(root: &Path, segment_bytes: u64) -> io::Result<Self> {
        let authority = acquire_archive_authority(root)?;
        Self::open_with_authority(root, segment_bytes, authority)
    }

    #[cfg(test)]
    pub(crate) fn open_for_catalogue(
        root: &Path,
        segment_bytes: u64,
        catalogue: &CatalogueConnection,
    ) -> io::Result<Self> {
        Self::open_with_authority(root, segment_bytes, Arc::clone(&catalogue.authority))
    }

    #[cfg(test)]
    fn open_with_faults(
        root: &Path,
        segment_bytes: u64,
        fault: ArchiveWriterFaultInjection,
    ) -> io::Result<Self> {
        let mut writer = Self::open(root, segment_bytes)?;
        writer.fault = fault;
        Ok(writer)
    }

    #[cfg(test)]
    fn open_for_catalogue_with_faults(
        root: &Path,
        segment_bytes: u64,
        catalogue: &CatalogueConnection,
        fault: ArchiveWriterFaultInjection,
    ) -> io::Result<Self> {
        let mut writer = Self::open_for_catalogue(root, segment_bytes, catalogue)?;
        writer.fault = fault;
        Ok(writer)
    }

    pub fn append(&mut self, message: &Message) -> io::Result<PendingRawLocation> {
        self.append_raw(message.id, &message.raw)
    }

    pub fn append_raw(&mut self, id: u64, raw: &[u8]) -> io::Result<PendingRawLocation> {
        self.ensure_ready()?;
        let frame_bytes = FRAME_HEADER_BYTES
            .checked_add(raw.len() as u64)
            .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "frame size overflow"))?;
        if self.offset > 0 && self.offset + frame_bytes > self.segment_bytes {
            self.sync_current_file()?;
            let next_segment_number = self.segment_number + 1;
            let next_segment_name = format!("segment-{next_segment_number:06}.arc");
            let next_file = OpenOptions::new()
                .create(true)
                .create_new(true)
                .read(true)
                .append(true)
                .open(self.root.join(&next_segment_name))?;
            self.file = next_file;
            self.segment_number = next_segment_number;
            self.segment_name = next_segment_name;
            self.offset = 0;
            self.file_dirty = false;
            self.namespace_sync_pending = false;
            self.current_segment_created = true;
        }
        let start = self.offset;
        self.write_frame_component(ArchiveWriterWriteComponent::Magic, FRAME_MAGIC)?;
        self.write_frame_component(ArchiveWriterWriteComponent::Id, &id.to_le_bytes())?;
        self.write_frame_component(
            ArchiveWriterWriteComponent::Length,
            &(raw.len() as u64).to_le_bytes(),
        )?;
        let checksum = fnv64(raw);
        let blake3 = *blake3::hash(raw).as_bytes();
        self.write_frame_component(
            ArchiveWriterWriteComponent::Checksum,
            &checksum.to_le_bytes(),
        )?;
        self.write_frame_component(ArchiveWriterWriteComponent::Payload, raw)?;
        self.offset += frame_bytes;
        let ordinal = self.pending_references.len();
        self.pending_records += 1;
        self.pending_frame_bytes += frame_bytes;
        self.pending_references.push(RawReference {
            doc_id: id,
            location: ArchiveLocation {
                segment: self.segment_name.clone(),
                offset: start,
                frame_bytes,
            },
            blake3,
        });
        if self.current_segment_created {
            self.namespace_sync_pending = true;
        }
        Ok(PendingRawLocation {
            batch: Arc::clone(&self.pending_batch),
            ordinal,
            doc_id: id,
            frame_bytes,
        })
    }

    pub fn durable_barrier(&mut self) -> io::Result<DurableRawBatch> {
        self.ensure_ready()?;
        if self.file_dirty {
            self.sync_current_file()?;
        }
        if self.namespace_sync_pending {
            self.sync_namespace()?;
            self.namespace_sync_pending = false;
        }
        let entries = std::mem::take(&mut self.pending_references)
            .into_iter()
            .map(|reference| DurableRawLocation { reference })
            .collect();
        let receipt = DurableRawBatch {
            batch: Arc::clone(&self.pending_batch),
            authority: Arc::clone(&self.authority),
            entries,
            frame_bytes: self.pending_frame_bytes,
        };
        self.pending_batch = Arc::new(RawBatchIdentity);
        self.pending_records = 0;
        self.pending_frame_bytes = 0;
        Ok(receipt)
    }

    pub fn sync(&mut self) -> io::Result<()> {
        self.durable_barrier().map(|_| ())
    }

    pub fn state(&self) -> ArchiveWriterState {
        if self.poisoned.is_some() {
            ArchiveWriterState::Poisoned
        } else {
            ArchiveWriterState::Ready
        }
    }

    pub fn pending_raw(&self) -> (u64, u64) {
        (self.pending_records, self.pending_frame_bytes)
    }

    pub fn file_dirty(&self) -> bool {
        self.file_dirty
    }

    pub fn namespace_sync_pending(&self) -> bool {
        self.namespace_sync_pending
    }

    fn ensure_ready(&self) -> io::Result<()> {
        if let Some(reason) = &self.poisoned {
            return Err(io::Error::other(format!(
                "archive writer is poisoned: {reason}"
            )));
        }
        Ok(())
    }

    fn poison(&mut self, error: io::Error) -> io::Error {
        if self.poisoned.is_none() {
            self.poisoned = Some(error.to_string());
        }
        error
    }

    fn write_frame_component(
        &mut self,
        component: ArchiveWriterWriteComponent,
        bytes: &[u8],
    ) -> io::Result<()> {
        self.file_dirty = true;
        #[cfg(not(test))]
        let _ = component;
        #[cfg(test)]
        if self.fault.fail_component == Some(component) {
            if component == ArchiveWriterWriteComponent::Payload && self.fault.partial_payload {
                let prefix_len = bytes.len().min(1);
                self.file
                    .write_all(&bytes[..prefix_len])
                    .map_err(|error| self.poison(error))?;
            }
            return Err(self.poison(io::Error::other(format!(
                "injected failure during {component:?}"
            ))));
        }
        self.file
            .write_all(bytes)
            .map_err(|error| self.poison(error))
    }

    fn sync_current_file(&mut self) -> io::Result<()> {
        #[cfg(test)]
        {
            self.fault.sync_calls += 1;
            if self.fault.fail_sync_remaining > 0 {
                self.fault.fail_sync_remaining -= 1;
                return Err(io::Error::other("injected segment sync failure"));
            }
        }
        self.file.sync_all()?;
        self.file_dirty = false;
        Ok(())
    }

    fn sync_namespace(&mut self) -> io::Result<()> {
        #[cfg(test)]
        {
            self.fault.namespace_sync_calls += 1;
            self.fault.namespace_sync_paths.push(self.root.clone());
            if self.fault.fail_namespace_remaining > 0 {
                self.fault.fail_namespace_remaining -= 1;
                return Err(io::Error::other("injected namespace sync failure"));
            }
        }
        sync_archive_namespace(&self.root)
    }
}

fn sync_archive_namespace(path: &Path) -> io::Result<()> {
    #[cfg(unix)]
    {
        File::open(path)?.sync_all()
    }
    #[cfg(not(unix))]
    {
        let _ = path;
        Ok(())
    }
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
/// [`read_record`], the physical frame validator. This inventory path is
/// diagnostic; it is not the catalogue-linked authoritative reader.
pub fn inventory_records(root: &Path) -> io::Result<Vec<RecordInventory>> {
    validate_existing_catalogue(&root.join("metadata.sqlite")).map_err(sqlite_io)?;
    let catalog = Connection::open_with_flags(
        root.join("metadata.sqlite"),
        OpenFlags::SQLITE_OPEN_READ_ONLY,
    )
    .map_err(sqlite_io)?;
    let mut statement = catalog
        .prepare(
            "SELECT doc_id, segment, archive_offset, frame_bytes, raw_blake3 FROM messages ORDER BY doc_id",
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
                row.get::<_, Vec<u8>>(4).map_err(|error| error.to_string()),
            ))
        })
        .map_err(sqlite_io)?;
    let mut inventory = Vec::new();
    for row in rows {
        let (doc_id, segment, offset, frame_bytes, raw_blake3) = match row {
            Ok(value) => value,
            // A row whose primary key cannot be decoded cannot be represented
            // as a RecordInventory without inventing an identifier.
            Err(error) => return Err(sqlite_io(error)),
        };
        let raw_blake3 = match raw_blake3 {
            Ok(value) => value,
            Err(error) => {
                inventory.push(inventory_inconsistent(
                    doc_id,
                    None,
                    format!("catalogue BLAKE3 is invalid: {error}"),
                ));
                continue;
            }
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
                    None,
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
                    None,
                    format!("catalog frame length is invalid: {error}"),
                ));
                continue;
            }
        };
        if offset < 0 || frame_bytes < 0 {
            inventory.push(inventory_inconsistent(
                doc_id,
                None,
                "negative catalogue archive coordinate",
            ));
            continue;
        }
        let (offset, frame_bytes) = match (u64::try_from(offset), u64::try_from(frame_bytes)) {
            (Ok(offset), Ok(frame_bytes)) => (offset, frame_bytes),
            _ => {
                inventory.push(inventory_inconsistent(
                    doc_id,
                    None,
                    "catalogue archive coordinate does not fit in an archive coordinate",
                ));
                continue;
            }
        };
        let location = ArchiveLocation {
            segment,
            offset,
            frame_bytes,
        };
        if doc_id < 0 {
            inventory.push(inventory_inconsistent(
                doc_id,
                Some(location),
                "negative catalogue record id",
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
            Ok((record_id, raw)) if record_id == doc_id as u64 => {
                let digest_matches = raw_blake3.len() == 32
                    && raw_blake3.as_slice() == blake3::hash(&raw).as_bytes();
                if digest_matches {
                    inventory.push(RecordInventory {
                        doc_id,
                        location: Some(location),
                        status: RecordInventoryStatus::AvailableValidated,
                    });
                } else {
                    inventory.push(inventory_inconsistent(
                        doc_id,
                        Some(location),
                        "catalogue/RAW BLAKE3 linkage mismatch",
                    ));
                }
            }
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

type CatalogueFrameClaims = (HashSet<(String, u64)>, HashSet<RawReference>);

fn catalogue_frame_sets(root: &Path) -> io::Result<CatalogueFrameClaims> {
    validate_existing_catalogue(&root.join("metadata.sqlite")).map_err(sqlite_io)?;
    let catalog = Connection::open_with_flags(
        root.join("metadata.sqlite"),
        OpenFlags::SQLITE_OPEN_READ_ONLY,
    )
    .map_err(sqlite_io)?;
    let mut statement = catalog
        .prepare("SELECT doc_id,segment,archive_offset,frame_bytes,raw_blake3 FROM messages")
        .map_err(sqlite_io)?;
    let rows = statement
        .query_map([], |row| {
            Ok((
                row.get::<_, i64>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, i64>(2)?,
                row.get::<_, i64>(3)?,
                row.get::<_, Vec<u8>>(4)?,
            ))
        })
        .map_err(sqlite_io)?;
    // Segment + offset is the minimum physical claim key. frame_bytes is
    // intentionally excluded so a malformed catalogue length cannot make the
    // real frame look like an orphan.
    let mut locations = HashSet::new();
    let mut references = HashSet::new();
    for row in rows {
        let (doc_id, segment, offset, frame_bytes, digest) = row.map_err(sqlite_io)?;
        if archive_segment_path(&root.join("archive"), &segment).is_err() || offset < 0 {
            continue;
        }
        // Establish the physical claim before validating any authoritative
        // attribute. A negative doc_id/frame_bytes still claims this segment
        // and offset and must prevent the candidate from becoming an orphan.
        let physical_claim = (segment.clone(), offset as u64);
        locations.insert(physical_claim);
        if doc_id < 0 || frame_bytes < 0 {
            continue;
        }
        let location = ArchiveLocation {
            segment: segment.clone(),
            offset: offset as u64,
            frame_bytes: frame_bytes as u64,
        };
        if digest.len() == 32 {
            let mut blake3 = [0u8; 32];
            blake3.copy_from_slice(&digest);
            references.insert(RawReference {
                doc_id: doc_id as u64,
                location,
                blake3,
            });
        }
    }
    Ok((locations, references))
}

/// Scan every named archive segment without changing any archive or catalogue
/// file. The scanner advances only over structurally bounded frames; after a
/// bad magic or an unsafe length it stops that segment instead of guessing a
/// new boundary. A valid frame is catalogued only on an exact physical
/// identity match, including its BLAKE3 digest.
fn allocate_frame_body(body_len: u64) -> io::Result<Vec<u8>> {
    let body_len = usize::try_from(body_len).map_err(|_| {
        io::Error::new(
            io::ErrorKind::InvalidData,
            "archive frame body length does not fit in memory",
        )
    })?;
    let mut body = Vec::new();
    body.try_reserve_exact(body_len)
        .map_err(|error| io::Error::other(format!("archive frame allocation failed: {error}")))?;
    body.resize(body_len, 0);
    Ok(body)
}

pub fn inventory_physical(root: &Path) -> io::Result<PhysicalInventory> {
    let catalogue_path = root.join("metadata.sqlite");
    let catalogue_present = match fs::metadata(&catalogue_path) {
        Ok(_) => true,
        Err(error) if error.kind() == io::ErrorKind::NotFound => false,
        Err(error) => return Err(error),
    };
    let (catalogue_locations, catalogue_references, catalogue_records) = if catalogue_present {
        let (locations, references) = catalogue_frame_sets(root)?;
        (locations, references, inventory_records(root)?)
    } else {
        // A missing catalogue cannot establish any physical claim.  The
        // scanner remains useful as an unlinked RAW salvage inventory.
        (HashSet::new(), HashSet::new(), Vec::new())
    };
    let mut result = PhysicalInventory {
        catalogued_records: catalogue_records.len() as u64,
        validated_catalogued_records: catalogue_records
            .iter()
            .filter(|record| matches!(record.status, RecordInventoryStatus::AvailableValidated))
            .count() as u64,
        inconsistent_catalogued_records: catalogue_records
            .iter()
            .filter(|record| matches!(record.status, RecordInventoryStatus::Inconsistent { .. }))
            .count() as u64,
        catalogued_physically_missing: catalogue_records
            .iter()
            .filter(|record| matches!(record.status, RecordInventoryStatus::PhysicallyMissing))
            .count() as u64,
        ..Default::default()
    };
    let archive_root = root.join("archive");
    let mut paths = fs::read_dir(&archive_root)
        .map_err(|error| {
            if error.kind() == io::ErrorKind::NotFound {
                io::Error::new(io::ErrorKind::NotFound, "archive directory is missing")
            } else {
                error
            }
        })?
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| path.extension().and_then(|value| value.to_str()) == Some("arc"))
        .collect::<Vec<_>>();
    paths.sort();

    for path in paths {
        let segment = path
            .file_name()
            .and_then(|value| value.to_str())
            .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "invalid segment name"))?
            .to_string();
        archive_segment_path(&archive_root, &segment)?;
        let file_len = fs::metadata(&path)?.len();
        let mut file = File::open(&path)?;
        let mut offset = 0u64;
        while offset < file_len {
            let remaining = file_len - offset;
            let mut header = [0u8; 32];
            if remaining < FRAME_HEADER_BYTES {
                result.incomplete_tails += 1;
                result.frames.push(PhysicalFrameInventory {
                    location: ArchiveLocation {
                        segment: segment.clone(),
                        offset,
                        frame_bytes: remaining,
                    },
                    doc_id: None,
                    blake3: None,
                    status: PhysicalFrameStatus::IncompleteTail {
                        reason: "fewer than 32 bytes remain for a frame header".into(),
                    },
                });
                break;
            }
            file.read_exact(&mut header)?;
            if &header[..8] != FRAME_MAGIC {
                result.physical_corruptions += 1;
                result.frames.push(PhysicalFrameInventory {
                    location: ArchiveLocation {
                        segment: segment.clone(),
                        offset,
                        frame_bytes: remaining,
                    },
                    doc_id: None,
                    blake3: None,
                    status: PhysicalFrameStatus::PhysicalCorruption {
                        reason: "frame magic mismatch; no safe resynchronization boundary".into(),
                    },
                });
                break;
            }
            let doc_id = u64::from_le_bytes(header[8..16].try_into().unwrap());
            let body_len = u64::from_le_bytes(header[16..24].try_into().unwrap());
            let checksum = u64::from_le_bytes(header[24..32].try_into().unwrap());
            let frame_bytes = match FRAME_HEADER_BYTES.checked_add(body_len) {
                Some(value) => value,
                None => {
                    result.physical_corruptions += 1;
                    result.frames.push(PhysicalFrameInventory {
                        location: ArchiveLocation {
                            segment: segment.clone(),
                            offset,
                            frame_bytes: remaining,
                        },
                        doc_id: Some(doc_id),
                        blake3: None,
                        status: PhysicalFrameStatus::PhysicalCorruption {
                            reason: "frame length overflows the archive coordinate".into(),
                        },
                    });
                    break;
                }
            };
            if frame_bytes > remaining {
                result.physical_corruptions += 1;
                result.frames.push(PhysicalFrameInventory {
                    location: ArchiveLocation {
                        segment: segment.clone(),
                        offset,
                        frame_bytes: remaining,
                    },
                    doc_id: Some(doc_id),
                    blake3: None,
                    status: PhysicalFrameStatus::PhysicalCorruption {
                        reason: "declared body extends past EOF; length is unauthenticated".into(),
                    },
                });
                break;
            }
            let mut body = allocate_frame_body(body_len)?;
            file.read_exact(&mut body)?;
            let location = ArchiveLocation {
                segment: segment.clone(),
                offset,
                frame_bytes,
            };
            offset += frame_bytes;
            if fnv64(&body) != checksum {
                result.physical_corruptions += 1;
                result.frames.push(PhysicalFrameInventory {
                    location,
                    doc_id: Some(doc_id),
                    blake3: None,
                    status: PhysicalFrameStatus::PhysicalCorruption {
                        reason: "frame checksum mismatch".into(),
                    },
                });
                break;
            }
            let digest = *blake3::hash(&body).as_bytes();
            let reference = RawReference {
                doc_id,
                location: location.clone(),
                blake3: digest,
            };
            let status = if catalogue_references.contains(&reference) {
                PhysicalFrameStatus::CataloguedValidated
            } else if catalogue_locations.contains(&(location.segment.clone(), location.offset)) {
                PhysicalFrameStatus::CataloguedInconsistent
            } else {
                result.orphan_valid_frames += 1;
                PhysicalFrameStatus::OrphanValidated
            };
            result.frames.push(PhysicalFrameInventory {
                location,
                doc_id: Some(doc_id),
                blake3: Some(digest),
                status,
            });
        }
    }
    Ok(result)
}

/// Validate and read one physical frame by coordinates.
///
/// This low-level primitive has no catalogue/BLAKE3 binding and is therefore
/// not an authoritative catalogue read. Use [`read_archived_raw`] for that.
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

pub(crate) fn validate_catalog_record(
    archive_root: &Path,
    connection: &Connection,
    doc_id: i64,
    canonical_message_id: &str,
) -> io::Result<()> {
    let row = connection
        .query_row(
            "SELECT message_id,segment,archive_offset,frame_bytes,raw_blake3 FROM messages WHERE doc_id=?1",
            [doc_id],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, i64>(2)?,
                    row.get::<_, i64>(3)?,
                    row.get::<_, Vec<u8>>(4)?,
                ))
            },
        )
        .optional()
        .map_err(sqlite_io)?
        .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "catalogue messages row missing"))?;
    if row.0 != canonical_message_id {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "catalogue source message_id mismatch",
        ));
    }
    if row.2 < 0 || row.3 < 0 {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "catalogue archive coordinate is negative",
        ));
    }
    let reference = raw_reference_from_catalog_values(doc_id, row.1, row.2, row.3, row.4)?;
    read_authoritative_raw(archive_root, &reference)?;
    Ok(())
}

fn raw_reference_from_catalog_values(
    doc_id: i64,
    segment: String,
    offset: i64,
    frame_bytes: i64,
    raw_blake3: Vec<u8>,
) -> io::Result<RawReference> {
    if doc_id < 0 {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "negative catalogue document ID",
        ));
    }
    if offset < 0 {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "negative archive offset",
        ));
    }
    if frame_bytes < 0 {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "negative archive frame length",
        ));
    }
    if raw_blake3.len() != 32 {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "catalogue raw BLAKE3 length is not 32 bytes",
        ));
    }
    let mut blake3 = [0u8; 32];
    blake3.copy_from_slice(&raw_blake3);
    Ok(RawReference {
        doc_id: doc_id as u64,
        location: ArchiveLocation {
            segment,
            offset: offset as u64,
            frame_bytes: frame_bytes as u64,
        },
        blake3,
    })
}

pub(crate) fn read_catalogue_raw(
    archive_root: &Path,
    connection: &Connection,
    doc_id: u64,
) -> io::Result<Vec<u8>> {
    let catalog_id = i64::try_from(doc_id).map_err(|_| {
        io::Error::new(
            io::ErrorKind::InvalidInput,
            "archive record id does not fit in the catalog",
        )
    })?;
    let (segment, offset, frame_bytes, raw_blake3) = connection
        .query_row(
            "SELECT segment,archive_offset,frame_bytes,raw_blake3 FROM messages WHERE doc_id=?1",
            [catalog_id],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, i64>(1)?,
                    row.get::<_, i64>(2)?,
                    row.get::<_, Vec<u8>>(3)?,
                ))
            },
        )
        .map_err(sqlite_io)?;
    let reference =
        raw_reference_from_catalog_values(catalog_id, segment, offset, frame_bytes, raw_blake3)?;
    read_authoritative_raw(archive_root, &reference)
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
    let metadata = open_catalogue(&root.join("metadata.sqlite")).map_err(sqlite_io)?;
    let mut statement = metadata
        .prepare("SELECT doc_id, segment, archive_offset, frame_bytes, raw_blake3 FROM messages WHERE doc_id >= ?1 AND doc_id < ?2 ORDER BY doc_id")
        .map_err(sqlite_io)?;
    let rows = statement
        .query_map(params![start_id as i64, end_id as i64], |row| {
            Ok((
                row.get::<_, i64>(0)? as u64,
                row.get::<_, String>(1)?,
                row.get::<_, i64>(2)?,
                row.get::<_, i64>(3)?,
                row.get::<_, Vec<u8>>(4)?,
            ))
        })
        .map_err(sqlite_io)?;
    let mut stats = PipelineStats::default();
    for row in rows {
        let (id, segment, offset, frame_bytes, digest) = row.map_err(sqlite_io)?;
        let reference =
            raw_reference_from_catalog_values(id as i64, segment, offset, frame_bytes, digest)?;
        let read_started = Instant::now();
        let raw = read_authoritative_raw(&root.join("archive"), &reference)?;
        stats.read_us += read_started.elapsed().as_micros();
        let parse_started = Instant::now();
        let message = parse_archived_message(id, &raw)?;
        stats.parse_us += parse_started.elapsed().as_micros();
        callback(&message)?;
        stats.messages += 1;
    }
    Ok(stats)
}

fn catalogue_inconsistent(reason: impl Into<String>) -> rusqlite::Error {
    rusqlite::Error::InvalidParameterName(format!("CatalogInconsistent: {}", reason.into()))
}

const CATALOGUE_SCHEMA: &str = "
CREATE TABLE messages (doc_id INTEGER PRIMARY KEY, message_id TEXT NOT NULL UNIQUE, timestamp INTEGER NOT NULL, sender TEXT NOT NULL, recipients TEXT NOT NULL, subject TEXT NOT NULL, account TEXT NOT NULL, folder TEXT NOT NULL, thread TEXT NOT NULL, segment TEXT NOT NULL, archive_offset INTEGER NOT NULL, frame_bytes INTEGER NOT NULL, raw_blake3 BLOB NOT NULL CHECK(length(raw_blake3)=32));
CREATE TABLE attachments (doc_id INTEGER NOT NULL, filename TEXT NOT NULL, mime TEXT NOT NULL, bytes INTEGER NOT NULL, content_hash TEXT NOT NULL, PRIMARY KEY(doc_id, filename));
CREATE INDEX messages_timestamp ON messages(timestamp);
CREATE INDEX messages_sender ON messages(sender);
CREATE INDEX messages_folder ON messages(folder);
CREATE TABLE gmail_state (source_account TEXT PRIMARY KEY, history_id TEXT NOT NULL, complete INTEGER NOT NULL DEFAULT 0);
CREATE TABLE gmail_messages (source_account TEXT NOT NULL, gmail_message_id TEXT NOT NULL, doc_id INTEGER NOT NULL UNIQUE, thread_id TEXT NOT NULL, label_ids TEXT NOT NULL, internal_date_ms INTEGER, message_history_id TEXT, source_state TEXT NOT NULL, first_seen_unix INTEGER NOT NULL, last_seen_unix INTEGER NOT NULL, PRIMARY KEY(source_account, gmail_message_id));
CREATE INDEX gmail_messages_state ON gmail_messages(source_account, source_state);
CREATE TABLE imap_messages (source_account TEXT NOT NULL, mailbox TEXT NOT NULL, uid_validity INTEGER NOT NULL, uid INTEGER NOT NULL, doc_id INTEGER NOT NULL UNIQUE, flags TEXT NOT NULL, internal_date TEXT, internal_date_ms INTEGER, rfc822_size INTEGER, source_state TEXT NOT NULL, first_seen_unix INTEGER NOT NULL, last_seen_unix INTEGER NOT NULL, PRIMARY KEY(source_account, mailbox, uid_validity, uid));
CREATE INDEX imap_messages_state ON imap_messages(source_account, source_state);
CREATE TABLE imap_scan_state (source_account TEXT NOT NULL, mailbox TEXT NOT NULL, uid_validity INTEGER NOT NULL, scanned_through_uid INTEGER NOT NULL, last_uid_next INTEGER NOT NULL, updated_unix INTEGER NOT NULL, PRIMARY KEY(source_account, mailbox, uid_validity));
CREATE TABLE imap_mailboxes (source_account TEXT NOT NULL, mailbox TEXT NOT NULL, delimiter TEXT, attributes TEXT NOT NULL, special_use TEXT NOT NULL, selectable INTEGER NOT NULL, last_seen_unix INTEGER NOT NULL, PRIMARY KEY(source_account, mailbox));
";

fn create_catalogue_with_authority(
    path: &Path,
    authority: Arc<ArchiveAuthority>,
) -> rusqlite::Result<CatalogueConnection> {
    if path.exists() {
        return Err(rusqlite::Error::InvalidPath(path.to_path_buf()));
    }
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).ok();
    }
    let mut connection = Connection::open(path)?;
    let transaction = connection.transaction()?;
    transaction.execute_batch(CATALOGUE_SCHEMA)?;
    transaction.commit()?;
    // The schema is complete before either marker can make this file look like
    // a Memoria catalogue. A failure leaves an explicitly invalid version 0
    // file, never a partially constructed valid v1 catalogue.
    connection.execute_batch(&format!(
        "PRAGMA application_id={MEMORIA_CATALOGUE_APPLICATION_ID}; PRAGMA user_version={MEMORIA_CATALOGUE_VERSION}; PRAGMA journal_mode=DELETE; PRAGMA synchronous=EXTRA;"
    ))?;
    Ok(CatalogueConnection {
        connection,
        authority,
    })
}

#[cfg(test)]
pub(crate) fn create_catalogue(path: &Path) -> rusqlite::Result<CatalogueConnection> {
    create_catalogue_with_authority(path, Arc::new(ArchiveAuthority { _lock: None }))
}

#[derive(Clone, Copy)]
struct ExpectedColumn {
    name: &'static str,
    declared_type: &'static str,
    not_null: bool,
    default: Option<&'static str>,
    primary_key: i64,
}

#[derive(Clone, Copy)]
struct ExpectedIndex {
    name: &'static str,
    columns: &'static [&'static str],
    unique: bool,
    partial: bool,
}

#[derive(Debug)]
struct ActualColumn {
    cid: i64,
    name: String,
    declared_type: String,
    not_null: bool,
    default: Option<String>,
    primary_key: i64,
    hidden: i64,
}

#[derive(Debug)]
struct ActualIndex {
    name: String,
    unique: bool,
    origin: String,
    partial: bool,
}

fn table_columns(connection: &Connection, table: &str) -> rusqlite::Result<Vec<ActualColumn>> {
    connection
        .prepare(&format!("PRAGMA table_xinfo({table})"))?
        .query_map([], |row| {
            Ok(ActualColumn {
                cid: row.get(0)?,
                name: row.get(1)?,
                declared_type: row.get(2)?,
                not_null: row.get::<_, i64>(3)? != 0,
                default: row.get(4)?,
                primary_key: row.get(5)?,
                hidden: row.get(6)?,
            })
        })?
        .collect()
}

fn validate_table(
    connection: &Connection,
    table: &str,
    expected: &[ExpectedColumn],
) -> rusqlite::Result<()> {
    let actual = table_columns(connection, table)?;
    if actual.is_empty() {
        return Err(catalogue_inconsistent(format!("missing table {table}")));
    }
    if actual.len() != expected.len() {
        return Err(catalogue_inconsistent(format!(
            "invalid column count for {table}"
        )));
    }
    for (position, (column, wanted)) in actual.iter().zip(expected).enumerate() {
        if column.hidden != 0
            || column.cid != position as i64
            || column.name != wanted.name
            || !column
                .declared_type
                .eq_ignore_ascii_case(wanted.declared_type)
            || column.not_null != wanted.not_null
            || column.default.as_deref().map(str::trim) != wanted.default
            || column.primary_key != wanted.primary_key
        {
            return Err(catalogue_inconsistent(format!(
                "invalid {table} column {}",
                wanted.name
            )));
        }
    }
    Ok(())
}

fn table_indexes(connection: &Connection, table: &str) -> rusqlite::Result<Vec<ActualIndex>> {
    connection
        .prepare(&format!("PRAGMA index_list({table})"))?
        .query_map([], |row| {
            Ok(ActualIndex {
                name: row.get(1)?,
                unique: row.get::<_, i64>(2)? != 0,
                origin: row.get(3)?,
                partial: row.get::<_, i64>(4)? != 0,
            })
        })?
        .collect()
}

fn index_columns(connection: &Connection, name: &str) -> rusqlite::Result<Vec<String>> {
    connection
        .prepare(&format!("PRAGMA index_info({name})"))?
        .query_map([], |row| row.get::<_, String>(2))?
        .collect()
}

fn validate_indexes(
    connection: &Connection,
    table: &str,
    expected: &[ExpectedIndex],
    allowed_unique: &[&[&str]],
    required_unique: &[&[&str]],
) -> rusqlite::Result<()> {
    let actual = table_indexes(connection, table)?;
    for index in &actual {
        let columns = index_columns(connection, &index.name)?;
        match index.origin.as_str() {
            "c" => {
                let Some(wanted) = expected.iter().find(|wanted| wanted.name == index.name) else {
                    return Err(catalogue_inconsistent(format!(
                        "unexpected index {}",
                        index.name
                    )));
                };
                if index.unique != wanted.unique
                    || index.partial != wanted.partial
                    || columns
                        .iter()
                        .map(String::as_str)
                        .ne(wanted.columns.iter().copied())
                {
                    return Err(catalogue_inconsistent(format!(
                        "invalid index {}",
                        index.name
                    )));
                }
            }
            "u" | "pk" => {
                if !index.unique
                    || index.partial
                    || !allowed_unique.iter().any(|wanted| {
                        columns
                            .iter()
                            .map(String::as_str)
                            .eq(wanted.iter().copied())
                    })
                {
                    return Err(catalogue_inconsistent(format!(
                        "invalid unique index {}",
                        index.name
                    )));
                }
            }
            origin => {
                return Err(catalogue_inconsistent(format!(
                    "unsupported index origin {origin}"
                )));
            }
        }
    }
    for wanted in expected {
        if !actual.iter().any(|index| index.name == wanted.name) {
            return Err(catalogue_inconsistent(format!(
                "missing index {}",
                wanted.name
            )));
        }
    }
    for wanted in required_unique {
        let present = actual.iter().any(|index| {
            index.origin == "u"
                && index.unique
                && !index.partial
                && index_columns(connection, &index.name)
                    .map(|columns| {
                        columns
                            .iter()
                            .map(String::as_str)
                            .eq(wanted.iter().copied())
                    })
                    .unwrap_or(false)
        });
        if !present {
            return Err(catalogue_inconsistent(format!(
                "missing UNIQUE constraint on {table}"
            )));
        }
    }
    Ok(())
}

fn validate_messages_schema_sql(connection: &Connection) -> rusqlite::Result<()> {
    // The v1 catalogue is an internal format: SQLite's own canonical rendering
    // of Memoria's reference DDL is the exact contract for this table. The
    // existing catalogue is read-only; only the independent memory probe is
    // created and written.
    let reference = Connection::open_in_memory()?;
    reference.execute_batch(CATALOGUE_SCHEMA)?;
    let expected: String = reference.query_row(
        "SELECT sql FROM sqlite_schema WHERE type='table' AND name='messages'",
        [],
        |row| row.get(0),
    )?;
    let actual: String = connection.query_row(
        "SELECT sql FROM sqlite_schema WHERE type='table' AND name='messages'",
        [],
        |row| row.get(0),
    )?;
    if actual != expected {
        return Err(catalogue_inconsistent(
            "messages DDL differs from canonical v1 DDL",
        ));
    }
    Ok(())
}

fn validate_catalogue_schema(connection: &Connection) -> rusqlite::Result<()> {
    let expected_tables: HashSet<&str> = [
        "messages",
        "attachments",
        "gmail_state",
        "gmail_messages",
        "imap_messages",
        "imap_scan_state",
        "imap_mailboxes",
    ]
    .into_iter()
    .collect();
    let actual_tables: HashSet<String> = connection
        .prepare("PRAGMA table_list")?
        .query_map([], |row| {
            Ok((row.get::<_, String>(1)?, row.get::<_, String>(2)?))
        })?
        .filter_map(|row| match row {
            Ok((name, kind)) if !name.starts_with("sqlite_") && kind == "table" => Some(Ok(name)),
            Ok((name, _)) if !name.starts_with("sqlite_") => Some(Err(catalogue_inconsistent(
                format!("unexpected non-table object {name}"),
            ))),
            Ok(_) => None,
            Err(error) => Some(Err(error)),
        })
        .collect::<rusqlite::Result<_>>()?;
    if actual_tables.len() != expected_tables.len()
        || expected_tables
            .iter()
            .any(|table| !actual_tables.contains(*table))
    {
        return Err(catalogue_inconsistent("catalogue table set mismatch"));
    }
    validate_table(
        connection,
        "messages",
        &[
            ExpectedColumn {
                name: "doc_id",
                declared_type: "INTEGER",
                not_null: false,
                default: None,
                primary_key: 1,
            },
            ExpectedColumn {
                name: "message_id",
                declared_type: "TEXT",
                not_null: true,
                default: None,
                primary_key: 0,
            },
            ExpectedColumn {
                name: "timestamp",
                declared_type: "INTEGER",
                not_null: true,
                default: None,
                primary_key: 0,
            },
            ExpectedColumn {
                name: "sender",
                declared_type: "TEXT",
                not_null: true,
                default: None,
                primary_key: 0,
            },
            ExpectedColumn {
                name: "recipients",
                declared_type: "TEXT",
                not_null: true,
                default: None,
                primary_key: 0,
            },
            ExpectedColumn {
                name: "subject",
                declared_type: "TEXT",
                not_null: true,
                default: None,
                primary_key: 0,
            },
            ExpectedColumn {
                name: "account",
                declared_type: "TEXT",
                not_null: true,
                default: None,
                primary_key: 0,
            },
            ExpectedColumn {
                name: "folder",
                declared_type: "TEXT",
                not_null: true,
                default: None,
                primary_key: 0,
            },
            ExpectedColumn {
                name: "thread",
                declared_type: "TEXT",
                not_null: true,
                default: None,
                primary_key: 0,
            },
            ExpectedColumn {
                name: "segment",
                declared_type: "TEXT",
                not_null: true,
                default: None,
                primary_key: 0,
            },
            ExpectedColumn {
                name: "archive_offset",
                declared_type: "INTEGER",
                not_null: true,
                default: None,
                primary_key: 0,
            },
            ExpectedColumn {
                name: "frame_bytes",
                declared_type: "INTEGER",
                not_null: true,
                default: None,
                primary_key: 0,
            },
            ExpectedColumn {
                name: "raw_blake3",
                declared_type: "BLOB",
                not_null: true,
                default: None,
                primary_key: 0,
            },
        ],
    )?;
    validate_table(
        connection,
        "attachments",
        &[
            ExpectedColumn {
                name: "doc_id",
                declared_type: "INTEGER",
                not_null: true,
                default: None,
                primary_key: 1,
            },
            ExpectedColumn {
                name: "filename",
                declared_type: "TEXT",
                not_null: true,
                default: None,
                primary_key: 2,
            },
            ExpectedColumn {
                name: "mime",
                declared_type: "TEXT",
                not_null: true,
                default: None,
                primary_key: 0,
            },
            ExpectedColumn {
                name: "bytes",
                declared_type: "INTEGER",
                not_null: true,
                default: None,
                primary_key: 0,
            },
            ExpectedColumn {
                name: "content_hash",
                declared_type: "TEXT",
                not_null: true,
                default: None,
                primary_key: 0,
            },
        ],
    )?;
    validate_table(
        connection,
        "gmail_state",
        &[
            ExpectedColumn {
                name: "source_account",
                declared_type: "TEXT",
                not_null: false,
                default: None,
                primary_key: 1,
            },
            ExpectedColumn {
                name: "history_id",
                declared_type: "TEXT",
                not_null: true,
                default: None,
                primary_key: 0,
            },
            ExpectedColumn {
                name: "complete",
                declared_type: "INTEGER",
                not_null: true,
                default: Some("0"),
                primary_key: 0,
            },
        ],
    )?;
    validate_table(
        connection,
        "gmail_messages",
        &[
            ExpectedColumn {
                name: "source_account",
                declared_type: "TEXT",
                not_null: true,
                default: None,
                primary_key: 1,
            },
            ExpectedColumn {
                name: "gmail_message_id",
                declared_type: "TEXT",
                not_null: true,
                default: None,
                primary_key: 2,
            },
            ExpectedColumn {
                name: "doc_id",
                declared_type: "INTEGER",
                not_null: true,
                default: None,
                primary_key: 0,
            },
            ExpectedColumn {
                name: "thread_id",
                declared_type: "TEXT",
                not_null: true,
                default: None,
                primary_key: 0,
            },
            ExpectedColumn {
                name: "label_ids",
                declared_type: "TEXT",
                not_null: true,
                default: None,
                primary_key: 0,
            },
            ExpectedColumn {
                name: "internal_date_ms",
                declared_type: "INTEGER",
                not_null: false,
                default: None,
                primary_key: 0,
            },
            ExpectedColumn {
                name: "message_history_id",
                declared_type: "TEXT",
                not_null: false,
                default: None,
                primary_key: 0,
            },
            ExpectedColumn {
                name: "source_state",
                declared_type: "TEXT",
                not_null: true,
                default: None,
                primary_key: 0,
            },
            ExpectedColumn {
                name: "first_seen_unix",
                declared_type: "INTEGER",
                not_null: true,
                default: None,
                primary_key: 0,
            },
            ExpectedColumn {
                name: "last_seen_unix",
                declared_type: "INTEGER",
                not_null: true,
                default: None,
                primary_key: 0,
            },
        ],
    )?;
    validate_table(
        connection,
        "imap_messages",
        &[
            ExpectedColumn {
                name: "source_account",
                declared_type: "TEXT",
                not_null: true,
                default: None,
                primary_key: 1,
            },
            ExpectedColumn {
                name: "mailbox",
                declared_type: "TEXT",
                not_null: true,
                default: None,
                primary_key: 2,
            },
            ExpectedColumn {
                name: "uid_validity",
                declared_type: "INTEGER",
                not_null: true,
                default: None,
                primary_key: 3,
            },
            ExpectedColumn {
                name: "uid",
                declared_type: "INTEGER",
                not_null: true,
                default: None,
                primary_key: 4,
            },
            ExpectedColumn {
                name: "doc_id",
                declared_type: "INTEGER",
                not_null: true,
                default: None,
                primary_key: 0,
            },
            ExpectedColumn {
                name: "flags",
                declared_type: "TEXT",
                not_null: true,
                default: None,
                primary_key: 0,
            },
            ExpectedColumn {
                name: "internal_date",
                declared_type: "TEXT",
                not_null: false,
                default: None,
                primary_key: 0,
            },
            ExpectedColumn {
                name: "internal_date_ms",
                declared_type: "INTEGER",
                not_null: false,
                default: None,
                primary_key: 0,
            },
            ExpectedColumn {
                name: "rfc822_size",
                declared_type: "INTEGER",
                not_null: false,
                default: None,
                primary_key: 0,
            },
            ExpectedColumn {
                name: "source_state",
                declared_type: "TEXT",
                not_null: true,
                default: None,
                primary_key: 0,
            },
            ExpectedColumn {
                name: "first_seen_unix",
                declared_type: "INTEGER",
                not_null: true,
                default: None,
                primary_key: 0,
            },
            ExpectedColumn {
                name: "last_seen_unix",
                declared_type: "INTEGER",
                not_null: true,
                default: None,
                primary_key: 0,
            },
        ],
    )?;
    validate_table(
        connection,
        "imap_scan_state",
        &[
            ExpectedColumn {
                name: "source_account",
                declared_type: "TEXT",
                not_null: true,
                default: None,
                primary_key: 1,
            },
            ExpectedColumn {
                name: "mailbox",
                declared_type: "TEXT",
                not_null: true,
                default: None,
                primary_key: 2,
            },
            ExpectedColumn {
                name: "uid_validity",
                declared_type: "INTEGER",
                not_null: true,
                default: None,
                primary_key: 3,
            },
            ExpectedColumn {
                name: "scanned_through_uid",
                declared_type: "INTEGER",
                not_null: true,
                default: None,
                primary_key: 0,
            },
            ExpectedColumn {
                name: "last_uid_next",
                declared_type: "INTEGER",
                not_null: true,
                default: None,
                primary_key: 0,
            },
            ExpectedColumn {
                name: "updated_unix",
                declared_type: "INTEGER",
                not_null: true,
                default: None,
                primary_key: 0,
            },
        ],
    )?;
    validate_table(
        connection,
        "imap_mailboxes",
        &[
            ExpectedColumn {
                name: "source_account",
                declared_type: "TEXT",
                not_null: true,
                default: None,
                primary_key: 1,
            },
            ExpectedColumn {
                name: "mailbox",
                declared_type: "TEXT",
                not_null: true,
                default: None,
                primary_key: 2,
            },
            ExpectedColumn {
                name: "delimiter",
                declared_type: "TEXT",
                not_null: false,
                default: None,
                primary_key: 0,
            },
            ExpectedColumn {
                name: "attributes",
                declared_type: "TEXT",
                not_null: true,
                default: None,
                primary_key: 0,
            },
            ExpectedColumn {
                name: "special_use",
                declared_type: "TEXT",
                not_null: true,
                default: None,
                primary_key: 0,
            },
            ExpectedColumn {
                name: "selectable",
                declared_type: "INTEGER",
                not_null: true,
                default: None,
                primary_key: 0,
            },
            ExpectedColumn {
                name: "last_seen_unix",
                declared_type: "INTEGER",
                not_null: true,
                default: None,
                primary_key: 0,
            },
        ],
    )?;
    validate_indexes(
        connection,
        "messages",
        &[
            ExpectedIndex {
                name: "messages_timestamp",
                columns: &["timestamp"],
                unique: false,
                partial: false,
            },
            ExpectedIndex {
                name: "messages_sender",
                columns: &["sender"],
                unique: false,
                partial: false,
            },
            ExpectedIndex {
                name: "messages_folder",
                columns: &["folder"],
                unique: false,
                partial: false,
            },
        ],
        &[&["message_id"]],
        &[&["message_id"]],
    )?;
    validate_indexes(
        connection,
        "attachments",
        &[],
        &[&["doc_id", "filename"]],
        &[],
    )?;
    validate_indexes(connection, "gmail_state", &[], &[&["source_account"]], &[])?;
    validate_indexes(
        connection,
        "gmail_messages",
        &[ExpectedIndex {
            name: "gmail_messages_state",
            columns: &["source_account", "source_state"],
            unique: false,
            partial: false,
        }],
        &[&["source_account", "gmail_message_id"], &["doc_id"]],
        &[&["doc_id"]],
    )?;
    validate_indexes(
        connection,
        "imap_messages",
        &[ExpectedIndex {
            name: "imap_messages_state",
            columns: &["source_account", "source_state"],
            unique: false,
            partial: false,
        }],
        &[
            &["source_account", "mailbox", "uid_validity", "uid"],
            &["doc_id"],
        ],
        &[&["doc_id"]],
    )?;
    validate_indexes(
        connection,
        "imap_scan_state",
        &[],
        &[&["source_account", "mailbox", "uid_validity"]],
        &[],
    )?;
    validate_indexes(
        connection,
        "imap_mailboxes",
        &[],
        &[&["source_account", "mailbox"]],
        &[],
    )?;
    validate_messages_schema_sql(connection)?;
    Ok(())
}

pub fn validate_existing_catalogue(path: &Path) -> rusqlite::Result<()> {
    let connection = Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_ONLY)?;
    let application_id: i64 =
        connection.query_row("PRAGMA application_id", [], |row| row.get(0))?;
    let user_version: i64 = connection.query_row("PRAGMA user_version", [], |row| row.get(0))?;
    if application_id != MEMORIA_CATALOGUE_APPLICATION_ID {
        return Err(catalogue_inconsistent("unsupported application_id"));
    }
    if user_version != MEMORIA_CATALOGUE_VERSION {
        return Err(catalogue_inconsistent("unsupported catalogue version"));
    }
    validate_catalogue_schema(&connection)
}

fn configure_catalogue_runtime(connection: &Connection) -> rusqlite::Result<()> {
    connection.execute_batch("PRAGMA journal_mode=DELETE; PRAGMA synchronous=EXTRA;")?;
    Ok(())
}

pub(crate) fn open_catalogue(path: &Path) -> rusqlite::Result<CatalogueConnection> {
    validate_existing_catalogue(path)?;
    let connection = Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_WRITE)?;
    configure_catalogue_runtime(&connection)?;
    Ok(CatalogueConnection {
        connection,
        authority: Arc::new(ArchiveAuthority { _lock: None }),
    })
}

fn create_catalogue_for_authority(
    path: &Path,
    authority: Arc<ArchiveAuthority>,
) -> rusqlite::Result<CatalogueConnection> {
    if path.exists() {
        validate_existing_catalogue(path)?;
        let connection = Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_WRITE)?;
        configure_catalogue_runtime(&connection)?;
        Ok(CatalogueConnection {
            connection,
            authority,
        })
    } else {
        create_catalogue_with_authority(path, authority)
    }
}

#[cfg(test)]
pub(crate) fn create_metadata(path: &Path) -> rusqlite::Result<CatalogueConnection> {
    create_catalogue_for_authority(path, Arc::new(ArchiveAuthority { _lock: None }))
}

fn insert_metadata(
    connection: &Connection,
    message: &Message,
    location: &DurableRawLocation,
) -> rusqlite::Result<()> {
    if location.reference.doc_id != message.id {
        return Err(batch_mismatch(
            "catalogue message does not match durable RAW document ID",
        ));
    }
    connection.execute(
        "INSERT INTO messages VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13)",
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
            location.reference.location.segment,
            location.reference.location.offset as i64,
            location.reference.location.frame_bytes as i64,
            &location.reference.blake3[..]
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
#[cfg(test)]
pub(crate) fn insert_gmail_metadata(
    connection: &Connection,
    source_account: &str,
    gmail_id: &str,
    doc_id: i64,
    thread_id: &str,
    label_ids_json: &str,
    internal_date_ms: Option<i64>,
    message_history_id: Option<&str>,
    location: &DurableRawLocation,
) -> rusqlite::Result<()> {
    if i64::try_from(location.reference.doc_id).ok() != Some(doc_id) {
        return Err(batch_mismatch(
            "Gmail identity does not match durable RAW document ID",
        ));
    }
    let transaction = connection.unchecked_transaction()?;
    insert_gmail_metadata_in_transaction(
        &transaction,
        source_account,
        gmail_id,
        doc_id,
        thread_id,
        label_ids_json,
        internal_date_ms,
        message_history_id,
        location,
    )?;
    transaction.commit()
}

#[allow(clippy::too_many_arguments)]
#[cfg(test)]
fn insert_gmail_metadata_with_hook<F>(
    connection: &Connection,
    source_account: &str,
    gmail_id: &str,
    doc_id: i64,
    thread_id: &str,
    label_ids_json: &str,
    internal_date_ms: Option<i64>,
    message_history_id: Option<&str>,
    location: &DurableRawLocation,
    after_messages: F,
) -> rusqlite::Result<()>
where
    F: FnOnce() -> rusqlite::Result<()>,
{
    let transaction = connection.unchecked_transaction()?;
    insert_gmail_metadata_in_transaction(
        &transaction,
        source_account,
        gmail_id,
        doc_id,
        thread_id,
        label_ids_json,
        internal_date_ms,
        message_history_id,
        location,
    )?;
    after_messages()?;
    transaction.commit()
}

#[allow(clippy::too_many_arguments)]
fn insert_gmail_metadata_in_transaction(
    transaction: &rusqlite::Transaction<'_>,
    source_account: &str,
    gmail_id: &str,
    doc_id: i64,
    thread_id: &str,
    label_ids_json: &str,
    internal_date_ms: Option<i64>,
    message_history_id: Option<&str>,
    location: &DurableRawLocation,
) -> rusqlite::Result<()> {
    if i64::try_from(location.reference.doc_id).ok() != Some(doc_id) {
        return Err(batch_mismatch(
            "Gmail identity does not match durable RAW document ID",
        ));
    }
    transaction.execute(
        "INSERT INTO messages(doc_id,message_id,timestamp,sender,recipients,subject,account,folder,thread,segment,archive_offset,frame_bytes,raw_blake3) VALUES (?1,?2,0,'','','',?3,'','',?4,?5,?6,?7)",
        params![doc_id, format!("gmail:{source_account}:{gmail_id}"), source_account, location.reference.location.segment, location.reference.location.offset as i64, location.reference.location.frame_bytes as i64, &location.reference.blake3[..]],
    )?;
    let now = chrono_like_now();
    transaction.execute(
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

pub(crate) fn upsert_imap_scan_state(
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

pub(crate) fn upsert_imap_mailbox(
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
#[cfg(test)]
pub(crate) fn insert_imap_metadata(
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
    location: &DurableRawLocation,
) -> rusqlite::Result<()> {
    if i64::try_from(location.reference.doc_id).ok() != Some(doc_id) {
        return Err(batch_mismatch(
            "IMAP identity does not match durable RAW document ID",
        ));
    }
    let transaction = connection.unchecked_transaction()?;
    insert_imap_metadata_in_transaction(
        &transaction,
        source_account,
        mailbox,
        uid_validity,
        uid,
        flags_json,
        internal_date,
        internal_date_ms,
        rfc822_size,
        doc_id,
        location,
    )?;
    transaction.commit()
}

#[allow(clippy::too_many_arguments)]
#[cfg(test)]
fn insert_imap_metadata_with_hook<F>(
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
    location: &DurableRawLocation,
    after_messages: F,
) -> rusqlite::Result<()>
where
    F: FnOnce() -> rusqlite::Result<()>,
{
    let transaction = connection.unchecked_transaction()?;
    insert_imap_metadata_in_transaction(
        &transaction,
        source_account,
        mailbox,
        uid_validity,
        uid,
        flags_json,
        internal_date,
        internal_date_ms,
        rfc822_size,
        doc_id,
        location,
    )?;
    after_messages()?;
    transaction.commit()
}

#[allow(clippy::too_many_arguments)]
fn insert_imap_metadata_in_transaction(
    transaction: &rusqlite::Transaction<'_>,
    source_account: &str,
    mailbox: &str,
    uid_validity: u32,
    uid: u32,
    flags_json: &str,
    internal_date: Option<&str>,
    internal_date_ms: Option<i64>,
    rfc822_size: Option<u32>,
    doc_id: i64,
    location: &DurableRawLocation,
) -> rusqlite::Result<()> {
    if i64::try_from(location.reference.doc_id).ok() != Some(doc_id) {
        return Err(batch_mismatch(
            "IMAP identity does not match durable RAW document ID",
        ));
    }
    let message_id = format!("imap:{source_account}:{mailbox}:{uid_validity}:{uid}");
    transaction.execute(
        "INSERT INTO messages(doc_id,message_id,timestamp,sender,recipients,subject,account,folder,thread,segment,archive_offset,frame_bytes,raw_blake3) VALUES (?1,?2,?3,'','','',?4,?5,'',?6,?7,?8,?9)",
        params![
            doc_id,
            message_id,
            internal_date_ms.unwrap_or(0),
            source_account,
            mailbox,
            location.reference.location.segment,
            location.reference.location.offset as i64,
            location.reference.location.frame_bytes as i64,
            &location.reference.blake3[..]
        ],
    )?;
    transaction.execute(
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

fn batch_mismatch(message: &'static str) -> rusqlite::Error {
    rusqlite::Error::InvalidParameterName(message.into())
}

fn validate_pending_batch<'a, I>(
    catalogue: &CatalogueConnection,
    durable: &DurableRawBatch,
    entries: I,
) -> rusqlite::Result<u64>
where
    I: IntoIterator<Item = (&'a PendingRawLocation, i64)>,
{
    if !Arc::ptr_eq(&catalogue.authority, &durable.authority) {
        return Err(batch_mismatch(
            "durable RAW batch belongs to another archive authority",
        ));
    }
    let mut ordinals = HashSet::new();
    let mut frame_bytes = 0u64;
    let mut count = 0u64;
    for (pending, doc_id) in entries {
        let durable_location = durable.entries.get(pending.ordinal);
        if !Arc::ptr_eq(&pending.batch, &durable.batch)
            || i64::try_from(pending.doc_id).ok() != Some(doc_id)
            || durable_location.map(|location| location.reference.doc_id) != Some(pending.doc_id)
            || !ordinals.insert(pending.ordinal)
        {
            return Err(batch_mismatch(
                "staged RAW locations do not cover the durable batch exactly",
            ));
        }
        frame_bytes = frame_bytes
            .checked_add(pending.frame_bytes)
            .ok_or_else(|| batch_mismatch("batch frame byte count overflow"))?;
        if durable_location.map(|location| location.reference.location.frame_bytes)
            != Some(pending.frame_bytes)
        {
            return Err(batch_mismatch(
                "staged RAW locations do not cover the durable batch exactly",
            ));
        }
        count += 1;
    }
    let durable_records = durable.records();
    if count != durable_records || ordinals.len() as u64 != durable_records {
        return Err(batch_mismatch(
            "staged RAW locations do not cover the durable batch exactly",
        ));
    }
    if (0..durable.entries.len()).any(|ordinal| !ordinals.contains(&ordinal)) {
        return Err(batch_mismatch(
            "staged RAW locations do not cover the durable batch exactly",
        ));
    }
    if frame_bytes != durable.frame_bytes {
        return Err(batch_mismatch(
            "durable RAW batch has a different frame byte count",
        ));
    }
    Ok(frame_bytes)
}

/// Publishes an exact Gmail batch after its RAW entries are durable.
pub(crate) fn publish_gmail_batch(
    connection: &CatalogueConnection,
    batch: &[GmailBatchRecord],
    durable: &DurableRawBatch,
) -> rusqlite::Result<()> {
    validate_pending_batch(
        connection,
        durable,
        batch.iter().map(|record| (&record.pending, record.doc_id)),
    )?;
    let transaction = connection.unchecked_transaction()?;
    for record in batch {
        let durable_location = &durable.entries[record.pending.ordinal];
        insert_gmail_metadata_in_transaction(
            &transaction,
            &record.source_account,
            &record.gmail_id,
            record.doc_id,
            &record.thread_id,
            &record.label_ids_json,
            record.internal_date_ms,
            record.message_history_id.as_deref(),
            durable_location,
        )?;
    }
    transaction.commit()
}

pub(crate) fn publish_imap_batch(
    connection: &CatalogueConnection,
    batch: &[ImapBatchRecord],
    durable: &DurableRawBatch,
) -> rusqlite::Result<()> {
    validate_pending_batch(
        connection,
        durable,
        batch.iter().map(|record| (&record.pending, record.doc_id)),
    )?;
    let transaction = connection.unchecked_transaction()?;
    for record in batch {
        let durable_location = &durable.entries[record.pending.ordinal];
        insert_imap_metadata_in_transaction(
            &transaction,
            &record.source_account,
            &record.mailbox,
            record.uid_validity,
            record.uid,
            &record.flags_json,
            record.internal_date.as_deref(),
            record.internal_date_ms,
            record.rfc822_size,
            record.doc_id,
            durable_location,
        )?;
    }
    transaction.commit()
}

/// Publishes an exact corpus batch after its RAW entries are durable.
pub(crate) fn publish_catalogue_batch(
    connection: &CatalogueConnection,
    batch: &[CatalogueBatchRecord],
    durable: &DurableRawBatch,
) -> rusqlite::Result<()> {
    validate_pending_batch(
        connection,
        durable,
        batch
            .iter()
            .map(|record| (&record.pending, record.message.id as i64)),
    )?;
    let transaction = connection.unchecked_transaction()?;
    for record in batch {
        let durable_location = &durable.entries[record.pending.ordinal];
        insert_metadata(&transaction, &record.message, durable_location)?;
    }
    transaction.commit()
}

pub(crate) fn repair_gmail_metadata(
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

pub(crate) fn mark_gmail_deleted_at_history(
    connection: &Connection,
    source_account: &str,
    gmail_id: &str,
    history_id: Option<&str>,
) -> rusqlite::Result<()> {
    connection.execute(
        "UPDATE gmail_messages SET source_state='deleted',message_history_id=COALESCE(?3,message_history_id),last_seen_unix=?4 WHERE source_account=?1 AND gmail_message_id=?2",
        params![source_account, gmail_id, history_id, chrono_like_now()],
    )?;
    Ok(())
}

pub(crate) fn mark_gmail_missing_from_full_sync(
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

pub(crate) fn set_gmail_state(
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

/// Opaque handle for the derived SQLite FTS index.
///
/// The underlying connection is intentionally not exposed: this index is not
/// a catalogue-authority connection and must not become a route to arbitrary
/// SQL against a caller-selected catalogue.
pub struct SqliteFtsIndex {
    connection: Connection,
}

impl SqliteFtsIndex {
    pub fn open(path: &Path) -> rusqlite::Result<Self> {
        Ok(Self {
            connection: Connection::open(path)?,
        })
    }
}

pub fn create_sqlite_fts(path: &Path) -> rusqlite::Result<SqliteFtsIndex> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).ok();
    }
    let index = SqliteFtsIndex::open(path)?;
    index.connection.execute_batch("PRAGMA journal_mode=DELETE; PRAGMA synchronous=NORMAL; CREATE VIRTUAL TABLE IF NOT EXISTS docs USING fts5(doc_id UNINDEXED, sender, recipients, subject, body, folder, account, tokenize='unicode61'); CREATE TABLE IF NOT EXISTS attrs (doc_id INTEGER PRIMARY KEY, timestamp INTEGER NOT NULL, sender TEXT NOT NULL, folder TEXT NOT NULL, account TEXT NOT NULL);")?;
    Ok(index)
}

pub fn index_sqlite(connection: &mut SqliteFtsIndex, config: CorpusConfig) -> rusqlite::Result<()> {
    index_sqlite_range(connection, config, 0, config.messages)
}

pub fn index_sqlite_range(
    connection: &mut SqliteFtsIndex,
    config: CorpusConfig,
    start: u64,
    count: u64,
) -> rusqlite::Result<()> {
    let transaction = connection.connection.transaction()?;
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
    connection: &mut SqliteFtsIndex,
    archive_root: &Path,
    start_id: u64,
    end_id: u64,
) -> io::Result<PipelineStats> {
    let transaction = connection.connection.transaction().map_err(sqlite_io)?;
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
    raw_blake3: Vec<u8>,
}

fn read_gmail_catalog_raw(root: &Path, row: &GmailCatalogRow) -> io::Result<Vec<u8>> {
    let reference = raw_reference_from_catalog_values(
        row.doc_id as i64,
        row.location.segment.clone(),
        row.location.offset as i64,
        row.location.frame_bytes as i64,
        row.raw_blake3.clone(),
    )?;
    read_authoritative_raw(&root.join("archive"), &reference)
}

fn for_each_gmail_catalog_row<F>(root: &Path, mut visit: F) -> io::Result<()>
where
    F: FnMut(GmailCatalogRow) -> io::Result<()>,
{
    let connection = open_catalogue(&root.join("metadata.sqlite")).map_err(sqlite_io)?;
    let mut statement = connection
        .prepare(
            "SELECT doc_id,source_account,labels,source_state,timestamp,segment,archive_offset,frame_bytes,raw_blake3
             FROM (
               SELECT g.doc_id AS doc_id,g.source_account AS source_account,g.label_ids AS labels,g.source_state AS source_state,
                      COALESCE(g.internal_date_ms,0) AS timestamp,
                      m.segment AS segment,m.archive_offset AS archive_offset,m.frame_bytes AS frame_bytes,m.raw_blake3 AS raw_blake3
               FROM gmail_messages g JOIN messages m ON m.doc_id=g.doc_id
               UNION ALL
               SELECT i.doc_id AS doc_id,i.source_account AS source_account,'[]' AS labels,i.source_state AS source_state,
                      COALESCE(i.internal_date_ms,0) AS timestamp,
                      m.segment AS segment,m.archive_offset AS archive_offset,m.frame_bytes AS frame_bytes,m.raw_blake3 AS raw_blake3
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
                raw_blake3: row.get(8)?,
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

fn delete_indexed_doc(
    writer: &mut IndexWriter,
    fields: TantivyFields,
    state_transaction: &rusqlite::Transaction<'_>,
    doc_id: u64,
) -> io::Result<()> {
    writer.delete_term(Term::from_field_u64(fields.doc_id, doc_id));
    state_transaction
        .execute("DELETE FROM indexed_docs WHERE doc_id=?1", [doc_id as i64])
        .map_err(sqlite_io)?;
    Ok(())
}

pub fn index_gmail_archive(root: &Path) -> io::Result<GmailIndexStats> {
    index_gmail_archive_with_mode_and_observer_and_config(
        root,
        |_| {},
        GmailIndexWriterConfig::default(),
        GmailIndexMode::Incremental,
    )
}

/// Revalidate every present RAW record before rebuilding the derived index.
pub fn rebuild_gmail_archive(root: &Path) -> io::Result<GmailIndexStats> {
    index_gmail_archive_with_mode_and_observer_and_config(
        root,
        |_| {},
        GmailIndexWriterConfig::default(),
        GmailIndexMode::Rebuild,
    )
}

/// Indexe l'archive et signale des phases de diagnostic au code expérimental.
/// Le callback n'influence pas le pipeline produit et peut rester un no-op.
pub fn index_gmail_archive_with_observer<F>(root: &Path, observe: F) -> io::Result<GmailIndexStats>
where
    F: FnMut(&str),
{
    index_gmail_archive_with_mode_and_observer_and_config(
        root,
        observe,
        GmailIndexWriterConfig::default(),
        GmailIndexMode::Incremental,
    )
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum GmailIndexMode {
    Incremental,
    Rebuild,
}

pub fn index_gmail_archive_with_observer_and_config<F>(
    root: &Path,
    observe: F,
    writer_config: GmailIndexWriterConfig,
) -> io::Result<GmailIndexStats>
where
    F: FnMut(&str),
{
    index_gmail_archive_with_mode_and_observer_and_config(
        root,
        observe,
        writer_config,
        GmailIndexMode::Incremental,
    )
}

fn index_gmail_archive_with_mode_and_observer_and_config<F>(
    root: &Path,
    mut observe: F,
    writer_config: GmailIndexWriterConfig,
    mode: GmailIndexMode,
) -> io::Result<GmailIndexStats>
where
    F: FnMut(&str),
{
    let open_started = Instant::now();
    let (index, fields) = open_or_create_gmail_tantivy(root).map_err(io::Error::other)?;
    let inventory_by_id: HashMap<u64, RecordInventoryStatus> = if mode == GmailIndexMode::Rebuild {
        inventory_records(root)?
            .into_iter()
            .filter_map(|record| {
                u64::try_from(record.doc_id)
                    .ok()
                    .map(|id| (id, record.status))
            })
            .collect()
    } else {
        HashMap::new()
    };
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
        if row.source_state != "present" {
            if unchanged {
                stats.skipped += 1;
                return Ok(());
            }
            delete_indexed_doc(&mut writer, fields, &state_transaction, row.doc_id)?;
            stats.removed += 1;
            changed = true;
            return Ok(());
        }
        if mode == GmailIndexMode::Incremental && unchanged {
            if read_gmail_catalog_raw(root, &row).is_err() {
                delete_indexed_doc(&mut writer, fields, &state_transaction, row.doc_id)?;
                stats.raw_inconsistent += 1;
                stats.removed += 1;
                changed = true;
                return Ok(());
            }
            stats.skipped += 1;
            return Ok(());
        }
        if mode == GmailIndexMode::Rebuild {
            match inventory_by_id.get(&row.doc_id) {
                Some(RecordInventoryStatus::AvailableValidated) => {}
                Some(RecordInventoryStatus::PhysicallyMissing) => {
                    delete_indexed_doc(&mut writer, fields, &state_transaction, row.doc_id)?;
                    stats.raw_unavailable += 1;
                    stats.removed += 1;
                    changed = true;
                    return Ok(());
                }
                Some(RecordInventoryStatus::Inconsistent { .. }) | None => {
                    delete_indexed_doc(&mut writer, fields, &state_transaction, row.doc_id)?;
                    stats.raw_inconsistent += 1;
                    stats.removed += 1;
                    changed = true;
                    return Ok(());
                }
            }
        }
        writer.delete_term(Term::from_field_u64(fields.doc_id, row.doc_id));
        let read_started = Instant::now();
        let raw = match read_gmail_catalog_raw(root, &row) {
            Ok(raw) => raw,
            Err(_) => {
                delete_indexed_doc(&mut writer, fields, &state_transaction, row.doc_id)?;
                stats.raw_inconsistent += 1;
                stats.removed += 1;
                changed = true;
                return Ok(());
            }
        };
        stats.read_us += read_started.elapsed().as_micros();
        let parse_started = Instant::now();
        let parsed = match parse_gmail_message(&raw, labels_for_index(&row.labels)) {
            Ok(parsed) => parsed,
            Err(_) => {
                stats.parse_failures += 1;
                delete_indexed_doc(&mut writer, fields, &state_transaction, row.doc_id)?;
                stats.removed += 1;
                changed = true;
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
                delete_indexed_doc(&mut writer, fields, &state_transaction, *doc_id)?;
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
    stats.partial =
        stats.raw_unavailable > 0 || stats.raw_inconsistent > 0 || stats.parse_failures > 0;
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
    let catalog = open_catalogue(&root.join("metadata.sqlite")).map_err(sqlite_io)?;
    read_catalogue_raw(&root.join("archive"), &catalog, doc_id)
}

pub fn read_authoritative_raw(
    archive_root: &Path,
    reference: &RawReference,
) -> io::Result<Vec<u8>> {
    let (record_id, raw) = read_record(archive_root, &reference.location)?;
    if record_id != reference.doc_id {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "catalog/archive doc_id mismatch",
        ));
    }
    if *blake3::hash(&raw).as_bytes() != reference.blake3 {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "catalogue/RAW BLAKE3 linkage mismatch",
        ));
    }
    Ok(raw)
}

pub fn export_message_eml(root: &Path, doc_id: u64, destination: &Path) -> io::Result<()> {
    let raw = read_archived_raw(root, doc_id)?;
    fs::write(destination, raw)
}

pub fn sqlite_search(index: &SqliteFtsIndex, query: &str) -> rusqlite::Result<Vec<SearchHit>> {
    let connection = &index.connection;
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

pub fn sqlite_date_count(index: &SqliteFtsIndex, start: i64, end: i64) -> rusqlite::Result<u64> {
    let connection = &index.connection;
    connection.query_row(
        "SELECT COUNT(*) FROM attrs WHERE timestamp BETWEEN ?1 AND ?2",
        params![start, end],
        |row| row.get::<_, i64>(0).map(|value| value as u64),
    )
}

pub fn sqlite_text_date_search(
    index: &SqliteFtsIndex,
    text: &str,
    start: i64,
    end: i64,
) -> rusqlite::Result<Vec<SearchHit>> {
    let mut statement = index.connection.prepare(
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
    let mut session = ArchiveSession::create(root, segment_bytes)?;
    let (writer, metadata) = session.parts_mut();
    let mut staged = Vec::new();
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
        let pending = writer.append(&message)?;
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
        staged.push(CatalogueBatchRecord::new(message, pending));
        if staged.len() >= CATALOGUE_BATCH_RECORD_LIMIT
            || writer.pending_raw().1 >= CATALOGUE_BATCH_BYTES_LIMIT
        {
            let durable = writer.durable_barrier()?;
            publish_catalogue_batch(metadata, &staged, &durable).map_err(sqlite_io)?;
            staged.clear();
        }
    }
    if !staged.is_empty() {
        let durable = writer.durable_barrier()?;
        publish_catalogue_batch(metadata, &staged, &durable).map_err(sqlite_io)?;
    }
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
            "memoria-raw-read-{label}-{}-{nonce}",
            std::process::id()
        ))
    }

    fn append_durable_raw(
        writer: &mut ArchiveWriter,
        doc_id: u64,
        raw: &[u8],
    ) -> DurableRawLocation {
        writer.append_raw(doc_id, raw).unwrap();
        writer
            .durable_barrier()
            .unwrap()
            .entries
            .into_iter()
            .next()
            .unwrap()
    }

    fn append_durable_message(writer: &mut ArchiveWriter, message: &Message) -> DurableRawLocation {
        writer.append(message).unwrap();
        writer
            .durable_barrier()
            .unwrap()
            .entries
            .into_iter()
            .next()
            .unwrap()
    }

    #[test]
    fn tier_a_catalogue_uses_delete_journal_and_extra_synchronous() {
        let root = raw_read_test_root("catalogue-pragmas");
        let _ = fs::remove_dir_all(&root);
        let connection = create_metadata(&root.join("metadata.sqlite")).unwrap();
        let journal_mode = connection
            .query_row("PRAGMA journal_mode", [], |row| row.get::<_, String>(0))
            .unwrap();
        let synchronous = connection
            .query_row("PRAGMA synchronous", [], |row| row.get::<_, i64>(0))
            .unwrap();
        assert_eq!(journal_mode.to_ascii_lowercase(), "delete");
        assert_eq!(synchronous, 3);
        drop(connection);
        let reopened = open_catalogue(&root.join("metadata.sqlite")).unwrap();
        let reopened_journal = reopened
            .query_row("PRAGMA journal_mode", [], |row| row.get::<_, String>(0))
            .unwrap();
        let reopened_synchronous = reopened
            .query_row("PRAGMA synchronous", [], |row| row.get::<_, i64>(0))
            .unwrap();
        assert_eq!(reopened_journal.to_ascii_lowercase(), "delete");
        assert_eq!(reopened_synchronous, 3);
        drop(reopened);
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn catalogue_publication_rolls_back_and_retry_reuses_identity() {
        let root = raw_read_test_root("catalogue-atomic");
        let _ = fs::remove_dir_all(&root);
        let connection = create_metadata(&root.join("metadata.sqlite")).unwrap();
        let mut writer =
            ArchiveWriter::open_for_catalogue(&root.join("archive"), 4096, &connection).unwrap();
        writer.append_raw(0, b"gmail atomic").unwrap();
        let gmail_durable = writer.durable_barrier().unwrap();
        let gmail_location = &gmail_durable.entries()[0];

        let gmail_failure = insert_gmail_metadata_with_hook(
            &connection,
            "account",
            "gmail-atomic",
            0,
            "thread",
            "[]",
            None,
            None,
            gmail_location,
            || Err(rusqlite::Error::InvalidQuery),
        );
        assert!(gmail_failure.is_err());
        assert_eq!(
            connection
                .query_row("SELECT COUNT(*) FROM messages", [], |row| row
                    .get::<_, i64>(0))
                .unwrap(),
            0
        );
        assert_eq!(
            connection
                .query_row("SELECT COUNT(*) FROM gmail_messages", [], |row| row
                    .get::<_, i64>(0))
                .unwrap(),
            0
        );
        insert_gmail_metadata(
            &connection,
            "account",
            "gmail-atomic",
            0,
            "thread",
            "[]",
            None,
            None,
            gmail_location,
        )
        .unwrap();

        writer.append_raw(1, b"imap atomic").unwrap();
        let imap_durable = writer.durable_barrier().unwrap();
        let imap_location = &imap_durable.entries()[0];
        let imap_failure = insert_imap_metadata_with_hook(
            &connection,
            "account",
            "INBOX",
            17,
            42,
            "[]",
            None,
            None,
            None,
            1,
            imap_location,
            || Err(rusqlite::Error::InvalidQuery),
        );
        assert!(imap_failure.is_err());
        assert_eq!(
            connection
                .query_row("SELECT COUNT(*) FROM messages WHERE doc_id=1", [], |row| {
                    row.get::<_, i64>(0)
                })
                .unwrap(),
            0
        );
        assert_eq!(
            connection
                .query_row("SELECT COUNT(*) FROM imap_messages", [], |row| row
                    .get::<_, i64>(0))
                .unwrap(),
            0
        );
        insert_imap_metadata(
            &connection,
            "account",
            "INBOX",
            17,
            42,
            "[]",
            None,
            None,
            None,
            1,
            imap_location,
        )
        .unwrap();
        assert_eq!(
            connection
                .query_row(
                    "SELECT COUNT(*) FROM imap_messages WHERE source_account='account' AND mailbox='INBOX' AND uid_validity=17 AND uid=42",
                    [],
                    |row| row.get::<_, i64>(0)
                )
                .unwrap(),
            1
        );
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn raw_batch_publication_requires_exact_receipt_and_retries_after_sqlite_failure() {
        let root = raw_read_test_root("raw-batch");
        let _ = fs::remove_dir_all(&root);
        let connection = create_metadata(&root.join("metadata.sqlite")).unwrap();
        let mut writer =
            ArchiveWriter::open_for_catalogue(&root.join("archive"), 4096, &connection).unwrap();
        let first = writer.append_raw(0, b"first").unwrap();
        let second = writer.append_raw(1, b"second").unwrap();
        let durable = writer.durable_barrier().unwrap();
        assert_eq!(
            durable
                .entries()
                .iter()
                .map(|entry| entry.reference().doc_id)
                .collect::<Vec<_>>(),
            vec![0, 1]
        );
        assert_eq!(
            durable.entries()[0].reference().blake3,
            *blake3::hash(b"first").as_bytes()
        );
        assert_eq!(
            durable.entries()[1].reference().blake3,
            *blake3::hash(b"second").as_bytes()
        );
        let gmail_record = |id: i64, gmail_id: &str, pending: PendingRawLocation| {
            GmailBatchRecord::new(
                "account".into(),
                gmail_id.into(),
                id,
                format!("thread-{id}"),
                "[]".into(),
                None,
                None,
                pending,
            )
        };
        let mut records = vec![gmail_record(0, "g0", first), gmail_record(1, "g1", second)];
        records[1].pending.ordinal = 0;
        assert!(publish_gmail_batch(&connection, &records, &durable).is_err());
        records[1].pending.ordinal = 2;
        assert!(publish_gmail_batch(&connection, &records, &durable).is_err());
        records[1].pending.ordinal = 1;
        records[1].doc_id = 9;
        assert!(publish_gmail_batch(&connection, &records, &durable).is_err());
        records[1].doc_id = 1;
        let mut wrong_receipt = durable.clone();
        wrong_receipt.entries.pop();
        assert!(publish_gmail_batch(&connection, &records, &wrong_receipt).is_err());
        assert_eq!(
            connection
                .query_row("SELECT COUNT(*) FROM messages", [], |row| row
                    .get::<_, i64>(0))
                .unwrap(),
            0
        );

        let duplicate_first = writer.append_raw(2, b"retry").unwrap();
        let duplicate_second = writer.append_raw(3, b"retry-again").unwrap();
        let retry_receipt = writer.durable_barrier().unwrap();
        let duplicate_batch = vec![
            gmail_record(2, "same", duplicate_first),
            gmail_record(3, "same", duplicate_second),
        ];
        assert!(publish_gmail_batch(&connection, &duplicate_batch, &retry_receipt).is_err());
        assert_eq!(
            connection
                .query_row("SELECT COUNT(*) FROM messages", [], |row| row
                    .get::<_, i64>(0))
                .unwrap(),
            0
        );
        assert!(read_record(
            &root.join("archive"),
            &retry_receipt.entries()[0].reference().location
        )
        .is_ok());
        assert!(read_record(
            &root.join("archive"),
            &retry_receipt.entries()[1].reference().location
        )
        .is_ok());

        let repaired_location = writer.append_raw(4, b"repaired").unwrap();
        let repaired_receipt = writer.durable_barrier().unwrap();
        let repaired_batch = vec![gmail_record(4, "same", repaired_location)];
        publish_gmail_batch(&connection, &repaired_batch, &repaired_receipt).unwrap();
        assert_eq!(
            connection
                .query_row("SELECT COUNT(*) FROM messages", [], |row| row
                    .get::<_, i64>(0))
                .unwrap(),
            1
        );
        assert_eq!(
            connection
                .query_row(
                    "SELECT raw_blake3 FROM messages WHERE doc_id=4",
                    [],
                    |row| { row.get::<_, Vec<u8>>(0) }
                )
                .unwrap(),
            blake3::hash(b"repaired").as_bytes()
        );
        assert_eq!(
            connection
                .query_row("SELECT COUNT(*) FROM gmail_messages", [], |row| row
                    .get::<_, i64>(0))
                .unwrap(),
            1
        );
        drop(writer);
        drop(connection);
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn raw_batch_barrier_failure_publishes_no_identity() {
        let root = raw_read_test_root("raw-batch-barrier-failure");
        let _ = fs::remove_dir_all(&root);
        let connection = create_metadata(&root.join("metadata.sqlite")).unwrap();
        let mut writer = ArchiveWriter::open_for_catalogue_with_faults(
            &root.join("archive"),
            4096,
            &connection,
            ArchiveWriterFaultInjection {
                fail_sync_remaining: 1,
                ..Default::default()
            },
        )
        .unwrap();
        let pending = writer.append_raw(0, b"pending").unwrap();
        let record = GmailBatchRecord::new(
            "account".into(),
            "g0".into(),
            0,
            "thread".into(),
            "[]".into(),
            None,
            None,
            pending,
        );
        assert!(writer.durable_barrier().is_err());
        assert_eq!(
            connection
                .query_row("SELECT COUNT(*) FROM gmail_messages", [], |row| row
                    .get::<_, i64>(0))
                .unwrap(),
            0
        );
        let durable = writer.durable_barrier().unwrap();
        publish_gmail_batch(&connection, &[record], &durable).unwrap();
        let stored_blake3 = connection
            .query_row(
                "SELECT raw_blake3 FROM messages WHERE doc_id=0",
                [],
                |row| row.get::<_, Vec<u8>>(0),
            )
            .unwrap();
        assert_eq!(stored_blake3, blake3::hash(b"pending").as_bytes());
        drop(writer);
        drop(connection);
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn imap_raw_batch_publishes_multiple_records_in_one_catalogue_unit() {
        let root = raw_read_test_root("imap-raw-batch");
        let _ = fs::remove_dir_all(&root);
        let connection = create_metadata(&root.join("metadata.sqlite")).unwrap();
        let mut writer =
            ArchiveWriter::open_for_catalogue(&root.join("archive"), 4096, &connection).unwrap();
        let first = writer.append_raw(0, b"imap-first").unwrap();
        let second = writer.append_raw(1, b"imap-second").unwrap();
        let durable = writer.durable_barrier().unwrap();
        let record = |uid: u32, doc_id: i64, pending: PendingRawLocation| {
            ImapBatchRecord::new(
                "account".into(),
                "INBOX".into(),
                7,
                uid,
                "[]".into(),
                None,
                None,
                None,
                doc_id,
                pending,
            )
        };
        let records = vec![record(10, 0, first), record(11, 1, second)];
        publish_imap_batch(&connection, &records, &durable).unwrap();
        assert_eq!(
            connection
                .query_row("SELECT COUNT(*) FROM messages", [], |row| row
                    .get::<_, i64>(0))
                .unwrap(),
            2
        );
        let hashes = {
            let mut statement = connection
                .prepare("SELECT raw_blake3 FROM messages ORDER BY doc_id")
                .unwrap();
            statement
                .query_map([], |row| row.get::<_, Vec<u8>>(0))
                .unwrap()
                .collect::<rusqlite::Result<Vec<_>>>()
                .unwrap()
        };
        assert_eq!(hashes[0], blake3::hash(b"imap-first").as_bytes());
        assert_eq!(hashes[1], blake3::hash(b"imap-second").as_bytes());
        assert_eq!(
            connection
                .query_row("SELECT COUNT(*) FROM imap_messages", [], |row| row
                    .get::<_, i64>(0))
                .unwrap(),
            2
        );
        drop(writer);
        drop(connection);
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn raw_batch_rejects_another_batch_and_record_or_byte_mismatches() {
        let root = raw_read_test_root("raw-batch-mismatch");
        let _ = fs::remove_dir_all(&root);
        let connection = create_metadata(&root.join("metadata.sqlite")).unwrap();
        let mut writer =
            ArchiveWriter::open_for_catalogue(&root.join("archive"), 4096, &connection).unwrap();
        let location = writer.append_raw(0, b"mismatch").unwrap();
        let durable = writer.durable_barrier().unwrap();
        let record = GmailBatchRecord::new(
            "account".into(),
            "g0".into(),
            0,
            "thread".into(),
            "[]".into(),
            None,
            None,
            location,
        );
        let records = vec![record];
        writer.append_raw(1, b"another batch").unwrap();
        let another_batch = writer.durable_barrier().unwrap();
        assert!(publish_gmail_batch(&connection, &records, &another_batch).is_err());
        let mut wrong_bytes = durable.clone();
        wrong_bytes.frame_bytes += 1;
        assert!(publish_gmail_batch(&connection, &records, &wrong_bytes).is_err());
        let wrong_count = Vec::new();
        assert!(publish_gmail_batch(&connection, &wrong_count, &durable).is_err());
        assert_eq!(
            connection
                .query_row("SELECT COUNT(*) FROM messages", [], |row| row
                    .get::<_, i64>(0))
                .unwrap(),
            0
        );
        drop(writer);
        drop(connection);
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn durable_batch_can_publish_only_to_its_catalogue_authority() {
        let root = raw_read_test_root("raw-batch-authority");
        let _ = fs::remove_dir_all(&root);
        let catalogue_a = create_metadata(&root.join("catalogue-a.sqlite")).unwrap();
        let catalogue_b = create_metadata(&root.join("catalogue-b.sqlite")).unwrap();
        let mut writer_a =
            ArchiveWriter::open_for_catalogue(&root.join("archive-a"), 4096, &catalogue_a).unwrap();
        let pending = writer_a.append_raw(7, b"authority-bound").unwrap();
        let durable = writer_a.durable_barrier().unwrap();
        let record = GmailBatchRecord::new(
            "account".into(),
            "authority-bound".into(),
            7,
            "thread".into(),
            "[]".into(),
            None,
            None,
            pending.clone(),
        );

        let error = publish_gmail_batch(&catalogue_b, &[record], &durable).unwrap_err();
        assert!(error.to_string().contains("another archive authority"));
        assert_eq!(
            catalogue_b
                .query_row("SELECT COUNT(*) FROM messages", [], |row| row
                    .get::<_, i64>(0))
                .unwrap(),
            0
        );

        let record = GmailBatchRecord::new(
            "account".into(),
            "authority-bound".into(),
            7,
            "thread".into(),
            "[]".into(),
            None,
            None,
            pending,
        );
        publish_gmail_batch(&catalogue_a, &[record], &durable).unwrap();
        assert_eq!(
            catalogue_a
                .query_row("SELECT COUNT(*) FROM messages", [], |row| row
                    .get::<_, i64>(0))
                .unwrap(),
            1
        );
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn session_authority_survives_writer_replacement() {
        let root = raw_read_test_root("session-authority-lifetime");
        let _ = fs::remove_dir_all(&root);
        let mut session = ArchiveSession::create(&root, 4096).unwrap();
        session.replace_writer_for_test();
        let error = ArchiveWriter::open(&root.join("archive"), 4096).unwrap_err();
        assert!(error.to_string().contains("ArchiveAlreadyLocked"));
        drop(session);
        let writer = ArchiveWriter::open(&root.join("archive"), 4096).unwrap();
        drop(writer);
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn archive_writer_pending_batches_and_empty_barriers_are_explicit() {
        let root = raw_read_test_root("writer-pending");
        let _ = fs::remove_dir_all(&root);
        let mut writer = ArchiveWriter::open(&root, 4096).unwrap();

        let empty = writer.durable_barrier().unwrap();
        assert_eq!(empty.records(), 0);
        assert_eq!(empty.frame_bytes(), 0);
        assert!(!writer.file_dirty());
        assert!(!writer.namespace_sync_pending());
        assert_eq!(writer.fault.sync_calls, 0);
        assert_eq!(writer.fault.namespace_sync_calls, 0);
        let empty_again = writer.durable_barrier().unwrap();
        assert_eq!(empty_again.records(), 0);
        assert_eq!(empty_again.frame_bytes(), 0);
        assert!(!Arc::ptr_eq(&empty_again.batch, &empty.batch));
        assert_eq!(writer.fault.sync_calls, 0);
        assert_eq!(writer.fault.namespace_sync_calls, 0);

        let pending = writer.append_raw(7, b"pending").unwrap();
        assert_eq!(writer.pending_raw(), (1, pending.frame_bytes));
        assert_eq!(writer.state(), ArchiveWriterState::Ready);
        assert!(writer.file_dirty());
        assert!(writer.namespace_sync_pending());
        let durable = writer.durable_barrier().unwrap();
        assert!(Arc::ptr_eq(&durable.batch, &pending.batch));
        assert_eq!(durable.records(), 1);
        assert_eq!(durable.frame_bytes(), pending.frame_bytes);
        assert_eq!(durable.entries()[0].reference().doc_id, 7);
        assert_eq!(writer.pending_raw(), (0, 0));
        assert!(!writer.file_dirty());
        assert!(!writer.namespace_sync_pending());
        assert_eq!(writer.fault.sync_calls, 1);
        assert_eq!(writer.fault.namespace_sync_calls, 1);
        writer.durable_barrier().unwrap();
        assert_eq!(writer.fault.sync_calls, 1);
        assert_eq!(writer.fault.namespace_sync_calls, 1);
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn archive_writer_poison_is_sticky_for_every_frame_component() {
        let components = [
            ArchiveWriterWriteComponent::Magic,
            ArchiveWriterWriteComponent::Id,
            ArchiveWriterWriteComponent::Length,
            ArchiveWriterWriteComponent::Checksum,
            ArchiveWriterWriteComponent::Payload,
        ];
        for component in components {
            let root = raw_read_test_root(&format!("writer-poison-{component:?}"));
            let _ = fs::remove_dir_all(&root);
            let mut writer = ArchiveWriter::open_with_faults(
                &root,
                4096,
                ArchiveWriterFaultInjection {
                    fail_component: Some(component),
                    partial_payload: component == ArchiveWriterWriteComponent::Payload,
                    ..Default::default()
                },
            )
            .unwrap();
            let first = writer.append_raw(7, b"poison").unwrap_err();
            assert!(first.to_string().contains("injected failure"));
            assert_eq!(writer.state(), ArchiveWriterState::Poisoned);
            let second = writer.append_raw(8, b"later").unwrap_err();
            assert!(second.to_string().contains("archive writer is poisoned"));
            let barrier = writer.durable_barrier().unwrap_err();
            assert!(barrier.to_string().contains("archive writer is poisoned"));
            let _ = fs::remove_dir_all(root);
        }
    }

    #[test]
    fn archive_writer_sync_failure_is_retryable_and_rotation_syncs_old_segment() {
        let root = raw_read_test_root("writer-sync-failure");
        let _ = fs::remove_dir_all(&root);
        let mut writer = ArchiveWriter::open_with_faults(
            &root,
            4096,
            ArchiveWriterFaultInjection {
                fail_sync_remaining: 1,
                ..Default::default()
            },
        )
        .unwrap();
        writer.append_raw(1, &[1u8; 64]).unwrap();
        assert!(writer.durable_barrier().is_err());
        assert_eq!(writer.state(), ArchiveWriterState::Ready);
        assert_eq!(writer.pending_raw().0, 1);
        assert!(writer.file_dirty());
        assert!(writer.durable_barrier().is_ok());
        assert_eq!(writer.fault.sync_calls, 2);
        let _ = fs::remove_dir_all(&root);

        let root = raw_read_test_root("writer-rotation");
        let _ = fs::remove_dir_all(&root);
        let mut writer = ArchiveWriter::open(&root, 1024).unwrap();
        writer.append_raw(1, &[1u8; 800]).unwrap();
        writer.durable_barrier().unwrap();
        let first_segment = root.join("segment-000000.arc");
        writer.append_raw(2, &[2u8; 800]).unwrap();
        assert_eq!(writer.segment_name, "segment-000001.arc");
        assert_eq!(writer.fault.sync_calls, 2);
        assert!(fs::metadata(first_segment).unwrap().len() > 0);
        assert!(writer.namespace_sync_pending());
        writer.durable_barrier().unwrap();
        assert_eq!(writer.fault.sync_calls, 3);
        assert!(!writer.namespace_sync_pending());
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn archive_writer_namespace_barrier_failure_is_retryable() {
        let root = raw_read_test_root("writer-namespace-failure");
        let _ = fs::remove_dir_all(&root);
        let mut writer = ArchiveWriter::open_with_faults(
            &root,
            4096,
            ArchiveWriterFaultInjection {
                fail_namespace_remaining: 1,
                ..Default::default()
            },
        )
        .unwrap();
        writer.append_raw(1, b"namespace").unwrap();
        assert!(writer.durable_barrier().is_err());
        assert!(!writer.file_dirty());
        assert!(writer.namespace_sync_pending());
        writer.durable_barrier().unwrap();
        assert_eq!(writer.fault.sync_calls, 1);
        assert_eq!(writer.fault.namespace_sync_calls, 2);
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn archive_writer_namespace_sync_is_scoped_to_archive_root() {
        for root_preexisting in [true, false] {
            let root = raw_read_test_root(if root_preexisting {
                "namespace-existing-root"
            } else {
                "namespace-created-root"
            });
            let _ = fs::remove_dir_all(&root);
            if root_preexisting {
                fs::create_dir_all(&root).unwrap();
            }
            assert_eq!(root.exists(), root_preexisting);
            let mut writer = ArchiveWriter::open_with_faults(
                &root,
                4096,
                ArchiveWriterFaultInjection::default(),
            )
            .unwrap();
            writer.append_raw(1, b"namespace scope").unwrap();
            writer.durable_barrier().unwrap();
            assert_eq!(writer.fault.namespace_sync_paths, vec![root.clone()]);
            let _ = fs::remove_dir_all(root);
        }
    }

    #[test]
    fn archive_writer_reopen_appends_after_an_uncertain_frame_without_truncating() {
        let root = raw_read_test_root("writer-reopen");
        let _ = fs::remove_dir_all(&root);
        let mut writer = ArchiveWriter::open(&root, 4096).unwrap();
        writer.append_raw(1, b"durable").unwrap();
        writer.durable_barrier().unwrap();
        drop(writer);

        let mut failed = ArchiveWriter::open_with_faults(
            &root,
            4096,
            ArchiveWriterFaultInjection {
                fail_component: Some(ArchiveWriterWriteComponent::Payload),
                partial_payload: true,
                ..Default::default()
            },
        )
        .unwrap();
        assert!(failed.append_raw(2, b"uncertain").is_err());
        drop(failed);
        let length_after_failure = fs::metadata(root.join("segment-000000.arc")).unwrap().len();

        let mut reopened = ArchiveWriter::open(&root, 4096).unwrap();
        let later = append_durable_raw(&mut reopened, 3, b"later");
        let final_length = fs::metadata(root.join("segment-000000.arc")).unwrap().len();
        assert!(final_length > length_after_failure);
        let (record_id, raw) = read_record(&root, &later.reference.location).unwrap();
        assert_eq!(record_id, 3);
        assert_eq!(raw, b"later");
        let _ = fs::remove_dir_all(root);
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
        let location = append_durable_message(&mut writer, &message);
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
        let location = append_durable_message(&mut writer, &message);
        let (id, raw) = read_record(&archive, &location.reference.location).unwrap();
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
        writer.append_raw(1, first_raw).unwrap();
        writer.append_raw(2, second_raw).unwrap();
        let durable = writer.durable_barrier().unwrap();
        let second_location = durable.entries()[1].reference().location.clone();
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
                &durable.entries()[0]
            } else {
                &durable.entries()[1]
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
    fn authoritative_read_checks_catalogue_blake3_after_restart_and_corruption() {
        let root = raw_read_test_root("blake3-linkage");
        let archive = root.join("archive");
        fs::create_dir_all(&archive).unwrap();
        let raw_a = b"same-size-A";
        let raw_b = b"same-size-B";
        let mut writer = ArchiveWriter::open(&archive, 4096).unwrap();
        let durable_a = append_durable_raw(&mut writer, 7, raw_a);
        let location_a = durable_a.reference.location.clone();
        drop(writer);

        // A is durable but unpublished. After restart, B is the legitimate
        // publication for the same document identity.
        let mut writer = ArchiveWriter::open(&archive, 4096).unwrap();
        let durable_b = append_durable_raw(&mut writer, 7, raw_b);
        let location_b = durable_b.reference.location.clone();
        let durable_c = append_durable_raw(&mut writer, 7, b"different-size-C");
        let location_c = durable_c.reference.location.clone();
        let catalog = create_metadata(&root.join("metadata.sqlite")).unwrap();
        let message = Message {
            id: 7,
            message_id: "fixture-7".into(),
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
            raw: raw_b.to_vec(),
        };
        insert_metadata(&catalog, &message, &durable_b).unwrap();

        catalog
            .execute(
                "UPDATE messages SET segment=?1, archive_offset=?2, frame_bytes=?3 WHERE doc_id=7",
                params![
                    location_a.segment,
                    location_a.offset as i64,
                    location_a.frame_bytes as i64
                ],
            )
            .unwrap();
        drop(catalog);
        assert!(read_archived_raw(&root, 7)
            .unwrap_err()
            .to_string()
            .contains("BLAKE3"));

        let catalog = open_catalogue(&root.join("metadata.sqlite")).unwrap();
        catalog
            .execute(
                "UPDATE messages SET segment=?1, archive_offset=?2, frame_bytes=?3 WHERE doc_id=7",
                params![
                    location_c.segment,
                    location_c.offset as i64,
                    location_c.frame_bytes as i64
                ],
            )
            .unwrap();
        assert!(read_archived_raw(&root, 7)
            .unwrap_err()
            .to_string()
            .contains("BLAKE3"));
        catalog
            .execute(
                "UPDATE messages SET segment=?1, archive_offset=?2, frame_bytes=?3 WHERE doc_id=7",
                params![
                    location_b.segment,
                    location_b.offset as i64,
                    location_b.frame_bytes as i64
                ],
            )
            .unwrap();
        assert_eq!(read_archived_raw(&root, 7).unwrap(), raw_b);

        catalog
            .execute(
                "UPDATE messages SET raw_blake3=?1 WHERE doc_id=7",
                [vec![0u8; 32]],
            )
            .unwrap();
        assert!(read_archived_raw(&root, 7)
            .unwrap_err()
            .to_string()
            .contains("BLAKE3"));
        catalog
            .execute(
                "UPDATE messages SET raw_blake3=?1 WHERE doc_id=7",
                [&durable_b.reference.blake3[..]],
            )
            .unwrap();
        let segment_path = archive.join(&location_b.segment);
        let mut bytes = fs::read(&segment_path).unwrap();
        let payload_offset = location_b.offset as usize + FRAME_HEADER_BYTES as usize;
        bytes[payload_offset] ^= 1;
        fs::write(&segment_path, &bytes).unwrap();
        assert!(read_archived_raw(&root, 7)
            .unwrap_err()
            .to_string()
            .contains("checksum"));
        let checksum = fnv64(&bytes[payload_offset..payload_offset + raw_b.len()]);
        bytes[location_b.offset as usize + 24..location_b.offset as usize + 32]
            .copy_from_slice(&checksum.to_le_bytes());
        fs::write(&segment_path, &bytes).unwrap();
        assert!(read_archived_raw(&root, 7)
            .unwrap_err()
            .to_string()
            .contains("BLAKE3"));
        drop(catalog);
        drop(writer);
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn legacy_catalogue_is_rejected_without_modification() {
        let root = raw_read_test_root("legacy-catalogue");
        fs::create_dir_all(&root).unwrap();
        let path = root.join("metadata.sqlite");
        let connection = Connection::open(&path).unwrap();
        connection.execute_batch("PRAGMA application_id=0; PRAGMA user_version=0; CREATE TABLE messages (doc_id INTEGER PRIMARY KEY, message_id TEXT NOT NULL, segment TEXT NOT NULL, archive_offset INTEGER NOT NULL, frame_bytes INTEGER NOT NULL);").unwrap();
        drop(connection);
        let before = fs::read(&path).unwrap();
        assert!(open_catalogue(&path).is_err());
        assert_eq!(fs::read(&path).unwrap(), before);
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn incompatible_catalogues_are_rejected_read_only_by_all_authoritative_openers() {
        for variant in [
            "bad-application-id",
            "future-version",
            "missing-table",
            "missing-raw-column",
            "extra-column",
            "partial-index",
            "wrong-index-order",
            "missing-message-unique",
            "extra-table",
        ] {
            let root = raw_read_test_root(variant);
            fs::create_dir_all(root.join("archive")).unwrap();
            let mut writer = ArchiveWriter::open(&root.join("archive"), 4096).unwrap();
            let raw = b"catalogue validation fixture";
            let location = append_durable_raw(&mut writer, 0, raw);
            let catalog = create_catalogue(&root.join("metadata.sqlite")).unwrap();
            let message = Message {
                id: 0,
                message_id: "validation-fixture".into(),
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
                raw: raw.to_vec(),
            };
            insert_metadata(&catalog, &message, &location).unwrap();
            drop(catalog);
            drop(writer);
            let catalog = Connection::open(root.join("metadata.sqlite")).unwrap();
            match variant {
                "bad-application-id" => catalog.execute_batch("PRAGMA application_id=1234;").unwrap(),
                "future-version" => catalog.execute_batch("PRAGMA user_version=2;").unwrap(),
                "missing-table" => catalog.execute_batch("DROP TABLE imap_mailboxes;").unwrap(),
                "missing-raw-column" => catalog.execute_batch("DROP TABLE messages; CREATE TABLE messages (doc_id INTEGER PRIMARY KEY, message_id TEXT NOT NULL UNIQUE, timestamp INTEGER NOT NULL, sender TEXT NOT NULL, recipients TEXT NOT NULL, subject TEXT NOT NULL, account TEXT NOT NULL, folder TEXT NOT NULL, thread TEXT NOT NULL, segment TEXT NOT NULL, archive_offset INTEGER NOT NULL, frame_bytes INTEGER NOT NULL);").unwrap(),
                "extra-column" => catalog.execute_batch("ALTER TABLE messages ADD COLUMN unexpected TEXT;").unwrap(),
                "partial-index" => catalog.execute_batch("DROP INDEX messages_timestamp; CREATE INDEX messages_timestamp ON messages(timestamp) WHERE timestamp >= 0;").unwrap(),
                "wrong-index-order" => catalog.execute_batch("DROP INDEX gmail_messages_state; CREATE INDEX gmail_messages_state ON gmail_messages(source_state, source_account);").unwrap(),
                "missing-message-unique" => catalog.execute_batch("DROP INDEX messages_timestamp; DROP INDEX messages_sender; DROP INDEX messages_folder; ALTER TABLE messages RENAME TO messages_old; CREATE TABLE messages (doc_id INTEGER PRIMARY KEY, message_id TEXT NOT NULL, timestamp INTEGER NOT NULL, sender TEXT NOT NULL, recipients TEXT NOT NULL, subject TEXT NOT NULL, account TEXT NOT NULL, folder TEXT NOT NULL, thread TEXT NOT NULL, segment TEXT NOT NULL, archive_offset INTEGER NOT NULL, frame_bytes INTEGER NOT NULL, raw_blake3 BLOB NOT NULL CHECK(length(raw_blake3)=32)); CREATE INDEX messages_timestamp ON messages(timestamp); CREATE INDEX messages_sender ON messages(sender); CREATE INDEX messages_folder ON messages(folder); DROP TABLE messages_old;").unwrap(),
                "extra-table" => catalog.execute_batch("CREATE TABLE unexpected_table (id INTEGER);").unwrap(),
                _ => unreachable!(),
            }
            drop(catalog);
            let path = root.join("metadata.sqlite");
            let before = fs::read(&path).unwrap();
            let sidecars = [
                path.with_file_name("metadata.sqlite-wal"),
                path.with_file_name("metadata.sqlite-shm"),
                path.with_file_name("metadata.sqlite-journal"),
            ];
            let sidecars_before: Vec<_> = sidecars.iter().map(|path| fs::read(path).ok()).collect();
            assert!(validate_existing_catalogue(&path).is_err());
            assert_eq!(fs::read(&path).unwrap(), before);
            assert!(read_archived_raw(&root, 0).is_err());
            assert_eq!(fs::read(&path).unwrap(), before);
            assert_eq!(
                sidecars
                    .iter()
                    .map(|path| fs::read(path).ok())
                    .collect::<Vec<_>>(),
                sidecars_before
            );
            let _ = fs::remove_dir_all(root);
        }
    }

    #[test]
    fn correct_application_id_with_legacy_version_zero_is_rejected_unchanged() {
        let root = raw_read_test_root("version-zero");
        fs::create_dir_all(&root).unwrap();
        let path = root.join("metadata.sqlite");
        let connection = create_catalogue(&path).unwrap();
        drop(connection);
        let connection = Connection::open(&path).unwrap();
        connection.execute_batch("PRAGMA user_version=0;").unwrap();
        drop(connection);
        let before = fs::read(&path).unwrap();
        assert!(open_catalogue(&path).is_err());
        assert_eq!(fs::read(&path).unwrap(), before);
        for suffix in ["-wal", "-shm", "-journal"] {
            assert!(!root.join(format!("metadata.sqlite{suffix}")).exists());
        }
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn divergent_raw_blake3_ddl_is_rejected_against_canonical_schema() {
        for (variant, check) in [
            ("none", "1"),
            ("lower-bound", "length(raw_blake3)>=32"),
            ("even", "length(raw_blake3)%2=0"),
            ("wrong-column", "length(doc_id)=32"),
            (
                "comment",
                "raw_blake3 IS NOT NULL /* length(raw_blake3)=32 */",
            ),
        ] {
            let root = raw_read_test_root(&format!("raw-check-{variant}"));
            fs::create_dir_all(&root).unwrap();
            let path = root.join("metadata.sqlite");
            drop(create_catalogue(&path).unwrap());
            let connection = Connection::open(&path).unwrap();
            connection
                .execute_batch(&format!(
                    "DROP INDEX messages_timestamp;
                 DROP INDEX messages_sender;
                 DROP INDEX messages_folder;
                 ALTER TABLE messages RENAME TO messages_old;
                 CREATE TABLE messages (doc_id INTEGER PRIMARY KEY, message_id TEXT NOT NULL UNIQUE, timestamp INTEGER NOT NULL, sender TEXT NOT NULL, recipients TEXT NOT NULL, subject TEXT NOT NULL, account TEXT NOT NULL, folder TEXT NOT NULL, thread TEXT NOT NULL, segment TEXT NOT NULL, archive_offset INTEGER NOT NULL, frame_bytes INTEGER NOT NULL, raw_blake3 BLOB NOT NULL CHECK({check}));
                 CREATE INDEX messages_timestamp ON messages(timestamp);
                 CREATE INDEX messages_sender ON messages(sender);
                 CREATE INDEX messages_folder ON messages(folder);
                 DROP TABLE messages_old;"
                ))
                .unwrap();
            drop(connection);
            let before = fs::read(&path).unwrap();
            assert!(validate_existing_catalogue(&path).is_err(), "{variant}");
            assert_eq!(fs::read(&path).unwrap(), before);
            for suffix in ["-wal", "-shm", "-journal"] {
                assert!(!root.join(format!("metadata.sqlite{suffix}")).exists());
            }
            let _ = fs::remove_dir_all(root);
        }
    }

    #[test]
    fn record_read_rejects_inconsistent_coordinates() {
        let root = raw_read_test_root("coordinates");
        let archive = root.join("archive");
        fs::create_dir_all(&archive).unwrap();
        let mut writer = ArchiveWriter::open(&archive, 4096).unwrap();
        let location = append_durable_raw(&mut writer, 7, b"coordinate fixture");
        drop(writer);

        let mut wrong_frame_bytes = location.reference.location.clone();
        wrong_frame_bytes.frame_bytes -= 1;
        assert_eq!(
            read_record(&archive, &wrong_frame_bytes)
                .unwrap_err()
                .kind(),
            io::ErrorKind::InvalidData
        );

        let mut outside = location.reference.location.clone();
        outside.offset = u64::MAX;
        assert_eq!(
            read_record(&archive, &outside).unwrap_err().kind(),
            io::ErrorKind::InvalidData
        );

        let mut invalid_segment = location.reference.location;
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
        let location = append_durable_raw(&mut writer, 9, b"bounded fixture")
            .reference
            .location;
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
        let mut writer =
            ArchiveWriter::open_for_catalogue(&root.join("archive"), 64 * 1024, &catalog).unwrap();
        let raws = vec![
            b"frame one payload".to_vec(),
            b"frame two payload".to_vec(),
            b"frame three payload".to_vec(),
        ];
        let mut batch = Vec::new();
        for (id, raw) in raws.iter().enumerate() {
            let pending = writer.append_raw(id as u64, raw).unwrap();
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
            batch.push(CatalogueBatchRecord::new(message, pending));
        }
        let durable = writer.durable_barrier().unwrap();
        publish_catalogue_batch(&catalog, &batch, &durable).unwrap();
        let locations = durable
            .entries()
            .iter()
            .map(|entry| entry.reference().location.clone())
            .collect();
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
        let physical = inventory_physical(&root).unwrap();
        assert_eq!(physical.catalogued_physically_missing, 1);
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
        assert!(result[1].location.is_none());
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
        assert!(result[1].location.is_none());
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
    fn inventory_reads_past_an_unpublished_tail_without_modifying_inputs() {
        let (root, _, _) = inventory_fixture("read-only-tail");
        let segment_path = root.join("archive/segment-000000.arc");
        let sqlite_paths = [
            root.join("metadata.sqlite"),
            root.join("metadata.sqlite-wal"),
            root.join("metadata.sqlite-shm"),
        ];
        let mut file = OpenOptions::new().append(true).open(&segment_path).unwrap();
        file.write_all(&[0u8; 9]).unwrap();
        drop(file);
        let before_segment = fs::read(&segment_path).unwrap();
        let before_sqlite = sqlite_paths
            .iter()
            .map(|path| fs::read(path).ok())
            .collect::<Vec<_>>();

        let result = inventory_records(&root).unwrap();
        assert_eq!(result.len(), 3);
        assert!(result
            .iter()
            .all(|record| matches!(record.status, RecordInventoryStatus::AvailableValidated)));
        assert_eq!(fs::read(&segment_path).unwrap(), before_segment);
        let after_sqlite = sqlite_paths
            .iter()
            .map(|path| fs::read(path).ok())
            .collect::<Vec<_>>();
        assert_eq!(after_sqlite, before_sqlite);
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn physical_inventory_distinguishes_same_doc_id_orphan_and_continues() {
        let (root, _, _) = inventory_fixture("physical-orphan-same-id");
        let catalog = open_catalogue(&root.join("metadata.sqlite")).unwrap();
        let mut writer =
            ArchiveWriter::open_for_catalogue(&root.join("archive"), 64 * 1024, &catalog).unwrap();
        let _orphan_pending = writer
            .append_raw(1, b"not MIME and never published")
            .unwrap();
        writer.durable_barrier().unwrap();
        let published_raw = b"From: sender@example.test\r\nSubject: published\r\n\r\npublished";
        let published_pending = writer.append_raw(3, published_raw).unwrap();
        let published_durable = writer.durable_barrier().unwrap();
        let message = Message {
            id: 3,
            message_id: "inventory-published-3".into(),
            timestamp: 0,
            sender: "sender@example.test".into(),
            recipients: Vec::new(),
            subject: "published".into(),
            text_body: "published".into(),
            html_body: None,
            account: "fixture".into(),
            folder: "Inbox".into(),
            thread: "thread".into(),
            attachments: Vec::new(),
            raw: published_raw.to_vec(),
        };
        publish_catalogue_batch(
            &catalog,
            &[CatalogueBatchRecord::new(message, published_pending)],
            &published_durable,
        )
        .unwrap();
        drop(writer);
        drop(catalog);

        let before = fs::read(root.join("archive/segment-000000.arc")).unwrap();
        let inventory = inventory_physical(&root).unwrap();
        assert_eq!(inventory.orphan_valid_frames, 1);
        assert_eq!(inventory.validated_catalogued_records, 4);
        assert_eq!(inventory.incomplete_tails, 0);
        assert!(inventory.frames.iter().any(|frame| {
            frame.doc_id == Some(1) && matches!(frame.status, PhysicalFrameStatus::OrphanValidated)
        }));
        assert!(inventory.frames.iter().any(|frame| {
            frame.doc_id == Some(3)
                && matches!(frame.status, PhysicalFrameStatus::CataloguedValidated)
        }));
        // The scanner is diagnostic only, including for an invalid MIME orphan.
        assert_eq!(
            fs::read(root.join("archive/segment-000000.arc")).unwrap(),
            before
        );
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn physical_inventory_reports_partial_tail_without_truncating() {
        let (root, _, _) = inventory_fixture("physical-incomplete-tail");
        let path = root.join("archive/segment-000000.arc");
        let mut file = OpenOptions::new().append(true).open(&path).unwrap();
        file.write_all(b"MAARC001").unwrap();
        drop(file);
        let before = fs::read(&path).unwrap();
        let inventory = inventory_physical(&root).unwrap();
        assert_eq!(inventory.incomplete_tails, 1);
        assert_eq!(inventory.orphan_valid_frames, 0);
        assert_eq!(fs::read(&path).unwrap(), before);
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn rebuild_indexes_catalogue_only_and_ignores_valid_orphan() {
        let (root, _) = gmail_inventory_fixture("rebuild-ignores-orphan");
        let mut writer = ArchiveWriter::open(&root.join("archive"), 64 * 1024).unwrap();
        writer
            .append_raw(
                1,
                b"From: orphan@example.test\r\nSubject: orphan-only\r\n\r\nsecret-orphan-term",
            )
            .unwrap();
        writer.durable_barrier().unwrap();
        drop(writer);
        let inventory = inventory_physical(&root).unwrap();
        assert_eq!(inventory.orphan_valid_frames, 1);
        let stats = rebuild_gmail_archive(&root).unwrap();
        assert_eq!(stats.indexed, 3);
        assert!(indexed_search_ids(&root, "orphan-only").is_empty());
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn physical_inventory_keeps_catalogued_wrong_digest_inconsistent() {
        let (root, locations, _) = inventory_fixture("physical-wrong-digest");
        let catalog = Connection::open(root.join("metadata.sqlite")).unwrap();
        catalog
            .execute(
                "UPDATE messages SET raw_blake3=?1 WHERE doc_id=1",
                params![vec![0u8; 32]],
            )
            .unwrap();
        drop(catalog);
        let inventory = inventory_physical(&root).unwrap();
        assert_eq!(inventory.orphan_valid_frames, 0);
        assert_eq!(inventory.inconsistent_catalogued_records, 1);
        let frame = inventory
            .frames
            .iter()
            .find(|frame| frame.location == locations[1])
            .unwrap();
        assert!(matches!(
            frame.status,
            PhysicalFrameStatus::CataloguedInconsistent
        ));
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn physical_inventory_treats_wrong_catalogue_frame_bytes_as_claimed_inconsistent() {
        let (root, locations, _) = inventory_fixture("physical-wrong-frame-bytes");
        let catalog = Connection::open(root.join("metadata.sqlite")).unwrap();
        catalog
            .execute(
                "UPDATE messages SET frame_bytes=frame_bytes+1 WHERE doc_id=1",
                [],
            )
            .unwrap();
        drop(catalog);
        let inventory = inventory_physical(&root).unwrap();
        assert_eq!(inventory.orphan_valid_frames, 0);
        assert_eq!(inventory.inconsistent_catalogued_records, 1);
        assert_eq!(inventory.catalogued_physically_missing, 0);
        let frame = inventory
            .frames
            .iter()
            .find(|frame| frame.location == locations[1])
            .unwrap();
        assert!(matches!(
            frame.status,
            PhysicalFrameStatus::CataloguedInconsistent
        ));
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn physical_inventory_keeps_claim_for_negative_catalogue_doc_id() {
        let (root, locations, _) = inventory_fixture("physical-negative-doc-id");
        let catalog = Connection::open(root.join("metadata.sqlite")).unwrap();
        catalog
            .execute("UPDATE messages SET doc_id=-1 WHERE doc_id=1", [])
            .unwrap();
        drop(catalog);
        let inventory = inventory_physical(&root).unwrap();
        assert_eq!(inventory.orphan_valid_frames, 0);
        let frame = inventory
            .frames
            .iter()
            .find(|frame| frame.location == locations[1])
            .unwrap();
        assert!(matches!(
            frame.status,
            PhysicalFrameStatus::CataloguedInconsistent
        ));
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn physical_inventory_keeps_claim_for_negative_catalogue_frame_bytes() {
        let (root, locations, _) = inventory_fixture("physical-negative-frame-bytes");
        let catalog = Connection::open(root.join("metadata.sqlite")).unwrap();
        catalog
            .execute("UPDATE messages SET frame_bytes=-1 WHERE doc_id=1", [])
            .unwrap();
        drop(catalog);
        let inventory = inventory_physical(&root).unwrap();
        assert_eq!(inventory.orphan_valid_frames, 0);
        let frame = inventory
            .frames
            .iter()
            .find(|frame| frame.location == locations[1])
            .unwrap();
        assert!(matches!(
            frame.status,
            PhysicalFrameStatus::CataloguedInconsistent
        ));
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn archive_summary_is_read_only_for_catalogue_and_sidecars() {
        let (root, _, _) = inventory_fixture("summary-read-only");
        let catalogue = root.join("metadata.sqlite");
        let sidecars = [
            root.join("metadata.sqlite-journal"),
            root.join("metadata.sqlite-wal"),
            root.join("metadata.sqlite-shm"),
        ];
        let before = fs::read(&catalogue).unwrap();
        let before_sidecars = sidecars
            .iter()
            .map(|path| fs::read(path).ok())
            .collect::<Vec<_>>();
        let summary = archive_summary(&root).unwrap();
        assert_eq!(summary.catalogued_records, 3);
        assert_eq!(fs::read(&catalogue).unwrap(), before);
        assert_eq!(
            sidecars
                .iter()
                .map(|path| fs::read(path).ok())
                .collect::<Vec<_>>(),
            before_sidecars
        );
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn physical_inventory_body_allocation_failure_is_an_error() {
        let error = allocate_frame_body(u64::MAX).unwrap_err();
        assert!(matches!(
            error.kind(),
            io::ErrorKind::InvalidData | io::ErrorKind::Other
        ));
    }

    #[test]
    fn physical_inventory_stops_after_checksum_corruption_without_authentic_framing() {
        let (root, locations, _) = inventory_fixture("physical-checksum-corruption");
        let path = root.join("archive/segment-000000.arc");
        let mut bytes = fs::read(&path).unwrap();
        bytes[locations[1].offset as usize + 24] ^= 1;
        fs::write(&path, &bytes).unwrap();
        let inventory = inventory_physical(&root).unwrap();
        assert_eq!(inventory.physical_corruptions, 1);
        assert_eq!(inventory.incomplete_tails, 0);
        assert_eq!(inventory.validated_catalogued_records, 2);
        assert!(!inventory
            .frames
            .iter()
            .any(|frame| frame.location == locations[2]));
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn physical_inventory_stops_on_untrusted_length_and_does_not_call_it_tail() {
        let (root, locations, _) = inventory_fixture("physical-length-corruption");
        let path = root.join("archive/segment-000000.arc");
        let mut bytes = fs::read(&path).unwrap();
        let body_len = locations[1].frame_bytes - FRAME_HEADER_BYTES + 1;
        bytes[locations[1].offset as usize + 16..locations[1].offset as usize + 24]
            .copy_from_slice(&body_len.to_le_bytes());
        fs::write(&path, &bytes).unwrap();
        let inventory = inventory_physical(&root).unwrap();
        assert_eq!(inventory.physical_corruptions, 1);
        assert_eq!(inventory.incomplete_tails, 0);
        assert!(!inventory
            .frames
            .iter()
            .any(|frame| frame.location == locations[2]));
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn physical_inventory_classifies_terminal_body_overrun_as_corruption() {
        let (root, locations, _) = inventory_fixture("physical-terminal-length-corruption");
        let path = root.join("archive/segment-000000.arc");
        let mut bytes = fs::read(&path).unwrap();
        let offset = locations[2].offset as usize;
        let body_len = locations[2].frame_bytes - FRAME_HEADER_BYTES + 1;
        bytes[offset + 16..offset + 24].copy_from_slice(&body_len.to_le_bytes());
        fs::write(&path, &bytes).unwrap();
        let inventory = inventory_physical(&root).unwrap();
        assert_eq!(inventory.physical_corruptions, 1);
        assert_eq!(inventory.incomplete_tails, 0);
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
        let first_location = append_durable_raw(&mut writer, 0, &first);
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
        let second_location = append_durable_raw(&mut writer, 1, &second);
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

    fn gmail_inventory_fixture(label: &str) -> (PathBuf, Vec<ArchiveLocation>) {
        let root = std::env::temp_dir().join(format!(
            "atlas-gmail-index-a2-{label}-{}-{}",
            std::process::id(),
            Instant::now().elapsed().as_nanos()
        ));
        let _ = fs::remove_dir_all(&root);
        let catalog = create_metadata(&root.join("metadata.sqlite")).unwrap();
        let mut writer =
            ArchiveWriter::open_for_catalogue(&root.join("archive"), 64 * 1024, &catalog).unwrap();
        let mut batch = Vec::new();
        for (id, term) in ["a2-alpha", "a2-beta", "a2-gamma"].into_iter().enumerate() {
            let raw = format!(
                "From: fixture@example.test\r\nTo: reader@example.test\r\nSubject: {term}\r\n\r\n{term}"
            )
            .into_bytes();
            let pending = writer.append_raw(id as u64, &raw).unwrap();
            batch.push(GmailBatchRecord::new(
                "fixture-account".into(),
                format!("gmail-a2-{id}"),
                id as i64,
                format!("thread-{id}"),
                "[\"INBOX\"]".into(),
                Some(id as i64),
                Some(format!("{id}")),
                pending,
            ));
        }
        let durable = writer.durable_barrier().unwrap();
        publish_gmail_batch(&catalog, &batch, &durable).unwrap();
        let locations = durable
            .entries()
            .iter()
            .map(|entry| entry.reference().location.clone())
            .collect();
        drop(catalog);
        drop(writer);
        (root, locations)
    }

    fn indexed_doc_ids(root: &Path) -> Vec<u64> {
        let state = Connection::open(gmail_index_state_path(root)).unwrap();
        let mut statement = state
            .prepare("SELECT doc_id FROM indexed_docs ORDER BY doc_id")
            .unwrap();
        statement
            .query_map([], |row| row.get::<_, i64>(0))
            .unwrap()
            .map(|row| row.unwrap() as u64)
            .collect()
    }

    fn indexed_search_ids(root: &Path, term: &str) -> Vec<u64> {
        GmailSearchIndex::open(root)
            .unwrap()
            .search(term, 10)
            .unwrap()
            .into_iter()
            .map(|result| result.doc_id)
            .collect()
    }

    #[test]
    fn gmail_index_revalidates_central_corruption_and_recovers_after_repair() {
        let (root, locations) = gmail_inventory_fixture("central-corruption");
        let stats = index_gmail_archive(&root).unwrap();
        assert_eq!(stats.indexed, 3);
        assert!(!stats.partial);
        assert_eq!(indexed_doc_ids(&root), vec![0, 1, 2]);

        let segment_path = root.join("archive").join(&locations[1].segment);
        let before = fs::read(&segment_path).unwrap();
        let mut corrupted = before.clone();
        corrupted[locations[1].offset as usize + 32] ^= 1;
        fs::write(&segment_path, &corrupted).unwrap();
        let stats = rebuild_gmail_archive(&root).unwrap();
        assert_eq!(stats.raw_inconsistent, 1);
        assert_eq!(stats.raw_unavailable, 0);
        assert!(stats.partial);
        assert_eq!(indexed_doc_ids(&root), vec![0, 2]);
        assert_eq!(indexed_search_ids(&root, "a2-alpha"), vec![0]);
        assert!(indexed_search_ids(&root, "a2-beta").is_empty());
        assert_eq!(indexed_search_ids(&root, "a2-gamma"), vec![2]);

        fs::write(&segment_path, before).unwrap();
        let stats = rebuild_gmail_archive(&root).unwrap();
        assert_eq!(stats.indexed, 3);
        assert!(!stats.partial);
        assert_eq!(indexed_doc_ids(&root), vec![0, 1, 2]);
        assert_eq!(indexed_search_ids(&root, "a2-beta"), vec![1]);
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn gmail_index_rejects_catalogue_substitution_and_wrong_digest() {
        let (root, locations) = gmail_inventory_fixture("linked-secondary");
        assert_eq!(index_gmail_archive(&root).unwrap().indexed, 3);

        let catalog = Connection::open(root.join("metadata.sqlite")).unwrap();
        catalog
            .execute(
                "UPDATE messages SET segment=?1, archive_offset=?2, frame_bytes=?3 WHERE doc_id=1",
                params![
                    locations[0].segment,
                    locations[0].offset as i64,
                    locations[0].frame_bytes as i64
                ],
            )
            .unwrap();
        drop(catalog);
        let stats = index_gmail_archive(&root).unwrap();
        assert_eq!(stats.raw_inconsistent, 1);
        assert_eq!(indexed_doc_ids(&root), vec![0, 2]);

        let catalog = Connection::open(root.join("metadata.sqlite")).unwrap();
        catalog
            .execute(
                "UPDATE messages SET segment=?1, archive_offset=?2, frame_bytes=?3, raw_blake3=?4 WHERE doc_id=1",
                params![
                    locations[1].segment,
                    locations[1].offset as i64,
                    locations[1].frame_bytes as i64,
                    vec![0u8; 32]
                ],
            )
            .unwrap();
        drop(catalog);
        let stats = index_gmail_archive(&root).unwrap();
        assert_eq!(stats.raw_inconsistent, 1);
        assert_eq!(indexed_doc_ids(&root), vec![0, 2]);
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn gmail_index_incremental_fast_path_revalidates_post_index_corruption() {
        let (root, locations) = gmail_inventory_fixture("incremental-fast-path");
        index_gmail_archive(&root).unwrap();
        let stats = index_gmail_archive(&root).unwrap();
        assert_eq!(stats.skipped, 3);
        assert_eq!(stats.indexed, 0);
        assert!(!stats.partial);

        let segment_path = root.join("archive").join(&locations[1].segment);
        let before = fs::read(&segment_path).unwrap();
        let mut corrupted = before.clone();
        corrupted[locations[1].offset as usize + 32] ^= 1;
        fs::write(&segment_path, corrupted).unwrap();
        let stats = index_gmail_archive(&root).unwrap();
        assert_eq!(stats.skipped, 2);
        assert_eq!(stats.raw_inconsistent, 1);
        assert!(stats.partial);
        assert!(indexed_search_ids(&root, "a2-beta").is_empty());

        let stats = rebuild_gmail_archive(&root).unwrap();
        assert_eq!(stats.raw_inconsistent, 1);
        assert!(stats.partial);
        assert_eq!(indexed_doc_ids(&root), vec![0, 2]);
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn gmail_index_is_partial_for_one_missing_segment_and_recovers_after_restore() {
        let (root, locations) = gmail_inventory_fixture("missing-segment");
        index_gmail_archive(&root).unwrap();
        let original_segment = root.join("archive").join(&locations[1].segment);
        let missing_segment = "segment-000001.arc";
        fs::copy(
            &original_segment,
            root.join("archive").join(missing_segment),
        )
        .unwrap();
        let catalog = Connection::open(root.join("metadata.sqlite")).unwrap();
        catalog
            .execute(
                "UPDATE messages SET segment=?1, archive_offset=0 WHERE doc_id=1",
                params![missing_segment],
            )
            .unwrap();
        drop(catalog);
        fs::remove_file(root.join("archive").join(missing_segment)).unwrap();

        let stats = rebuild_gmail_archive(&root).unwrap();
        assert_eq!(stats.raw_unavailable, 1);
        assert!(stats.partial);
        assert_eq!(indexed_doc_ids(&root), vec![0, 2]);
        assert_eq!(indexed_search_ids(&root, "a2-alpha"), vec![0]);
        assert_eq!(indexed_search_ids(&root, "a2-gamma"), vec![2]);

        let catalog = Connection::open(root.join("metadata.sqlite")).unwrap();
        catalog
            .execute(
                "UPDATE messages SET segment=?1, archive_offset=?2, frame_bytes=?3 WHERE doc_id=1",
                params![
                    locations[1].segment,
                    locations[1].offset as i64,
                    locations[1].frame_bytes as i64
                ],
            )
            .unwrap();
        drop(catalog);
        let stats = rebuild_gmail_archive(&root).unwrap();
        assert_eq!(stats.indexed, 3);
        assert!(!stats.partial);
        assert_eq!(indexed_doc_ids(&root), vec![0, 1, 2]);
        assert_eq!(indexed_search_ids(&root, "a2-beta"), vec![1]);
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn gmail_index_parse_failure_removes_tantivy_and_state_together() {
        let (root, _locations) = gmail_inventory_fixture("parse-failure");
        index_gmail_archive(&root).unwrap();
        let mut writer = ArchiveWriter::open(&root.join("archive"), 64 * 1024).unwrap();
        let malformed = b"Content-Type: text/plain\r\nContent-Transfer-Encoding: base64\r\n\r\n%%%";
        let malformed_location = append_durable_raw(&mut writer, 1, malformed)
            .reference
            .location;
        drop(writer);
        let catalog = Connection::open(root.join("metadata.sqlite")).unwrap();
        catalog
            .execute(
                "UPDATE messages SET segment=?1, archive_offset=?2, frame_bytes=?3 WHERE doc_id=1",
                params![
                    malformed_location.segment,
                    malformed_location.offset as i64,
                    malformed_location.frame_bytes as i64
                ],
            )
            .unwrap();
        drop(catalog);

        let stats = index_gmail_archive(&root).unwrap();
        assert_eq!(stats.parse_failures, 0);
        assert_eq!(stats.raw_unavailable, 0);
        assert_eq!(stats.raw_inconsistent, 1);
        assert!(stats.partial);
        assert_eq!(indexed_doc_ids(&root), vec![0, 2]);
        assert!(indexed_search_ids(&root, "a2-beta").is_empty());
        assert_eq!(indexed_search_ids(&root, "a2-gamma"), vec![2]);
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn search_finds_a_message_using_attachment_text_only() {
        let root = std::env::temp_dir().join(format!(
            "mail-attachment-text-search-{}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&root);
        let catalog = create_metadata(&root.join("metadata.sqlite")).unwrap();
        let mut writer =
            ArchiveWriter::open_for_catalogue(&root.join("archive"), 4096, &catalog).unwrap();
        let raw = b"From: fixture@example.test\r\nTo: reader@example.test\r\nSubject: hello\r\nContent-Type: multipart/mixed; boundary=part\r\n\r\n--part\r\nContent-Type: text/plain\r\n\r\nhello\r\n--part\r\nContent-Type: text/plain; charset=iso-8859-1\r\nContent-Disposition: attachment; filename=note.txt\r\n\r\ncaf\xe9 phrase-secrete-947\r\n--part--\r\n";
        let location = append_durable_raw(&mut writer, 0, raw);
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
        let location = append_durable_raw(&mut writer, 0, raw.as_bytes());
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
        let location = append_durable_raw(&mut writer, 0, raw.as_bytes());
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
        let mut writer =
            ArchiveWriter::open_for_catalogue(&root.join("archive"), 4096, &catalog).unwrap();
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
        let mut batch = Vec::new();
        for (id, (raw, labels, timestamp)) in messages.into_iter().enumerate() {
            let raw = raw.as_bytes();
            let pending = writer.append_raw(id as u64, raw).unwrap();
            batch.push(GmailBatchRecord::new(
                "fixture-account".into(),
                format!("gmail-{id}"),
                id as i64,
                format!("thread-{id}"),
                labels.into(),
                Some(timestamp),
                Some(format!("{id}")),
                pending,
            ));
        }
        let durable = writer.durable_barrier().unwrap();
        publish_gmail_batch(&catalog, &batch, &durable).unwrap();
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
        let location = append_durable_message(&mut writer, &message);
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
        let location = append_durable_message(&mut writer, &message);
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
        let location = append_durable_message(&mut writer, &message);
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
}
