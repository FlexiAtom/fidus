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
    /// Encodes this validated record using the v1 line protocol.
    pub fn to_line(&self) -> String {
        let mut line = format!(
            "FIDUS_RESULT version=1 kind={} run_id={} status={} execution_mode={}",
            self.kind, self.run_id, self.status, self.fields["execution_mode"]
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
        line
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
        let parsed = parse_result_line(&summary.to_line()).unwrap().unwrap();
        assert_eq!(parsed, summary);
        assert_eq!(validate_run(&[business, parsed]).unwrap().status, "ok");
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
