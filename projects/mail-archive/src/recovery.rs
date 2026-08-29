//! Conservative, read-only Tier A recovery planning.
//!
//! This module deliberately has no executor.  A plan is evidence and a
//! proposed next action, never permission to mutate the archive.

use crate::{gmail, ArchiveLocation, PhysicalFrameStatus, RecordInventoryStatus};
use rusqlite::{Connection, OpenFlags};
use std::io;
use std::path::{Path, PathBuf};

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum GmailRecoveryResult {
    Recovered {
        doc_id: i64,
        location: ArchiveLocation,
    },
    AlreadyAvailable,
    AccountMismatch,
    UnsafeInconsistent(String),
    RecoveryConflict,
    SourceUnavailable,
    SourceContentChanged {
        expected: [u8; 32],
        fetched: [u8; 32],
    },
}

struct GmailRecoveryRecord {
    location: ArchiveLocation,
    source_account: String,
    gmail_id: String,
    expected_blake3: [u8; 32],
}

enum PreparationError {
    Result(GmailRecoveryResult),
    Error(gmail::GmailError),
}

fn preparation_error(error: impl Into<String>) -> PreparationError {
    PreparationError::Result(GmailRecoveryResult::UnsafeInconsistent(error.into()))
}

fn recovery_record(root: &Path, doc_id: i64) -> Result<GmailRecoveryRecord, PreparationError> {
    let inventory = crate::inventory_records(root)
        .map_err(|error| PreparationError::Error(gmail::GmailError::Other(error.to_string())))?;
    let physical = crate::inventory_physical(root)
        .map_err(|error| PreparationError::Error(gmail::GmailError::Other(error.to_string())))?;
    let record = inventory
        .into_iter()
        .find(|record| record.doc_id == doc_id)
        .ok_or_else(|| preparation_error(format!("catalogue record {doc_id} not found")))?;
    match record.status {
        RecordInventoryStatus::AvailableValidated => {
            return Err(PreparationError::Result(
                GmailRecoveryResult::AlreadyAvailable,
            ))
        }
        RecordInventoryStatus::Inconsistent { reason } => {
            return Err(preparation_error(format!(
                "CataloguedInconsistent: {reason}"
            )))
        }
        RecordInventoryStatus::PhysicallyMissing => {}
    }
    let location = record
        .location
        .clone()
        .ok_or_else(|| preparation_error("missing catalogue location"))?;
    if physical.frames.iter().any(|frame| {
        frame.location.segment == location.segment
            && frame.location.offset == location.offset
            && matches!(frame.status, PhysicalFrameStatus::CataloguedInconsistent)
    }) {
        return Err(preparation_error(
            "CataloguedInconsistent: physical frame contradicts catalogue claim",
        ));
    }
    let connection = Connection::open_with_flags(
        root.join("metadata.sqlite"),
        OpenFlags::SQLITE_OPEN_READ_ONLY,
    )
    .map_err(|error| PreparationError::Error(gmail::GmailError::Other(error.to_string())))?;
    let mut rows = connection
        .prepare(
            "SELECT source_account,gmail_message_id,source_state
             FROM gmail_messages WHERE doc_id=?1 ORDER BY source_account,gmail_message_id",
        )
        .map_err(|error| PreparationError::Error(gmail::GmailError::Other(error.to_string())))?;
    let identities = rows
        .query_map([doc_id], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
            ))
        })
        .map_err(|error| PreparationError::Error(gmail::GmailError::Other(error.to_string())))?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|error| PreparationError::Error(gmail::GmailError::Other(error.to_string())))?;
    let has_other_source: bool = connection
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM imap_messages WHERE doc_id=?1)",
            [doc_id],
            |row| row.get(0),
        )
        .map_err(|error| PreparationError::Error(gmail::GmailError::Other(error.to_string())))?;
    if identities.len() != 1 || has_other_source {
        return Err(preparation_error("Gmail identity is missing or ambiguous"));
    }
    let (source_account, gmail_id, source_state) = identities.into_iter().next().unwrap();
    if source_state != "present" || source_account.is_empty() || gmail_id.is_empty() {
        return Err(preparation_error(
            "Gmail source identity is not present and complete",
        ));
    }
    let (segment, offset, frame_bytes, digest) = connection
        .query_row(
            "SELECT segment,archive_offset,frame_bytes,raw_blake3 FROM messages WHERE doc_id=?1",
            [doc_id],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, i64>(1)?,
                    row.get::<_, i64>(2)?,
                    row.get::<_, Vec<u8>>(3)?,
                ))
            },
        )
        .map_err(|error| PreparationError::Error(gmail::GmailError::Other(error.to_string())))?;
    let expected_blake3: [u8; 32] = digest
        .try_into()
        .map_err(|_| preparation_error("historical BLAKE3 is invalid"))?;
    let location = ArchiveLocation {
        segment,
        offset: u64::try_from(offset).map_err(|_| preparation_error("negative archive offset"))?,
        frame_bytes: u64::try_from(frame_bytes)
            .map_err(|_| preparation_error("negative archive frame length"))?,
    };
    Ok(GmailRecoveryRecord {
        location,
        source_account,
        gmail_id,
        expected_blake3,
    })
}

/// Re-fetch and repair exactly one missing Gmail RAW record.
pub fn recover_missing_gmail_raw<T: gmail::GmailTransport>(
    root: &Path,
    doc_id: i64,
    transport: &mut T,
    segment_bytes: u64,
) -> Result<GmailRecoveryResult, gmail::GmailError> {
    recover_missing_gmail_raw_with_hook(root, doc_id, transport, segment_bytes, |_, _| Ok(()))
}

fn recover_missing_gmail_raw_with_hook<T, F>(
    root: &Path,
    doc_id: i64,
    transport: &mut T,
    segment_bytes: u64,
    before_catalogue_publish: F,
) -> Result<GmailRecoveryResult, gmail::GmailError>
where
    T: gmail::GmailTransport,
    F: FnOnce(&crate::CatalogueConnection, &mut String) -> rusqlite::Result<()>,
{
    let authority = crate::acquire_recovery_authority(root)
        .map_err(|error| gmail::GmailError::Io(error.to_string()))?;
    let record = match recovery_record(root, doc_id) {
        Ok(record) => record,
        Err(PreparationError::Result(result)) => return Ok(result),
        Err(PreparationError::Error(error)) => return Err(error),
    };
    let connection = Connection::open_with_flags(
        root.join("metadata.sqlite"),
        OpenFlags::SQLITE_OPEN_READ_ONLY,
    )
    .map_err(|error| gmail::GmailError::Other(error.to_string()))?;
    let message_id: String = connection
        .query_row(
            "SELECT message_id FROM messages WHERE doc_id=?1",
            [doc_id],
            |row| row.get(0),
        )
        .map_err(|error| gmail::GmailError::Other(error.to_string()))?;
    let canonical = gmail::gmail_message_identity(&record.source_account, &record.gmail_id);
    if message_id != canonical {
        return Ok(GmailRecoveryResult::UnsafeInconsistent(
            "messages.message_id does not match canonical Gmail identity".into(),
        ));
    }
    let profile = transport.profile()?;
    let authenticated = profile
        .email_address
        .as_deref()
        .map(gmail::gmail_source_account);
    let expected_account = record.source_account.as_str();
    if authenticated.as_deref() != Some(expected_account) {
        return Ok(GmailRecoveryResult::AccountMismatch);
    }
    let raw_message = match transport.get_raw(&record.gmail_id) {
        Ok(raw) => raw,
        Err(gmail::GmailError::Http(404)) => return Ok(GmailRecoveryResult::SourceUnavailable),
        Err(error) => return Err(error),
    };
    if raw_message.id != record.gmail_id {
        return Ok(GmailRecoveryResult::UnsafeInconsistent(
            "Gmail response ID does not match requested identity".into(),
        ));
    }
    let raw = gmail::decode_raw(&raw_message.raw)?;
    // Re-read all Tier-A preconditions after the remote proof and before opening a writer.
    let current = match recovery_record(root, doc_id) {
        Ok(record) => record,
        Err(PreparationError::Result(result)) => return Ok(result),
        Err(PreparationError::Error(error)) => return Err(error),
    };
    if current.source_account != record.source_account
        || current.gmail_id != record.gmail_id
        || current.expected_blake3 != record.expected_blake3
        || current.location != record.location
    {
        return Ok(GmailRecoveryResult::UnsafeInconsistent(
            "record changed during revalidation".into(),
        ));
    }
    let fetched = *blake3::hash(&raw).as_bytes();
    if fetched != record.expected_blake3 {
        return Ok(GmailRecoveryResult::SourceContentChanged {
            expected: record.expected_blake3,
            fetched,
        });
    }
    let mut session =
        crate::ArchiveSession::recovery_with_authority(root, segment_bytes, authority)
            .map_err(|error| gmail::GmailError::Io(error.to_string()))?;
    let (writer, _) = session.parts_mut();
    let pending = writer
        .append_raw(doc_id as u64, &raw)
        .map_err(|error| gmail::GmailError::Io(error.to_string()))?;
    let durable = writer
        .durable_barrier()
        .map_err(|error| gmail::GmailError::Io(error.to_string()))?;
    let location = durable.entries()[0].reference().location.clone();
    let (_, connection) = session.parts_mut();
    let mut canonical_message_id = canonical;
    before_catalogue_publish(connection, &mut canonical_message_id)
        .map_err(|error| gmail::GmailError::Other(error.to_string()))?;
    let published = session
        .publish_gmail_recovery(
            doc_id,
            &record.source_account,
            &record.gmail_id,
            &canonical_message_id,
            &record.location,
            &record.expected_blake3,
            &pending,
            &durable,
        )
        .map_err(|error| gmail::GmailError::Other(error.to_string()))?;
    if !published {
        return Ok(GmailRecoveryResult::RecoveryConflict);
    }
    Ok(GmailRecoveryResult::Recovered { doc_id, location })
}

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
    use crate::gmail::{
        GmailError, GmailTransport, HistoryPage, ListPage, MetadataMessage, Profile, RawMessage,
    };
    use crate::{ArchiveSession, CatalogueBatchRecord, Message};
    use base64::Engine;
    use rusqlite::Connection;
    use std::fs;

    struct FakeGmail {
        response: Result<Vec<u8>, GmailError>,
        fetched_id: Option<String>,
        authenticated_account: Option<String>,
        returned_id: Option<String>,
    }

    impl GmailTransport for FakeGmail {
        fn list(&mut self, _: Option<&str>, _: Option<&str>) -> Result<ListPage, GmailError> {
            Ok(ListPage::default())
        }
        fn get_raw(&mut self, id: &str) -> Result<RawMessage, GmailError> {
            self.fetched_id = Some(id.into());
            self.response.clone().map(|bytes| RawMessage {
                id: self.returned_id.clone().unwrap_or_else(|| id.into()),
                thread_id: String::new(),
                label_ids: Vec::new(),
                history_id: None,
                internal_date: None,
                raw: base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(bytes),
            })
        }
        fn get_metadata(&mut self, _: &str) -> Result<MetadataMessage, GmailError> {
            unreachable!()
        }
        fn profile(&mut self) -> Result<Profile, GmailError> {
            Ok(Profile {
                history_id: "fixture-history".into(),
                email_address: self.authenticated_account.clone(),
            })
        }
        fn history(&mut self, _: &str, _: Option<&str>) -> Result<HistoryPage, GmailError> {
            unreachable!()
        }
    }

    impl Clone for GmailError {
        fn clone(&self) -> Self {
            match self {
                GmailError::Config(value) => GmailError::Config(value.clone()),
                GmailError::Http(value) => GmailError::Http(*value),
                GmailError::HistoryExpired => GmailError::HistoryExpired,
                GmailError::Json(value) => GmailError::Json(value.clone()),
                GmailError::Io(value) => GmailError::Io(value.clone()),
                GmailError::Other(value) => GmailError::Other(value.clone()),
            }
        }
    }

    type SidecarSnapshot = (String, Option<(u64, [u8; 32])>);

    #[derive(Clone, Debug, Eq, PartialEq)]
    struct ArchiveSnapshot {
        segments: Vec<(String, u64, [u8; 32])>,
        catalogue: Vec<u8>,
        sidecars: Vec<SidecarSnapshot>,
    }

    fn archive_snapshot(root: &std::path::Path) -> ArchiveSnapshot {
        let mut segments = fs::read_dir(root.join("archive"))
            .unwrap()
            .filter_map(Result::ok)
            .filter(|entry| entry.file_name().to_string_lossy().ends_with(".arc"))
            .map(|entry| {
                let bytes = fs::read(entry.path()).unwrap();
                (
                    entry.file_name().to_string_lossy().into_owned(),
                    bytes.len() as u64,
                    *blake3::hash(&bytes).as_bytes(),
                )
            })
            .collect::<Vec<_>>();
        segments.sort_by(|left, right| left.0.cmp(&right.0));
        ArchiveSnapshot {
            segments,
            catalogue: fs::read(root.join("metadata.sqlite")).unwrap(),
            sidecars: [
                "metadata.sqlite-wal",
                "metadata.sqlite-shm",
                "metadata.sqlite-journal",
            ]
            .into_iter()
            .map(|name| {
                let path = root.join(name);
                let state = fs::read(&path)
                    .ok()
                    .map(|bytes| (bytes.len() as u64, *blake3::hash(&bytes).as_bytes()));
                (name.into(), state)
            })
            .collect(),
        }
    }

    fn table_snapshot(
        connection: &Connection,
        sql: &str,
        columns: usize,
    ) -> Vec<Vec<rusqlite::types::Value>> {
        connection
            .prepare(sql)
            .unwrap()
            .query_map([], |row| {
                (0..columns)
                    .map(|index| row.get(index))
                    .collect::<rusqlite::Result<Vec<rusqlite::types::Value>>>()
            })
            .unwrap()
            .map(Result::unwrap)
            .collect()
    }

    fn gmail_missing_fixture(label: &str, state: &str) -> (std::path::PathBuf, Vec<u8>) {
        let root = std::env::temp_dir().join(format!(
            "atlas-recovery-exact-{label}-{}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&root);
        let (root, location) = published_fixture(label);
        let raw = b"From: fixture@example.test\r\n\r\nbody".to_vec();
        let source_account = gmail::gmail_source_account("account-a");
        let message_identity = gmail::gmail_message_identity(&source_account, "gmail-1");
        let connection = Connection::open(root.join("metadata.sqlite")).unwrap();
        connection
            .execute(
                "UPDATE messages SET message_id=?1 WHERE doc_id=1",
                [message_identity],
            )
            .unwrap();
        connection.execute("INSERT INTO gmail_messages(source_account,gmail_message_id,doc_id,thread_id,label_ids,source_state,first_seen_unix,last_seen_unix) VALUES (?1,'gmail-1',1,'thread','[]',?2,0,0)", rusqlite::params![source_account, state]).unwrap();
        drop(connection);
        fs::remove_file(root.join("archive").join(location.segment)).unwrap();
        (root, raw)
    }

    #[test]
    fn disposition_labels_are_stable() {
        assert_eq!(RecoveryDisposition::SalvageOnly.label(), "salvage");
        assert!(!RecoveryDisposition::NoAction.label().is_empty());
    }

    #[test]
    fn exact_gmail_recovery_publishes_byte_exact_raw() {
        let (root, raw) = gmail_missing_fixture("exact", "present");
        let old_location = crate::inventory_records(&root).unwrap()[0]
            .location
            .clone()
            .unwrap();
        let mut transport = FakeGmail {
            response: Ok(raw.clone()),
            fetched_id: None,
            authenticated_account: Some("account-a".into()),
            returned_id: None,
        };
        let result = recover_missing_gmail_raw(&root, 1, &mut transport, 4096).unwrap();
        assert!(matches!(result, GmailRecoveryResult::Recovered { .. }));
        assert_eq!(transport.fetched_id.as_deref(), Some("gmail-1"));
        assert_eq!(crate::read_archived_raw(&root, 1).unwrap(), raw);
        assert!(matches!(
            crate::inventory_records(&root).unwrap()[0].status,
            RecordInventoryStatus::AvailableValidated
        ));
        let repaired = crate::inventory_records(&root).unwrap()[0]
            .location
            .clone()
            .unwrap();
        assert_ne!(repaired, old_location);
        assert!(crate::inventory_physical(&root)
            .unwrap()
            .frames
            .iter()
            .any(|frame| {
                frame.location == repaired
                    && frame.status == crate::PhysicalFrameStatus::CataloguedValidated
            }));
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn mismatch_404_network_and_deleted_are_fail_closed() {
        for (label, state, response, expected_fetch) in [
            ("mismatch", "present", Ok(b"changed".to_vec()), true),
            ("missing", "present", Err(GmailError::Http(404)), true),
            (
                "network",
                "present",
                Err(GmailError::Other("network".into())),
                true,
            ),
            (
                "deleted",
                "deleted",
                Ok(b"From: fixture@example.test\r\n\r\nbody".to_vec()),
                false,
            ),
        ] {
            let (root, _) = gmail_missing_fixture(label, state);
            let before = archive_snapshot(&root);
            let mut transport = FakeGmail {
                response,
                fetched_id: None,
                authenticated_account: Some("account-a".into()),
                returned_id: None,
            };
            let result = recover_missing_gmail_raw(&root, 1, &mut transport, 4096);
            if label == "network" {
                assert!(result.is_err());
            } else if label == "missing" {
                assert_eq!(result.unwrap(), GmailRecoveryResult::SourceUnavailable);
            } else if label == "deleted" {
                assert!(matches!(
                    result.unwrap(),
                    GmailRecoveryResult::UnsafeInconsistent(_)
                ));
            } else {
                assert!(matches!(
                    result.unwrap(),
                    GmailRecoveryResult::SourceContentChanged { .. }
                ));
            }
            assert_eq!(transport.fetched_id.is_some(), expected_fetch);
            assert_eq!(archive_snapshot(&root), before);
            let _ = fs::remove_dir_all(root);
        }
    }

    #[test]
    fn recovery_refuses_ambiguous_or_inconsistent_records_without_fetch() {
        let (root, _) = gmail_missing_fixture("ambiguous", "present");
        let connection = Connection::open(root.join("metadata.sqlite")).unwrap();
        connection.execute("INSERT INTO imap_messages(source_account,mailbox,uid_validity,uid,doc_id,flags,source_state,first_seen_unix,last_seen_unix) VALUES ('account-b','INBOX',42,9,1,'[]','present',0,0)", []).unwrap();
        drop(connection);
        let mut transport = FakeGmail {
            response: Ok(Vec::new()),
            fetched_id: None,
            authenticated_account: Some("account-a".into()),
            returned_id: None,
        };
        let result = recover_missing_gmail_raw(&root, 1, &mut transport, 4096).unwrap();
        assert!(matches!(result, GmailRecoveryResult::UnsafeInconsistent(_)));
        assert!(transport.fetched_id.is_none());
        let _ = fs::remove_dir_all(root);

        let (root, location) = published_fixture("inconsistent");
        let source_account = gmail::gmail_source_account("account-a");
        let message_identity = gmail::gmail_message_identity(&source_account, "gmail-1");
        let connection = Connection::open(root.join("metadata.sqlite")).unwrap();
        connection
            .execute(
                "UPDATE messages SET message_id=?1, frame_bytes=?2 WHERE doc_id=1",
                rusqlite::params![message_identity, location.frame_bytes as i64 + 1],
            )
            .unwrap();
        connection.execute("INSERT INTO gmail_messages(source_account,gmail_message_id,doc_id,thread_id,label_ids,source_state,first_seen_unix,last_seen_unix) VALUES (?1,'gmail-1',1,'thread','[]','present',0,0)", [&source_account]).unwrap();
        drop(connection);
        let before = archive_snapshot(&root);
        let mut transport = FakeGmail {
            response: Ok(Vec::new()),
            fetched_id: None,
            authenticated_account: Some("account-a".into()),
            returned_id: None,
        };
        assert!(matches!(
            recover_missing_gmail_raw(&root, 1, &mut transport, 4096).unwrap(),
            GmailRecoveryResult::UnsafeInconsistent(_)
        ));
        assert!(transport.fetched_id.is_none());
        assert_eq!(archive_snapshot(&root), before);
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn account_and_returned_id_mismatches_stop_before_archive_write() {
        let (root, _) = gmail_missing_fixture("wrong-account", "present");
        let before = archive_snapshot(&root);
        let mut transport = FakeGmail {
            response: Ok(Vec::new()),
            fetched_id: None,
            authenticated_account: Some("account-b".into()),
            returned_id: None,
        };
        assert_eq!(
            recover_missing_gmail_raw(&root, 1, &mut transport, 4096).unwrap(),
            GmailRecoveryResult::AccountMismatch
        );
        assert!(transport.fetched_id.is_none());
        assert_eq!(archive_snapshot(&root), before);
        let _ = fs::remove_dir_all(root);

        let (root, raw) = gmail_missing_fixture("wrong-response-id", "present");
        let mut transport = FakeGmail {
            response: Ok(raw),
            fetched_id: None,
            authenticated_account: Some("account-a".into()),
            returned_id: Some("gmail-2".into()),
        };
        let before = archive_snapshot(&root);
        assert!(matches!(
            recover_missing_gmail_raw(&root, 1, &mut transport, 4096).unwrap(),
            GmailRecoveryResult::UnsafeInconsistent(_)
        ));
        assert_eq!(archive_snapshot(&root), before);
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn contradictory_message_identity_is_rejected_before_profile() {
        let (root, _) = gmail_missing_fixture("contradictory-message-id", "present");
        let connection = Connection::open(root.join("metadata.sqlite")).unwrap();
        connection
            .execute(
                "UPDATE messages SET message_id='contradictory' WHERE doc_id=1",
                [],
            )
            .unwrap();
        drop(connection);
        let before = archive_snapshot(&root);
        let mut transport = FakeGmail {
            response: Ok(Vec::new()),
            fetched_id: None,
            authenticated_account: None,
            returned_id: None,
        };
        assert!(matches!(
            recover_missing_gmail_raw(&root, 1, &mut transport, 4096).unwrap(),
            GmailRecoveryResult::UnsafeInconsistent(_)
        ));
        assert!(transport.fetched_id.is_none());
        assert_eq!(archive_snapshot(&root), before);
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn catalogue_conflict_after_durable_append_leaves_fresh_orphan() {
        let (root, raw) = gmail_missing_fixture("cas-conflict", "present");
        let old_segments = fs::read_dir(root.join("archive")).unwrap().count();
        let before_connection = Connection::open(root.join("metadata.sqlite")).unwrap();
        let old_message: (i64, String, String, i64, i64, Vec<u8>) = before_connection
            .query_row(
                "SELECT doc_id,message_id,segment,archive_offset,frame_bytes,raw_blake3
                 FROM messages WHERE doc_id=1",
                [],
                |row| {
                    Ok((
                        row.get(0)?,
                        row.get(1)?,
                        row.get(2)?,
                        row.get(3)?,
                        row.get(4)?,
                        row.get(5)?,
                    ))
                },
            )
            .unwrap();
        let old_gmail: (
            String,
            String,
            i64,
            String,
            String,
            Option<i64>,
            Option<String>,
            String,
        ) = before_connection
            .query_row(
                "SELECT source_account,gmail_message_id,doc_id,thread_id,label_ids,
                            internal_date_ms,message_history_id,source_state
                     FROM gmail_messages WHERE doc_id=1",
                [],
                |row| {
                    Ok((
                        row.get(0)?,
                        row.get(1)?,
                        row.get(2)?,
                        row.get(3)?,
                        row.get(4)?,
                        row.get(5)?,
                        row.get(6)?,
                        row.get(7)?,
                    ))
                },
            )
            .unwrap();
        drop(before_connection);
        let before_connection = Connection::open(root.join("metadata.sqlite")).unwrap();
        let old_messages_full = table_snapshot(
            &before_connection,
            "SELECT doc_id,message_id,timestamp,sender,recipients,subject,account,folder,
                    thread,segment,archive_offset,frame_bytes,raw_blake3
             FROM messages WHERE doc_id=1",
            13,
        );
        let old_gmail_full = table_snapshot(
            &before_connection,
            "SELECT source_account,gmail_message_id,doc_id,thread_id,label_ids,
                    internal_date_ms,message_history_id,source_state,first_seen_unix,last_seen_unix
             FROM gmail_messages WHERE doc_id=1",
            10,
        );
        let old_frontier = table_snapshot(
            &before_connection,
            "SELECT source_account,history_id,complete FROM gmail_state ORDER BY source_account",
            3,
        );
        drop(before_connection);
        let mut transport = FakeGmail {
            response: Ok(raw),
            fetched_id: None,
            authenticated_account: Some("account-a".into()),
            returned_id: None,
        };
        let result =
            recover_missing_gmail_raw_with_hook(&root, 1, &mut transport, 4096, |_, message_id| {
                *message_id = "gmail:stale".into();
                Ok(())
            })
            .unwrap();
        assert_eq!(result, GmailRecoveryResult::RecoveryConflict);
        assert_eq!(
            fs::read_dir(root.join("archive")).unwrap().count(),
            old_segments + 1
        );
        assert_eq!(
            crate::inventory_records(&root).unwrap()[0].status,
            RecordInventoryStatus::PhysicallyMissing
        );
        let physical = crate::inventory_physical(&root).unwrap();
        assert_eq!(physical.orphan_valid_frames, 1);
        let after_connection = Connection::open(root.join("metadata.sqlite")).unwrap();
        let after_message: (i64, String, String, i64, i64, Vec<u8>) = after_connection
            .query_row(
                "SELECT doc_id,message_id,segment,archive_offset,frame_bytes,raw_blake3
                 FROM messages WHERE doc_id=1",
                [],
                |row| {
                    Ok((
                        row.get(0)?,
                        row.get(1)?,
                        row.get(2)?,
                        row.get(3)?,
                        row.get(4)?,
                        row.get(5)?,
                    ))
                },
            )
            .unwrap();
        assert_eq!(after_message, old_message);
        assert_eq!(
            after_connection
                .query_row(
                    "SELECT source_account,gmail_message_id,doc_id,thread_id,label_ids,
                            internal_date_ms,message_history_id,source_state
                     FROM gmail_messages WHERE doc_id=1",
                    [],
                    |row| {
                        Ok((
                            row.get::<_, String>(0)?,
                            row.get::<_, String>(1)?,
                            row.get::<_, i64>(2)?,
                            row.get::<_, String>(3)?,
                            row.get::<_, String>(4)?,
                            row.get::<_, Option<i64>>(5)?,
                            row.get::<_, Option<String>>(6)?,
                            row.get::<_, String>(7)?,
                        ))
                    },
                )
                .unwrap(),
            old_gmail
        );
        assert_eq!(
            table_snapshot(
                &after_connection,
                "SELECT doc_id,message_id,timestamp,sender,recipients,subject,account,folder,
                        thread,segment,archive_offset,frame_bytes,raw_blake3
                 FROM messages WHERE doc_id=1",
                13,
            ),
            old_messages_full
        );
        assert_eq!(
            table_snapshot(
                &after_connection,
                "SELECT source_account,gmail_message_id,doc_id,thread_id,label_ids,
                        internal_date_ms,message_history_id,source_state,first_seen_unix,last_seen_unix
                 FROM gmail_messages WHERE doc_id=1",
                10,
            ),
            old_gmail_full
        );
        assert_eq!(
            table_snapshot(
                &after_connection,
                "SELECT source_account,history_id,complete FROM gmail_state ORDER BY source_account",
                3,
            ),
            old_frontier
        );
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn same_doc_id_orphan_is_not_adopted_by_recovery() {
        let (root, location) = published_fixture("same-doc-orphan-executor");
        let raw = b"From: fixture@example.test\r\n\r\nbody".to_vec();
        let source_account = gmail::gmail_source_account("account-a");
        let message_identity = gmail::gmail_message_identity(&source_account, "gmail-1");
        let connection = Connection::open(root.join("metadata.sqlite")).unwrap();
        connection
            .execute(
                "UPDATE messages SET message_id=?1, segment='segment-999999.arc' WHERE doc_id=1",
                [message_identity],
            )
            .unwrap();
        connection.execute("INSERT INTO gmail_messages(source_account,gmail_message_id,doc_id,thread_id,label_ids,source_state,first_seen_unix,last_seen_unix) VALUES (?1,'gmail-1',1,'thread','[]','present',0,0)", [&source_account]).unwrap();
        drop(connection);
        let mut transport = FakeGmail {
            response: Ok(raw.clone()),
            fetched_id: None,
            authenticated_account: Some("account-a".into()),
            returned_id: None,
        };
        assert!(matches!(
            recover_missing_gmail_raw(&root, 1, &mut transport, 4096).unwrap(),
            GmailRecoveryResult::Recovered { .. }
        ));
        let physical = crate::inventory_physical(&root).unwrap();
        assert_eq!(physical.orphan_valid_frames, 1);
        assert_eq!(crate::read_archived_raw(&root, 1).unwrap(), raw);
        assert_ne!(location.segment, "segment-999999.arc");
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn recovery_is_no_action_when_raw_is_already_available() {
        let (root, location) = published_fixture("already-available");
        let source_account = gmail::gmail_source_account("account-a");
        let message_identity = gmail::gmail_message_identity(&source_account, "gmail-1");
        let connection = Connection::open(root.join("metadata.sqlite")).unwrap();
        connection
            .execute(
                "UPDATE messages SET message_id=?1 WHERE doc_id=1",
                [message_identity],
            )
            .unwrap();
        connection.execute("INSERT INTO gmail_messages(source_account,gmail_message_id,doc_id,thread_id,label_ids,source_state,first_seen_unix,last_seen_unix) VALUES (?1,'gmail-1',1,'thread','[]','present',0,0)", [&source_account]).unwrap();
        drop(location);
        let mut transport = FakeGmail {
            response: Ok(Vec::new()),
            fetched_id: None,
            authenticated_account: Some("account-a".into()),
            returned_id: None,
        };
        assert_eq!(
            recover_missing_gmail_raw(&root, 1, &mut transport, 4096).unwrap(),
            GmailRecoveryResult::AlreadyAvailable
        );
        assert!(transport.fetched_id.is_none());
        let _ = fs::remove_dir_all(root);
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
