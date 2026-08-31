# Memoria

Memoria is a local desktop application for archiving, searching and reading
email. The current product path is centered on Gmail, with a strict read-only
connector and a local archive that remains usable without a network
connection.

The project is under active development. It is a functional desktop
application, not yet a stabilized end-user release.

## Product model

Memoria follows a conservative authority model:

> the original MIME/RAW bytes are the local byte authority; acquisition and
> provenance are recorded separately; parsing, search and presentation remain
> derived unless explicitly defined otherwise.

The current product path is:

```text
Gmail / IMAP / future import sources
              │
              ▼
       acquisition module
              │
              ▼
RAW MIME archive  ← byte-exact local authority
              │
              ├── SQLite catalogue
              │      mixed authority:
              │      physical coordinates, source identities,
              │      source state and navigation metadata
              │
              ├── Tantivy search index
              │      derived / rebuildable
              │
              ▼
Memoria desktop UI / Slint
              │
              ├── MIME parsing and attachment extraction
              ├── system thumbnail / text-extraction helpers
              └── system browser for sanitized HTML
```

A persisted field is not automatically authoritative. The conceptual model for
sources, acquisition and provenance is defined in
[`docs/ARCHITECTURE.md`](docs/ARCHITECTURE.md).

## Current features

- Create or open a Memoria archive from the desktop UI.
- Remember recently used archives and local source configuration between runs.
- Synchronize one Gmail source from the UI without blocking the interface.
- Perform an initial full Gmail synchronization, then incremental
  synchronization through Gmail history and reconciliation when needed.
- Keep locally archived RAW messages when a message is later deleted at Gmail;
  source state changes do not erase the local reference bytes.
- Show synchronization progress and the subsequent derived Tantivy index update
  as separate phases.
- Search with free text and structured filters for sender, recipient, date,
  attachment presence/type and Gmail labels.
- Sort filter-only searches by newest first; textual searches use Tantivy/BM25.
- Read messages in an integrated text reader and inspect their metadata.
- List attachments on demand from the archived RAW, then open or save them
  through desktop integration.
- Export individual messages or displayed search results as byte-exact EML.
- Request image/PDF previews through desktop thumbnail services when available.
- Index supported attachment text as derived search data: `text/*` internally,
  PDF through the optional Linux provider, and PDF/DOCX through registered
  Windows IFilter support where available.
- Open sanitized HTML mail in the system browser without bundling a browser
  engine.
- Use French or English UI text according to the system locale, with Slint
  accessibility and keyboard navigation enabled.
- Import multiple IMAP mailboxes through the experimental read-only CLI. IMAP
  is not yet integrated into the normal desktop source workflow.

## Architecture and authority

The Rust archive/search API is independent from Slint. The UI controller calls
the same archive and synchronization logic used by experimental command-line
tools; it does not launch a CLI subprocess.

- The RAW archive is append-only and segmented. Each validated frame contains
  the original message bytes.
- The logical conservation unit is the individual validated RAW record, not the
  segment that happens to contain it.
- SQLite is a **mixed-authority** catalogue. It contains Tier A coordinates,
  identities and provenance together with mutable source state and lower
  authority navigation data. It is neither the byte authority nor wholly
  disposable in the current design.
- Tier A mutation is **single-writer enforced; multiwriter deliberately
  unsupported**. Competing writers are rejected by an OS-backed archive lock.
- Tantivy, parsed MIME, extracted text, rendered HTML, previews, ranking and UI
  views are derived Tier B/C data. Their failure must not rewrite or replace
  the authoritative RAW.
- Source modules may attest only facts they can actually observe or verify.
  MIME `Message-ID`, sender, subject and similar content are observed message
  content, not automatically acquisition provenance.

The stable authority boundaries are documented in
[`docs/ARCHITECTURE.md`](docs/ARCHITECTURE.md) and the assurance requirements in
[`docs/ASSURANCE.md`](docs/ASSURANCE.md).

## Gmail access and privacy

Gmail access uses the OAuth desktop flow and the single scope:

```text
https://www.googleapis.com/auth/gmail.readonly
```

The current Gmail connector lists messages, downloads them with Gmail
`format=RAW`, stores the decoded RFC/MIME bytes and records Gmail-specific
identity/state separately from the RAW payload.

The application does not delete, trash, modify, label, send, insert or import
anything into Gmail. A future restoration or migration connector with write
capability must be a separate, explicitly authorized capability rather than an
extension of the current read-only contract.

OAuth client credentials and user tokens stay outside the archive and are not
committed to the repository. Memoria does not ship a project OAuth client
secret; developers provide their own local Google Desktop OAuth credentials.

The security threat model and credential/network boundaries are defined in
[`docs/SECURITY.md`](docs/SECURITY.md).

## HTML mail and attachments

The integrated reader remains text-first. When a message has a suitable HTML
part, **Open HTML** opens a sanitized document in the user's default system
browser.

The HTML view is served by an ephemeral localhost server bound only to
`127.0.0.1` on an OS-selected port. Sessions are bounded; opaque session and
resource tokens protect routes. Embedded `cid:` resources are served locally
from the current MIME message. Automatic HTTP/HTTPS resource loading is
blocked. Scripts, event handlers, forms, iframes and objects are neutralized,
and a strict CSP is applied.

Attachments are treated as untrusted content. Opening or saving one is an
explicit user action. Temporary extraction and external thumbnail/text helpers
remain derived operations and do not gain authority over the RAW archive.

No WebKit, WebView2, WebKitGTK, QtWebEngine or Chromium engine is bundled into
Memoria.

## Recovery and integrity status

Recovery is evidence-driven. Ambiguity is preserved as ambiguity rather than
converted into authority by heuristics.

The currently closed baseline includes:

- validated authoritative RAW reads;
- non-destructive use of partially damaged archives;
- coherent publication of source identity/provenance;
- RAW durability before catalogue/source publication;
- Gmail frontier consistency;
- physical inventory and orphan/contradiction classification;
- enforced single-writer authority;
- **R1** read-only recovery planning;
- **R2.1a** exact Gmail re-fetch;
- **R2.1b** exact IMAP re-fetch;
- **R2.2a** byte-exact export of a validated orphan RAW.

Exact source re-fetch requires the provider-specific durable identity and the
historical RAW digest to match; a source returning different bytes does not
silently replace the expected historical RAW. Recovery does not implicitly
advance source frontiers or become a synchronization operation.

R2 remains reserved for bounded recovery actions. The broader persistent
acquisition/provenance model is tracked separately as **M1**, because it also
serves normal acquisition and future EML/MBOX/Outlook/MailStore-style imports.
Its provenance categories are composable per assertion rather than one global
`provenance_level` for a message.

Incomplete-tail cleanup, catalogue relink, catalogue-loss reconstruction and a
complete recovery UI remain future work.

The normative recovery contracts are in
[`docs/RECOVERY.md`](docs/RECOVERY.md). The general conservation guarantees and
closed assurance baseline are in [`docs/ASSURANCE.md`](docs/ASSURANCE.md).

## Search

The search API accepts a structured request rather than a Gmail-like query
string. It supports:

- free text in indexed message fields;
- sender and recipient fragments;
- calendar-date ranges;
- attachment presence;
- exact MIME types and MIME families such as `image/*`;
- labels already present in the archive.

Supplied constraints combine with AND semantics. Selecting several labels
requires all of them. Filter-only searches are ordered by date; textual
searches use BM25 ranking. An empty text query without filters intentionally
does not load the entire archive.

The structured-search path has been exercised with a deterministic synthetic
corpus at million-message scale. Measurements and failed approaches remain in
`experiments/` rather than in this README.

## Building and running

From the workspace root:

```bash
cargo build --release -p mail-archive-experiment --bin mail-archive-app
cargo test --workspace
```

Open an existing archive directly:

```bash
target/release/mail-archive-app --archive /path/to/archive
```

Or start without an explicit archive:

```bash
target/release/mail-archive-app
```

Memoria reopens a valid recent archive when configured. Otherwise it offers to
open an existing archive or create a new one. Invalid folders are not silently
initialized as archives.

The UI can select a local Google Desktop OAuth credentials JSON file when the
user explicitly chooses **Add Gmail account**. Tokens remain in the normal user
configuration area outside the archive.

## Current platform status

### Linux / KDE Wayland

The main desktop workflow has been exercised on Linux/KDE Wayland, including
search, keyboard navigation, archive/synchronization views, attachment actions,
system previews and browser-based HTML opening.

### Windows x86-64

The same crate and Slint sources build for Windows x86-64 MSVC. Native GitHub
Actions build paths exist, including the Windows document-extraction helper
path. Full native validation of menus, HiDPI, UI Automation, dialogs, OAuth and
the interactive workflow still needs to be completed on a real Windows
machine.

macOS and Windows 7 are not current supported product targets.

## Development and tests

The normal workspace checks are:

```bash
cargo fmt --all -- --check
cargo test --workspace
cargo check --workspace
git diff --check
```

Tests use deterministic fixtures for archive frames, MIME parsing, search,
Gmail/IMAP transport behavior, synchronization state, recovery, HTML/CID
sanitization, configuration and UI/controller logic. Real mail data is used
only locally for offline validation and is not part of the repository.

## Project documentation

The repository separates current product description, authority documents,
durable knowledge and experimental evidence:

```text
README.md                         current product state and user-visible limits

docs/AGENTS.md                   working rules and documentation routing
docs/ARCHITECTURE.md             conceptual model and authority boundaries
docs/ASSURANCE.md                A/B/C criticality and conservation guarantees
docs/SECURITY.md                 security threat model and capability policy
docs/RECOVERY.md                 recovery evidence, states and bounded actions
docs/ROADMAP.md                  priorities and dependencies
docs/KNOWLEDGE.md                durable verified technical facts

WORKLOG.md                        lightweight development history
experiments/                      measurements, probes and detailed reports
projects/mail-archive/            current Memoria implementation
```

The specialized documents are authoritative for their own domain. Durable
facts belong in `docs/KNOWLEDGE.md`; detailed measurements, failed attempts and
reproducible probes belong in `experiments/`.

## Roadmap

The current planning horizons are deliberately compact:

```text
NOW    stabilize source/acquisition/provenance boundaries and specify M1
NEXT   finish the minimum recovery product and local imports
THEN   background synchronization, backup, restoration and migration
LATER  storage optimization, richer extraction/search and broader platforms
```

Near-term work therefore focuses first on the persistent acquisition/provenance
model and honest representation of partially known provenance, then on bounded
recovery actions and local source imports. The detailed priority order and exit
criteria live in [`docs/ROADMAP.md`](docs/ROADMAP.md).

## Current limitations

- Gmail is the primary source integrated into the desktop UI. IMAP read-only
  multi-mailbox import exists only through the experimental CLI.
- EML/MBOX/Outlook/MailStore-style sources are represented in the conceptual
  architecture but are not current product integrations.
- There is no complete restoration or write-back workflow to Gmail or another
  provider.
- Recovery does not yet include destructive tail cleanup, general catalogue
  relink, full catalogue-loss reconstruction or a complete recovery UI.
- Attachment text indexing is provider/platform dependent; there is no OCR or
  semantic attachment search.
- Thumbnail support depends on desktop providers installed on the machine.
- Active HTML and automatic remote resources are intentionally disabled.
- The Windows build path exists, but native Windows UX/OAuth validation remains
  incomplete.
- Memoria is not yet a stabilized release with a packaged installer or signed
  distribution.
- The archive contract is local-filesystem single-writer; distributed NFS/SMB
  multiwriter behavior is outside the current contract.
