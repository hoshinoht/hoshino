//! Target-specific system snapshots normalized into the public model.
#[cfg(target_os = "linux")]
mod linux;
#[cfg(target_os = "macos")]
mod macos;
#[cfg(target_os = "windows")]
mod windows;

use crate::model::{
    Cpu, Diagnostic, DiagnosticSeverity, Disk, Memory, OperatingSystem, Seconds, SecondsUnit,
    System,
};

#[derive(Debug)]
pub struct SystemFacts {
    pub system: System,
    pub diagnostics: Vec<Diagnostic>,
}

#[derive(Debug)]
pub(crate) struct PlatformSnapshot {
    pub os_name: Option<String>,
    pub os_version: Option<String>,
    pub architecture: Option<String>,
    pub cpu_label: Option<String>,
    pub cpu_utilization_percent: Option<f64>,
    pub cpu_frequency_mhz: Option<u64>,
    pub memory: Option<(u64, u64)>,
    pub disks: Vec<RawDisk>,
    pub uptime_seconds: Option<u64>,
}

#[derive(Debug)]
pub(crate) struct RawDisk {
    pub mount: String,
    pub available_bytes: u64,
    pub total_bytes: u64,
}

/// Collect a one-shot system snapshot without waiting for CPU sampling.
pub fn collect() -> SystemFacts {
    #[cfg(target_os = "linux")]
    return linux::collect();
    #[cfg(target_os = "macos")]
    return macos::collect();
    #[cfg(target_os = "windows")]
    return windows::collect();
    #[allow(unreachable_code)]
    SystemFacts {
        system: System::default(),
        diagnostics: vec![diagnostic("system-platform", "unsupported target platform")],
    }
}

pub(crate) fn normalize(raw: PlatformSnapshot) -> SystemFacts {
    let mut diagnostics = Vec::new();
    let memory = raw.memory.and_then(|(used_bytes, total_bytes)| {
        (total_bytes > 0 && used_bytes <= total_bytes).then_some(Memory {
            used_bytes,
            total_bytes,
        })
    });
    if raw.memory.is_some() && memory.is_none() {
        diagnostics.push(diagnostic(
            "system-memory",
            "invalid memory measurement omitted",
        ));
    }
    let mut disks = Vec::new();
    for disk in raw.disks {
        if disk.mount.is_empty() || disk.total_bytes == 0 || disk.available_bytes > disk.total_bytes
        {
            diagnostics.push(diagnostic(
                "system-disk",
                "invalid disk measurement omitted",
            ));
            continue;
        }
        disks.push(Disk {
            mount: disk.mount,
            used_bytes: disk.total_bytes - disk.available_bytes,
            total_bytes: disk.total_bytes,
        });
    }
    let utilization_percent = raw
        .cpu_utilization_percent
        .filter(|value| value.is_finite() && (0.0..=100.0).contains(value));
    if raw.cpu_utilization_percent.is_some() && utilization_percent.is_none() {
        diagnostics.push(diagnostic("system-cpu", "invalid CPU utilization omitted"));
    }
    SystemFacts {
        system: System {
            os: raw.os_name.map(|name| OperatingSystem {
                name,
                version: raw.os_version,
                architecture: raw.architecture,
            }),
            cpu: (raw.cpu_label.is_some()
                || utilization_percent.is_some()
                || raw.cpu_frequency_mhz.is_some())
            .then_some(Cpu {
                label: raw.cpu_label,
                utilization_percent,
                frequency_mhz: raw.cpu_frequency_mhz.filter(|value| *value > 0),
            }),
            memory,
            disks,
            uptime: raw.uptime_seconds.map(|value| Seconds {
                value,
                unit: SecondsUnit::Seconds,
            }),
        },
        diagnostics,
    }
}

pub(crate) fn diagnostic(code: &str, message: &str) -> Diagnostic {
    Diagnostic {
        severity: DiagnosticSeverity::Warning,
        code: code.into(),
        message: message.into(),
        subject: None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn raw() -> PlatformSnapshot {
        PlatformSnapshot {
            os_name: Some("TestOS".into()),
            os_version: Some("1".into()),
            architecture: Some("test".into()),
            cpu_label: Some("Test CPU".into()),
            cpu_utilization_percent: Some(100.0),
            cpu_frequency_mhz: Some(2_400),
            memory: Some((0, 1)),
            disks: vec![RawDisk {
                mount: "/".into(),
                available_bytes: 0,
                total_bytes: 1,
            }],
            uptime_seconds: Some(0),
        }
    }

    #[test]
    fn preserves_explicit_units_and_boundaries() {
        let facts = normalize(raw());
        assert_eq!(facts.system.memory.unwrap().total_bytes, 1);
        assert_eq!(facts.system.disks[0].used_bytes, 1);
        assert_eq!(facts.system.cpu.unwrap().utilization_percent, Some(100.0));
        assert_eq!(facts.system.uptime.unwrap().unit, SecondsUnit::Seconds);
    }

    #[test]
    fn unavailable_and_invalid_values_are_not_fabricated() {
        let mut input = raw();
        input.cpu_frequency_mhz = Some(0);
        input.cpu_utilization_percent = Some(100.1);
        input.memory = Some((2, 0));
        input.disks = vec![RawDisk {
            mount: "".into(),
            available_bytes: 2,
            total_bytes: 1,
        }];
        let facts = normalize(input);
        assert_eq!(facts.system.cpu.unwrap().frequency_mhz, None);
        assert_eq!(facts.system.memory, None);
        assert!(facts.system.disks.is_empty());
        assert_eq!(facts.diagnostics.len(), 3);
    }

    #[test]
    fn keeps_multiple_valid_disks() {
        let mut input = raw();
        input.disks.push(RawDisk {
            mount: "/data".into(),
            available_bytes: 8,
            total_bytes: 10,
        });
        let facts = normalize(input);
        assert_eq!(facts.system.disks.len(), 2);
        assert_eq!(facts.system.disks[1].used_bytes, 2);
    }
}
