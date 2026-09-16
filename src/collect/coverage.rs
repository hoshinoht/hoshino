use crate::{
    limits::Limits,
    model::{
        Coverage, CoverageFormat, Diagnostic, DiagnosticSeverity, Freshness, Seconds, SecondsUnit,
    },
    regular_file::{OpenRegularError, SymlinkPolicy, open_regular},
};
use quick_xml::{
    XmlVersion,
    events::{BytesStart, Event},
    reader::Reader,
};
use std::{
    fs::{self, Metadata},
    io::{self, Read},
    path::{Path, PathBuf},
    time::{SystemTime, UNIX_EPOCH},
};

const CANDIDATES: &[&str] = &[
    "coverage/lcov.info",
    "lcov.info",
    "coverage/coverage-summary.json",
    "coverage-summary.json",
    "coverage.xml",
    "cobertura.xml",
    "coverage/cobertura-coverage.xml",
    "target/site/jacoco/jacoco.xml",
    "coverage.out",
    "cover.out",
];

type ParsedCoverage = (CoverageFormat, Option<f64>, Option<f64>, Option<f64>);

pub fn collect(
    root: &Path,
    configured: Option<&Path>,
    newest: Option<SystemTime>,
    truncated: bool,
    limits: &Limits,
    diagnostics: &mut Vec<Diagnostic>,
) -> Option<Coverage> {
    let mut paths = Vec::with_capacity(configured.is_some() as usize + CANDIDATES.len());
    if let Some(path) = configured {
        paths.push((
            if path.is_absolute() {
                path.to_path_buf()
            } else {
                root.join(path)
            },
            true,
        ));
    }
    paths.extend(
        CANDIDATES
            .iter()
            .map(|candidate| (root.join(candidate), false)),
    );

    let mut seen = std::collections::HashSet::new();
    for (path, is_configured) in paths {
        if !is_configured {
            match fs::symlink_metadata(&path) {
                Ok(metadata) if metadata.file_type().is_symlink() => {
                    diagnostics.push(d("coverage-auto-symlink", &path));
                    continue;
                }
                Ok(_) => {}
                Err(error) if error.kind() == io::ErrorKind::NotFound => continue,
                Err(_) => {
                    diagnostics.push(d("coverage-metadata-failed", &path));
                    continue;
                }
            }
        }
        if !seen.insert(dedup_key(&path)) {
            continue;
        }

        let metadata = match fs::metadata(&path) {
            Ok(metadata) => metadata,
            Err(error) if error.kind() == io::ErrorKind::NotFound && !is_configured => continue,
            Err(_) => {
                diagnostics.push(d(
                    if is_configured {
                        "coverage-configured-missing"
                    } else {
                        "coverage-metadata-failed"
                    },
                    &path,
                ));
                continue;
            }
        };
        if !metadata.is_file() {
            diagnostics.push(d("coverage-not-regular-file", &path));
            continue;
        }
        if metadata.len() > limits.coverage_max_bytes {
            diagnostics.push(d("coverage-too-large", &path));
            continue;
        }

        let (contents, metadata) = match read_bounded(
            &path,
            limits.coverage_max_bytes,
            if is_configured {
                SymlinkPolicy::Follow
            } else {
                SymlinkPolicy::NoFollow
            },
        ) {
            Ok(contents) => contents,
            Err(ReadFailure::TooLarge) => {
                diagnostics.push(d("coverage-too-large", &path));
                continue;
            }
            Err(ReadFailure::NotRegular) => {
                diagnostics.push(d("coverage-not-regular-file", &path));
                continue;
            }
            Err(ReadFailure::Io | ReadFailure::Utf8) => {
                diagnostics.push(d("coverage-read-failed", &path));
                continue;
            }
        };
        let Some((format, line, branch, function)) = parse(&path, &contents) else {
            diagnostics.push(d("coverage-malformed", &path));
            continue;
        };

        let modified = metadata.modified().ok();
        let freshness = if truncated {
            Freshness::Unknown
        } else if modified
            .zip(newest)
            .is_some_and(|(report, source)| report < source)
        {
            Freshness::Stale
        } else {
            Freshness::Current
        };
        return Some(Coverage {
            line_percent: line,
            branch_percent: branch,
            function_percent: function,
            source: None,
            format: Some(format),
            report_path: Some(report_path(root, &path)),
            report_modified: modified
                .and_then(|time| time.duration_since(UNIX_EPOCH).ok())
                .map(|duration| Seconds {
                    value: duration.as_secs(),
                    unit: SecondsUnit::Seconds,
                }),
            stale: freshness,
        });
    }
    None
}

fn dedup_key(path: &Path) -> PathBuf {
    fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf())
}

fn report_path(root: &Path, path: &Path) -> String {
    path.strip_prefix(root)
        .unwrap_or(path)
        .display()
        .to_string()
}

enum ReadFailure {
    TooLarge,
    NotRegular,
    Io,
    Utf8,
}

fn read_bounded(
    path: &Path,
    max_bytes: u64,
    policy: SymlinkPolicy,
) -> Result<(String, Metadata), ReadFailure> {
    let file = open_regular(path, policy).map_err(|error| match error {
        OpenRegularError::NotRegular => ReadFailure::NotRegular,
        OpenRegularError::Io(_) => ReadFailure::Io,
        #[cfg(not(any(unix, windows)))]
        OpenRegularError::NonblockingUnavailable => ReadFailure::Io,
    })?;
    let metadata = file.metadata().map_err(|_| ReadFailure::Io)?;
    let mut bytes = Vec::new();
    let read = file
        .take(max_bytes.saturating_add(1))
        .read_to_end(&mut bytes)
        .map_err(|_| ReadFailure::Io)?;
    if (read as u64) > max_bytes {
        return Err(ReadFailure::TooLarge);
    }
    String::from_utf8(bytes)
        .map(|contents| (contents, metadata))
        .map_err(|_| ReadFailure::Utf8)
}

fn parse(path: &Path, contents: &str) -> Option<ParsedCoverage> {
    let trimmed = contents.trim_start();
    let name = path
        .file_name()
        .and_then(|value| value.to_str())
        .unwrap_or_default();
    if trimmed.starts_with('<') {
        return parse_xml(contents);
    }
    if trimmed.starts_with('{') || name.contains("summary") {
        return parse_istanbul(contents);
    }
    if trimmed.lines().next()?.trim_start().starts_with("mode:")
        || matches!(name, "coverage.out" | "cover.out")
    {
        return parse_go(contents);
    }
    let looks_like_lcov = contents.lines().any(|line| {
        line == "end_of_record"
            || matches!(
                line.split_once(':').map(|(kind, _)| kind),
                Some(
                    "TN" | "SF"
                        | "FN"
                        | "FNDA"
                        | "FNF"
                        | "FNH"
                        | "DA"
                        | "LF"
                        | "LH"
                        | "BRDA"
                        | "BRF"
                        | "BRH"
                )
            )
    });
    if name.ends_with(".info") || looks_like_lcov {
        return parse_lcov(contents);
    }
    if path.extension()?.eq_ignore_ascii_case("xml") {
        return parse_xml(contents);
    }
    None
}

fn parse_lcov(contents: &str) -> Option<ParsedCoverage> {
    let mut line_hit = 0_u64;
    let mut line_total = 0_u64;
    let mut branch_hit = 0_u64;
    let mut branch_total = 0_u64;
    let mut function_hit = 0_u64;
    let mut function_total = 0_u64;
    let mut line_summary = None;
    let mut line_summary_hit = None;
    let mut branch_summary = None;
    let mut branch_summary_hit = None;
    let mut function_summary = None;
    let mut function_summary_hit = None;
    let mut recognized = false;

    for raw_line in contents.lines() {
        let line = raw_line.trim_end_matches('\r');
        if line.is_empty() {
            continue;
        }
        let (kind, value) = line.split_once(':').unwrap_or((line, ""));
        match kind {
            "TN" => {
                recognized = true;
            }
            "SF" => {
                if value.is_empty() {
                    return None;
                }
                recognized = true;
            }
            "FN" => {
                let (line_number, name) = value.split_once(',')?;
                line_number.parse::<u64>().ok()?;
                if name.is_empty() {
                    return None;
                }
                recognized = true;
            }
            "FNDA" => {
                let (count, name) = value.split_once(',')?;
                let count = count.parse::<u64>().ok()?;
                if name.is_empty() {
                    return None;
                }
                function_total = function_total.checked_add(1)?;
                function_hit = function_hit.checked_add((count > 0) as u64)?;
                recognized = true;
            }
            "FNF" => {
                function_summary = Some(value.parse::<u64>().ok()?);
                recognized = true;
            }
            "FNH" => {
                function_summary_hit = Some(value.parse::<u64>().ok()?);
                recognized = true;
            }
            "DA" => {
                let mut fields = value.split(',');
                fields.next()?.parse::<u64>().ok()?;
                let count = fields.next()?.parse::<u64>().ok()?;
                if let Some(checksum) = fields.next()
                    && (checksum.is_empty() || fields.next().is_some())
                {
                    return None;
                }
                line_total = line_total.checked_add(1)?;
                line_hit = line_hit.checked_add((count > 0) as u64)?;
                recognized = true;
            }
            "LF" => {
                line_summary = Some(value.parse::<u64>().ok()?);
                recognized = true;
            }
            "LH" => {
                line_summary_hit = Some(value.parse::<u64>().ok()?);
                recognized = true;
            }
            "BRDA" => {
                let mut fields = value.split(',');
                fields.next()?.parse::<u64>().ok()?;
                fields.next()?.parse::<u64>().ok()?;
                fields.next()?.parse::<u64>().ok()?;
                let taken = fields.next()?;
                if fields.next().is_some() {
                    return None;
                }
                let hit = if taken == "-" {
                    false
                } else {
                    taken.parse::<u64>().ok()?;
                    taken != "0"
                };
                branch_total = branch_total.checked_add(1)?;
                branch_hit = branch_hit.checked_add(hit as u64)?;
                recognized = true;
            }
            "BRF" => {
                branch_summary = Some(value.parse::<u64>().ok()?);
                recognized = true;
            }
            "BRH" => {
                branch_summary_hit = Some(value.parse::<u64>().ok()?);
                recognized = true;
            }
            "end_of_record" => {
                if !value.is_empty() {
                    return None;
                }
                recognized = true;
            }
            _ => {}
        }
    }
    if !recognized {
        return None;
    }
    validate_summary(line_summary, line_summary_hit)?;
    validate_summary(branch_summary, branch_summary_hit)?;
    validate_summary(function_summary, function_summary_hit)?;
    Some((
        CoverageFormat::Lcov,
        ratio(line_hit, line_total),
        ratio(branch_hit, branch_total),
        ratio(function_hit, function_total),
    ))
}

fn validate_summary(total: Option<u64>, hit: Option<u64>) -> Option<()> {
    match (total, hit) {
        (Some(total), Some(hit)) if hit <= total => Some(()),
        (None, None) => Some(()),
        _ => None,
    }
}

fn parse_go(contents: &str) -> Option<ParsedCoverage> {
    let mut lines = contents.lines().map(|line| line.trim_end_matches('\r'));
    let mode_line = lines.find(|line| !line.trim().is_empty())?;
    let (prefix, mode) = mode_line.split_once(':')?;
    if prefix != "mode" || !matches!(mode.trim(), "set" | "count" | "atomic") {
        return None;
    }

    let mut covered = 0_u64;
    let mut total = 0_u64;
    for line in lines {
        if line.trim().is_empty() {
            continue;
        }
        let mut fields = line.split_whitespace();
        let location = fields.next()?;
        let statements = fields.next()?.parse::<u64>().ok()?;
        let count = fields.next()?.parse::<u64>().ok()?;
        if fields.next().is_some() {
            return None;
        }
        let (_, range) = location.rsplit_once(':')?;
        let (start, end) = range.split_once(',')?;
        parse_go_position(start)?;
        parse_go_position(end)?;
        total = total.checked_add(statements)?;
        if count > 0 {
            covered = covered.checked_add(statements)?;
        }
    }
    Some((
        CoverageFormat::GoCoverprofile,
        ratio(covered, total),
        None,
        None,
    ))
}

fn parse_go_position(position: &str) -> Option<()> {
    let (line, column) = position.split_once('.')?;
    line.parse::<u64>().ok()?;
    column.parse::<u64>().ok()?;
    Some(())
}

fn parse_istanbul(contents: &str) -> Option<ParsedCoverage> {
    let value: serde_json::Value = serde_json::from_str(contents).ok()?;
    let total = value.get("total")?;
    Some((
        CoverageFormat::IstanbulSummary,
        istanbul_metric(total, "lines")?,
        istanbul_metric(total, "branches")?,
        istanbul_metric(total, "functions")?,
    ))
}

fn istanbul_metric(total: &serde_json::Value, name: &str) -> Option<Option<f64>> {
    let metric = total.get(name)?.as_object()?;
    let denominator = metric.get("total")?.as_u64()?;
    let covered = metric.get("covered")?.as_u64()?;
    if covered > denominator {
        return None;
    }
    let percent = metric.get("pct")?;
    if denominator == 0 {
        if covered != 0 {
            return None;
        }
        return match percent {
            serde_json::Value::String(value) if value == "Unknown" => Some(None),
            serde_json::Value::Number(value) => Some(Some(finite_percent(value.as_f64()?)?)),
            _ => None,
        };
    }
    Some(Some(finite_percent(percent.as_f64()?)?))
}

fn parse_xml(contents: &str) -> Option<ParsedCoverage> {
    let mut reader = Reader::from_str(contents);
    reader.config_mut().enable_all_checks(true);
    let mut buffer = Vec::new();
    let mut root = None::<String>;
    let mut depth = 0_u32;
    let mut closed = false;
    let mut cobertura_line = None;
    let mut cobertura_branch = None;
    let mut jacoco_line = None;
    let mut jacoco_branch = None;
    let mut jacoco_method = None;

    loop {
        buffer.clear();
        match reader.read_event_into(&mut buffer) {
            Ok(Event::Start(event)) => {
                if root.is_some() && closed {
                    return None;
                }
                let name = event.name().as_ref().to_owned();
                if root.is_none() {
                    if name == "coverage" {
                        cobertura_line = Some(xml_rate(&event, "line-rate")?);
                        cobertura_branch = Some(xml_rate(&event, "branch-rate")?);
                    } else if name != "report" {
                        return None;
                    }
                    root = Some(name);
                    depth = 1;
                } else {
                    if root.as_deref() == Some("report") && depth == 1 && name == "counter" {
                        add_jacoco_counter(
                            &event,
                            &mut jacoco_line,
                            &mut jacoco_branch,
                            &mut jacoco_method,
                        )?;
                    }
                    depth = depth.checked_add(1)?;
                }
            }
            Ok(Event::Empty(event)) => {
                if root.is_some() && closed {
                    return None;
                }
                let name = event.name().as_ref().to_owned();
                if root.is_none() {
                    if name == "coverage" {
                        cobertura_line = Some(xml_rate(&event, "line-rate")?);
                        cobertura_branch = Some(xml_rate(&event, "branch-rate")?);
                    } else if name != "report" {
                        return None;
                    }
                    root = Some(name);
                    closed = true;
                } else if root.as_deref() == Some("report") && depth == 1 && name == "counter" {
                    add_jacoco_counter(
                        &event,
                        &mut jacoco_line,
                        &mut jacoco_branch,
                        &mut jacoco_method,
                    )?;
                }
            }
            Ok(Event::End(_)) => {
                if depth == 0 {
                    return None;
                }
                depth -= 1;
                if depth == 0 {
                    closed = true;
                }
            }
            Ok(Event::Text(text)) => {
                if (closed || root.is_none())
                    && !text
                        .as_ref()
                        .chars()
                        .all(|character| character.is_ascii_whitespace())
                {
                    return None;
                }
            }
            Ok(Event::CData(text)) => {
                if (closed || root.is_none())
                    && !text
                        .as_ref()
                        .chars()
                        .all(|character| character.is_ascii_whitespace())
                {
                    return None;
                }
            }
            Ok(Event::Eof) => break,
            Ok(_) => {}
            Err(_) => return None,
        }
    }

    if !closed || depth != 0 {
        return None;
    }
    match root.as_deref() {
        Some("coverage") => Some((
            CoverageFormat::CoberturaXml,
            cobertura_line,
            cobertura_branch,
            None,
        )),
        Some("report") => Some((
            CoverageFormat::JacocoXml,
            jacoco_line.map(|counter| ratio(counter.1, counter.0))?,
            jacoco_branch.map(|counter| ratio(counter.1, counter.0))?,
            jacoco_method.map(|counter| ratio(counter.1, counter.0))?,
        )),
        _ => None,
    }
}

fn xml_rate(event: &BytesStart<'_>, name: &str) -> Option<f64> {
    let rate = xml_attribute(event, name)?.parse::<f64>().ok()?;
    if !rate.is_finite() || !(0.0..=1.0).contains(&rate) {
        return None;
    }
    finite_percent(rate * 100.0)
}

fn xml_attribute(event: &BytesStart<'_>, name: &str) -> Option<String> {
    let mut value = None;
    for attribute in event.attributes() {
        let attribute = attribute.ok()?;
        if attribute.key.as_ref() == name {
            if value.is_some() {
                return None;
            }
            value = Some(
                attribute
                    .normalized_value(XmlVersion::default())
                    .ok()?
                    .into_owned(),
            );
        }
    }
    value
}

fn add_jacoco_counter(
    event: &BytesStart<'_>,
    line: &mut Option<(u64, u64)>,
    branch: &mut Option<(u64, u64)>,
    method: &mut Option<(u64, u64)>,
) -> Option<()> {
    let kind = xml_attribute(event, "type")?;
    let slot = match kind.as_str() {
        "LINE" => line,
        "BRANCH" => branch,
        "METHOD" => method,
        _ => return Some(()),
    };
    if slot.is_some() {
        return None;
    }
    let missed = xml_attribute(event, "missed")?.parse::<u64>().ok()?;
    let covered = xml_attribute(event, "covered")?.parse::<u64>().ok()?;
    let total = missed.checked_add(covered)?;
    *slot = Some((total, covered));
    Some(())
}

fn ratio(hit: u64, total: u64) -> Option<f64> {
    if total == 0 || hit > total {
        return None;
    }
    finite_percent((hit as f64) * 100.0 / (total as f64))
}

fn finite_percent(value: f64) -> Option<f64> {
    if value.is_finite() && (0.0..=100.0).contains(&value) {
        Some(value)
    } else {
        None
    }
}

fn d(code: &str, path: &Path) -> Diagnostic {
    Diagnostic {
        severity: DiagnosticSeverity::Warning,
        code: code.into(),
        message: "coverage candidate was skipped".into(),
        subject: Some(path.display().to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        fs::{create_dir_all, remove_dir_all, write},
        sync::atomic::{AtomicU64, Ordering},
        time::Duration,
    };
    #[cfg(unix)]
    use std::{os::unix::fs::symlink, process::Command};

    static NEXT_FIXTURE: AtomicU64 = AtomicU64::new(0);

    fn fixture() -> PathBuf {
        let id = NEXT_FIXTURE.fetch_add(1, Ordering::Relaxed);
        let root =
            std::env::temp_dir().join(format!("hoshino-coverage-{}-{id}", std::process::id()));
        create_dir_all(&root).unwrap();
        root
    }

    fn finish(root: &Path) {
        remove_dir_all(root).unwrap();
    }

    fn coverage(path: &str, contents: &str) -> ParsedCoverage {
        parse(Path::new(path), contents).unwrap()
    }

    fn istanbul_metric_json(pct: &str, total: u64, covered: u64) -> String {
        format!(
            r#"{{"total":{{"lines":{{"total":{total},"covered":{covered},"pct":{pct}}},"branches":{{"total":{total},"covered":{covered},"pct":{pct}}},"functions":{{"total":{total},"covered":{covered},"pct":{pct}}}}}}}"#
        )
    }

    #[test]
    fn lcov_covers_zero_hundred_mixed_and_zero_denominators() {
        let cases = [
            ("DA:1,0\nBRDA:1,0,0,0\nFNDA:0,a\n", (0.0, 0.0, 0.0)),
            ("DA:1,1\nBRDA:1,0,0,1\nFNDA:1,a\n", (100.0, 100.0, 100.0)),
            (
                "DA:1,1\nDA:2,0\nBRDA:1,0,0,1\nBRDA:1,0,1,-\nFNDA:1,a\nFNDA:0,b\n",
                (50.0, 50.0, 50.0),
            ),
            (
                "TN:\nSF:file.go\nLF:0\nLH:0\nBRF:0\nBRH:0\nFNF:0\nFNH:0\nend_of_record\n",
                (f64::NAN, f64::NAN, f64::NAN),
            ),
        ];
        for (contents, expected) in cases {
            let parsed = coverage("lcov.info", contents);
            assert_eq!(parsed.0, CoverageFormat::Lcov);
            if expected.0.is_nan() {
                assert_eq!(parsed.1, None);
                assert_eq!(parsed.2, None);
                assert_eq!(parsed.3, None);
            } else {
                assert_eq!(parsed.1, Some(expected.0));
                assert_eq!(parsed.2, Some(expected.1));
                assert_eq!(parsed.3, Some(expected.2));
            }
        }
    }

    #[test]
    fn go_coverprofile_weights_num_statements_and_allows_zero_denominator() {
        let mixed = coverage(
            "coverage.out",
            "mode: count\nfile.go:1.1,2.1 3 1\nfile.go:3.1,4.1 1 0\n",
        );
        assert_eq!(mixed.0, CoverageFormat::GoCoverprofile);
        assert_eq!(mixed.1, Some(75.0));
        assert_eq!(coverage("coverage.out", "mode: set\n").1, None);
        assert_eq!(
            coverage("coverage.out", "mode: set\nfile.go:1.1,2.1 1 1\n").1,
            Some(100.0)
        );
        assert_eq!(
            coverage("coverage.out", "mode: set\nfile.go:1.1,2.1 1 0\n").1,
            Some(0.0)
        );
    }

    #[test]
    fn malformed_go_and_lcov_are_rejected() {
        assert!(parse(Path::new("coverage.out"), "mode: invalid\n").is_none());
        assert!(parse(Path::new("coverage.out"), "mode: set\nfile.go:bad 1 1\n").is_none());
        assert!(parse(Path::new("lcov.info"), "DA:bad\n").is_none());
        assert!(parse(Path::new("lcov.info"), "BRDA:1,0,0,NaN\n").is_none());
    }

    #[test]
    fn istanbul_covers_zero_hundred_mixed_and_rejects_invalid_percentages() {
        let zero = coverage(
            "coverage-summary.json",
            r#"{"total":{"lines":{"total":0,"covered":0,"pct":"Unknown"},"branches":{"total":0,"covered":0,"pct":"Unknown"},"functions":{"total":0,"covered":0,"pct":"Unknown"}}}"#,
        );
        assert_eq!(zero.0, CoverageFormat::IstanbulSummary);
        assert_eq!((zero.1, zero.2, zero.3), (None, None, None));
        assert_eq!(
            coverage("coverage-summary.json", &istanbul_metric_json("0", 2, 0)).1,
            Some(0.0)
        );
        assert_eq!(
            coverage("coverage-summary.json", &istanbul_metric_json("100", 2, 2)).1,
            Some(100.0)
        );
        assert_eq!(
            coverage("coverage-summary.json", &istanbul_metric_json("50", 2, 1)).1,
            Some(50.0)
        );
        assert!(
            parse(
                Path::new("coverage-summary.json"),
                &istanbul_metric_json("101", 2, 1)
            )
            .is_none()
        );
        assert!(
            parse(
                Path::new("coverage-summary.json"),
                &istanbul_metric_json("-1", 2, 0)
            )
            .is_none()
        );
        assert!(parse(Path::new("coverage-summary.json"), "{}\n").is_none());
    }

    #[test]
    fn cobertura_uses_root_rates_and_validates_finite_bounds() {
        assert_eq!(
            coverage(
                "coverage.xml",
                r#"<coverage line-rate="0" branch-rate="0"/>"#
            ),
            (CoverageFormat::CoberturaXml, Some(0.0), Some(0.0), None)
        );
        assert_eq!(
            coverage(
                "coverage.xml",
                r#"<coverage line-rate="1" branch-rate="1"/>"#
            ),
            (CoverageFormat::CoberturaXml, Some(100.0), Some(100.0), None)
        );
        assert_eq!(
            coverage(
                "coverage.xml",
                r#"<coverage line-rate="0.5" branch-rate="0.25"><sources><source>src</source></sources></coverage>"#
            ),
            (CoverageFormat::CoberturaXml, Some(50.0), Some(25.0), None)
        );
        assert!(
            parse(
                Path::new("coverage.xml"),
                r#"<coverage line-rate="NaN" branch-rate="0"/>"#
            )
            .is_none()
        );
        assert!(
            parse(
                Path::new("coverage.xml"),
                r#"<coverage line-rate="1.1" branch-rate="0"/>"#
            )
            .is_none()
        );
        assert!(parse(Path::new("coverage.xml"), "<coverage line-rate=\"0\">").is_none());
    }

    #[test]
    fn jacoco_uses_line_branch_and_method_counters() {
        let zero = coverage(
            "jacoco.xml",
            r#"<report><counter type="LINE" missed="0" covered="0"/><counter type="BRANCH" missed="0" covered="0"/><counter type="METHOD" missed="0" covered="0"/></report>"#,
        );
        assert_eq!(zero.0, CoverageFormat::JacocoXml);
        assert_eq!((zero.1, zero.2, zero.3), (None, None, None));
        assert_eq!(
            coverage(
                "jacoco.xml",
                r#"<report><counter type="LINE" missed="0" covered="2"/><counter type="BRANCH" missed="2" covered="0"/><counter type="METHOD" missed="1" covered="1"/></report>"#,
            ),
            (
                CoverageFormat::JacocoXml,
                Some(100.0),
                Some(0.0),
                Some(50.0)
            )
        );
        assert!(parse(
            Path::new("jacoco.xml"),
            r#"<report><counter type="LINE" missed="NaN" covered="1"/><counter type="BRANCH" missed="0" covered="0"/><counter type="METHOD" missed="0" covered="0"/></report>"#
        )
        .is_none());
        assert!(parse(
            Path::new("jacoco.xml"),
            r#"<report><counter type="LINE" missed="0" covered="1"/><counter type="BRANCH" missed="0" covered="0"/></report>"#
        )
        .is_none());
    }

    #[test]
    fn collect_skips_oversized_and_malformed_candidates() {
        let root = fixture();
        write(root.join("coverage/lcov.info"), b"too large").unwrap_or_else(|_| {
            create_dir_all(root.join("coverage")).unwrap();
            write(root.join("coverage/lcov.info"), b"too large").unwrap();
        });
        write(root.join("lcov.info"), b"DA:1,1\n").unwrap();
        let mut diagnostics = Vec::new();
        let limits = Limits {
            coverage_max_bytes: 7,
            ..Limits::default()
        };
        let result = collect(&root, None, None, false, &limits, &mut diagnostics).unwrap();
        assert_eq!(result.report_path.as_deref(), Some("lcov.info"));
        assert_eq!(result.line_percent, Some(100.0));
        assert!(
            diagnostics
                .iter()
                .any(|item| item.code == "coverage-too-large")
        );
        finish(&root);
    }

    #[cfg(unix)]
    #[test]
    fn collect_skips_auto_symlinks_and_non_regular_candidates() {
        let root = fixture();
        create_dir_all(root.join("coverage")).unwrap();
        write(root.join("report.info"), b"DA:1,0\n").unwrap();
        symlink("../report.info", root.join("coverage/lcov.info")).unwrap();
        write(root.join("lcov.info"), b"DA:1,1\n").unwrap();
        let mut diagnostics = Vec::new();
        let result = collect(
            &root,
            None,
            None,
            false,
            &Limits::default(),
            &mut diagnostics,
        )
        .unwrap();
        assert_eq!(result.report_path.as_deref(), Some("lcov.info"));
        assert!(
            diagnostics
                .iter()
                .any(|item| item.code == "coverage-auto-symlink")
        );
        finish(&root);

        let root = fixture();
        let fifo = root.join("coverage/lcov.info");
        create_dir_all(fifo.parent().unwrap()).unwrap();
        let Ok(status) = Command::new("mkfifo").arg(&fifo).status() else {
            finish(&root);
            return;
        };
        assert!(status.success());
        write(root.join("lcov.info"), b"DA:1,1\n").unwrap();
        let mut diagnostics = Vec::new();
        let result = collect(
            &root,
            None,
            None,
            false,
            &Limits::default(),
            &mut diagnostics,
        )
        .unwrap();
        assert_eq!(result.report_path.as_deref(), Some("lcov.info"));
        assert!(
            diagnostics
                .iter()
                .any(|item| item.code == "coverage-not-regular-file")
        );
        finish(&root);
    }

    #[cfg(unix)]
    #[test]
    fn configured_regular_file_symlink_is_supported() {
        let root = fixture();
        write(root.join("report.info"), b"DA:1,1\n").unwrap();
        symlink("report.info", root.join("configured.info")).unwrap();
        let mut diagnostics = Vec::new();
        let result = collect(
            &root,
            Some(Path::new("configured.info")),
            None,
            false,
            &Limits::default(),
            &mut diagnostics,
        )
        .unwrap();
        assert_eq!(result.report_path.as_deref(), Some("configured.info"));
        assert!(diagnostics.is_empty());
        finish(&root);
    }

    #[test]
    fn configured_report_wins_and_candidates_have_documented_order() {
        let root = fixture();
        create_dir_all(root.join("coverage")).unwrap();
        write(root.join("coverage/lcov.info"), b"DA:1,0\n").unwrap();
        write(root.join("lcov.info"), b"DA:1,1\n").unwrap();
        write(root.join("custom.info"), b"DA:1,1\nDA:2,1\n").unwrap();
        let mut diagnostics = Vec::new();
        let configured = collect(
            &root,
            Some(Path::new("custom.info")),
            None,
            false,
            &Limits::default(),
            &mut diagnostics,
        )
        .unwrap();
        assert_eq!(configured.report_path.as_deref(), Some("custom.info"));
        assert_eq!(configured.line_percent, Some(100.0));
        let mut diagnostics = Vec::new();
        let ordered = collect(
            &root,
            None,
            None,
            false,
            &Limits::default(),
            &mut diagnostics,
        )
        .unwrap();
        assert_eq!(ordered.report_path.as_deref(), Some("coverage/lcov.info"));
        assert_eq!(ordered.line_percent, Some(0.0));
        finish(&root);
    }

    #[test]
    fn configured_alias_is_deduplicated_before_fallback() {
        let root = fixture();
        create_dir_all(root.join("coverage")).unwrap();
        write(root.join("coverage/lcov.info"), b"DA:bad\n").unwrap();
        write(root.join("lcov.info"), b"DA:1,1\n").unwrap();
        let mut diagnostics = Vec::new();
        let result = collect(
            &root,
            Some(Path::new("coverage/../coverage/lcov.info")),
            None,
            false,
            &Limits::default(),
            &mut diagnostics,
        )
        .unwrap();
        assert_eq!(result.report_path.as_deref(), Some("lcov.info"));
        assert_eq!(
            diagnostics
                .iter()
                .filter(|item| item.code == "coverage-malformed")
                .count(),
            1
        );
        finish(&root);
    }

    #[test]
    fn freshness_reports_current_stale_and_unknown_and_mtime() {
        let root = fixture();
        write(root.join("lcov.info"), b"DA:1,1\n").unwrap();
        let future = SystemTime::now() + Duration::from_secs(60);
        let past = UNIX_EPOCH + Duration::from_secs(1);
        let mut diagnostics = Vec::new();
        let stale = collect(
            &root,
            None,
            Some(future),
            false,
            &Limits::default(),
            &mut diagnostics,
        )
        .unwrap();
        assert_eq!(stale.stale, Freshness::Stale);
        assert!(stale.report_modified.is_some());
        let current = collect(
            &root,
            None,
            Some(past),
            false,
            &Limits::default(),
            &mut diagnostics,
        )
        .unwrap();
        assert_eq!(current.stale, Freshness::Current);
        let unknown = collect(
            &root,
            None,
            Some(past),
            true,
            &Limits::default(),
            &mut diagnostics,
        )
        .unwrap();
        assert_eq!(unknown.stale, Freshness::Unknown);
        finish(&root);
    }
}
