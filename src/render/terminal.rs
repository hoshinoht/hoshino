//! Terminal capability discovery, fixed one-shot image preparation, and live ownership guards.

use std::{
    io::{self, IsTerminal, Write},
    num::NonZeroU32,
};

use crossterm::{
    cursor::{Hide, Show},
    execute,
    terminal::{self, EnterAlternateScreen, LeaveAlternateScreen},
};
use image::{
    DynamicImage, ExtendedColorType, ImageEncoder, RgbaImage, codecs::png::PngEncoder, imageops,
};
use ratatui::layout::{Rect, Size};
use ratatui_image::protocol::{
    Protocol as FixedProtocol, halfblocks::Halfblocks, iterm2::Iterm2, kitty::Kitty, sixel::Sixel,
};

use crate::{
    config::{ImagePosition, ImageSettings, Theme},
    limits::Limits,
    render::image::{
        CellPixels, DecodedImage, ImageError, ImageSession, PixelRect, ProtocolDecision,
        RenderMode, ResizePlan, SelectedProtocol, TerminalCapabilities, TerminalKind, resize_plan,
    },
    render::{
        kitty_one_shot::{self, SerializedImage},
        text::{CardBody, CardLine, CardSpan, layout_line, section_rail, section_rows},
        theme::{Palette, Role, SpanStyle, expand_span},
    },
};

const MIN_TEXT_COLUMNS: u16 = 20;
const MIN_LEFT_IMAGE_COLUMNS: u16 = 43;

pub fn one_shot_width(columns: u16, full: bool) -> u16 {
    columns.min(if full { 120 } else { 104 })
}

pub fn one_shot_image_eligible(width: u16, position: ImagePosition) -> bool {
    width
        >= match position {
            ImagePosition::Left => MIN_LEFT_IMAGE_COLUMNS,
            ImagePosition::Top => 34,
        }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct WindowGeometry {
    pub columns: u16,
    pub rows: u16,
    pub width_px: u16,
    pub height_px: u16,
}

pub trait TerminalProbe {
    fn stdout_is_terminal(&mut self) -> bool;
    fn stdin_is_terminal(&mut self) -> bool;
    fn env_var(&mut self, name: &str) -> Option<String>;
    fn window_geometry(&mut self) -> Option<WindowGeometry>;
}

struct SystemProbe;

impl TerminalProbe for SystemProbe {
    fn stdout_is_terminal(&mut self) -> bool {
        io::stdout().is_terminal()
    }

    fn stdin_is_terminal(&mut self) -> bool {
        io::stdin().is_terminal()
    }

    fn env_var(&mut self, name: &str) -> Option<String> {
        std::env::var(name).ok()
    }

    fn window_geometry(&mut self) -> Option<WindowGeometry> {
        terminal::window_size().ok().map(|size| WindowGeometry {
            columns: size.columns,
            rows: size.rows,
            width_px: size.width,
            height_px: size.height,
        })
    }
}

/// Detect without emitting terminal queries.  A non-TTY stdout returns before
/// looking at stdin, environment state, or terminal geometry.
pub fn detect_capabilities(mode: RenderMode, settings: &ImageSettings) -> TerminalCapabilities {
    detect_capabilities_with(&mut SystemProbe, mode, settings)
}

pub fn detect_capabilities_with(
    probe: &mut impl TerminalProbe,
    mode: RenderMode,
    settings: &ImageSettings,
) -> TerminalCapabilities {
    if !probe.stdout_is_terminal() {
        return TerminalCapabilities {
            stdin_tty: false,
            stdout_tty: false,
            terminal: TerminalKind::Other,
            cell_pixels: None,
            is_tmux: false,
        };
    }

    let stdin_tty = mode == RenderMode::Live && probe.stdin_is_terminal();
    let term_program = probe.env_var("TERM_PROGRAM");
    let term = probe.env_var("TERM");
    let terminal = if term_program
        .as_deref()
        .is_some_and(|value| value.eq_ignore_ascii_case("ghostty"))
        || term
            .as_deref()
            .is_some_and(|value| value.contains("ghostty"))
    {
        TerminalKind::Ghostty
    } else if term_program.as_deref() == Some("iTerm.app") {
        TerminalKind::Iterm2
    } else {
        TerminalKind::Other
    };
    let is_tmux = probe.env_var("TMUX").is_some();
    let configured = settings
        .cell_width_px
        .zip(settings.cell_height_px)
        .and_then(|(width, height)| CellPixels::new(u32::from(width), u32::from(height)).ok());
    let cell_pixels = configured.or_else(|| {
        let geometry = probe.window_geometry()?;
        derive_cell_pixels(geometry)
    });
    TerminalCapabilities {
        stdin_tty,
        stdout_tty: true,
        terminal,
        cell_pixels,
        is_tmux,
    }
}

pub fn derive_cell_pixels(geometry: WindowGeometry) -> Option<CellPixels> {
    if geometry.columns == 0
        || geometry.rows == 0
        || geometry.width_px == 0
        || geometry.height_px == 0
    {
        return None;
    }
    CellPixels::new(
        u32::from(geometry.width_px) / u32::from(geometry.columns),
        u32::from(geometry.height_px) / u32::from(geometry.rows),
    )
    .ok()
}

#[derive(Debug)]
pub enum OneShotError {
    Image(ImageError),
    Backend(String),
    Narrow { available: u16, required: u16 },
}

impl From<ImageError> for OneShotError {
    fn from(error: ImageError) -> Self {
        Self::Image(error)
    }
}

impl std::fmt::Display for OneShotError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Image(error) => error.fmt(f),
            Self::Backend(error) => write!(f, "cannot prepare terminal image: {error}"),
            Self::Narrow {
                available,
                required,
            } => write!(
                f,
                "terminal is too narrow for image and compact text ({available} columns; need {required})"
            ),
        }
    }
}

impl std::error::Error for OneShotError {}

#[derive(Clone)]
pub struct PreparedImage {
    pub protocol: FixedProtocol,
    pub cells: Size,
    pub resize: Option<ResizePlan>,
}

pub enum OneShotPrepared {
    Image(PreparedImage),
    TextFallback,
}

pub fn prepare_one_shot(
    decision: ProtocolDecision,
    decoded: &DecodedImage,
    settings: &ImageSettings,
    session: &ImageSession,
    cell_pixels: Option<CellPixels>,
) -> Result<OneShotPrepared, OneShotError> {
    let cells = Size::new(settings.width_cells, settings.max_height_rows);
    let source = PixelRect {
        width: decoded.first_frame.width(),
        height: decoded.first_frame.height(),
    };
    let fixed = match decision.protocol {
        SelectedProtocol::None
        | SelectedProtocol::Text
        | SelectedProtocol::BoundedQueryRequired => {
            return Ok(OneShotPrepared::TextFallback);
        }
        SelectedProtocol::Halfblocks => FixedProtocol::Halfblocks(
            Halfblocks::new(DynamicImage::ImageRgba8(decoded.first_frame.clone()), cells)
                .map_err(|error| OneShotError::Backend(error.to_string()))?,
        ),
        SelectedProtocol::Kitty | SelectedProtocol::Iterm2 | SelectedProtocol::Sixel => {
            let pixels = cell_pixels.ok_or(ImageError::InvalidGeometry(
                "pixel protocols require non-query cell pixel geometry",
            ))?;
            let plan = resize_plan(
                source,
                PixelRect {
                    width: u32::from(cells.width),
                    height: u32::from(cells.height),
                },
                pixels,
                settings.fit,
            )?;
            let image = resize_frame(&decoded.first_frame, plan);
            let image = DynamicImage::ImageRgba8(image);
            let protocol = match decision.protocol {
                SelectedProtocol::Kitty => FixedProtocol::Kitty(
                    Kitty::new(image, cells, session.id.get(), decision.is_tmux)
                        .map_err(|error| OneShotError::Backend(error.to_string()))?,
                ),
                SelectedProtocol::Iterm2 => FixedProtocol::ITerm2(
                    Iterm2::new(image, cells, decision.is_tmux)
                        .map_err(|error| OneShotError::Backend(error.to_string()))?,
                ),
                SelectedProtocol::Sixel => FixedProtocol::Sixel(
                    Sixel::new(image, cells, decision.is_tmux)
                        .map_err(|error| OneShotError::Backend(error.to_string()))?,
                ),
                _ => unreachable!(),
            };
            return Ok(OneShotPrepared::Image(PreparedImage {
                protocol,
                cells,
                resize: Some(plan),
            }));
        }
    };
    Ok(OneShotPrepared::Image(PreparedImage {
        protocol: fixed,
        cells,
        resize: None,
    }))
}

fn resize_frame(source: &RgbaImage, plan: ResizePlan) -> RgbaImage {
    let resized = imageops::resize(
        source,
        plan.resized.width,
        plan.resized.height,
        imageops::FilterType::CatmullRom,
    );
    match plan.crop {
        Some(crop) => {
            imageops::crop_imm(&resized, crop.x, crop.y, crop.width, crop.height).to_image()
        }
        None => resized,
    }
}

/// Fully prepares the query-free Kitty stream before stdout is touched.
pub fn prepare_kitty_one_shot(
    decoded: &DecodedImage,
    settings: &ImageSettings,
    limits: &Limits,
    cell_pixels: CellPixels,
    terminal_columns: u16,
    identity_height: u16,
    tmux: bool,
) -> Result<(SerializedImage, Size), OneShotError> {
    let max_columns = match settings.position {
        ImagePosition::Left => terminal_columns
            .checked_sub(37)
            .ok_or(OneShotError::Narrow {
                available: terminal_columns,
                required: MIN_LEFT_IMAGE_COLUMNS,
            })?,
        ImagePosition::Top => terminal_columns
            .checked_sub(2)
            .ok_or(OneShotError::Narrow {
                available: terminal_columns,
                required: MIN_TEXT_COLUMNS,
            })?,
    };
    if max_columns == 0 || terminal_columns < MIN_TEXT_COLUMNS {
        return Err(OneShotError::Narrow {
            available: terminal_columns,
            required: MIN_TEXT_COLUMNS,
        });
    }
    let requested = Size::new(
        settings.width_cells.min(max_columns),
        match settings.position {
            ImagePosition::Left => settings
                .max_height_rows
                .min(identity_height.saturating_sub(2)),
            ImagePosition::Top => settings.max_height_rows,
        },
    );
    if requested.height == 0 {
        return Err(OneShotError::Narrow {
            available: identity_height,
            required: 3,
        });
    }
    let plan = resize_plan(
        PixelRect {
            width: decoded.first_frame.width(),
            height: decoded.first_frame.height(),
        },
        PixelRect {
            width: u32::from(requested.width),
            height: u32::from(requested.height),
        },
        cell_pixels,
        settings.fit,
    )?;
    ensure_image_bytes(plan.resized, limits)?;
    if let Some(crop) = plan.crop {
        ensure_image_bytes(
            PixelRect {
                width: crop.width,
                height: crop.height,
            },
            limits,
        )?;
    }
    let transformed = resize_frame(&decoded.first_frame, plan);
    let columns = cells_for(transformed.width(), cell_pixels.width, requested.width)?;
    let rows = cells_for(transformed.height(), cell_pixels.height, requested.height)?;
    let png = encode_png(&transformed, limits)?;
    let serialized =
        kitty_one_shot::serialize(&png, kitty_one_shot::next_image_id(), columns, rows, tmux)
            .map_err(|_| OneShotError::Image(ImageError::Limit("one-shot image transport")))?;
    Ok((serialized, Size::new(columns, rows)))
}

fn cells_for(pixels: u32, cell_pixels: u32, maximum: u16) -> Result<u16, OneShotError> {
    let cells = pixels
        .checked_add(cell_pixels - 1)
        .ok_or(ImageError::InvalidGeometry("cell geometry overflows"))?
        / cell_pixels;
    let cells =
        u16::try_from(cells).map_err(|_| ImageError::InvalidGeometry("cell geometry overflows"))?;
    Ok(cells.clamp(1, maximum))
}

fn ensure_image_bytes(rect: PixelRect, limits: &Limits) -> Result<(), OneShotError> {
    let bytes = u64::from(rect.width)
        .checked_mul(u64::from(rect.height))
        .and_then(|pixels| pixels.checked_mul(4))
        .ok_or(ImageError::Limit("decoded bytes"))?;
    if bytes > limits.image_max_decoded_bytes {
        return Err(ImageError::Limit("decoded bytes").into());
    }
    Ok(())
}

fn encode_png(image: &RgbaImage, limits: &Limits) -> Result<Vec<u8>, OneShotError> {
    let mut writer = CappedWriter::new(limits.image_max_file_bytes);
    PngEncoder::new(&mut writer)
        .write_image(
            image.as_raw(),
            image.width(),
            image.height(),
            ExtendedColorType::Rgba8,
        )
        .map_err(|error| {
            if writer.exceeded {
                OneShotError::Image(ImageError::Limit("file bytes"))
            } else {
                OneShotError::Backend(error.to_string())
            }
        })?;
    Ok(writer.bytes)
}

struct CappedWriter {
    bytes: Vec<u8>,
    limit: u64,
    exceeded: bool,
}

impl CappedWriter {
    fn new(limit: u64) -> Self {
        Self {
            bytes: Vec::new(),
            limit,
            exceeded: false,
        }
    }
}

impl Write for CappedWriter {
    fn write(&mut self, buffer: &[u8]) -> io::Result<usize> {
        let length =
            u64::try_from(buffer.len()).map_err(|_| io::Error::other("PNG output too large"))?;
        let next = u64::try_from(self.bytes.len())
            .ok()
            .and_then(|current| current.checked_add(length));
        if next.is_none_or(|size| size > self.limit) {
            self.exceeded = true;
            return Err(io::Error::other("PNG output exceeds configured file limit"));
        }
        self.bytes.extend_from_slice(buffer);
        Ok(buffer.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

/// Composes the single append-safe one-shot frame. Placeholder rows are already
/// serialized by the fixed Kitty transport; this only places them inside it.
pub fn compose_one_shot_card(
    body: &CardBody,
    full: bool,
    width: u16,
    color: bool,
    theme: Theme,
    image: Option<(&SerializedImage, Size, ImagePosition)>,
) -> String {
    let width = usize::from(width);
    debug_assert!(width >= 20);
    let inner = width - 2;
    let palette = Palette::for_theme(theme);
    let mut rows = vec![title_row(body, inner, color, palette)];
    match image {
        Some((image, cells, ImagePosition::Left))
            if width >= usize::from(MIN_LEFT_IMAGE_COLUMNS)
                && placeholder_rows_match(image, cells)
                && usize::from(cells.width) + 2 <= inner - 33 =>
        {
            let image_width = usize::from(cells.width);
            let image_slot = image_width + 2;
            // `│ image │ text │`: the divider and frame each keep one cell of padding.
            let text_width = inner - image_slot - 3;
            let identity_height = if full {
                body.identity_layout_height
            } else {
                body.identity.len()
            };
            let placeholder_height = image.placeholder_rows.len().min(identity_height);
            let top_padding = (identity_height - placeholder_height) / 2;
            for index in 0..identity_height {
                let left = if (top_padding..top_padding + placeholder_height).contains(&index) {
                    image.placeholder_rows[index - top_padding]
                        .strip_suffix('\n')
                        .unwrap_or(&image.placeholder_rows[index - top_padding])
                        .to_owned()
                } else {
                    " ".repeat(image_width)
                };
                rows.push(format!(
                    "{} {} {} {} {}",
                    styled("│", Role::Border, SpanStyle::Solid, color, palette),
                    left,
                    styled("│", Role::Border, SpanStyle::Solid, color, palette),
                    body.identity.get(index).map_or_else(
                        || " ".repeat(text_width),
                        |line| serialize_line(line, text_width, color, palette)
                    ),
                    styled("│", Role::Border, SpanStyle::Solid, color, palette),
                ));
            }
        }
        Some((image, cells, ImagePosition::Top))
            if width >= 34
                && placeholder_rows_match(image, cells)
                && usize::from(cells.width) <= inner =>
        {
            let image_width = usize::from(cells.width);
            let left_padding = (inner - image_width) / 2;
            for placeholder in &image.placeholder_rows {
                rows.push(format!(
                    "{}{}{}{}{}",
                    styled("│", Role::Border, SpanStyle::Solid, color, palette),
                    " ".repeat(left_padding),
                    placeholder.strip_suffix('\n').unwrap_or(placeholder),
                    " ".repeat(inner - image_width - left_padding),
                    styled("│", Role::Border, SpanStyle::Solid, color, palette),
                ));
            }
            rows.extend(
                body.identity
                    .iter()
                    .map(|line| framed_line(line, inner, color, palette)),
            );
        }
        _ => rows.extend(
            body.identity
                .iter()
                .map(|line| framed_line(line, inner, color, palette)),
        ),
    }
    if full {
        for section in &body.sections {
            let heading = section_rail(section.name, inner);
            rows.push(framed_section_rail(&heading, inner, color, palette));
            rows.extend(
                section_rows(section, inner - 2, width >= 80)
                    .iter()
                    .map(|line| framed_line(line, inner, color, palette)),
            );
        }
    }
    rows.push(format!(
        "{}{}{}",
        styled("╰", Role::Border, SpanStyle::Solid, color, palette),
        styled(
            &"─".repeat(inner),
            Role::Border,
            SpanStyle::Solid,
            color,
            palette
        ),
        styled("╯", Role::Border, SpanStyle::Solid, color, palette),
    ));
    format!("{}\n", rows.join("\n"))
}

fn placeholder_rows_match(image: &SerializedImage, cells: Size) -> bool {
    image.placeholder_rows.len() == usize::from(cells.height)
        && image.placeholder_rows.iter().all(|row| {
            cell_width(&strip_ansi(row.strip_suffix('\n').unwrap_or(row)))
                == usize::from(cells.width)
        })
}

fn title_row(body: &CardBody, inner: usize, color: bool, palette: Palette) -> String {
    let title = serialize_spans(&body.title, inner.saturating_sub(2), color, palette, false);
    let title_width = cell_width(&strip_ansi(&title));
    format!(
        "{}{}{}{}{}",
        styled("╭─", Role::Border, SpanStyle::Solid, color, palette),
        title,
        styled(" ", Role::Border, SpanStyle::Solid, color, palette),
        styled(
            &"─".repeat(inner.saturating_sub(title_width + 2)),
            Role::Border,
            SpanStyle::Solid,
            color,
            palette
        ),
        styled("╮", Role::Border, SpanStyle::Solid, color, palette),
    )
}

/// `│ content │` with one cell of padding on each side of the content.
fn framed_line(line: &CardLine, inner: usize, color: bool, palette: Palette) -> String {
    format!(
        "{} {} {}",
        styled("│", Role::Border, SpanStyle::Solid, color, palette),
        serialize_line(line, inner - 2, color, palette),
        styled("│", Role::Border, SpanStyle::Solid, color, palette),
    )
}
fn framed_section_rail(line: &CardLine, inner: usize, color: bool, palette: Palette) -> String {
    format!(
        "{}{}{}",
        styled("├", Role::Border, SpanStyle::Solid, color, palette),
        serialize_line(line, inner, color, palette),
        styled("┤", Role::Border, SpanStyle::Solid, color, palette),
    )
}

fn serialize_line(line: &CardLine, width: usize, color: bool, palette: Palette) -> String {
    serialize_spans(&layout_line(line, width), width, color, palette, true)
}

fn serialize_spans(
    spans: &[CardSpan],
    width: usize,
    color: bool,
    palette: Palette,
    pad: bool,
) -> String {
    let mut output = String::new();
    let mut used = 0;
    for span in spans {
        if used == width {
            break;
        }
        let text = fit_cells(&span.text, width - used);
        used += cell_width(&text);
        for chunk in expand_span(&text, span.role, span.style, palette) {
            output.push_str(&if color {
                palette.ansi_foreground_rgb(chunk.color)
            } else {
                String::new()
            });
            output.push_str(&chunk.text);
            if color {
                output.push_str("\x1b[0m");
            }
        }
    }
    if pad {
        output.push_str(&" ".repeat(width - used));
    }
    output
}

fn styled(text: &str, role: Role, style: SpanStyle, color: bool, palette: Palette) -> String {
    serialize_spans(
        &[CardSpan {
            text: text.into(),
            role,
            style,
        }],
        cell_width(text),
        color,
        palette,
        true,
    )
}

fn cell_width(value: &str) -> usize {
    ratatui::text::Line::from(value).width()
}

fn fit_cells(value: &str, width: usize) -> String {
    if cell_width(value) <= width {
        return value.into();
    }
    if width == 0 {
        return String::new();
    }
    if width == 1 {
        return "…".into();
    }
    let left = (width - 1) / 2;
    let right = width - 1 - left;
    format!(
        "{}…{}",
        take_cells(value.chars(), left),
        take_cells(value.chars().rev(), right)
            .chars()
            .rev()
            .collect::<String>()
    )
}

fn take_cells(chars: impl Iterator<Item = char>, limit: usize) -> String {
    let mut output = String::new();
    for character in chars {
        if cell_width(&format!("{output}{character}")) > limit {
            break;
        }
        output.push(character);
    }
    output
}

fn strip_ansi(value: &str) -> String {
    let mut output = String::new();
    let mut escape = false;
    let mut csi = false;
    for character in value.chars() {
        if csi {
            if ('@'..='~').contains(&character) {
                csi = false;
            }
        } else if escape {
            escape = false;
            csi = character == '[';
        } else if character == '\x1b' {
            escape = true;
        } else {
            output.push(character);
        }
    }
    output
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct OneShotLayout {
    pub image: Rect,
    pub text: Rect,
    pub height: u16,
}

pub fn one_shot_layout(
    size: Size,
    cells: Size,
    position: ImagePosition,
    text_lines: u16,
) -> Result<OneShotLayout, OneShotError> {
    let text_lines = text_lines.max(1);
    match position {
        ImagePosition::Left => {
            let required = cells.width.saturating_add(MIN_TEXT_COLUMNS);
            if size.width < required {
                return Err(OneShotError::Narrow {
                    available: size.width,
                    required,
                });
            }
            let height = cells.height.max(text_lines);
            Ok(OneShotLayout {
                image: Rect::new(0, 0, cells.width, cells.height),
                text: Rect::new(cells.width, 0, size.width - cells.width, text_lines),
                height,
            })
        }
        ImagePosition::Top => {
            if size.width < MIN_TEXT_COLUMNS {
                return Err(OneShotError::Narrow {
                    available: size.width,
                    required: MIN_TEXT_COLUMNS,
                });
            }
            Ok(OneShotLayout {
                image: Rect::new(0, 0, cells.width.min(size.width), cells.height),
                text: Rect::new(0, cells.height, size.width, text_lines),
                height: cells.height.saturating_add(text_lines),
            })
        }
    }
}

pub trait TerminalOperations {
    fn enable_raw(&mut self) -> io::Result<()>;
    fn enter_alternate(&mut self) -> io::Result<()>;
    fn hide_cursor(&mut self) -> io::Result<()>;
    fn show_cursor(&mut self) -> io::Result<()>;
    fn leave_alternate(&mut self) -> io::Result<()>;
    fn disable_raw(&mut self) -> io::Result<()>;
}

/// Proof that both streams were checked before live mode takes terminal ownership.
#[derive(Clone, Copy, Debug)]
pub struct LivePreflight(());

pub fn prevalidate_live(caps: TerminalCapabilities) -> Result<LivePreflight, ImageError> {
    if caps.stdin_tty && caps.stdout_tty {
        Ok(LivePreflight(()))
    } else {
        Err(ImageError::LiveRequiresTtys)
    }
}

pub struct CrosstermOperations<W: Write> {
    writer: W,
}

impl<W: Write> CrosstermOperations<W> {
    pub fn new(writer: W) -> Self {
        Self { writer }
    }
}

impl<W: Write> TerminalOperations for CrosstermOperations<W> {
    fn enable_raw(&mut self) -> io::Result<()> {
        terminal::enable_raw_mode()
    }
    fn enter_alternate(&mut self) -> io::Result<()> {
        execute!(self.writer, EnterAlternateScreen)
    }
    fn hide_cursor(&mut self) -> io::Result<()> {
        execute!(self.writer, Hide)
    }
    fn show_cursor(&mut self) -> io::Result<()> {
        execute!(self.writer, Show)
    }
    fn leave_alternate(&mut self) -> io::Result<()> {
        execute!(self.writer, LeaveAlternateScreen)
    }
    fn disable_raw(&mut self) -> io::Result<()> {
        terminal::disable_raw_mode()
    }
}

/// Owns only state hoshino successfully entered, restoring it in reverse order.
pub struct LiveTerminalGuard<'a, O: TerminalOperations> {
    operations: &'a mut O,
    raw: bool,
    alternate: bool,
    cursor_hidden: bool,
}

impl<'a, O: TerminalOperations> LiveTerminalGuard<'a, O> {
    pub fn enter(operations: &'a mut O, _preflight: LivePreflight) -> io::Result<Self> {
        let mut guard = Self {
            operations,
            raw: false,
            alternate: false,
            cursor_hidden: false,
        };
        guard.operations.enable_raw()?;
        guard.raw = true;
        if let Err(error) = guard.operations.enter_alternate() {
            let _ = guard.restore();
            return Err(error);
        }
        guard.alternate = true;
        if let Err(error) = guard.operations.hide_cursor() {
            let _ = guard.restore();
            return Err(error);
        }
        guard.cursor_hidden = true;
        Ok(guard)
    }

    pub fn restore(&mut self) -> io::Result<()> {
        let mut first_error = None;
        if self.cursor_hidden {
            if let Err(error) = self.operations.show_cursor() {
                first_error = Some(error);
            }
            self.cursor_hidden = false;
        }
        if self.alternate {
            if let Err(error) = self.operations.leave_alternate()
                && first_error.is_none()
            {
                first_error = Some(error);
            }
            self.alternate = false;
        }
        if self.raw {
            if let Err(error) = self.operations.disable_raw()
                && first_error.is_none()
            {
                first_error = Some(error);
            }
            self.raw = false;
        }
        first_error.map_or(Ok(()), Err)
    }
}

impl<O: TerminalOperations> Drop for LiveTerminalGuard<'_, O> {
    fn drop(&mut self) {
        let _ = self.restore();
    }
}

/// The only raw graphics control sequence hoshino owns: deletion of its own
/// caller-generated Kitty image ID after live placeholders have disappeared.
pub fn kitty_delete_owned(id: NonZeroU32, is_tmux: bool) -> Vec<u8> {
    let payload = format!("_Ga=d,d=I,i={},q=2", id.get());
    if is_tmux {
        format!("\x1bPtmux;\x1b\x1b{payload}\x1b\x1b\\\x1b\\").into_bytes()
    } else {
        format!("\x1b{payload}\x1b\\").into_bytes()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    use crate::{config::ImageFit, render::image::ProtocolDecision};

    #[derive(Default)]
    struct Probe {
        stdout: bool,
        stdin: bool,
        env: HashMap<String, String>,
        geometry: Option<WindowGeometry>,
        calls: Vec<&'static str>,
    }

    impl TerminalProbe for Probe {
        fn stdout_is_terminal(&mut self) -> bool {
            self.calls.push("stdout");
            self.stdout
        }
        fn stdin_is_terminal(&mut self) -> bool {
            self.calls.push("stdin");
            self.stdin
        }
        fn env_var(&mut self, name: &str) -> Option<String> {
            self.calls.push("env");
            self.env.get(name).cloned()
        }
        fn window_geometry(&mut self) -> Option<WindowGeometry> {
            self.calls.push("geometry");
            self.geometry
        }
    }

    fn settings() -> ImageSettings {
        ImageSettings::default()
    }
    fn decoded() -> DecodedImage {
        DecodedImage {
            format: image::ImageFormat::Png,
            first_frame: RgbaImage::new(400, 100),
            frames: vec![],
        }
    }
    fn session(protocol: SelectedProtocol) -> ImageSession {
        ImageSession::new(ProtocolDecision {
            protocol,
            is_tmux: true,
        })
    }

    #[test]
    fn non_tty_stops_before_every_other_probe() {
        let mut probe = Probe::default();
        let caps = detect_capabilities_with(&mut probe, RenderMode::OneShot, &settings());
        assert!(!caps.stdout_tty);
        assert_eq!(probe.calls, ["stdout"]);
    }

    #[test]
    fn live_checks_both_ttys_but_does_not_query() {
        let mut probe = Probe {
            stdout: true,
            stdin: true,
            ..Probe::default()
        };
        probe.env.insert("TERM_PROGRAM".into(), "Ghostty".into());
        probe.geometry = Some(WindowGeometry {
            columns: 100,
            rows: 50,
            width_px: 800,
            height_px: 800,
        });
        let caps = detect_capabilities_with(&mut probe, RenderMode::Live, &settings());
        assert!(caps.stdin_tty && caps.stdout_tty);
        assert_eq!(caps.terminal, TerminalKind::Ghostty);
        assert_eq!(caps.cell_pixels, Some(CellPixels::new(8, 16).unwrap()));
        assert_eq!(
            probe.calls.iter().filter(|call| **call == "stdin").count(),
            1
        );
        assert!(prevalidate_live(caps).is_ok());
        assert!(matches!(
            prevalidate_live(TerminalCapabilities {
                stdin_tty: false,
                ..caps
            }),
            Err(ImageError::LiveRequiresTtys)
        ));
    }

    #[test]
    fn invalid_window_geometry_falls_back_without_division() {
        assert_eq!(
            derive_cell_pixels(WindowGeometry {
                columns: 0,
                rows: 10,
                width_px: 800,
                height_px: 160
            }),
            None
        );
        assert_eq!(
            derive_cell_pixels(WindowGeometry {
                columns: 80,
                rows: 10,
                width_px: 0,
                height_px: 160
            }),
            None
        );
        assert_eq!(
            derive_cell_pixels(WindowGeometry {
                columns: 800,
                rows: 10,
                width_px: 80,
                height_px: 160
            }),
            None
        );
    }

    #[test]
    fn configured_geometry_wins_without_window_probe() {
        let mut probe = Probe {
            stdout: true,
            ..Probe::default()
        };
        let mut settings = settings();
        settings.cell_width_px = Some(9);
        settings.cell_height_px = Some(17);
        let caps = detect_capabilities_with(&mut probe, RenderMode::OneShot, &settings);
        assert_eq!(caps.cell_pixels, Some(CellPixels::new(9, 17).unwrap()));
        assert!(!probe.calls.contains(&"geometry"));
    }

    #[test]
    fn fixed_protocols_pre_resize_and_preserve_kitty_session_id() {
        let mut settings = settings();
        settings.width_cells = 10;
        settings.max_height_rows = 10;
        settings.fit = ImageFit::Contain;
        let session = session(SelectedProtocol::Kitty);
        let prepared = prepare_one_shot(
            session.decision,
            &decoded(),
            &settings,
            &session,
            Some(CellPixels::new(8, 16).unwrap()),
        )
        .unwrap();
        let OneShotPrepared::Image(prepared) = prepared else {
            panic!("image expected")
        };
        assert!(matches!(prepared.protocol, FixedProtocol::Kitty(_)));
        assert_eq!(
            prepared.resize.unwrap().resized,
            PixelRect {
                width: 80,
                height: 20
            }
        );
        assert_eq!(prepared.cells, Size::new(10, 10));
        assert_ne!(session.id.get(), 0);
        let mut cover = settings.clone();
        cover.fit = ImageFit::Cover;
        let prepared = prepare_one_shot(
            session.decision,
            &decoded(),
            &cover,
            &session,
            Some(CellPixels::new(8, 16).unwrap()),
        )
        .unwrap();
        let OneShotPrepared::Image(prepared) = prepared else {
            panic!("image expected")
        };
        assert_eq!(
            prepared.resize.unwrap().resized,
            PixelRect {
                width: 640,
                height: 160
            }
        );
    }

    #[test]
    fn wrappers_and_halfblocks_follow_the_selected_backend() {
        for protocol in [
            SelectedProtocol::Iterm2,
            SelectedProtocol::Sixel,
            SelectedProtocol::Halfblocks,
        ] {
            let session = session(protocol);
            let prepared =
                prepare_one_shot(session.decision, &decoded(), &settings(), &session, None);
            if protocol == SelectedProtocol::Halfblocks {
                assert!(matches!(
                    prepared.unwrap(),
                    OneShotPrepared::Image(PreparedImage {
                        protocol: FixedProtocol::Halfblocks(_),
                        ..
                    })
                ));
            } else {
                assert!(matches!(
                    prepared,
                    Err(OneShotError::Image(ImageError::InvalidGeometry(_)))
                ));
            }
        }
    }

    #[test]
    fn layout_preserves_left_top_and_narrow_text_fallback() {
        let cells = Size::new(24, 12);
        let left = one_shot_layout(Size::new(80, 30), cells, ImagePosition::Left, 5).unwrap();
        assert_eq!(left.image, Rect::new(0, 0, 24, 12));
        assert_eq!(left.text, Rect::new(24, 0, 56, 5));
        let top = one_shot_layout(Size::new(80, 30), cells, ImagePosition::Top, 5).unwrap();
        assert_eq!(top.text, Rect::new(0, 12, 80, 5));
        assert!(matches!(
            one_shot_layout(Size::new(43, 30), cells, ImagePosition::Left, 5),
            Err(OneShotError::Narrow { .. })
        ));
    }

    #[derive(Default)]
    struct Ops {
        calls: Vec<&'static str>,
        fail: Option<&'static str>,
    }
    impl Ops {
        fn call(&mut self, name: &'static str) -> io::Result<()> {
            self.calls.push(name);
            if self.fail == Some(name) {
                Err(io::Error::other("injected"))
            } else {
                Ok(())
            }
        }
    }
    impl TerminalOperations for Ops {
        fn enable_raw(&mut self) -> io::Result<()> {
            self.call("raw+")
        }
        fn enter_alternate(&mut self) -> io::Result<()> {
            self.call("alt+")
        }
        fn hide_cursor(&mut self) -> io::Result<()> {
            self.call("hide")
        }
        fn show_cursor(&mut self) -> io::Result<()> {
            self.call("show")
        }
        fn leave_alternate(&mut self) -> io::Result<()> {
            self.call("alt-")
        }
        fn disable_raw(&mut self) -> io::Result<()> {
            self.call("raw-")
        }
    }

    #[test]
    fn live_guard_restores_reverse_order_on_setup_and_render_failures() {
        let mut setup = Ops {
            fail: Some("hide"),
            ..Ops::default()
        };
        let preflight = LivePreflight(());
        assert!(LiveTerminalGuard::enter(&mut setup, preflight).is_err());
        assert_eq!(setup.calls, ["raw+", "alt+", "hide", "alt-", "raw-"]);
        let mut render = Ops::default();
        {
            let _guard = LiveTerminalGuard::enter(&mut render, preflight).unwrap();
            // A caller's rendering error drops the guard before it is returned.
        }
        assert_eq!(
            render.calls,
            ["raw+", "alt+", "hide", "show", "alt-", "raw-"]
        );
    }

    #[test]
    fn kitty_targeted_deletion_is_byte_exact_direct_and_tmux() {
        let id = NonZeroU32::new(42).unwrap();
        assert_eq!(
            kitty_delete_owned(id, false),
            b"\x1b_Ga=d,d=I,i=42,q=2\x1b\\"
        );
        assert_eq!(
            kitty_delete_owned(id, true),
            b"\x1bPtmux;\x1b\x1b_Ga=d,d=I,i=42,q=2\x1b\x1b\\\x1b\\"
        );
    }

    #[test]
    fn one_shot_card_is_inset_append_safe_and_cell_bounded() {
        let image = SerializedImage {
            transfer: vec![],
            placeholder_rows: vec!["I0\n".into(), "I1\n".into(), "I2\n".into()],
        };
        let line = |text: &str| {
            CardLine::new(
                Role::Text,
                vec![CardSpan {
                    text: text.into(),
                    role: Role::Text,
                    style: SpanStyle::Solid,
                }],
            )
        };
        let body = CardBody {
            title: vec![CardSpan {
                text: "hoshino".into(),
                role: Role::Primary,
                style: SpanStyle::Solid,
            }],
            identity: (0..6)
                .map(|index| line(&format!("identity {index}")))
                .collect(),
            identity_layout_height: 6,
            sections: vec![crate::render::text::CardSection {
                name: "details",
                role: Role::Secondary,
                left: vec![line("section row")],
                right: vec![],
            }],
        };
        for (full, cap) in [(false, 104), (true, 120)] {
            for width in [20, 21, 33, 34, 35, 40, 42, 43, 44, 80, 104, 120, 160] {
                let effective = one_shot_width(width, full).min(cap);
                for position in [ImagePosition::Left, ImagePosition::Top] {
                    let eligible = one_shot_image_eligible(effective, position);
                    for color in [false, true] {
                        let output = compose_one_shot_card(
                            &body,
                            full,
                            effective,
                            color,
                            Theme::DuskDarker,
                            eligible.then_some((&image, Size::new(2, 3), position)),
                        );
                        let lines: Vec<_> = output.lines().collect();
                        assert!(output.ends_with('\n'));
                        assert!(
                            lines.iter().all(|row| {
                                cell_width(&strip_ansi(row)) == usize::from(effective)
                            })
                        );
                        let title = strip_ansi(lines.first().unwrap());
                        assert!(title.starts_with("╭─"));
                        assert!(title.contains("hoshino"));
                        assert!(title[title.find("hoshino").unwrap() + 7..].contains('─'));
                        assert!(strip_ansi(lines.last().unwrap()).starts_with('╰'));
                        if eligible {
                            assert!(output.contains("I0"));
                        } else {
                            assert!(!output.contains("I0"));
                        }
                        if full {
                            assert!(strip_ansi(&output).contains("├─ details ─"));
                        } else {
                            assert!(!output.contains("details"));
                        }
                    }
                }
            }
        }
        let left = compose_one_shot_card(
            &body,
            true,
            43,
            false,
            Theme::DuskDarker,
            Some((&image, Size::new(2, 3), ImagePosition::Left)),
        );
        let lines: Vec<_> = left.lines().collect();
        assert!(
            lines[1..7]
                .iter()
                .all(|row| row.chars().nth(5) == Some('│'))
        );
        assert_eq!(lines[2].chars().nth(1), Some(' '));
        assert_eq!(lines[2].chars().nth(4), Some(' '));
        assert_eq!(lines[1].chars().nth(2), Some(' '));
        assert_eq!(lines[6].chars().nth(2), Some(' '));
        assert!(lines[7].starts_with("├─ details ─"));
        assert!(lines[7].ends_with('┤'));
        assert!(!lines[7].starts_with("│  │"));
        // Bands end on their last fact; there is no trailing spacer row.
        assert!(lines[lines.len() - 2].starts_with("│ section row"));
        for color in [false, true] {
            let output = compose_one_shot_card(&body, true, 120, color, Theme::DuskDarker, None);
            let rail = output
                .lines()
                .map(strip_ansi)
                .find(|line| line.contains("─ details ─"))
                .unwrap();
            assert!(rail.starts_with("├─ details ─") && rail.ends_with('┤'));
            assert_eq!(cell_width(&rail), 120);
        }
    }

    #[test]
    fn prepared_kitty_placeholders_are_bounded_to_inset_slots() {
        let line = CardLine::new(
            Role::Text,
            vec![CardSpan {
                text: "identity".into(),
                role: Role::Text,
                style: SpanStyle::Solid,
            }],
        );
        let body = CardBody {
            title: vec![CardSpan {
                text: "hoshino".into(),
                role: Role::Primary,
                style: SpanStyle::Solid,
            }],
            identity: vec![line; 6],
            identity_layout_height: 6,
            sections: vec![],
        };
        for (position, width, slot) in [
            (ImagePosition::Left, 43, 6),
            (ImagePosition::Left, 44, 7),
            (ImagePosition::Top, 34, 32),
            (ImagePosition::Top, 35, 33),
        ] {
            let mut image_settings = settings();
            image_settings.position = position;
            image_settings.width_cells = 120;
            let (serialized, cells) = prepare_kitty_one_shot(
                &decoded(),
                &image_settings,
                &Limits::default(),
                CellPixels::new(8, 16).unwrap(),
                width,
                body.identity.len() as u16,
                false,
            )
            .unwrap();
            assert_eq!(cells.width, slot);
            assert!(serialized.placeholder_rows.iter().all(|row| {
                cell_width(&strip_ansi(row.strip_suffix('\n').unwrap())) == usize::from(slot)
            }));
            if position == ImagePosition::Left {
                assert!(cells.height <= 4);
            }
            let output = compose_one_shot_card(
                &body,
                false,
                width,
                true,
                Theme::DuskDarker,
                Some((&serialized, cells, position)),
            );
            assert!(
                output
                    .lines()
                    .all(|row| { cell_width(&strip_ansi(row)) == usize::from(width) })
            );
        }
    }

    #[test]
    fn one_shot_geometry_has_exact_caps_and_image_thresholds() {
        assert_eq!(one_shot_width(160, false), 104);
        assert_eq!(one_shot_width(160, true), 120);
        for width in 1..=19 {
            assert!(!one_shot_image_eligible(width, ImagePosition::Left));
            assert!(!one_shot_image_eligible(width, ImagePosition::Top));
        }
        assert!(!one_shot_image_eligible(42, ImagePosition::Left));
        assert!(one_shot_image_eligible(43, ImagePosition::Left));
        assert!(one_shot_image_eligible(44, ImagePosition::Left));
        assert!(!one_shot_image_eligible(33, ImagePosition::Top));
        assert!(one_shot_image_eligible(34, ImagePosition::Top));
        assert!(one_shot_image_eligible(35, ImagePosition::Top));
    }

    #[test]
    fn png_encoding_round_trips_and_honors_the_file_cap() {
        let image = RgbaImage::from_pixel(4, 3, image::Rgba([1, 2, 3, 255]));
        let png = encode_png(&image, &Limits::default()).unwrap();
        let round_trip = image::load_from_memory(&png).unwrap().to_rgba8();
        assert_eq!(round_trip, image);

        let limits = Limits {
            image_max_file_bytes: 1,
            ..Limits::default()
        };
        assert!(matches!(
            encode_png(&image, &limits),
            Err(OneShotError::Image(ImageError::Limit("file bytes")))
        ));
    }
}
