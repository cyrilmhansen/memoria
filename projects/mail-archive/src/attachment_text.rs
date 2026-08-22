use std::io::{self, Read, Write};
use std::path::Path;
use std::process::{Command, Stdio};
use std::sync::mpsc;
use std::thread;
use std::time::{Duration, Instant};

use mailparse::ParsedMail;

use crate::AttachmentPayload;

pub const MAX_INPUT_BYTES: usize = 64 * 1024 * 1024;
pub const MAX_OUTPUT_BYTES: usize = 8 * 1024 * 1024;
pub const EXTRACTION_TIMEOUT: Duration = Duration::from_secs(10);

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
    if payload.info.mime == "application/pdf" {
        return extract_pdf(&payload.bytes);
    }
    if payload.info.mime.starts_with("text/") {
        let text = payload.decoded_text.as_deref().unwrap_or_else(|| "");
        let text = if payload.info.mime == "text/html" {
            crate::html_text(text)
        } else {
            text.to_string()
        };
        return ExtractionResult::Text(text.chars().take(MAX_OUTPUT_BYTES).collect());
    }
    ExtractionResult::Unsupported
}

fn extract_pdf(bytes: &[u8]) -> ExtractionResult {
    extract_pdf_with_command(
        bytes,
        Path::new("pdftotext"),
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
        let result = extract_pdf(&test_pdf_fixture("phrase-secrete-947"));
        match result {
            ExtractionResult::Text(text) => assert!(text.contains("phrase-secrete-947")),
            ExtractionResult::Unsupported => {}
            ExtractionResult::Failed => panic!("pdftotext failed for the controlled PDF fixture"),
        }
    }

    #[test]
    fn malformed_pdf_never_panics_or_returns_unbounded_output() {
        let result = extract_pdf(b"not a PDF");
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
}
