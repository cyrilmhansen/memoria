use mailparse::{parse_mail, ParsedMail};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DeliveryReportKind {
    Mdn,
    Dsn,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum DeliveryReportAnalysis {
    Ordinary,
    Mdn(MdnReport),
    Dsn(DsnReport),
    Malformed {
        kind: DeliveryReportKind,
        reason: String,
    },
    Unsupported {
        kind: DeliveryReportKind,
        reason: String,
    },
    Unparseable {
        reason: String,
    },
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct MdnReport {
    pub reporting_ua: Option<String>,
    pub original_recipient: Option<String>,
    pub final_recipient: String,
    pub original_message_id: Option<String>,
    pub disposition: MdnDisposition,
    pub has_third_part: bool,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct MdnDisposition {
    pub action_mode: String,
    pub sending_mode: String,
    pub disposition_type: String,
    pub modifiers: Vec<String>,
}

#[derive(Clone, Debug, Eq, PartialEq, Default)]
pub struct DsnMessageFields {
    pub original_envelope_id: Option<String>,
    pub reporting_mta: String,
    pub dsn_gateway: Option<String>,
    pub received_from_mta: Option<String>,
    pub arrival_date: Option<String>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DsnRecipient {
    pub original_recipient: Option<String>,
    pub final_recipient: String,
    pub action: String,
    pub status: String,
    pub remote_mta: Option<String>,
    pub diagnostic_code: Option<String>,
    pub last_attempt_date: Option<String>,
    pub final_log_id: Option<String>,
    pub will_retry_until: Option<String>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DsnReport {
    pub message: DsnMessageFields,
    pub recipients: Vec<DsnRecipient>,
    pub original_message_id: Option<String>,
    pub has_third_part: bool,
}

#[derive(Clone, Debug)]
struct Fields(Vec<(String, String)>);

impl Fields {
    fn parse(body: &str) -> Self {
        let mut fields: Vec<(String, String)> = Vec::new();
        for line in body.replace("\r\n", "\n").lines() {
            if (line.starts_with(' ') || line.starts_with('\t')) && !fields.is_empty() {
                fields.last_mut().unwrap().1.push(' ');
                fields.last_mut().unwrap().1.push_str(line.trim());
            } else if let Some((name, value)) = line.split_once(':') {
                fields.push((name.trim().to_ascii_lowercase(), value.trim().to_string()));
            }
        }
        Self(fields)
    }

    fn get(&self, name: &str) -> Option<String> {
        self.0
            .iter()
            .find(|(key, _)| key == name)
            .map(|(_, value)| value.clone())
    }
}

pub fn analyze_delivery_report(raw: &[u8]) -> DeliveryReportAnalysis {
    let parsed = match parse_mail(raw) {
        Ok(parsed) => parsed,
        Err(error) => {
            return DeliveryReportAnalysis::Unparseable {
                reason: format!("MIME parse failed: {error}"),
            };
        }
    };

    if !parsed
        .ctype
        .mimetype
        .eq_ignore_ascii_case("multipart/report")
    {
        if parsed
            .ctype
            .mimetype
            .eq_ignore_ascii_case("message/disposition-notification")
            || has_subpart_type(&parsed, "message/disposition-notification")
        {
            return malformed(DeliveryReportKind::Mdn, "MDN is not multipart/report");
        }
        if parsed
            .ctype
            .mimetype
            .eq_ignore_ascii_case("message/delivery-status")
            || has_subpart_type(&parsed, "message/delivery-status")
        {
            return malformed(DeliveryReportKind::Dsn, "DSN is not multipart/report");
        }
        return DeliveryReportAnalysis::Ordinary;
    }

    let report_type = parsed
        .ctype
        .params
        .get("report-type")
        .map(|value| value.to_ascii_lowercase());
    let Some(second) = parsed.subparts.get(1) else {
        return malformed(
            kind_from_report_type(report_type.as_deref()).unwrap_or(DeliveryReportKind::Dsn),
            "multipart/report has no structured report part",
        );
    };
    let second_type = second.ctype.mimetype.to_ascii_lowercase();

    if second_type == "message/global-delivery-status"
        || second_type == "message/global-disposition-notification"
    {
        return unsupported(
            if second_type.contains("delivery") {
                DeliveryReportKind::Dsn
            } else {
                DeliveryReportKind::Mdn
            },
            "RFC 6533 global report types are not implemented",
        );
    }

    match (report_type.as_deref(), second_type.as_str()) {
        (Some("disposition-notification"), "message/disposition-notification") => {
            analyze_mdn(&parsed, second)
        }
        (Some("delivery-status"), "message/delivery-status") => analyze_dsn(&parsed, second),
        (_, "message/disposition-notification") => unsupported(
            DeliveryReportKind::Mdn,
            "MDN part has an unsupported report-type",
        ),
        (_, "message/delivery-status") => unsupported(
            DeliveryReportKind::Dsn,
            "DSN part has an unsupported report-type",
        ),
        (_, type_name) if type_name.contains("global-") => unsupported(
            kind_from_report_type(report_type.as_deref()).unwrap_or(DeliveryReportKind::Dsn),
            "RFC 6533 global report type is not implemented",
        ),
        _ => malformed(
            kind_from_report_type(report_type.as_deref()).unwrap_or(DeliveryReportKind::Dsn),
            "multipart/report has an unexpected structured report part",
        ),
    }
}

fn analyze_mdn(parsed: &ParsedMail<'_>, second: &ParsedMail<'_>) -> DeliveryReportAnalysis {
    let body = match second.get_body() {
        Ok(body) => body,
        Err(error) => return malformed(DeliveryReportKind::Mdn, error.to_string()),
    };
    let fields = Fields::parse(&body);
    let Some(final_recipient) = parse_typed_value(fields.get("final-recipient")) else {
        return malformed(DeliveryReportKind::Mdn, "Final-Recipient is required");
    };
    let Some(disposition) = fields.get("disposition") else {
        return malformed(DeliveryReportKind::Mdn, "Disposition is required");
    };
    let Some(disposition) = parse_disposition(&disposition) else {
        return malformed(
            DeliveryReportKind::Mdn,
            "invalid Disposition syntax or value",
        );
    };
    let original_recipient = fields
        .get("original-recipient")
        .and_then(|value| parse_typed_value(Some(value)));
    let has_third_part = parsed.subparts.len() > 2;
    let original_message_id = fields.get("original-message-id");
    DeliveryReportAnalysis::Mdn(MdnReport {
        reporting_ua: fields.get("reporting-ua"),
        original_recipient,
        final_recipient,
        original_message_id,
        disposition,
        has_third_part,
    })
}

fn analyze_dsn(parsed: &ParsedMail<'_>, second: &ParsedMail<'_>) -> DeliveryReportAnalysis {
    let body = match second.get_body() {
        Ok(body) => body,
        Err(error) => return malformed(DeliveryReportKind::Dsn, error.to_string()),
    };
    let groups = split_field_groups(&body);
    let Some(message_group) = groups.first() else {
        return malformed(DeliveryReportKind::Dsn, "message fields are missing");
    };
    let Some(reporting_mta) = message_group.get("reporting-mta") else {
        return malformed(DeliveryReportKind::Dsn, "Reporting-MTA is required");
    };
    if groups.len() < 2 {
        return malformed(
            DeliveryReportKind::Dsn,
            "at least one recipient is required",
        );
    }

    let mut recipients = Vec::new();
    for group in groups.iter().skip(1) {
        let Some(final_recipient) = parse_typed_value(group.get("final-recipient")) else {
            return malformed(DeliveryReportKind::Dsn, "Final-Recipient is required");
        };
        let Some(action) = group.get("action") else {
            return malformed(DeliveryReportKind::Dsn, "Action is required");
        };
        let action = action.to_ascii_lowercase();
        if !matches!(
            action.as_str(),
            "failed" | "delayed" | "delivered" | "relayed" | "expanded"
        ) {
            return malformed(DeliveryReportKind::Dsn, "unsupported Action value");
        }
        let Some(status) = group.get("status") else {
            return malformed(DeliveryReportKind::Dsn, "Status is required");
        };
        if !reasonable_status(&status) {
            return malformed(DeliveryReportKind::Dsn, "invalid Status syntax");
        }
        let will_retry_until = group.get("will-retry-until");
        if will_retry_until.is_some() && action != "delayed" {
            return malformed(
                DeliveryReportKind::Dsn,
                "Will-Retry-Until is only valid for delayed delivery",
            );
        }
        recipients.push(DsnRecipient {
            original_recipient: group
                .get("original-recipient")
                .and_then(|value| parse_typed_value(Some(value))),
            final_recipient,
            action,
            status,
            remote_mta: group.get("remote-mta"),
            diagnostic_code: group.get("diagnostic-code").map(strip_service_prefix),
            last_attempt_date: group.get("last-attempt-date"),
            final_log_id: group.get("final-log-id"),
            will_retry_until,
        });
    }

    let original_message_id = parsed.subparts.get(2).and_then(|part| {
        part.get_body().ok().and_then(|body| {
            body.lines()
                .find_map(|line| line.strip_prefix("Message-ID: ").map(str::to_owned))
        })
    });
    DeliveryReportAnalysis::Dsn(DsnReport {
        message: DsnMessageFields {
            original_envelope_id: message_group.get("original-envelope-id"),
            reporting_mta: reporting_mta.clone(),
            dsn_gateway: message_group.get("dsn-gateway"),
            received_from_mta: message_group.get("received-from-mta"),
            arrival_date: message_group.get("arrival-date"),
        },
        recipients,
        original_message_id,
        has_third_part: parsed.subparts.len() > 2,
    })
}

fn split_field_groups(body: &str) -> Vec<Fields> {
    body.replace("\r\n", "\n")
        .split("\n\n")
        .map(Fields::parse)
        .filter(|fields| !fields.0.is_empty())
        .collect()
}

fn parse_typed_value(value: Option<String>) -> Option<String> {
    let value = value?;
    let (_, value) = value.split_once(';')?;
    let value = value.trim();
    (!value.is_empty()).then(|| value.to_string())
}

fn parse_disposition(value: &str) -> Option<MdnDisposition> {
    let mut sections = value.split(';').map(str::trim);
    let action = sections.next()?.to_ascii_lowercase();
    let disposition = sections.next()?.to_ascii_lowercase();
    let (action_mode, sending_mode) = action
        .split_once('/')
        .map(|(mode, sending)| (mode.trim(), sending.trim()))?;
    if !matches!(action_mode, "automatic-action" | "manual-action")
        || !matches!(sending_mode, "mdn-sent-automatically" | "mdn-sent-manually")
    {
        return None;
    }
    let mut disposition_parts = disposition.split('/');
    let disposition_type = disposition_parts.next()?.trim().to_string();
    if !matches!(
        disposition_type.as_str(),
        "displayed" | "deleted" | "processed" | "dispatched"
    ) {
        return None;
    }
    Some(MdnDisposition {
        action_mode: action_mode.to_string(),
        sending_mode: sending_mode.to_string(),
        disposition_type,
        modifiers: disposition_parts
            .flat_map(|part| part.split(','))
            .map(str::trim)
            .filter(|part| !part.is_empty())
            .map(str::to_string)
            .collect(),
    })
}

fn strip_service_prefix(value: String) -> String {
    value
        .split_once(';')
        .map(|(_, detail)| detail.trim().to_string())
        .unwrap_or(value)
}

fn reasonable_status(value: &str) -> bool {
    let mut parts = value.split('.');
    let (Some(class), Some(subject), Some(detail), None) =
        (parts.next(), parts.next(), parts.next(), parts.next())
    else {
        return false;
    };
    matches!(class, "2" | "4" | "5")
        && valid_status_component(subject)
        && valid_status_component(detail)
}

fn valid_status_component(value: &str) -> bool {
    (1..=3).contains(&value.len())
        && value.as_bytes().iter().all(u8::is_ascii_digit)
        && (value.len() == 1 || !value.starts_with('0'))
}

fn has_subpart_type(parsed: &ParsedMail<'_>, mime: &str) -> bool {
    parsed
        .subparts
        .iter()
        .any(|part| part.ctype.mimetype.eq_ignore_ascii_case(mime))
}

fn kind_from_report_type(value: Option<&str>) -> Option<DeliveryReportKind> {
    match value {
        Some("disposition-notification") => Some(DeliveryReportKind::Mdn),
        Some("delivery-status") => Some(DeliveryReportKind::Dsn),
        _ => None,
    }
}

fn malformed(kind: DeliveryReportKind, reason: impl Into<String>) -> DeliveryReportAnalysis {
    DeliveryReportAnalysis::Malformed {
        kind,
        reason: reason.into(),
    }
}

fn unsupported(kind: DeliveryReportKind, reason: impl Into<String>) -> DeliveryReportAnalysis {
    DeliveryReportAnalysis::Unsupported {
        kind,
        reason: reason.into(),
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use super::*;
    use serde::Deserialize;
    use std::fs;
    use std::path::PathBuf;

    #[derive(Debug, Deserialize)]
    struct GoldenRecipient {
        original_recipient: Option<String>,
        final_recipient: Option<String>,
        action: Option<String>,
        status: Option<String>,
        diagnostic_code: Option<String>,
        #[serde(default)]
        extra_fields: BTreeMap<String, String>,
    }

    #[derive(Debug, Deserialize)]
    struct GoldenOracle {
        case: String,
        kind: String,
        validity: String,
        original_message_id: Option<String>,
        recipients: Vec<GoldenRecipient>,
        action: Option<String>,
        #[serde(default)]
        message_fields: BTreeMap<String, String>,
    }

    fn corpus_root() -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../experiments/mdn-dsn-corpus-probe")
    }

    fn fixture(name: &str) -> Vec<u8> {
        fs::read(corpus_root().join("fixtures").join(format!("{name}.eml"))).unwrap()
    }

    fn oracle(name: &str) -> GoldenOracle {
        serde_json::from_slice(
            &fs::read(
                corpus_root()
                    .join("oracles")
                    .join(format!("{name}.expected.json")),
            )
            .unwrap(),
        )
        .unwrap()
    }

    fn assert_golden(name: &str) {
        let expected = oracle(name);
        assert_eq!(expected.case, name);
        let result = analyze_delivery_report(&fixture(name));
        match (expected.kind.as_str(), expected.validity.as_str(), result) {
            ("ordinary", "valid", DeliveryReportAnalysis::Ordinary) => {}
            ("mdn", "valid", DeliveryReportAnalysis::Mdn(actual)) => {
                assert_eq!(actual.original_message_id, expected.original_message_id);
                assert_eq!(
                    actual.final_recipient,
                    expected.recipients[0].final_recipient.clone().unwrap()
                );
                assert_eq!(
                    actual.original_recipient,
                    expected.recipients[0].original_recipient
                );
                let disposition = parse_disposition(&expected.action.clone().unwrap()).unwrap();
                assert_eq!(actual.disposition, disposition);
            }
            ("dsn", "valid", DeliveryReportAnalysis::Dsn(actual)) => {
                assert_eq!(actual.original_message_id, expected.original_message_id);
                assert_eq!(
                    actual.message.reporting_mta,
                    expected.message_fields["reporting-mta"]
                );
                assert_eq!(
                    actual.message.original_envelope_id,
                    expected.message_fields.get("original-envelope-id").cloned()
                );
                assert_eq!(
                    actual.message.dsn_gateway,
                    expected.message_fields.get("dsn-gateway").cloned()
                );
                assert_eq!(
                    actual.message.received_from_mta,
                    expected.message_fields.get("received-from-mta").cloned()
                );
                assert_eq!(
                    actual.message.arrival_date,
                    expected.message_fields.get("arrival-date").cloned()
                );
                assert_eq!(actual.recipients.len(), expected.recipients.len());
                for (actual, expected) in actual.recipients.iter().zip(expected.recipients) {
                    assert_eq!(actual.original_recipient, expected.original_recipient);
                    assert_eq!(actual.final_recipient, expected.final_recipient.unwrap());
                    assert_eq!(actual.action, expected.action.unwrap().to_ascii_lowercase());
                    assert_eq!(actual.status, expected.status.unwrap());
                    assert_eq!(actual.diagnostic_code, expected.diagnostic_code);
                    assert_eq!(
                        actual.last_attempt_date,
                        expected.extra_fields.get("last-attempt-date").cloned()
                    );
                    assert_eq!(
                        actual.final_log_id,
                        expected.extra_fields.get("final-log-id").cloned()
                    );
                    assert_eq!(
                        actual.will_retry_until,
                        expected.extra_fields.get("will-retry-until").cloned()
                    );
                }
            }
            (
                "mdn",
                "malformed",
                DeliveryReportAnalysis::Malformed {
                    kind: DeliveryReportKind::Mdn,
                    ..
                },
            )
            | (
                "dsn",
                "malformed",
                DeliveryReportAnalysis::Malformed {
                    kind: DeliveryReportKind::Dsn,
                    ..
                },
            )
            | (
                "mdn",
                "unsupported",
                DeliveryReportAnalysis::Unsupported {
                    kind: DeliveryReportKind::Mdn,
                    ..
                },
            )
            | (
                "dsn",
                "unsupported",
                DeliveryReportAnalysis::Unsupported {
                    kind: DeliveryReportKind::Dsn,
                    ..
                },
            ) => {}
            (kind, validity, actual) => {
                panic!("{name}: expected {kind}/{validity}, got {actual:?}")
            }
        }
    }

    #[test]
    fn all_committed_golden_fixtures_match() {
        for entry in fs::read_dir(corpus_root().join("fixtures")).unwrap() {
            let path = entry.unwrap().path();
            if path.extension().and_then(|value| value.to_str()) == Some("eml") {
                assert_golden(path.file_stem().unwrap().to_str().unwrap());
            }
        }
    }

    #[test]
    fn ordinary_text_is_not_a_delivery_report() {
        let result = analyze_delivery_report(
            b"Subject: Delivery Status Notification\r\n\r\nUndelivered mail and Read receipt.\r\n",
        );
        assert_eq!(result, DeliveryReportAnalysis::Ordinary);
    }

    fn dsn_with_status(status: &str) -> Vec<u8> {
        format!(
            "Content-Type: multipart/report; report-type=delivery-status; boundary=b\r\n\r\n--b\r\nContent-Type: text/plain\r\n\r\nsummary\r\n--b\r\nContent-Type: message/delivery-status\r\n\r\nReporting-MTA: dns; mta.example.invalid\r\n\r\nFinal-Recipient: rfc822; recipient@example.invalid\r\nAction: failed\r\nStatus: {status}\r\n\r\n--b--\r\n"
        )
        .into_bytes()
    }

    fn mdn_with_disposition(disposition: &str) -> Vec<u8> {
        format!(
            "Content-Type: multipart/report; report-type=disposition-notification; boundary=b\r\n\r\n--b\r\nContent-Type: text/plain\r\n\r\nsummary\r\n--b\r\nContent-Type: message/disposition-notification\r\n\r\nFinal-Recipient: rfc822; recipient@example.invalid\r\nDisposition: {disposition}\r\n\r\n--b--\r\n"
        )
        .into_bytes()
    }

    #[test]
    fn status_syntax_accepts_extensible_three_part_codes() {
        for status in ["5.7.26", "2.1.23", "4.0.0"] {
            assert!(matches!(
                analyze_delivery_report(&dsn_with_status(status)),
                DeliveryReportAnalysis::Dsn(_)
            ));
        }
        for status in ["9.1.1", "5.01.1", "5.1.001", "5.1234.1"] {
            assert!(matches!(
                analyze_delivery_report(&dsn_with_status(status)),
                DeliveryReportAnalysis::Malformed {
                    kind: DeliveryReportKind::Dsn,
                    ..
                }
            ));
        }
    }

    #[test]
    fn mdn_disposition_allows_ows_and_separates_modifiers() {
        for disposition in [
            "manual-action / MDN-sent-manually ; displayed",
            "automatic-action/MDN-sent-automatically ; processed / error",
            "automatic-action / MDN-sent-automatically ; displayed / error , x-corpus-extension",
        ] {
            assert!(matches!(
                analyze_delivery_report(&mdn_with_disposition(disposition)),
                DeliveryReportAnalysis::Mdn(_)
            ));
        }
        let result = analyze_delivery_report(&mdn_with_disposition(
            "automatic-action / MDN-sent-automatically ; displayed / error , x-corpus-extension",
        ));
        let DeliveryReportAnalysis::Mdn(report) = result else {
            panic!("unexpected result: {result:?}");
        };
        assert_eq!(report.disposition.action_mode, "automatic-action");
        assert_eq!(report.disposition.sending_mode, "mdn-sent-automatically");
        assert_eq!(
            report.disposition.modifiers,
            ["error", "x-corpus-extension"]
        );
    }

    #[test]
    fn unparseable_mime_does_not_classify_by_text() {
        let result = analyze_delivery_report(b"Content-Type: text/plain\r\n\rdelivery-status");
        assert!(matches!(result, DeliveryReportAnalysis::Unparseable { .. }));
    }

    #[test]
    fn report_with_incoherent_second_part_is_malformed() {
        let raw = b"Content-Type: multipart/report; report-type=delivery-status; boundary=b\r\n\r\n--b\r\nContent-Type: text/plain\r\n\r\nnot a delivery status part\r\n--b--\r\n";
        assert!(matches!(
            analyze_delivery_report(raw),
            DeliveryReportAnalysis::Malformed {
                kind: DeliveryReportKind::Dsn,
                ..
            }
        ));
        let raw = b"Content-Type: multipart/report; report-type=disposition-notification; boundary=b\r\n\r\n--b\r\nContent-Type: text/plain\r\n\r\nnot an MDN part\r\n--b--\r\n";
        assert!(matches!(
            analyze_delivery_report(raw),
            DeliveryReportAnalysis::Malformed {
                kind: DeliveryReportKind::Mdn,
                ..
            }
        ));
    }

    #[test]
    fn folded_fields_preserve_a_separator() {
        let raw = b"Content-Type: multipart/report; report-type=delivery-status; boundary=b\r\n\r\n--b\r\nContent-Type: text/plain\r\n\r\nsummary\r\n--b\r\nContent-Type: message/delivery-status\r\n\r\nReporting-MTA: dns; mta.example.invalid\r\n\r\nFinal-Recipient: rfc822; recipient@example.invalid\r\nAction: failed\r\nStatus: 5.1.1\r\nDiagnostic-Code: smtp; 550 mailbox\r\n unavailable\r\n\r\n--b--\r\n";
        let DeliveryReportAnalysis::Dsn(report) = analyze_delivery_report(raw) else {
            panic!("unexpected result");
        };
        assert_eq!(
            report.recipients[0].diagnostic_code.as_deref(),
            Some("550 mailbox unavailable")
        );
    }

    #[test]
    fn direct_structural_invariants_are_checked() {
        assert!(matches!(
            analyze_delivery_report(&fixture("negative-10-missing-required")),
            DeliveryReportAnalysis::Malformed {
                kind: DeliveryReportKind::Dsn,
                ..
            }
        ));
        let dsn = match analyze_delivery_report(&fixture("dsn-04-two-recipients")) {
            DeliveryReportAnalysis::Dsn(value) => value,
            other => panic!("unexpected result: {other:?}"),
        };
        assert_eq!(dsn.recipients.len(), 2);
        assert!(matches!(
            analyze_delivery_report(&fixture("rfc6533-01-global-dsn")),
            DeliveryReportAnalysis::Unsupported {
                kind: DeliveryReportKind::Dsn,
                ..
            }
        ));
        assert!(matches!(
            analyze_delivery_report(&fixture("dsn-13-relayed-expanded-fields")),
            DeliveryReportAnalysis::Dsn(_)
        ));
        let mut invalid_retry = fixture("dsn-13-relayed-expanded-fields");
        let source = b"Action: relayed\r\n";
        let replacement =
            b"Action: relayed\r\nWill-Retry-Until: Thu, 21 Aug 2026 11:00:00 +0000\r\n";
        let offset = invalid_retry
            .windows(source.len())
            .position(|window| window == source)
            .unwrap();
        invalid_retry.splice(offset..offset + source.len(), replacement.iter().copied());
        assert!(matches!(
            analyze_delivery_report(&invalid_retry),
            DeliveryReportAnalysis::Malformed {
                kind: DeliveryReportKind::Dsn,
                ..
            }
        ));
        assert!(matches!(
            analyze_delivery_report(&fixture("mdn-13-original-id-without-third")),
            DeliveryReportAnalysis::Mdn(_)
        ));
        assert!(matches!(
            analyze_delivery_report(&fixture("mdn-14-third-without-original-id")),
            DeliveryReportAnalysis::Mdn(_)
        ));
    }
}
