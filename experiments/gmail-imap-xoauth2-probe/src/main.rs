use async_imap::Authenticator;
#[cfg(test)]
use base64::{engine::general_purpose::STANDARD, Engine as _};
use futures::TryStreamExt;
use rustls::pki_types::ServerName;
use sha2::{Digest, Sha256};
use std::env;
use std::fmt;
use std::io::Read;
use std::sync::Arc;
use std::time::Duration;
#[cfg(test)]
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
#[cfg(test)]
use tokio::net::TcpListener;
use tokio::net::TcpStream;
use tokio::time::timeout;
use tokio_rustls::TlsConnector;

const TOKEN_ENV: &str = "MEMORIA_GMAIL_IMAP_XOAUTH2_TOKEN";

#[derive(Debug)]
struct Config {
    user: String,
    token: Secret,
    timeout: Duration,
    fetch: bool,
}

struct Secret(String);

impl fmt::Debug for Secret {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("[redacted]")
    }
}

impl Secret {
    fn expose(&self) -> &str {
        &self.0
    }
}

struct XOAuth2 {
    user: String,
    token: Secret,
}

impl fmt::Debug for XOAuth2 {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("XOAuth2")
            .field("user", &self.user)
            .field("token", &"[redacted]")
            .finish()
    }
}

impl Authenticator for XOAuth2 {
    type Response = Vec<u8>;

    fn process(&mut self, _challenge: &[u8]) -> Self::Response {
        format!(
            "user={}\x01auth=Bearer {}\x01\x01",
            self.user,
            self.token.expose()
        )
        .into_bytes()
    }
}

fn option(args: &[String], name: &str) -> Option<String> {
    args.windows(2)
        .find(|pair| pair[0] == name)
        .map(|pair| pair[1].clone())
}

fn has_option(args: &[String], name: &str) -> bool {
    args.iter().any(|arg| arg == name)
}

fn load_token(args: &[String]) -> Result<Secret, String> {
    let stdin_requested = has_option(args, "--token-stdin");
    let env_token = env::var(TOKEN_ENV).ok();
    let file_path = option(args, "--token-file");

    // Keep the documented preference order when more than one source is
    // present. The explicit stdin switch wins over the environment and file.
    if stdin_requested {
        let mut token = String::new();
        std::io::stdin()
            .read_to_string(&mut token)
            .map_err(|_| "cannot read token from stdin".to_string())?;
        return nonempty_token(token);
    }
    if let Some(token) = env_token {
        return nonempty_token(token);
    }
    if let Some(path) = file_path {
        let token =
            std::fs::read_to_string(path).map_err(|_| "cannot read token file".to_string())?;
        return nonempty_token(token);
    }
    Err(
        "no token source; use --token-stdin, MEMORIA_GMAIL_IMAP_XOAUTH2_TOKEN, or --token-file"
            .into(),
    )
}

fn nonempty_token(token: String) -> Result<Secret, String> {
    let token = token.trim().to_owned();
    if token.is_empty() {
        Err("token source is empty".into())
    } else {
        Ok(Secret(token))
    }
}

fn config() -> Result<Config, String> {
    let args = env::args().collect::<Vec<_>>();
    let timeout_ms = option(&args, "--timeout-ms")
        .unwrap_or_else(|| "10000".into())
        .parse::<u64>()
        .map_err(|_| "invalid --timeout-ms")?;
    Ok(Config {
        user: option(&args, "--user").ok_or("--user is required")?,
        token: load_token(&args)?,
        timeout: Duration::from_millis(timeout_ms),
        fetch: has_option(&args, "--fetch"),
    })
}

fn redact(text: &str, token: &str) -> String {
    if token.is_empty() {
        "[redacted error]".into()
    } else {
        text.replace(token, "[redacted]")
    }
}

async fn run(config: Config) -> Result<(), String> {
    let token = config.token.expose().to_owned();
    let stream = timeout(config.timeout, TcpStream::connect(("imap.gmail.com", 993)))
        .await
        .map_err(|_| "connection timeout".to_string())?
        .map_err(|error| format!("connection failed: {}", redact(&error.to_string(), &token)))?;
    stream
        .set_nodelay(true)
        .map_err(|error| format!("TCP setup failed: {}", redact(&error.to_string(), &token)))?;

    let provider = rustls::crypto::aws_lc_rs::default_provider();
    let mut roots = rustls::RootCertStore::empty();
    roots.extend(webpki_roots::TLS_SERVER_ROOTS.iter().cloned());
    let tls_config = rustls::ClientConfig::builder_with_provider(provider.into())
        .with_safe_default_protocol_versions()
        .map_err(|_| "TLS configuration failed".to_string())?
        .with_root_certificates(roots)
        .with_no_client_auth();
    let connector = TlsConnector::from(Arc::new(tls_config));
    let server_name = ServerName::try_from("imap.gmail.com".to_owned())
        .map_err(|_| "TLS server name failed".to_string())?;
    let tls_stream = timeout(config.timeout, connector.connect(server_name, stream))
        .await
        .map_err(|_| "TLS handshake timeout".to_string())?
        .map_err(|error| {
            format!(
                "TLS handshake failed: {}",
                redact(&error.to_string(), &token)
            )
        })?;

    let mut client = async_imap::Client::new(tls_stream);
    timeout(config.timeout, client.read_response())
        .await
        .map_err(|_| "greeting timeout".to_string())?
        .map_err(|_| "greeting failed".to_string())?
        .ok_or("server closed before greeting")?;

    let auth = XOAuth2 {
        user: config.user.clone(),
        token: config.token,
    };
    let mut session = match timeout(config.timeout, client.authenticate("XOAUTH2", auth)).await {
        Ok(Ok(session)) => session,
        Ok(Err((_error, _client))) => return Err("XOAUTH2 authentication failed".into()),
        Err(_) => return Err("XOAUTH2 authentication timeout".into()),
    };
    println!("auth=xoauth2 status=success");

    let capabilities = timeout(config.timeout, session.capabilities())
        .await
        .map_err(|_| "CAPABILITY timeout".to_string())?
        .map_err(|error| format!("CAPABILITY failed: {}", redact(&error.to_string(), &token)))?;
    println!(
        "capabilities={}",
        capabilities
            .iter()
            .map(|capability| format!("{capability:?}"))
            .collect::<Vec<_>>()
            .join(",")
    );

    let mailbox_count = {
        let mut mailboxes = timeout(config.timeout, session.list(None::<&str>, Some("*")))
            .await
            .map_err(|_| "LIST timeout".to_string())?
            .map_err(|error| format!("LIST failed: {}", redact(&error.to_string(), &token)))?;
        let mut mailbox_count = 0;
        while let Some(_mailbox) = timeout(config.timeout, mailboxes.try_next())
            .await
            .map_err(|_| "LIST response timeout".to_string())?
            .map_err(|error| {
                format!(
                    "LIST response failed: {}",
                    redact(&error.to_string(), &token)
                )
            })?
        {
            mailbox_count += 1;
        }
        mailbox_count
    };
    println!("mailbox_count={mailbox_count}");

    if config.fetch {
        let mailbox = timeout(config.timeout, session.examine("INBOX"))
            .await
            .map_err(|_| "EXAMINE timeout".to_string())?
            .map_err(|error| format!("EXAMINE failed: {}", redact(&error.to_string(), &token)))?;
        println!(
            "examine=INBOX exists={} uidvalidity={:?} uidnext={:?}",
            mailbox.exists, mailbox.uid_validity, mailbox.uid_next
        );
        let query =
            "UID FLAGS INTERNALDATE RFC822.SIZE X-GM-MSGID X-GM-THRID X-GM-LABELS BODY.PEEK[]";
        let mut fetched = timeout(config.timeout, session.uid_fetch("1:*", query))
            .await
            .map_err(|_| "UID FETCH timeout".to_string())?
            .map_err(|error| format!("UID FETCH failed: {}", redact(&error.to_string(), &token)))?;
        let mut count = 0usize;
        let mut bytes = 0usize;
        let mut digest = Sha256::new();
        while let Some(fetch) = timeout(config.timeout, fetched.try_next())
            .await
            .map_err(|_| "FETCH response timeout".to_string())?
            .map_err(|error| {
                format!(
                    "FETCH response failed: {}",
                    redact(&error.to_string(), &token)
                )
            })?
        {
            if let Some(body) = fetch.body() {
                bytes += body.len();
                digest.update(body);
            }
            count += 1;
        }
        println!(
            "fetched={count} raw_bytes={bytes} aggregate_sha256={:x}",
            digest.finalize()
        );
    }

    timeout(config.timeout, session.logout())
        .await
        .map_err(|_| "logout timeout".to_string())?
        .map_err(|_| "logout failed".to_string())?;
    println!("logout=success");
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

#[cfg(test)]
mod tests {
    use super::*;

    const TEST_USER: &str = "test@example.invalid";
    const TEST_TOKEN: &str = "TEST_TOKEN";

    async fn fake_imap_server(listener: TcpListener, fail_auth: bool) -> Result<(), String> {
        let (socket, _) = listener
            .accept()
            .await
            .map_err(|_| "fake server accept failed".to_string())?;
        let (read_half, mut write_half) = socket.into_split();
        let mut reader = BufReader::new(read_half);
        write_half
            .write_all(b"* OK fake IMAP ready\r\n")
            .await
            .map_err(|_| "fake server greeting failed".to_string())?;

        let mut line = String::new();
        reader
            .read_line(&mut line)
            .await
            .map_err(|_| "fake server AUTHENTICATE read failed".to_string())?;
        let tag = line
            .split_whitespace()
            .next()
            .ok_or_else(|| "fake server missing command tag".to_string())?
            .to_owned();
        if !line.to_ascii_uppercase().contains(" AUTHENTICATE XOAUTH2") {
            return Err("fake server did not observe AUTHENTICATE XOAUTH2".into());
        }
        write_half
            .write_all(b"+ \r\n")
            .await
            .map_err(|_| "fake server challenge failed".to_string())?;
        line.clear();
        reader
            .read_line(&mut line)
            .await
            .map_err(|_| "fake server SASL response read failed".to_string())?;
        let payload = STANDARD
            .decode(line.trim())
            .map_err(|_| "fake server SASL response was not base64".to_string())?;
        let expected = format!("user={TEST_USER}\x01auth=Bearer {TEST_TOKEN}\x01\x01");
        if payload != expected.as_bytes() {
            return Err("fake server SASL payload mismatch".into());
        }

        if fail_auth {
            write_half
                .write_all(format!("{tag} NO [AUTHENTICATIONFAILED] rejected\r\n").as_bytes())
                .await
                .map_err(|_| "fake server auth failure failed".to_string())?;
            return Ok(());
        }

        write_half
            .write_all(format!("{tag} OK XOAUTH2 authenticated\r\n").as_bytes())
            .await
            .map_err(|_| "fake server auth success failed".to_string())?;

        loop {
            line.clear();
            let read = reader
                .read_line(&mut line)
                .await
                .map_err(|_| "fake server command read failed".to_string())?;
            if read == 0 {
                return Err("fake server client disconnected before LOGOUT".into());
            }
            let command = line.to_ascii_uppercase();
            let command_tag = line
                .split_whitespace()
                .next()
                .ok_or_else(|| "fake server missing post-auth tag".to_string())?;
            if command.contains("CAPABILITY") {
                write_half
                    .write_all(
                        format!(
                            "* CAPABILITY IMAP4REV1 AUTH=XOAUTH2 X-GM-EXT-1\r\n{command_tag} OK CAPABILITY completed\r\n"
                        )
                        .as_bytes(),
                    )
                    .await
                    .map_err(|_| "fake server CAPABILITY failed".to_string())?;
            } else if command.contains("LIST") {
                write_half
                    .write_all(
                        format!(
                            "* LIST (\\HasNoChildren) \".\" \"INBOX\"\r\n{command_tag} OK LIST completed\r\n"
                        )
                        .as_bytes(),
                    )
                    .await
                    .map_err(|_| "fake server LIST failed".to_string())?;
            } else if command.contains("LOGOUT") {
                write_half
                    .write_all(
                        format!("* BYE logging out\r\n{command_tag} OK LOGOUT completed\r\n")
                            .as_bytes(),
                    )
                    .await
                    .map_err(|_| "fake server LOGOUT failed".to_string())?;
                return Ok(());
            } else {
                return Err("fake server observed an unexpected command".into());
            }
        }
    }

    async fn run_local_wire_auth(
        addr: std::net::SocketAddr,
        fail_auth: bool,
    ) -> Result<(), String> {
        let stream = TcpStream::connect(addr)
            .await
            .map_err(|_| "local client connect failed".to_string())?;
        let mut client = async_imap::Client::new(stream);
        client
            .read_response()
            .await
            .map_err(|_| "local client greeting failed".to_string())?
            .ok_or_else(|| "local client received no greeting".to_string())?;
        let auth = XOAuth2 {
            user: TEST_USER.into(),
            token: Secret(TEST_TOKEN.into()),
        };
        let mut session = match client.authenticate("XOAUTH2", auth).await {
            Ok(session) => session,
            Err((_error, _client)) => return Err("XOAUTH2 authentication failed".into()),
        };
        if fail_auth {
            return Err("fake server unexpectedly accepted authentication".into());
        }
        session
            .capabilities()
            .await
            .map_err(|_| "local client CAPABILITY failed".to_string())?;
        {
            let mut mailboxes = session
                .list(None::<&str>, Some("*"))
                .await
                .map_err(|_| "local client LIST failed".to_string())?;
            while mailboxes
                .try_next()
                .await
                .map_err(|_| "local client LIST stream failed".to_string())?
                .is_some()
            {}
        }
        session
            .logout()
            .await
            .map_err(|_| "local client LOGOUT failed".to_string())?;
        Ok(())
    }

    #[test]
    fn xoauth2_response_has_expected_sasl_shape() {
        let mut auth = XOAuth2 {
            user: "user@example.test".into(),
            token: Secret("fixture-token".into()),
        };
        assert_eq!(
            auth.process(b"").as_slice(),
            b"user=user@example.test\x01auth=Bearer fixture-token\x01\x01"
        );
    }

    #[test]
    fn debug_redacts_token() {
        let auth = XOAuth2 {
            user: "user@example.test".into(),
            token: Secret("fixture-token".into()),
        };
        let debug = format!("{auth:?}");
        assert!(!debug.contains("fixture-token"));
        assert!(debug.contains("[redacted]"));
    }

    #[test]
    fn errors_redact_token() {
        let error = redact("server rejected fixture-token", "fixture-token");
        assert_eq!(error, "server rejected [redacted]");
        assert!(!error.contains("fixture-token"));
    }

    #[tokio::test]
    async fn xoauth2_is_verified_on_the_wire_then_readonly_commands_work() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(fake_imap_server(listener, false));
        let client =
            tokio::time::timeout(Duration::from_secs(2), run_local_wire_auth(address, false))
                .await
                .unwrap()
                .unwrap();
        assert_eq!(client, ());
        server.await.unwrap().unwrap();
    }

    #[tokio::test]
    async fn xoauth2_failure_is_generic_and_does_not_expose_token() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(fake_imap_server(listener, true));
        let error =
            tokio::time::timeout(Duration::from_secs(2), run_local_wire_auth(address, true))
                .await
                .unwrap()
                .unwrap_err();
        assert!(!error.contains(TEST_TOKEN));
        assert_eq!(error, "XOAUTH2 authentication failed");
        server.await.unwrap().unwrap();
    }

    #[test]
    fn token_is_not_available_without_a_source() {
        let args = vec!["probe".into()];
        assert!(load_token(&args).is_err());
    }
}
