# SARIF

SARIF 2.1.0. The log names the published schema in `$schema`.

- Each rule becomes a `reportingDescriptor`; findings become `results` with `partialFingerprints`
  carrying the same `fingerprint` the baseline uses, so GitHub can track a finding across commits.
- Every rule is listed, including rules that are off by default. A rule that is off by default
  carries `defaultConfiguration.enabled: false`. SARIF has no "off" level, so a consumer should
  read `enabled` rather than `level` to learn that a rule does not run by default.
- The descriptors state each rule's built-in default, not the run's `--rule` or configuration
  overrides. A rule enabled for the run can produce results although its descriptor says
  `enabled: false`, and a rule turned off for the run still shows its built-in default. A rule that
  is off for the run is not computed, so it never backs a result.
- `verdict` travels in a result property, since SARIF has no field for it.
- An incomplete run sets `invocations[0].properties.incomplete`. SARIF does not carry the counted
  causes; use the `json` or `jsonl` report for those.

Back to [Reports](../reports.md).
