# HTML remote-resource evidence corpus

Date: 2026-08-25

## Scope

This is an offline experiment. It observes what an original decoded HTML
document contains; it does not decide that a resource is a tracker and never
fetches an HTTP resource. No Memoria product code, UI, SQLite, or Tantivy code
was changed.

The corpus contains exactly 30 small `.html` fixtures and 30 independent JSON
goldens. All hosts use `example.invalid`. The cases cover local references,
HTTP/HTTPS and protocol-relative images, dimensions, inline visibility styles,
CSS `url(...)`, case/whitespace, entities, percent-encoding, `srcset`, broken
HTML, links, comments, text URLs, CID/data references, and multiple images.

## Current product pipeline

The current product path is:

```text
decoded text/html from MIME
        |
        +-- original HTML: not currently inventoried
        |
        +-- exact cid: normalization and opaque local-route rewrite
        |
        +-- ammonia 4.1.4
        |
        +-- CSP and localhost response
        |
        `-- explicitly opened system browser
```

CID rewriting happens before sanitization so a matched embedded resource is
not lost. The route is local and opaque. The existing product tests verify that
`ammonia` removes scripts, forms, and event-handler attributes while preserving
an external link. The probe's direct sanitizer audit additionally observed:

```text
remote_img_preserved=true
script_preserved=false
form_preserved=false
event_handler_preserved=false
external_link_preserved=true
```

Therefore `ammonia` is not the remote-load barrier by itself: an external image
URL can remain as an inert HTML attribute. The current CSP is the relevant
browser barrier (`default-src 'none'`, `img-src 'self'`, `connect-src 'none'`,
`script-src 'none'`, and related restrictions). The evidence probe runs before
all of these transformations on the original HTML.

## Probe and parser choice

The probe is `experiments/html-remote-evidence-probe/`. It uses
`html5ever 0.39.0` with a minimal TreeSink and `url 2.5.8` for URL structure.
`ammonia 4.1.4` is included only to audit the already-used sanitizer; it is
not added to the product. `markup5ever 0.39.0` is the direct companion needed
by the TreeSink types. No network client or socket code exists in the probe.

`html5ever` was chosen because it provides browser-like tokenization and
recovery for malformed HTML while exposing element attributes. A DOM utility
crate was not added: the small sink stores only element names, attributes,
parent-independent text for `style`, and enough information for this corpus.
CSS is intentionally not parsed as a full stylesheet. The probe extracts
simple `url(...)` occurrences from inline `style` and `<style>` text and
reports that boundary as a limitation.

Reproducible commands:

```text
cargo fmt --manifest-path experiments/html-remote-evidence-probe/Cargo.toml -- --check
cargo check --offline --manifest-path experiments/html-remote-evidence-probe/Cargo.toml
cargo run --offline --manifest-path experiments/html-remote-evidence-probe/Cargo.toml -- \
  experiments/html-remote-evidence-probe/fixtures
cargo run --offline --manifest-path experiments/html-remote-evidence-probe/Cargo.toml -- \
  --audit-sanitizer
```

Observed result: `checked=30 network_fetches=0`.

## Observation model

Each golden contains only deterministic observations:

- `remote_resources`: image or CSS-background, source location, original URL,
  `http`/`https`/`protocol-relative` scheme, host, declared dimensions,
  inline hidden state, and explicit signals;
- `local_references`: `cid:`, `data:`, relative, and fragment references;
- `links`: external `<a href>` URLs, kept separate from automatically loaded
  resources.

Signals currently are only:

- `tiny-dimensions`: a declared width or height is `1`;
- `hidden`: inline `display:none`, `visibility:hidden`, or `opacity:0`;
- `query-parameters`: the URL has a parsed query.

This taxonomy targets image/CSS loads relevant to mail-tracking evidence; it is
not an exhaustive inventory of every network mechanism available to HTML.

There is no `is_tracker`, score, ownership inference, third-party inference,
or entropy classifier. A one-pixel image with `?id=abc` is therefore observed
as an image with those properties, not as a proven tracker.

## Findings and limits

- `cid:`, `data:`, relative URLs, fragments, comments, prose, and external
  links do not become automatic remote resources in this model.
- HTTP, HTTPS, and protocol-relative image URLs are observed without opening
  them. `srcset` candidates are reported separately.
- Dimensions and three simple inline visibility properties are observable;
  computed CSS, viewport positioning, and layout are not.
- CSS `url(...)` is observed in inline styles and simple style blocks, but this
  is not a CSS parser and does not claim full CSS coverage.
- HTML5ever recovers the malformed fixture sufficiently to expose its image;
  recovery behavior is an observation of this parser, not a normative claim
  about every browser.
- URL percent-encoding is preserved in the recorded URL. The probe does not
  decode it into a second semantic URL, avoiding an implicit equivalence rule.

The experiment establishes a useful future boundary: deterministic evidence
can be collected from original HTML independently of sanitization, while any
tracking suspicion policy must remain a separate, explicit layer. It does not
justify changing the current blocking policy or adding product analysis yet.

## Product API follow-up

Memoria now exposes `analyze_html_remote_evidence(&str) -> HtmlRemoteEvidence`
in `html_remote_evidence`. It uses the same 30 fixtures and 30 goldens directly
from product tests. The public result keeps `remote_resources`,
`local_references`, and `links` separate; `RemoteResourceSignal` contains only
the three explicit signals above and has no tracker verdict.

The product adds a direct `html5ever 0.39` dependency. Its transitive
`markup5ever` and the existing `ammonia`/`url` graph were already present, so
no new package family was introduced. The CI-profile application measured
36,993,824 bytes after the change; no before/after attribution was attempted
because this build was not paired with a clean pre-change binary.

The product corpus tests pass, including a deterministic malformed-input
exploration loop. Full workspace validation is otherwise green except for the
pre-existing HTML preview test that writes under the read-only `/var/tmp`
mount in this environment.

The product TreeSink owns qualified names inside `Rc<RefCell<Node>>` handles;
it does not use `Box::leak`. A repeated 1,000-image document analyzed 20 times
kept a stable observation count, and a `Weak` handle confirmed that the sink's
name storage is released after analysis. A malformed table/formatting fixture
that exercises html5ever reconstruction produced exactly two image
observations, with no duplicate generated resource entries.
