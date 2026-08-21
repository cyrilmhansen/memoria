use crate::{read_html_document, HtmlResource};
use ammonia::Builder;
use getrandom::fill as random_fill;
use std::collections::HashMap;
use std::io::{self, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;
use std::time::Instant;

const SESSION_TTL: Duration = Duration::from_secs(10 * 60);
const MAX_SESSIONS: usize = 8;

const CSP: &str = "default-src 'none'; img-src 'self'; style-src 'unsafe-inline'; script-src 'none'; connect-src 'none'; object-src 'none'; frame-src 'none'; form-action 'none'; base-uri 'none'";

struct HtmlSession {
    html: String,
    resources: HashMap<String, HtmlResource>,
    created: Instant,
}

struct ServerInner {
    address: std::net::SocketAddr,
    sessions: Arc<Mutex<HashMap<String, HtmlSession>>>,
    stopped: Arc<AtomicBool>,
    thread: Mutex<Option<thread::JoinHandle<()>>>,
}

impl Drop for ServerInner {
    fn drop(&mut self) {
        self.stopped.store(true, Ordering::Release);
        let _ = TcpStream::connect(self.address);
        if let Some(thread) = self.thread.get_mut().ok().and_then(Option::take) {
            let _ = thread.join();
        }
    }
}

#[derive(Clone)]
pub struct HtmlPreviewServer {
    inner: Arc<ServerInner>,
}

impl HtmlPreviewServer {
    pub fn start() -> io::Result<Self> {
        let listener = TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0))?;
        listener.set_nonblocking(true)?;
        let address = listener.local_addr()?;
        let sessions = Arc::new(Mutex::new(HashMap::new()));
        let stopped = Arc::new(AtomicBool::new(false));
        let thread_sessions = sessions.clone();
        let thread_stopped = stopped.clone();
        let thread = thread::spawn(move || {
            while !thread_stopped.load(Ordering::Acquire) {
                match listener.accept() {
                    Ok((stream, _)) => handle_connection(stream, &thread_sessions),
                    Err(error) if error.kind() == io::ErrorKind::WouldBlock => {
                        thread::sleep(Duration::from_millis(10));
                    }
                    Err(_) => break,
                }
            }
        });
        Ok(Self {
            inner: Arc::new(ServerInner {
                address,
                sessions,
                stopped,
                thread: Mutex::new(Some(thread)),
            }),
        })
    }

    pub fn open_message(&self, archive: &Path, doc_id: u64) -> io::Result<Option<String>> {
        let Some(document) = read_html_document(archive, doc_id)? else {
            return Ok(None);
        };
        let session = random_token()?;
        let mut resources = HashMap::new();
        let mut replacements = Vec::new();
        for resource in document.resources {
            let resource_token = random_token()?;
            replacements.push((resource.content_id.clone(), resource_token.clone()));
            resources.insert(resource_token, resource);
        }
        let html = sanitize_html(&document.html, &session, &replacements);
        let mut sessions = self
            .inner
            .sessions
            .lock()
            .map_err(|_| io::Error::other("HTML session lock poisoned"))?;
        prune_sessions(&mut sessions);
        while sessions.len() >= MAX_SESSIONS {
            let Some(oldest) = sessions
                .iter()
                .min_by_key(|(_, value)| value.created)
                .map(|(key, _)| key.clone())
            else {
                break;
            };
            sessions.remove(&oldest);
        }
        sessions.insert(
            session.clone(),
            HtmlSession {
                html,
                resources,
                created: Instant::now(),
            },
        );
        Ok(Some(format!(
            "http://127.0.0.1:{}/{}/html",
            self.inner.address.port(),
            session
        )))
    }
}

fn prune_sessions(sessions: &mut HashMap<String, HtmlSession>) {
    let now = Instant::now();
    sessions.retain(|_, session| now.duration_since(session.created) < SESSION_TTL);
}

fn random_token() -> io::Result<String> {
    let mut bytes = [0_u8; 24];
    random_fill(&mut bytes).map_err(|error| io::Error::other(error.to_string()))?;
    Ok(bytes.iter().map(|byte| format!("{byte:02x}")).collect())
}

fn replace_ascii_case_insensitive(mut html: String, needle: &str, replacement: &str) -> String {
    let lower_needle = needle.to_ascii_lowercase();
    loop {
        let lower = html.to_ascii_lowercase();
        let Some(position) = lower.find(&lower_needle) else {
            return html;
        };
        html.replace_range(position..position + needle.len(), replacement);
    }
}

fn sanitize_html(html: &str, session: &str, replacements: &[(String, String)]) -> String {
    let mut rewritten = html.to_string();
    for (content_id, token) in replacements {
        let route = format!("/{session}/cid/{token}");
        rewritten = replace_ascii_case_insensitive(rewritten, &format!("cid:{content_id}"), &route);
        rewritten =
            replace_ascii_case_insensitive(rewritten, &format!("cid:&lt;{content_id}&gt;"), &route);
    }
    let builder = Builder::default();
    let clean = builder.clean(&rewritten).to_string();
    format!(
        "<!doctype html><html><head><meta charset=\"utf-8\"><meta http-equiv=\"Content-Security-Policy\" content=\"{CSP}\"></head><body>{clean}</body></html>"
    )
}

fn handle_connection(mut stream: TcpStream, sessions: &Arc<Mutex<HashMap<String, HtmlSession>>>) {
    let _ = stream.set_read_timeout(Some(Duration::from_secs(2)));
    let mut request = Vec::with_capacity(2048);
    let mut buffer = [0_u8; 1024];
    while request.len() < 16 * 1024 {
        match stream.read(&mut buffer) {
            Ok(0) => break,
            Ok(read) => {
                request.extend_from_slice(&buffer[..read]);
                if request.windows(4).any(|window| window == b"\r\n\r\n") {
                    break;
                }
            }
            Err(_) => return,
        }
    }
    let request_line = request
        .split(|byte| *byte == b'\n')
        .next()
        .and_then(|line| std::str::from_utf8(line).ok())
        .unwrap_or_default()
        .trim_end_matches('\r');
    let mut fields = request_line.split_whitespace();
    if fields.next() != Some("GET") {
        respond(
            &mut stream,
            "405 Method Not Allowed",
            "text/plain",
            b"method not allowed",
        );
        return;
    }
    let path = fields.next().unwrap_or_default();
    let segments = path
        .split('/')
        .filter(|segment| !segment.is_empty())
        .collect::<Vec<_>>();
    if segments.len() < 2
        || segments.len() > 3
        || segments.iter().any(|segment| segment.contains(".."))
    {
        respond(&mut stream, "404 Not Found", "text/plain", b"not found");
        return;
    }
    let mut sessions = match sessions.lock() {
        Ok(sessions) => sessions,
        Err(_) => {
            respond(
                &mut stream,
                "500 Internal Server Error",
                "text/plain",
                b"server error",
            );
            return;
        }
    };
    prune_sessions(&mut sessions);
    let Some(session) = sessions.get(segments[0]) else {
        respond(&mut stream, "404 Not Found", "text/plain", b"not found");
        return;
    };
    if segments[1] == "html" && segments.len() == 2 {
        respond(
            &mut stream,
            "200 OK",
            "text/html; charset=utf-8",
            session.html.as_bytes(),
        );
    } else if segments[1] == "cid" && segments.len() == 3 {
        let Some(resource) = session.resources.get(segments[2]) else {
            respond(&mut stream, "404 Not Found", "text/plain", b"not found");
            return;
        };
        respond(
            &mut stream,
            "200 OK",
            safe_mime(&resource.mime),
            &resource.bytes,
        );
    } else {
        respond(&mut stream, "404 Not Found", "text/plain", b"not found");
    }
}

fn safe_mime(mime: &str) -> &str {
    if mime
        .bytes()
        .all(|byte| byte.is_ascii_alphanumeric() || b"!#$&^_.+-/*".contains(&byte))
    {
        mime
    } else {
        "application/octet-stream"
    }
}

fn respond(stream: &mut TcpStream, status: &str, content_type: &str, body: &[u8]) {
    let header = format!(
        "HTTP/1.1 {status}\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\nCache-Control: no-store\r\nX-Content-Type-Options: nosniff\r\nContent-Security-Policy: {CSP}\r\nConnection: close\r\n\r\n",
        body.len()
    );
    let _ = stream.write_all(header.as_bytes());
    let _ = stream.write_all(body);
}

#[cfg(test)]
mod tests {
    use super::{
        prune_sessions, sanitize_html, HtmlPreviewServer, HtmlSession, MAX_SESSIONS, SESSION_TTL,
    };
    use crate::{create_metadata, insert_metadata, ArchiveWriter, Message};
    use std::collections::HashMap;
    use std::fs;
    use std::io::{Read, Write};
    use std::net::TcpStream;
    use std::path::PathBuf;
    use std::time::{Duration, Instant};

    #[test]
    fn html_sessions_are_bounded_and_expire() {
        let mut sessions = HashMap::new();
        sessions.insert(
            "expired".into(),
            HtmlSession {
                html: String::new(),
                resources: HashMap::new(),
                created: Instant::now() - SESSION_TTL - Duration::from_secs(1),
            },
        );
        for index in 0..(MAX_SESSIONS + 2) {
            sessions.insert(
                index.to_string(),
                HtmlSession {
                    html: String::new(),
                    resources: HashMap::new(),
                    created: Instant::now(),
                },
            );
        }
        prune_sessions(&mut sessions);
        while sessions.len() > MAX_SESSIONS {
            let key = sessions.keys().next().cloned().unwrap();
            sessions.remove(&key);
        }
        assert!(!sessions.contains_key("expired"));
        assert!(sessions.len() <= MAX_SESSIONS);
    }

    #[test]
    fn sanitizes_active_content_and_rewrites_cid() {
        let html = sanitize_html(
            r#"<div onclick="bad()"><script>bad()</script><form action="https://evil.test"><img src="cid:logo@test"><img src="https://evil.test/pixel"></form><a href="https://example.test">link</a></div>"#,
            "session",
            &[("logo@test".into(), "resource".into())],
        );
        assert!(!html.contains("<script"));
        assert!(!html.contains("bad()"));
        assert!(!html.contains("onclick"));
        assert!(!html.contains("<form"));
        assert!(html.contains("/session/cid/resource"));
        assert!(html.contains("https://example.test"));
        assert!(html.contains("connect-src 'none'"));
        assert!(html.contains("img-src 'self'"));
    }

    #[test]
    fn serves_sanitized_html_and_cid_without_exposing_other_messages() {
        let root = PathBuf::from(format!(
            "/var/tmp/atlas-html-preview-test-{}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).unwrap();
        fs::create_dir_all(root.join("archive")).unwrap();
        create_metadata(&root.join("metadata.sqlite")).unwrap();
        let raw = b"From: sender@example.test\r\nTo: recipient@example.test\r\nSubject: HTML\r\nContent-Type: multipart/related; boundary=mail\r\n\r\n--mail\r\nContent-Type: text/html\r\n\r\n<p>Hello</p><img src=\"cid:logo@example.test\"><script>bad()</script>\r\n--mail\r\nContent-Type: image/png\r\nContent-ID: <logo@example.test>\r\nContent-Transfer-Encoding: base64\r\n\r\naGVsbG8=\r\n--mail--\r\n";
        let message = Message {
            id: 9,
            message_id: "html-fixture".into(),
            timestamp: 0,
            sender: "sender@example.test".into(),
            recipients: vec!["recipient@example.test".into()],
            subject: "HTML".into(),
            text_body: "Hello".into(),
            html_body: Some("<p>Hello</p>".into()),
            account: "fixture".into(),
            folder: "Inbox".into(),
            thread: "thread".into(),
            attachments: Vec::new(),
            raw: raw.to_vec(),
        };
        let mut writer = ArchiveWriter::open(&root.join("archive"), 4096).unwrap();
        let location = writer.append(&message).unwrap();
        writer.sync().unwrap();
        let connection = create_metadata(&root.join("metadata.sqlite")).unwrap();
        insert_metadata(&connection, &message, &location).unwrap();

        let server = HtmlPreviewServer::start().unwrap();
        let url = server.open_message(&root, 9).unwrap().unwrap();
        let parsed = url::Url::parse(&url).unwrap();
        let path = parsed.path();
        let mut stream = TcpStream::connect(("127.0.0.1", parsed.port().unwrap())).unwrap();
        write!(
            stream,
            "GET {path} HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n"
        )
        .unwrap();
        let mut response = Vec::new();
        stream.read_to_end(&mut response).unwrap();
        let response = String::from_utf8(response).unwrap();
        assert!(response.starts_with("HTTP/1.1 200 OK"));
        assert!(response.contains("<p>Hello</p>"));
        assert!(!response.contains("<script"));
        assert!(response.contains("/cid/"));
        assert!(!response.contains("other-message"));

        let cid_path = response
            .split("src=\"")
            .nth(1)
            .unwrap()
            .split('"')
            .next()
            .unwrap();
        let mut resource = TcpStream::connect(("127.0.0.1", parsed.port().unwrap())).unwrap();
        write!(
            resource,
            "GET {cid_path} HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n"
        )
        .unwrap();
        let mut resource_response = Vec::new();
        resource.read_to_end(&mut resource_response).unwrap();
        assert!(resource_response.ends_with(b"hello"));
        fs::remove_dir_all(root).unwrap();
    }
}
