use mail_archive_experiment::i18n::{
    imap_recovery_error_label, imap_source_configuration_mismatch, invalid_doc_id,
    recovery_result_label, Language,
};
use mail_archive_experiment::imap::{imap_source_account, sync_imap_mailboxes, ImapConfig};
use mail_archive_experiment::recovery::recover_missing_imap_raw;
use std::env;
use std::path::PathBuf;
use std::time::Duration;

fn option(args: &[String], name: &str) -> Option<String> {
    args.windows(2)
        .find(|pair| pair[0] == name)
        .map(|pair| pair[1].clone())
}

fn options(args: &[String], name: &str) -> Vec<String> {
    args.windows(2)
        .filter(|pair| pair[0] == name)
        .map(|pair| pair[1].clone())
        .collect()
}

fn main() {
    let args = env::args().collect::<Vec<_>>();
    let required = |name: &str| {
        option(&args, name).unwrap_or_else(|| {
            eprintln!("missing {name}");
            std::process::exit(2);
        })
    };
    let archive = PathBuf::from(required("--archive"));
    let host = required("--host");
    let server_name = option(&args, "--server-name").unwrap_or_else(|| host.clone());
    let port = option(&args, "--port")
        .unwrap_or_else(|| "993".into())
        .parse()
        .unwrap_or_else(|_| {
            eprintln!("invalid --port");
            std::process::exit(2);
        });
    let username = required("--username");
    let password = required("--password");
    let ca_cert = PathBuf::from(required("--ca-cert"));
    let mailboxes = options(&args, "--mailbox");
    let mailbox = mailboxes.first().cloned().unwrap_or_else(|| "INBOX".into());
    let derived_source_account = imap_source_account(&username, &host, port);
    if let Some(configured_source_account) = option(&args, "--source") {
        if configured_source_account != derived_source_account {
            eprintln!("{}", imap_source_configuration_mismatch(Language::system()));
            std::process::exit(2);
        }
    }
    let source_account = derived_source_account;
    let limit = option(&args, "--limit").map(|value| {
        value.parse().unwrap_or_else(|_| {
            eprintln!("invalid --limit");
            std::process::exit(2);
        })
    });
    let timeout_ms = option(&args, "--timeout-ms")
        .unwrap_or_else(|| "10000".into())
        .parse()
        .unwrap_or_else(|_| {
            eprintln!("invalid --timeout-ms");
            std::process::exit(2);
        });
    let config = ImapConfig {
        host,
        server_name,
        port,
        username,
        password,
        ca_cert,
        mailbox,
        mailboxes,
        all_mailboxes: args.iter().any(|arg| arg == "--all-mailboxes"),
        source_account,
        limit,
        timeout: Duration::from_millis(timeout_ms),
    };
    if args.iter().any(|arg| arg == "recover-imap-raw") {
        let doc_id = required("--doc-id").parse().unwrap_or_else(|_| {
            eprintln!("{}", invalid_doc_id(Language::system()));
            std::process::exit(2);
        });
        match recover_missing_imap_raw(&archive, doc_id, &config, 64 * 1024 * 1024) {
            Ok(result) => println!("{}", recovery_result_label(Language::system(), &result)),
            Err(error) => {
                eprintln!("{}", imap_recovery_error_label(Language::system(), &error));
                std::process::exit(1);
            }
        }
        return;
    }
    match sync_imap_mailboxes(&archive, &config) {
        Ok(summary) => {
            println!("capabilities={:?}", summary.discovery.capabilities);
            for mailbox in &summary.discovery.mailboxes {
                println!(
                    "mailbox={:?} delimiter={:?} selectable={} attributes={:?} special_use={:?}",
                    mailbox.name,
                    mailbox.delimiter,
                    mailbox.selectable,
                    mailbox.attributes,
                    mailbox.special_use
                );
            }
            let mut failed = false;
            for result in summary.results {
                match result.stats {
                    Some(stats) => {
                        let indexed = stats.index.as_ref().map(|value| value.indexed).unwrap_or(0);
                        println!(
                            "selected_mailbox={:?} examined={} raw_fetched={} new_messages={} network_bytes={} archive_bytes_added={} uidvalidity={} uidnext={} frontier_before={} frontier_after={:?} indexed={indexed}",
                            result.mailbox,
                            stats.examined,
                            stats.raw_fetched,
                            stats.new_messages,
                            stats.network_bytes,
                            stats.archive_bytes_added,
                            stats.uid_validity,
                            stats.uid_next,
                            stats.frontier_before,
                            stats.frontier_after,
                        );
                    }
                    None => {
                        failed = true;
                        eprintln!(
                            "mailbox={:?} error={}",
                            result.mailbox,
                            result.error.unwrap_or_default()
                        )
                    }
                }
            }
            if failed {
                std::process::exit(1);
            }
        }
        Err(error) => {
            eprintln!("error={error}");
            std::process::exit(1);
        }
    }
}
