use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Deserialize, Default, PartialEq, Eq, Serialize)]
#[serde(default, deny_unknown_fields)]
pub struct LimitOverrides {
    pub subprocess_timeout_ms: Option<u64>,
    pub subprocess_output_bytes: Option<u64>,
    pub scan_max_files: Option<u64>,
    pub scan_max_file_bytes: Option<u64>,
    pub scan_max_read_bytes: Option<u64>,
    pub scan_elapsed_ms: Option<u64>,
    pub coverage_max_bytes: Option<u64>,
    pub image_max_file_bytes: Option<u64>,
    pub image_max_dimension_px: Option<u32>,
    pub image_max_decoded_bytes: Option<u64>,
    pub image_max_frames: Option<u32>,
    pub live_refresh_ms: Option<u64>,
    pub frame_min_delay_ms: Option<u64>,
    pub frame_max_delay_ms: Option<u64>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Limits {
    pub subprocess_timeout_ms: u64,
    pub subprocess_output_bytes: u64,
    pub scan_max_files: u64,
    pub scan_max_file_bytes: u64,
    pub scan_max_read_bytes: u64,
    pub scan_elapsed_ms: u64,
    pub coverage_max_bytes: u64,
    pub image_max_file_bytes: u64,
    pub image_max_dimension_px: u32,
    pub image_max_decoded_bytes: u64,
    pub image_max_frames: u32,
    pub live_refresh_ms: u64,
    pub frame_min_delay_ms: u64,
    pub frame_max_delay_ms: u64,
}

impl Default for Limits {
    fn default() -> Self {
        Self {
            subprocess_timeout_ms: 500,
            subprocess_output_bytes: 65_536,
            scan_max_files: 10_000,
            scan_max_file_bytes: 1_048_576,
            scan_max_read_bytes: 8_388_608,
            scan_elapsed_ms: 250,
            coverage_max_bytes: 8_388_608,
            image_max_file_bytes: 16_777_216,
            image_max_dimension_px: 4_096,
            image_max_decoded_bytes: 67_108_864,
            image_max_frames: 120,
            live_refresh_ms: 1_000,
            frame_min_delay_ms: 20,
            frame_max_delay_ms: 10_000,
        }
    }
}

impl Limits {
    pub fn apply(&mut self, values: &LimitOverrides) -> Result<(), LimitError> {
        set(
            &mut self.subprocess_timeout_ms,
            values.subprocess_timeout_ms,
            "subprocess_timeout_ms",
            1,
            10_000,
        )?;
        set(
            &mut self.subprocess_output_bytes,
            values.subprocess_output_bytes,
            "subprocess_output_bytes",
            1,
            1_048_576,
        )?;
        set(
            &mut self.scan_max_files,
            values.scan_max_files,
            "scan_max_files",
            1,
            100_000,
        )?;
        set(
            &mut self.scan_max_file_bytes,
            values.scan_max_file_bytes,
            "scan_max_file_bytes",
            1,
            16_777_216,
        )?;
        set(
            &mut self.scan_max_read_bytes,
            values.scan_max_read_bytes,
            "scan_max_read_bytes",
            1,
            134_217_728,
        )?;
        set(
            &mut self.scan_elapsed_ms,
            values.scan_elapsed_ms,
            "scan_elapsed_ms",
            1,
            10_000,
        )?;
        set(
            &mut self.coverage_max_bytes,
            values.coverage_max_bytes,
            "coverage_max_bytes",
            1,
            67_108_864,
        )?;
        set(
            &mut self.image_max_file_bytes,
            values.image_max_file_bytes,
            "image_max_file_bytes",
            1,
            67_108_864,
        )?;
        set(
            &mut self.image_max_dimension_px,
            values.image_max_dimension_px,
            "image_max_dimension_px",
            1,
            16_384,
        )?;
        set(
            &mut self.image_max_decoded_bytes,
            values.image_max_decoded_bytes,
            "image_max_decoded_bytes",
            1,
            536_870_912,
        )?;
        set(
            &mut self.image_max_frames,
            values.image_max_frames,
            "image_max_frames",
            1,
            1_000,
        )?;
        set(
            &mut self.live_refresh_ms,
            values.live_refresh_ms,
            "live_refresh_ms",
            100,
            60_000,
        )?;
        set(
            &mut self.frame_min_delay_ms,
            values.frame_min_delay_ms,
            "frame_min_delay_ms",
            1,
            1_000,
        )?;
        set(
            &mut self.frame_max_delay_ms,
            values.frame_max_delay_ms,
            "frame_max_delay_ms",
            1,
            60_000,
        )?;
        if self.frame_min_delay_ms > self.frame_max_delay_ms {
            return Err(LimitError::new(
                "frame_min_delay_ms",
                "must not exceed frame_max_delay_ms",
            ));
        }
        Ok(())
    }
}

fn set<T: Copy + Ord + std::fmt::Display>(
    target: &mut T,
    value: Option<T>,
    name: &'static str,
    min: T,
    max: T,
) -> Result<(), LimitError> {
    if let Some(value) = value {
        if value < min || value > max {
            return Err(LimitError::new(
                name,
                format!("must be between {min} and {max}"),
            ));
        }
        *target = value;
    }
    Ok(())
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LimitError {
    pub field: &'static str,
    pub message: String,
}
impl LimitError {
    fn new(field: &'static str, message: impl Into<String>) -> Self {
        Self {
            field,
            message: message.into(),
        }
    }
}
impl std::fmt::Display for LimitError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "limits.{} {}", self.field, self.message)
    }
}
impl std::error::Error for LimitError {}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn every_hard_ceiling_accepts_boundary_and_rejects_next_value() {
        let cases: Vec<(&str, LimitOverrides, LimitOverrides)> = vec![
            (
                "subprocess",
                LimitOverrides {
                    subprocess_timeout_ms: Some(10_000),
                    ..Default::default()
                },
                LimitOverrides {
                    subprocess_timeout_ms: Some(10_001),
                    ..Default::default()
                },
            ),
            (
                "output",
                LimitOverrides {
                    subprocess_output_bytes: Some(1_048_576),
                    ..Default::default()
                },
                LimitOverrides {
                    subprocess_output_bytes: Some(1_048_577),
                    ..Default::default()
                },
            ),
            (
                "scan",
                LimitOverrides {
                    scan_max_files: Some(100_000),
                    ..Default::default()
                },
                LimitOverrides {
                    scan_max_files: Some(100_001),
                    ..Default::default()
                },
            ),
            (
                "scan file bytes",
                LimitOverrides {
                    scan_max_file_bytes: Some(16_777_216),
                    ..Default::default()
                },
                LimitOverrides {
                    scan_max_file_bytes: Some(16_777_217),
                    ..Default::default()
                },
            ),
            (
                "scan read bytes",
                LimitOverrides {
                    scan_max_read_bytes: Some(134_217_728),
                    ..Default::default()
                },
                LimitOverrides {
                    scan_max_read_bytes: Some(134_217_729),
                    ..Default::default()
                },
            ),
            (
                "scan elapsed",
                LimitOverrides {
                    scan_elapsed_ms: Some(10_000),
                    ..Default::default()
                },
                LimitOverrides {
                    scan_elapsed_ms: Some(10_001),
                    ..Default::default()
                },
            ),
            (
                "coverage",
                LimitOverrides {
                    coverage_max_bytes: Some(67_108_864),
                    ..Default::default()
                },
                LimitOverrides {
                    coverage_max_bytes: Some(67_108_865),
                    ..Default::default()
                },
            ),
            (
                "image",
                LimitOverrides {
                    image_max_dimension_px: Some(16_384),
                    ..Default::default()
                },
                LimitOverrides {
                    image_max_dimension_px: Some(16_385),
                    ..Default::default()
                },
            ),
            (
                "image file bytes",
                LimitOverrides {
                    image_max_file_bytes: Some(67_108_864),
                    ..Default::default()
                },
                LimitOverrides {
                    image_max_file_bytes: Some(67_108_865),
                    ..Default::default()
                },
            ),
            (
                "image decoded bytes",
                LimitOverrides {
                    image_max_decoded_bytes: Some(536_870_912),
                    ..Default::default()
                },
                LimitOverrides {
                    image_max_decoded_bytes: Some(536_870_913),
                    ..Default::default()
                },
            ),
            (
                "frames",
                LimitOverrides {
                    image_max_frames: Some(1_000),
                    ..Default::default()
                },
                LimitOverrides {
                    image_max_frames: Some(1_001),
                    ..Default::default()
                },
            ),
            (
                "live",
                LimitOverrides {
                    live_refresh_ms: Some(60_000),
                    ..Default::default()
                },
                LimitOverrides {
                    live_refresh_ms: Some(60_001),
                    ..Default::default()
                },
            ),
            (
                "frame",
                LimitOverrides {
                    frame_max_delay_ms: Some(60_000),
                    ..Default::default()
                },
                LimitOverrides {
                    frame_max_delay_ms: Some(60_001),
                    ..Default::default()
                },
            ),
            (
                "frame minimum",
                LimitOverrides {
                    frame_min_delay_ms: Some(1_000),
                    ..Default::default()
                },
                LimitOverrides {
                    frame_min_delay_ms: Some(1_001),
                    ..Default::default()
                },
            ),
        ];
        for (name, valid, invalid) in cases {
            assert!(Limits::default().apply(&valid).is_ok(), "{name}");
            assert!(Limits::default().apply(&invalid).is_err(), "{name}");
        }
    }
}
