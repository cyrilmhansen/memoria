use mail_archive_experiment::gmail::GmailTransport;
use mail_archive_experiment::*;
use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::Instant;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<String> = env::args().collect();
    let command = args.get(1).map(String::as_str).unwrap_or("benchmark");
    let messages = option(&args, "--messages")
        .unwrap_or_else(|| "5000".into())
        .parse::<u64>()?;
    let seed = option(&args, "--seed")
        .unwrap_or_else(|| DEFAULT_SEED.to_string())
        .parse::<u64>()?;
    let profile = option(&args, "--profile")
        .map(|value| CorpusProfile::parse(&value).ok_or_else(|| format!("profil inconnu: {value}")))
        .transpose()?
        .unwrap_or(CorpusProfile::Personal);
    let (default_attachment_rate, default_duplicate_rate, default_max_attachment_bytes) =
        profile.defaults();
    let out = PathBuf::from(
        option(&args, "--out").unwrap_or_else(|| "/tmp/mail-archive-experiment".into()),
    );
    let segment_bytes = option(&args, "--segment-bytes")
        .unwrap_or_else(|| (64 * 1024 * 1024).to_string())
        .parse::<u64>()?;
    let config = CorpusConfig {
        messages,
        seed,
        profile,
        attachment_rate: option(&args, "--attachment-rate")
            .unwrap_or_else(|| default_attachment_rate.to_string())
            .parse()?,
        duplicate_rate: option(&args, "--duplicate-rate")
            .unwrap_or_else(|| default_duplicate_rate.to_string())
            .parse()?,
        max_attachment_bytes: option(&args, "--max-attachment-bytes")
            .unwrap_or_else(|| default_max_attachment_bytes.to_string())
            .parse()?,
        measure_compression: option(&args, "--compression").is_some(),
    };
    match command {
        "generate" => generate(&out, config, segment_bytes)?,
        "benchmark" => benchmark(
            &out,
            config,
            option(&args, "--queries")
                .unwrap_or_else(|| "200".into())
                .parse()?,
            segment_bytes,
        )?,
        "cas-benchmark" => cas_benchmark(&out, config)?,
        "gmail-sync" => gmail_sync(&args, &out)?,
        "recover-gmail-raw" => recover_gmail_raw(&args, &out)?,
        "gmail-report" => gmail_report(&args, &out)?,
        "archive-inventory" => archive_inventory(&args, &out)?,
        "recovery-plan" => recovery_plan(&args, &out)?,
        "gmail-index" => gmail_index(&args, &out)?,
        "search" => search(&args, &out)?,
        "help" | "--help" | "-h" => help(),
        other => return Err(format!("commande inconnue: {other}").into()),
    }
    Ok(())
}

fn option(args: &[String], name: &str) -> Option<String> {
    args.windows(2)
        .find(|pair| pair[0] == name)
        .map(|pair| pair[1].clone())
}

fn print_stats(
    command: &str,
    config: CorpusConfig,
    stats: &DatasetStats,
    archive_bytes: u64,
    segment_bytes: u64,
    import_ms: u128,
) {
    println!("command={command}\nprofile={}\nmessages={}\narchive_bytes={}\nraw_bytes={}\nmean_bytes={}\nmin_bytes={}\nmedian_bytes={}\np90_bytes={}\np99_bytes={}\nmax_bytes={}\nmime_text_bytes={}\ncompressed_bytes={}\nzstd_bytes={}\ntext_compressed_bytes={}\nattachment_compressed_bytes={}\ntext_zstd_bytes={}\nattachment_zstd_bytes={}\nattachments={}\nunique_attachment_hashes={}\nattachment_bytes={}\nunique_attachment_bytes={}\nduplicate_attachment_objects={}\nduplicate_attachment_bytes={}\nduplicate_size_p50={}\nduplicate_size_p90={}\nduplicate_size_max={}\nsegment_bytes={}\nimport_ms={}", config.profile.name(), stats.messages, archive_bytes, stats.bytes, stats.mean_bytes, stats.min_bytes, stats.median_bytes, stats.p90_bytes, stats.p99_bytes, stats.max_bytes, stats.mime_text_bytes, stats.compressed_bytes, stats.zstd_bytes, stats.text_compressed_bytes, stats.attachment_compressed_bytes, stats.text_zstd_bytes, stats.attachment_zstd_bytes, stats.attachments, stats.unique_attachment_hashes, stats.attachment_bytes, stats.unique_attachment_bytes, stats.duplicate_attachment_objects, stats.duplicate_attachment_bytes, stats.duplicate_size_p50, stats.duplicate_size_p90, stats.duplicate_size_max, segment_bytes, import_ms);
}

fn generate(
    out: &Path,
    config: CorpusConfig,
    segment_bytes: u64,
) -> Result<(), Box<dyn std::error::Error>> {
    fs::create_dir_all(out)?;
    let started = Instant::now();
    let (stats, archive_bytes) = build_archive(out, config, segment_bytes)?;
    write_manifest(&out.join("manifest.txt"), config, &stats)?;
    print_stats(
        "generate",
        config,
        &stats,
        archive_bytes,
        segment_bytes,
        started.elapsed().as_millis(),
    );
    Ok(())
}

fn cas_benchmark(out: &Path, config: CorpusConfig) -> Result<(), Box<dyn std::error::Error>> {
    let variants = [
        CasVariant::Inline,
        CasVariant::Exact,
        CasVariant::Decoded,
        CasVariant::Hybrid {
            threshold: 64 * 1024,
        },
        CasVariant::Hybrid {
            threshold: 256 * 1024,
        },
    ];
    for variant in variants {
        let directory = match variant {
            CasVariant::Hybrid { threshold } => format!("hybrid-{threshold}"),
            _ => variant.name().into(),
        };
        let stats = run_cas(&out.join(directory), config, variant)?;
        println!("command=cas-benchmark\nprofile={}\nvariant={}\nmessages={}\ninput_bytes={}\nphysical_bytes={}\nsaved_bytes={}\nmessage_store_bytes={}\nblob_bytes={}\nmanifest_bytes={}\nblobs={}\nunique_blob_bytes={}\nexternalized_objects={}\nhashed_bytes={}\nhash_us={}\nimport_us={}\nreconstruction_us={}\nrandom_access_us={}\nmax_blob_bytes={}", config.profile.name(), stats.variant, stats.messages, stats.input_bytes, stats.physical_bytes, stats.input_bytes.saturating_add(stats.messages * 32).saturating_sub(stats.physical_bytes), stats.message_store_bytes, stats.blob_bytes, stats.manifest_bytes, stats.blobs, stats.unique_blob_bytes, stats.externalized_objects, stats.hashed_bytes, stats.hash_us, stats.import_us, stats.reconstruction_us, stats.random_access_us, stats.max_blob_bytes);
    }
    Ok(())
}

fn gmail_sync(args: &[String], default_out: &Path) -> Result<(), Box<dyn std::error::Error>> {
    let archive = PathBuf::from(
        option(args, "--archive").unwrap_or_else(|| default_out.display().to_string()),
    );
    let credentials =
        PathBuf::from(option(args, "--credentials").ok_or("--credentials is required")?);
    let token_dir = option(args, "--token-dir")
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            dirs::config_dir()
                .unwrap_or_else(|| PathBuf::from("."))
                .join("mail-archive-experiment/tokens")
        });
    if credentials.starts_with(&archive) || token_dir.starts_with(&archive) {
        return Err("credentials and tokens must be outside the archive directory".into());
    }
    let requested_account = option(args, "--account");
    let query = option(args, "--query");
    let max = option(args, "--max-messages")
        .map(|value| value.parse::<u64>())
        .transpose()?;
    let mut transport = gmail::HttpGmail::authenticate(&credentials, &token_dir)?;
    let email = transport
        .profile()?
        .email_address
        .ok_or("Gmail profile did not return an email address")?;
    let account = gmail::gmail_source_account(&email);
    if requested_account
        .as_deref()
        .is_some_and(|value| gmail::gmail_source_account(value) != account)
    {
        return Err("--account does not match the authenticated Gmail profile".into());
    }
    let stats = gmail::sync_account(&archive, &account, &mut transport, query.as_deref(), max)?;
    println!("command=gmail-sync\nfull_sync={}\nexamined={}\nnew_messages={}\nlabel_changes={}\ndeletions_detected={}\nnetwork_bytes={}\narchive_bytes_added={}\nmime_messages={}\nmime_parse_failures={}\nattachments={}\nattachment_encoded_bytes={}\nattachment_decoded_bytes={}\nattachment_unique_encoded_objects={}\nattachment_unique_encoded_bytes={}\nattachment_unique_decoded_objects={}\nattachment_unique_decoded_bytes={}\nattachment_encoded_over_64k_bytes={}\nduration_ms={}", stats.full_sync, stats.examined, stats.new_messages, stats.label_changes, stats.deletions, stats.network_bytes, stats.archive_bytes_added, stats.mime_messages, stats.mime_parse_failures, stats.attachments, stats.attachment_encoded_bytes, stats.attachment_decoded_bytes, stats.attachment_unique_encoded_objects, stats.attachment_unique_encoded_bytes, stats.attachment_unique_decoded_objects, stats.attachment_unique_decoded_bytes, stats.attachment_encoded_over_64k_bytes, stats.duration_ms);
    Ok(())
}

fn recover_gmail_raw(
    args: &[String],
    default_out: &Path,
) -> Result<(), Box<dyn std::error::Error>> {
    let archive = PathBuf::from(
        option(args, "--archive").unwrap_or_else(|| default_out.display().to_string()),
    );
    let doc_id = option(args, "--doc-id")
        .ok_or("--doc-id is required")?
        .parse::<i64>()?;
    let credentials =
        PathBuf::from(option(args, "--credentials").ok_or("--credentials is required")?);
    let token_dir = option(args, "--token-dir")
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            dirs::config_dir()
                .unwrap_or_else(|| PathBuf::from("."))
                .join("mail-archive-experiment/tokens")
        });
    if credentials.starts_with(&archive) || token_dir.starts_with(&archive) {
        return Err("credentials and tokens must be outside the archive directory".into());
    }
    let mut transport = gmail::HttpGmail::authenticate(&credentials, &token_dir)?;
    let result =
        recovery::recover_missing_gmail_raw(&archive, doc_id, &mut transport, 64 * 1024 * 1024)?;
    println!("command=recover-gmail-raw\ndoc_id={doc_id}\nresult={result:?}");
    Ok(())
}

fn gmail_report(args: &[String], default_out: &Path) -> Result<(), Box<dyn std::error::Error>> {
    let archive = PathBuf::from(
        option(args, "--archive").unwrap_or_else(|| default_out.display().to_string()),
    );
    let report = gmail::analyze_archived_mime(&archive)?;
    println!(
        "command=gmail-report\nmessages={}\nraw_p50={}\nraw_p90={}\nraw_p99={}\nraw_max={}\nmultipart_messages={}\nparts={}\nleaves={}\nattachments={}\nattachment_bytes_decoded={}\nattachment_bytes_encoded={}\nattachment_unique_encoded_objects={}\nattachment_unique_encoded_bytes={}\nattachment_unique_decoded_objects={}\nattachment_unique_decoded_bytes={}\nattachment_encoded_over_64k_bytes={}\nattachment_candidate_bytes_decoded={}\nattachment_candidate_bytes_encoded={}\nattachment_candidate_unique_encoded_objects={}\nattachment_candidate_unique_encoded_bytes={}\nattachment_candidate_unique_decoded_objects={}\nattachment_candidate_unique_decoded_bytes={}\nattachment_candidate_encoded_over_64k_objects={}\nattachment_candidate_encoded_over_64k_bytes={}\nattachment_candidate_encoded_over_64k_unique_objects={}\nattachment_candidate_encoded_over_64k_unique_bytes={}\ninline_parts={}\ninline_bytes={}\nfilename_or_name_parts={}\nfilename_or_name_bytes={}\ncontent_id_parts={}\ncontent_id_bytes={}\nimage_parts={}\nimage_bytes={}\npdf_parts={}\npdf_bytes={}\nzip_parts={}\nzip_bytes={}\noffice_parts={}\noffice_bytes={}\nother_application_parts={}\nother_application_bytes={}\nchecksum_verified_frames={}\nsegment_files={}\nphysical_archive_bytes={}",
        report.messages,
        report.percentile(0.50),
        report.percentile(0.90),
        report.percentile(0.99),
        report.max_raw(),
        report.multipart_messages,
        report.parts,
        report.leaves,
        report.attachments,
        report.attachment_bytes,
        report.attachment_encoded_bytes,
        report.attachment_unique_encoded_objects,
        report.attachment_unique_encoded_bytes,
        report.attachment_unique_decoded_objects,
        report.attachment_unique_decoded_bytes,
        report.attachment_encoded_over_64k_bytes,
        report.attachment_candidate_bytes_decoded,
        report.attachment_candidate_bytes_encoded,
        report.attachment_candidate_unique_encoded_objects,
        report.attachment_candidate_unique_encoded_bytes,
        report.attachment_candidate_unique_decoded_objects,
        report.attachment_candidate_unique_decoded_bytes,
        report.attachment_candidate_encoded_over_64k_objects,
        report.attachment_candidate_encoded_over_64k_bytes,
        report.attachment_candidate_encoded_over_64k_unique_objects,
        report.attachment_candidate_encoded_over_64k_unique_bytes,
        report.inline,
        report.inline_bytes,
        report.filename_or_name,
        report.filename_or_name_bytes,
        report.content_id,
        report.content_id_bytes,
        report.image_parts,
        report.image_bytes,
        report.pdf_parts,
        report.pdf_bytes,
        report.zip_parts,
        report.zip_bytes,
        report.office_parts,
        report.office_bytes,
        report.other_application_parts,
        report.other_application_bytes,
        report.checksum_verified_frames,
        report.segment_files,
        report.physical_archive_bytes,
    );
    Ok(())
}

fn archive_inventory(
    args: &[String],
    default_out: &Path,
) -> Result<(), Box<dyn std::error::Error>> {
    let archive = PathBuf::from(
        option(args, "--archive").unwrap_or_else(|| default_out.display().to_string()),
    );
    let inventory = inventory_physical(&archive)?;
    println!(
        "command=archive-inventory\ncatalogued_records={}\nvalidated_catalogued_records={}\norphan_valid_frames={}\ninconsistent_catalogued_records={}\ncatalogued_physically_missing={}\nphysical_corruptions={}\nincomplete_tails={}",
        inventory.catalogued_records,
        inventory.validated_catalogued_records,
        inventory.orphan_valid_frames,
        inventory.inconsistent_catalogued_records,
        inventory.catalogued_physically_missing,
        inventory.physical_corruptions,
        inventory.incomplete_tails,
    );
    Ok(())
}

fn recovery_plan(args: &[String], default_out: &Path) -> Result<(), Box<dyn std::error::Error>> {
    let archive = PathBuf::from(
        option(args, "--archive").unwrap_or_else(|| default_out.display().to_string()),
    );
    let plan = recovery::plan_recovery(&archive)?;
    println!(
        "command=recovery-plan\ncatalogue_state={}\narchive_state={}\nitems={}",
        plan.catalogue_state,
        plan.archive_state,
        plan.items.len()
    );
    for item in plan.items {
        println!(
            "subject={}\nstatus={}\ndisposition={}\nautomatic={}\nproposed_action={}\nreason={}",
            item.subject,
            item.status,
            item.disposition.label(),
            item.automatic,
            item.proposed_action,
            item.evidence.facts.join("; ")
        );
    }
    Ok(())
}

fn gmail_index(args: &[String], default_out: &Path) -> Result<(), Box<dyn std::error::Error>> {
    let archive = PathBuf::from(
        option(args, "--archive").unwrap_or_else(|| default_out.display().to_string()),
    );
    let started = Instant::now();
    let stats = index_gmail_archive(&archive)?;
    let search_open_started = Instant::now();
    let search_index = GmailSearchIndex::open(&archive)?;
    let search_open_us = search_open_started.elapsed().as_micros();
    let workload = [
        ("rare", "invoice"),
        ("frequent", "the"),
        ("multiple", "meeting project"),
        ("phrase", "\"thank you\""),
        ("sender", "from:noreply"),
        ("recipient", "to:example"),
        ("date", "after:2000-01-01 before:2100-01-01"),
        ("text_date", "invoice after:2000-01-01"),
        ("text_sender", "invoice from:noreply"),
        ("label", "label:INBOX"),
        ("none", "term_that_cannot_exist_7f3a"),
    ];
    println!(
        "command=gmail-index\nexamined={}\nindexed={}\nskipped={}\nremoved={}\nparse_failures={}\nattachment_encountered={}\nattachment_supported={}\nattachment_extracted={}\nattachment_unsupported={}\nattachment_extraction_failures={}\nattachment_decoded_bytes={}\nattachment_extracted_bytes={}\nattachment_extracted_chars={}\narchive_read_us={}\nparse_us={}\nindex_us={}\nopen_us={}\nindex_bytes={}\nwall_ms={}",
        stats.examined,
        stats.indexed,
        stats.skipped,
        stats.removed,
        stats.parse_failures,
        stats.attachment_encountered,
        stats.attachment_supported,
        stats.attachment_extracted,
        stats.attachment_unsupported,
        stats.attachment_extraction_failures,
        stats.attachment_decoded_bytes,
        stats.attachment_extracted_bytes,
        stats.attachment_extracted_chars,
        stats.read_us,
        stats.parse_us,
        stats.index_us,
        stats.open_us,
        stats.index_bytes,
        started.elapsed().as_millis(),
    );
    println!("search_open_us={}", search_open_us);
    let first_query_started = Instant::now();
    let _ = search_index.search("invoice", 20)?;
    println!(
        "first_query_us={}",
        first_query_started.elapsed().as_micros()
    );
    for (name, query) in workload {
        let mut durations = Vec::new();
        let mut result_count = 0usize;
        for _ in 0..20 {
            let query_started = Instant::now();
            let results = search_index.search(query, 20)?;
            durations.push(query_started.elapsed());
            result_count = results.len();
        }
        let latency = latency_stats(durations);
        println!(
            "workload_{}_results={}\nworkload_{}_p50_us={}\nworkload_{}_p95_us={}\nworkload_{}_p99_us={}\nworkload_{}_max_us={}",
            name, result_count, name, latency.p50_us, name, latency.p95_us, name, latency.p99_us, name, latency.max_us
        );
    }
    Ok(())
}

fn search(args: &[String], default_out: &Path) -> Result<(), Box<dyn std::error::Error>> {
    let archive = PathBuf::from(
        option(args, "--archive").unwrap_or_else(|| default_out.display().to_string()),
    );
    let query = args
        .iter()
        .skip(2)
        .filter(|value| {
            !value.starts_with("--")
                && *value != option(args, "--archive").as_deref().unwrap_or_default()
        })
        .cloned()
        .collect::<Vec<_>>()
        .join(" ");
    if query.is_empty() {
        return Err("search requires a query".into());
    }
    for result in search_gmail_archive(&archive, &query, 20)? {
        println!(
            "doc_id={} score={:.5} timestamp={} source_account={} archive_message_id={}",
            result.doc_id,
            result.score,
            result.timestamp,
            result.source_account,
            result.archive_message_id
        );
    }
    Ok(())
}

fn benchmark(
    out: &Path,
    config: CorpusConfig,
    query_repetitions: usize,
    segment_bytes: u64,
) -> Result<(), Box<dyn std::error::Error>> {
    let reset = ArchiveSession::reset(out, segment_bytes)?;
    drop(reset);
    let started = Instant::now();
    let (stats, archive_bytes) = build_archive(out, config, segment_bytes)?;
    let import_ms = started.elapsed().as_millis();
    let sqlite_path = out.join("sqlite-fts.db");
    let mut sqlite = create_sqlite_fts(&sqlite_path)?;
    let sqlite_started = Instant::now();
    let sqlite_pipeline = index_sqlite_archive(&mut sqlite, out, 0, config.messages)?;
    let sqlite_index_ms = sqlite_started.elapsed().as_millis();
    let tantivy_path = out.join("tantivy");
    let (tantivy, fields) = create_tantivy(&tantivy_path)?;
    let tantivy_started = Instant::now();
    let tantivy_pipeline = index_tantivy_archive(&tantivy, fields, out, 0, config.messages)?;
    let tantivy_index_ms = tantivy_started.elapsed().as_millis();
    let reader = tantivy.reader()?;
    let mut queries = Vec::new();
    for _ in 0..query_repetitions.max(10) {
        queries.extend(benchmark_queries());
    }
    let sqlite_latency = run_queries(&queries, |query| {
        let _ = sqlite_search(&sqlite, query);
    });
    let tantivy_latency = run_queries(&queries, |query| {
        let _ = tantivy_search(&tantivy, &reader, fields, query);
    });
    let sqlite_open_started = Instant::now();
    let reopened_sqlite = SqliteFtsIndex::open(&sqlite_path)?;
    let sqlite_open_us = sqlite_open_started.elapsed().as_micros();
    let sqlite_first_started = Instant::now();
    let _ = sqlite_search(&reopened_sqlite, "quartz");
    let sqlite_first_query_us = sqlite_first_started.elapsed().as_micros();
    drop(reopened_sqlite);
    let tantivy_open_started = Instant::now();
    let (reopened_tantivy, reopened_fields) =
        tantivy::Index::open_in_dir(&tantivy_path).map(|index| (index, fields))?;
    let reopened_reader = reopened_tantivy.reader()?;
    let tantivy_open_us = tantivy_open_started.elapsed().as_micros();
    let tantivy_first_started = Instant::now();
    let _ = tantivy_search(
        &reopened_tantivy,
        &reopened_reader,
        reopened_fields,
        "quartz",
    );
    let tantivy_first_query_us = tantivy_first_started.elapsed().as_micros();
    let mut workload_metrics = Vec::new();
    for (name, workload) in benchmark_workloads() {
        let mut workload_queries = Vec::new();
        for _ in 0..query_repetitions.max(10) {
            workload_queries.extend(workload.iter().cloned());
        }
        let sqlite_stats = run_queries(&workload_queries, |query| {
            let _ = sqlite_search(&sqlite, query);
        });
        let tantivy_stats = run_queries(&workload_queries, |query| {
            let _ = tantivy_search(&tantivy, &reader, fields, query);
        });
        workload_metrics.push((name, sqlite_stats, tantivy_stats));
    }
    let hot_sqlite_path = out.join("sqlite-hot-fts.db");
    let mut hot_sqlite = create_sqlite_fts(&hot_sqlite_path)?;
    // The generator advances roughly one message per third of a day: 270
    // messages approximate the latest 90 days.
    let hot_count = config.messages.min(270);
    let hot_start = config.messages.saturating_sub(hot_count);
    index_sqlite_archive(&mut hot_sqlite, out, hot_start, config.messages)?;
    let hot_sqlite_latency = run_queries(&queries, |query| {
        let _ = sqlite_search(&hot_sqlite, query);
    });
    let hot_tantivy_path = out.join("tantivy-hot");
    let (hot_tantivy, hot_fields) = create_tantivy(&hot_tantivy_path)?;
    index_tantivy_archive(&hot_tantivy, hot_fields, out, hot_start, config.messages)?;
    let hot_reader = hot_tantivy.reader()?;
    let hot_tantivy_latency = run_queries(&queries, |query| {
        let _ = tantivy_search(&hot_tantivy, &hot_reader, hot_fields, query);
    });
    let date_start = 1_577_836_800i64;
    let date_end = date_start + 90 * 86_400;
    let sqlite_date_hits = sqlite_date_count(&sqlite, date_start, date_end)?;
    let tantivy_date_hits = tantivy_date_search(&reader, fields, date_start, date_end)?.len();
    let sqlite_text_date_started = Instant::now();
    let _ = sqlite_text_date_search(&sqlite, "archive", date_start, date_end)?;
    let sqlite_text_date_us = sqlite_text_date_started.elapsed().as_micros();
    let tantivy_text_date_started = Instant::now();
    let _ = tantivy_text_date_search(&tantivy, &reader, fields, "archive", date_start, date_end)?;
    let tantivy_text_date_us = tantivy_text_date_started.elapsed().as_micros();
    let sqlite_bytes = fs::metadata(&sqlite_path)?.len();
    let hot_sqlite_bytes = fs::metadata(&hot_sqlite_path)?.len();
    let tantivy_bytes = directory_bytes(&tantivy_path)?;
    let hot_tantivy_bytes = directory_bytes(&hot_tantivy_path)?;
    write_manifest(&out.join("manifest.txt"), config, &stats)?;
    println!("command=benchmark\nmessages={}\nseed={}\narchive_bytes={}\nraw_bytes={}\ncompressed_bytes={}\nzstd_bytes={}\nmin_bytes={}\nmedian_bytes={}\nmax_bytes={}\nattachments={}\nunique_attachment_hashes={}\nattachment_bytes={}\nunique_attachment_bytes={}\nsegment_bytes={}\nimport_ms={}\narchive_read_us={}\narchive_parse_us={}\nsqlite_index_ms={}\ntantivy_index_ms={}\nsqlite_pipeline_read_us={}\nsqlite_pipeline_parse_us={}\ntantivy_pipeline_read_us={}\ntantivy_pipeline_parse_us={}\nsqlite_open_us={}\nsqlite_first_query_us={}\ntantivy_open_us={}\ntantivy_first_query_us={}\nsqlite_text_date_us={}\ntantivy_text_date_us={}\nsqlite_bytes={}\ntantivy_bytes={}\nsqlite_hot_bytes={}\ntantivy_hot_bytes={}\nsqlite_date_hits={}\ntantivy_date_hits={}\nsqlite_p50_us={}\nsqlite_p95_us={}\nsqlite_p99_us={}\nsqlite_max_us={}\ntantivy_p50_us={}\ntantivy_p95_us={}\ntantivy_p99_us={}\ntantivy_max_us={}\nsqlite_hot_p50_us={}\nsqlite_hot_p95_us={}\nsqlite_hot_p99_us={}\ntantivy_hot_p50_us={}\ntantivy_hot_p95_us={}\ntantivy_hot_p99_us={}\nquery_count={}", stats.messages, config.seed, archive_bytes, stats.bytes, stats.compressed_bytes, stats.zstd_bytes, stats.min_bytes, stats.median_bytes, stats.max_bytes, stats.attachments, stats.unique_attachment_hashes, stats.attachment_bytes, stats.unique_attachment_bytes, segment_bytes, import_ms, sqlite_pipeline.read_us + tantivy_pipeline.read_us, sqlite_pipeline.parse_us + tantivy_pipeline.parse_us, sqlite_index_ms, tantivy_index_ms, sqlite_pipeline.read_us, sqlite_pipeline.parse_us, tantivy_pipeline.read_us, tantivy_pipeline.parse_us, sqlite_open_us, sqlite_first_query_us, tantivy_open_us, tantivy_first_query_us, sqlite_text_date_us, tantivy_text_date_us, sqlite_bytes, tantivy_bytes, hot_sqlite_bytes, hot_tantivy_bytes, sqlite_date_hits, tantivy_date_hits, sqlite_latency.p50_us, sqlite_latency.p95_us, sqlite_latency.p99_us, sqlite_latency.max_us, tantivy_latency.p50_us, tantivy_latency.p95_us, tantivy_latency.p99_us, tantivy_latency.max_us, hot_sqlite_latency.p50_us, hot_sqlite_latency.p95_us, hot_sqlite_latency.p99_us, hot_tantivy_latency.p50_us, hot_tantivy_latency.p95_us, hot_tantivy_latency.p99_us, queries.len());
    print_stats(
        "benchmark-corpus",
        config,
        &stats,
        archive_bytes,
        segment_bytes,
        import_ms,
    );
    for (name, sqlite_stats, tantivy_stats) in workload_metrics {
        println!("workload_{}_sqlite_p50_us={}\nworkload_{}_sqlite_p95_us={}\nworkload_{}_sqlite_p99_us={}\nworkload_{}_tantivy_p50_us={}\nworkload_{}_tantivy_p95_us={}\nworkload_{}_tantivy_p99_us={}", name, sqlite_stats.p50_us, name, sqlite_stats.p95_us, name, sqlite_stats.p99_us, name, tantivy_stats.p50_us, name, tantivy_stats.p95_us, name, tantivy_stats.p99_us);
    }
    Ok(())
}

fn help() {
    println!("mail-archive-experiment\n\ncommands: generate | benchmark | cas-benchmark | gmail-sync | recover-gmail-raw | gmail-report | archive-inventory | recovery-plan | gmail-index | search\noptions: --messages N --seed N --profile light|personal|heavy --queries N --segment-bytes N --attachment-rate P --duplicate-rate P --max-attachment-bytes N --compression --out PATH\ngmail-sync: --archive PATH --credentials PATH --token-dir PATH [--account KEY] [--max-messages N] [--query GMAIL_QUERY]\nrecover-gmail-raw: --archive PATH --doc-id N --credentials PATH [--token-dir PATH] (one explicit exact recovery)\ngmail-report: --archive PATH (offline aggregate MIME/checksum report)\narchive-inventory: --archive PATH (read-only physical/catalogue reconciliation)\nrecovery-plan: --archive PATH (strictly read-only Tier A recovery policy plan)\ngmail-index: --archive PATH (offline Tantivy build and aggregate workload)\nsearch: --archive PATH QUERY (local Tantivy search)\nGmail sync and recovery requests only gmail.readonly and never write to Gmail.");
}
