# Synthetic MDN/DSN corpus and mailparse probe

Date: 2026-08-24

## Scope and normative baseline

This is an experimental corpus only. It does not change Memoria, its RAW
format, SQLite, Tantivy, or any product schema.

The fixtures follow the structural rules of [RFC 8098](https://www.rfc-editor.org/rfc/rfc8098.html)
for message disposition notifications and [RFC 3464](https://www.rfc-editor.org/rfc/rfc3464.html)
for delivery status notifications. [RFC 6533](https://www.rfc-editor.org/rfc/rfc6533.html)
is represented by prospective fixtures using the internationalized report
media types; those cases are intentionally not treated as supported by this
probe.

All addresses use `example.invalid`. Message-IDs and boundaries are
deterministic. No real mailbox data, credentials, or personal content is used.

## Corpus

The permanent corpus contains exactly 44 `.eml` files and 44 checked
`.expected.json` golden oracles. They are independent of the mailparse data
model, but are generated from the same deterministic fixture specifications as
the `.eml` files; they are not an independently authored second corpus.
Mutation tests prove that the checker compares changed fixture content with
the unchanged golden values.

| Category | Count | Validity split |
| --- | ---: | --- |
| MDN | 15 | valid |
| DSN | 14 | valid |
| Human-looking negatives and malformed reports | 11 | 4 ordinary valid, 1 unsupported MDN, 2 malformed DSN (incoherent structure and missing `Reporting-MTA`), 1 isolated malformed MDN, 2 malformed reports, 1 malformed DSN with missing required fields |
| RFC 6533 prospective cases | 4 | unsupported (2 DSN, 2 MDN) |

The valid MDNs cover displayed, deleted, processed and dispatched dispositions,
manual and automatic actions, manually and automatically sent MDNs, an error
modifier, absent/present `Original-Recipient`, folded fields, case variation,
unknown extension fields, and absent/present third parts. Every MDN oracle has
at most one recipient. Two cross-cases explicitly cover an original message
with `Message-ID` but no third report part, and a third part without a
`Message-ID`; `Original-Message-ID` follows the original header, not the
presence of that third part.

The valid DSNs cover failed, delayed, delivered, relayed and expanded results, one and multiple
recipients, different outcomes per recipient, optional/or present
`Original-Recipient`, `Final-Recipient`, `Action`, `Status`,
`Diagnostic-Code`, `Remote-MTA`, absent original content, original headers,
and a complete `message/rfc822` part. One targeted fixture keeps the
per-message fields separate from per-recipient blocks and includes
`Original-Envelope-Id`, `Arrival-Date`, `Last-Attempt-Date`,
`Will-Retry-Until`, `Final-Log-ID`, `Received-From-MTA`, and `DSN-Gateway`.
Another explicitly omits the required `Reporting-MTA` field.

The initial `dsn-13` construction was not correct: it put
`Last-Attempt-Date`, `Final-Log-ID`, and `Will-Retry-Until` in the per-message
block. It was corrected so the per-message block contains only
`Original-Envelope-Id`, `Reporting-MTA`, `DSN-Gateway`, `Received-From-MTA`,
and `Arrival-Date`; the per-recipient blocks contain the other fields. A third
recipient is `delayed` and carries `Will-Retry-Until`. The checker rejects a
mutation placing that field on the `relayed` recipient, enforcing the RFC
constraint that it must not occur for other actions.

The negative cases include ordinary messages mentioning delivery/read-receipt
phrases, a wrong `report-type`, an incoherent `multipart/mixed`, isolated
report parts, truncated/broken reports, missing required fields, and unknown
extension fields. The RFC 6533 cases use `message/global-delivery-status`,
`message/global-disposition-notification`, `message/global-headers`, and
`message/global` with deterministic escaped non-ASCII test values.

The mutation matrix is intentional rather than padding: folded fields test
unfolding; casing and whitespace test the case-insensitive/tokenized fields;
optional fields test absent `Original-Recipient`, diagnostic data and third
parts; multiple DSN recipient blocks test per-recipient grouping; and unknown
`X-Corpus-*` fields test forward-compatible ignoring.

The checker has seven mutation tests that keep the original oracle unchanged:
MDN `displayed` → `deleted`; DSN `Action: failed` → `delivered`; removal of
`Final-Recipient`; removal of `Reporting-MTA`; modification of `Status`; and
modification of the second recipient block. Every mutation is rejected by the
semantic comparison. The seventh adds `Will-Retry-Until` to a `relayed`
recipient and is classified malformed.

## Probe and observed mailparse representation

The isolated probe is `experiments/mdn-dsn-corpus-probe/`. It uses
`mailparse 0.16.1` and has three operations:

```text
cargo run --offline --manifest-path experiments/mdn-dsn-corpus-probe/Cargo.toml -- \
  experiments/mdn-dsn-corpus-probe --generate
cargo run --offline --manifest-path experiments/mdn-dsn-corpus-probe/Cargo.toml -- \
  experiments/mdn-dsn-corpus-probe --check
cargo run --offline --manifest-path experiments/mdn-dsn-corpus-probe/Cargo.toml -- \
  experiments/mdn-dsn-corpus-probe --dump-mailparse
```

The checker passes all 44 fixtures:

```text
checked=44 counts={
  "dsn malformed": 4, "dsn unsupported": 2, "dsn valid": 14,
  "mdn malformed": 2, "mdn unsupported": 3, "mdn valid": 15,
  "ordinary valid": 4
}
```

The dump shows the following actual API shape:

- `multipart/report` is exposed as one `ParsedMail` whose `ctype.mimetype` is
  `multipart/report`, whose `ctype.params` contain `report-type` and
  `boundary`, and whose report parts are in `subparts`.
- The human-readable first part is a normal `text/plain` subpart.
- `message/disposition-notification` and `message/delivery-status` are normal
  leaf `ParsedMail` values. Their fields are only available after decoding the
  part body and parsing its field lines; mailparse does not expose an MDN/DSN
  semantic structure.
- DSN recipient blocks remain one decoded body containing blank-separated
  field groups. There is no dedicated per-recipient collection or typed
  `Status`/`Diagnostic-Code` representation.
- A third `message/rfc822` part is exposed as a subpart with decoded body
  bytes. A `text/rfc822-headers` part is likewise just a typed part and body;
  the caller must parse the returned headers. The dump did not expose a
  product-level original-message relation automatically.
- `message/global-*` parts are parsed as MIME types and parameters, but no
  internationalized MDN/DSN semantics are supplied by mailparse.
- All 38 deliberately malformed/prospective fixtures were accepted by
  `parse_mail`; malformedness therefore cannot be inferred from a parse error
  alone. The probe performs the additional structural checks and reports
  missing parts/required fields as malformed or unsupported.

One small normalization is explicit in the probe: a DSN
`Diagnostic-Code: smtp; ...` value is retained as the service plus detail in
the raw MIME observation, while the independent oracle records the diagnostic
detail after the structured `smtp;` prefix. This is a probe comparison rule,
not a product decision.

## Oracle and correlation rules

The JSON oracles are generated from the fixture specifications, not from a
`ParsedMail` tree. They describe intended semantic fields and expected
classification independently of mailparse, with the generation caveat noted
above. The mutation tests demonstrate that the checker compares the parsed
fixture observation to those retained values.

Correlation is deliberately narrow:

- an explicit `Original-Message-ID` is a **strong, explicit structured
  correlation**, never proof of identity or authenticity: an MDN can be forged;
- a `Message-ID` found in a third `message/rfc822` or
  `text/rfc822-headers` part is a strong structured value present in that part,
  but does not by itself prove delivery semantics or authenticity;
- `Original-Envelope-Id` is a structured DSN transaction identifier, distinct
  from `Message-ID` and not an authenticity proof;
- recipient fields and structured original-recipient values are **plausible**
  correlation aids, not identity proofs;
- Subject, prose, “undelivered” wording, and other free text are **impossible**
  bases for authoritative correlation in this experiment.

## Result and limits

**Fact verified:** mailparse is sufficient for the MIME-tree inspection, but it
does not provide a semantic MDN/DSN model. The product parser now supplies the
small layer above `ParsedMail` for report-type validation, MDN field rules,
DSN per-message/per-recipient grouping, and RFC 6533 policy.

**Fact verified:** MDN validation checks `Final-Recipient` and `Disposition`;
`Original-Message-ID` is expected exactly when the original message had a
`Message-ID` header, independently of whether a third report part exists.
`Reporting-UA` is retained but is not required by this validator. DSN
validation checks the per-message `Reporting-MTA` and required per-recipient
fields, including the `Will-Retry-Until` action constraint.

**Fact verified:** the negative corpus demonstrates that human-looking text is
not enough to classify a message as an MDN or DSN.

**Decision:** no product schema, UI, correlation logic, DKIM/SPF verification,
or report ingestion was implemented. This corpus is the oracle for a later
bounded experiment.

**Open limitation:** RFC 6533 fixtures are prospective and classified as
unsupported here; no claim is made about production internationalized-report
support.

## Product-parser hardening

The product parser extends the corpus with two golden fixtures, bringing the
total to 44: one DSN exercises the valid extensible statuses `5.7.26`,
`2.1.23`, and `4.0.0`, and one MDN exercises optional whitespace and comma/
slash-separated modifiers. Direct regression tests cover invalid status
syntax (`9.1.1`, leading zeroes, and overlong components), folded-field
unfolding, both MDN and DSN reports with an incoherent second part, and a
non-parseable MIME document whose bytes merely mention delivery-status.

An unparseable complete MIME input is now reported as `Unparseable { reason }`;
the parser no longer guesses MDN or DSN from text. Structurally recognizable
but invalid reports remain `Malformed` with their known kind.
