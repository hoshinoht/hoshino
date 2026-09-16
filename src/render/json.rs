use crate::model::Snapshot;

/// JSON presentation options are intentionally limited to whitespace.  The
/// schema and values always come directly from the frozen snapshot model.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct JsonOptions {
    pub pretty: bool,
}

/// Serialize exactly one schema-v1 snapshot document.
pub fn render_json(snapshot: &Snapshot) -> Result<String, serde_json::Error> {
    render_json_with_options(snapshot, JsonOptions::default())
}

pub fn render_json_pretty(snapshot: &Snapshot) -> Result<String, serde_json::Error> {
    render_json_with_options(snapshot, JsonOptions { pretty: true })
}

pub fn render_json_with_options(
    snapshot: &Snapshot,
    options: JsonOptions,
) -> Result<String, serde_json::Error> {
    if options.pretty {
        serde_json::to_string_pretty(snapshot)
    } else {
        serde_json::to_string(snapshot)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{Context, Cpu, OperatingSystem, Seconds, SecondsUnit, SnapshotMode, System};

    fn fixture() -> Snapshot {
        let mut snapshot = Snapshot::empty(SnapshotMode::OneShot);
        snapshot.context = Context {
            worktree: Some("/work/project".into()),
            directory: Some("/work/project/src".into()),
        };
        snapshot.system = System {
            os: Some(OperatingSystem {
                name: "macOS".into(),
                version: Some("26".into()),
                architecture: Some("aarch64".into()),
            }),
            cpu: Some(Cpu {
                label: Some("CPU".into()),
                utilization_percent: None,
                frequency_mhz: Some(3_200),
            }),
            memory: None,
            disks: Vec::new(),
            uptime: Some(Seconds {
                value: 42,
                unit: SecondsUnit::Seconds,
            }),
        };
        snapshot
    }

    #[test]
    fn compact_json_is_one_round_trippable_schema_document() {
        let snapshot = fixture();
        let output = render_json(&snapshot).unwrap();
        let document: serde_json::Value = serde_json::from_str(&output).unwrap();
        assert!(document.is_object());
        assert!(output.contains("\"schema_version\":1"));
        assert!(output.contains("\"unit\":\"seconds\""));
        assert!(!output.contains('\n'));
        assert_eq!(serde_json::from_str::<Snapshot>(&output).unwrap(), snapshot);
    }

    #[test]
    fn pretty_json_only_changes_whitespace_and_never_adds_terminal_codes() {
        let snapshot = fixture();
        let compact = render_json(&snapshot).unwrap();
        let pretty = render_json_pretty(&snapshot).unwrap();
        let compact_value: serde_json::Value = serde_json::from_str(&compact).unwrap();
        let pretty_value: serde_json::Value = serde_json::from_str(&pretty).unwrap();
        assert_eq!(compact_value, pretty_value);
        for byte in compact.bytes().chain(pretty.bytes()) {
            assert_ne!(byte, 0x1b);
            assert!(byte >= 0x20 || byte == b'\n' || byte == b'\t');
        }
        assert!(!compact.contains("\x1b_G"));
        assert!(!compact.contains("\x1b]"));
    }
}
