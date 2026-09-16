use serde::{Deserialize, Serialize};

pub const SCHEMA_VERSION: u32 = 1;

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct Snapshot {
    pub schema_version: u32,
    pub mode: SnapshotMode,
    pub context: Context,
    pub project: Option<Project>,
    pub git: Option<Git>,
    pub system: System,
    pub time: LocalTime,
    pub diagnostics: Vec<Diagnostic>,
}

impl Snapshot {
    pub fn empty(mode: SnapshotMode) -> Self {
        Self {
            schema_version: SCHEMA_VERSION,
            mode,
            context: Context {
                worktree: None,
                directory: None,
            },
            project: None,
            git: None,
            system: System::default(),
            time: LocalTime::default(),
            diagnostics: Vec::new(),
        }
    }
}

#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum SnapshotMode {
    OneShot,
    Live,
    Hook,
}

#[derive(Clone, Debug, Default, Deserialize, PartialEq, Serialize)]
pub struct Context {
    pub worktree: Option<String>,
    pub directory: Option<String>,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct Project {
    pub primary_language: Option<String>,
    pub toolchains: Vec<Toolchain>,
    pub loc: Option<Loc>,
    pub coverage: Option<Coverage>,
}
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct Toolchain {
    pub language: String,
    pub runtime: String,
    pub version: Option<String>,
}
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct Loc {
    pub total: u64,
    pub unit: CountUnit,
    pub truncated: bool,
    pub by_language: Vec<LanguageLoc>,
}
impl Loc {
    pub fn totals_reconcile(&self) -> bool {
        self.by_language
            .iter()
            .map(|entry| entry.lines)
            .sum::<u64>()
            == self.total
    }
}
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct LanguageLoc {
    pub language: String,
    pub lines: u64,
    pub unit: CountUnit,
}
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct Coverage {
    pub line_percent: Option<f64>,
    pub branch_percent: Option<f64>,
    pub function_percent: Option<f64>,
    pub source: Option<String>,
    pub format: Option<CoverageFormat>,
    pub report_path: Option<String>,
    pub report_modified: Option<Seconds>,
    pub stale: Freshness,
}
#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum CoverageFormat {
    Lcov,
    CoberturaXml,
    JacocoXml,
    GoCoverprofile,
    IstanbulSummary,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct Git {
    pub head: GitHead,
    pub dirty: bool,
    pub staged: u32,
    pub unstaged: u32,
    pub conflicts: u32,
    pub ahead: Option<u32>,
    pub upstream: Option<String>,
}
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(tag = "kind", rename_all = "kebab-case")]
pub enum GitHead {
    Branch { name: String },
    Detached { commit: String },
    Unborn,
}

#[derive(Clone, Debug, Default, Deserialize, PartialEq, Serialize)]
pub struct System {
    pub os: Option<OperatingSystem>,
    pub cpu: Option<Cpu>,
    pub memory: Option<Memory>,
    pub disks: Vec<Disk>,
    pub uptime: Option<Seconds>,
}
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct OperatingSystem {
    pub name: String,
    pub version: Option<String>,
    pub architecture: Option<String>,
}
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct Cpu {
    pub label: Option<String>,
    pub utilization_percent: Option<f64>,
    pub frequency_mhz: Option<u64>,
}
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct Memory {
    pub used_bytes: u64,
    pub total_bytes: u64,
}
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct Disk {
    pub mount: String,
    pub used_bytes: u64,
    pub total_bytes: u64,
}

#[derive(Clone, Debug, Default, Deserialize, PartialEq, Serialize)]
pub struct LocalTime {
    pub local: Option<String>,
    pub date: Option<String>,
    pub timezone_name: Option<String>,
    pub timezone_offset_seconds: Option<i32>,
    pub day_progress_percent: Option<f64>,
}

#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum Freshness {
    Current,
    Stale,
    Unknown,
}
#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum CountUnit {
    Lines,
}
#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Serialize)]
pub struct Seconds {
    pub value: u64,
    pub unit: SecondsUnit,
}
#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Serialize)]
pub enum SecondsUnit {
    #[serde(rename = "seconds")]
    Seconds,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct Diagnostic {
    pub severity: DiagnosticSeverity,
    pub code: String,
    pub message: String,
    pub subject: Option<String>,
}
#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum DiagnosticSeverity {
    Info,
    Warning,
    Error,
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn schema_is_versioned_round_trippable_and_keeps_null_unavailability() {
        let mut snapshot = Snapshot::empty(SnapshotMode::OneShot);
        snapshot.system.cpu = Some(Cpu {
            label: Some("test".into()),
            utilization_percent: None,
            frequency_mhz: None,
        });
        snapshot.system.uptime = Some(Seconds {
            value: 42,
            unit: SecondsUnit::Seconds,
        });
        snapshot.system.os = Some(OperatingSystem {
            name: "macOS".into(),
            version: Some("26".into()),
            architecture: Some("aarch64".into()),
        });
        snapshot.project = Some(Project {
            primary_language: Some("Rust".into()),
            toolchains: Vec::new(),
            loc: Some(Loc {
                total: 3,
                unit: CountUnit::Lines,
                truncated: false,
                by_language: vec![LanguageLoc {
                    language: "Rust".into(),
                    lines: 3,
                    unit: CountUnit::Lines,
                }],
            }),
            coverage: Some(Coverage {
                line_percent: Some(90.0),
                branch_percent: None,
                function_percent: None,
                source: None,
                format: Some(CoverageFormat::Lcov),
                report_path: Some("coverage/lcov.info".into()),
                report_modified: Some(Seconds {
                    value: 1_700_000_000,
                    unit: SecondsUnit::Seconds,
                }),
                stale: Freshness::Current,
            }),
        });
        let json = serde_json::to_string_pretty(&snapshot).unwrap();
        assert!(json.contains("\"schema_version\": 1"));
        assert!(json.contains("\"utilization_percent\": null"));
        assert!(json.contains("\"unit\": \"seconds\""));
        assert!(json.contains("\"by_language\""));
        assert!(json.contains("\"report_path\": \"coverage/lcov.info\""));
        assert!(json.contains("\"report_modified\""));
        assert!(json.contains("\"architecture\": \"aarch64\""));
        assert!(
            snapshot
                .project
                .as_ref()
                .unwrap()
                .loc
                .as_ref()
                .unwrap()
                .totals_reconcile()
        );
        assert_eq!(serde_json::from_str::<Snapshot>(&json).unwrap(), snapshot);
    }
}
