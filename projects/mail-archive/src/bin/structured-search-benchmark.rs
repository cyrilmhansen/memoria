use base64::engine::general_purpose::STANDARD as BASE64;
use base64::Engine;
use mail_archive_experiment::{
    create_metadata, directory_bytes, index_gmail_archive_with_observer_and_config, latency_stats,
    ArchiveWriter, AttachmentFilter, GmailIndexWriterConfig, GmailSearchIndex, SearchRequest,
};
use rusqlite::params;
use serde_json::json;
use std::env;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

const DEFAULT_MESSAGES: u64 = 1_000_000;
const SEGMENT_BYTES: u64 = 64 * 1024 * 1024;
const DAY_MS: i64 = 86_400_000;
const END_MS: i64 = 1_767_225_600_000; // 2026-01-01 UTC

#[derive(Default)]
struct Population {
    messages: u64,
    with_attachment: u64,
    image: u64,
    pdf: u64,
    zip: u64,
    office: u64,
    starred: u64,
    work: u64,
    inbox: u64,
    sender_zero: u64,
    recent: u64,
}

struct Workload {
    name: &'static str,
    request: SearchRequest,
}

fn option(args: &[String], name: &str) -> Option<String> {
    args.windows(2)
        .find(|pair| pair[0] == name)
        .map(|pair| pair[1].clone())
}

fn safe_archive(path: &Path) -> io::Result<()> {
    fs::create_dir_all(path.join("archive"))?;
    Ok(())
}

fn labels(hash: u64) -> Vec<String> {
    let mut result = Vec::new();
    let bucket = hash % 100;
    if bucket < 55 {
        result.push("INBOX".to_string());
    } else {
        result.push("ARCHIVE".to_string());
    }
    if bucket < 18 {
        result.push("SENT".to_string());
    }
    if bucket < 30 {
        result.push("WORK".to_string());
    }
    if bucket < 8 {
        result.push("STARRED".to_string());
    }
    if bucket.is_multiple_of(13) {
        result.push("IMPORTANT".to_string());
    }
    result
}

fn sender_rank(hash: u64) -> usize {
    let roll = hash % 100;
    if roll < 22 {
        0
    } else if roll < 34 {
        1
    } else if roll < 43 {
        2
    } else {
        3 + ((hash >> 8) as usize % 997)
    }
}

fn mime_kind(hash: u64) -> (&'static str, &'static str) {
    match hash % 100 {
        0..=34 => ("image/jpeg", "photo.jpg"),
        35..=59 => ("application/pdf", "report.pdf"),
        60..=74 => (
            "application/vnd.openxmlformats-officedocument.wordprocessingml.document",
            "document.docx",
        ),
        75..=84 => ("application/zip", "bundle.zip"),
        _ => ("text/csv", "export.csv"),
    }
}

fn payload_size(hash: u64, mime: &str) -> usize {
    let roll = (hash >> 16) % 100;
    match mime {
        "image/jpeg" => {
            if roll < 85 {
                4096
            } else {
                64 * 1024
            }
        }
        "application/pdf" => {
            if roll < 90 {
                8192
            } else {
                128 * 1024
            }
        }
        "application/zip" => {
            if roll < 90 {
                32 * 1024
            } else {
                256 * 1024
            }
        }
        "text/csv" => {
            if roll < 90 {
                2048
            } else {
                32 * 1024
            }
        }
        _ => {
            if roll < 90 {
                4096
            } else {
                64 * 1024
            }
        }
    }
}

fn message_timestamp(hash: u64) -> i64 {
    let roll = hash % 100;
    let (start_days, span_days) = if roll < 45 {
        (0, 2 * 365)
    } else if roll < 75 {
        (2 * 365, 3 * 365)
    } else if roll < 90 {
        (5 * 365, 3 * 365)
    } else {
        (8 * 365, 3 * 365)
    };
    END_MS - (start_days + ((hash >> 16) % span_days as u64) as i64) * DAY_MS
}

fn raw_message(id: u64, hash: u64, population: &mut Population) -> (Vec<u8>, Vec<String>, i64) {
    let sender = sender_rank(hash);
    let timestamp = message_timestamp(hash);
    let labels = labels(hash);
    let mime_attachment = hash % 100 < 30;
    let rare = format!("rarepulse{:04}", id % 10_000);
    let body = format!("project archive meeting {rare} synthetic correspondence {id}");
    let mut raw = format!(
        "From: Contact {sender:04} <contact-{sender:04}@example.test>\r\nTo: team@example.test\r\nSubject: Project update {id}\r\nDate-Unix: {timestamp}\r\nMIME-Version: 1.0\r\n"
    )
    .into_bytes();
    if mime_attachment {
        let (mime, filename) = mime_kind(hash >> 24);
        let size = payload_size(hash, mime);
        let payload = vec![b'A' + (hash % 20) as u8; size];
        let encoded = BASE64.encode(payload);
        raw.extend_from_slice(
            format!(
                "Content-Type: multipart/mixed; boundary=atlas\r\n\r\n--atlas\r\nContent-Type: text/plain\r\n\r\n{body}\r\n--atlas\r\nContent-Type: {mime}\r\nContent-Disposition: attachment; filename=\"{filename}\"\r\nContent-Transfer-Encoding: base64\r\n\r\n{encoded}\r\n--atlas--\r\n"
            )
            .as_bytes(),
        );
        population.with_attachment += 1;
        match mime {
            "image/jpeg" => population.image += 1,
            "application/pdf" => population.pdf += 1,
            "application/zip" => population.zip += 1,
            value if value.starts_with("application/vnd.") => population.office += 1,
            _ => {}
        }
    } else {
        raw.extend_from_slice(format!("Content-Type: text/plain\r\n\r\n{body}\r\n").as_bytes());
    }
    population.messages += 1;
    if labels.iter().any(|label| label == "STARRED") {
        population.starred += 1;
    }
    if labels.iter().any(|label| label == "WORK") {
        population.work += 1;
    }
    if labels.iter().any(|label| label == "INBOX") {
        population.inbox += 1;
    }
    if sender == 0 {
        population.sender_zero += 1;
    }
    if timestamp >= 1_704_067_200_000 {
        population.recent += 1;
    }
    (raw, labels, timestamp)
}

fn rss_kib() -> Option<u64> {
    let status = fs::read_to_string("/proc/self/status").ok()?;
    status.lines().find_map(|line| {
        line.strip_prefix("VmRSS:")?
            .split_whitespace()
            .next()?
            .parse()
            .ok()
    })
}

fn snapshot(phases: &Arc<Mutex<Vec<(String, u64)>>>, label: &str) {
    if let Some(rss) = rss_kib() {
        phases.lock().unwrap().push((label.to_string(), rss));
    }
}

fn peak_rss_kib(running: Arc<AtomicBool>, peak: Arc<AtomicU64>) {
    while running.load(Ordering::Relaxed) {
        if let Some(kib) = rss_kib() {
            peak.fetch_max(kib, Ordering::Relaxed);
        }
        thread::sleep(Duration::from_millis(25));
    }
}

fn workloads() -> Vec<Workload> {
    vec![
        Workload {
            name: "text_frequent",
            request: SearchRequest {
                text: "project".into(),
                ..Default::default()
            },
        },
        Workload {
            name: "text_rare",
            request: SearchRequest {
                text: "rarepulse9999".into(),
                ..Default::default()
            },
        },
        Workload {
            name: "date_selective",
            request: SearchRequest {
                date_from: Some(1_704_067_200_000),
                date_to: Some(1_767_225_600_000),
                ..Default::default()
            },
        },
        Workload {
            name: "attachment",
            request: SearchRequest {
                attachment: AttachmentFilter::With,
                ..Default::default()
            },
        },
        Workload {
            name: "without_attachment",
            request: SearchRequest {
                attachment: AttachmentFilter::Without,
                ..Default::default()
            },
        },
        Workload {
            name: "mime_image",
            request: SearchRequest {
                attachment_mime: Some("image/*".into()),
                ..Default::default()
            },
        },
        Workload {
            name: "mime_pdf",
            request: SearchRequest {
                attachment_mime: Some("application/pdf".into()),
                ..Default::default()
            },
        },
        Workload {
            name: "label_starred",
            request: SearchRequest {
                labels: vec!["STARRED".into()],
                ..Default::default()
            },
        },
        Workload {
            name: "label_work",
            request: SearchRequest {
                labels: vec!["WORK".into()],
                ..Default::default()
            },
        },
        Workload {
            name: "sender_frequent",
            request: SearchRequest {
                from: Some("contact-0000@example.test".into()),
                ..Default::default()
            },
        },
        Workload {
            name: "sender_fragment",
            request: SearchRequest {
                from: Some("contact".into()),
                ..Default::default()
            },
        },
        Workload {
            name: "text_date",
            request: SearchRequest {
                text: "project".into(),
                date_from: Some(1_704_067_200_000),
                date_to: Some(1_767_225_600_000),
                ..Default::default()
            },
        },
        Workload {
            name: "text_attachment",
            request: SearchRequest {
                text: "project".into(),
                attachment: AttachmentFilter::With,
                ..Default::default()
            },
        },
        Workload {
            name: "text_mime",
            request: SearchRequest {
                text: "project".into(),
                attachment_mime: Some("application/pdf".into()),
                ..Default::default()
            },
        },
        Workload {
            name: "text_label",
            request: SearchRequest {
                text: "project".into(),
                labels: vec!["STARRED".into()],
                ..Default::default()
            },
        },
        Workload {
            name: "no_result",
            request: SearchRequest {
                text: "never-present-atlas-term".into(),
                ..Default::default()
            },
        },
    ]
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<String> = env::args().collect();
    let messages = option(&args, "--messages")
        .map(|v| v.parse())
        .transpose()?
        .unwrap_or(DEFAULT_MESSAGES);
    let out = PathBuf::from(option(&args, "--out").ok_or("--out is required")?);
    let seed = option(&args, "--seed")
        .map(|v| v.parse())
        .transpose()?
        .unwrap_or(0xA71A_2026_u64);
    let defaults = GmailIndexWriterConfig::default();
    let writer_config = GmailIndexWriterConfig {
        memory_budget_bytes: option(&args, "--writer-budget")
            .map(|value| value.parse())
            .transpose()?
            .unwrap_or(defaults.memory_budget_bytes),
        worker_threads: option(&args, "--writer-workers")
            .map(|value| value.parse())
            .transpose()?
            .or(Some(3)),
        merge_threads: option(&args, "--merge-threads")
            .map(|value| value.parse())
            .transpose()?
            .unwrap_or(defaults.merge_threads),
        no_merge_policy: args.iter().any(|value| value == "--no-merge"),
    };
    if out.exists() && out.join("metadata.sqlite").exists() {
        return Err("refusing to overwrite an existing campaign directory".into());
    }
    safe_archive(&out)?;
    let running = Arc::new(AtomicBool::new(true));
    let peak = Arc::new(AtomicU64::new(0));
    let sampler = {
        let running = running.clone();
        let peak = peak.clone();
        thread::spawn(move || peak_rss_kib(running, peak))
    };
    let phases = Arc::new(Mutex::new(Vec::new()));
    snapshot(&phases, "startup");
    let started = Instant::now();
    let metadata = create_metadata(&out.join("metadata.sqlite"))?;
    snapshot(&phases, "catalog_opened");
    let transaction = metadata.unchecked_transaction()?;
    let mut message_insert = transaction.prepare("INSERT INTO messages(doc_id,message_id,timestamp,sender,recipients,subject,account,folder,thread,segment,archive_offset,frame_bytes) VALUES (?1,?2,0,'','','',?3,'','',?4,?5,?6)")?;
    let mut gmail_insert = transaction.prepare("INSERT INTO gmail_messages(source_account,gmail_message_id,doc_id,thread_id,label_ids,internal_date_ms,message_history_id,source_state,first_seen_unix,last_seen_unix) VALUES ('synthetic','gmail-'||?1,?2,'thread-'||?1,?3,?4,?5,'present',0,0)")?;
    let mut writer = ArchiveWriter::open(&out.join("archive"), SEGMENT_BYTES)?;
    let mut population = Population::default();
    for id in 0..messages {
        let hash = seed.wrapping_add(id.wrapping_mul(0x9e37_79b9));
        let (raw, labels, timestamp) = raw_message(id, hash, &mut population);
        let location = writer.append_raw(id, &raw)?;
        message_insert.execute(params![
            id as i64,
            format!("gmail:synthetic:gmail-{id}"),
            "synthetic",
            location.segment,
            location.offset as i64,
            location.frame_bytes as i64,
        ])?;
        gmail_insert.execute(params![
            id as i64,
            id as i64,
            json!(labels).to_string(),
            timestamp,
            format!("{id}"),
        ])?;
    }
    drop(message_insert);
    drop(gmail_insert);
    transaction.commit()?;
    writer.sync()?;
    snapshot(&phases, "after_archive_generation");
    let generate_ms = started.elapsed().as_millis();
    let index_started = Instant::now();
    snapshot(&phases, "before_index");
    let observer_phases = phases.clone();
    let index_stats = index_gmail_archive_with_observer_and_config(
        &out,
        |label| {
            snapshot(&observer_phases, label);
        },
        writer_config,
    )?;
    let index_ms = index_started.elapsed().as_millis();
    snapshot(&phases, "after_index");
    let search = GmailSearchIndex::open(&out)?;
    snapshot(&phases, "before_searches");
    let mut output = String::new();
    output.push_str(&format!("command=structured-search-benchmark\nmessages={messages}\nseed={seed}\nwriter_budget_bytes={}\nwriter_workers={}\nmerge_threads={}\nno_merge_policy={}\ngenerate_ms={generate_ms}\nindex_ms={index_ms}\nindex_examined={}\nindex_indexed={}\nindex_read_us={}\nindex_parse_us={}\nindex_commit_us={}\nindex_segments_before_commit={}\nindex_segments_after_commit={}\nindex_segments_after_index={}\narchive_bytes={}\nindex_bytes={}\npeak_rss_kib={}\npopulation_with_attachment={}\npopulation_image={}\npopulation_pdf={}\npopulation_zip={}\npopulation_office={}\npopulation_starred={}\npopulation_work={}\npopulation_inbox={}\npopulation_sender_zero={}\npopulation_recent={}\n", writer_config.memory_budget_bytes, writer_config.worker_threads.unwrap_or(0), writer_config.merge_threads, writer_config.no_merge_policy, index_stats.examined, index_stats.indexed, index_stats.read_us, index_stats.parse_us, index_stats.index_us, index_stats.segments_before_commit, index_stats.segments_after_commit, index_stats.segments_after_index, directory_bytes(out.join("archive"))?, index_stats.index_bytes, peak.load(Ordering::Relaxed), population.with_attachment, population.image, population.pdf, population.zip, population.office, population.starred, population.work, population.inbox, population.sender_zero, population.recent));
    for mut workload in workloads() {
        workload.request.limit = 50;
        let _ = search.search_request(&workload.request)?;
        let mut durations = Vec::with_capacity(100);
        let mut result_count = 0;
        for _ in 0..100 {
            let started = Instant::now();
            result_count = search.search_request(&workload.request)?.len();
            durations.push(started.elapsed());
        }
        let stats = latency_stats(durations);
        snapshot(&phases, "after_workload");
        output.push_str(&format!("workload_{}_results={}\nworkload_{}_p50_us={}\nworkload_{}_p95_us={}\nworkload_{}_p99_us={}\nworkload_{}_max_us={}\n", workload.name, result_count, workload.name, stats.p50_us, workload.name, stats.p95_us, workload.name, stats.p99_us, workload.name, stats.max_us));
    }
    running.store(false, Ordering::Relaxed);
    sampler.join().map_err(|_| "RSS sampler panicked")?;
    for (label, rss) in phases.lock().unwrap().iter() {
        output.push_str(&format!("rss_phase_{label}_kib={rss}\n"));
    }
    print!("{output}");
    Ok(())
}
