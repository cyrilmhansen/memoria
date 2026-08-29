use crate::GmailIndexStats;
use async_imap::imap_proto::{AttributeValue, Response, Status};
use async_imap::types::{Fetch, NameAttribute};
use futures::TryStreamExt;
use rusqlite::{Connection, OptionalExtension};
use rustls::pki_types::{pem::PemObject, CertificateDer, ServerName};
use std::collections::HashSet;
use std::fmt::Debug;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;
use tokio::io::{AsyncRead, AsyncWrite};
use tokio::net::TcpStream;
use tokio::runtime::Builder;
use tokio::time::timeout;
use tokio_rustls::TlsConnector;

#[derive(Clone, Debug)]
pub struct ImapConfig {
    pub host: String,
    pub server_name: String,
    pub port: u16,
    pub username: String,
    pub password: String,
    pub ca_cert: PathBuf,
    pub mailbox: String,
    pub mailboxes: Vec<String>,
    pub all_mailboxes: bool,
    pub source_account: String,
    pub limit: Option<u32>,
    pub timeout: Duration,
}

pub fn imap_source_account(username: &str, host: &str, port: u16) -> String {
    format!("imap:{username}@{host}:{port}")
}

const EXACT_FETCH_ITEMS: &str = "(UID BODY.PEEK[])";

pub fn is_canonical_imap_source_account(source_account: &str) -> bool {
    let Some(value) = source_account.strip_prefix("imap:") else {
        return false;
    };
    let Some((username_host, port)) = value.rsplit_once(':') else {
        return false;
    };
    let Some((username, host)) = username_host.split_once('@') else {
        return false;
    };
    !username.is_empty() && !host.is_empty() && port.parse::<u16>().is_ok_and(|port| port > 0)
}

pub fn validate_source_configuration(config: &ImapConfig) -> Result<(), ImapError> {
    if config.username.is_empty() || config.host.is_empty() || config.port == 0 {
        return Err(ImapError::Config(
            "IMAP username, host and port must be valid".into(),
        ));
    }
    let expected = imap_source_account(&config.username, &config.host, config.port);
    if !is_canonical_imap_source_account(&expected) || config.source_account != expected {
        return Err(ImapError::Config(
            "source account does not match authenticated IMAP credentials".into(),
        ));
    }
    Ok(())
}

pub fn imap_message_identity(
    source_account: &str,
    mailbox: &str,
    uid_validity: u32,
    uid: u32,
) -> String {
    format!("imap:{source_account}:{mailbox}:{uid_validity}:{uid}")
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ImapMailbox {
    pub name: String,
    pub delimiter: Option<String>,
    pub attributes: Vec<String>,
    pub special_use: Vec<String>,
    pub selectable: bool,
}

#[derive(Clone, Debug, Default)]
pub struct ImapDiscovery {
    pub capabilities: Vec<String>,
    pub mailboxes: Vec<ImapMailbox>,
}

#[derive(Clone, Debug, Default)]
pub struct ImapMailboxResult {
    pub mailbox: String,
    pub stats: Option<ImapSyncStats>,
    pub error: Option<String>,
}

#[derive(Clone, Debug, Default)]
pub struct ImapMultiSyncStats {
    pub discovery: ImapDiscovery,
    pub results: Vec<ImapMailboxResult>,
}

#[derive(Clone, Debug, Default)]
pub struct ImapSyncStats {
    pub examined: u64,
    pub raw_fetched: u64,
    pub new_messages: u64,
    pub network_bytes: u64,
    pub archive_bytes_added: u64,
    pub uid_validity: u32,
    pub uid_next: u32,
    pub frontier_before: u32,
    pub frontier_after: Option<u32>,
    pub index: Option<GmailIndexStats>,
}

const IMPORT_BATCH_RECORD_LIMIT: usize = 64;
const IMPORT_BATCH_BYTES_LIMIT: u64 = 8 * 1024 * 1024;

#[derive(Debug)]
pub enum ImapError {
    Config(String),
    Io(String),
    Protocol(String),
    UidValidityChanged { expected: u32, observed: u32 },
    SourceUnavailable,
    FetchAmbiguous,
    FetchWrongUid { expected: u32, observed: u32 },
}

struct FetchRun {
    uid_validity: u32,
    uid_next: u32,
    examined: u64,
    scanned_through_uid: Option<u32>,
}

impl std::fmt::Display for ImapError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Config(value) => write!(formatter, "configuration: {value}"),
            Self::Io(value) => write!(formatter, "I/O error: {value}"),
            Self::Protocol(value) => write!(formatter, "IMAP error: {value}"),
            Self::UidValidityChanged { expected, observed } => write!(
                formatter,
                "UIDVALIDITY changed for mailbox (expected {expected}, observed {observed})"
            ),
            Self::SourceUnavailable => write!(formatter, "IMAP message is unavailable"),
            Self::FetchAmbiguous => write!(formatter, "IMAP FETCH response is ambiguous"),
            Self::FetchWrongUid { expected, observed } => write!(
                formatter,
                "IMAP FETCH returned UID {observed}, requested {expected}"
            ),
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ImapRawMessage {
    pub uid_validity: u32,
    pub uid: u32,
    pub raw: Vec<u8>,
}

pub fn fetch_exact_raw(
    config: &ImapConfig,
    mailbox: &str,
    expected_uid_validity: u32,
    uid: u32,
) -> Result<ImapRawMessage, ImapError> {
    validate_source_configuration(config)?;
    let runtime = Builder::new_multi_thread()
        .enable_io()
        .enable_time()
        .build()
        .map_err(|error| ImapError::Io(format!("create Tokio runtime: {error}")))?;
    runtime.block_on(fetch_exact_raw_async(
        config,
        mailbox,
        expected_uid_validity,
        uid,
    ))
}

impl std::error::Error for ImapError {}

#[derive(Debug)]
struct FetchedMessage {
    uid_validity: u32,
    uid: u32,
    flags_json: String,
    internal_date: Option<String>,
    internal_date_ms: Option<i64>,
    rfc822_size: Option<u32>,
    raw: Vec<u8>,
}

pub fn sync_imap(root: &Path, config: &ImapConfig) -> Result<ImapSyncStats, ImapError> {
    validate_source_configuration(config)?;
    if config.mailbox.is_empty() {
        return Err(ImapError::Config("mailbox must not be empty".into()));
    }
    if config.limit == Some(0) {
        return Err(ImapError::Config("limit must be greater than zero".into()));
    }
    let mut session = crate::ArchiveSession::create(root, 64 * 1024 * 1024)
        .map_err(|error| ImapError::Io(error.to_string()))?;
    let (writer, metadata) = session.parts_mut();
    let known_uid_validity =
        crate::imap_known_uid_validity(metadata, &config.source_account, &config.mailbox)
            .map_err(|error| ImapError::Io(error.to_string()))?;
    let scan_state = crate::imap_scan_state(metadata, &config.source_account, &config.mailbox)
        .map_err(|error| ImapError::Io(error.to_string()))?;
    let frontier_before = scan_state.map(|(_, frontier)| frontier).unwrap_or(0);

    let mut stats = ImapSyncStats {
        frontier_before,
        ..Default::default()
    };
    let mut next_doc_id =
        crate::next_doc_id(metadata).map_err(|error| ImapError::Io(error.to_string()))?;
    let mut staged = Vec::new();
    let mut pending_ids = HashSet::new();
    let runtime = Builder::new_multi_thread()
        .enable_io()
        .enable_time()
        .build()
        .map_err(|error| ImapError::Io(format!("create Tokio runtime: {error}")))?;
    let fetch_start = if scan_state.is_some() {
        frontier_before.saturating_add(1)
    } else {
        1
    };
    let fetch_result = runtime.block_on(fetch_mailbox(
        config,
        known_uid_validity,
        fetch_start,
        config.limit,
        |message| {
            stats.raw_fetched += 1;
            stats.network_bytes += message.raw.len() as u64;
            if imap_identity_is_valid(
                root,
                metadata,
                &config.source_account,
                &config.mailbox,
                message.uid_validity,
                message.uid,
            )? {
                return Ok(());
            }
            let identity = (
                config.source_account.clone(),
                config.mailbox.clone(),
                message.uid_validity,
                message.uid,
            );
            if pending_ids.contains(&identity) {
                return Ok(());
            }
            ensure_imap_message_id_available(
                metadata,
                &config.source_account,
                &config.mailbox,
                message.uid_validity,
                message.uid,
            )?;
            let doc_id = next_doc_id;
            next_doc_id = next_doc_id
                .checked_add(1)
                .ok_or_else(|| ImapError::Io("document ID overflow".into()))?;
            let location = writer
                .append_raw(doc_id as u64, &message.raw)
                .map_err(|error| ImapError::Io(error.to_string()))?;
            pending_ids.insert(identity);
            staged.push(crate::ImapBatchRecord::new(
                config.source_account.clone(),
                config.mailbox.clone(),
                message.uid_validity,
                message.uid,
                message.flags_json.clone(),
                message.internal_date.clone(),
                message.internal_date_ms,
                message.rfc822_size,
                doc_id,
                location,
            ));
            if batch_full(&staged) {
                flush_batch(metadata, writer, &mut staged, &mut pending_ids, &mut stats)?;
            }
            Ok(())
        },
    ));
    drop(runtime);
    let fetch_result = fetch_result?;
    stats.uid_validity = fetch_result.uid_validity;
    stats.uid_next = fetch_result.uid_next;
    stats.examined = fetch_result.examined;
    flush_batch_if_needed(metadata, writer, &mut staged, &mut pending_ids, &mut stats)?;
    if let Some(scanned_through_uid) = fetch_result.scanned_through_uid {
        crate::upsert_imap_scan_state(
            metadata,
            &config.source_account,
            &config.mailbox,
            fetch_result.uid_validity,
            scanned_through_uid,
            fetch_result.uid_next,
        )
        .map_err(|error| ImapError::Io(error.to_string()))?;
        stats.frontier_after = Some(scanned_through_uid);
    } else {
        stats.frontier_after = Some(frontier_before);
    }
    stats.index = Some(
        crate::index_gmail_archive(root)
            .map_err(|error| ImapError::Io(format!("update search index: {error}")))?,
    );
    Ok(stats)
}

fn batch_full(batch: &[crate::ImapBatchRecord]) -> bool {
    batch.len() >= IMPORT_BATCH_RECORD_LIMIT
        || batch
            .iter()
            .map(crate::ImapBatchRecord::frame_bytes)
            .sum::<u64>()
            >= IMPORT_BATCH_BYTES_LIMIT
}

fn imap_identity_is_valid(
    root: &Path,
    connection: &Connection,
    source: &str,
    mailbox: &str,
    uid_validity: u32,
    uid: u32,
) -> Result<bool, ImapError> {
    let doc_id = connection
        .query_row(
            "SELECT doc_id FROM imap_messages WHERE source_account=?1 AND mailbox=?2 AND uid_validity=?3 AND uid=?4",
            rusqlite::params![source, mailbox, uid_validity as i64, uid as i64],
            |row| row.get::<_, i64>(0),
        )
        .optional()
        .map_err(|error| ImapError::Io(error.to_string()))?;
    let Some(doc_id) = doc_id else {
        return Ok(false);
    };
    crate::validate_catalog_record(
        &root.join("archive"),
        connection,
        doc_id,
        &imap_message_identity(source, mailbox, uid_validity, uid),
    )
    .map_err(|error| {
        ImapError::Io(format!(
            "known IMAP identity {source}/{mailbox}/{uid_validity}/{uid} is not valid locally: {error}"
        ))
    })?;
    Ok(true)
}

fn ensure_imap_message_id_available(
    connection: &Connection,
    source: &str,
    mailbox: &str,
    uid_validity: u32,
    uid: u32,
) -> Result<(), ImapError> {
    let canonical = imap_message_identity(source, mailbox, uid_validity, uid);
    let exists = connection
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM messages WHERE message_id=?1)",
            [canonical.as_str()],
            |row| row.get::<_, bool>(0),
        )
        .map_err(|error| ImapError::Io(error.to_string()))?;
    if exists {
        return Err(ImapError::Io(format!(
            "historical messages row already uses IMAP identity {source}/{mailbox}/{uid_validity}/{uid} without source metadata"
        )));
    }
    Ok(())
}

fn flush_batch_if_needed(
    connection: &crate::CatalogueConnection,
    writer: &mut crate::ArchiveWriter,
    staged: &mut Vec<crate::ImapBatchRecord>,
    pending_ids: &mut HashSet<(String, String, u32, u32)>,
    stats: &mut ImapSyncStats,
) -> Result<(), ImapError> {
    if staged.is_empty() {
        return Ok(());
    }
    flush_batch(connection, writer, staged, pending_ids, stats)
}

fn flush_batch(
    connection: &crate::CatalogueConnection,
    writer: &mut crate::ArchiveWriter,
    staged: &mut Vec<crate::ImapBatchRecord>,
    pending_ids: &mut HashSet<(String, String, u32, u32)>,
    stats: &mut ImapSyncStats,
) -> Result<(), ImapError> {
    let durable = writer
        .durable_barrier()
        .map_err(|error| ImapError::Io(error.to_string()))?;
    let records = staged.len() as u64;
    let frame_bytes = staged
        .iter()
        .map(crate::ImapBatchRecord::frame_bytes)
        .sum::<u64>();
    crate::publish_imap_batch(connection, staged, &durable)
        .map_err(|error| ImapError::Io(error.to_string()))?;
    stats.new_messages += records;
    stats.archive_bytes_added += frame_bytes;
    staged.clear();
    pending_ids.clear();
    Ok(())
}

pub fn sync_imap_mailboxes(
    root: &Path,
    config: &ImapConfig,
) -> Result<ImapMultiSyncStats, ImapError> {
    validate_source_configuration(config)?;
    let discovery = discover_mailboxes(config)?;
    let mut session = crate::ArchiveSession::create(root, 64 * 1024 * 1024)
        .map_err(|error| ImapError::Io(error.to_string()))?;
    let (_, metadata) = session.parts_mut();
    for mailbox in &discovery.mailboxes {
        crate::upsert_imap_mailbox(
            metadata,
            &config.source_account,
            &mailbox.name,
            mailbox.delimiter.as_deref(),
            &serde_json::to_string(&mailbox.attributes)
                .map_err(|error| ImapError::Io(error.to_string()))?,
            &serde_json::to_string(&mailbox.special_use)
                .map_err(|error| ImapError::Io(error.to_string()))?,
            mailbox.selectable,
        )
        .map_err(|error| ImapError::Io(error.to_string()))?;
    }
    drop(session);
    let selected = if config.all_mailboxes {
        discovery
            .mailboxes
            .iter()
            .filter(|mailbox| mailbox.selectable)
            .map(|mailbox| mailbox.name.clone())
            .collect::<Vec<_>>()
    } else if !config.mailboxes.is_empty() {
        config.mailboxes.clone()
    } else {
        vec![config.mailbox.clone()]
    };
    if selected.is_empty() {
        return Err(ImapError::Config("no selectable mailbox selected".into()));
    }
    let known_names = discovery
        .mailboxes
        .iter()
        .map(|mailbox| mailbox.name.as_str())
        .collect::<std::collections::HashSet<_>>();
    let mut results = Vec::with_capacity(selected.len());
    for mailbox in selected {
        let mut mailbox_config = config.clone();
        mailbox_config.mailbox = mailbox.clone();
        mailbox_config.mailboxes.clear();
        mailbox_config.all_mailboxes = false;
        let result = if !known_names.contains(mailbox.as_str()) {
            Err(ImapError::Config(format!(
                "mailbox not returned by LIST: {mailbox}"
            )))
        } else if discovery
            .mailboxes
            .iter()
            .find(|entry| entry.name == mailbox)
            .is_some_and(|entry| !entry.selectable)
        {
            Err(ImapError::Config(format!(
                "mailbox is not selectable: {mailbox}"
            )))
        } else {
            sync_imap(root, &mailbox_config)
        };
        match result {
            Ok(stats) => results.push(ImapMailboxResult {
                mailbox,
                stats: Some(stats),
                error: None,
            }),
            Err(error) => results.push(ImapMailboxResult {
                mailbox,
                stats: None,
                error: Some(error.to_string()),
            }),
        }
    }
    Ok(ImapMultiSyncStats { discovery, results })
}

pub fn discover_mailboxes(config: &ImapConfig) -> Result<ImapDiscovery, ImapError> {
    let runtime = Builder::new_multi_thread()
        .enable_io()
        .enable_time()
        .build()
        .map_err(|error| ImapError::Io(format!("create Tokio runtime: {error}")))?;
    runtime.block_on(async {
        let stream = tls_stream(config).await?;
        let mut client = async_imap::Client::new(stream);
        timeout(config.timeout, client.read_response())
            .await
            .map_err(|_| ImapError::Protocol("greeting timeout".into()))?
            .map_err(|error| ImapError::Protocol(format!("greeting failed: {error}")))?
            .ok_or_else(|| ImapError::Protocol("server closed before greeting".into()))?;
        let (mut session, _) = timeout(
            config.timeout,
            client.login_with_capabilities(&config.username, &config.password),
        )
        .await
        .map_err(|_| ImapError::Protocol("login timeout".into()))?
        .map_err(|(error, _)| ImapError::Protocol(format!("login failed: {error}")))?;
        let capabilities = timeout(config.timeout, session.capabilities())
            .await
            .map_err(|_| ImapError::Protocol("CAPABILITY timeout".into()))?
            .map_err(|error| ImapError::Protocol(format!("CAPABILITY failed: {error}")))?
            .iter()
            .map(|capability| format!("{capability:?}"))
            .collect::<Vec<_>>();
        let mut mailboxes = Vec::new();
        {
            let mut names = timeout(config.timeout, session.list(None, Some("*")))
                .await
                .map_err(|_| ImapError::Protocol("LIST timeout".into()))?
                .map_err(|error| ImapError::Protocol(format!("LIST failed: {error}")))?;
            while let Some(name) = timeout(config.timeout, names.try_next())
                .await
                .map_err(|_| ImapError::Protocol("LIST response timeout".into()))?
                .map_err(|error| ImapError::Protocol(format!("LIST response failed: {error}")))?
            {
                let attributes = name
                    .attributes()
                    .iter()
                    .map(|attribute| format!("{attribute:?}"))
                    .collect::<Vec<_>>();
                let special_use = name
                    .attributes()
                    .iter()
                    .filter_map(special_use_name)
                    .map(str::to_owned)
                    .collect::<Vec<_>>();
                mailboxes.push(ImapMailbox {
                    name: name.name().to_owned(),
                    delimiter: name.delimiter().map(str::to_owned),
                    selectable: !name
                        .attributes()
                        .iter()
                        .any(|attribute| matches!(attribute, NameAttribute::NoSelect)),
                    attributes,
                    special_use,
                });
            }
        }
        timeout(config.timeout, session.logout())
            .await
            .map_err(|_| ImapError::Protocol("logout timeout".into()))?
            .map_err(|error| ImapError::Protocol(format!("logout failed: {error}")))?;
        Ok(ImapDiscovery {
            capabilities,
            mailboxes,
        })
    })
}

fn special_use_name(attribute: &NameAttribute<'_>) -> Option<&'static str> {
    match attribute {
        NameAttribute::All => Some("\\All"),
        NameAttribute::Archive => Some("\\Archive"),
        NameAttribute::Drafts => Some("\\Drafts"),
        NameAttribute::Flagged => Some("\\Flagged"),
        NameAttribute::Junk => Some("\\Junk"),
        NameAttribute::Sent => Some("\\Sent"),
        NameAttribute::Trash => Some("\\Trash"),
        _ => None,
    }
}

async fn fetch_mailbox<F>(
    config: &ImapConfig,
    expected_uid_validity: Option<u32>,
    start_uid: u32,
    limit: Option<u32>,
    on_message: F,
) -> Result<FetchRun, ImapError>
where
    F: FnMut(FetchedMessage) -> Result<(), ImapError>,
{
    let stream = tls_stream(config).await?;
    fetch_session(
        config,
        stream,
        expected_uid_validity,
        start_uid,
        limit,
        on_message,
    )
    .await
}

async fn fetch_exact_raw_async(
    config: &ImapConfig,
    mailbox_name: &str,
    expected_uid_validity: u32,
    uid: u32,
) -> Result<ImapRawMessage, ImapError> {
    let stream = tls_stream(config).await?;
    let mut client = async_imap::Client::new(stream);
    timeout(config.timeout, client.read_response())
        .await
        .map_err(|_| ImapError::Protocol("greeting timeout".into()))?
        .map_err(|error| ImapError::Protocol(format!("greeting failed: {error}")))?
        .ok_or_else(|| ImapError::Protocol("server closed before greeting".into()))?;
    let (mut session, _) = timeout(
        config.timeout,
        client.login_with_capabilities(&config.username, &config.password),
    )
    .await
    .map_err(|_| ImapError::Protocol("login timeout".into()))?
    .map_err(|(error, _)| ImapError::Protocol(format!("login failed: {error}")))?;
    let mailbox = timeout(config.timeout, session.examine(mailbox_name))
        .await
        .map_err(|_| ImapError::Protocol("EXAMINE timeout".into()))?
        .map_err(|error| ImapError::Protocol(format!("EXAMINE failed: {error}")))?;
    let observed_uid_validity = mailbox
        .uid_validity
        .ok_or_else(|| ImapError::Protocol("EXAMINE returned no UIDVALIDITY".into()))?;
    let result = fetch_exact_uid_in_session(
        &mut session,
        observed_uid_validity,
        expected_uid_validity,
        uid,
        config.timeout,
    )
    .await?;
    timeout(config.timeout, session.logout())
        .await
        .map_err(|_| ImapError::Protocol("logout timeout".into()))?
        .map_err(|error| ImapError::Protocol(format!("logout failed: {error}")))?;
    Ok(result)
}

#[derive(Default)]
struct FetchAccumulator {
    fetch_responses: usize,
    uid_values: Vec<u32>,
    body_payloads: Vec<Vec<u8>>,
    responses_without_uid: usize,
}

fn inspect_fetch_response(response: &Response<'_>, accumulator: &mut FetchAccumulator) {
    let Response::Fetch(_, attributes) = response else {
        return;
    };
    accumulator.fetch_responses += 1;
    let mut response_uids = 0;
    for attribute in attributes {
        match attribute {
            AttributeValue::Uid(uid) => {
                response_uids += 1;
                accumulator.uid_values.push(*uid);
            }
            AttributeValue::BodySection {
                section: None,
                index: None,
                data: Some(data),
            } => accumulator.body_payloads.push(data.to_vec()),
            _ => {}
        }
    }
    if response_uids == 0 {
        accumulator.responses_without_uid += 1;
    }
}

fn resolve_fetch_accumulator(
    accumulator: FetchAccumulator,
    requested_uid: u32,
    uid_validity: u32,
) -> Result<Option<ImapRawMessage>, ImapError> {
    if accumulator.fetch_responses == 0 {
        return Ok(None);
    }
    if accumulator.fetch_responses == 1
        && accumulator.responses_without_uid == 1
        && accumulator.uid_values.is_empty()
        && accumulator.body_payloads.len() <= 1
    {
        return Ok(None);
    }
    if accumulator.fetch_responses != 1
        || accumulator.responses_without_uid > 0
        || accumulator.uid_values.len() != 1
        || accumulator.body_payloads.len() != 1
    {
        return Err(ImapError::FetchAmbiguous);
    }
    let returned_uid = accumulator.uid_values[0];
    if returned_uid != requested_uid {
        return Err(ImapError::FetchWrongUid {
            expected: requested_uid,
            observed: returned_uid,
        });
    }
    Ok(Some(ImapRawMessage {
        uid_validity,
        uid: returned_uid,
        raw: accumulator.body_payloads.into_iter().next().unwrap(),
    }))
}

async fn fetch_exact_uid_in_session<T>(
    session: &mut async_imap::Session<T>,
    observed_uid_validity: u32,
    expected_uid_validity: u32,
    uid: u32,
    timeout_duration: Duration,
) -> Result<ImapRawMessage, ImapError>
where
    T: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin + Debug + Send,
{
    if observed_uid_validity != expected_uid_validity {
        return Err(ImapError::UidValidityChanged {
            expected: expected_uid_validity,
            observed: observed_uid_validity,
        });
    }
    let request_id = timeout(
        timeout_duration,
        session.run_command(format!("UID FETCH {uid} {EXACT_FETCH_ITEMS}")),
    )
    .await
    .map_err(|_| ImapError::Protocol("UID FETCH timeout".into()))?
    .map_err(|error| ImapError::Protocol(format!("UID FETCH failed: {error}")))?;
    let mut accumulator = FetchAccumulator::default();
    loop {
        let response = timeout(timeout_duration, session.read_response())
            .await
            .map_err(|_| ImapError::Protocol("FETCH response timeout".into()))?
            .map_err(|error| ImapError::Protocol(format!("FETCH response failed: {error}")))?
            .ok_or_else(|| {
                ImapError::Protocol("connection closed before FETCH completion".into())
            })?;
        match response.parsed() {
            Response::Fetch(_, _) => inspect_fetch_response(response.parsed(), &mut accumulator),
            Response::Done { tag, status, .. } if tag == &request_id => {
                if *status != Status::Ok {
                    return Err(ImapError::Protocol(format!(
                        "UID FETCH completed with status {status:?}"
                    )));
                }
                return resolve_fetch_accumulator(accumulator, uid, observed_uid_validity)?
                    .ok_or(ImapError::SourceUnavailable);
            }
            _ => {}
        }
    }
}

async fn tls_stream(
    config: &ImapConfig,
) -> Result<tokio_rustls::client::TlsStream<TcpStream>, ImapError> {
    let stream = timeout(
        config.timeout,
        TcpStream::connect((config.host.as_str(), config.port)),
    )
    .await
    .map_err(|_| ImapError::Protocol("connection timeout".into()))?
    .map_err(|error| ImapError::Io(format!("connection failed: {error}")))?;
    stream
        .set_nodelay(true)
        .map_err(|error| ImapError::Io(format!("set TCP options: {error}")))?;
    let ca = CertificateDer::from_pem_file(&config.ca_cert)
        .map_err(|error| ImapError::Config(format!("read CA certificate: {error}")))?;
    let mut roots = rustls::RootCertStore::empty();
    roots
        .add(ca)
        .map_err(|error| ImapError::Config(format!("add CA certificate: {error}")))?;
    let provider = rustls::crypto::aws_lc_rs::default_provider();
    let tls_config = rustls::ClientConfig::builder_with_provider(provider.into())
        .with_safe_default_protocol_versions()
        .map_err(|error| ImapError::Config(format!("TLS configuration: {error}")))?
        .with_root_certificates(roots)
        .with_no_client_auth();
    let connector = TlsConnector::from(Arc::new(tls_config));
    let server_name = ServerName::try_from(config.server_name.clone())
        .map_err(|error| ImapError::Config(format!("invalid TLS server name: {error}")))?;
    timeout(config.timeout, connector.connect(server_name, stream))
        .await
        .map_err(|_| ImapError::Protocol("TLS handshake timeout".into()))?
        .map_err(|error| ImapError::Protocol(format!("TLS handshake failed: {error}")))
}

async fn fetch_session<S>(
    config: &ImapConfig,
    stream: S,
    expected_uid_validity: Option<u32>,
    start_uid: u32,
    limit: Option<u32>,
    mut on_message: impl FnMut(FetchedMessage) -> Result<(), ImapError>,
) -> Result<FetchRun, ImapError>
where
    S: AsyncRead + AsyncWrite + Unpin + Send + Debug + 'static,
{
    let mut client = async_imap::Client::new(stream);
    timeout(config.timeout, client.read_response())
        .await
        .map_err(|_| ImapError::Protocol("greeting timeout".into()))?
        .map_err(|error| ImapError::Protocol(format!("greeting failed: {error}")))?
        .ok_or_else(|| ImapError::Protocol("server closed before greeting".into()))?;
    let (mut session, _) = timeout(
        config.timeout,
        client.login_with_capabilities(&config.username, &config.password),
    )
    .await
    .map_err(|_| ImapError::Protocol("login timeout".into()))?
    .map_err(|(error, _)| ImapError::Protocol(format!("login failed: {error}")))?;
    let mailbox = timeout(config.timeout, session.examine(&config.mailbox))
        .await
        .map_err(|_| ImapError::Protocol("EXAMINE timeout".into()))?
        .map_err(|error| ImapError::Protocol(format!("EXAMINE failed: {error}")))?;
    let uid_validity = mailbox
        .uid_validity
        .ok_or_else(|| ImapError::Protocol("EXAMINE returned no UIDVALIDITY".into()))?;
    if let Some(expected) = expected_uid_validity {
        if expected != uid_validity {
            return Err(ImapError::UidValidityChanged {
                expected,
                observed: uid_validity,
            });
        }
    }
    let uid_next = mailbox
        .uid_next
        .ok_or_else(|| ImapError::Protocol("EXAMINE returned no UIDNEXT".into()))?;
    let snapshot_last_uid = uid_next.saturating_sub(1);
    let end_uid = limit
        .map(|count| start_uid.saturating_add(count.saturating_sub(1)))
        .unwrap_or(snapshot_last_uid)
        .min(snapshot_last_uid);
    let mut examined = 0;
    let scanned_through_uid = if start_uid <= end_uid {
        Some(end_uid)
    } else if limit.is_none() {
        Some(snapshot_last_uid)
    } else {
        None
    };
    if start_uid <= end_uid {
        let sequence = format!("{start_uid}:{end_uid}");
        let mut fetched = timeout(
            config.timeout,
            session.uid_fetch(
                &sequence,
                "(UID FLAGS INTERNALDATE RFC822.SIZE BODY.PEEK[])",
            ),
        )
        .await
        .map_err(|_| ImapError::Protocol("UID FETCH timeout".into()))?
        .map_err(|error| ImapError::Protocol(format!("UID FETCH failed: {error}")))?;
        while let Some(fetch) = timeout(config.timeout, fetched.try_next())
            .await
            .map_err(|_| ImapError::Protocol("FETCH response timeout".into()))?
            .map_err(|error| ImapError::Protocol(format!("FETCH response failed: {error}")))?
        {
            let message = fetch_to_owned(&fetch, uid_validity)?;
            examined += 1;
            on_message(message)?;
        }
    }
    timeout(config.timeout, session.logout())
        .await
        .map_err(|_| ImapError::Protocol("logout timeout".into()))?
        .map_err(|error| ImapError::Protocol(format!("logout failed: {error}")))?;
    Ok(FetchRun {
        uid_validity,
        uid_next,
        examined,
        scanned_through_uid,
    })
}

fn fetch_to_owned(fetch: &Fetch, uid_validity: u32) -> Result<FetchedMessage, ImapError> {
    let uid = fetch
        .uid
        .ok_or_else(|| ImapError::Protocol("FETCH response had no UID".into()))?;
    let raw = fetch
        .body()
        .ok_or_else(|| ImapError::Protocol("FETCH response had no BODY.PEEK[]".into()))?
        .to_vec();
    let internal_date = fetch.internal_date();
    Ok(FetchedMessage {
        uid_validity,
        uid,
        flags_json: serde_json::to_string(
            &fetch
                .flags()
                .map(|flag| format!("{flag:?}"))
                .collect::<Vec<_>>(),
        )
        .map_err(|error| ImapError::Protocol(format!("serialize FLAGS: {error}")))?,
        internal_date: internal_date.map(|value| value.to_rfc3339()),
        internal_date_ms: internal_date.map(|value| value.timestamp_millis()),
        rfc822_size: fetch.size,
        raw,
    })
}

#[cfg(test)]
mod tests {
    use super::{
        imap_source_account, inspect_fetch_response, resolve_fetch_accumulator, special_use_name,
        FetchAccumulator, ImapError, NameAttribute, EXACT_FETCH_ITEMS,
    };
    use async_imap::imap_proto::Response;
    use std::collections::VecDeque;
    use std::io;
    use std::pin::Pin;
    use std::sync::{Arc, Mutex};
    use std::task::{Context, Poll};
    use tokio::io::{AsyncRead, AsyncWrite, ReadBuf};

    #[derive(Clone, Copy)]
    enum WireFetch {
        Exact,
        None,
        NoUid,
        ValidAndNoUid,
        WrongUid,
        Multiple,
        CompletionNo,
    }

    struct WireState {
        input: VecDeque<u8>,
        client_buffer: Vec<u8>,
        commands: Vec<String>,
        uid_validity: u32,
        fetch: WireFetch,
    }

    #[derive(Clone)]
    struct WireStream {
        state: Arc<Mutex<WireState>>,
    }

    impl WireStream {
        fn new(uid_validity: u32, fetch: WireFetch) -> (Self, Arc<Mutex<WireState>>) {
            let state = Arc::new(Mutex::new(WireState {
                input: VecDeque::from(b"* OK IMAP4rev1 ready\r\n".to_vec()),
                client_buffer: Vec::new(),
                commands: Vec::new(),
                uid_validity,
                fetch,
            }));
            (
                Self {
                    state: state.clone(),
                },
                state,
            )
        }

        fn respond(state: &mut WireState, line: &str) {
            let Some(tag) = line.split_whitespace().next() else {
                return;
            };
            let upper = line.to_ascii_uppercase();
            let response = if upper.contains(" LOGIN ") {
                format!("{tag} OK LOGIN completed\r\n")
            } else if upper.contains(" EXAMINE ") {
                format!(
                    "* 1 EXISTS\r\n* OK [UIDVALIDITY {}] valid\r\n* OK [UIDNEXT 43] next\r\n{tag} OK EXAMINE completed\r\n",
                    state.uid_validity
                )
            } else if upper.contains("UID FETCH") {
                match state.fetch {
                    WireFetch::Exact => format!(
                        "* 1 FETCH (UID 42 BODY[] {{3}}\r\nabc)\r\n{tag} OK FETCH completed\r\n"
                    ),
                    WireFetch::None => format!("{tag} OK FETCH completed\r\n"),
                    WireFetch::NoUid => format!(
                        "* 1 FETCH (BODY[] {{3}}\r\nabc)\r\n{tag} OK FETCH completed\r\n"
                    ),
                    WireFetch::ValidAndNoUid => format!(
                        "* 1 FETCH (UID 42 BODY[] {{3}}\r\nabc)\r\n* 1 FETCH (BODY[] {{3}}\r\ndef)\r\n{tag} OK FETCH completed\r\n"
                    ),
                    WireFetch::WrongUid => format!(
                        "* 1 FETCH (UID 43 BODY[] {{3}}\r\nabc)\r\n{tag} OK FETCH completed\r\n"
                    ),
                    WireFetch::Multiple => format!(
                        "* 1 FETCH (UID 42 BODY[] {{3}}\r\nabc)\r\n* 2 FETCH (UID 43 BODY[] {{3}}\r\ndef)\r\n{tag} OK FETCH completed\r\n"
                    ),
                    WireFetch::CompletionNo => format!(
                        "* 1 FETCH (UID 42 BODY[] {{3}}\r\nabc)\r\n{tag} NO FETCH failed\r\n"
                    ),
                }
            } else if upper.contains(" LOGOUT") {
                format!("* BYE closing\r\n{tag} OK LOGOUT completed\r\n")
            } else {
                format!("{tag} OK completed\r\n")
            };
            state.input.extend(response.into_bytes());
        }
    }

    impl std::fmt::Debug for WireStream {
        fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            formatter.write_str("WireStream")
        }
    }

    impl AsyncRead for WireStream {
        fn poll_read(
            self: Pin<&mut Self>,
            _cx: &mut Context<'_>,
            buffer: &mut ReadBuf<'_>,
        ) -> Poll<io::Result<()>> {
            let mut state = self.state.lock().unwrap();
            let count = buffer.remaining().min(state.input.len());
            for byte in state.input.drain(..count) {
                buffer.put_slice(&[byte]);
            }
            if count == 0 {
                Poll::Pending
            } else {
                Poll::Ready(Ok(()))
            }
        }
    }

    impl AsyncWrite for WireStream {
        fn poll_write(
            self: Pin<&mut Self>,
            _cx: &mut Context<'_>,
            bytes: &[u8],
        ) -> Poll<io::Result<usize>> {
            let mut state = self.state.lock().unwrap();
            state.client_buffer.extend_from_slice(bytes);
            while let Some(end) = state.client_buffer.iter().position(|byte| *byte == b'\n') {
                let line = state.client_buffer.drain(..=end).collect::<Vec<_>>();
                let line = String::from_utf8_lossy(&line).trim().to_string();
                state.commands.push(line.clone());
                Self::respond(&mut state, &line);
            }
            Poll::Ready(Ok(bytes.len()))
        }

        fn poll_flush(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<io::Result<()>> {
            Poll::Ready(Ok(()))
        }

        fn poll_shutdown(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<io::Result<()>> {
            Poll::Ready(Ok(()))
        }
    }

    fn run_wire(
        uid_validity: u32,
        expected_uid_validity: u32,
        fetch: WireFetch,
    ) -> (
        Result<super::ImapRawMessage, ImapError>,
        Arc<Mutex<WireState>>,
    ) {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_io()
            .enable_time()
            .build()
            .unwrap();
        let (stream, state) = WireStream::new(uid_validity, fetch);
        let result = runtime.block_on(async move {
            let mut client = async_imap::Client::new(stream);
            client.read_response().await.unwrap().unwrap();
            let (mut session, _) = client
                .login_with_capabilities("account", "password")
                .await
                .map_err(|(error, _)| error)
                .unwrap();
            let mailbox = session.examine("INBOX").await.unwrap();
            super::fetch_exact_uid_in_session(
                &mut session,
                mailbox.uid_validity.unwrap(),
                expected_uid_validity,
                42,
                Duration::from_millis(100),
            )
            .await
        });
        (result, state)
    }
    use std::time::{Duration, SystemTime, UNIX_EPOCH};

    #[test]
    fn imap_identity_is_mailbox_uidvalidity_and_uid() {
        let suffix = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let root = std::env::temp_dir().join(format!("memoria-imap-test-{suffix}"));
        let connection = crate::create_metadata(&root.join("metadata.sqlite")).unwrap();
        let mut writer =
            crate::ArchiveWriter::open(&root.join("archive"), 64 * 1024 * 1024).unwrap();
        let raw = b"From: imap@example.test\r\n\r\nfixture\r\n";
        writer.append_raw(0, raw).unwrap();
        let durable = writer.durable_barrier().unwrap();
        crate::insert_imap_metadata(
            &connection,
            "imap-test",
            "INBOX",
            17,
            42,
            "[\"Recent\"]",
            None,
            None,
            Some(raw.len() as u32),
            0,
            &durable.entries()[0],
        )
        .unwrap();
        assert!(crate::imap_message_exists(&connection, "imap-test", "INBOX", 17, 42).unwrap());
        assert!(!crate::imap_message_exists(&connection, "imap-test", "INBOX", 18, 42).unwrap());
        assert_eq!(
            connection
                .query_row(
                    "SELECT message_id FROM messages WHERE doc_id=0",
                    [],
                    |row| row.get::<_, String>(0)
                )
                .unwrap(),
            "imap:imap-test:INBOX:17:42"
        );
        drop(writer);
        drop(connection);
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn imap_scan_frontier_is_explicit_and_keyed_by_uidvalidity() {
        let suffix = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let root = std::env::temp_dir().join(format!("memoria-imap-scan-test-{suffix}"));
        let connection = crate::create_metadata(&root.join("metadata.sqlite")).unwrap();
        assert_eq!(
            crate::imap_scan_state(&connection, "imap-test", "INBOX").unwrap(),
            None
        );
        crate::upsert_imap_scan_state(&connection, "imap-test", "INBOX", 17, 12, 13).unwrap();
        assert_eq!(
            crate::imap_scan_state(&connection, "imap-test", "INBOX").unwrap(),
            Some((17, 12))
        );
        assert_eq!(
            crate::imap_known_uid_validity(&connection, "imap-test", "INBOX").unwrap(),
            Some(17)
        );
        crate::upsert_imap_scan_state(&connection, "imap-test", "INBOX", 17, 15, 16).unwrap();
        assert_eq!(
            crate::imap_scan_state(&connection, "imap-test", "INBOX").unwrap(),
            Some((17, 15))
        );
        drop(connection);
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn list_attributes_keep_special_use_separate_from_mailbox_name() {
        assert_eq!(special_use_name(&NameAttribute::Sent), Some("\\Sent"));
        assert_eq!(special_use_name(&NameAttribute::NoSelect), None);
        assert_eq!(
            special_use_name(&NameAttribute::Extension("\\HasChildren".into())),
            None
        );
    }

    #[test]
    fn exact_fetch_parser_is_single_uid_and_body_peek() {
        assert_eq!(EXACT_FETCH_ITEMS, "(UID BODY.PEEK[])");
        let (_, response) =
            Response::from_bytes(b"* 1 FETCH (UID 42 BODY[] {3}\r\nabc BODY[] {3}\r\ndef)\r\n")
                .unwrap();
        let mut accumulator = FetchAccumulator::default();
        inspect_fetch_response(&response, &mut accumulator);
        assert!(matches!(
            resolve_fetch_accumulator(accumulator, 42, 17),
            Err(ImapError::FetchAmbiguous)
        ));
        assert_eq!(
            imap_source_account("account-a", "imap.example.test", 993),
            "imap:account-a@imap.example.test:993"
        );
    }

    #[test]
    fn exact_fetch_uses_public_wire_path_and_waits_for_tagged_completion() {
        let (result, state) = run_wire(111, 111, WireFetch::Exact);
        assert_eq!(result.unwrap().raw, b"abc");
        let commands = &state.lock().unwrap().commands;
        assert!(commands.iter().any(|command| {
            let upper = command.to_ascii_uppercase();
            upper.contains("UID FETCH 42") && upper.contains("BODY.PEEK[]")
        }));
        assert!(!commands
            .iter()
            .any(|command| command.to_ascii_uppercase().contains("FETCH 42")
                && !command.to_ascii_uppercase().contains("UID FETCH")));

        let (result, state) = run_wire(222, 111, WireFetch::Exact);
        assert!(matches!(
            result,
            Err(ImapError::UidValidityChanged {
                expected: 111,
                observed: 222
            })
        ));
        assert!(!state
            .lock()
            .unwrap()
            .commands
            .iter()
            .any(|command| command.to_ascii_uppercase().contains("UID FETCH")));

        let (result, _) = run_wire(111, 111, WireFetch::NoUid);
        assert!(matches!(result, Err(ImapError::SourceUnavailable)));
        let (result, _) = run_wire(111, 111, WireFetch::WrongUid);
        assert!(matches!(
            result,
            Err(ImapError::FetchWrongUid {
                expected: 42,
                observed: 43
            })
        ));
        let (result, _) = run_wire(111, 111, WireFetch::Multiple);
        assert!(matches!(result, Err(ImapError::FetchAmbiguous)));
        let (result, _) = run_wire(111, 111, WireFetch::CompletionNo);
        assert!(matches!(result, Err(ImapError::Protocol(message)) if message.contains("status")));
    }

    #[test]
    fn exact_fetch_with_no_wire_fetch_response_is_source_unavailable() {
        let (result, state) = run_wire(111, 111, WireFetch::None);
        assert!(matches!(result, Err(ImapError::SourceUnavailable)));
        let commands = state.lock().unwrap().commands.clone();
        assert!(commands
            .iter()
            .any(|command| command.to_ascii_uppercase().contains("UID FETCH 42")));
    }

    #[test]
    fn exact_fetch_valid_response_plus_missing_uid_is_ambiguous() {
        let (result, state) = run_wire(111, 111, WireFetch::ValidAndNoUid);
        assert!(matches!(result, Err(ImapError::FetchAmbiguous)));
        let commands = state.lock().unwrap().commands.clone();
        assert!(commands
            .iter()
            .any(|command| command.to_ascii_uppercase().contains("UID FETCH 42")));
    }

    #[test]
    fn invalid_source_is_rejected_before_multi_mailbox_persistence() {
        let root = std::env::temp_dir().join(format!(
            "memoria-imap-invalid-source-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&root);
        let config = super::ImapConfig {
            host: "imap.example.test".into(),
            server_name: "imap.example.test".into(),
            port: 993,
            username: "account-a".into(),
            password: "password".into(),
            ca_cert: root.join("ca.pem"),
            mailbox: "INBOX".into(),
            mailboxes: vec!["INBOX".into(), "Archive".into()],
            all_mailboxes: true,
            source_account: "legacy-arbitrary-key".into(),
            limit: None,
            timeout: Duration::from_millis(10),
        };
        assert!(matches!(
            super::sync_imap_mailboxes(&root, &config),
            Err(ImapError::Config(_))
        ));
        assert!(!root.join("metadata.sqlite").exists());
        assert!(!root.join("archive").exists());
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn source_configuration_uses_one_structural_predicate() {
        assert!(!super::is_canonical_imap_source_account("imap:@host:993"));
        assert!(!super::is_canonical_imap_source_account("imap:user@:993"));
        assert!(!super::is_canonical_imap_source_account("imap:user@host:0"));
        assert!(super::is_canonical_imap_source_account(
            "imap:user@host:993"
        ));
    }
}
