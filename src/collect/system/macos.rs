use super::{PlatformSnapshot, RawDisk, SystemFacts, normalize};
use sysinfo::{CpuRefreshKind, Disks, MemoryRefreshKind, RefreshKind, System};

pub(super) fn collect() -> SystemFacts {
    let system = System::new_with_specifics(
        RefreshKind::nothing()
            .with_memory(MemoryRefreshKind::nothing().with_ram())
            .with_cpu(CpuRefreshKind::nothing().with_frequency()),
    );
    let disks = Disks::new_with_refreshed_list();
    let cpu = system.cpus().first();
    normalize(PlatformSnapshot {
        os_name: System::name().or_else(|| Some("macOS".into())),
        os_version: System::os_version(),
        architecture: Some(std::env::consts::ARCH.into()),
        cpu_label: cpu
            .map(|cpu| cpu.brand().trim().to_owned())
            .filter(|label| !label.is_empty()),
        cpu_utilization_percent: None,
        cpu_frequency_mhz: cpu.map(|cpu| cpu.frequency()),
        memory: Some((system.used_memory(), system.total_memory())),
        disks: disks
            .list()
            .iter()
            .map(|disk| RawDisk {
                mount: disk.mount_point().display().to_string(),
                available_bytes: disk.available_space(),
                total_bytes: disk.total_space(),
            })
            .collect(),
        uptime_seconds: Some(System::uptime()),
    })
}

#[cfg(test)]
mod tests {
    #[test]
    fn host_snapshot_is_plausible_without_machine_specific_values() {
        let facts = super::collect();
        assert!(
            facts
                .system
                .os
                .as_ref()
                .is_some_and(|os| !os.name.is_empty())
        );
        assert!(facts.system.memory.as_ref().is_some_and(
            |memory| memory.total_bytes > 0 && memory.used_bytes <= memory.total_bytes
        ));
        assert!(
            facts
                .system
                .uptime
                .as_ref()
                .is_some_and(|uptime| uptime.value > 0)
        );
        assert!(
            facts
                .system
                .disks
                .iter()
                .all(|disk| !disk.mount.is_empty() && disk.used_bytes <= disk.total_bytes)
        );
    }
}
