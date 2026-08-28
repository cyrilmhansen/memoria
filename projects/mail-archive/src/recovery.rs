//! Conservative, read-only Tier A recovery planning.
//!
//! This module deliberately has no executor.  A plan is evidence and a
//! proposed next action, never permission to mutate the archive.

use crate::{ArchiveLocation, PhysicalFrameStatus, RecordInventoryStatus};
use rusqlite::{Connection, OpenFlags};
use std::io;
use std::path::{Path, PathBuf};

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub enum RecoveryDisposition {
    NoAction,
    RecoverableWithSource,
    RecoverableWithUserChoice,
    SalvageOnly,
    UnrecoverableLocally,
    UnsafeToRepairAutomatically,
    SafeCleanupCandidate,
}

impl RecoveryDisposition {
    pub fn label(self) -> &'static str {
        match self {
            Self::NoAction => "no-action",
            Self::RecoverableWithSource => "source",
            Self::RecoverableWithUserChoice => "user-choice",
            Self::SalvageOnly => "salvage",
            Self::UnrecoverableLocally => "unrecoverable-locally",
            Self::UnsafeToRepairAutomatically => "unsafe-automatic-repair",
            Self::SafeCleanupCandidate => "cleanup-candidate",
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RecoveryEvidence {
    pub facts: Vec<String>,
    pub source_identities: Vec<SourceIdentityEvidence>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SourceIdentityEvidence {
    pub kind: String,
    pub source_account: String,
    pub gmail_message_id: Option<String>,
    pub mailbox: Option<String>,
    pub uid_validity: Option<u32>,
    pub uid: Option<u32>,
    pub doc_id: i64,
    pub source_state: String,
    pub valid_for_refetch: bool,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RecoveryItem {
    pub subject: String,
    pub status: String,
    pub disposition: RecoveryDisposition,
    pub automatic: bool,
    pub proposed_action: String,
    pub evidence: RecoveryEvidence,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RecoveryPlan {
    pub archive: PathBuf,
    pub catalogue_state: String,
    pub archive_state: String,
    pub items: Vec<RecoveryItem>,
}

impl RecoveryPlan {
    pub fn is_empty(&self) -> bool {
        self.items.is_empty()
    }
}

fn item(
    subject: impl Into<String>,
    status: impl Into<String>,
    disposition: RecoveryDisposition,
    action: impl Into<String>,
    facts: Vec<String>,
) -> RecoveryItem {
    RecoveryItem {
        subject: subject.into(),
        status: status.into(),
        disposition,
        automatic: false,
        proposed_action: action.into(),
        evidence: RecoveryEvidence {
            facts,
            source_identities: Vec::new(),
        },
    }
}

#[derive(Default)]
struct SourceEvidence {
    identities: Vec<SourceIdentityEvidence>,
    invalid: bool,
    refetchable: usize,
    deleted: bool,
}

fn source_evidence(path: &Path, doc_id: i64) -> io::Result<SourceEvidence> {
    let connection = Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_ONLY)
        .map_err(io::Error::other)?;
    let mut result = SourceEvidence::default();
    let mut gmail = connection
        .prepare("SELECT source_account,gmail_message_id,source_state FROM gmail_messages WHERE doc_id=?1 ORDER BY source_account,gmail_message_id")
        .map_err(io::Error::other)?;
    for row in gmail
        .query_map([doc_id], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
            ))
        })
        .map_err(io::Error::other)?
    {
        let (account, message_id, state) = row.map_err(io::Error::other)?;
        let valid = !account.is_empty()
            && !message_id.is_empty()
            && matches!(state.as_str(), "present" | "deleted");
        if state == "deleted" {
            result.deleted = true;
        }
        if valid && state == "present" {
            result.refetchable += 1;
        } else if !valid {
            result.invalid = true;
        }
        result.identities.push(SourceIdentityEvidence {
            kind: "Gmail".into(),
            source_account: account,
            gmail_message_id: Some(message_id),
            mailbox: None,
            uid_validity: None,
            uid: None,
            doc_id,
            source_state: state.clone(),
            valid_for_refetch: valid && state == "present",
        });
    }
    let mut imap = connection
        .prepare("SELECT source_account,mailbox,uid_validity,uid,source_state FROM imap_messages WHERE doc_id=?1 ORDER BY source_account,mailbox,uid_validity,uid")
        .map_err(io::Error::other)?;
    for row in imap
        .query_map([doc_id], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, i64>(2)?,
                row.get::<_, i64>(3)?,
                row.get::<_, String>(4)?,
            ))
        })
        .map_err(io::Error::other)?
    {
        let (account, mailbox, uid_validity, uid, state) = row.map_err(io::Error::other)?;
        let valid_numbers = u32::try_from(uid_validity)
            .ok()
            .filter(|value| *value != 0)
            .zip(u32::try_from(uid).ok().filter(|value| *value != 0));
        let valid = !account.is_empty()
            && !mailbox.is_empty()
            && valid_numbers.is_some()
            && matches!(state.as_str(), "present" | "deleted");
        if valid && state == "present" {
            result.refetchable += 1;
        } else if !valid {
            result.invalid = true;
        }
        let (uid_validity, uid) = valid_numbers.unzip();
        result.identities.push(SourceIdentityEvidence {
            kind: "IMAP".into(),
            source_account: account,
            gmail_message_id: None,
            mailbox: Some(mailbox),
            uid_validity,
            uid,
            doc_id,
            source_state: state.clone(),
            valid_for_refetch: valid && state == "present",
        });
        if state == "deleted" {
            result.deleted = true;
        }
    }
    Ok(result)
}

fn classify_missing(
    path: &Path,
    doc_id: i64,
    location: &Option<ArchiveLocation>,
) -> io::Result<RecoveryItem> {
    let source = source_evidence(path, doc_id)?;
    let mut facts = vec![
        "catalogue claim exists".into(),
        "physical frame is missing".into(),
    ];
    for identity in &source.identities {
        facts.push(format!("observed source identity: {identity:?}"));
    }
    if source.invalid || source.refetchable > 1 || (source.deleted && source.refetchable > 0) {
        facts.push(
            "source identity is invalid or multiple incompatible identities claim this doc_id"
                .into(),
        );
        return Ok(item(
            format!("doc {doc_id}"),
            "PhysicallyMissing",
            RecoveryDisposition::UnsafeToRepairAutomatically,
            "resolve source identity before any re-fetch",
            facts,
        ));
    }
    if source.refetchable == 1 {
        facts.push("durable, present source identity is valid for a future re-fetch".into());
        facts.push(format!("catalogue coordinate: {location:?}"));
        let mut result = item(
            format!("doc {doc_id}"),
            "PhysicallyMissing",
            RecoveryDisposition::RecoverableWithSource,
            "RefetchGmailMessage or RefetchImapMessage (explicit future action)",
            facts,
        );
        result.evidence.source_identities = source.identities;
        Ok(result)
    } else if source.deleted {
        facts.push(
            "source identity is observed but source_state=deleted; re-fetch is not promised".into(),
        );
        let mut result = item(
            format!("doc {doc_id}"),
            "PhysicallyMissing",
            RecoveryDisposition::RecoverableWithUserChoice,
            "confirm whether an explicit source recovery is still desired",
            facts,
        );
        result.evidence.source_identities = source.identities;
        Ok(result)
    } else {
        facts.push("no durable Gmail/IMAP identity is present".into());
        let mut result = item(
            format!("doc {doc_id}"),
            "PhysicallyMissing",
            RecoveryDisposition::UnrecoverableLocally,
            "preserve the missing claim and report it",
            facts,
        );
        result.evidence.source_identities = source.identities;
        Ok(result)
    }
}

/// Build a deterministic plan without acquiring writer authority or opening
/// SQLite in read-write mode.  No network, sidecar, WAL, or recovery file is
/// created by this function.
pub fn plan_recovery(root: &Path) -> io::Result<RecoveryPlan> {
    let catalogue = root.join("metadata.sqlite");
    let archive = root.join("archive");
    let catalogue_state = match std::fs::metadata(&catalogue) {
        Ok(metadata) if metadata.is_file() => {
            if crate::validate_existing_catalogue(&catalogue).is_ok() {
                "valid-v1".to_string()
            } else {
                // Do not expose the SQLite error as an authority claim. Both a
                // bad schema/version and an unreadable database are fail-closed.
                "invalid-or-unreadable".to_string()
            }
        }
        Ok(_) => "invalid-or-unreadable".to_string(),
        Err(error) if error.kind() == io::ErrorKind::NotFound => "absent".to_string(),
        Err(error) => return Err(error),
    };
    let archive_state = if !archive.exists() {
        "absent".to_string()
    } else {
        "present".to_string()
    };
    let mut plan = RecoveryPlan {
        archive: root.to_path_buf(),
        catalogue_state: catalogue_state.clone(),
        archive_state: archive_state.clone(),
        items: Vec::new(),
    };
    if catalogue_state == "absent" {
        plan.items.push(item(
            "catalogue",
            "CatalogueLost",
            RecoveryDisposition::RecoverableWithUserChoice,
            "inspect any validated RAW frames; do not recreate provenance",
            vec![
                "metadata.sqlite is absent".into(),
                "RAW-only recovery cannot restore source identities or sync state".into(),
            ],
        ));
    } else if catalogue_state != "valid-v1" {
        plan.items.push(item(
            "catalogue",
            "CatalogueInvalidOrUnreadable",
            RecoveryDisposition::UnsafeToRepairAutomatically,
            "preserve the database and obtain a verified backup/schema decision",
            vec!["catalogue validation failed; no row is treated as authoritative".into()],
        ));
    }
    if catalogue_state == "invalid-or-unreadable" {
        if archive_state == "absent" {
            plan.items.push(item(
                "archive",
                "ArchiveRawAbsent",
                RecoveryDisposition::UnrecoverableLocally,
                "restore the original archive from a verified copy",
                vec!["archive directory is absent".into()],
            ));
        }
        return Ok(plan);
    }

    if archive_state == "absent" {
        if catalogue_state != "valid-v1" {
            return Ok(plan);
        }
        let records = crate::inventory_records(root)?;
        for record in records {
            match &record.status {
                RecordInventoryStatus::PhysicallyMissing => {
                    plan.items.push(classify_missing(
                        &catalogue,
                        record.doc_id,
                        &record.location,
                    )?);
                }
                RecordInventoryStatus::Inconsistent { reason } => plan.items.push(item(
                    format!("doc {}", record.doc_id),
                    "CataloguedInconsistent",
                    RecoveryDisposition::UnsafeToRepairAutomatically,
                    "diagnose without re-linking",
                    vec![reason.clone()],
                )),
                RecordInventoryStatus::AvailableValidated => {}
            }
        }
        return Ok(plan);
    }
    let records = if catalogue_state == "valid-v1" {
        crate::inventory_records(root)?
    } else {
        Vec::new()
    };
    let physical = crate::inventory_physical(root)?;
    for record in &records {
        match &record.status {
            RecordInventoryStatus::PhysicallyMissing => {
                let contradiction = record.location.as_ref().is_some_and(|location| {
                    physical.frames.iter().any(|frame| {
                        frame.location.segment == location.segment
                            && frame.location.offset == location.offset
                            && matches!(frame.status, PhysicalFrameStatus::CataloguedInconsistent)
                    })
                });
                if contradiction {
                    plan.items.push(item(
                        format!("doc {}", record.doc_id),
                        "CataloguedInconsistent",
                        RecoveryDisposition::UnsafeToRepairAutomatically,
                        "diagnose the physical/catalogue contradiction; do not re-link",
                        vec![
                            "inventory_records reported PhysicallyMissing".into(),
                            "inventory_physical found a claimed frame at the same location but it is inconsistent".into(),
                        ],
                    ));
                } else {
                    plan.items.push(classify_missing(
                        &catalogue,
                        record.doc_id,
                        &record.location,
                    )?);
                }
            }
            RecordInventoryStatus::Inconsistent { reason } => plan.items.push(item(
                format!("doc {}", record.doc_id),
                "CataloguedInconsistent",
                RecoveryDisposition::UnsafeToRepairAutomatically,
                "diagnose and present a candidate only; do not re-link",
                vec![
                    "catalogue and physical evidence disagree".into(),
                    reason.clone(),
                ],
            )),
            RecordInventoryStatus::AvailableValidated => {}
        }
    }
    for frame in physical.frames {
        match frame.status {
            PhysicalFrameStatus::OrphanValidated => plan.items.push(item(
                format!("{} @ {}", frame.location.segment, frame.location.offset),
                "OrphanValidated",
                RecoveryDisposition::SalvageOnly,
                "retain in place and optionally export byte-exactly/manifest it",
                vec![
                    "framing, physical checksum and BLAKE3 are valid".into(),
                    format!("doc_id {:?} is not a source identity", frame.doc_id),
                    "same doc_id elsewhere cannot adopt this frame".into(),
                ],
            )),
            PhysicalFrameStatus::PhysicalCorruption { reason } => plan.items.push(item(
                format!("{} @ {}", frame.location.segment, frame.location.offset),
                "PhysicalCorruption",
                if records.iter().any(|record| {
                    record.location.as_ref().is_some_and(|location| {
                        location.segment == frame.location.segment
                            && location.offset == frame.location.offset
                    })
                }) {
                    RecoveryDisposition::UnsafeToRepairAutomatically
                } else {
                    RecoveryDisposition::UnrecoverableLocally
                },
                "preserve evidence; recover only from a verified source or backup",
                vec![
                    reason,
                    "corrupt bytes are not a reconstruction source".into(),
                ],
            )),
            PhysicalFrameStatus::IncompleteTail { reason } => plan.items.push(item(
                format!("{} @ {}", frame.location.segment, frame.location.offset),
                "IncompleteTail",
                if records.iter().any(|record| {
                    record.location.as_ref().is_some_and(|location| {
                        location.segment == frame.location.segment
                            && location.offset < frame.location.offset + frame.location.frame_bytes
                            && frame.location.offset < location.offset + location.frame_bytes
                    })
                }) {
                    RecoveryDisposition::UnsafeToRepairAutomatically
                } else {
                    RecoveryDisposition::SafeCleanupCandidate
                },
                "consider future terminal-tail cleanup only after explicit safety proof",
                vec![reason, "no truncation is performed by R1".into()],
            )),
            PhysicalFrameStatus::CataloguedValidated
            | PhysicalFrameStatus::CataloguedInconsistent => {}
        }
    }
    plan.items
        .sort_by(|a, b| a.subject.cmp(&b.subject).then(a.status.cmp(&b.status)));
    Ok(plan)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{ArchiveSession, CatalogueBatchRecord, Message};
    use rusqlite::Connection;
    use std::fs;

    #[test]
    fn disposition_labels_are_stable() {
        assert_eq!(RecoveryDisposition::SalvageOnly.label(), "salvage");
        assert!(!RecoveryDisposition::NoAction.label().is_empty());
    }

    #[test]
    fn missing_archive_is_planned_without_creating_anything() {
        let root = std::env::temp_dir().join(format!(
            "atlas-recovery-test-{}-missing",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&root);
        let plan = plan_recovery(&root).unwrap();
        assert_eq!(plan.catalogue_state, "absent");
        assert_eq!(plan.archive_state, "absent");
        assert!(plan
            .items
            .iter()
            .any(|entry| entry.status == "CatalogueLost"));
        assert!(!root.exists(), "planner must not create an archive");
    }

    #[test]
    fn invalid_catalogue_fails_closed_and_is_repeatable() {
        let root = std::env::temp_dir().join(format!(
            "atlas-recovery-test-{}-invalid",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(root.join("archive")).unwrap();
        fs::write(root.join("metadata.sqlite"), b"not sqlite").unwrap();
        let before = fs::read(root.join("metadata.sqlite")).unwrap();
        let first = plan_recovery(&root).unwrap();
        let second = plan_recovery(&root).unwrap();
        assert_eq!(first, second);
        assert_eq!(first.catalogue_state, "invalid-or-unreadable");
        assert!(first.items.iter().any(|entry| {
            entry.status == "CatalogueInvalidOrUnreadable"
                && entry.disposition == RecoveryDisposition::UnsafeToRepairAutomatically
        }));
        assert_eq!(fs::read(root.join("metadata.sqlite")).unwrap(), before);
        assert!(!root.join("metadata.sqlite-wal").exists());
        assert!(!root.join("metadata.sqlite-shm").exists());
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn healthy_and_same_doc_orphan_plans_are_read_only() {
        let root =
            std::env::temp_dir().join(format!("atlas-recovery-test-{}-orphan", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        let mut session = ArchiveSession::create(&root, 4096).unwrap();
        session.writer_mut().append_raw(7, b"orphan A").unwrap();
        let orphan_durable = session.writer_mut().durable_barrier().unwrap();
        let published = session.writer_mut().append_raw(7, b"published B").unwrap();
        let durable = session.writer_mut().durable_barrier().unwrap();
        let message = Message {
            id: 7,
            message_id: "recovery-published".into(),
            timestamp: 0,
            sender: String::new(),
            recipients: Vec::new(),
            subject: String::new(),
            text_body: String::new(),
            html_body: None,
            account: String::new(),
            folder: String::new(),
            thread: String::new(),
            attachments: Vec::new(),
            raw: b"published B".to_vec(),
        };
        session
            .publish_catalogue_batch(&[CatalogueBatchRecord::new(message, published)], &durable)
            .unwrap();
        let orphan_location = &orphan_durable.entries()[0].reference().location;
        let segment_path = root.join("archive").join(&orphan_location.segment);
        let raw_before = fs::read(&segment_path).unwrap();
        let catalog_before = fs::read(root.join("metadata.sqlite")).unwrap();
        let plan = plan_recovery(&root).unwrap();
        assert!(plan.items.iter().any(|entry| {
            entry.status == "OrphanValidated"
                && entry.subject.contains(&orphan_location.offset.to_string())
                && entry.disposition == RecoveryDisposition::SalvageOnly
        }));
        assert!(!plan.items.iter().any(|entry| entry.automatic));
        assert_eq!(fs::read(&segment_path).unwrap(), raw_before);
        assert_eq!(
            fs::read(root.join("metadata.sqlite")).unwrap(),
            catalog_before
        );
        assert!(!root.join("metadata.sqlite-wal").exists());
        assert!(!root.join("metadata.sqlite-shm").exists());
        drop(session);
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn source_evidence_is_explicit_and_archive_absence_is_per_record() {
        let root = std::env::temp_dir().join(format!(
            "atlas-recovery-test-{}-sources",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&root);
        let mut session = ArchiveSession::create(&root, 4096).unwrap();
        let mut pending = Vec::new();
        let mut messages = Vec::new();
        for id in 1..=3u64 {
            let raw = format!("From: source{id}@example.test\r\n\r\nbody").into_bytes();
            pending.push(session.writer_mut().append_raw(id, &raw).unwrap());
            messages.push(Message {
                id,
                message_id: format!("source-{id}"),
                timestamp: 0,
                sender: String::new(),
                recipients: Vec::new(),
                subject: String::new(),
                text_body: String::new(),
                html_body: None,
                account: String::new(),
                folder: String::new(),
                thread: String::new(),
                attachments: Vec::new(),
                raw,
            });
        }
        let durable = session.writer_mut().durable_barrier().unwrap();
        let batch = messages
            .into_iter()
            .zip(pending)
            .map(|(message, location)| CatalogueBatchRecord::new(message, location))
            .collect::<Vec<_>>();
        session.publish_catalogue_batch(&batch, &durable).unwrap();
        drop(session);
        fs::remove_dir_all(root.join("archive")).unwrap();
        let connection = Connection::open(root.join("metadata.sqlite")).unwrap();
        connection
            .execute("INSERT INTO gmail_messages(source_account,gmail_message_id,doc_id,thread_id,label_ids,source_state,first_seen_unix,last_seen_unix) VALUES ('account-a','gmail-1',1,'thread','[]','present',0,0)", [])
            .unwrap();
        connection
            .execute("INSERT INTO gmail_messages(source_account,gmail_message_id,doc_id,thread_id,label_ids,source_state,first_seen_unix,last_seen_unix) VALUES ('','',2,'thread','[]','present',0,0)", [])
            .unwrap();
        connection
            .execute("INSERT INTO imap_messages(source_account,mailbox,uid_validity,uid,doc_id,flags,source_state,first_seen_unix,last_seen_unix) VALUES ('account-b','INBOX',42,9,3,'[]','present',0,0)", [])
            .unwrap();
        drop(connection);
        let plan = plan_recovery(&root).unwrap();
        let by_doc = |doc: &str| {
            plan.items
                .iter()
                .find(|entry| entry.subject == format!("doc {doc}"))
        };
        assert_eq!(
            by_doc("1").unwrap().disposition,
            RecoveryDisposition::RecoverableWithSource
        );
        assert_eq!(
            by_doc("2").unwrap().disposition,
            RecoveryDisposition::UnsafeToRepairAutomatically
        );
        assert_eq!(
            by_doc("3").unwrap().disposition,
            RecoveryDisposition::RecoverableWithSource
        );
        assert!(by_doc("1").unwrap().evidence.source_identities[0].source_account == "account-a");

        let connection = Connection::open(root.join("metadata.sqlite")).unwrap();
        connection.execute("INSERT INTO imap_messages(source_account,mailbox,uid_validity,uid,doc_id,flags,source_state,first_seen_unix,last_seen_unix) VALUES ('account-b','INBOX',42,10,1,'[]','present',0,0)", []).unwrap();
        drop(connection);
        let contradictory = plan_recovery(&root).unwrap();
        assert_eq!(
            contradictory
                .items
                .iter()
                .find(|entry| entry.subject == "doc 1")
                .unwrap()
                .disposition,
            RecoveryDisposition::UnsafeToRepairAutomatically
        );

        let connection = Connection::open(root.join("metadata.sqlite")).unwrap();
        connection
            .execute("DELETE FROM imap_messages WHERE doc_id=1", [])
            .unwrap();
        connection
            .execute(
                "UPDATE gmail_messages SET source_state='deleted' WHERE doc_id=1",
                [],
            )
            .unwrap();
        connection
            .execute("UPDATE imap_messages SET uid_validity=0 WHERE doc_id=3", [])
            .unwrap();
        drop(connection);
        let changed = plan_recovery(&root).unwrap();
        assert_eq!(
            changed
                .items
                .iter()
                .find(|entry| entry.subject == "doc 1")
                .unwrap()
                .disposition,
            RecoveryDisposition::RecoverableWithUserChoice
        );
        assert_eq!(
            changed
                .items
                .iter()
                .find(|entry| entry.subject == "doc 3")
                .unwrap()
                .disposition,
            RecoveryDisposition::UnsafeToRepairAutomatically
        );
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn lost_catalogue_only_reports_real_validated_raw_frames() {
        let root = std::env::temp_dir().join(format!(
            "atlas-recovery-test-{}-lost-catalogue",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&root);
        let mut session = ArchiveSession::create(&root, 4096).unwrap();
        session.writer_mut().append_raw(10, b"raw one").unwrap();
        session.writer_mut().append_raw(11, b"raw two").unwrap();
        session.writer_mut().durable_barrier().unwrap();
        drop(session);
        fs::remove_file(root.join("metadata.sqlite")).unwrap();
        let plan = plan_recovery(&root).unwrap();
        assert_eq!(
            plan.items
                .iter()
                .filter(|entry| entry.status == "OrphanValidated")
                .count(),
            2
        );
        assert!(plan.items.iter().all(|entry| {
            entry.status != "CatalogueLost"
                || entry.disposition == RecoveryDisposition::RecoverableWithUserChoice
        }));
        fs::remove_dir_all(root.join("archive")).unwrap();
        fs::create_dir_all(root.join("archive")).unwrap();
        let empty = plan_recovery(&root).unwrap();
        assert!(!empty
            .items
            .iter()
            .any(|entry| entry.status == "OrphanValidated"));
        assert!(!empty.items.iter().any(|entry| {
            entry.status == "CatalogueLost"
                && entry
                    .proposed_action
                    .contains("export validated RAW frames")
        }));
        let _ = fs::remove_dir_all(root);
    }

    fn published_fixture(label: &str) -> (std::path::PathBuf, crate::ArchiveLocation) {
        let root = std::env::temp_dir().join(format!(
            "atlas-recovery-test-{}-{label}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&root);
        let mut session = ArchiveSession::create(&root, 4096).unwrap();
        let raw = b"From: fixture@example.test\r\n\r\nbody".to_vec();
        let pending = session.writer_mut().append_raw(1, &raw).unwrap();
        let durable = session.writer_mut().durable_barrier().unwrap();
        let message = Message {
            id: 1,
            message_id: format!("fixture-{label}"),
            timestamp: 0,
            sender: String::new(),
            recipients: Vec::new(),
            subject: String::new(),
            text_body: String::new(),
            html_body: None,
            account: String::new(),
            folder: String::new(),
            thread: String::new(),
            attachments: Vec::new(),
            raw,
        };
        session
            .publish_catalogue_batch(&[CatalogueBatchRecord::new(message, pending)], &durable)
            .unwrap();
        let location = durable.entries()[0].reference().location.clone();
        drop(session);
        (root, location)
    }

    #[test]
    fn planner_prioritizes_wrong_frame_length_as_catalogue_contradiction() {
        let (root, location) = published_fixture("planner-length");
        let connection = Connection::open(root.join("metadata.sqlite")).unwrap();
        connection
            .execute(
                "UPDATE messages SET frame_bytes=?1 WHERE doc_id=1",
                [location.frame_bytes as i64 + 100],
            )
            .unwrap();
        drop(connection);
        let plan = plan_recovery(&root).unwrap();
        let entry = plan
            .items
            .iter()
            .find(|entry| entry.subject == "doc 1")
            .unwrap();
        assert_eq!(entry.status, "CataloguedInconsistent");
        assert_eq!(
            entry.disposition,
            RecoveryDisposition::UnsafeToRepairAutomatically
        );
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn planner_never_calls_a_corruption_salvage() {
        let (root, location) = published_fixture("planner-corruption");
        let path = root.join("archive").join(&location.segment);
        let mut bytes = fs::read(&path).unwrap();
        bytes[location.offset as usize + 24] ^= 1;
        fs::write(&path, bytes).unwrap();
        let plan = plan_recovery(&root).unwrap();
        let entry = plan
            .items
            .iter()
            .find(|entry| entry.status == "PhysicalCorruption")
            .unwrap();
        assert_eq!(
            entry.disposition,
            RecoveryDisposition::UnsafeToRepairAutomatically
        );
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn planner_distinguishes_unclaimed_and_claimed_incomplete_tail() {
        let (root, location) = published_fixture("planner-tail");
        let path = root.join("archive").join(&location.segment);
        let mut bytes = fs::read(&path).unwrap();
        let tail_offset = bytes.len() as u64;
        bytes.push(0xaa);
        fs::write(&path, &bytes).unwrap();
        let plan = plan_recovery(&root).unwrap();
        assert_eq!(
            plan.items
                .iter()
                .find(|entry| entry.status == "IncompleteTail")
                .unwrap()
                .disposition,
            RecoveryDisposition::SafeCleanupCandidate
        );

        let connection = Connection::open(root.join("metadata.sqlite")).unwrap();
        connection
            .execute(
                "UPDATE messages SET archive_offset=?1, frame_bytes=1 WHERE doc_id=1",
                [tail_offset as i64],
            )
            .unwrap();
        drop(connection);
        let claimed = plan_recovery(&root).unwrap();
        assert_eq!(
            claimed
                .items
                .iter()
                .find(|entry| entry.status == "IncompleteTail")
                .unwrap()
                .disposition,
            RecoveryDisposition::UnsafeToRepairAutomatically
        );
        let _ = fs::remove_dir_all(root);
    }
}
