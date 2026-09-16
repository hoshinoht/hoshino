//! Bounded live presentation. Image encoding is deliberately delegated to
//! `ratatui-image`'s worker protocol so terminal events remain responsive.

use std::{
    io::{self, Write},
    sync::mpsc::{self, Receiver, TryRecvError},
    thread::{self, JoinHandle},
    time::{Duration, Instant},
};

use crossterm::event::{self, Event, KeyCode, KeyEventKind};
use image::DynamicImage;
use ratatui::{
    Terminal,
    backend::CrosstermBackend,
    layout::{Rect, Size},
    style::{Color, Style},
    text::{Line, Span, Text},
    widgets::{Block, Borders, Paragraph},
};
use ratatui_image::{
    FontSize, Resize, StatefulImage,
    protocol::{
        StatefulProtocol, StatefulProtocolType, halfblocks::Halfblocks, iterm2::Iterm2,
        kitty::StatefulKitty, sixel::Sixel,
    },
    thread::{ResizeRequest, ResizeResponse, ThreadProtocol},
};

use crate::{
    config::Settings,
    model::Snapshot,
    render::{
        image::{
            DecodedImage, ImageError, ImageSession, SelectedProtocol, TerminalCapabilities,
            TerminalKind,
        },
        terminal::{
            CrosstermOperations, LiveTerminalGuard, TerminalOperations, kitty_delete_owned,
            prevalidate_live,
        },
        text::{
            CardBody, CardLine, CardSpan, card_body, expanded_card_spans, section_rail,
            section_rows,
        },
        theme::{Role, palette},
    },
};

const SPINNER: [&str; 4] = ["⠋", "⠙", "⠹", "⠸"];
const SPINNER_INTERVAL: Duration = Duration::from_millis(125);

#[derive(Debug)]
pub enum LiveError {
    Image(ImageError),
    Event(String),
    Refresh(String),
    Render(String),
    Terminal(String),
    InvalidInput(&'static str),
}

impl From<ImageError> for LiveError {
    fn from(error: ImageError) -> Self {
        Self::Image(error)
    }
}
impl std::fmt::Display for LiveError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Image(error) => error.fmt(f),
            Self::Event(error) => write!(f, "live event error: {error}"),
            Self::Refresh(error) => write!(f, "live refresh error: {error}"),
            Self::Render(error) => write!(f, "live render error: {error}"),
            Self::Terminal(error) => write!(f, "live terminal error: {error}"),
            Self::InvalidInput(error) => f.write_str(error),
        }
    }
}
impl std::error::Error for LiveError {}

/// Supplies already-collected snapshots and a cadence selected by integration.
pub trait SnapshotProvider {
    fn refresh_interval(&self) -> Duration;
    fn refresh(&mut self) -> Result<Snapshot, LiveError>;
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum LiveEvent {
    Quit,
    Resize,
    Tick,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct LiveOptions {
    pub full: bool,
}

pub trait EventSource {
    /// Validate resources that can fail before hoshino owns terminal modes.
    fn prevalidate(&mut self) -> Result<(), LiveError> {
        Ok(())
    }
    fn next(&mut self, timeout: Duration) -> Result<Option<LiveEvent>, LiveError>;
}

pub trait LiveRenderer {
    /// `image_rendered` is true only after the protocol placeholder was drawn.
    fn draw(
        &mut self,
        snapshot: &Snapshot,
        spinner: usize,
        options: LiveOptions,
    ) -> Result<bool, LiveError>;
    fn resize(&mut self) {}
}

pub struct CrosstermEventSource;
impl EventSource for CrosstermEventSource {
    fn next(&mut self, timeout: Duration) -> Result<Option<LiveEvent>, LiveError> {
        if !event::poll(timeout).map_err(|error| LiveError::Event(error.to_string()))? {
            return Ok(Some(LiveEvent::Tick));
        }
        Ok(map_crossterm_event(
            event::read().map_err(|error| LiveError::Event(error.to_string()))?,
        ))
    }
}

fn map_crossterm_event(event: Event) -> Option<LiveEvent> {
    match event {
        Event::Resize(_, _) => Some(LiveEvent::Resize),
        Event::Key(key) if key.kind == KeyEventKind::Press => match key.code {
            KeyCode::Char('q') | KeyCode::Esc => Some(LiveEvent::Quit),
            KeyCode::Char('c')
                if key
                    .modifiers
                    .contains(crossterm::event::KeyModifiers::CONTROL) =>
            {
                Some(LiveEvent::Quit)
            }
            _ => None,
        },
        _ => None,
    }
}

pub(crate) fn resolve_live_protocol(
    session: &ImageSession,
    caps: TerminalCapabilities,
) -> SelectedProtocol {
    match session.decision.protocol {
        SelectedProtocol::BoundedQueryRequired => match (caps.terminal, caps.cell_pixels) {
            (TerminalKind::Ghostty, Some(_)) => SelectedProtocol::Kitty,
            (TerminalKind::Iterm2, Some(_)) => SelectedProtocol::Iterm2,
            _ => SelectedProtocol::Text,
        },
        protocol => protocol,
    }
}

fn prevalidate_inputs(
    settings: &Settings,
    has_image: bool,
    caps: TerminalCapabilities,
    session: &ImageSession,
) -> Result<SelectedProtocol, LiveError> {
    if settings.image.width_cells == 0 || settings.image.max_height_rows == 0 {
        return Err(LiveError::InvalidInput(
            "live image cell bounds must be nonzero",
        ));
    }
    let protocol = resolve_live_protocol(session, caps);
    if has_image
        && matches!(
            protocol,
            SelectedProtocol::Kitty | SelectedProtocol::Iterm2 | SelectedProtocol::Sixel
        )
        && caps.cell_pixels.is_none()
    {
        return Err(LiveError::Image(ImageError::InvalidGeometry(
            "live pixel protocols require non-query cell pixel geometry",
        )));
    }
    Ok(protocol)
}

const MIN_LEFT_CARD_WIDTH: u16 = 43;
const MIN_TOP_CARD_WIDTH: u16 = 34;
const MIN_RAIL_WIDTH: u16 = 32;

#[derive(Clone, Debug)]
struct LivePlan {
    card: Rect,
    image: Option<Rect>,
    lines: Vec<CardLine>,
}

fn live_plan(
    area: Rect,
    settings: &Settings,
    body: CardBody,
    full: bool,
    has_image: bool,
) -> LivePlan {
    let cap = if full { 120 } else { 104 };
    let card = Rect::new(area.x, area.y, area.width.min(cap), area.height);
    let inner_width = card.width.saturating_sub(2);
    let inner_height = card.height.saturating_sub(2);
    let left_image_layout = has_image
        && settings.image.position == crate::config::ImagePosition::Left
        && card.width >= MIN_LEFT_CARD_WIDTH;
    let identity_rows = if left_image_layout && full {
        body.identity_layout_height
    } else {
        body.identity.len()
    };
    let mut semantic = body.identity.clone();
    semantic.extend(
        std::iter::repeat_with(blank_line).take(identity_rows.saturating_sub(semantic.len())),
    );
    for section in &body.sections {
        semantic.push(section_rail(
            section.name,
            section.role,
            usize::from(inner_width),
        ));
        semantic.extend(section_rows(
            section,
            usize::from(inner_width),
            card.width >= 80,
        ));
    }
    let required = u16::try_from(semantic.len()).unwrap_or(u16::MAX);
    if required > inner_height {
        return fallback_plan(card, body.identity, full);
    }

    let mut image = None;
    let mut lines = semantic;
    if left_image_layout {
        let identity_height = u16::try_from(identity_rows).unwrap_or(u16::MAX);
        let image_width = settings
            .image
            .width_cells
            .min(inner_width.saturating_sub(1 + MIN_RAIL_WIDTH + 2));
        if identity_height >= 3 && image_width > 0 {
            let image_height = settings.image.max_height_rows.min(identity_height - 2);
            if image_height > 0 {
                let top_padding = (identity_height - image_height) / 2;
                let top = card.y + 1 + top_padding;
                image = Some(Rect::new(card.x + 2, top, image_width, image_height));
                lines = inset_left_lines(lines, identity_rows, image_width, inner_width);
            }
        }
    } else if has_image
        && settings.image.position == crate::config::ImagePosition::Top
        && card.width >= MIN_TOP_CARD_WIDTH
    {
        let image_width = settings.image.width_cells.min(inner_width);
        let image_height = settings.image.max_height_rows;
        if image_width > 0
            && image_height > 0
            && required.saturating_add(image_height) <= inner_height
        {
            image = Some(Rect::new(
                card.x + 1 + (inner_width - image_width) / 2,
                card.y + 1,
                image_width,
                image_height,
            ));
            lines.splice(
                0..0,
                std::iter::repeat_with(blank_line).take(usize::from(image_height)),
            );
        }
    }
    LivePlan { card, image, lines }
}

fn blank_line() -> CardLine {
    semantic_line(Role::Text, vec![])
}

fn fallback_plan(card: Rect, identity: Vec<CardLine>, full: bool) -> LivePlan {
    let capacity = usize::from(card.height.saturating_sub(2));
    let footer = if full {
        "resize for details · full details hidden"
    } else {
        "resize for details"
    };
    let mut lines = identity
        .into_iter()
        .take(capacity.saturating_sub(1))
        .collect::<Vec<_>>();
    if capacity > 0 {
        lines.push(semantic_line(
            Role::Warning,
            vec![CardSpan {
                text: footer.into(),
                role: Role::Warning,
                style: crate::render::theme::SpanStyle::Solid,
            }],
        ));
    }
    LivePlan {
        card,
        image: None,
        lines,
    }
}

fn inset_left_lines(
    lines: Vec<CardLine>,
    identity_rows: usize,
    image_width: u16,
    inner_width: u16,
) -> Vec<CardLine> {
    let text_width = usize::from(inner_width - image_width - 3);
    lines
        .into_iter()
        .enumerate()
        .map(|(index, line)| {
            if index >= identity_rows {
                return line;
            }
            let mut fragments = vec![
                CardSpan {
                    text: " ".repeat(usize::from(image_width + 2)),
                    role: Role::Text,
                    style: crate::render::theme::SpanStyle::Solid,
                },
                CardSpan {
                    text: "│".into(),
                    role: Role::Border,
                    style: crate::render::theme::SpanStyle::Solid,
                },
            ];
            fragments.extend(fit_fragments(&line.fragments, text_width));
            semantic_line(line.role, fragments)
        })
        .collect()
}

fn semantic_line(role: Role, fragments: Vec<CardSpan>) -> CardLine {
    let text = fragments.iter().map(|span| span.text.as_str()).collect();
    CardLine {
        text,
        role,
        fragments,
    }
}

fn fit_fragments(fragments: &[CardSpan], width: usize) -> Vec<CardSpan> {
    let mut used = 0;
    let mut output = Vec::new();
    for fragment in fragments {
        if used >= width {
            break;
        }
        let text = fit_cells(&fragment.text, width - used);
        used += ratatui::text::Line::from(text.as_str()).width();
        output.push(CardSpan {
            text,
            ..fragment.clone()
        });
    }
    output
}

fn fit_cells(value: &str, width: usize) -> String {
    if ratatui::text::Line::from(value).width() <= width {
        return value.into();
    }
    if width == 0 {
        return String::new();
    }
    if width == 1 {
        return "…".into();
    }
    let left = take_cells(value.chars(), (width - 1) / 2);
    let right = take_cells(
        value.chars().rev(),
        width - 1 - ratatui::text::Line::from(left.as_str()).width(),
    )
    .chars()
    .rev()
    .collect::<String>();
    format!("{left}…{right}")
}

fn take_cells(characters: impl Iterator<Item = char>, width: usize) -> String {
    let mut output = String::new();
    for character in characters {
        let candidate = format!("{output}{character}");
        if ratatui::text::Line::from(candidate.as_str()).width() > width {
            break;
        }
        output.push(character);
    }
    output
}

/// Testable terminal ownership and loop core. Cleanup is deliberately outside
/// the guard scope, ensuring a Kitty deletion follows cursor/alternate/raw restore.
#[expect(
    clippy::too_many_arguments,
    reason = "the injected seams make terminal ownership and cleanup independently testable"
)]
pub fn run_live_with<P, E, R, O, D>(
    initial: Snapshot,
    mut provider: P,
    settings: Settings,
    options: LiveOptions,
    has_image: bool,
    caps: TerminalCapabilities,
    session: ImageSession,
    events: &mut E,
    renderer: &mut R,
    operations: &mut O,
    mut delete: D,
) -> Result<(), LiveError>
where
    P: SnapshotProvider,
    E: EventSource,
    R: LiveRenderer,
    O: TerminalOperations,
    D: FnMut() -> io::Result<()>,
{
    let protocol = prevalidate_inputs(&settings, has_image, caps, &session)?;
    events.prevalidate()?;
    let preflight = prevalidate_live(caps)?;
    let mut kitty_rendered = false;
    let result = (|| {
        let _guard = LiveTerminalGuard::enter(operations, preflight)
            .map_err(|error| LiveError::Terminal(error.to_string()))?;
        let mut snapshot = initial;
        let mut spinner = 0;
        let mut next_refresh = Instant::now() + provider.refresh_interval().max(SPINNER_INTERVAL);
        loop {
            kitty_rendered |= renderer.draw(&snapshot, spinner, options)?
                && protocol == SelectedProtocol::Kitty
                && has_image;
            let now = Instant::now();
            let until_refresh = next_refresh.saturating_duration_since(now);
            let event = events.next(until_refresh.min(SPINNER_INTERVAL))?;
            spinner = (spinner + 1) % SPINNER.len();
            match event {
                Some(LiveEvent::Quit) => break Ok(()),
                Some(LiveEvent::Resize) => renderer.resize(),
                Some(LiveEvent::Tick) | None => {}
            }
            if Instant::now() >= next_refresh {
                snapshot = provider.refresh()?;
                next_refresh = Instant::now() + provider.refresh_interval().max(SPINNER_INTERVAL);
            }
        }
    })();
    if kitty_rendered
        && let Err(error) = delete()
        && result.is_ok()
    {
        return Err(LiveError::Terminal(error.to_string()));
    }
    result
}

/// Runs the production Crossterm/Ratatui live card. `snapshot_provider` owns
/// collection and its refresh cadence; this module never probes stdio.
pub fn run_live<P: SnapshotProvider>(
    initial: Snapshot,
    snapshot_provider: P,
    settings: Settings,
    options: LiveOptions,
    decoded: Option<DecodedImage>,
    caps: TerminalCapabilities,
    session: ImageSession,
) -> Result<(), LiveError> {
    let has_image = decoded.is_some();
    let protocol = prevalidate_inputs(&settings, has_image, caps, &session)?;
    let mut renderer =
        CrosstermRenderer::new(&settings, decoded, caps, protocol, session.id.get())?;
    let mut events = CrosstermEventSource;
    let mut operations = CrosstermOperations::new(io::stdout());
    let delete_id = session.id;
    let is_tmux = session.decision.is_tmux;
    run_live_with(
        initial,
        snapshot_provider,
        settings,
        options,
        has_image,
        caps,
        session,
        &mut events,
        &mut renderer,
        &mut operations,
        move || {
            let mut stdout = io::stdout();
            stdout.write_all(&kitty_delete_owned(delete_id, is_tmux))?;
            stdout.flush()
        },
    )
}

struct LiveImage {
    protocol: Option<ThreadProtocol>,
    responses: Receiver<Result<ResizeResponse, String>>,
    worker: Option<JoinHandle<()>>,
    frames: Vec<crate::render::image::DecodedFrame>,
    frame_index: usize,
    next_frame: Instant,
    font: FontSize,
    kitty_id: Option<(u32, bool)>,
}

impl LiveImage {
    fn new(
        image: DecodedImage,
        caps: TerminalCapabilities,
        protocol: SelectedProtocol,
        id: u32,
    ) -> Result<Self, LiveError> {
        let font = match (caps.cell_pixels, protocol) {
            (Some(font), _) => FontSize::new(font.width as u16, font.height as u16),
            (None, SelectedProtocol::Halfblocks) => FontSize::new(1, 2),
            (None, _) => {
                return Err(LiveError::InvalidInput(
                    "live image needs cell pixel geometry",
                ));
            }
        };
        let frames = image.frames;
        let source = DynamicImage::ImageRgba8(image.first_frame);
        let kind = match protocol {
            SelectedProtocol::Kitty => {
                StatefulProtocolType::Kitty(StatefulKitty::new(id, caps.is_tmux))
            }
            SelectedProtocol::Iterm2 => StatefulProtocolType::ITerm2(
                Iterm2::new(source.clone(), Size::new(1, 1), caps.is_tmux)
                    .map_err(|error| LiveError::Render(error.to_string()))?,
            ),
            SelectedProtocol::Sixel => StatefulProtocolType::Sixel(
                Sixel::new(source.clone(), Size::new(1, 1), caps.is_tmux)
                    .map_err(|error| LiveError::Render(error.to_string()))?,
            ),
            SelectedProtocol::Halfblocks => StatefulProtocolType::Halfblocks(
                Halfblocks::new(source.clone(), Size::new(1, 1))
                    .map_err(|error| LiveError::Render(error.to_string()))?,
            ),
            _ => {
                return Err(LiveError::InvalidInput(
                    "text protocol has no live image state",
                ));
            }
        };
        let (request_tx, request_rx) = mpsc::channel::<ResizeRequest>();
        let (response_tx, responses) = mpsc::channel();
        let worker = thread::spawn(move || {
            while let Ok(request) = request_rx.recv() {
                let _ =
                    response_tx.send(request.resize_encode().map_err(|error| error.to_string()));
            }
        });
        Ok(Self {
            protocol: Some(ThreadProtocol::new(
                request_tx,
                Some(StatefulProtocol::new(source, font, None, kind)),
            )),
            responses,
            worker: Some(worker),
            next_frame: Instant::now()
                + frames.first().map_or(Duration::ZERO, |frame| {
                    Duration::from_millis(frame.delay_ms)
                }),
            frames,
            frame_index: 0,
            font,
            kitty_id: (protocol == SelectedProtocol::Kitty).then_some((id, caps.is_tmux)),
        })
    }

    fn render(&mut self, frame: &mut ratatui::Frame<'_>, area: Rect) -> bool {
        self.advance_kitty_frame();
        let Some(protocol) = self.protocol.as_mut() else {
            return false;
        };
        loop {
            match self.responses.try_recv() {
                Ok(Ok(response)) => {
                    let _ = protocol.update_resized_protocol(response);
                }
                Ok(Err(_)) => return false,
                Err(TryRecvError::Empty | TryRecvError::Disconnected) => break,
            }
        }
        frame.render_stateful_widget(
            StatefulImage::new().resize(Resize::Fit(None)),
            area,
            protocol,
        );
        true
    }

    fn advance_kitty_frame(&mut self) {
        let Some((id, is_tmux)) = self.kitty_id else {
            return;
        };
        if self.frames.len() < 2 || Instant::now() < self.next_frame {
            return;
        }
        self.frame_index = (self.frame_index + 1) % self.frames.len();
        let frame = &self.frames[self.frame_index];
        self.next_frame = Instant::now() + Duration::from_millis(frame.delay_ms);
        if let Some(protocol) = self.protocol.as_mut() {
            protocol.replace_protocol(StatefulProtocol::new(
                DynamicImage::ImageRgba8(frame.image.clone()),
                self.font,
                None,
                StatefulProtocolType::Kitty(StatefulKitty::new(id, is_tmux)),
            ));
        }
    }
}

impl Drop for LiveImage {
    fn drop(&mut self) {
        drop(self.protocol.take());
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}

struct CrosstermRenderer {
    terminal: Terminal<CrosstermBackend<io::Stdout>>,
    settings: Settings,
    image: Option<LiveImage>,
}

fn draw_live_plan(
    frame: &mut ratatui::Frame<'_>,
    plan: &LivePlan,
    block: Block<'_>,
    colors: crate::render::theme::Palette,
    mut render_image: impl FnMut(&mut ratatui::Frame<'_>, Rect) -> bool,
) -> bool {
    let inner = block.inner(plan.card);
    frame.render_widget(block, plan.card);
    let lines = plan
        .lines
        .iter()
        .map(|line| {
            let fitted = semantic_line(
                line.role,
                fit_fragments(&line.fragments, usize::from(inner.width)),
            );
            Line::from(
                expanded_card_spans(&fitted, colors)
                    .into_iter()
                    .map(|chunk| {
                        Span::styled(
                            chunk.text,
                            Style::default().fg(Color::Rgb(
                                chunk.color.red,
                                chunk.color.green,
                                chunk.color.blue,
                            )),
                        )
                    })
                    .collect::<Vec<_>>(),
            )
        })
        .collect::<Vec<_>>();
    frame.render_widget(Paragraph::new(Text::from(lines)), inner);
    plan.image.is_some_and(|area| render_image(frame, area))
}

impl CrosstermRenderer {
    fn new(
        settings: &Settings,
        decoded: Option<DecodedImage>,
        caps: TerminalCapabilities,
        protocol: SelectedProtocol,
        id: u32,
    ) -> Result<Self, LiveError> {
        let image = match decoded {
            Some(image)
                if !matches!(
                    protocol,
                    SelectedProtocol::Text
                        | SelectedProtocol::None
                        | SelectedProtocol::BoundedQueryRequired
                ) =>
            {
                Some(LiveImage::new(image, caps, protocol, id)?)
            }
            _ => None,
        };
        Ok(Self {
            terminal: Terminal::new(CrosstermBackend::new(io::stdout()))
                .map_err(|error| LiveError::Terminal(error.to_string()))?,
            settings: settings.clone(),
            image,
        })
    }
}

impl LiveRenderer for CrosstermRenderer {
    fn draw(
        &mut self,
        snapshot: &Snapshot,
        spinner: usize,
        options: LiveOptions,
    ) -> Result<bool, LiveError> {
        let colors = palette(self.settings.theme);
        let border = colors.color(Role::Border);
        let image = &mut self.image;
        let mut image_rendered = false;
        self.terminal
            .draw(|frame| {
                let area = frame.area();
                let block = Block::default()
                    .borders(Borders::ALL)
                    .title(format!(" hoshino · {} refresh ", SPINNER[spinner]))
                    .style(Style::default().fg(Color::Rgb(border.red, border.green, border.blue)));
                let body = card_body(snapshot, options.full);
                let plan = live_plan(area, &self.settings, body, options.full, image.is_some());
                image_rendered =
                    draw_live_plan(frame, &plan, block, colors, |frame, image_area| {
                        image
                            .as_mut()
                            .is_some_and(|image| image.render(frame, image_area))
                    });
            })
            .map_err(|error| LiveError::Render(error.to_string()))?;
        Ok(image_rendered)
    }
    fn resize(&mut self) {
        self.terminal.autoresize().ok();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        config::{ImageSettings, Theme},
        model::SnapshotMode,
        render::image::{CellPixels, ProtocolDecision},
    };

    fn caps() -> TerminalCapabilities {
        TerminalCapabilities {
            stdin_tty: true,
            stdout_tty: true,
            terminal: TerminalKind::Ghostty,
            cell_pixels: Some(CellPixels::new(8, 16).unwrap()),
            is_tmux: false,
        }
    }
    fn session(protocol: SelectedProtocol) -> ImageSession {
        ImageSession::new(ProtocolDecision {
            protocol,
            is_tmux: false,
        })
    }
    fn snapshot() -> Snapshot {
        Snapshot::empty(SnapshotMode::Live)
    }
    fn body(full: bool) -> CardBody {
        let identity = (0..6)
            .map(|index| {
                semantic_line(
                    Role::Primary,
                    vec![CardSpan {
                        text: format!("identity {index}"),
                        role: Role::Primary,
                        style: crate::render::theme::SpanStyle::Solid,
                    }],
                )
            })
            .collect();
        let sections = if full {
            vec![
                crate::render::text::CardSection {
                    name: "system & health",
                    role: Role::System(crate::render::theme::SystemRole::Health),
                    left: vec![semantic_line(
                        Role::System(crate::render::theme::SystemRole::Health),
                        vec![CardSpan {
                            text: "system detail".into(),
                            role: Role::System(crate::render::theme::SystemRole::Health),
                            style: crate::render::theme::SpanStyle::Solid,
                        }],
                    )],
                    right: vec![],
                },
                crate::render::text::CardSection {
                    name: "active context",
                    role: Role::Git,
                    left: vec![semantic_line(
                        Role::Git,
                        vec![CardSpan {
                            text: "context detail".into(),
                            role: Role::Git,
                            style: crate::render::theme::SpanStyle::Solid,
                        }],
                    )],
                    right: vec![],
                },
                crate::render::text::CardSection {
                    name: "project telemetry",
                    role: Role::Secondary,
                    left: vec![semantic_line(
                        Role::Secondary,
                        vec![CardSpan {
                            text: "telemetry detail".into(),
                            role: Role::Secondary,
                            style: crate::render::theme::SpanStyle::Solid,
                        }],
                    )],
                    right: vec![],
                },
            ]
        } else {
            Vec::new()
        };
        CardBody {
            title: vec![],
            identity,
            identity_layout_height: 6,
            sections,
        }
    }
    struct Provider {
        calls: usize,
        interval: Duration,
        fail: bool,
    }
    impl SnapshotProvider for Provider {
        fn refresh_interval(&self) -> Duration {
            self.interval
        }
        fn refresh(&mut self) -> Result<Snapshot, LiveError> {
            self.calls += 1;
            if self.fail {
                Err(LiveError::Refresh("injected".into()))
            } else {
                Ok(snapshot())
            }
        }
    }
    struct Events {
        events: Vec<Result<Option<LiveEvent>, LiveError>>,
        preflight: Result<(), LiveError>,
    }
    impl EventSource for Events {
        fn prevalidate(&mut self) -> Result<(), LiveError> {
            std::mem::replace(&mut self.preflight, Ok(()))
        }
        fn next(&mut self, _: Duration) -> Result<Option<LiveEvent>, LiveError> {
            self.events.remove(0)
        }
    }
    #[derive(Default)]
    struct Renderer {
        calls: usize,
        resized: usize,
        fail: bool,
        image: bool,
        full: Option<bool>,
    }
    impl LiveRenderer for Renderer {
        fn draw(
            &mut self,
            _: &Snapshot,
            _: usize,
            options: LiveOptions,
        ) -> Result<bool, LiveError> {
            self.calls += 1;
            self.full = Some(options.full);
            if self.fail {
                Err(LiveError::Render("injected".into()))
            } else {
                Ok(self.image)
            }
        }
        fn resize(&mut self) {
            self.resized += 1;
        }
    }
    #[derive(Default)]
    struct Ops {
        calls: Vec<&'static str>,
    }
    impl TerminalOperations for Ops {
        fn enable_raw(&mut self) -> io::Result<()> {
            self.calls.push("raw+");
            Ok(())
        }
        fn enter_alternate(&mut self) -> io::Result<()> {
            self.calls.push("alt+");
            Ok(())
        }
        fn hide_cursor(&mut self) -> io::Result<()> {
            self.calls.push("hide");
            Ok(())
        }
        fn show_cursor(&mut self) -> io::Result<()> {
            self.calls.push("show");
            Ok(())
        }
        fn leave_alternate(&mut self) -> io::Result<()> {
            self.calls.push("alt-");
            Ok(())
        }
        fn disable_raw(&mut self) -> io::Result<()> {
            self.calls.push("raw-");
            Ok(())
        }
    }
    fn run(
        events: Vec<Result<Option<LiveEvent>, LiveError>>,
        renderer: Renderer,
        protocol: SelectedProtocol,
        decoded: bool,
    ) -> (Result<(), LiveError>, Ops, Renderer, usize) {
        let mut events = Events {
            events,
            preflight: Ok(()),
        };
        let mut renderer = renderer;
        let mut ops = Ops::default();
        let mut deletes = 0;
        let result = run_live_with(
            snapshot(),
            Provider {
                calls: 0,
                interval: Duration::ZERO,
                fail: false,
            },
            Settings::default(),
            LiveOptions::default(),
            decoded,
            caps(),
            session(protocol),
            &mut events,
            &mut renderer,
            &mut ops,
            || {
                deletes += 1;
                Ok(())
            },
        );
        (result, ops, renderer, deletes)
    }
    #[test]
    fn exits_and_restores_for_q_esc_and_ctrl_c() {
        for event in [LiveEvent::Quit, LiveEvent::Quit, LiveEvent::Quit] {
            let (result, ops, _, _) = run(
                vec![Ok(Some(event))],
                Renderer::default(),
                SelectedProtocol::Text,
                false,
            );
            assert!(result.is_ok());
            assert_eq!(ops.calls, ["raw+", "alt+", "hide", "show", "alt-", "raw-"]);
        }
    }
    #[test]
    fn crossterm_exit_keys_map_without_owning_terminal() {
        use crossterm::event::{KeyEvent, KeyModifiers};

        for event in [
            Event::Key(KeyEvent::new(KeyCode::Char('q'), KeyModifiers::NONE)),
            Event::Key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE)),
            Event::Key(KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL)),
        ] {
            assert_eq!(map_crossterm_event(event), Some(LiveEvent::Quit));
        }
        assert_eq!(
            map_crossterm_event(Event::Key(KeyEvent::new(
                KeyCode::Char('c'),
                KeyModifiers::NONE,
            ))),
            None
        );
    }
    #[test]
    fn resize_and_spinner_draw_before_exit() {
        let (result, _, renderer, _) = run(
            vec![
                Ok(Some(LiveEvent::Resize)),
                Ok(Some(LiveEvent::Tick)),
                Ok(Some(LiveEvent::Quit)),
            ],
            Renderer::default(),
            SelectedProtocol::Text,
            false,
        );
        assert!(result.is_ok());
        assert_eq!(renderer.resized, 1);
        assert_eq!(renderer.calls, 3);
    }
    #[test]
    fn full_option_reaches_the_injected_renderer() {
        let mut events = Events {
            events: vec![Ok(Some(LiveEvent::Quit))],
            preflight: Ok(()),
        };
        let mut renderer = Renderer::default();
        let mut ops = Ops::default();
        let result = run_live_with(
            snapshot(),
            Provider {
                calls: 0,
                interval: Duration::ZERO,
                fail: false,
            },
            Settings::default(),
            LiveOptions { full: true },
            false,
            caps(),
            session(SelectedProtocol::Text),
            &mut events,
            &mut renderer,
            &mut ops,
            || Ok(()),
        );
        assert!(result.is_ok());
        assert_eq!(renderer.full, Some(true));
    }
    #[test]
    fn layout_honors_left_top_and_narrow_text_fallback() {
        let mut settings = Settings::default();
        settings.image.width_cells = 12;
        settings.image.max_height_rows = 6;
        let left = live_plan(Rect::new(0, 0, 43, 12), &settings, body(false), false, true);
        assert_eq!(left.image, Some(Rect::new(2, 2, 6, 4)));
        assert_eq!(left.lines[0].fragments[0].text, "        ");
        assert_eq!(left.lines[0].fragments[1].text, "│");
        let mut too_short_for_padding = body(false);
        too_short_for_padding.identity.truncate(2);
        assert_eq!(
            live_plan(
                Rect::new(0, 0, 43, 12),
                &settings,
                too_short_for_padding,
                false,
                true
            )
            .image,
            None
        );
        settings.image.position = crate::config::ImagePosition::Top;
        let top = live_plan(Rect::new(0, 0, 40, 20), &settings, body(false), false, true);
        assert_eq!(top.image, Some(Rect::new(14, 1, 12, 6)));
        assert_eq!(
            live_plan(Rect::new(0, 0, 20, 2), &settings, body(false), false, true).image,
            None
        );
    }
    #[test]
    fn image_thresholds_and_width_caps_recompute_for_every_resize() {
        let mut settings = Settings::default();
        settings.image.width_cells = 12;
        settings.image.max_height_rows = 1;
        for (width, visible) in [(42, false), (43, true), (44, true)] {
            let plan = live_plan(
                Rect::new(0, 0, width, 20),
                &settings,
                body(false),
                false,
                true,
            );
            assert_eq!(plan.image.is_some(), visible, "left {width}");
        }
        settings.image.position = crate::config::ImagePosition::Top;
        for (width, visible) in [(33, false), (34, true), (35, true)] {
            let plan = live_plan(
                Rect::new(0, 0, width, 20),
                &settings,
                body(false),
                false,
                true,
            );
            assert_eq!(plan.image.is_some(), visible, "top {width}");
        }
        for (width, expected) in [(160, 104), (40, 40), (160, 104)] {
            assert_eq!(
                live_plan(
                    Rect::new(0, 0, width, 20),
                    &settings,
                    body(false),
                    false,
                    false
                )
                .card
                .width,
                expected
            );
        }
        assert_eq!(
            live_plan(Rect::new(0, 0, 160, 30), &settings, body(true), true, false)
                .card
                .width,
            120
        );
    }

    #[test]
    fn left_identity_inset_stops_before_full_width_sections() {
        let mut settings = Settings::default();
        settings.image.width_cells = 8;
        settings.image.max_height_rows = 3;
        let plan = live_plan(Rect::new(0, 0, 104, 30), &settings, body(true), true, true);
        assert_eq!(plan.lines[0].fragments[1].text, "│");
        let section = &plan.lines[6];
        assert!(section.text.contains("[ system & health ]"));
        assert_eq!(
            section.role,
            Role::System(crate::render::theme::SystemRole::Health)
        );
        assert_ne!(
            section.fragments.first().map(|span| span.text.as_str()),
            Some("│")
        );
    }

    #[test]
    fn top_image_drops_before_semantic_fallback_and_height_never_clips() {
        let mut settings = Settings::default();
        settings.image.position = crate::config::ImagePosition::Top;
        settings.image.width_cells = 8;
        settings.image.max_height_rows = 3;
        // Three section bodies each include heading, detail, and trailing spacer.
        let required = u16::try_from(body(true).identity.len() + 9).unwrap();
        let dropped = live_plan(
            Rect::new(0, 0, 80, required + 2),
            &settings,
            body(true),
            true,
            true,
        );
        assert_eq!(dropped.image, None);
        assert_eq!(dropped.lines.len(), usize::from(required));
        for height in [required + 1, required + 2, required + 3] {
            let plan = live_plan(
                Rect::new(0, 0, 80, height),
                &settings,
                body(true),
                true,
                true,
            );
            assert!(plan.lines.len() <= usize::from(plan.card.height.saturating_sub(2)));
            if height < required + 2 {
                assert!(
                    plan.lines
                        .last()
                        .is_some_and(|line| line.text.contains("resize for details")
                            && line.text.contains("full details hidden"))
                );
            }
        }
    }

    #[test]
    fn test_backend_has_one_capped_block_and_semantic_line_order() {
        let plan = live_plan(
            Rect::new(0, 0, 160, 30),
            &Settings::default(),
            body(true),
            true,
            false,
        );
        let mut terminal = Terminal::new(ratatui::backend::TestBackend::new(160, 30)).unwrap();
        terminal
            .draw(|frame| {
                let block = Block::default().borders(Borders::ALL).title(" hoshino ");
                let inner = block.inner(plan.card);
                frame.render_widget(block, plan.card);
                let lines = plan
                    .lines
                    .iter()
                    .map(|line| {
                        Line::from(
                            expanded_card_spans(line, palette(Theme::DuskDarker))
                                .into_iter()
                                .map(|chunk| Span::styled(chunk.text, Style::default()))
                                .collect::<Vec<_>>(),
                        )
                    })
                    .collect::<Vec<_>>();
                frame.render_widget(Paragraph::new(Text::from(lines)), inner);
            })
            .unwrap();
        let buffer = terminal.backend().buffer();
        assert_eq!(buffer[(0, 0)].symbol(), "┌");
        assert_eq!(buffer[(119, 0)].symbol(), "┐");
        assert_eq!(buffer[(120, 0)].symbol(), " ");
        let order = plan
            .lines
            .iter()
            .map(|line| line.text.as_str())
            .collect::<Vec<_>>()
            .join("\n");
        assert!(
            order.find("[ system & health ]").unwrap() < order.find("[ active context ]").unwrap()
        );
        assert!(
            order.find("[ active context ]").unwrap()
                < order.find("[ project telemetry ]").unwrap()
        );
    }

    #[test]
    fn semantic_reservation_is_painted_before_left_and_top_image_markers() {
        for position in [
            crate::config::ImagePosition::Left,
            crate::config::ImagePosition::Top,
        ] {
            let mut settings = Settings::default();
            settings.image.position = position;
            settings.image.width_cells = 8;
            settings.image.max_height_rows = 3;
            let plan = live_plan(Rect::new(0, 0, 80, 20), &settings, body(false), false, true);
            let image_area = plan.image.expect("fixture geometry displays image");
            let mut terminal = Terminal::new(ratatui::backend::TestBackend::new(80, 20)).unwrap();
            let mut rendered = false;
            terminal
                .draw(|frame| {
                    rendered = draw_live_plan(
                        frame,
                        &plan,
                        Block::default().borders(Borders::ALL),
                        palette(Theme::DuskDarker),
                        |frame, area| {
                            frame.render_widget(Paragraph::new("M"), area);
                            true
                        },
                    )
                })
                .unwrap();
            assert!(rendered);
            assert_eq!(
                terminal.backend().buffer()[(image_area.x, image_area.y)].symbol(),
                "M"
            );
        }
    }

    #[test]
    fn dropped_image_plan_never_reports_a_protocol_placeholder() {
        let mut settings = Settings::default();
        settings.image.width_cells = 8;
        let narrow = live_plan(Rect::new(0, 0, 42, 20), &settings, body(false), false, true);
        settings.image.position = crate::config::ImagePosition::Top;
        settings.image.max_height_rows = 3;
        let short = live_plan(Rect::new(0, 0, 80, 13), &settings, body(true), true, true);
        for plan in [narrow, short] {
            let mut calls = 0;
            let mut terminal = Terminal::new(ratatui::backend::TestBackend::new(80, 20)).unwrap();
            let mut rendered = false;
            terminal
                .draw(|frame| {
                    rendered = draw_live_plan(
                        frame,
                        &plan,
                        Block::default().borders(Borders::ALL),
                        palette(Theme::DuskDarker),
                        |_, _| {
                            calls += 1;
                            true
                        },
                    )
                })
                .unwrap();
            assert!(!rendered);
            assert_eq!(calls, 0);
        }
    }
    #[test]
    fn halfblocks_constructs_without_pixel_geometry() {
        let mut no_geometry = caps();
        no_geometry.cell_pixels = None;
        let image = DecodedImage {
            format: image::ImageFormat::Png,
            first_frame: image::RgbaImage::new(1, 1),
            frames: vec![],
        };
        let mut live = LiveImage::new(image, no_geometry, SelectedProtocol::Halfblocks, 1).unwrap();
        let mut terminal = Terminal::new(ratatui::backend::TestBackend::new(20, 6)).unwrap();
        terminal
            .draw(|frame| assert!(live.render(frame, frame.area())))
            .unwrap();
    }
    #[test]
    fn refreshes_on_the_provider_interval_and_restores_refresh_errors() {
        struct TimedEvents {
            events: Vec<LiveEvent>,
        }
        impl EventSource for TimedEvents {
            fn next(&mut self, _: Duration) -> Result<Option<LiveEvent>, LiveError> {
                thread::sleep(SPINNER_INTERVAL);
                Ok(Some(self.events.remove(0)))
            }
        }
        struct RefreshProvider {
            fail: bool,
            calls: std::sync::Arc<std::sync::atomic::AtomicUsize>,
        }
        impl SnapshotProvider for RefreshProvider {
            fn refresh_interval(&self) -> Duration {
                SPINNER_INTERVAL
            }
            fn refresh(&mut self) -> Result<Snapshot, LiveError> {
                self.calls
                    .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                if self.fail {
                    Err(LiveError::Refresh("injected".into()))
                } else {
                    Ok(snapshot())
                }
            }
        }
        let calls = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let mut events = TimedEvents {
            events: vec![LiveEvent::Tick, LiveEvent::Quit],
        };
        let mut renderer = Renderer::default();
        let mut ops = Ops::default();
        assert!(
            run_live_with(
                snapshot(),
                RefreshProvider {
                    fail: false,
                    calls: calls.clone()
                },
                Settings::default(),
                LiveOptions::default(),
                false,
                caps(),
                session(SelectedProtocol::Text),
                &mut events,
                &mut renderer,
                &mut ops,
                || Ok(())
            )
            .is_ok()
        );
        assert_eq!(calls.load(std::sync::atomic::Ordering::Relaxed), 1);

        let mut events = TimedEvents {
            events: vec![LiveEvent::Tick],
        };
        let mut renderer = Renderer::default();
        let mut ops = Ops::default();
        let result = run_live_with(
            snapshot(),
            RefreshProvider { fail: true, calls },
            Settings::default(),
            LiveOptions::default(),
            false,
            caps(),
            session(SelectedProtocol::Text),
            &mut events,
            &mut renderer,
            &mut ops,
            || Ok(()),
        );
        assert!(matches!(result, Err(LiveError::Refresh(_))));
        assert_eq!(ops.calls, ["raw+", "alt+", "hide", "show", "alt-", "raw-"]);
    }
    #[test]
    fn errors_restore_and_delete_only_rendered_kitty() {
        let (result, ops, _, deletes) = run(
            vec![Err(LiveError::Event("injected".into()))],
            Renderer {
                image: true,
                ..Renderer::default()
            },
            SelectedProtocol::Kitty,
            true,
        );
        assert!(matches!(result, Err(LiveError::Event(_))));
        assert_eq!(ops.calls, ["raw+", "alt+", "hide", "show", "alt-", "raw-"]);
        assert_eq!(deletes, 1);
        let (_, _, _, deletes) = run(
            vec![Ok(Some(LiveEvent::Quit))],
            Renderer {
                image: true,
                ..Renderer::default()
            },
            SelectedProtocol::Iterm2,
            true,
        );
        assert_eq!(deletes, 0);
        let (_, _, _, deletes) = run(
            vec![Ok(Some(LiveEvent::Quit))],
            Renderer::default(),
            SelectedProtocol::Kitty,
            true,
        );
        assert_eq!(deletes, 0, "a dropped image never owns Kitty cleanup");
    }
    #[test]
    fn cleanup_failure_does_not_replace_a_live_error() {
        let mut events = Events {
            events: vec![Err(LiveError::Event("primary".into()))],
            preflight: Ok(()),
        };
        let mut renderer = Renderer {
            image: true,
            ..Renderer::default()
        };
        let mut ops = Ops::default();
        let result = run_live_with(
            snapshot(),
            Provider {
                calls: 0,
                interval: Duration::ZERO,
                fail: false,
            },
            Settings::default(),
            LiveOptions::default(),
            true,
            caps(),
            session(SelectedProtocol::Kitty),
            &mut events,
            &mut renderer,
            &mut ops,
            || Err(io::Error::other("cleanup")),
        );
        assert!(matches!(result, Err(LiveError::Event(message)) if message == "primary"));
        assert_eq!(ops.calls, ["raw+", "alt+", "hide", "show", "alt-", "raw-"]);
    }
    #[test]
    fn failed_prevalidation_never_owns_terminal() {
        let mut events = Events {
            events: vec![],
            preflight: Err(LiveError::Event("bad".into())),
        };
        let mut renderer = Renderer::default();
        let mut ops = Ops::default();
        let result = run_live_with(
            snapshot(),
            Provider {
                calls: 0,
                interval: Duration::ZERO,
                fail: false,
            },
            Settings {
                image: ImageSettings {
                    width_cells: 0,
                    ..ImageSettings::default()
                },
                ..Settings::default()
            },
            LiveOptions::default(),
            false,
            caps(),
            session(SelectedProtocol::Text),
            &mut events,
            &mut renderer,
            &mut ops,
            || Ok(()),
        );
        assert!(matches!(result, Err(LiveError::InvalidInput(_))));
        assert!(ops.calls.is_empty());
    }
    #[test]
    fn auto_uses_trusted_geometry_or_text_without_query() {
        assert_eq!(
            resolve_live_protocol(&session(SelectedProtocol::BoundedQueryRequired), caps()),
            SelectedProtocol::Kitty
        );
        let mut no_geometry = caps();
        no_geometry.cell_pixels = None;
        assert_eq!(
            resolve_live_protocol(
                &session(SelectedProtocol::BoundedQueryRequired),
                no_geometry
            ),
            SelectedProtocol::Text
        );
    }
    #[test]
    fn kitty_animation_advances_clamped_decoded_frames_with_the_same_session_id() {
        let frames = vec![
            crate::render::image::DecodedFrame {
                image: image::RgbaImage::new(1, 1),
                delay_ms: 20,
            },
            crate::render::image::DecodedFrame {
                image: image::RgbaImage::new(1, 1),
                delay_ms: 20,
            },
        ];
        let image = DecodedImage {
            format: image::ImageFormat::Gif,
            first_frame: frames[0].image.clone(),
            frames,
        };
        let id = session(SelectedProtocol::Kitty).id.get();
        let mut live = LiveImage::new(image, caps(), SelectedProtocol::Kitty, id).unwrap();
        live.next_frame = Instant::now();
        live.advance_kitty_frame();
        assert_eq!(live.frame_index, 1);
        assert_eq!(live.kitty_id, Some((id, false)));
    }
}
