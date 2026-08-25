use std::io::{self, Read, Write};
use std::path::Path;
use std::process::{Command, Stdio};
#[cfg(windows)]
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{mpsc, OnceLock};
use std::thread;
use std::time::{Duration, Instant};

use mailparse::ParsedMail;

use crate::AttachmentPayload;

pub const MAX_INPUT_BYTES: usize = 64 * 1024 * 1024;
pub const MAX_OUTPUT_BYTES: usize = 8 * 1024 * 1024;
pub const EXTRACTION_TIMEOUT: Duration = Duration::from_secs(10);
#[cfg(windows)]
static IFILTER_TEMP_COUNTER: AtomicU64 = AtomicU64::new(0);

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct ProviderId(String);

impl ProviderId {
    pub fn new(value: impl Into<String>) -> Self {
        Self(value.into())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BackendKind {
    BuiltIn,
    ExternalExecutable,
    WindowsIFilter,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ProviderAvailability {
    Available,
    Unavailable,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ExtractionProvider {
    pub id: ProviderId,
    /// Diagnostic fallback; UI text must be translated from `id`.
    pub display_name: String,
    pub backend_kind: BackendKind,
    pub supported_types: Vec<String>,
    pub availability: ProviderAvailability,
    pub version: Option<String>,
    pub executable_path: Option<std::path::PathBuf>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ProviderSelection {
    Automatic,
    Explicit(ProviderId),
}

pub fn discover_providers() -> Vec<ExtractionProvider> {
    static DISCOVERED: OnceLock<Vec<ExtractionProvider>> = OnceLock::new();
    DISCOVERED
        .get_or_init(|| {
            let providers = vec![
                ExtractionProvider {
                    id: ProviderId::new("memoria-text"),
                    display_name: "Memoria built-in text decoder".into(),
                    backend_kind: BackendKind::BuiltIn,
                    supported_types: vec!["text/*".into()],
                    availability: ProviderAvailability::Available,
                    version: Some("v1".into()),
                    executable_path: None,
                },
                discover_pdftotext(),
            ];
            #[cfg(windows)]
            {
                let mut providers = providers;
                providers.insert(1, discover_windows_ifilter());
                providers
            }
            #[cfg(not(windows))]
            {
                providers
            }
        })
        .clone()
}

#[cfg(windows)]
fn discover_windows_ifilter() -> ExtractionProvider {
    let executable_path = resolve_ifilter_helper();
    let supported_types = executable_path
        .as_deref()
        .map(|path| {
            [
                ("application/pdf", ".pdf"),
                (
                    "application/vnd.openxmlformats-officedocument.wordprocessingml.document",
                    ".docx",
                ),
            ]
            .into_iter()
            .filter_map(|(mime, extension)| {
                ifilter_helper_supports_extension(path, extension).then_some(mime.into())
            })
            .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    let available = !supported_types.is_empty();
    ExtractionProvider {
        id: ProviderId::new("windows-ifilter"),
        display_name: "Windows registered IFilter".into(),
        backend_kind: BackendKind::WindowsIFilter,
        supported_types,
        availability: if available {
            ProviderAvailability::Available
        } else {
            ProviderAvailability::Unavailable
        },
        version: None,
        executable_path: available.then_some(executable_path).flatten(),
    }
}

#[cfg(windows)]
fn resolve_ifilter_helper() -> Option<std::path::PathBuf> {
    if let Some(path) = std::env::var_os("MEMORIA_IFILTER_HELPER") {
        let path = std::path::PathBuf::from(path);
        if path.is_file() {
            return Some(path);
        }
    }
    let current = std::env::current_exe().ok()?;
    let sibling = current.parent()?.join("memoria-ifilter-helper.exe");
    sibling.is_file().then_some(sibling)
}

#[cfg(windows)]
fn ifilter_helper_supports_extension(program: &Path, extension: &str) -> bool {
    let mut child = match Command::new(program)
        .args(["discover", extension])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
    {
        Ok(child) => child,
        Err(_) => return false,
    };
    let deadline = Instant::now() + Duration::from_secs(2);
    loop {
        match child.try_wait() {
            Ok(Some(status)) => return status.success(),
            Ok(None) if Instant::now() >= deadline => {
                let _ = child.kill();
                let _ = child.wait();
                return false;
            }
            Ok(None) => thread::sleep(Duration::from_millis(5)),
            Err(_) => {
                let _ = child.kill();
                let _ = child.wait();
                return false;
            }
        }
    }
}

pub fn providers_for_mime(mime: &str) -> Vec<ExtractionProvider> {
    let normalized = mime.trim().to_ascii_lowercase();
    discover_providers()
        .into_iter()
        .filter(|provider| {
            provider.availability == ProviderAvailability::Available
                && provider.supported_types.iter().any(|supported| {
                    supported == &normalized
                        || (supported == "text/*" && normalized.starts_with("text/"))
                })
        })
        .collect()
}

pub fn selected_provider(mime: &str, selection: &ProviderSelection) -> Option<ExtractionProvider> {
    let candidates = providers_for_mime(mime);
    match selection {
        ProviderSelection::Automatic => candidates.into_iter().next(),
        ProviderSelection::Explicit(id) => {
            candidates.into_iter().find(|provider| provider.id == *id)
        }
    }
}

fn discover_pdftotext() -> ExtractionProvider {
    discover_pdftotext_at(resolve_executable("pdftotext"))
}

fn discover_pdftotext_at(executable_path: Option<std::path::PathBuf>) -> ExtractionProvider {
    let availability = if executable_path.is_some() {
        ProviderAvailability::Available
    } else {
        ProviderAvailability::Unavailable
    };
    ExtractionProvider {
        id: ProviderId::new("poppler-pdftotext"),
        display_name: "Poppler pdftotext".into(),
        backend_kind: BackendKind::ExternalExecutable,
        supported_types: vec!["application/pdf".into()],
        availability,
        version: None,
        executable_path,
    }
}

fn resolve_executable(name: &str) -> Option<std::path::PathBuf> {
    let path = std::env::var_os("PATH")?;
    for directory in std::env::split_paths(&path) {
        let candidate = directory.join(name);
        if candidate.is_file() {
            return Some(candidate);
        }
        #[cfg(windows)]
        for extension in [".exe", ".cmd", ".bat"] {
            let candidate = directory.join(format!("{name}{extension}"));
            if candidate.is_file() {
                return Some(candidate);
            }
        }
    }
    None
}

#[derive(Clone, Debug, Default)]
pub struct AttachmentTextStats {
    pub encountered: u64,
    pub supported: u64,
    pub extracted: u64,
    pub unsupported: u64,
    pub failures: u64,
    pub decoded_bytes: u64,
    pub extracted_bytes: u64,
    pub extracted_chars: u64,
}

#[derive(Debug)]
enum ExtractionResult {
    Text(String),
    Unsupported,
    Failed,
}

pub fn extract_attachment_texts(
    parsed: &ParsedMail<'_>,
) -> Result<(String, AttachmentTextStats), String> {
    let payloads = crate::attachment_payloads(parsed).map_err(|error| error.to_string())?;
    let mut stats = AttachmentTextStats::default();
    let mut text = String::new();
    for payload in payloads {
        stats.encountered += 1;
        stats.decoded_bytes += payload.info.decoded_bytes;
        match extract_one(&payload) {
            ExtractionResult::Text(value) => {
                stats.supported += 1;
                if !value.is_empty() {
                    stats.extracted += 1;
                    stats.extracted_bytes += value.len() as u64;
                    stats.extracted_chars += value.chars().count() as u64;
                    if !text.is_empty() {
                        text.push('\n');
                    }
                    text.push_str(&value);
                }
            }
            ExtractionResult::Unsupported => stats.unsupported += 1,
            ExtractionResult::Failed => stats.failures += 1,
        }
    }
    Ok((text, stats))
}

fn extract_one(payload: &AttachmentPayload) -> ExtractionResult {
    if payload.bytes.len() > MAX_INPUT_BYTES {
        return ExtractionResult::Failed;
    }
    let provider = selected_provider(&payload.info.mime, &ProviderSelection::Automatic);
    match provider.as_ref().map(|provider| provider.id.as_str()) {
        #[cfg(windows)]
        Some("windows-ifilter") => {
            let extension = match payload.info.mime.as_str() {
                "application/pdf" => ".pdf",
                "application/vnd.openxmlformats-officedocument.wordprocessingml.document" => {
                    ".docx"
                }
                _ => return ExtractionResult::Unsupported,
            };
            return extract_windows_ifilter(
                &payload.bytes,
                provider
                    .as_ref()
                    .and_then(|provider| provider.executable_path.as_deref())
                    .expect("available helper"),
                extension,
            );
        }
        Some("poppler-pdftotext") => {
            return extract_pdf(
                &payload.bytes,
                provider
                    .as_ref()
                    .and_then(|provider| provider.executable_path.as_deref())
                    .expect("available executable"),
            );
        }
        Some("memoria-text") => {}
        _ => return ExtractionResult::Unsupported,
    }
    if payload.info.mime.starts_with("text/") {
        let text = payload.decoded_text.as_deref().unwrap_or("");
        let text = if payload.info.mime == "text/html" {
            crate::html_text(text)
        } else {
            text.to_string()
        };
        return ExtractionResult::Text(text.chars().take(MAX_OUTPUT_BYTES).collect());
    }
    ExtractionResult::Unsupported
}

#[cfg(windows)]
fn extract_windows_ifilter(bytes: &[u8], helper: &Path, extension: &str) -> ExtractionResult {
    let serial = IFILTER_TEMP_COUNTER.fetch_add(1, Ordering::Relaxed);
    let directory =
        std::env::temp_dir().join(format!("memoria-ifilter-{}-{serial}", std::process::id()));
    if std::fs::create_dir(&directory).is_err() {
        return ExtractionResult::Failed;
    }
    let input = directory.join(format!("attachment{extension}"));
    let result = match std::fs::write(&input, bytes) {
        Ok(()) => run_ifilter_helper(helper, &input),
        Err(_) => ExtractionResult::Failed,
    };
    let _ = std::fs::remove_file(&input);
    let _ = std::fs::remove_dir(&directory);
    result
}

#[cfg(windows)]
fn run_ifilter_helper(program: &Path, input: &Path) -> ExtractionResult {
    let mut child = match Command::new(program)
        .arg(input)
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
    {
        Ok(child) => child,
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            return ExtractionResult::Unsupported
        }
        Err(_) => return ExtractionResult::Failed,
    };
    let (limit_sender, limit_receiver) = mpsc::channel();
    let mut stdout = child.stdout.take().expect("piped stdout");
    let reader = thread::spawn(move || {
        let mut output = Vec::new();
        let mut buffer = [0u8; 8192];
        let mut truncated = false;
        loop {
            let read = stdout.read(&mut buffer).map_err(|_| ())?;
            if read == 0 {
                break;
            }
            let remaining = MAX_OUTPUT_BYTES.saturating_sub(output.len());
            if read > remaining {
                output.extend_from_slice(&buffer[..remaining]);
                truncated = true;
                let _ = limit_sender.send(());
                break;
            }
            output.extend_from_slice(&buffer[..read]);
        }
        Ok::<_, ()>((output, truncated))
    });
    let deadline = Instant::now() + EXTRACTION_TIMEOUT;
    let mut limited = false;
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break Some(status),
            Ok(None) if limit_receiver.try_recv().is_ok() => {
                limited = true;
                let _ = child.kill();
                break child.wait().ok();
            }
            Ok(None) if Instant::now() >= deadline => {
                let _ = child.kill();
                break child.wait().ok();
            }
            Ok(None) => thread::sleep(Duration::from_millis(5)),
            Err(_) => {
                let _ = child.kill();
                break child.wait().ok();
            }
        }
    };
    let (output, truncated) = reader.join().ok().and_then(Result::ok).unwrap_or_default();
    if limited || truncated || !status.as_ref().is_some_and(|status| status.success()) {
        return if status
            .as_ref()
            .is_some_and(|status| status.code() == Some(3))
        {
            ExtractionResult::Unsupported
        } else {
            ExtractionResult::Failed
        };
    }
    ExtractionResult::Text(String::from_utf8_lossy(&output).trim().to_string())
}

fn extract_pdf(bytes: &[u8], executable: &Path) -> ExtractionResult {
    extract_pdf_with_command(
        bytes,
        executable,
        &["-layout", "-enc", "UTF-8", "-", "-"],
        EXTRACTION_TIMEOUT,
    )
}

fn extract_pdf_with_command(
    bytes: &[u8],
    program: &Path,
    args: &[&str],
    timeout: Duration,
) -> ExtractionResult {
    let mut child = match Command::new(program)
        .args(args)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
    {
        Ok(child) => child,
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            return ExtractionResult::Unsupported
        }
        Err(_) => return ExtractionResult::Failed,
    };

    let mut stdin = child.stdin.take().expect("piped stdin");
    let input = bytes.to_vec();
    let writer = thread::spawn(move || {
        let result = stdin.write_all(&input);
        drop(stdin);
        result
    });
    let (limit_sender, limit_receiver) = mpsc::channel();
    let mut stdout = child.stdout.take().expect("piped stdout");
    let reader = thread::spawn(move || {
        let mut output = Vec::new();
        let mut buffer = [0u8; 8192];
        let mut truncated = false;
        loop {
            let read = stdout.read(&mut buffer)?;
            if read == 0 {
                break;
            }
            let remaining = MAX_OUTPUT_BYTES.saturating_sub(output.len());
            if read > remaining {
                output.extend_from_slice(&buffer[..remaining]);
                truncated = true;
                let _ = limit_sender.send(());
                break;
            } else {
                output.extend_from_slice(&buffer[..read]);
            }
        }
        Ok::<_, io::Error>((output, truncated))
    });

    let deadline = Instant::now() + timeout;
    let mut timed_out = false;
    let mut output_limit = false;
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break Some(status),
            Ok(None) if limit_receiver.try_recv().is_ok() => {
                output_limit = true;
                let _ = child.kill();
                break child.wait().ok();
            }
            Ok(None) if Instant::now() >= deadline => {
                timed_out = true;
                let _ = child.kill();
                break child.wait().ok();
            }
            Ok(None) => thread::sleep(Duration::from_millis(5)),
            Err(_) => {
                let _ = child.kill();
                break child.wait().ok();
            }
        }
    };
    let _ = writer.join();
    let (output, truncated) = reader.join().ok().and_then(Result::ok).unwrap_or_default();
    if timed_out || output_limit || truncated || !status.is_some_and(|status| status.success()) {
        return ExtractionResult::Failed;
    }
    ExtractionResult::Text(String::from_utf8_lossy(&output).trim().to_string())
}

#[cfg(test)]
pub(crate) fn test_pdf_fixture(text: &str) -> Vec<u8> {
    let objects = [
            b"<< /Type /Catalog /Pages 2 0 R >>".to_vec(),
            b"<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_vec(),
            b"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] /Resources << /Font << /F1 5 0 R >> >> /Contents 4 0 R >>".to_vec(),
            format!("<< /Length {} >>\nstream\nBT\n/F1 12 Tf\n72 720 Td\n({text}) Tj\nET\nendstream", text.len() + 39).into_bytes(),
            b"<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>".to_vec(),
        ];
    let mut pdf = b"%PDF-1.4\n".to_vec();
    let mut offsets = Vec::new();
    for (index, object) in objects.iter().enumerate() {
        offsets.push(pdf.len());
        pdf.extend_from_slice(format!("{} 0 obj\n", index + 1).as_bytes());
        pdf.extend_from_slice(object);
        pdf.extend_from_slice(b"\nendobj\n");
    }
    let xref = pdf.len();
    pdf.extend_from_slice(
        format!("xref\n0 {}\n0000000000 65535 f \n", objects.len() + 1).as_bytes(),
    );
    for offset in offsets {
        pdf.extend_from_slice(format!("{offset:010} 00000 n \n").as_bytes());
    }
    pdf.extend_from_slice(
        format!(
            "trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{}\n%%EOF\n",
            objects.len() + 1,
            xref
        )
        .as_bytes(),
    );
    pdf
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn text_plain_attachment_is_extracted_without_a_provider() {
        let raw = b"Content-Type: multipart/mixed; boundary=x\r\n\r\n--x\r\nContent-Type: text/plain\r\nContent-Disposition: attachment; filename=note.txt\r\n\r\nphrase-secrete-947\r\n--x--\r\n";
        let parsed = mailparse::parse_mail(raw).unwrap();
        let (text, stats) = extract_attachment_texts(&parsed).unwrap();
        assert_eq!(text, "phrase-secrete-947");
        assert_eq!(stats.extracted, 1);
        assert_eq!(stats.unsupported, 0);
    }

    #[test]
    fn text_plain_attachment_respects_mime_charset() {
        let raw = b"Content-Type: multipart/mixed; boundary=x\r\n\r\n--x\r\nContent-Type: text/plain; charset=iso-8859-1\r\nContent-Disposition: attachment; filename=note.txt\r\n\r\ncaf\xe9 phrase-secrete-947\r\n--x--\r\n";
        let parsed = mailparse::parse_mail(raw).unwrap();
        let (text, _) = extract_attachment_texts(&parsed).unwrap();
        assert!(text.contains("café"));
        assert!(text.contains("phrase-secrete-947"));
    }

    #[test]
    fn pdf_fixture_uses_optional_system_provider() {
        let Some(provider) = selected_provider("application/pdf", &ProviderSelection::Automatic)
        else {
            return;
        };
        let result = extract_pdf(
            &test_pdf_fixture("phrase-secrete-947"),
            provider.executable_path.as_deref().unwrap(),
        );
        match result {
            ExtractionResult::Text(text) => assert!(text.contains("phrase-secrete-947")),
            ExtractionResult::Unsupported => {}
            ExtractionResult::Failed => panic!("pdftotext failed for the controlled PDF fixture"),
        }
    }

    #[test]
    fn malformed_pdf_never_panics_or_returns_unbounded_output() {
        let Some(provider) = selected_provider("application/pdf", &ProviderSelection::Automatic)
        else {
            return;
        };
        let result = extract_pdf(b"not a PDF", provider.executable_path.as_deref().unwrap());
        assert!(matches!(
            result,
            ExtractionResult::Failed | ExtractionResult::Unsupported
        ));
    }

    #[cfg(unix)]
    #[test]
    fn provider_stdout_is_bounded_before_collection_can_grow() {
        let started = Instant::now();
        let result = extract_pdf_with_command(
            b"ignored",
            Path::new("/bin/sh"),
            &["-c", "head -c 9000000 /dev/zero; sleep 1"],
            Duration::from_secs(2),
        );
        assert!(matches!(result, ExtractionResult::Failed));
        assert!(started.elapsed() < Duration::from_millis(500));
    }

    #[cfg(unix)]
    #[test]
    fn timed_out_provider_is_killed_and_reaped() {
        let started = Instant::now();
        let result = extract_pdf_with_command(
            b"ignored",
            Path::new("/bin/sh"),
            &["-c", "sleep 1"],
            Duration::from_millis(25),
        );
        assert!(matches!(result, ExtractionResult::Failed));
        assert!(started.elapsed() < Duration::from_millis(500));
    }

    #[test]
    fn provider_discovery_and_automatic_selection_are_stable() {
        let providers = discover_providers();
        assert!(providers
            .iter()
            .any(|provider| provider.id.as_str() == "memoria-text"));
        assert!(providers
            .iter()
            .any(|provider| provider.id.as_str() == "poppler-pdftotext"));
        assert_eq!(
            providers_for_mime("text/plain")[0].id.as_str(),
            "memoria-text"
        );
        assert!(
            selected_provider("application/x-unknown", &ProviderSelection::Automatic).is_none()
        );
        let text = selected_provider("text/plain", &ProviderSelection::Automatic).unwrap();
        assert_eq!(text.id.as_str(), "memoria-text");
        assert_eq!(text.id, ProviderId::new("memoria-text"));
    }

    #[test]
    fn missing_pdftotext_is_an_unavailable_provider() {
        let provider = discover_pdftotext_at(None);
        assert_eq!(provider.id.as_str(), "poppler-pdftotext");
        assert_eq!(provider.availability, ProviderAvailability::Unavailable);
        assert!(provider.executable_path.is_none());
        assert!(provider.version.is_none());
    }

    #[test]
    fn explicit_selection_can_represent_a_future_provider_choice() {
        let selection = ProviderSelection::Explicit(ProviderId::new("poppler-pdftotext"));
        let selected = selected_provider("application/pdf", &selection);
        if discover_providers().iter().any(|provider| {
            provider.id.as_str() == "poppler-pdftotext"
                && provider.availability == ProviderAvailability::Available
        }) {
            assert_eq!(selected.unwrap().id.as_str(), "poppler-pdftotext");
        } else {
            assert!(selected.is_none());
        }
    }
}
