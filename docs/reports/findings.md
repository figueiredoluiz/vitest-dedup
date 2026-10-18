# Findings: `verdict`, `fingerprint` and structured fields

Machine formats carry both `verdict` and `fingerprint`.

## `verdict`

`verdict` names what the finding claims, because the remedies differ:

| Verdict | Meaning |
| --- | --- |
| `redundant` | One copy can go. |
| `consolidatable` | Every case is needed; they belong in one table. |
| `name-collision` | Two tests share a title. |
| `similar-uncertain` | A review prompt: the tests look equivalent, but that is not established. |

## `fingerprint`

`fingerprint` is a stable identity for one finding across runs: the rule plus the normalized
participants, not file positions, so it survives reformatting and line moves. Baselines match on it.

## Structured fields

Structured fields are provided rather than asking a consumer to parse `message`.

- `inert` carries a per-participant `.skip`/`.todo` flag parallel to `locations` and `titles`, so an
  integration can tell which side of a duplicate is dead code.
- `registrationFiles` is optional. Declaration locations identify source text, and file-scoped rules
  use the file registering each test. When those differ, a finding may carry `registrationFiles`,
  an array parallel to `locations`. Each entry lists the sorted caller paths represented by that
  declaration, including callers collapsed onto one declaration site.
  - SARIF places it in result `properties`; HTML includes it in its JSON data.
  - Without this field, every participant belongs to its source file.
  - Baseline identities include differing caller provenance; ordinary findings retain their
    existing fingerprints.
  - Inline suppressions and HTML snippets refer to declaration source locations.

Registrations that come from called helper functions are described on
[Imported runners](../imported-runners.md).

Back to [Reports](../reports.md).
