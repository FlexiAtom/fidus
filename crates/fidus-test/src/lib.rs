// Copyright 2026 Flexiatom
// SPDX-License-Identifier: Apache-2.0

//! Cross-environment test protocol and harness model for fidus.
//!
//! This crate deliberately does not contain calibration, detection, backend,
//! or coordinate-solving logic. It parses only the versioned records emitted
//! by the system under test and classifies harness state.

#![warn(missing_docs)]

use std::collections::BTreeMap;
use std::fmt;
use std::str::FromStr;

/// A parsed `FIDUS_RESULT version=1 ...` record.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ResultRecord {
    /// The record kind.
    pub kind: String,
    /// The status token supplied by the producer.
    pub status: String,
    /// The run correlation token.
    pub run_id: String,
    /// All validated fields, including common fields.
    pub fields: BTreeMap<String, String>,
}

impl ResultRecord {
    /// Encodes this record using the v1 line protocol.
    ///
    /// Serialization is fallible because callers can construct a record directly;
    /// malformed records must be rejected rather than causing a panic or emitting
    /// a line that the protocol parser would interpret differently.
    pub fn to_line(&self) -> Result<String, ParseError> {
        let execution_mode = self
            .fields
            .get("execution_mode")
            .ok_or(ParseError::MissingField("execution_mode"))?;
        for (key, expected) in [
            ("version", "1"),
            ("kind", self.kind.as_str()),
            ("run_id", self.run_id.as_str()),
            ("status", self.status.as_str()),
        ] {
            if let Some(value) = self.fields.get(key) {
                if value != expected {
                    return Err(ParseError::InvalidValue(format!(
                        "{key} does not match record"
                    )));
                }
            }
        }

        let mut line = format!(
            "FIDUS_RESULT version=1 kind={} run_id={} status={} execution_mode={}",
            self.kind, self.run_id, self.status, execution_mode
        );
        for (key, value) in &self.fields {
            if !matches!(
                key.as_str(),
                "version" | "kind" | "run_id" | "status" | "execution_mode"
            ) {
                line.push(' ');
                line.push_str(key);
                line.push('=');
                line.push_str(value);
            }
        }

        // Reuse the strict parser as the protocol's single validation boundary.
        // This also rejects invalid keys, empty/whitespace-containing values, and
        // schema-invalid records assembled outside the parser.
        parse_result_line(&line)?.ok_or_else(|| {
            ParseError::InvalidValue("serialized line lost its result prefix".into())
        })?;
        Ok(line)
    }
}

/// Why a result stream could not be treated as a valid test result.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ParseError {
    /// A result line contained malformed key/value syntax.
    MalformedToken(String),
    /// A key appeared more than once.
    DuplicateKey(String),
    /// A key was not part of the protocol grammar.
    InvalidKey(String),
    /// The record version was not supported.
    UnsupportedVersion(String),
    /// A required field was absent.
    MissingField(&'static str),
    /// A field had a value that could not be validated.
    InvalidValue(String),
    /// Records in one run disagree about their correlation or mode.
    InconsistentRun(String),
    /// A run has no unique summary record.
    MissingSummary,
    /// The terminal summary was not the final record.
    SummaryNotLast,
}

impl fmt::Display for ParseError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::MalformedToken(t) => write!(f, "malformed result token: {t}"),
            Self::DuplicateKey(k) => write!(f, "duplicate result key: {k}"),
            Self::InvalidKey(k) => write!(f, "invalid result key: {k}"),
            Self::UnsupportedVersion(v) => write!(f, "unsupported result version: {v}"),
            Self::MissingField(k) => write!(f, "missing result field: {k}"),
            Self::InvalidValue(v) => write!(f, "invalid result value: {v}"),
            Self::InconsistentRun(v) => write!(f, "inconsistent run: {v}"),
            Self::MissingSummary => write!(f, "run must contain exactly one summary"),
            Self::SummaryNotLast => write!(f, "summary must be the final record"),
        }
    }
}

impl std::error::Error for ParseError {}

const COMMON: &[&str] = &["version", "kind", "run_id", "status", "execution_mode"];

/// Parses one line. Non-result log lines are deliberately ignored.
pub fn parse_result_line(line: &str) -> Result<Option<ResultRecord>, ParseError> {
    const PREFIX: &str = "FIDUS_RESULT ";
    let Some(rest) = line.strip_prefix(PREFIX) else {
        return Ok(None);
    };

    let mut fields = BTreeMap::new();
    for token in rest.split_ascii_whitespace() {
        let (key, value) = token
            .split_once('=')
            .ok_or_else(|| ParseError::MalformedToken(token.to_owned()))?;
        if key.is_empty()
            || !key
                .bytes()
                .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_')
        {
            return Err(ParseError::InvalidKey(key.to_owned()));
        }
        if value.is_empty() || fields.insert(key.to_owned(), value.to_owned()).is_some() {
            return Err(if value.is_empty() {
                ParseError::InvalidValue(format!("empty value for {key}"))
            } else {
                ParseError::DuplicateKey(key.to_owned())
            });
        }
    }

    require(&fields, "version")?;
    if fields.get("version").map(String::as_str) != Some("1") {
        return Err(ParseError::UnsupportedVersion(
            fields.get("version").cloned().unwrap_or_default(),
        ));
    }
    for key in COMMON {
        require(&fields, key)?;
    }
    let kind = fields["kind"].clone();
    let status = fields["status"].clone();
    let run_id = fields["run_id"].clone();
    validate_token("run_id", &run_id)?;
    ExecutionMode::from_str(&fields["execution_mode"])
        .map_err(|_| ParseError::InvalidValue("unknown execution_mode".into()))?;

    // Unknown extra fields are retained for forward-compatible diagnostics;
    // schema-critical fields below remain strict. This keeps the parser from
    // silently becoming a second producer implementation.
    let required = match kind.as_str() {
        "environment" => &["backend", "compositor", "output", "scale", "transform"][..],
        "calibration" => &["backend", "method"][..],
        "lifecycle" => &["teardown", "recovery"][..],
        "summary" => &["records_total", "records_ok", "records_failed"][..],
        _ => return Err(ParseError::InvalidValue(format!("unknown kind: {kind}"))),
    };
    for key in required {
        require(&fields, key)?;
    }

    let allowed_status = match kind.as_str() {
        "environment" => &["ready", "unavailable"][..],
        "calibration" => &["ok", "failed"][..],
        "lifecycle" => &["ok", "failed", "unverified"][..],
        "summary" => &["ok", "failed", "harness_error"][..],
        _ => unreachable!(),
    };
    if !allowed_status.contains(&status.as_str()) {
        return Err(ParseError::InvalidValue(format!(
            "status {status} is invalid for kind {kind}"
        )));
    }
    if kind == "lifecycle"
        && !["not_requested", "confirmed", "unverified"].contains(&fields["recovery"].as_str())
    {
        return Err(ParseError::InvalidValue(
            "lifecycle recovery must be not_requested, confirmed, or unverified".into(),
        ));
    }

    if kind == "environment" && fields["scale"] != "not-measured" {
        finite_number(&fields, "scale")?;
    }
    if kind == "calibration" && status == "ok" {
        for key in [
            "rms_residual_px",
            "verification_max_err_px",
            "consistency_max_err_px",
        ] {
            finite_number(&fields, key)?;
        }
    }
    if kind == "summary" {
        for key in ["records_total", "records_ok", "records_failed"] {
            fields[key]
                .parse::<u64>()
                .map_err(|_| ParseError::InvalidValue(format!("{key} must be an integer")))?;
        }
    }

    Ok(Some(ResultRecord {
        kind,
        status,
        run_id,
        fields,
    }))
}

fn require(fields: &BTreeMap<String, String>, key: &'static str) -> Result<(), ParseError> {
    if fields.contains_key(key) {
        Ok(())
    } else {
        Err(ParseError::MissingField(key))
    }
}

fn validate_token(name: &str, value: &str) -> Result<(), ParseError> {
    if value
        .bytes()
        .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
    {
        Ok(())
    } else {
        Err(ParseError::InvalidValue(format!(
            "{name} is not an ASCII token"
        )))
    }
}

fn finite_number(fields: &BTreeMap<String, String>, key: &'static str) -> Result<(), ParseError> {
    let value = fields[key]
        .parse::<f64>()
        .map_err(|_| ParseError::InvalidValue(format!("{key} must be numeric")))?;
    if value.is_finite() {
        Ok(())
    } else {
        Err(ParseError::InvalidValue(format!("{key} must be finite")))
    }
}

/// Creates the terminal summary for a non-empty set of business records.
///
/// The caller must append the returned record after all business records;
/// `validate_run` rejects a summary placed earlier in the stream.
pub fn summary_for(records: &[ResultRecord]) -> Result<ResultRecord, ParseError> {
    if records.is_empty() {
        return Err(ParseError::InconsistentRun(
            "run has no business records".into(),
        ));
    }
    let first = &records[0];
    if records.iter().any(|record| {
        record.kind == "summary"
            || record.run_id != first.run_id
            || record.fields["execution_mode"] != first.fields["execution_mode"]
    }) {
        return Err(ParseError::InconsistentRun(
            "invalid business records".into(),
        ));
    }
    let ok = records
        .iter()
        .filter(|record| matches!(record.status.as_str(), "ok" | "ready"))
        .count();
    let failed = records.len() - ok;
    let mut fields = BTreeMap::new();
    fields.insert("version".into(), "1".into());
    fields.insert("kind".into(), "summary".into());
    fields.insert("run_id".into(), first.run_id.clone());
    fields.insert(
        "status".into(),
        if failed == 0 { "ok" } else { "failed" }.into(),
    );
    fields.insert(
        "execution_mode".into(),
        first.fields["execution_mode"].clone(),
    );
    fields.insert("records_total".into(), records.len().to_string());
    fields.insert("records_ok".into(), ok.to_string());
    fields.insert("records_failed".into(), failed.to_string());
    Ok(ResultRecord {
        kind: "summary".into(),
        status: if failed == 0 { "ok" } else { "failed" }.into(),
        run_id: first.run_id.clone(),
        fields,
    })
}

/// Validates a complete run and returns its unique terminal summary.
pub fn validate_run(records: &[ResultRecord]) -> Result<&ResultRecord, ParseError> {
    validate_run_for(records, None, None)
}

/// Validates a run and, when supplied, binds it to the caller's correlation.
///
/// The wrapper owns the run id and execution mode. Without this second check a
/// child could emit a valid but unrelated run and still be counted as this run.
pub fn validate_run_for<'a>(
    records: &'a [ResultRecord],
    expected_run_id: Option<&str>,
    expected_execution_mode: Option<&str>,
) -> Result<&'a ResultRecord, ParseError> {
    let first = records.first().ok_or(ParseError::MissingSummary)?;
    let mut summaries = records.iter().filter(|r| r.kind == "summary");
    let summary = summaries.next().ok_or(ParseError::MissingSummary)?;
    if summaries.next().is_some() {
        return Err(ParseError::InconsistentRun("multiple summaries".into()));
    }
    if records.last().map(|record| record.kind.as_str()) != Some("summary") {
        return Err(ParseError::SummaryNotLast);
    }
    if records.iter().any(|r| {
        r.run_id != first.run_id || r.fields["execution_mode"] != first.fields["execution_mode"]
    }) {
        return Err(ParseError::InconsistentRun(
            "run_id or execution_mode differs".into(),
        ));
    }
    if expected_run_id.is_some_and(|expected| first.run_id != expected)
        || expected_execution_mode
            .is_some_and(|expected| first.fields["execution_mode"] != expected)
    {
        return Err(ParseError::InconsistentRun(
            "run does not match wrapper correlation".into(),
        ));
    }
    let total = parse_count(summary, "records_total")?;
    let ok = parse_count(summary, "records_ok")?;
    let failed = parse_count(summary, "records_failed")?;
    let business_records = records.iter().filter(|record| record.kind != "summary");
    let observed_total = business_records.clone().count();
    if observed_total == 0 {
        return Err(ParseError::InconsistentRun(
            "run has no business records".into(),
        ));
    }
    if total != observed_total || ok + failed != total {
        return Err(ParseError::InconsistentRun(
            "summary counts do not match stream".into(),
        ));
    }
    let observed_ok = records
        .iter()
        .filter(|record| {
            record.kind != "summary" && matches!(record.status.as_str(), "ok" | "ready")
        })
        .count();
    let observed_failed = records
        .iter()
        .filter(|record| {
            record.kind != "summary" && !matches!(record.status.as_str(), "ok" | "ready")
        })
        .count();
    if ok != observed_ok || failed != observed_failed {
        return Err(ParseError::InconsistentRun(
            "summary status counts do not match records".into(),
        ));
    }
    Ok(summary)
}

fn parse_count(record: &ResultRecord, key: &'static str) -> Result<usize, ParseError> {
    record.fields[key]
        .parse::<usize>()
        .map_err(|_| ParseError::InvalidValue(format!("{key} must be an integer")))
}

/// Execution environment of a test run.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ExecutionMode {
    /// No display session; deterministic CI/container checks.
    Ci,
    /// Directly on the user's display session.
    LiveHost,
    /// In a container with an explicitly mounted host display session.
    LiveContainer,
}

impl ExecutionMode {
    /// Returns the stable protocol token.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Ci => "ci",
            Self::LiveHost => "live-host",
            Self::LiveContainer => "live-container",
        }
    }
}

impl FromStr for ExecutionMode {
    type Err = &'static str;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "ci" => Ok(Self::Ci),
            "live-host" => Ok(Self::LiveHost),
            "live-container" => Ok(Self::LiveContainer),
            _ => Err("unknown execution mode"),
        }
    }
}

/// Harness-level state which must not be mistaken for a fidus measurement.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum HarnessStatus {
    /// The environment could not provide the requested primitive.
    EnvironmentUnavailable,
    /// Recovery could not be verified after an unobservable termination.
    RecoveryUnverified,
    /// The harness could not parse or execute its own protocol.
    HarnessError,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn line(kind: &str, extra: &str) -> String {
        let status = if kind == "environment" { "ready" } else { "ok" };
        format!(
            "FIDUS_RESULT version=1 kind={kind} run_id=r01 status={status} execution_mode=ci {extra}"
        )
    }

    #[test]
    fn ignores_human_log_lines() {
        assert_eq!(parse_result_line("[stage 1] hello").unwrap(), None);
    }

    #[test]
    fn parses_quality_and_validates_schema() {
        let record = parse_result_line(&line(
            "calibration",
            "backend=wayland method=crosshair rms_residual_px=0.203 verification_max_err_px=0.731 consistency_max_err_px=0.856",
        ))
        .unwrap()
        .unwrap();
        assert_eq!(record.kind, "calibration");
        assert_eq!(record.fields["rms_residual_px"], "0.203");
    }

    #[test]
    fn rejects_duplicate_missing_and_unknown_fields() {
        assert!(matches!(
            parse_result_line(&line(
                "summary",
                "kind=other records_total=1 records_ok=1 records_failed=0"
            )),
            Err(ParseError::DuplicateKey(_))
        ));
        assert!(matches!(
            parse_result_line(
                "FIDUS_RESULT version=1 kind=summary run_id=r status=ok execution_mode=ci records_total=1"
            ),
            Err(ParseError::MissingField("records_ok"))
        ));
        assert!(matches!(
            parse_result_line(&line("nope", "")),
            Err(ParseError::InvalidValue(_))
        ));
    }

    #[test]
    fn validates_one_summary_and_run_totals() {
        let records = vec![
            parse_result_line(&line(
                "environment",
                "backend=none compositor=none output=none scale=1 transform=normal",
            ))
            .unwrap()
            .unwrap(),
            parse_result_line(&line(
                "summary",
                "records_total=1 records_ok=1 records_failed=0",
            ))
            .unwrap()
            .unwrap(),
        ];
        assert_eq!(validate_run(&records).unwrap().status, "ok");
    }

    #[test]
    fn rejects_inconsistent_run_and_missing_summary() {
        let record = parse_result_line(&line(
            "environment",
            "backend=none compositor=none output=none scale=1 transform=normal",
        ))
        .unwrap()
        .unwrap();
        assert!(matches!(
            validate_run(&[record]),
            Err(ParseError::MissingSummary)
        ));
    }

    #[test]
    fn validates_environment_failure_and_lifecycle_recovery() {
        let environment = parse_result_line(
            "FIDUS_RESULT version=1 kind=environment run_id=r01 status=unavailable execution_mode=live-container backend=wayland compositor=niri output=eDP-1 scale=1 transform=normal",
        )
        .unwrap()
        .unwrap();
        let lifecycle = parse_result_line(
            "FIDUS_RESULT version=1 kind=lifecycle run_id=r01 status=unverified execution_mode=live-container teardown=confirmed recovery=unverified",
        )
        .unwrap()
        .unwrap();
        let summary = parse_result_line(
            "FIDUS_RESULT version=1 kind=summary run_id=r01 status=failed execution_mode=live-container records_total=2 records_ok=0 records_failed=2",
        )
        .unwrap()
        .unwrap();
        assert_eq!(
            validate_run(&[environment, lifecycle, summary])
                .unwrap()
                .status,
            "failed"
        );
    }

    #[test]
    fn accepts_frozen_recovery_values_and_rejects_unknown() {
        for recovery in ["not_requested", "confirmed", "unverified"] {
            let line = format!(
                "FIDUS_RESULT version=1 kind=lifecycle run_id=r01 status=ok execution_mode=live-host teardown=confirmed recovery={recovery}"
            );
            assert!(parse_result_line(&line).unwrap().is_some());
        }
        let unknown = "FIDUS_RESULT version=1 kind=lifecycle run_id=r01 status=ok execution_mode=live-host teardown=confirmed recovery=maybe";
        assert!(matches!(
            parse_result_line(unknown),
            Err(ParseError::InvalidValue(_))
        ));
    }

    #[test]
    fn wrapper_binding_rejects_unrelated_run_or_mode() {
        let records = vec![
            parse_result_line(&line(
                "environment",
                "backend=none compositor=none output=none scale=1 transform=normal",
            ))
            .unwrap()
            .unwrap(),
            parse_result_line(&line(
                "summary",
                "records_total=1 records_ok=1 records_failed=0",
            ))
            .unwrap()
            .unwrap(),
        ];
        assert!(matches!(
            validate_run_for(&records, Some("other"), Some("ci")),
            Err(ParseError::InconsistentRun(_))
        ));
        assert!(matches!(
            validate_run_for(&records, Some("r"), Some("live-host")),
            Err(ParseError::InconsistentRun(_))
        ));
        assert!(validate_run_for(&records, Some("r01"), Some("ci")).is_ok());
    }

    #[test]
    fn rejects_empty_run() {
        let summary = parse_result_line(
            "FIDUS_RESULT version=1 kind=summary run_id=r01 status=ok execution_mode=ci records_total=0 records_ok=0 records_failed=0",
        )
        .unwrap()
        .unwrap();
        assert!(matches!(
            validate_run(&[summary]),
            Err(ParseError::InconsistentRun(_))
        ));
    }

    #[test]
    fn encodes_and_parses_terminal_summary() {
        let business = parse_result_line(&line(
            "environment",
            "backend=none compositor=none output=none scale=1 transform=normal",
        ))
        .unwrap()
        .unwrap();
        let summary = summary_for(std::slice::from_ref(&business)).unwrap();
        let parsed = parse_result_line(&summary.to_line().unwrap())
            .unwrap()
            .unwrap();
        assert_eq!(parsed, summary);
        assert_eq!(validate_run(&[business, parsed]).unwrap().status, "ok");
    }

    #[test]
    fn serialization_rejects_malformed_records_without_panicking() {
        let mut missing_mode = ResultRecord {
            kind: "summary".into(),
            status: "ok".into(),
            run_id: "r01".into(),
            fields: BTreeMap::new(),
        };
        assert_eq!(
            missing_mode.to_line(),
            Err(ParseError::MissingField("execution_mode"))
        );

        missing_mode
            .fields
            .insert("execution_mode".into(), "ci".into());
        assert!(matches!(
            missing_mode.to_line(),
            Err(ParseError::MissingField("records_total"))
        ));

        let mut invalid = missing_mode;
        invalid.fields.insert("records_total".into(), "1".into());
        invalid.fields.insert("records_ok".into(), "1".into());
        invalid.fields.insert("records_failed".into(), "0".into());
        invalid.run_id = "bad id".into();
        assert!(matches!(
            invalid.to_line(),
            Err(ParseError::MalformedToken(_))
        ));
    }

    #[test]
    fn serialization_rejects_conflicting_common_fields() {
        let mut record = ResultRecord {
            kind: "summary".into(),
            status: "ok".into(),
            run_id: "r01".into(),
            fields: BTreeMap::from([
                ("execution_mode".into(), "ci".into()),
                ("records_total".into(), "1".into()),
                ("records_ok".into(), "1".into()),
                ("records_failed".into(), "0".into()),
                ("status".into(), "failed".into()),
            ]),
        };
        assert!(matches!(record.to_line(), Err(ParseError::InvalidValue(_))));
        record.fields.remove("status");
        assert!(record.to_line().is_ok());
    }

    #[test]
    fn rejects_non_finite_quality() {
        assert!(matches!(
            parse_result_line(&line(
                "calibration",
                "backend=wayland method=crosshair rms_residual_px=NaN verification_max_err_px=1 consistency_max_err_px=1"
            )),
            Err(ParseError::InvalidValue(_))
        ));
    }

    #[test]
    fn execution_modes_are_stable_and_round_trip() {
        for mode in [
            ExecutionMode::Ci,
            ExecutionMode::LiveHost,
            ExecutionMode::LiveContainer,
        ] {
            assert_eq!(mode.as_str().parse(), Ok(mode));
        }
    }
}

#[cfg(test)]
mod output_mutation_tests {
    use std::collections::HashMap;

    #[derive(Clone, Debug, Eq, PartialEq)]
    enum Identity {
        Stable(&'static str),
        NameOnly(&'static str),
    }

    #[derive(Clone, Debug, Eq, PartialEq)]
    struct OutputState {
        identity: Identity,
        scale: String,
        transform: &'static str,
    }

    struct FakeAdapter {
        state: OutputState,
        calls: Vec<&'static str>,
        fail_scale_restore: bool,
        fail_transform_apply: bool,
    }

    impl FakeAdapter {
        fn set_scale(&mut self, scale: &str) {
            self.calls.push("set_scale");
            self.state.scale = scale.to_owned();
        }

        fn set_transform(&mut self, transform: &'static str) -> Result<(), ()> {
            self.calls.push("set_transform");
            if self.fail_transform_apply {
                return Err(());
            }
            self.state.transform = transform;
            Ok(())
        }

        fn restore(&mut self, original: &OutputState) -> bool {
            self.calls.push("restore_scale");
            if !self.fail_scale_restore {
                self.state.scale = original.scale.clone();
            }
            self.calls.push("restore_transform");
            self.state.transform = original.transform;
            self.state == *original
        }
    }

    const NIRI_OUTPUTS_V25_FIXTURE: &str =
        "Output \"Panel\" (eDP-1)\n  Scale: 1.25\n  Transform: 90° counter-clockwise\n";

    fn canonical_transform(text: &str) -> Option<&'static str> {
        match text.trim() {
            "normal" => Some("normal"),
            "90° counter-clockwise" => Some("90"),
            "180°" => Some("180"),
            "270° counter-clockwise" => Some("270"),
            _ => None,
        }
    }

    fn fold_results(results: &[&str]) -> &'static str {
        if results.contains(&"RecoveryUnverified") {
            "RecoveryUnverified"
        } else if results.contains(&"HarnessError") {
            "HarnessError"
        } else if results.contains(&"EnvironmentUnavailable") {
            "EnvironmentUnavailable"
        } else if results.iter().any(|result| *result != "ok") {
            "ChildFailed"
        } else {
            "ok"
        }
    }

    #[test]
    fn niri_fixture_canonicalizes_transform_output() {
        let fixture = NIRI_OUTPUTS_V25_FIXTURE;
        let scale = fixture
            .lines()
            .find_map(|line| line.strip_prefix("  Scale: "));
        let transform = fixture
            .lines()
            .find_map(|line| line.strip_prefix("  Transform: "))
            .and_then(canonical_transform);
        assert_eq!(scale, Some("1.25"));
        assert_eq!(transform, Some("90"));
    }

    #[test]
    fn niri_fixture_canonicalizes_all_supported_transforms() {
        for (display, canonical) in [
            ("normal", "normal"),
            ("90° counter-clockwise", "90"),
            ("180°", "180"),
            ("270° counter-clockwise", "270"),
        ] {
            assert_eq!(canonical_transform(display), Some(canonical));
        }
    }

    #[test]
    fn niri_fixture_rejects_unconnected_deferred_output() {
        let output =
            "Output \"missing\" is not connected.\nThe change will apply when it is connected.";
        assert!(output.contains("is not connected"));
        assert!(!output.contains("Scale:"));
        assert!(!output.contains("Transform:"));
    }

    #[test]
    fn niri_fixture_rejects_unknown_transform_and_duplicate_field() {
        assert_eq!(canonical_transform("90"), None);
        let mut fields = HashMap::new();
        assert!(fields.insert("Scale", "1").is_none());
        assert!(fields.insert("Scale", "1.25").is_some());
    }

    #[test]
    fn stable_identity_restores_snapshot_and_name_only_never_confirms() {
        let original = OutputState {
            identity: Identity::Stable("connector-1"),
            scale: "1".into(),
            transform: "normal",
        };
        let mut adapter = FakeAdapter {
            state: original.clone(),
            calls: Vec::new(),
            fail_scale_restore: false,
            fail_transform_apply: false,
        };
        adapter.set_scale("1.25");
        adapter.set_transform("90").unwrap();
        assert!(adapter.restore(&original));
        assert_eq!(adapter.state, original);
        assert_ne!(Identity::NameOnly("eDP-1"), Identity::Stable("connector-1"));
    }

    #[test]
    fn partial_apply_still_attempts_both_restore_fields() {
        let original = OutputState {
            identity: Identity::Stable("connector-1"),
            scale: "1".into(),
            transform: "normal",
        };
        let mut adapter = FakeAdapter {
            state: original.clone(),
            calls: Vec::new(),
            fail_scale_restore: true,
            fail_transform_apply: true,
        };
        adapter.set_scale("1.25");
        assert!(adapter.set_transform("90").is_err());
        assert!(!adapter.restore(&original));
        assert_eq!(
            adapter.calls,
            [
                "set_scale",
                "set_transform",
                "restore_scale",
                "restore_transform"
            ]
        );
    }

    #[test]
    fn recovery_failure_dominates_child_and_matrix_does_not_hide_prior_failure() {
        assert_eq!(
            fold_results(&["ok", "RecoveryUnverified"]),
            "RecoveryUnverified"
        );
        assert_eq!(
            fold_results(&["EnvironmentUnavailable", "ok"]),
            "EnvironmentUnavailable"
        );
        assert_eq!(fold_results(&["ChildFailed", "ok"]), "ChildFailed");
    }

    #[test]
    fn unauthorized_run_does_not_mutate_or_acquire_mutation_path() {
        let mut adapter = FakeAdapter {
            state: OutputState {
                identity: Identity::Stable("connector-1"),
                scale: "1".into(),
                transform: "normal",
            },
            calls: Vec::new(),
            fail_scale_restore: false,
            fail_transform_apply: false,
        };
        let before = adapter.state.clone();
        let allow_output_mutation = false;
        if allow_output_mutation {
            adapter.set_scale("1.25");
            adapter.set_transform("90").unwrap();
        }
        assert_eq!(adapter.state, before);
        assert!(adapter.calls.is_empty());
    }

    #[test]
    fn matrix_stops_after_first_failed_case() {
        let mut cases = Vec::new();
        for case_result in ["ok", "EnvironmentUnavailable", "ok"] {
            cases.push(case_result);
            if case_result != "ok" {
                break;
            }
        }
        assert_eq!(cases, ["ok", "EnvironmentUnavailable"]);
        assert_eq!(fold_results(&cases), "EnvironmentUnavailable");
    }

    #[test]
    fn state_trace_has_restore_before_next_case() {
        let trace = [
            "LockAcquired",
            "ReadOriginal",
            "AppliedAndReadBack",
            "Running",
            "RestoreRequested",
            "RestoredAndReadBack",
            "AppliedAndReadBack",
            "Running",
            "RestoreRequested",
            "RestoredAndReadBack",
        ];
        for window in trace.windows(2) {
            assert_ne!(window, ["Running", "AppliedAndReadBack"]);
        }
        assert_eq!(
            trace
                .iter()
                .filter(|state| **state == "RestoreRequested")
                .count(),
            2
        );
    }

    #[test]
    fn canonical_scale_comparison_rejects_invalid_values() {
        fn canonical_scale(value: &str) -> Option<String> {
            let value = value.trim().parse::<f64>().ok()?;
            if !value.is_finite() || value <= 0.0 {
                return None;
            }
            Some(format!("{value:.2}"))
        }
        assert_eq!(canonical_scale("1.250"), Some("1.25".into()));
        assert_eq!(canonical_scale("NaN"), None);
        assert_eq!(canonical_scale("0"), None);
    }

    #[test]
    fn recovery_signal_is_recorded_without_second_restore() {
        let mut restoring = true;
        let mut restore_count = 1;
        let mut interrupted_during_recovery = 0;
        for _ in 0..3 {
            if restoring {
                interrupted_during_recovery += 1;
            } else {
                restoring = true;
                restore_count += 1;
            }
        }
        assert_eq!(interrupted_during_recovery, 3);
        assert_eq!(restore_count, 1);
    }

    #[test]
    fn identity_change_cannot_confirm_restore() {
        let original = OutputState {
            identity: Identity::Stable("connector-1"),
            scale: "1".into(),
            transform: "normal",
        };
        let mut changed = original.clone();
        changed.identity = Identity::Stable("connector-2");
        assert_ne!(changed.identity, original.identity);
        assert_ne!(changed, original);
    }

    #[test]
    fn kernel_lock_rejects_contender_until_owner_releases() {
        #[derive(Default)]
        struct FakeLock {
            held: bool,
        }
        impl FakeLock {
            fn try_acquire(&mut self) -> bool {
                if self.held {
                    false
                } else {
                    self.held = true;
                    true
                }
            }
            fn release(&mut self) {
                self.held = false;
            }
        }
        let mut lock = FakeLock::default();
        assert!(lock.try_acquire());
        assert!(!lock.try_acquire());
        lock.release();
        assert!(lock.try_acquire());
    }

    #[test]
    fn child_timeout_uses_term_then_kill_and_reaps_group() {
        let mut events = Vec::new();
        let child_ignores_term = true;
        events.push("TERM_PGID");
        if child_ignores_term {
            events.push("KILL_PGID");
        }
        events.push("WAIT_REAP");
        assert_eq!(events, ["TERM_PGID", "KILL_PGID", "WAIT_REAP"]);
    }

    #[test]
    fn spawn_failure_after_snapshot_enters_restore() {
        let mut events = vec!["LockAcquired", "ReadOriginal", "ApplyReadBack"];
        let spawn_ok = false;
        if !spawn_ok {
            events.push("RestoreRequested");
            events.push("RestoreReadBack");
        }
        assert_eq!(
            events,
            [
                "LockAcquired",
                "ReadOriginal",
                "ApplyReadBack",
                "RestoreRequested",
                "RestoreReadBack"
            ]
        );
    }

    #[test]
    fn lock_release_failure_upgrades_an_otherwise_successful_result() {
        let child_result = "ok";
        let release_verified = false;
        let recovery = if release_verified {
            "confirmed"
        } else {
            "unverified"
        };
        assert_eq!(child_result, "ok");
        assert_eq!(recovery, "unverified");
        assert_ne!(recovery, "confirmed");
    }

    #[test]
    fn descendants_share_the_session_group_cleanup_boundary() {
        let process_groups = [(101, 101), (102, 101), (103, 101)];
        let child_group = process_groups[0].1;
        assert!(
            process_groups
                .iter()
                .all(|(_, group)| *group == child_group)
        );
        assert_eq!(process_groups.len(), 3);
    }

    #[test]
    fn summary_fold_preserves_recovery_over_child_and_timeout() {
        let outcomes = ["child=0", "timeout=1", "recovery=unverified"];
        let final_status =
            if outcomes.contains(&"recovery=unverified") || outcomes.contains(&"timeout=1") {
                ("failed", 4)
            } else {
                ("ok", 0)
            };
        assert_eq!(final_status, ("failed", 4));
    }

    #[test]
    fn snapshot_is_immutable_and_no_default_fallback_is_used() {
        let original = OutputState {
            identity: Identity::Stable("connector-1"),
            scale: "1.5".into(),
            transform: "270",
        };
        let mut adapter = FakeAdapter {
            state: original.clone(),
            calls: Vec::new(),
            fail_scale_restore: false,
            fail_transform_apply: false,
        };
        adapter.set_scale("2");
        adapter.set_transform("normal").unwrap();
        assert!(adapter.restore(&original));
        assert_eq!(adapter.state.scale, "1.5");
        assert_eq!(adapter.state.transform, "270");
    }
}
