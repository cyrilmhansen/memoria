use mailparse::{parse_mail, ParsedMail};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::env;
use std::fs;
use std::path::{Path, PathBuf};

const FIXTURES: &str = "fixtures";
const ORACLES: &str = "oracles";

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
struct Recipient {
    original_recipient: Option<String>,
    final_recipient: Option<String>,
    action: Option<String>,
    status: Option<String>,
    diagnostic_code: Option<String>,
    extra_fields: BTreeMap<String, String>,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
struct Oracle {
    case: String,
    kind: String,
    validity: String,
    original_message_id: Option<String>,
    recipients: Vec<Recipient>,
    action: Option<String>,
    message_fields: BTreeMap<String, String>,
}

struct Spec {
    oracle: Oracle,
    eml: String,
}

fn main() {
    let root = PathBuf::from(env::args().nth(1).unwrap_or_else(|| ".".into()));
    let command = env::args().nth(2).unwrap_or_else(|| "--check".into());
    let result = match command.as_str() {
        "--generate" => generate(&root),
        "--check" => check(&root),
        "--dump-mailparse" => dump_mailparse(&root),
        _ => Err("usage: probe ROOT [--generate|--check|--dump-mailparse]".into()),
    };
    if let Err(error) = result {
        eprintln!("error: {error}");
        std::process::exit(1);
    }
}

fn report_message(
    id: &str,
    report_type: &str,
    second_type: &str,
    second_body: &str,
    third: Option<(&str, String)>,
    top_extra: &str,
) -> String {
    let boundary = format!("report-{id}");
    let third_part = third
        .map(|(mime, body)| {
            format!(
                "--{boundary}\r\nContent-Type: {mime}\r\nContent-Transfer-Encoding: 7bit\r\n\r\n{body}\r\n"
            )
        })
        .unwrap_or_default();
    format!(
        "From: notifier@example.invalid\r\nTo: sender@example.invalid\r\nSubject: Synthetic {id}\r\nMessage-ID: <{id}@example.invalid>\r\nMIME-Version: 1.0\r\nContent-Type: multipart/report; report-type={report_type}; boundary=\"{boundary}\"\r\n{top_extra}\r\n--{boundary}\r\nContent-Type: text/plain; charset=utf-8\r\nContent-Transfer-Encoding: 7bit\r\n\r\nSynthetic report {id}.\r\n--{boundary}\r\nContent-Type: {second_type}\r\nContent-Transfer-Encoding: 7bit\r\n\r\n{second_body}\r\n{third_part}--{boundary}--\r\n"
    )
}

fn mdn_body(
    final_recipient: &str,
    original_recipient: Option<&str>,
    original_message_id: Option<&str>,
    action: &str,
    error: bool,
    folded: bool,
    extension: bool,
) -> String {
    let mut fields = String::from("Reporting-UA: Atlas MUA; corpus-probe\r\n");
    if let Some(value) = original_recipient {
        if folded {
            fields.push_str(&format!("Original-Recipient: rfc822;\r\n {value}\r\n"));
        } else {
            fields.push_str(&format!("Original-Recipient: rfc822; {value}\r\n"));
        }
    }
    if let Some(value) = original_message_id {
        fields.push_str(&format!("Original-Message-ID: {value}\r\n"));
    }
    if folded {
        fields.push_str(&format!(
            "Final-Recipient: rfc822;\r\n {final_recipient}\r\n"
        ));
    } else {
        fields.push_str(&format!("Final-Recipient: rfc822; {final_recipient}\r\n"));
    }
    fields.push_str(&format!("Disposition: {action}\r\n"));
    if error {
        fields.push_str("Error: synthetic modifier error\r\n");
    }
    if extension {
        fields.push_str("X-Corpus-Extension: retained for probe\r\n");
    }
    fields
}

fn dsn_body_with_message_fields(
    recipients: &[Recipient],
    folded: bool,
    _extension: bool,
    message_fields: &BTreeMap<String, String>,
) -> String {
    let mut result = String::new();
    for (name, value) in message_fields {
        let display_name = match name.as_str() {
            "reporting-mta" => "Reporting-MTA",
            "arrival-date" => "Arrival-Date",
            "original-envelope-id" => "Original-Envelope-Id",
            "last-attempt-date" => "Last-Attempt-Date",
            "will-retry-until" => "Will-Retry-Until",
            "final-log-id" => "Final-Log-ID",
            "received-from-mta" => "Received-From-MTA",
            "dsn-gateway" => "DSN-Gateway",
            "x-corpus-message" => "X-Corpus-Message",
            other => other,
        };
        result.push_str(&format!("{display_name}: {value}\r\n"));
    }
    for recipient in recipients {
        result.push_str("\r\n");
        if let Some(value) = &recipient.original_recipient {
            result.push_str(&format!("Original-Recipient: rfc822; {value}\r\n"));
        }
        if folded {
            result.push_str(&format!(
                "Final-Recipient: rfc822;\r\n {}\r\n",
                recipient.final_recipient.as_deref().unwrap_or("")
            ));
        } else {
            result.push_str(&format!(
                "Final-Recipient: rfc822; {}\r\n",
                recipient.final_recipient.as_deref().unwrap_or("")
            ));
        }
        result.push_str(&format!(
            "Action: {}\r\n",
            recipient.action.as_deref().unwrap_or("failed")
        ));
        result.push_str(&format!(
            "Status: {}\r\n",
            recipient.status.as_deref().unwrap_or("5.0.0")
        ));
        if let Some(value) = &recipient.diagnostic_code {
            result.push_str(&format!("Diagnostic-Code: smtp; {value}\r\n"));
        }
        for (name, value) in &recipient.extra_fields {
            let display_name = match name.as_str() {
                "last-attempt-date" => "Last-Attempt-Date",
                "final-log-id" => "Final-Log-ID",
                "will-retry-until" => "Will-Retry-Until",
                other => other,
            };
            result.push_str(&format!("{display_name}: {value}\r\n"));
        }
        if recipient.action.as_deref() != Some("delivered") {
            result.push_str("Remote-MTA: dns; remote.example.invalid\r\n");
        }
    }
    result
}

fn mdn_spec(
    name: &str,
    disposition: &str,
    recipient: &str,
    original_recipient: Option<&str>,
    original_id: Option<&str>,
    third: Option<(&str, String)>,
    error: bool,
    folded: bool,
    extension: bool,
) -> Spec {
    let body = mdn_body(
        recipient,
        original_recipient,
        original_id,
        disposition,
        error,
        folded,
        extension,
    );
    let eml = report_message(
        name,
        "disposition-notification",
        "message/disposition-notification",
        &body,
        third.clone(),
        "",
    );
    Spec {
        oracle: Oracle {
            case: name.into(),
            kind: "mdn".into(),
            validity: "valid".into(),
            original_message_id: original_id.map(str::to_owned),
            recipients: vec![Recipient {
                original_recipient: original_recipient.map(str::to_owned),
                final_recipient: Some(recipient.into()),
                action: Some(disposition.into()),
                status: None,
                diagnostic_code: None,
                extra_fields: BTreeMap::new(),
            }],
            action: Some(disposition.into()),
            message_fields: BTreeMap::new(),
        },
        eml,
    }
}

fn dsn_spec_with_message_fields(
    name: &str,
    recipients: Vec<Recipient>,
    third: Option<(&str, String)>,
    folded: bool,
    extension: bool,
    message_fields: BTreeMap<String, String>,
) -> Spec {
    let body = dsn_body_with_message_fields(&recipients, folded, extension, &message_fields);
    let eml = report_message(
        name,
        "delivery-status",
        "message/delivery-status",
        &body,
        third.clone(),
        "",
    );
    let original_message_id = third.and_then(|(_, body)| {
        body.lines()
            .find_map(|line| line.strip_prefix("Message-ID: ").map(str::to_owned))
    });
    Spec {
        oracle: Oracle {
            case: name.into(),
            kind: "dsn".into(),
            validity: "valid".into(),
            original_message_id,
            recipients,
            action: None,
            message_fields,
        },
        eml,
    }
}

fn dsn_spec(
    name: &str,
    recipients: Vec<Recipient>,
    third: Option<(&str, String)>,
    folded: bool,
    extension: bool,
) -> Spec {
    let mut fields = vec![
        ("Reporting-MTA", "dns; mta.example.invalid"),
        ("Arrival-Date", "Thu, 21 Aug 2026 10:00:00 +0000"),
    ];
    if extension {
        fields.push(("X-Corpus-Message", "deterministic"));
    }
    dsn_spec_with_message_fields(
        name,
        recipients,
        third,
        folded,
        extension,
        message_fields(&fields),
    )
}

fn message_fields(entries: &[(&str, &str)]) -> BTreeMap<String, String> {
    entries
        .iter()
        .map(|(name, value)| (name.to_ascii_lowercase(), (*value).into()))
        .collect()
}

fn ordinary_spec(name: &str, subject: &str, body: &str) -> Spec {
    Spec {
        oracle: Oracle {
            case: name.into(),
            kind: "ordinary".into(),
            validity: "valid".into(),
            original_message_id: None,
            recipients: Vec::new(),
            action: None,
            message_fields: BTreeMap::new(),
        },
        eml: format!(
            "From: friend@example.invalid\r\nTo: reader@example.invalid\r\nSubject: {subject}\r\nMessage-ID: <{name}@example.invalid>\r\nMIME-Version: 1.0\r\nContent-Type: text/plain; charset=utf-8\r\n\r\n{body}\r\n"
        ),
    }
}

fn invalid_spec(name: &str, kind: &str, eml: String, validity: &str) -> Spec {
    Spec {
        oracle: Oracle {
            case: name.into(),
            kind: kind.into(),
            validity: validity.into(),
            original_message_id: None,
            recipients: Vec::new(),
            action: None,
            message_fields: BTreeMap::new(),
        },
        eml,
    }
}

fn all_specs() -> Vec<Spec> {
    let original = |name: &str| format!("<{name}-original@example.invalid>");
    let full_message = |name: &str| {
        (
            "message/rfc822",
            format!(
                "From: sender@example.invalid\r\nTo: recipient@example.invalid\r\nMessage-ID: {}\r\nSubject: original {name}\r\n\r\nOriginal body.\r\n",
                original(name)
            ),
        )
    };
    let headers_only = |name: &str| {
        (
            "text/rfc822-headers",
            format!(
                "From: sender@example.invalid\r\nMessage-ID: {}\r\nSubject: headers {name}\r\n",
                original(name)
            ),
        )
    };
    let mut specs = Vec::new();

    let mdn_inputs = [
        (
            "mdn-01-displayed-auto-full",
            "automatic-action/MDN-sent-automatically; displayed",
            true,
            true,
            false,
            false,
            false,
        ),
        (
            "mdn-02-deleted-manual-none",
            "manual-action/MDN-sent-manually; deleted",
            false,
            true,
            false,
            false,
            false,
        ),
        (
            "mdn-03-processed-auto-headers",
            "automatic-action/MDN-sent-automatically; processed",
            true,
            true,
            false,
            false,
            false,
        ),
        (
            "mdn-04-dispatched-manual-headers",
            "manual-action/MDN-sent-manually; dispatched",
            true,
            false,
            false,
            false,
            false,
        ),
        (
            "mdn-05-displayed-manual-error",
            "manual-action/MDN-sent-manually; displayed/error",
            true,
            true,
            true,
            false,
            false,
        ),
        (
            "mdn-06-deleted-auto-full",
            "automatic-action/MDN-sent-automatically; deleted",
            true,
            true,
            false,
            false,
            false,
        ),
        (
            "mdn-07-folded-fields",
            "manual-action/MDN-sent-manually; displayed",
            true,
            true,
            false,
            true,
            false,
        ),
        (
            "mdn-08-no-original-recipient",
            "automatic-action/MDN-sent-automatically; processed",
            false,
            true,
            false,
            false,
            false,
        ),
        (
            "mdn-09-no-third-part",
            "manual-action/MDN-sent-manually; deleted",
            true,
            false,
            false,
            false,
            false,
        ),
        (
            "mdn-10-extension-field",
            "automatic-action/MDN-sent-automatically; dispatched",
            true,
            true,
            false,
            false,
            true,
        ),
        (
            "mdn-11-casing-variation",
            "AUTOMATIC-ACTION/MDN-SENT-AUTOMATICALLY; DISPLAYED",
            true,
            true,
            false,
            false,
            false,
        ),
        (
            "mdn-12-processed-full",
            "manual-action/MDN-sent-manually; processed",
            true,
            true,
            false,
            false,
            false,
        ),
    ];
    for (name, disposition, has_original_recipient, has_third, error, folded, extension) in
        mdn_inputs
    {
        let orig_recipient = has_original_recipient.then_some("recipient@example.invalid");
        let original_id = if name == "mdn-09-no-third-part" {
            None
        } else {
            Some(original(name))
        };
        let third = if has_third {
            Some(full_message(name))
        } else if name == "mdn-03-processed-auto-headers"
            || name == "mdn-04-dispatched-manual-headers"
        {
            Some(headers_only(name))
        } else {
            None
        };
        specs.push(mdn_spec(
            name,
            disposition,
            "recipient@example.invalid",
            orig_recipient,
            original_id.as_deref(),
            third,
            error,
            folded,
            extension,
        ));
    }
    specs.push(mdn_spec(
        "mdn-13-original-id-without-third",
        "automatic-action/MDN-sent-automatically; displayed",
        "recipient@example.invalid",
        Some("recipient@example.invalid"),
        Some("<mdn-13-original@example.invalid>"),
        None,
        false,
        false,
        false,
    ));
    let no_id_original = (
        "message/rfc822",
        "From: sender@example.invalid\r\nTo: recipient@example.invalid\r\nSubject: no message id\r\n\r\nOriginal body without Message-ID.\r\n".to_string(),
    );
    specs.push(mdn_spec(
        "mdn-14-third-without-original-id",
        "manual-action/MDN-sent-manually; processed",
        "recipient@example.invalid",
        None,
        None,
        Some(no_id_original),
        false,
        false,
        false,
    ));

    let recipient = |address: &str,
                     action: &str,
                     status: &str,
                     diagnostic: Option<&str>,
                     original_recipient: bool|
     -> Recipient {
        Recipient {
            original_recipient: original_recipient.then(|| format!("orig-{address}")),
            final_recipient: Some(address.into()),
            action: Some(action.into()),
            status: Some(status.into()),
            diagnostic_code: diagnostic.map(str::to_owned),
            extra_fields: BTreeMap::new(),
        }
    };
    let recipient_with_extra =
        |address: &str, action: &str, status: &str, extra: &[(&str, &str)]| {
            let mut result = recipient(address, action, status, None, false);
            result.extra_fields = message_fields(extra);
            result
        };
    specs.push(dsn_spec(
        "dsn-01-failed-one",
        vec![recipient(
            "failed@example.invalid",
            "failed",
            "5.1.1",
            Some("550 5.1.1 unknown"),
            true,
        )],
        Some(full_message("dsn-01-failed-one")),
        false,
        false,
    ));
    specs.push(dsn_spec(
        "dsn-02-delayed-one",
        vec![recipient(
            "delayed@example.invalid",
            "delayed",
            "4.2.0",
            Some("451 4.2.0 temporary"),
            false,
        )],
        None,
        false,
        false,
    ));
    specs.push(dsn_spec(
        "dsn-03-delivered-one",
        vec![recipient(
            "delivered@example.invalid",
            "delivered",
            "2.0.0",
            None,
            true,
        )],
        Some(headers_only("dsn-03-delivered-one")),
        false,
        false,
    ));
    specs.push(dsn_spec(
        "dsn-04-two-recipients",
        vec![
            recipient(
                "failed@example.invalid",
                "failed",
                "5.1.1",
                Some("550 rejected"),
                true,
            ),
            recipient(
                "delivered@example.invalid",
                "delivered",
                "2.0.0",
                None,
                false,
            ),
        ],
        None,
        false,
        false,
    ));
    specs.push(dsn_spec(
        "dsn-05-three-results",
        vec![
            recipient(
                "one@example.invalid",
                "failed",
                "5.2.0",
                Some("550 mailbox"),
                false,
            ),
            recipient(
                "two@example.invalid",
                "delayed",
                "4.4.1",
                Some("451 retry"),
                true,
            ),
            recipient("three@example.invalid", "delivered", "2.0.0", None, false),
        ],
        None,
        false,
        false,
    ));
    specs.push(dsn_spec(
        "dsn-06-remote-mta",
        vec![recipient(
            "remote@example.invalid",
            "failed",
            "5.7.1",
            Some("550 policy"),
            true,
        )],
        Some(full_message("dsn-06-remote-mta")),
        false,
        false,
    ));
    specs.push(dsn_spec(
        "dsn-07-folded-fields",
        vec![recipient(
            "folded@example.invalid",
            "failed",
            "5.1.1",
            Some("550 folded"),
            true,
        )],
        None,
        true,
        false,
    ));
    specs.push(dsn_spec(
        "dsn-08-no-original-recipient",
        vec![recipient(
            "final@example.invalid",
            "delivered",
            "2.0.0",
            None,
            false,
        )],
        None,
        false,
        false,
    ));
    specs.push(dsn_spec(
        "dsn-09-message-headers",
        vec![recipient(
            "headers@example.invalid",
            "failed",
            "5.0.0",
            Some("550 headers"),
            false,
        )],
        Some(headers_only("dsn-09-message-headers")),
        false,
        false,
    ));
    specs.push(dsn_spec(
        "dsn-10-full-original",
        vec![recipient(
            "full@example.invalid",
            "failed",
            "5.1.1",
            Some("550 full"),
            true,
        )],
        Some(full_message("dsn-10-full-original")),
        false,
        false,
    ));
    specs.push(dsn_spec(
        "dsn-11-extension",
        vec![recipient(
            "extension@example.invalid",
            "delayed",
            "4.3.0",
            Some("451 extension"),
            false,
        )],
        None,
        false,
        true,
    ));
    specs.push(dsn_spec(
        "dsn-12-case-and-folding",
        vec![recipient(
            "case@example.invalid",
            "FAILED",
            "5.1.1",
            Some("550 case"),
            true,
        )],
        None,
        true,
        false,
    ));
    specs.push(dsn_spec_with_message_fields(
        "dsn-13-relayed-expanded-fields",
        vec![
            recipient_with_extra(
                "relayed@example.invalid",
                "relayed",
                "2.0.0",
                &[("Last-Attempt-Date", "Thu, 21 Aug 2026 10:05:00 +0000")],
            ),
            recipient_with_extra(
                "expanded@example.invalid",
                "expanded",
                "2.0.0",
                &[("Final-Log-ID", "log-947@example.invalid")],
            ),
            recipient_with_extra(
                "retry@example.invalid",
                "delayed",
                "4.2.0",
                &[
                    ("Last-Attempt-Date", "Thu, 21 Aug 2026 10:05:00 +0000"),
                    ("Will-Retry-Until", "Thu, 21 Aug 2026 11:00:00 +0000"),
                ],
            ),
        ],
        None,
        false,
        false,
        message_fields(&[
            ("Reporting-MTA", "dns; mta.example.invalid"),
            ("Original-Envelope-Id", "env-947@example.invalid"),
            ("Arrival-Date", "Thu, 21 Aug 2026 10:00:00 +0000"),
            ("Received-From-MTA", "dns; relay.example.invalid"),
            ("DSN-Gateway", "dns; gateway.example.invalid"),
        ]),
    ));
    let missing_reporting_mta = report_message(
        "dsn-14-missing-reporting-mta",
        "delivery-status",
        "message/delivery-status",
        "Arrival-Date: Thu, 21 Aug 2026 10:00:00 +0000\r\n\r\nFinal-Recipient: rfc822; missing-mta@example.invalid\r\nAction: failed\r\nStatus: 5.1.1\r\n",
        None,
        "",
    );
    specs.push(invalid_spec(
        "dsn-14-missing-reporting-mta",
        "dsn",
        missing_reporting_mta,
        "malformed",
    ));

    specs.extend([
        ordinary_spec(
            "negative-01-delivery-subject",
            "Delivery Status Notification",
            "This is an ordinary message, not a report.",
        ),
        ordinary_spec(
            "negative-02-undelivered-body",
            "Status update",
            "Undelivered mail is mentioned as ordinary prose.",
        ),
        ordinary_spec(
            "negative-03-read-receipt",
            "Read receipt",
            "Your message has been displayed in a quoted discussion.",
        ),
        ordinary_spec(
            "negative-04-displayed-text",
            "Daily note",
            "Your message has been displayed, according to this sentence.",
        ),
    ]);
    let wrong_report = report_message(
        "negative-05-wrong-report-type",
        "unknown-report",
        "message/disposition-notification",
        &mdn_body(
            "recipient@example.invalid",
            Some("recipient@example.invalid"),
            None,
            "automatic-action/MDN-sent-automatically; displayed",
            false,
            false,
            false,
        ),
        None,
        "",
    );
    specs.push(invalid_spec(
        "negative-05-wrong-report-type",
        "mdn",
        wrong_report,
        "unsupported",
    ));
    let incoherent = "From: notifier@example.invalid\r\nContent-Type: multipart/mixed; boundary=x\r\n\r\n--x\r\nContent-Type: message/delivery-status\r\n\r\nReporting-MTA: dns; mta.example.invalid\r\n\r\n--x--\r\n".into();
    specs.push(invalid_spec(
        "negative-06-incoherent-status",
        "dsn",
        incoherent,
        "malformed",
    ));
    let isolated = "From: notifier@example.invalid\r\nContent-Type: message/disposition-notification\r\n\r\nFinal-Recipient: rfc822; recipient@example.invalid\r\nDisposition: manual-action/MDN-sent-manually; displayed\r\n".into();
    specs.push(invalid_spec(
        "negative-07-isolated-mdn",
        "mdn",
        isolated,
        "malformed",
    ));
    let truncated = "From: notifier@example.invalid\r\nContent-Type: multipart/report; report-type=delivery-status; boundary=broken\r\n\r\n--broken\r\nContent-Type: text/plain\r\n\r\npartial".into();
    specs.push(invalid_spec(
        "negative-08-truncated-report",
        "dsn",
        truncated,
        "malformed",
    ));
    let broken_boundary = "From: notifier@example.invalid\r\nContent-Type: multipart/report; report-type=disposition-notification; boundary=broken\r\n\r\n--other\r\nContent-Type: text/plain\r\n\r\nwrong boundary\r\n--other--\r\n".into();
    specs.push(invalid_spec(
        "negative-09-broken-boundary",
        "mdn",
        broken_boundary,
        "malformed",
    ));
    let missing_required = report_message("negative-10-missing-required", "delivery-status", "message/delivery-status", "Reporting-MTA: dns; mta.example.invalid\r\n\r\nFinal-Recipient: rfc822; missing-action@example.invalid\r\n", None, "");
    specs.push(invalid_spec(
        "negative-10-missing-required",
        "dsn",
        missing_required,
        "malformed",
    ));

    let intl_dsn_body = "Reporting-MTA: dns; mta.example.invalid\r\n\r\nOriginal-Recipient: utf-8; unitext-\\x{00E9}@example.invalid\r\nFinal-Recipient: utf-8; destinataire-\\x{00E9}@example.invalid\r\nAction: failed\r\nStatus: 5.1.1\r\nDiagnostic-Code: smtp; bo\\x{00EE}te inconnue\r\n";
    specs.push(invalid_spec(
        "rfc6533-01-global-dsn",
        "dsn",
        report_message(
            "rfc6533-01-global-dsn",
            "delivery-status",
            "message/global-delivery-status",
            intl_dsn_body,
            None,
            "",
        ),
        "unsupported",
    ));
    specs.push(invalid_spec(
        "rfc6533-02-global-dsn-unitext",
        "dsn",
        report_message(
            "rfc6533-02-global-dsn-unitext",
            "delivery-status",
            "message/global-delivery-status",
            intl_dsn_body,
            Some((
                "message/global-headers",
                "Message-ID: <rfc6533-02-original@example.invalid>\r\nSubject: UTF-8\r\n"
                    .to_string(),
            )),
            "",
        ),
        "unsupported",
    ));
    let intl_mdn_body = mdn_body(
        "destinataire@example.invalid",
        Some("destinataire@example.invalid"),
        Some("<rfc6533-03-original@example.invalid>"),
        "automatic-action/MDN-sent-automatically; displayed",
        false,
        false,
        false,
    );
    specs.push(invalid_spec(
        "rfc6533-03-global-mdn",
        "mdn",
        report_message(
            "rfc6533-03-global-mdn",
            "disposition-notification",
            "message/global-disposition-notification",
            &intl_mdn_body,
            None,
            "",
        ),
        "unsupported",
    ));
    specs.push(invalid_spec("rfc6533-04-global-mdn-headers", "mdn", report_message("rfc6533-04-global-mdn-headers", "disposition-notification", "message/global-disposition-notification", &intl_mdn_body, Some(("message/global", "From: sender@example.invalid\r\nMessage-ID: <rfc6533-04-original@example.invalid>\r\n\r\nUTF-8 body\r\n".to_string())), ""), "unsupported"));
    specs
}

fn generate(root: &Path) -> Result<(), String> {
    let fixtures = root.join(FIXTURES);
    let oracles = root.join(ORACLES);
    fs::create_dir_all(&fixtures).map_err(|e| e.to_string())?;
    fs::create_dir_all(&oracles).map_err(|e| e.to_string())?;
    for spec in all_specs() {
        fs::write(fixtures.join(format!("{}.eml", spec.oracle.case)), spec.eml)
            .map_err(|e| e.to_string())?;
        fs::write(
            oracles.join(format!("{}.expected.json", spec.oracle.case)),
            serde_json::to_string_pretty(&spec.oracle).map_err(|e| e.to_string())? + "\n",
        )
        .map_err(|e| e.to_string())?;
    }
    println!("generated={} fixtures", all_specs().len());
    Ok(())
}

fn check(root: &Path) -> Result<(), String> {
    let specs = all_specs();
    let mut observed = 0;
    for spec in specs.iter() {
        let fixture = fs::read(
            root.join(FIXTURES)
                .join(format!("{}.eml", spec.oracle.case)),
        )
        .map_err(|e| format!("{} fixture: {e}", spec.oracle.case))?;
        let oracle: Oracle = serde_json::from_slice(
            &fs::read(
                root.join(ORACLES)
                    .join(format!("{}.expected.json", spec.oracle.case)),
            )
            .map_err(|e| format!("{} oracle: {e}", spec.oracle.case))?,
        )
        .map_err(|e| format!("{} oracle JSON: {e}", spec.oracle.case))?;
        validate_against_oracle(&fixture, &oracle)?;
        observed += 1;
    }
    let counts = specs
        .iter()
        .fold(BTreeMap::<String, usize>::new(), |mut map, spec| {
            *map.entry(format!("{} {}", spec.oracle.kind, spec.oracle.validity))
                .or_default() += 1;
            map
        });
    println!("checked={observed} counts={counts:?}");
    Ok(())
}

fn validate_against_oracle(raw: &[u8], oracle: &Oracle) -> Result<(), String> {
    let result = classify(raw)?;
    if result.kind != oracle.kind || result.validity != oracle.validity {
        return Err(format!(
            "{} mismatch: expected {}/{} observed {}/{}",
            oracle.case, oracle.kind, oracle.validity, result.kind, result.validity
        ));
    }
    if oracle.validity == "valid"
        && (result.original_message_id != oracle.original_message_id
            || result.recipients != oracle.recipients
            || result.message_fields != oracle.message_fields)
    {
        return Err(format!(
            "{} semantic oracle mismatch: expected id={:?} recipients={:?} fields={:?}, observed id={:?} recipients={:?} fields={:?}",
            oracle.case,
            oracle.original_message_id,
            oracle.recipients,
            oracle.message_fields,
            result.original_message_id,
            result.recipients,
            result.message_fields
        ));
    }
    Ok(())
}

#[derive(Debug, PartialEq)]
struct Observed {
    kind: String,
    validity: String,
    original_message_id: Option<String>,
    recipients: Vec<Recipient>,
    message_fields: BTreeMap<String, String>,
}

fn normalized_fields(body: &str) -> Vec<(String, String)> {
    let mut fields: Vec<(String, String)> = Vec::new();
    for line in body.replace("\r\n", "\n").lines() {
        if (line.starts_with(' ') || line.starts_with('\t')) && !fields.is_empty() {
            fields.last_mut().unwrap().1.push_str(line.trim());
        } else if let Some((name, value)) = line.split_once(':') {
            fields.push((name.trim().to_ascii_lowercase(), value.trim().to_string()));
        }
    }
    fields
}

fn field(fields: &[(String, String)], name: &str) -> Option<String> {
    fields
        .iter()
        .find(|(key, _)| key == name)
        .map(|(_, value)| value.clone())
}

fn status_part(part: &ParsedMail<'_>) -> Result<Vec<Vec<(String, String)>>, String> {
    let body = part.get_body().map_err(|e| format!("status body: {e}"))?;
    Ok(body
        .split("\r\n\r\n")
        .map(normalized_fields)
        .filter(|fields| !fields.is_empty())
        .collect())
}

fn classify(raw: &[u8]) -> Result<Observed, String> {
    let parsed = match parse_mail(raw) {
        Ok(parsed) => parsed,
        Err(_) => {
            let lower = String::from_utf8_lossy(raw).to_ascii_lowercase();
            let kind = if lower.contains("disposition-notification") {
                "mdn"
            } else if lower.contains("delivery-status") {
                "dsn"
            } else {
                "ordinary"
            };
            return Ok(Observed {
                kind: kind.into(),
                validity: "malformed".into(),
                original_message_id: None,
                recipients: Vec::new(),
                message_fields: BTreeMap::new(),
            });
        }
    };
    let top = parsed.ctype.mimetype.to_ascii_lowercase();
    if top != "multipart/report" {
        let kind = if top == "message/disposition-notification"
            || parsed.subparts.iter().any(|part| {
                part.ctype
                    .mimetype
                    .eq_ignore_ascii_case("message/disposition-notification")
            }) {
            "mdn"
        } else if top == "message/delivery-status"
            || parsed.subparts.iter().any(|part| {
                part.ctype
                    .mimetype
                    .eq_ignore_ascii_case("message/delivery-status")
            })
        {
            "dsn"
        } else {
            "ordinary"
        };
        return Ok(Observed {
            kind: kind.into(),
            validity: if kind == "ordinary" {
                "valid"
            } else {
                "malformed"
            }
            .into(),
            original_message_id: None,
            recipients: Vec::new(),
            message_fields: BTreeMap::new(),
        });
    }
    let report_type = parsed
        .ctype
        .params
        .get("report-type")
        .map(|v| v.to_ascii_lowercase());
    let Some(second) = parsed.subparts.get(1) else {
        let kind = match report_type.as_deref() {
            Some("disposition-notification") => "mdn",
            Some("delivery-status") => "dsn",
            _ => "ordinary",
        };
        return Ok(Observed {
            kind: kind.into(),
            validity: "malformed".into(),
            original_message_id: None,
            recipients: Vec::new(),
            message_fields: BTreeMap::new(),
        });
    };
    let second_type = second.ctype.mimetype.to_ascii_lowercase();
    if report_type.as_deref() == Some("disposition-notification")
        && second_type == "message/disposition-notification"
    {
        let fields = normalized_fields(&second.get_body().map_err(|e| e.to_string())?);
        let final_recipient = field(&fields, "final-recipient").map(|v| {
            v.split_once(';')
                .map(|(_, value)| value.trim().to_string())
                .unwrap_or(v)
        });
        let original_recipient = field(&fields, "original-recipient").map(|v| {
            v.split_once(';')
                .map(|(_, value)| value.trim().to_string())
                .unwrap_or(v)
        });
        let action = field(&fields, "disposition");
        let original_message_id = field(&fields, "original-message-id");
        let valid = final_recipient.is_some() && action.is_some();
        return Ok(Observed {
            kind: "mdn".into(),
            validity: if valid { "valid" } else { "malformed" }.into(),
            original_message_id,
            recipients: vec![Recipient {
                original_recipient,
                final_recipient,
                action,
                status: None,
                diagnostic_code: None,
                extra_fields: BTreeMap::new(),
            }],
            message_fields: BTreeMap::new(),
        });
    }
    if report_type.as_deref() == Some("delivery-status") && second_type == "message/delivery-status"
    {
        let groups = status_part(second)?;
        let mut recipients = Vec::new();
        let group_count = groups.len();
        for group in groups.iter().skip(1) {
            let final_recipient = field(&group, "final-recipient").map(|v| {
                v.split_once(';')
                    .map(|(_, value)| value.trim().to_string())
                    .unwrap_or(v)
            });
            let original_recipient = field(&group, "original-recipient").map(|v| {
                v.split_once(';')
                    .map(|(_, value)| value.trim().to_string())
                    .unwrap_or(v)
            });
            recipients.push(Recipient {
                original_recipient,
                final_recipient,
                action: field(&group, "action"),
                status: field(&group, "status"),
                diagnostic_code: field(&group, "diagnostic-code").map(|value| {
                    value
                        .split_once(';')
                        .map(|(_, detail)| detail.trim().to_string())
                        .unwrap_or(value)
                }),
                extra_fields: group
                    .iter()
                    .filter(|(key, _)| {
                        matches!(
                            key.as_str(),
                            "last-attempt-date" | "final-log-id" | "will-retry-until"
                        )
                    })
                    .cloned()
                    .collect(),
            });
        }
        let valid = group_count > 1
            && groups[0].iter().any(|(key, _)| key == "reporting-mta")
            && recipients.iter().all(|r| {
                let retry_allowed = r.action.as_deref() == Some("delayed")
                    || !r.extra_fields.contains_key("will-retry-until");
                r.final_recipient.is_some()
                    && r.action.is_some()
                    && r.status.is_some()
                    && retry_allowed
            });
        let message_fields = groups[0]
            .iter()
            .map(|(key, value)| (key.clone(), value.clone()))
            .collect();
        let original_message_id = parsed.subparts.get(2).and_then(|part| {
            part.get_body().ok().and_then(|body| {
                body.lines()
                    .find_map(|line| line.strip_prefix("Message-ID: ").map(str::to_owned))
            })
        });
        return Ok(Observed {
            kind: "dsn".into(),
            validity: if valid { "valid" } else { "malformed" }.into(),
            original_message_id,
            recipients,
            message_fields,
        });
    }
    let kind = if second_type.contains("disposition") {
        "mdn"
    } else if second_type.contains("delivery-status") {
        "dsn"
    } else {
        "ordinary"
    };
    Ok(Observed {
        kind: kind.into(),
        validity: "unsupported".into(),
        original_message_id: None,
        recipients: Vec::new(),
        message_fields: BTreeMap::new(),
    })
}

fn dump_mailparse(root: &Path) -> Result<(), String> {
    for entry in fs::read_dir(root.join(FIXTURES)).map_err(|e| e.to_string())? {
        let path = entry.map_err(|e| e.to_string())?.path();
        if path.extension().and_then(|e| e.to_str()) != Some("eml") {
            continue;
        }
        let raw = fs::read(&path).map_err(|e| e.to_string())?;
        let parsed = parse_mail(&raw).map_err(|e| format!("{}: {e}", path.display()))?;
        let parts = parsed.subparts.iter().map(|part| serde_json::json!({
            "mime": part.ctype.mimetype,
            "params": part.ctype.params,
            "subparts": part.subparts.len(),
            "decoded_body_bytes": part.get_body().map(|body| body.len()).unwrap_or(0),
            "headers": part.headers.iter().map(|header| header.get_key()).collect::<Vec<_>>()
        })).collect::<Vec<_>>();
        println!(
            "{}",
            serde_json::json!({"file": path.file_name().and_then(|name| name.to_str()).unwrap_or("<non-utf8>"), "top_mime": parsed.ctype.mimetype, "top_params": parsed.ctype.params, "top_subparts": parsed.subparts.len(), "parts": parts})
        );
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn corpus_has_expected_size_and_categories() {
        let specs = all_specs();
        assert_eq!(specs.len(), 42);
        assert_eq!(
            specs
                .iter()
                .filter(|s| s.oracle.kind == "mdn" && s.oracle.validity == "valid")
                .count(),
            14
        );
        assert_eq!(
            specs
                .iter()
                .filter(|s| s.oracle.kind == "dsn" && s.oracle.validity == "valid")
                .count(),
            13
        );
        assert_eq!(
            specs
                .iter()
                .filter(|s| s.oracle.case.starts_with("negative-"))
                .count(),
            10
        );
        assert_eq!(
            specs
                .iter()
                .filter(|s| s.oracle.case.starts_with("rfc6533-"))
                .count(),
            4
        );
    }

    #[test]
    fn mdn_invariant_is_one_recipient() {
        assert!(all_specs()
            .into_iter()
            .filter(|s| s.oracle.kind == "mdn")
            .all(|s| s.oracle.recipients.len() <= 1));
    }

    #[test]
    fn canonical_eml_fixture_preserves_crlf_bytes() {
        const FIXTURE: &[u8] = include_bytes!("../fixtures/mdn-01-displayed-auto-full.eml");
        assert!(FIXTURE.windows(2).any(|pair| pair == b"\r\n"));
        assert!(FIXTURE
            .iter()
            .enumerate()
            .all(|(index, byte)| *byte != b'\n' || (index > 0 && FIXTURE[index - 1] == b'\r')));
    }

    #[test]
    fn mdn_required_fields_and_original_id_rule_are_explicit() {
        for spec in all_specs()
            .into_iter()
            .filter(|spec| spec.oracle.kind == "mdn" && spec.oracle.validity == "valid")
        {
            let recipient = &spec.oracle.recipients[0];
            assert!(recipient.final_recipient.is_some());
            assert!(recipient.action.is_some());
            let third_has_message_id = [
                "Content-Type: message/rfc822",
                "Content-Type: text/rfc822-headers",
            ]
            .iter()
            .filter_map(|content_type| spec.eml.rfind(content_type))
            .any(|offset| spec.eml[offset..].contains("Message-ID:"));
            if spec.oracle.case == "mdn-13-original-id-without-third" {
                assert!(spec.oracle.original_message_id.is_some());
            } else if spec.oracle.case == "mdn-14-third-without-original-id" {
                assert!(!spec.oracle.original_message_id.is_some());
            } else {
                assert_eq!(
                    spec.oracle.original_message_id.is_some(),
                    third_has_message_id,
                    "{}",
                    spec.oracle.case
                );
            }
        }
    }

    fn mutation_must_fail(case: &str, mutate: impl FnOnce(&mut String)) {
        let spec = all_specs()
            .into_iter()
            .find(|spec| spec.oracle.case == case)
            .expect("fixture exists");
        let mut mutated = spec.eml;
        mutate(&mut mutated);
        assert!(
            validate_against_oracle(mutated.as_bytes(), &spec.oracle).is_err(),
            "mutation of {case} unexpectedly matched its oracle"
        );
    }

    #[test]
    fn semantic_checker_rejects_changed_eml_with_unchanged_oracle() {
        mutation_must_fail("mdn-01-displayed-auto-full", |eml| {
            *eml = eml.replacen("; displayed", "; deleted", 1);
        });
        mutation_must_fail("dsn-01-failed-one", |eml| {
            *eml = eml.replacen("Action: failed", "Action: delivered", 1);
        });
        mutation_must_fail("dsn-01-failed-one", |eml| {
            *eml = eml.replace("Final-Recipient: rfc822; failed@example.invalid\r\n", "");
        });
        mutation_must_fail("dsn-01-failed-one", |eml| {
            *eml = eml.replace("Reporting-MTA: dns; mta.example.invalid\r\n", "");
        });
        mutation_must_fail("dsn-01-failed-one", |eml| {
            *eml = eml.replacen("Status: 5.1.1", "Status: 5.2.0", 1);
        });
        mutation_must_fail("dsn-04-two-recipients", |eml| {
            *eml = eml.replacen(
                "Final-Recipient: rfc822; delivered@example.invalid",
                "Final-Recipient: rfc822; changed@example.invalid",
                1,
            );
        });
        mutation_must_fail("dsn-13-relayed-expanded-fields", |eml| {
            *eml = eml.replacen(
                "Action: relayed\r\n",
                "Action: relayed\r\nWill-Retry-Until: Thu, 21 Aug 2026 11:00:00 +0000\r\n",
                1,
            );
        });
    }

    #[test]
    fn dsn_per_message_fields_are_separate_from_recipient_fields() {
        let spec = all_specs()
            .into_iter()
            .find(|spec| spec.oracle.case == "dsn-13-relayed-expanded-fields")
            .unwrap();
        assert_eq!(spec.oracle.recipients.len(), 3);
        assert_eq!(spec.oracle.recipients[0].action.as_deref(), Some("relayed"));
        assert_eq!(
            spec.oracle.recipients[1].action.as_deref(),
            Some("expanded")
        );
        for field in [
            "original-envelope-id",
            "arrival-date",
            "received-from-mta",
            "dsn-gateway",
        ] {
            assert!(spec.oracle.message_fields.contains_key(field), "{field}");
        }
        assert!(spec.oracle.recipients[0]
            .extra_fields
            .contains_key("last-attempt-date"));
        assert!(spec.oracle.recipients[1]
            .extra_fields
            .contains_key("final-log-id"));
        assert!(spec.oracle.recipients[2]
            .extra_fields
            .contains_key("will-retry-until"));
    }
}
