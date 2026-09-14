// Copyright 2026 Flexiatom
// SPDX-License-Identifier: Apache-2.0

//! Minimal protocol utility for `fidus-test`.

use std::io::{self, BufRead, Write};
use std::process::{Command, ExitCode};

fn main() -> ExitCode {
    match std::env::args().nth(1).as_deref() {
        Some("parse") => parse_stdin(),
        Some("live-calibrate") => run_live_calibrate(),
        Some("mode") => {
            println!("ci live-host live-container");
            ExitCode::SUCCESS
        }
        _ => {
            eprintln!("usage: fidus-test parse | live-calibrate | mode");
            ExitCode::from(2)
        }
    }
}

fn run_live_calibrate() -> ExitCode {
    let run_id = format!("pid{}", std::process::id());
    let mode = match std::env::var("FIDUS_EXECUTION_MODE").as_deref() {
        Ok("ci") => "ci",
        Ok("live-host") => "live-host",
        Ok("live-container") => "live-container",
        Ok(unknown) => {
            eprintln!("harness error: unknown FIDUS_EXECUTION_MODE={unknown}");
            return ExitCode::from(3);
        }
        Err(_) => "live-host",
    };
    let binary = std::env::var_os("FIDUS_LIVE_CALIBRATE_BIN")
        .unwrap_or_else(|| "fidus-live-calibrate".into());
    let child = Command::new(binary)
        .env("FIDUS_EXECUTION_MODE", mode)
        .env("FIDUS_RUN_ID", &run_id)
        .output();
    let output = match child {
        Ok(output) => output,
        Err(error) => {
            eprintln!("live-calibrate launch failed: {error}");
            return ExitCode::from(2);
        }
    };
    io::stdout().write_all(&output.stdout).ok();
    io::stderr().write_all(&output.stderr).ok();
    let mut records = Vec::new();
    for line in String::from_utf8_lossy(&output.stdout).lines() {
        match fidus_test::parse_result_line(line) {
            Ok(Some(record)) => records.push(record),
            Ok(None) => {}
            Err(error) => {
                eprintln!("protocol error: {error}");
                return ExitCode::from(3);
            }
        }
    }
    let summary = match fidus_test::validate_run_for(&records, Some(&run_id), Some(mode)) {
        Ok(summary) => summary,
        Err(error) => {
            eprintln!("protocol error: {error}");
            return ExitCode::from(3);
        }
    };
    let child_code = output.status.code().unwrap_or(3) as u8;
    if child_code != 0 {
        return ExitCode::from(child_code);
    }
    // A producer may exit zero after emitting a valid failed summary. Treating
    // that as success would let a protocol-valid calibration failure pass CI.
    if summary.status == "failed" {
        return ExitCode::from(1);
    }
    ExitCode::SUCCESS
}

fn parse_stdin() -> ExitCode {
    let stdin = io::stdin();
    let mut parsed = Vec::new();
    for line in stdin.lock().lines() {
        let line = match line {
            Ok(line) => line,
            Err(error) => {
                eprintln!("input error: {error}");
                return ExitCode::from(3);
            }
        };
        match fidus_test::parse_result_line(&line) {
            Ok(Some(record)) => parsed.push(record),
            Ok(None) => {}
            Err(error) => {
                eprintln!("protocol error: {error}");
                return ExitCode::from(3);
            }
        }
    }
    match fidus_test::validate_run(&parsed) {
        Ok(summary) => {
            println!("run={} status={}", summary.run_id, summary.status);
            ExitCode::SUCCESS
        }
        Err(error) => {
            eprintln!("protocol error: {error}");
            ExitCode::from(3)
        }
    }
}
