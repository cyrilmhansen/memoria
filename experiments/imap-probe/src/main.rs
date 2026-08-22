use async_imap::types::Fetch;
use futures::TryStreamExt;
use rustls::pki_types::{pem::PemObject, CertificateDer, ServerName};
use sha2::{Digest, Sha256};
use std::env;
use std::fmt::Debug;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;
use tokio::io::{AsyncRead, AsyncWrite};
use tokio::net::TcpStream;
use tokio::time::timeout;
use tokio_rustls::TlsConnector;

const DEFAULT_USER: &str = "imap-probe";
const DEFAULT_PASSWORD: &str = "probe-pass";

#[derive(Debug)]
struct Config {
    host: String,
    port: u16,
    user: String,
    password: String,
    output: PathBuf,
    fixtures: Option<PathBuf>,
    timeout: Duration,
    tls: bool,
    ca_cert: Option<PathBuf>,
    server_name: String,
}

fn option(args: &[String], name: &str) -> Option<String> {
    args.windows(2)
        .find(|pair| pair[0] == name)
        .map(|pair| pair[1].clone())
}

fn has_option(args: &[String], name: &str) -> bool {
    args.iter().any(|arg| arg == name)
}

fn config() -> Result<Config, String> {
    let args = env::args().collect::<Vec<_>>();
    Ok(Config {
        host: option(&args, "--host").unwrap_or_else(|| "127.0.0.1".into()),
        port: option(&args, "--port")
            .unwrap_or_else(|| "3143".into())
            .parse()
            .map_err(|_| "invalid --port")?,
        user: option(&args, "--user").unwrap_or_else(|| DEFAULT_USER.into()),
        password: option(&args, "--password").unwrap_or_else(|| DEFAULT_PASSWORD.into()),
        output: option(&args, "--output")
            .map(PathBuf::from)
            .ok_or("--output is required")?,
        fixtures: option(&args, "--append-fixtures").map(PathBuf::from),
        timeout: Duration::from_millis(
            option(&args, "--timeout-ms")
                .unwrap_or_else(|| "10000".into())
                .parse()
                .map_err(|_| "invalid --timeout-ms")?,
        ),
        tls: has_option(&args, "--tls"),
        ca_cert: option(&args, "--ca-cert").map(PathBuf::from),
        server_name: option(&args, "--server-name").unwrap_or_else(|| "localhost".into()),
    })
}

fn sha256(bytes: &[u8]) -> String {
    let mut digest = Sha256::new();
    digest.update(bytes);
    format!("{:x}", digest.finalize())
}

fn fixture_files(root: &Path) -> Result<Vec<PathBuf>, String> {
    let mut files = fs::read_dir(root)
        .map_err(|error| format!("read fixtures: {error}"))?
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| path.extension().and_then(|value| value.to_str()) == Some("eml"))
        .collect::<Vec<_>>();
    files.sort();
    Ok(files)
}

async fn run(config: Config) -> Result<(), String> {
    fs::create_dir_all(&config.output).map_err(|error| format!("create output: {error}"))?;
    let stream = timeout(
        config.timeout,
        TcpStream::connect((config.host.as_str(), config.port)),
    )
    .await
    .map_err(|_| "connection timeout".to_string())?
    .map_err(|error| format!("connection failed: {error}"))?;
    stream
        .set_nodelay(true)
        .map_err(|error| format!("set TCP options: {error}"))?;

    if config.tls {
        let ca_path = config
            .ca_cert
            .as_ref()
            .ok_or("--ca-cert is required with --tls")?;
        let ca = CertificateDer::from_pem_file(ca_path)
            .map_err(|error| format!("read CA certificate: {error}"))?;
        let mut roots = rustls::RootCertStore::empty();
        roots
            .add(ca)
            .map_err(|error| format!("add CA certificate: {error}"))?;
        let provider = rustls::crypto::aws_lc_rs::default_provider();
        let tls_config = rustls::ClientConfig::builder_with_provider(provider.into())
            .with_safe_default_protocol_versions()
            .map_err(|error| format!("TLS configuration failed: {error}"))?
            .with_root_certificates(roots)
            .with_no_client_auth();
        let connector = TlsConnector::from(Arc::new(tls_config));
        let server_name = ServerName::try_from(config.server_name.clone())
            .map_err(|error| format!("TLS server name failed: {error}"))?;
        let tls_stream = timeout(config.timeout, connector.connect(server_name, stream))
            .await
            .map_err(|_| "TLS handshake timeout".to_string())?
            .map_err(|error| format!("TLS handshake failed: {error}"))?;
        return run_session(config, tls_stream).await;
    }
    run_session(config, stream).await
}

async fn run_session<S>(config: Config, stream: S) -> Result<(), String>
where
    S: AsyncRead + AsyncWrite + Unpin + Send + Debug + 'static,
{
    let mut client = async_imap::Client::new(stream);
    timeout(config.timeout, client.read_response())
        .await
        .map_err(|_| "greeting timeout".to_string())?
        .map_err(|error| format!("greeting failed: {error}"))?
        .ok_or("server closed before greeting")?;
    let (mut session, capabilities) = timeout(
        config.timeout,
        client.login_with_capabilities(&config.user, &config.password),
    )
    .await
    .map_err(|_| "login timeout".to_string())?
    .map_err(|(error, _)| format!("login failed: {error}"))?;
    let capability_text = capabilities
        .as_ref()
        .map(|value| {
            value
                .iter()
                .map(|capability| format!("{capability:?}"))
                .collect::<Vec<_>>()
                .join(",")
        })
        .unwrap_or_else(|| "unreported".into());
    println!("login_capabilities={capability_text}");
    let capabilities = timeout(config.timeout, session.capabilities())
        .await
        .map_err(|_| "CAPABILITY timeout".to_string())?
        .map_err(|error| format!("CAPABILITY failed: {error}"))?;
    println!(
        "capabilities={}",
        capabilities
            .iter()
            .map(|capability| format!("{capability:?}"))
            .collect::<Vec<_>>()
            .join(",")
    );

    {
        let mut mailboxes = timeout(config.timeout, session.list(None::<&str>, Some("*")))
            .await
            .map_err(|_| "LIST timeout".to_string())?
            .map_err(|error| format!("LIST failed: {error}"))?;
        while let Some(mailbox) = timeout(config.timeout, mailboxes.try_next())
            .await
            .map_err(|_| "LIST response timeout".to_string())?
            .map_err(|error| format!("LIST response failed: {error}"))?
        {
            println!("mailbox={}", mailbox.name());
        }
    }

    if let Some(fixtures) = &config.fixtures {
        for path in fixture_files(fixtures)? {
            let bytes = fs::read(&path).map_err(|error| format!("read fixture: {error}"))?;
            timeout(config.timeout, session.append("INBOX", None, None, &bytes))
                .await
                .map_err(|_| "APPEND timeout".to_string())?
                .map_err(|error| format!("APPEND failed: {error}"))?;
            println!("appended={}", path.file_name().unwrap().to_string_lossy());
        }
    }

    let mailbox = timeout(config.timeout, session.examine("INBOX"))
        .await
        .map_err(|_| "EXAMINE timeout".to_string())?
        .map_err(|error| format!("EXAMINE failed: {error}"))?;
    println!(
        "examine_exists={} uidvalidity={:?} uidnext={:?}",
        mailbox.exists, mailbox.uid_validity, mailbox.uid_next
    );
    if mailbox.exists == 0 {
        session
            .logout()
            .await
            .map_err(|error| format!("logout: {error}"))?;
        return Ok(());
    }

    let query = "(UID FLAGS INTERNALDATE RFC822.SIZE BODY.PEEK[])";
    let count = {
        let mut fetched = timeout(config.timeout, session.uid_fetch("1:*", query))
            .await
            .map_err(|_| "UID FETCH timeout".to_string())?
            .map_err(|error| format!("UID FETCH failed: {error}"))?;
        let mut count = 0usize;
        while let Some(message) = timeout(config.timeout, fetched.try_next())
            .await
            .map_err(|_| "FETCH response timeout".to_string())?
            .map_err(|error| format!("FETCH response failed: {error}"))?
        {
            save_fetch(&config.output, &message)?;
            count += 1;
        }
        count
    };
    println!("fetched={count}");
    timeout(config.timeout, session.logout())
        .await
        .map_err(|_| "logout timeout".to_string())?
        .map_err(|error| format!("logout failed: {error}"))?;
    Ok(())
}

fn save_fetch(output: &Path, fetch: &Fetch) -> Result<(), String> {
    let body = fetch.body().ok_or("FETCH response had no BODY.PEEK[]")?;
    let uid = fetch.uid.ok_or("FETCH response had no UID")?;
    let path = output.join(format!("{uid}.eml"));
    fs::write(&path, body).map_err(|error| format!("write {}: {error}", path.display()))?;
    let parsed = mailparse::parse_mail(body)
        .map_err(|error| format!("mailparse UID {uid} failed: {error}"))?;
    println!(
        "message={} uid={} size={} announced={:?} sha256={} flags={} seen={} internaldate={:?} parts={}",
        fetch.message,
        uid,
        body.len(),
        fetch.size,
        sha256(body),
        fetch.flags().count(),
        fetch.flags().any(|flag| matches!(flag, async_imap::types::Flag::Seen)),
        fetch.internal_date(),
        parsed.subparts.len()
    );
    Ok(())
}

#[tokio::main]
async fn main() {
    let result = match config() {
        Ok(config) => run(config).await,
        Err(error) => Err(error),
    };
    if let Err(error) = result {
        eprintln!("error={error}");
        std::process::exit(1);
    }
}
