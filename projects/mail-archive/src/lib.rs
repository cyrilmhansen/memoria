use flate2::{write::GzEncoder, Compression};
use mailparse::{parse_mail, MailHeaderMap, ParsedMail};
use rusqlite::{params, Connection};
use std::cmp::Ordering;
use std::collections::{BTreeMap, HashMap, HashSet};
use std::fs::{self, File, OpenOptions};
use std::io::{self, Read, Seek, SeekFrom, Write};
use std::ops::Bound;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};
use tantivy::collector::TopDocs;
use tantivy::query::{BooleanQuery, QueryParser, RangeQuery};
use tantivy::schema::{
    Field, IndexRecordOption, Schema, TextFieldIndexing, TextOptions, Value, INDEXED, STORED,
};
use tantivy::{doc, Index, IndexReader, IndexWriter, TantivyDocument, Term};

pub mod app_config;
pub mod gmail;

pub const DEFAULT_SEED: u64 = 0x4d_41_49_4c_41_52_43;
const FRAME_MAGIC: &[u8; 8] = b"MAARC001";

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
            Self::Personal => (30, 55, 1 * 1024 * 1024),
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

#[derive(Clone, Debug)]
pub struct ArchiveLocation {
    pub segment: String,
    pub offset: u64,
    pub frame_bytes: u64,
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
}

pub struct GmailSearchIndex {
    index: Index,
    reader: IndexReader,
    fields: TantivyFields,
    catalog: Connection,
}

impl GmailSearchIndex {
    pub fn open(root: &Path) -> io::Result<Self> {
        let (index, fields) = open_or_create_gmail_tantivy(root)
            .map_err(|error| io::Error::new(io::ErrorKind::Other, error))?;
        let reader = index
            .reader()
            .map_err(|error| io::Error::new(io::ErrorKind::Other, error))?;
        let catalog = Connection::open(root.join("metadata.sqlite")).map_err(sqlite_io)?;
        Ok(Self {
            index,
            reader,
            fields,
            catalog,
        })
    }

    pub fn search(&self, query: &str, limit: usize) -> io::Result<Vec<GmailSearchResult>> {
        let searcher = self.reader.searcher();
        let parsed = gmail_query(&self.index, self.fields, query)
            .map_err(|error| io::Error::new(io::ErrorKind::InvalidInput, error))?;
        let top_docs = searcher
            .search(&parsed, &TopDocs::with_limit(limit).order_by_score())
            .map_err(|error| io::Error::new(io::ErrorKind::Other, error))?;
        let mut results = Vec::new();
        for (score, address) in top_docs {
            let document: TantivyDocument = searcher
                .doc(address)
                .map_err(|error| io::Error::new(io::ErrorKind::Other, error))?;
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
        self.reader
            .reload()
            .map_err(|error| io::Error::new(io::ErrorKind::Other, error))
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
    let bytes = if decoded { payload } else { payload };
    blake3::hash(bytes).to_hex().to_string()
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

pub fn read_record(root: &Path, location: &ArchiveLocation) -> io::Result<(u64, Vec<u8>)> {
    let mut file = File::open(root.join(&location.segment))?;
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
    let mut body = vec![0u8; len as usize];
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
    let is_attachment = matches!(
        disposition.disposition,
        mailparse::DispositionType::Attachment
    ) || disposition.params.contains_key("filename")
        || part.ctype.params.contains_key("name");
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
    let fields = TantivyFields {
        doc_id: schema_builder.add_u64_field("doc_id", INDEXED | STORED),
        timestamp: schema_builder.add_i64_field("timestamp", INDEXED | STORED),
        sender: schema_builder.add_text_field("sender", text_options.clone()),
        recipients: schema_builder.add_text_field("recipients", text_options.clone()),
        subject: schema_builder.add_text_field("subject", text_options.clone()),
        body: schema_builder.add_text_field("body", text_options.clone()),
        folder: schema_builder.add_text_field("folder", text_options.clone()),
        account: schema_builder.add_text_field("account", text_options.clone()),
        labels: schema_builder.add_text_field("label", text_options.clone()),
        attachment_types: schema_builder.add_text_field("attachment_type", text_options),
        attachment_count: schema_builder.add_u64_field("attachment_count", INDEXED | STORED),
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
    pub folder: Field,
    pub account: Field,
    pub labels: Field,
    pub attachment_types: Field,
    pub attachment_count: Field,
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
    let mut writer = index
        .writer(50_000_000)
        .map_err(|error| io::Error::new(io::ErrorKind::Other, error))?;
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
            .map_err(|error| io::Error::new(io::ErrorKind::Other, error))?;
        Ok(())
    })?;
    writer
        .commit()
        .map_err(|error| io::Error::new(io::ErrorKind::Other, error))?;
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
        folder: field("folder")?,
        account: field("account")?,
        labels: field("label")?,
        attachment_types: field("attachment_type")?,
        attachment_count: field("attachment_count")?,
    })
}

fn open_or_create_gmail_tantivy(root: &Path) -> tantivy::Result<(Index, TantivyFields)> {
    let path = gmail_index_dir(root);
    fs::create_dir_all(&path)
        .map_err(|error| tantivy::TantivyError::SystemError(error.to_string()))?;
    if path.join("meta.json").exists() {
        let index = Index::open_in_dir(&path)?;
        let fields = tantivy_fields_from_schema(&index.schema())?;
        Ok((index, fields))
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

fn gmail_catalog_rows(root: &Path) -> io::Result<Vec<GmailCatalogRow>> {
    let connection = Connection::open(root.join("metadata.sqlite")).map_err(sqlite_io)?;
    let mut statement = connection
        .prepare(
            "SELECT g.doc_id,g.source_account,g.label_ids,g.source_state,
                    COALESCE(g.internal_date_ms,0),
                    m.segment,m.archive_offset,m.frame_bytes
             FROM gmail_messages g JOIN messages m ON m.doc_id=g.doc_id
             ORDER BY g.doc_id",
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
    rows.map(|row| row.map_err(sqlite_io)).collect()
}

fn labels_for_index(value: &str) -> Vec<String> {
    serde_json::from_str(value).unwrap_or_default()
}

pub fn index_gmail_archive(root: &Path) -> io::Result<GmailIndexStats> {
    let open_started = Instant::now();
    let (index, fields) = open_or_create_gmail_tantivy(root)
        .map_err(|error| io::Error::new(io::ErrorKind::Other, error))?;
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
    let rows = gmail_catalog_rows(root)?;
    let current: HashSet<u64> = rows.iter().map(|row| row.doc_id).collect();
    let mut writer = index
        .writer(50_000_000)
        .map_err(|error| io::Error::new(io::ErrorKind::Other, error))?;
    let mut changed = false;
    let mut state_deletes = Vec::new();
    let mut state_upserts = Vec::new();
    let index_started = Instant::now();
    for row in &rows {
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
            continue;
        }
        writer.delete_term(Term::from_field_u64(fields.doc_id, row.doc_id));
        if row.source_state != "present" {
            state_deletes.push(row.doc_id);
            stats.removed += 1;
            changed = true;
            continue;
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
                continue;
            }
        };
        stats.parse_us += parse_started.elapsed().as_micros();
        writer
            .add_document(doc!(
                fields.doc_id => row.doc_id,
                fields.timestamp => row.timestamp,
                fields.sender => parsed.sender,
                fields.recipients => parsed.recipients,
                fields.subject => parsed.subject,
                fields.body => parsed.body,
                fields.account => row.source_account,
                fields.labels => parsed.labels.join(" "),
                fields.attachment_types => parsed.attachment_types.join(" "),
                fields.attachment_count => parsed.attachment_count
            ))
            .map_err(|error| io::Error::new(io::ErrorKind::Other, error))?;
        state_upserts.push((
            row.doc_id,
            row.location.segment.clone(),
            row.location.offset,
            row.location.frame_bytes,
            row.labels.clone(),
            row.source_state.clone(),
        ));
        stats.indexed += 1;
        changed = true;
    }
    for (doc_id, _) in &known {
        if !current.contains(doc_id) {
            writer.delete_term(Term::from_field_u64(fields.doc_id, *doc_id));
            state_deletes.push(*doc_id);
            stats.removed += 1;
            changed = true;
        }
    }
    if changed {
        writer
            .commit()
            .map_err(|error| io::Error::new(io::ErrorKind::Other, error))?;
        let transaction = state.transaction().map_err(sqlite_io)?;
        for doc_id in state_deletes {
            transaction
                .execute("DELETE FROM indexed_docs WHERE doc_id=?1", [doc_id as i64])
                .map_err(sqlite_io)?;
        }
        for (doc_id, segment, offset, frame_bytes, labels, source_state) in state_upserts {
            transaction
                .execute(
                    "INSERT OR REPLACE INTO indexed_docs VALUES (?1,?2,?3,?4,?5,?6)",
                    params![
                        doc_id as i64,
                        segment,
                        offset as i64,
                        frame_bytes as i64,
                        labels,
                        source_state
                    ],
                )
                .map_err(sqlite_io)?;
        }
        transaction.commit().map_err(sqlite_io)?;
    }
    stats.index_us = index_started.elapsed().as_micros();
    stats.index_bytes = directory_bytes(&gmail_index_dir(root))?;
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

fn gmail_query(
    index: &Index,
    fields: TantivyFields,
    query: &str,
) -> tantivy::Result<Box<dyn tantivy::query::Query>> {
    let mut text_tokens = Vec::new();
    let mut after = None;
    let mut before = None;
    for token in query.split_whitespace() {
        if let Some(value) = token.strip_prefix("after:") {
            after = parse_date_token(value).map(|value| value * 1000);
        } else if let Some(value) = token.strip_prefix("before:") {
            before = parse_date_token(value).map(|value| value * 1000);
        } else if token.starts_with("from:") {
            text_tokens.push(token.replacen("from:", "sender:", 1));
        } else if token.starts_with("to:") {
            text_tokens.push(token.replacen("to:", "recipients:", 1));
        } else {
            text_tokens.push(token.to_string());
        }
    }
    let parser = QueryParser::for_index(
        index,
        vec![
            fields.sender,
            fields.recipients,
            fields.subject,
            fields.body,
            fields.account,
            fields.labels,
            fields.attachment_types,
        ],
    );
    let mut queries: Vec<Box<dyn tantivy::query::Query>> = Vec::new();
    if !text_tokens.is_empty() {
        queries.push(parser.parse_query(&text_tokens.join(" "))?);
    }
    if after.is_some() || before.is_some() {
        queries.push(Box::new(RangeQuery::new(
            after.map_or(Bound::Unbounded, |value| {
                Bound::Included(Term::from_field_i64(fields.timestamp, value))
            }),
            before.map_or(Bound::Unbounded, |value| {
                Bound::Excluded(Term::from_field_i64(fields.timestamp, value))
            }),
        )));
    }
    Ok(if queries.len() == 1 {
        queries.pop().unwrap()
    } else {
        Box::new(BooleanQuery::intersection(queries))
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
    let location = catalog
        .query_row(
            "SELECT segment,archive_offset,frame_bytes FROM messages WHERE doc_id=?1",
            [doc_id as i64],
            |row| {
                Ok(ArchiveLocation {
                    segment: row.get(0)?,
                    offset: row.get::<_, i64>(1)? as u64,
                    frame_bytes: row.get::<_, i64>(2)? as u64,
                })
            },
        )
        .map_err(sqlite_io)?;
    read_record(&root.join("archive"), &location).map(|(_, raw)| raw)
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
    io::Error::new(io::ErrorKind::Other, error)
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
        let path = root.join("segment-000000.arc");
        let length = fs::metadata(&path).unwrap().len();
        let file = OpenOptions::new().append(true).open(&path).unwrap();
        file.set_len(length + 9).unwrap();
        drop(file);
        let (frames, truncated) = recover_segments(&root).unwrap();
        assert_eq!(frames, 1);
        assert_eq!(truncated, 9);
        let _ = fs::remove_dir_all(&root);
    }
}
