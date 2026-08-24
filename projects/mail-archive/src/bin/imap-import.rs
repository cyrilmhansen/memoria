use mail_archive_experiment::imap::{sync_imap, ImapConfig};
use std::env;
use std::path::PathBuf;
use std::time::Duration;

fn option(args: &[String], name: &str) -> Option<String> {
    args.windows(2)
        .find(|pair| pair[0] == name)
        .map(|pair| pair[1].clone())
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
    let mailbox = option(&args, "--mailbox").unwrap_or_else(|| "INBOX".into());
    let source_account =
        option(&args, "--source").unwrap_or_else(|| format!("imap:{username}@{host}:{port}"));
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
        source_account,
        limit,
        timeout: Duration::from_millis(timeout_ms),
    };
    match sync_imap(&archive, &config) {
        Ok(stats) => {
            let indexed = stats.index.as_ref().map(|value| value.indexed).unwrap_or(0);
            println!(
                "examined={} raw_fetched={} new_messages={} network_bytes={} archive_bytes_added={} uidvalidity={} uidnext={} frontier_before={} frontier_after={:?} indexed={indexed}",
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
        Err(error) => {
            eprintln!("error={error}");
            std::process::exit(1);
        }
    }
}
