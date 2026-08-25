# Memoria

Memoria is a local desktop application for archiving, searching and reading
email. The current product path is centered on Gmail, with a strict read-only
connector and a local archive that remains usable without a network
connection.

The project is still under active development. It is a functional desktop
application, not yet a stabilized end-user release.

## What it does

Memoria is designed around a conservative rule: the original MIME/RAW message
is the local source of truth. Search indexes, catalogues and rendered views are
derived data that can be rebuilt or replaced without rewriting the archived
message.

The current end-to-end workflow is:

```text
Gmail (read-only)
      │
      ▼
RAW MIME archive  ← durable local source of truth
      │
      ├── SQLite catalogue
      ├── Tantivy search index
      │
      ▼
Memoria desktop UI / Slint
      │
      ├── system thumbnail services
      └── system browser for sanitized HTML
```

The goal is durable local archiving and fast local search. The RAW archive is
the byte authority; the SQLite catalogue has mixed authority, while search
indexes and rendered views are derived. Individual and batch
byte-exact EML export are available; complete restoration or provider migration
remain future work. Gmail is never treated as a writable target by the current
connector.

## Current features

- Create or open a Memoria archive from the desktop UI.
- Remember recently used archives and local source configuration between runs.
- Synchronize one Gmail source from the UI without blocking the interface.
- Perform an initial full synchronization, then incremental synchronization
  through Gmail history and reconciliation when needed.
- Keep locally archived RAW messages when a message is later deleted at Gmail;
  the source state is updated in the catalogue instead.
- Show synchronization progress, newly archived message counts and the
  subsequent Tantivy index update as separate phases.
- Search the archive with free text and structured filters for sender,
  recipient, date range, attachment presence, attachment MIME and Gmail
  labels. Filters combine with AND semantics.
- Sort filter-only searches by newest message first; textual searches use
  Tantivy/BM25 ranking.
- Read messages in an integrated text reader and inspect their metadata.
- List attachments on demand from the archived MIME, then open them with the
  associated desktop application or save them through a native dialog.
- Export an individual message or the displayed search results as byte-exact
  EML files.
- Request image and PDF previews through desktop thumbnail services when
  available.
- Use French or English UI text according to the system locale, with Slint
  accessibility and keyboard navigation enabled.
- Import IMAP readonly mailboxes, including multiple mailboxes, through the
  experimental CLI; this is not integrated into the product UI workflow.

## Architecture

The Rust archive/search API is independent from Slint. The UI controller calls
the same archive and synchronization logic used by the experimental command
line tools; it does not start a CLI subprocess.

- The archive is append-only and segmented. Each validated frame contains the
  original message bytes.
- SQLite is a mixed-authority catalogue: it stores Tier A coordinates,
  identities and provenance needed for lookup and reconciliation, alongside
  mutable source state and derived navigation metadata. It is not itself the
  byte authority and is not wholly disposable in the current design.
- Tantivy stores a derived, reconstructible search index. Its schema may evolve
  independently from the RAW archive.
- MIME parsing, text extraction, attachment listing and HTML rendering are
  derived operations. They never replace the RAW representation.
- System thumbnail helpers and the system browser are optional presentation
  integrations. Their failure must not make the archive unreadable.

## Gmail access and privacy

Gmail access uses the OAuth desktop flow and the single scope:

```text
https://www.googleapis.com/auth/gmail.readonly
```

The connector lists messages, downloads them with Gmail `format=RAW`, stores
the decoded RFC/MIME bytes, and keeps Gmail message/thread/label/history
metadata separately in SQLite. A later synchronization refreshes source
metadata and archives only messages that are not already known by the stable
`source/account + Gmail message id` identity.

The application does not delete, trash, modify, label, send, insert or import
anything into Gmail. OAuth client credentials and user tokens stay outside the
archive and are never committed to the repository. Memoria does not ship a
project OAuth client secret: developers must provide their own Google Desktop
OAuth credentials locally.

## Building and running

From the workspace root:

```bash
cargo build --release -p mail-archive-experiment --bin mail-archive-app
cargo test --workspace
```

To open an existing archive directly:

```bash
target/release/mail-archive-app --archive /path/to/archive
```

Memoria can also start without `--archive`:

```bash
target/release/mail-archive-app
```

It reopens a valid recent archive when one is configured. Otherwise it shows
actions to open an existing archive or create a new empty one. An archive is
recognized by its Memoria metadata catalogue and archive directory; invalid
folders are not initialized implicitly.

The UI can select a local Google Desktop OAuth credentials JSON file when the
user explicitly chooses **Add Gmail account**. The token directory is kept in
the standard user configuration area and outside the archive. No real
credentials belong in this repository.

The package also contains experimental command-line tools for corpus
generation, Gmail reporting, indexing and thumbnail probing. Their detailed
usage belongs in the experiment reports rather than this introduction.

## Desktop integration

Memoria uses Slint with the Winit backend, software rendering and accessibility
enabled. It does not embed a web browser engine.

For attachment previews, the application asks desktop thumbnail services from
a background task with a timeout. On KDE/Linux it tries the small KIO helper
and then the freedesktop thumbnail backend; no Qt or KF6 library is linked into
the main Memoria binary. Providers are optional and an unavailable preview
does not remove the **Open** or **Save as** actions.

Opening an attachment uses the operating system association API. Temporary
extraction files are private to the Memoria process and are removed with the
session store when the application exits.

## Search

The search API accepts a structured request rather than encoding advanced
filters into a Gmail-like query string. It supports:

- free text in the indexed message fields;
- sender and recipient fragments;
- date ranges entered as calendar dates;
- all, with attachments or without attachments;
- exact MIME filters such as `application/pdf` and MIME families such as
  `image/*`;
- labels already present in the archive.

All supplied constraints are ANDed. Selecting several labels requires all of
those labels for the first product version. An empty text query without filters
intentionally does not load the entire archive.

The structured search implementation has been exercised with a deterministic
synthetic corpus at million-message scale; measurements and limits are recorded
under `experiments/`.

## HTML mail and attachments

The integrated reader remains text-first. When a message has a suitable HTML
part, **Open HTML** opens the sanitized document in the user's default system
browser.

The HTML view is served by an ephemeral localhost server scoped to the Memoria
process, bound only to `127.0.0.1` on an OS-selected port. Its sessions are
bounded and expire; opaque session and resource tokens protect the routes.
Embedded `cid:` resources are served locally from the MIME message, including
their MIME type. Automatic HTTP/HTTPS image loading is blocked. Scripts, event
handlers, forms, iframes and objects are neutralized, and a strict CSP is
applied. No WebKit, WebView2, WebKitGTK, QtWebEngine or Chromium engine is
bundled into Memoria.

Attachments are extracted only when requested from the authoritative RAW for
opening/saving. During a derived Tantivy rebuild, `text/*` attachment parts
and supported document attachments are also processed for search: text parts
are decoded internally. On Linux, PDFs use the optional `pdftotext` provider;
DOCX is not supported. On Windows, PDF and DOCX use the registered system
IFilter through an isolated helper. Word COM Automation is not a Memoria
dependency.
Extracted text is never stored as a second authoritative attachment copy.

## Storage and recovery model

The RAW archive is authoritative and append-only. Tantivy and rendered/search
views are derived. SQLite is a mixed-authority catalogue, as described above:

- the demonstrated safe case is an incomplete tail after the last valid frame;
  `recover_segments` currently stops at the first invalid frame and truncates
  the segment suffix, so central corruption can discard valid later frames and
  remains a Tier A debt;
- a missing or incompatible Tantivy index can be reconstructed from the RAW
  archive and catalogue;
- Gmail source deletion changes local source metadata but does not erase the
  archived RAW message;
- attachment parsing, HTML rendering and previews can fail independently
  without changing the archived bytes.

These are the recovery capabilities currently implemented and exercised. They
do not claim all Tier A guarantees defined in
[`ASSURANCE.md`](ASSURANCE.md), including complete crash-consistency,
central-corruption recovery, partial-archive operation or a multi-writer
contract.

The archive is intentionally not a mirror that destructively follows Gmail.
Content-addressed attachment storage was evaluated separately and is not
adopted in the current RAW-inline authority path.

## Current platform status

### Linux / KDE Wayland

The main desktop workflow has been exercised on Linux/KDE Wayland, including
search, keyboard navigation, archive/synchronization views, attachment
actions, system previews and browser-based HTML opening.

### Windows x86-64

The same crate and Slint sources build for Windows x86-64 MSVC. The repository
contains the native GitHub Actions workflow and standard/static CRT build
artifacts. Cross-build and CI build paths are available; native Windows
validation of menus, HiDPI, UI Automation, native dialogs, OAuth and the full
interactive workflow still needs to be performed on a real Windows machine.

macOS and Windows 7 are not currently supported targets.

## Development / tests

The normal workspace checks are:

```bash
cargo fmt --all -- --check
cargo test --workspace
cargo check --workspace
git diff --check
```

The tests use deterministic fixtures for archive frames, MIME parsing,
structured search, Gmail transport behavior, synchronization state, HTML/CID
sanitization, configuration and UI/controller logic. Real Gmail data is used
only locally for offline validation and is not part of the repository.

## Project documentation

The repository keeps the durable memory separate from detailed experiments:

```text
AGENTS.md                         working rules and experiment discipline
KNOWLEDGE.md                      durable facts and architectural conclusions
WORKLOG.md                        lightweight development journal
experiments/                      measurements, probes and detailed reports
projects/mail-archive/            current Memoria implementation
```

`KNOWLEDGE.md` is deliberately concise. The reports under `experiments/` hold
the measurements, failed attempts and reproducible commands that support its
conclusions.

## Roadmap

The next product questions are deliberately limited to durable archive use:

- extend bounded attachment-text indexing beyond the current text/*, PDF and
  DOCX support, starting with formats justified by the real corpus;
- consider automatic/background synchronization after the manual workflow is
  stable;
- support complete restoration and Gmail-to-Gmail migration workflows;
- validate and package the native Windows desktop workflow;
- support multiple Gmail accounts in the product UI;
- add additional local sources such as MBOX when their requirements are known;
- improve long-term offline retention and recovery operations.

These are future directions, not current promises.

## Current limitations

- Gmail is the primary source integrated into the product UI today. IMAP
  readonly multi-mailbox import exists as an experimental CLI capability, but
  other providers are not integrated into the UI workflow.
- There is no complete restoration or write-back workflow to Gmail; individual
  and batch byte-exact EML export are already available.
- Attachment text indexing currently covers text/* parts, PDF where the
  platform provider is available, and DOCX through the registered Windows
  IFilter. Linux DOCX and other Office formats are not indexed; there is no
  OCR or semantic attachment search.
- Thumbnail support depends on desktop providers installed on the machine.
- Active HTML and automatic remote resources are intentionally disabled.
- The Windows build path exists, but native Windows UX/OAuth validation remains
  open.
- Memoria is not yet a stabilized release with a packaged installer or signed
  distribution.
