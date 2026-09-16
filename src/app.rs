use std::{
    env, fmt,
    io::{self, Write},
    path::{Path, PathBuf},
    time::Duration,
};

use crate::{
    cli::{Cli, Command},
    collect,
    config::{self, ImagePathOrigin, ImageSource, Settings},
    model::{Diagnostic, DiagnosticSeverity, Snapshot, SnapshotMode},
    render::{
        image::{self, RenderMode},
        json::render_json,
        terminal,
        text::{TextOptions, card_body, render_text},
        tui::{self, LiveError, LiveOptions, SnapshotProvider},
    },
};

#[derive(Debug)]
pub enum AppError {
    Config(config::ConfigError),
    Image(image::ImageError),
    Terminal(terminal::OneShotError),
    Live(LiveError),
    Io(io::Error),
    Json(serde_json::Error),
}

impl fmt::Display for AppError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Config(error) => error.fmt(f),
            Self::Image(error) => error.fmt(f),
            Self::Terminal(error) => error.fmt(f),
            Self::Live(error) => error.fmt(f),
            Self::Io(error) => error.fmt(f),
            Self::Json(error) => write!(f, "cannot serialize JSON output: {error}"),
        }
    }
}

impl std::error::Error for AppError {}

impl From<config::ConfigError> for AppError {
    fn from(error: config::ConfigError) -> Self {
        Self::Config(error)
    }
}
impl From<image::ImageError> for AppError {
    fn from(error: image::ImageError) -> Self {
        Self::Image(error)
    }
}
impl From<terminal::OneShotError> for AppError {
    fn from(error: terminal::OneShotError) -> Self {
        Self::Terminal(error)
    }
}
impl From<LiveError> for AppError {
    fn from(error: LiveError) -> Self {
        Self::Live(error)
    }
}
impl From<io::Error> for AppError {
    fn from(error: io::Error) -> Self {
        Self::Io(error)
    }
}
impl From<serde_json::Error> for AppError {
    fn from(error: serde_json::Error) -> Self {
        Self::Json(error)
    }
}

pub fn run() -> Result<(), AppError> {
    run_cli(
        Cli::parse_and_validate(),
        &env::current_dir().map_err(AppError::Io)?,
    )
}

fn run_cli(cli: Cli, cwd: &Path) -> Result<(), AppError> {
    if let Some(Command::Hook { shell }) = cli.command {
        println!("{}", crate::hooks::generate(shell));
        return Ok(());
    }
    // Outside a worktree, hooks must be silent even when user config is broken.
    if cli.hook_context && hook_context_suppressed(cwd) {
        return Ok(());
    }

    if cli.hook_context {
        let settings = hook_context_settings(&cli)?;
        return run_one_shot(cli, cwd, settings);
    }

    let loaded = config::load_config(cli.config.as_deref())?;
    let settings = config::resolve_settings(&cli, &loaded, env::var("NO_COLOR").ok().as_deref())?;
    match cli.command {
        Some(Command::Live) => run_live(cli, cwd, settings),
        None => run_one_shot(cli, cwd, settings),
        Some(Command::Hook { .. }) => {
            unreachable!("hook commands return before settings resolution")
        }
    }
}

fn hook_context_settings(cli: &Cli) -> Result<Settings, AppError> {
    let loaded = config::LoadedConfig {
        source: config::ConfigSource::BuiltIn,
        file: config::FileConfig::default(),
    };
    config::resolve_settings(cli, &loaded, env::var("NO_COLOR").ok().as_deref())
        .map_err(AppError::Config)
}

fn collect_snapshot(cwd: &Path, full: bool, mode: SnapshotMode, settings: &Settings) -> Snapshot {
    let project = collect::collect(
        cwd,
        full,
        &settings.limits,
        settings.coverage_report_path.as_deref(),
    );
    let system = collect::system::collect();
    let time = collect::time::collect();
    Snapshot {
        schema_version: crate::model::SCHEMA_VERSION,
        mode,
        context: project.context,
        project: project.project,
        git: project.git,
        system: system.system,
        time: time.time,
        diagnostics: project
            .diagnostics
            .into_iter()
            .chain(system.diagnostics)
            .chain(time.diagnostics)
            .collect(),
    }
}

fn one_shot_identity_height(body: &crate::render::text::CardBody, full: bool) -> usize {
    if full {
        body.identity_layout_height
    } else {
        body.identity.len()
    }
}

fn run_one_shot(cli: Cli, cwd: &Path, settings: Settings) -> Result<(), AppError> {
    // Hooks intentionally avoid all collection outside a worktree.
    if cli.hook_context && hook_context_suppressed(cwd) {
        return Ok(());
    }
    let mut snapshot = collect_snapshot(
        cwd,
        cli.full,
        if cli.hook_context {
            SnapshotMode::Hook
        } else {
            SnapshotMode::OneShot
        },
        &settings,
    );
    if cli.json {
        let mut stdout = io::stdout().lock();
        writeln!(stdout, "{}", render_json(&snapshot)?)?;
        return Ok(());
    }

    let caps = terminal::detect_capabilities(RenderMode::OneShot, &settings.image);
    if !caps.stdout_tty
        || cli.hook_context
        || matches!(settings.image.source, ImageSource::Disabled)
        || settings.image.protocol == crate::cli::Protocol::None
    {
        return print_text(&plain_text(&snapshot, cli.full, &settings, caps));
    }

    let decision = match image::select_protocol(RenderMode::OneShot, settings.image.protocol, caps)
    {
        Ok(decision) => decision,
        Err(error) => {
            return protocol_failure(
                cli.protocol.is_some(),
                error,
                &mut snapshot,
                cli.full,
                &settings,
                caps,
            );
        }
    };
    if matches!(
        decision.protocol,
        image::SelectedProtocol::None
            | image::SelectedProtocol::Text
            | image::SelectedProtocol::BoundedQueryRequired
    ) {
        return print_text(&plain_text(&snapshot, cli.full, &settings, caps));
    }
    if matches!(
        decision.protocol,
        image::SelectedProtocol::Iterm2
            | image::SelectedProtocol::Sixel
            | image::SelectedProtocol::Halfblocks
    ) {
        warn_unsupported_one_shot(&mut snapshot, settings.image.protocol);
        return print_text(&plain_text(&snapshot, cli.full, &settings, caps));
    }
    let width = terminal::one_shot_width(terminal_size().width, cli.full);
    let body = card_body(&snapshot, cli.full);
    let identity_height = one_shot_identity_height(&body, cli.full);
    // Geometry is decided before decoding or serializing so a dropped image has
    // no APC transfer or placeholder side effects.
    if width < 20
        || body.identity.is_empty()
        || (settings.image.position == config::ImagePosition::Left && identity_height < 3)
        || !terminal::one_shot_image_eligible(width, settings.image.position)
    {
        return print_text(&plain_text(&snapshot, cli.full, &settings, caps));
    }
    let image_result = match &settings.image.source {
        ImageSource::Bundled => image::load_bundled_logo(&settings.limits),
        ImageSource::Path { path, .. } => image::load_image(path, &settings.limits),
        ImageSource::Disabled => unreachable!("disabled sources return before loading"),
    };
    let decoded = match image_result {
        Ok(decoded) => decoded,
        Err(error) => {
            return image_failure(
                &settings.image.source,
                error,
                &mut snapshot,
                cli.full,
                &settings,
                caps,
            );
        }
    };
    let Some(cell_pixels) = caps.cell_pixels else {
        return protocol_failure(
            cli.protocol.is_some(),
            image::ImageError::MissingGeometry {
                protocol: settings.image.protocol,
            },
            &mut snapshot,
            cli.full,
            &settings,
            caps,
        );
    };
    let (serialized, cells) = match terminal::prepare_kitty_one_shot(
        &decoded,
        &settings.image,
        &settings.limits,
        cell_pixels,
        width,
        identity_height as u16,
        decision.is_tmux,
    ) {
        Ok(prepared) => prepared,
        Err(terminal::OneShotError::Image(error)) => {
            return image_failure(
                &settings.image.source,
                error,
                &mut snapshot,
                cli.full,
                &settings,
                caps,
            );
        }
        Err(terminal::OneShotError::Narrow { .. }) => {
            return print_text(&plain_text(&snapshot, cli.full, &settings, caps));
        }
        Err(error) => return Err(error.into()),
    };
    let rows = terminal::compose_one_shot_card(
        &body,
        cli.full,
        width,
        settings.color_enabled,
        settings.theme,
        Some((&serialized, cells, settings.image.position)),
    );
    let mut stdout = io::stdout().lock();
    stdout.write_all(&serialized.transfer)?;
    stdout.write_all(rows.as_bytes())?;
    stdout.flush()?;
    Ok(())
}

fn hook_context_suppressed(cwd: &Path) -> bool {
    collect::git::workdir(cwd).is_none()
}

fn image_failure(
    source: &ImageSource,
    error: image::ImageError,
    snapshot: &mut Snapshot,
    full: bool,
    settings: &Settings,
    caps: image::TerminalCapabilities,
) -> Result<(), AppError> {
    if matches!(
        source,
        ImageSource::Path {
            origin: ImagePathOrigin::Cli,
            ..
        }
    ) {
        return Err(error.into());
    }
    warn_image_fallback(snapshot, &error, "falling back to text");
    print_text(&plain_text(snapshot, full, settings, caps))
}

fn warn_image_fallback(snapshot: &mut Snapshot, error: &image::ImageError, detail: &str) {
    eprintln!("warning: {error}; {detail}");
    snapshot.diagnostics.push(Diagnostic {
        severity: DiagnosticSeverity::Warning,
        code: "image-fallback".into(),
        message: error.to_string(),
        subject: None,
    });
}

fn protocol_failure(
    explicit: bool,
    error: image::ImageError,
    snapshot: &mut Snapshot,
    full: bool,
    settings: &Settings,
    caps: image::TerminalCapabilities,
) -> Result<(), AppError> {
    if explicit {
        return Err(error.into());
    }
    warn_image_fallback(snapshot, &error, "falling back to text");
    print_text(&plain_text(snapshot, full, settings, caps))
}

fn warn_unsupported_one_shot(snapshot: &mut Snapshot, protocol: crate::cli::Protocol) {
    let message =
        format!("one-shot {protocol:?} image transport is unsupported; falling back to text");
    eprintln!("warning: {message}");
    snapshot.diagnostics.push(Diagnostic {
        severity: DiagnosticSeverity::Warning,
        code: "image-one-shot-unsupported".into(),
        message,
        subject: None,
    });
}

fn plain_text(
    snapshot: &Snapshot,
    full: bool,
    settings: &Settings,
    caps: image::TerminalCapabilities,
) -> String {
    render_text(
        snapshot,
        TextOptions {
            full,
            width: caps.stdout_tty.then(|| usize::from(terminal_size().width)),
            color: caps.stdout_tty && settings.color_enabled,
            theme: settings.theme,
            embedded: false,
        },
    )
}

fn print_text(text: &str) -> Result<(), AppError> {
    let mut stdout = io::stdout().lock();
    writeln!(stdout, "{text}")?;
    Ok(())
}

fn terminal_size() -> ratatui::layout::Size {
    crossterm::terminal::size()
        .map(|(width, height)| ratatui::layout::Size::new(width, height))
        .unwrap_or_else(|_| ratatui::layout::Size::new(80, 24))
}

fn run_live(cli: Cli, cwd: &Path, settings: Settings) -> Result<(), AppError> {
    let caps = terminal::detect_capabilities(RenderMode::Live, &settings.image);
    terminal::prevalidate_live(caps)?;
    let decision = image::select_protocol(RenderMode::Live, settings.image.protocol, caps)?;
    let mut initial = collect_snapshot(cwd, cli.full, SnapshotMode::Live, &settings);
    let session = image::ImageSession::new(decision);
    let selected = tui::resolve_live_protocol(&session, caps);
    let decoded = if !should_load_live_source(&settings.image.source, selected) {
        None
    } else if pixel_protocol_without_geometry(decision, caps) {
        let error = image::ImageError::MissingGeometry {
            protocol: settings.image.protocol,
        };
        if cli.protocol.is_some() {
            return Err(error.into());
        }
        warn_image_fallback(&mut initial, &error, "live mode will use text only");
        None
    } else {
        match load_live_source(&settings.image.source, &settings.limits) {
            Ok(decoded) => Some(decoded),
            Err(error)
                if matches!(
                    settings.image.source,
                    ImageSource::Path {
                        origin: ImagePathOrigin::Cli,
                        ..
                    }
                ) =>
            {
                return Err(error.into());
            }
            Err(error) => {
                warn_image_fallback(&mut initial, &error, "live mode will use text only");
                None
            }
        }
    };
    tui::run_live(
        initial,
        RuntimeProvider {
            cwd: cwd.to_path_buf(),
            full: cli.full,
            settings: settings.clone(),
        },
        settings,
        LiveOptions { full: cli.full },
        decoded,
        caps,
        session,
    )?;
    Ok(())
}

fn should_load_live_source(source: &ImageSource, protocol: image::SelectedProtocol) -> bool {
    !matches!(source, ImageSource::Disabled)
        && !matches!(
            protocol,
            image::SelectedProtocol::Text | image::SelectedProtocol::None
        )
}

fn load_live_source(
    source: &ImageSource,
    limits: &crate::limits::Limits,
) -> Result<image::DecodedImage, image::ImageError> {
    match source {
        ImageSource::Bundled => image::load_bundled_logo(limits),
        ImageSource::Path { path, .. } => image::load_image(path, limits),
        ImageSource::Disabled => unreachable!("disabled sources return before loading"),
    }
}

fn pixel_protocol_without_geometry(
    decision: image::ProtocolDecision,
    caps: image::TerminalCapabilities,
) -> bool {
    caps.cell_pixels.is_none()
        && matches!(
            decision.protocol,
            image::SelectedProtocol::Kitty
                | image::SelectedProtocol::Iterm2
                | image::SelectedProtocol::Sixel
        )
}

struct RuntimeProvider {
    cwd: PathBuf,
    full: bool,
    settings: Settings,
}

impl SnapshotProvider for RuntimeProvider {
    fn refresh_interval(&self) -> Duration {
        Duration::from_millis(self.settings.limits.live_refresh_ms)
    }

    fn refresh(&mut self) -> Result<Snapshot, LiveError> {
        Ok(collect_snapshot(
            &self.cwd,
            self.full,
            SnapshotMode::Live,
            &self.settings,
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn one_shot_uses_full_identity_layout_height_only_for_full_cards() {
        let snapshot = Snapshot::empty(SnapshotMode::OneShot);
        let full = card_body(&snapshot, true);
        let normal = card_body(&snapshot, false);
        assert_eq!(one_shot_identity_height(&full, true), 6);
        assert_eq!(
            one_shot_identity_height(&normal, false),
            normal.identity.len()
        );
    }

    fn non_tty_caps() -> image::TerminalCapabilities {
        image::TerminalCapabilities {
            stdin_tty: false,
            stdout_tty: false,
            terminal: image::TerminalKind::Other,
            cell_pixels: None,
            is_tmux: false,
        }
    }

    #[test]
    fn hook_context_is_suppressed_outside_a_worktree() {
        let path = std::env::temp_dir().join(format!("hoshino-no-repo-{}", std::process::id()));
        std::fs::create_dir_all(&path).unwrap();
        assert!(hook_context_suppressed(&path));
        std::fs::remove_dir(&path).unwrap();
    }

    #[test]
    fn snapshot_outside_git_keeps_system_and_time_but_omits_project_data() {
        let path = std::env::temp_dir().join(format!("hoshino-runtime-{}", std::process::id()));
        std::fs::create_dir_all(&path).unwrap();
        let snapshot = collect_snapshot(&path, false, SnapshotMode::OneShot, &Settings::default());
        std::fs::remove_dir(&path).unwrap();
        assert!(snapshot.git.is_none());
        assert!(snapshot.project.is_none());
        assert!(snapshot.context.directory.is_some());
    }

    #[test]
    fn hook_snapshot_keeps_project_data_inside_a_worktree() {
        let path = std::env::temp_dir().join(format!("hoshino-hook-repo-{}", std::process::id()));
        std::fs::create_dir_all(&path).unwrap();
        drop(git2::Repository::init(&path).unwrap());
        std::fs::write(path.join("card.rs"), "fn main() {}\n").unwrap();

        let snapshot = collect_snapshot(&path, false, SnapshotMode::Hook, &Settings::default());

        std::fs::remove_dir_all(&path).unwrap();
        assert_eq!(snapshot.mode, SnapshotMode::Hook);
        assert!(snapshot.git.is_some());
        assert!(snapshot.project.is_some());
    }

    #[test]
    fn compact_json_is_control_free() {
        let output = render_json(&Snapshot::empty(SnapshotMode::OneShot)).unwrap();
        assert!(serde_json::from_str::<serde_json::Value>(&output).is_ok());
        assert!(!output.bytes().any(|byte| byte < 0x20 || byte == 0x1b));
    }

    #[test]
    fn hook_generation_bypasses_config_loading() {
        let cli = Cli {
            full: false,
            json: false,
            config: Some(PathBuf::from("/definitely/missing/hoshino-config.toml")),
            color: None,
            image: None,
            no_image: false,
            protocol: None,
            hook_context: false,
            command: Some(Command::Hook {
                shell: crate::cli::Shell::Bash,
            }),
        };
        assert!(run_cli(cli, Path::new(".")).is_ok());
    }

    #[test]
    fn hook_context_outside_git_bypasses_config_loading() {
        let path =
            std::env::temp_dir().join(format!("hoshino-hook-no-repo-{}", std::process::id()));
        std::fs::create_dir_all(&path).unwrap();
        let cli = Cli {
            full: false,
            json: false,
            config: Some(PathBuf::from("/definitely/missing/hoshino-config.toml")),
            color: None,
            image: None,
            no_image: false,
            protocol: None,
            hook_context: true,
            command: None,
        };
        assert!(run_cli(cli, &path).is_ok());
        std::fs::remove_dir(&path).unwrap();
    }

    #[test]
    fn hook_context_inside_git_ignores_malformed_config_without_home_mutation() {
        let path = std::env::temp_dir().join(format!(
            "hoshino-hook-malformed-config-{}",
            std::process::id()
        ));
        std::fs::create_dir_all(&path).unwrap();
        drop(git2::Repository::init(&path).unwrap());
        let config_path = path.join("malformed.toml");
        std::fs::write(&config_path, "theme = [\n").unwrap();
        let cli = Cli {
            full: false,
            json: false,
            config: Some(config_path),
            color: None,
            image: None,
            no_image: false,
            protocol: None,
            hook_context: true,
            command: None,
        };
        let home_before = std::env::var_os("HOME");

        let settings = hook_context_settings(&cli).unwrap();
        assert!(settings.image.path.is_none());
        assert_eq!(settings.image.protocol, crate::cli::Protocol::Auto);
        assert!(run_cli(cli, &path).is_ok());
        assert_eq!(std::env::var_os("HOME"), home_before);

        std::fs::remove_dir_all(&path).unwrap();
    }

    #[test]
    fn non_tty_plain_and_json_boundaries_are_control_free() {
        let settings = Settings {
            color_enabled: true,
            ..Settings::default()
        };
        let snapshot = Snapshot::empty(SnapshotMode::OneShot);
        let plain = plain_text(&snapshot, false, &settings, non_tty_caps());
        let json = render_json(&snapshot).unwrap();

        assert!(!plain.contains('\x1b'));
        assert!(!plain.contains("\x1b_G"));
        assert!(!json.contains('\x1b'));
        assert!(!json.contains("\x1b_G"));
    }

    #[test]
    fn config_image_fallback_is_added_before_plain_text_is_rendered() {
        let mut snapshot = Snapshot::empty(SnapshotMode::OneShot);
        let error = image::ImageError::UnsupportedFormat;
        warn_image_fallback(&mut snapshot, &error, "falling back to text");
        let plain = plain_text(&snapshot, false, &Settings::default(), non_tty_caps());

        assert!(plain.contains("DIAG:"));
        assert!(plain.contains("image format"));
    }

    #[test]
    fn valid_prepared_one_shot_image_serializes_without_a_fallback_diagnostic() {
        let image_settings = crate::config::ImageSettings {
            width_cells: 1,
            max_height_rows: 1,
            ..crate::config::ImageSettings::default()
        };
        let decoded = image::DecodedImage {
            format: ::image::ImageFormat::Png,
            first_frame: ::image::RgbaImage::new(1, 1),
            frames: vec![],
        };
        let (serialized, cells) = terminal::prepare_kitty_one_shot(
            &decoded,
            &image_settings,
            &Settings::default().limits,
            image::CellPixels::new(8, 16).unwrap(),
            80,
            6,
            false,
        )
        .unwrap();
        assert!(!serialized.transfer.is_empty());
        assert_eq!(serialized.placeholder_rows.len(), usize::from(cells.height));
    }

    #[test]
    fn auto_text_and_protocol_none_are_silent_one_shot_paths() {
        let caps = image::TerminalCapabilities {
            stdin_tty: false,
            stdout_tty: true,
            terminal: image::TerminalKind::Other,
            cell_pixels: None,
            is_tmux: false,
        };
        let auto =
            image::select_protocol(RenderMode::OneShot, crate::cli::Protocol::Auto, caps).unwrap();
        assert_eq!(auto.protocol, image::SelectedProtocol::Text);
        let none =
            image::select_protocol(RenderMode::OneShot, crate::cli::Protocol::None, caps).unwrap();
        assert_eq!(none.protocol, image::SelectedProtocol::None);
    }

    #[test]
    fn protocol_and_image_failures_keep_their_own_sources() {
        assert!(matches!(
            Settings::default().image.source,
            ImageSource::Bundled
        ));
        assert!(pixel_protocol_without_geometry(
            image::ProtocolDecision {
                protocol: image::SelectedProtocol::Kitty,
                is_tmux: false,
            },
            non_tty_caps(),
        ));
        assert!(!pixel_protocol_without_geometry(
            image::ProtocolDecision {
                protocol: image::SelectedProtocol::Halfblocks,
                is_tmux: false,
            },
            non_tty_caps(),
        ));
    }

    #[test]
    fn live_source_resolution_honors_bundled_paths_disabled_and_text_protocols() {
        let limits = Settings::default().limits;
        assert!(load_live_source(&ImageSource::Bundled, &limits).is_ok());
        assert!(should_load_live_source(
            &ImageSource::Bundled,
            image::SelectedProtocol::Kitty
        ));
        assert!(!should_load_live_source(
            &ImageSource::Bundled,
            image::SelectedProtocol::None
        ));
        assert!(!should_load_live_source(
            &ImageSource::Disabled,
            image::SelectedProtocol::Kitty
        ));

        for origin in [ImagePathOrigin::Cli, ImagePathOrigin::Config] {
            let source = ImageSource::Path {
                path: PathBuf::from("/definitely/missing/hoshino-image.png"),
                origin,
            };
            assert!(should_load_live_source(
                &source,
                image::SelectedProtocol::Kitty
            ));
            assert!(load_live_source(&source, &limits).is_err());
        }
    }
}
