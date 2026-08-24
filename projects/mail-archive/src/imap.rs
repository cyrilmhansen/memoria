use crate::GmailIndexStats;
use async_imap::types::Fetch;
use futures::TryStreamExt;
use rustls::pki_types::{pem::PemObject, CertificateDer, ServerName};
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
    pub source_account: String,
    pub limit: Option<u32>,
    pub timeout: Duration,
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

#[derive(Debug)]
pub enum ImapError {
    Config(String),
    Io(String),
    Protocol(String),
    UidValidityChanged { expected: u32, observed: u32 },
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
        }
    }
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
    if config.mailbox.is_empty() {
        return Err(ImapError::Config("mailbox must not be empty".into()));
    }
    if config.source_account.is_empty() {
        return Err(ImapError::Config("source account must not be empty".into()));
    }
    let metadata = crate::create_metadata(&root.join("metadata.sqlite"))
        .map_err(|error| ImapError::Io(error.to_string()))?;
    let known_uid_validity =
        crate::imap_known_uid_validity(&metadata, &config.source_account, &config.mailbox)
            .map_err(|error| ImapError::Io(error.to_string()))?;
    let scan_state = crate::imap_scan_state(&metadata, &config.source_account, &config.mailbox)
        .map_err(|error| ImapError::Io(error.to_string()))?;
    let frontier_before = scan_state.map(|(_, frontier)| frontier).unwrap_or(0);

    let mut writer = crate::ArchiveWriter::open(&root.join("archive"), 64 * 1024 * 1024)
        .map_err(|error| ImapError::Io(error.to_string()))?;
    let mut stats = ImapSyncStats {
        frontier_before,
        ..Default::default()
    };
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
            if crate::imap_message_exists(
                &metadata,
                &config.source_account,
                &config.mailbox,
                message.uid_validity,
                message.uid,
            )
            .map_err(|error| ImapError::Io(error.to_string()))?
            {
                return Ok(());
            }
            let doc_id = crate::next_doc_id(&metadata)
                .map_err(|error| ImapError::Io(error.to_string()))? as u64;
            let location = writer
                .append_raw(doc_id, &message.raw)
                .map_err(|error| ImapError::Io(error.to_string()))?;
            crate::insert_imap_metadata(
                &metadata,
                &config.source_account,
                &config.mailbox,
                message.uid_validity,
                message.uid,
                &message.flags_json,
                message.internal_date.as_deref(),
                message.internal_date_ms,
                message.rfc822_size,
                doc_id as i64,
                &location,
            )
            .map_err(|error| ImapError::Io(error.to_string()))?;
            stats.new_messages += 1;
            stats.archive_bytes_added += location.frame_bytes;
            Ok(())
        },
    ));
    drop(runtime);
    let fetch_result = fetch_result?;
    stats.uid_validity = fetch_result.uid_validity;
    stats.uid_next = fetch_result.uid_next;
    stats.examined = fetch_result.examined;
    writer
        .sync()
        .map_err(|error| ImapError::Io(error.to_string()))?;
    if let Some(scanned_through_uid) = fetch_result.scanned_through_uid {
        crate::upsert_imap_scan_state(
            &metadata,
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
    drop(writer);
    drop(metadata);

    stats.index = Some(
        crate::index_gmail_archive(root)
            .map_err(|error| ImapError::Io(format!("update search index: {error}")))?,
    );
    Ok(stats)
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
    let stream = timeout(config.timeout, connector.connect(server_name, stream))
        .await
        .map_err(|_| ImapError::Protocol("TLS handshake timeout".into()))?
        .map_err(|error| ImapError::Protocol(format!("TLS handshake failed: {error}")))?;
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
    use std::time::{SystemTime, UNIX_EPOCH};

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
        let location = writer.append_raw(0, raw).unwrap();
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
            &location,
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
}
