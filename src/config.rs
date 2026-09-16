use std::{
    env, fs,
    path::{Path, PathBuf},
};

use serde::Deserialize;

use crate::{
    cli::{Cli, ColorMode, Protocol},
    limits::{LimitError, LimitOverrides, Limits},
};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Platform {
    Linux,
    Macos,
    Windows,
}

#[derive(Clone, Debug, Deserialize, Default, PartialEq, Eq)]
#[serde(default, deny_unknown_fields)]
pub struct FileConfig {
    pub theme: Option<Theme>,
    pub color: Option<ColorMode>,
    pub image: ImageConfig,
    pub coverage_report_path: Option<PathBuf>,
    pub limits: LimitOverrides,
}

#[derive(Clone, Debug, Default, Deserialize, PartialEq, Eq)]
#[serde(default, deny_unknown_fields)]
pub struct ImageConfig {
    pub path: Option<PathBuf>,
    pub protocol: Option<Protocol>,
    pub width_cells: Option<u16>,
    pub max_height_rows: Option<u16>,
    pub fit: Option<ImageFit>,
    pub position: Option<ImagePosition>,
    pub cell_width_px: Option<u16>,
    pub cell_height_px: Option<u16>,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq)]
#[serde(rename_all = "kebab-case")]
pub enum ImageFit {
    Contain,
    Cover,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq)]
#[serde(rename_all = "kebab-case")]
pub enum ImagePosition {
    Left,
    Top,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq)]
#[serde(rename_all = "kebab-case")]
pub enum Theme {
    Dusk,
    DuskDarker,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ConfigSource {
    BuiltIn,
    Default(PathBuf),
    Explicit(PathBuf),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LoadedConfig {
    pub source: ConfigSource,
    pub file: FileConfig,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Settings {
    pub theme: Theme,
    pub color: ColorMode,
    pub color_enabled: bool,
    pub image: ImageSettings,
    pub coverage_report_path: Option<PathBuf>,
    pub limits: Limits,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            theme: Theme::DuskDarker,
            color: ColorMode::Auto,
            color_enabled: true,
            image: ImageSettings::default(),
            coverage_report_path: None,
            limits: Limits::default(),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ImageSettings {
    /// The resolved source is authoritative; `path` remains until app wiring
    /// consumes this explicit source contract.
    pub source: ImageSource,
    pub path: Option<PathBuf>,
    pub protocol: Protocol,
    pub width_cells: u16,
    pub max_height_rows: u16,
    pub fit: ImageFit,
    pub position: ImagePosition,
    pub cell_width_px: Option<u16>,
    pub cell_height_px: Option<u16>,
}
impl Default for ImageSettings {
    fn default() -> Self {
        Self {
            source: ImageSource::Bundled,
            path: None,
            protocol: Protocol::Auto,
            width_cells: 24,
            max_height_rows: 12,
            fit: ImageFit::Contain,
            position: ImagePosition::Left,
            cell_width_px: None,
            cell_height_px: None,
        }
    }
}
impl ImageSettings {
    fn apply(&mut self, file: &ImageConfig) {
        if let Some(path) = &file.path {
            self.source = ImageSource::Path {
                path: path.clone(),
                origin: ImagePathOrigin::Config,
            };
            self.path = Some(path.clone());
        }
        self.protocol = file.protocol.unwrap_or(self.protocol);
        self.width_cells = file.width_cells.unwrap_or(self.width_cells);
        self.max_height_rows = file.max_height_rows.unwrap_or(self.max_height_rows);
        self.fit = file.fit.unwrap_or(self.fit);
        self.position = file.position.unwrap_or(self.position);
        self.cell_width_px = file.cell_width_px;
        self.cell_height_px = file.cell_height_px;
    }
    fn validate(&self) -> Result<(), ImageConfigError> {
        if !(1..=120).contains(&self.width_cells) {
            return Err(ImageConfigError(
                "image.width_cells must be between 1 and 120",
            ));
        }
        if !(1..=60).contains(&self.max_height_rows) {
            return Err(ImageConfigError(
                "image.max_height_rows must be between 1 and 60",
            ));
        }
        if self.cell_width_px.is_some() != self.cell_height_px.is_some() {
            return Err(ImageConfigError(
                "image.cell_width_px and image.cell_height_px must be configured together",
            ));
        }
        for (field, value) in [
            ("image.cell_width_px", self.cell_width_px),
            ("image.cell_height_px", self.cell_height_px),
        ] {
            if let Some(value) = value
                && !(1..=1_000).contains(&value)
            {
                return Err(ImageConfigError(if field == "image.cell_width_px" {
                    "image.cell_width_px must be between 1 and 1000"
                } else {
                    "image.cell_height_px must be between 1 and 1000"
                }));
            }
        }
        Ok(())
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ImagePathOrigin {
    Cli,
    Config,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ImageSource {
    Disabled,
    Bundled,
    Path {
        path: PathBuf,
        origin: ImagePathOrigin,
    },
}
#[derive(Clone, Debug)]
pub struct ImageConfigError(&'static str);
impl std::fmt::Display for ImageConfigError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.0)
    }
}
impl std::error::Error for ImageConfigError {}

pub fn current_default_config_path() -> PathBuf {
    let home = env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."));
    let xdg = env::var_os("XDG_CONFIG_HOME").map(PathBuf::from);
    let app_data = env::var_os("APPDATA").map(PathBuf::from);
    platform_config_path(
        current_platform(),
        &home,
        xdg.as_deref(),
        app_data.as_deref(),
    )
}

pub fn platform_config_path(
    platform: Platform,
    home: &Path,
    xdg_config_home: Option<&Path>,
    app_data: Option<&Path>,
) -> PathBuf {
    let base = match platform {
        Platform::Linux | Platform::Macos => xdg_config_home
            .map(Path::to_path_buf)
            .unwrap_or_else(|| home.join(".config")),
        Platform::Windows => app_data
            .map(Path::to_path_buf)
            .unwrap_or_else(|| home.join("AppData/Roaming")),
    };
    base.join("hoshino/config.toml")
}

const fn current_platform() -> Platform {
    if cfg!(target_os = "windows") {
        Platform::Windows
    } else if cfg!(target_os = "macos") {
        Platform::Macos
    } else {
        Platform::Linux
    }
}

pub fn load_config(explicit: Option<&Path>) -> Result<LoadedConfig, ConfigError> {
    load_config_with_default(explicit, &current_default_config_path())
}

pub fn load_config_with_default(
    explicit: Option<&Path>,
    default_path: &Path,
) -> Result<LoadedConfig, ConfigError> {
    let (path, source, required) = match explicit {
        Some(path) => (
            path.to_path_buf(),
            ConfigSource::Explicit(path.to_path_buf()),
            true,
        ),
        None => {
            let path = default_path.to_path_buf();
            (path.clone(), ConfigSource::Default(path), false)
        }
    };
    match fs::read_to_string(&path) {
        Ok(contents) => toml::from_str(&contents)
            .map(|file| LoadedConfig { source, file })
            .map_err(|error| ConfigError::Malformed {
                path,
                message: error.to_string(),
            }),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound && !required => {
            Ok(LoadedConfig {
                source: ConfigSource::BuiltIn,
                file: FileConfig::default(),
            })
        }
        Err(error) => Err(ConfigError::Read {
            path,
            message: error.to_string(),
        }),
    }
}

pub fn resolve_settings(
    cli: &Cli,
    loaded: &LoadedConfig,
    no_color: Option<&str>,
) -> Result<Settings, ConfigError> {
    let mut settings = Settings::default();
    settings.theme = loaded.file.theme.unwrap_or(settings.theme);
    settings.color = loaded.file.color.unwrap_or(settings.color);
    settings.image.apply(&loaded.file.image);
    settings.coverage_report_path = loaded.file.coverage_report_path.clone();
    settings
        .limits
        .apply(&loaded.file.limits)
        .map_err(ConfigError::Limits)?;

    settings.color = cli.color.unwrap_or(settings.color);
    settings.image.protocol = cli.protocol.unwrap_or(settings.image.protocol);
    if cli.no_image {
        settings.image.source = ImageSource::Disabled;
        settings.image.path = None;
    } else if let Some(image) = &cli.image {
        settings.image.source = ImageSource::Path {
            path: image.clone(),
            origin: ImagePathOrigin::Cli,
        };
        settings.image.path = Some(image.clone());
    }
    settings.image.validate().map_err(ConfigError::Image)?;
    settings.color_enabled = match settings.color {
        ColorMode::Always => true,
        ColorMode::Never => false,
        ColorMode::Auto => no_color.is_none_or(|value| value.is_empty()),
    };
    Ok(settings)
}

#[derive(Debug)]
pub enum ConfigError {
    Read { path: PathBuf, message: String },
    Malformed { path: PathBuf, message: String },
    Limits(LimitError),
    Image(ImageConfigError),
}
impl std::fmt::Display for ConfigError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Read { path, message } => {
                write!(f, "cannot read config {}: {message}", path.display())
            }
            Self::Malformed { path, message } => {
                write!(f, "invalid config {}: {message}", path.display())
            }
            Self::Limits(error) => write!(f, "invalid config: {error}"),
            Self::Image(error) => write!(f, "invalid config: {error}"),
        }
    }
}
impl std::error::Error for ConfigError {}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        sync::atomic::{AtomicUsize, Ordering},
        time::{SystemTime, UNIX_EPOCH},
    };

    fn temp_path(name: &str) -> PathBuf {
        static NEXT: AtomicUsize = AtomicUsize::new(0);
        std::env::temp_dir().join(format!(
            "hoshino-{name}-{}-{}",
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ))
    }
    fn cli(args: &[&str]) -> Cli {
        Cli::try_parse_from(args).unwrap()
    }

    #[test]
    fn paths_are_hoshino_owned_on_every_platform() {
        let home = Path::new("/home/example");
        assert_eq!(
            platform_config_path(Platform::Linux, home, Some(Path::new("/xdg")), None),
            PathBuf::from("/xdg/hoshino/config.toml")
        );
        assert_eq!(
            platform_config_path(Platform::Macos, home, None, None),
            PathBuf::from("/home/example/.config/hoshino/config.toml")
        );
        assert_eq!(
            platform_config_path(Platform::Windows, home, None, Some(Path::new("C:/AppData"))),
            PathBuf::from("C:/AppData/hoshino/config.toml")
        );
    }

    #[test]
    fn missing_default_is_accepted_but_explicit_missing_is_actionable() {
        let path = temp_path("missing");
        assert!(load_config_with_default(None, &path).is_ok());
        assert!(
            matches!(load_config(Some(&path)), Err(ConfigError::Read { path: got, .. }) if got == path)
        );
    }

    #[test]
    fn malformed_config_reports_path_and_toml_line() {
        let path = temp_path("bad");
        fs::write(&path, "theme = [\n").unwrap();
        let error = load_config(Some(&path)).unwrap_err().to_string();
        fs::remove_file(path).unwrap();
        assert!(error.contains("invalid config"));
        assert!(error.contains("line"));
    }

    #[test]
    fn cli_overrides_file_and_builtins() {
        let loaded = LoadedConfig {
            source: ConfigSource::BuiltIn,
            file: toml::from_str("theme = 'dusk'\ncolor = 'never'\ncoverage_report_path = 'coverage/custom.info'\n[image]\npath = 'config.png'\nprotocol = 'sixel'\nwidth_cells = 32\nmax_height_rows = 14\nfit = 'cover'\nposition = 'top'\ncell_width_px = 8\ncell_height_px = 16\n").unwrap(),
        };
        let settings = resolve_settings(
            &cli(&[
                "hoshino",
                "--color",
                "always",
                "--protocol",
                "kitty",
                "--image",
                "cli.png",
            ]),
            &loaded,
            Some("1"),
        )
        .unwrap();
        assert_eq!(settings.theme, Theme::Dusk);
        assert_eq!(settings.color, ColorMode::Always);
        assert_eq!(settings.image.protocol, Protocol::Kitty);
        assert_eq!(settings.image.path, Some(PathBuf::from("cli.png")));
        assert_eq!(
            settings.image.source,
            ImageSource::Path {
                path: PathBuf::from("cli.png"),
                origin: ImagePathOrigin::Cli,
            }
        );
        assert_eq!(settings.image.width_cells, 32);
        assert_eq!(settings.image.max_height_rows, 14);
        assert_eq!(settings.image.fit, ImageFit::Cover);
        assert_eq!(settings.image.position, ImagePosition::Top);
        assert_eq!(settings.image.cell_width_px, Some(8));
        assert_eq!(
            settings.coverage_report_path,
            Some(PathBuf::from("coverage/custom.info"))
        );
        assert!(settings.color_enabled);
    }

    #[test]
    fn no_color_only_changes_auto_color() {
        let loaded = LoadedConfig {
            source: ConfigSource::BuiltIn,
            file: FileConfig::default(),
        };
        assert!(
            !resolve_settings(&cli(&["hoshino"]), &loaded, Some("1"))
                .unwrap()
                .color_enabled
        );
        assert!(
            resolve_settings(&cli(&["hoshino", "--color", "always"]), &loaded, Some("1"))
                .unwrap()
                .color_enabled
        );
        assert!(
            resolve_settings(&cli(&["hoshino"]), &loaded, Some(""))
                .unwrap()
                .color_enabled
        );
    }

    #[test]
    fn image_geometry_requires_a_pixel_pair_and_reasonable_bounds() {
        for file in [
            "[image]\ncell_width_px = 8\n",
            "[image]\nwidth_cells = 0\n",
            "[image]\nmax_height_rows = 61\n",
            "[image]\ncell_width_px = 1001\ncell_height_px = 16\n",
        ] {
            let loaded = LoadedConfig {
                source: ConfigSource::BuiltIn,
                file: toml::from_str(file).unwrap(),
            };
            assert!(matches!(
                resolve_settings(&cli(&["hoshino"]), &loaded, None),
                Err(ConfigError::Image(_))
            ));
        }
        let loaded = LoadedConfig { source: ConfigSource::BuiltIn, file: toml::from_str("[image]\nwidth_cells = 120\nmax_height_rows = 60\ncell_width_px = 1000\ncell_height_px = 1000\n").unwrap() };
        assert!(resolve_settings(&cli(&["hoshino"]), &loaded, None).is_ok());
    }

    #[test]
    fn image_source_precedence_is_explicit() {
        let loaded = LoadedConfig {
            source: ConfigSource::BuiltIn,
            file: toml::from_str("[image]\npath = 'config.png'\nprotocol = 'kitty'\n").unwrap(),
        };
        let bundled = resolve_settings(
            &cli(&["hoshino"]),
            &LoadedConfig {
                source: ConfigSource::BuiltIn,
                file: FileConfig::default(),
            },
            None,
        )
        .unwrap();
        assert_eq!(bundled.image.source, ImageSource::Bundled);
        let disabled_without_overrides = resolve_settings(
            &cli(&["hoshino", "--no-image"]),
            &LoadedConfig {
                source: ConfigSource::BuiltIn,
                file: FileConfig::default(),
            },
            None,
        )
        .unwrap();
        assert_eq!(
            disabled_without_overrides.image.source,
            ImageSource::Disabled
        );

        let configured = resolve_settings(&cli(&["hoshino"]), &loaded, None).unwrap();
        assert_eq!(
            configured.image.source,
            ImageSource::Path {
                path: PathBuf::from("config.png"),
                origin: ImagePathOrigin::Config,
            }
        );

        let cli_override =
            resolve_settings(&cli(&["hoshino", "--image", "cli.png"]), &loaded, None).unwrap();
        assert_eq!(
            cli_override.image.source,
            ImageSource::Path {
                path: PathBuf::from("cli.png"),
                origin: ImagePathOrigin::Cli,
            }
        );

        let disabled = resolve_settings(&cli(&["hoshino", "--no-image"]), &loaded, None).unwrap();
        assert_eq!(disabled.image.source, ImageSource::Disabled);
        assert_eq!(disabled.image.path, None);
        assert_eq!(disabled.image.protocol, Protocol::Kitty);
    }
}
