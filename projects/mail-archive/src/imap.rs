use crate::GmailIndexStats;
use async_imap::types::Fetch;
use futures::TryStreamExt;
use rusqlite::OptionalExtension;
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
    pub new_messages: u64,
    pub network_bytes: u64,
    pub archive_bytes_added: u64,
    pub uid_validity: u32,
    pub index: Option<GmailIndexStats>,
}

#[derive(Debug)]
pub enum ImapError {
    Config(String),
    Io(String),
    Protocol(String),
    UidValidityChanged { expected: u32, observed: u32 },
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
    let runtime = Builder::new_multi_thread()
        .enable_io()
        .enable_time()
        .build()
        .map_err(|error| ImapError::Io(format!("create Tokio runtime: {error}")))?;
    let (uid_validity, fetched) = runtime.block_on(fetch_mailbox(config))?;
    drop(runtime);

    let metadata = crate::create_metadata(&root.join("metadata.sqlite"))
        .map_err(|error| ImapError::Io(error.to_string()))?;
    let existing_uid_validity: Option<u32> = metadata
        .query_row(
            "SELECT uid_validity FROM imap_messages WHERE source_account=?1 AND mailbox=?2 LIMIT 1",
            rusqlite::params![config.source_account, config.mailbox],
            |row| row.get::<_, i64>(0),
        )
        .optional()
        .map_err(|error| ImapError::Io(error.to_string()))?
        .map(|value: i64| value as u32);
    if let Some(expected) = existing_uid_validity {
        if expected != uid_validity {
            return Err(ImapError::UidValidityChanged {
                expected,
                observed: uid_validity,
            });
        }
    }

    let mut writer = crate::ArchiveWriter::open(&root.join("archive"), 64 * 1024 * 1024)
        .map_err(|error| ImapError::Io(error.to_string()))?;
    let mut stats = ImapSyncStats {
        examined: fetched.len() as u64,
        uid_validity,
        ..Default::default()
    };
    for message in fetched {
        if crate::imap_message_exists(
            &metadata,
            &config.source_account,
            &config.mailbox,
            uid_validity,
            message.uid,
        )
        .map_err(|error| ImapError::Io(error.to_string()))?
        {
            continue;
        }
        let doc_id =
            crate::next_doc_id(&metadata).map_err(|error| ImapError::Io(error.to_string()))? as u64;
        let location = writer
            .append_raw(doc_id, &message.raw)
            .map_err(|error| ImapError::Io(error.to_string()))?;
        crate::insert_imap_metadata(
            &metadata,
            &config.source_account,
            &config.mailbox,
            uid_validity,
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
        stats.network_bytes += message.raw.len() as u64;
        stats.archive_bytes_added += location.frame_bytes;
    }
    writer
        .sync()
        .map_err(|error| ImapError::Io(error.to_string()))?;
    drop(writer);
    drop(metadata);

    stats.index = Some(
        crate::index_gmail_archive(root)
            .map_err(|error| ImapError::Io(format!("update search index: {error}")))?,
    );
    Ok(stats)
}

async fn fetch_mailbox(config: &ImapConfig) -> Result<(u32, Vec<FetchedMessage>), ImapError> {
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
    fetch_session(config, stream).await
}

async fn fetch_session<S>(
    config: &ImapConfig,
    stream: S,
) -> Result<(u32, Vec<FetchedMessage>), ImapError>
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
    let sequence = config
        .limit
        .map(|limit| format!("1:{limit}"))
        .unwrap_or_else(|| "1:*".into());
    let messages = {
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
        let mut messages = Vec::new();
        while let Some(fetch) = timeout(config.timeout, fetched.try_next())
            .await
            .map_err(|_| ImapError::Protocol("FETCH response timeout".into()))?
            .map_err(|error| ImapError::Protocol(format!("FETCH response failed: {error}")))?
        {
            messages.push(fetch_to_owned(&fetch)?);
        }
        messages
    };
    timeout(config.timeout, session.logout())
        .await
        .map_err(|_| ImapError::Protocol("logout timeout".into()))?
        .map_err(|error| ImapError::Protocol(format!("logout failed: {error}")))?;
    Ok((uid_validity, messages))
}

fn fetch_to_owned(fetch: &Fetch) -> Result<FetchedMessage, ImapError> {
    let uid = fetch
        .uid
        .ok_or_else(|| ImapError::Protocol("FETCH response had no UID".into()))?;
    let raw = fetch
        .body()
        .ok_or_else(|| ImapError::Protocol("FETCH response had no BODY.PEEK[]".into()))?
        .to_vec();
    let internal_date = fetch.internal_date();
    Ok(FetchedMessage {
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
}
