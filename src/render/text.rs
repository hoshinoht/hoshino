use crate::{
    config::Theme,
    model::{
        Coverage, CoverageFormat, DiagnosticSeverity, Disk, Freshness, Git, GitHead, Seconds,
        Snapshot,
    },
};

use super::theme::{
    ExpandedChunk, Palette, Role, SpanStyle, SystemRole, expand_span, palette, runtime_role,
};

/// Pure rendering options. Embedded rows deliberately exclude the enclosing
/// frame because the caller already owns one (for example Ratatui's Block).
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct TextOptions {
    pub full: bool,
    pub width: Option<usize>,
    pub color: bool,
    pub theme: Theme,
    pub embedded: bool,
}
impl Default for TextOptions {
    fn default() -> Self {
        Self {
            full: false,
            width: None,
            color: false,
            theme: Theme::DuskDarker,
            embedded: false,
        }
    }
}
pub type RenderOptions = TextOptions;

/// A serializable semantic fragment. Its role and style are authoritative;
/// serializers must not infer either from its label or rendered characters.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CardSpan {
    pub text: String,
    pub role: Role,
    pub style: SpanStyle,
}

/// One semantic row. `text` is retained as a plain-text convenience for current
/// callers; it is derived from `fragments` and `trailing` and is never reparsed
/// for styling. Width-dependent parts stay symbolic until [`layout_line`].
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CardLine {
    pub text: String,
    pub role: Role,
    pub fragments: Vec<CardSpan>,
    /// Right-aligned support, dropped before anything else when space is short.
    pub trailing: Vec<CardSpan>,
    /// One width-dependent element inserted before `fragments[at]`.
    pub flex: Option<FlexSlot>,
}
impl CardLine {
    pub fn new(role: Role, fragments: Vec<CardSpan>) -> Self {
        row(role, fragments)
    }
}

/// Width-dependent instruments. Rows that share fixed widths lay out their
/// flexible element at the same width, which keeps table and ruler columns aligned.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Flex {
    /// Filled cells fade into `role`; the remainder is a secondary track.
    Meter { permille: u16, role: Role },
    /// A 24-hour ruler: elapsed cells and the current-time marker use Progress.
    Ruler { permille: u16 },
    /// Hour labels for a `Ruler` laid out at the same width.
    RulerAxis,
    /// A composition bar with one solid run per part, in order.
    Stack(Vec<(u64, Role)>),
    /// A secondary column caption padded to the flexible width.
    Caption(&'static str),
}
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct FlexSlot {
    pub at: usize,
    pub kind: Flex,
    pub min: usize,
    pub max: usize,
    /// Stacked form used when the element cannot fit at its minimum width; an
    /// empty form omits the row (for example a table header at narrow widths).
    pub compact: Option<Vec<CardSpan>>,
}

/// Shared, embedded compositor input. The later one-shot compositor reserves
/// image geometry only around `identity`; `sections` always span the full body.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CardBody {
    pub title: Vec<CardSpan>,
    pub identity: Vec<CardLine>,
    /// Reserved compact-header rows. Image compositors may use this height, but
    /// semantic sections always begin after the actual identity rows.
    pub identity_layout_height: usize,
    pub sections: Vec<CardSection>,
}
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CardSection {
    pub name: &'static str,
    pub role: Role,
    pub left: Vec<CardLine>,
    pub right: Vec<CardLine>,
}

const TRAILING_GAP: usize = 2;
const NORMAL_GUTTER: usize = 5;
const FULL_GUTTER: usize = 6;
const NORMAL_METER: (usize, usize) = (8, 30);
const FULL_METER: (usize, usize) = (8, 44);
const NORMAL_RULER: (usize, usize) = (17, 49);
const FULL_RULER: (usize, usize) = (17, 97);
const STACK: (usize, usize) = (10, 97);
/// Full's disk table: mount, meter, percent, used / size, and a state column.
const TABLE_NAME: usize = 22;
const TABLE_PERCENT: usize = 6;
const TABLE_DETAIL: usize = 23;
const TABLE_STATE: usize = 8;
/// A disk at or above this share of its capacity is marked `▲ full`.
const DISK_FULL_PERCENT: f64 = 95.0;

fn span(text: impl Into<String>, role: Role) -> CardSpan {
    CardSpan {
        text: sanitize(&text.into()),
        role,
        style: SpanStyle::Solid,
    }
}
fn gradient(text: impl Into<String>, role: Role) -> CardSpan {
    CardSpan {
        text: text.into(),
        role,
        style: SpanStyle::Gradient,
    }
}
fn row(role: Role, fragments: Vec<CardSpan>) -> CardLine {
    let mut line = CardLine {
        text: String::new(),
        role,
        fragments,
        trailing: Vec::new(),
        flex: None,
    };
    line.text = plain(&line);
    line
}
fn plain(line: &CardLine) -> String {
    let mut text = line
        .fragments
        .iter()
        .map(|fragment| fragment.text.as_str())
        .collect::<String>();
    if !line.trailing.is_empty() {
        text.push_str(&" ".repeat(TRAILING_GAP));
        text.extend(line.trailing.iter().map(|fragment| fragment.text.as_str()));
    }
    text
}
fn with_trailing(mut line: CardLine, trailing: Vec<CardSpan>) -> CardLine {
    line.trailing = trailing;
    line.text = plain(&line);
    line
}
fn with_flex(mut line: CardLine, at: usize, kind: Flex, (min, max): (usize, usize)) -> CardLine {
    line.flex = Some(FlexSlot {
        at,
        kind,
        min,
        max,
        compact: None,
    });
    line
}
fn with_compact(mut line: CardLine, compact: Vec<CardSpan>) -> CardLine {
    if let Some(slot) = line.flex.as_mut() {
        slot.compact = Some(compact);
    }
    line
}
fn labelled(label: &str, value: impl Into<String>, value_role: Role) -> CardLine {
    row(
        value_role,
        vec![
            span(format!("{label}: "), Role::Secondary),
            span(value, value_role),
        ],
    )
}
fn gutter(label: &str, width: usize) -> CardSpan {
    span(format!("{label:<width$}"), Role::Secondary)
}
fn permille(percent: f64) -> u16 {
    (percent.clamp(0.0, 100.0) * 10.0).round() as u16
}

/// `label ━━━━╌╌╌  80.8%  value` — the whole value takes the metric role.
fn meter_row(
    label: &str,
    percent: f64,
    role: Role,
    value: &str,
    state: Option<CardSpan>,
) -> CardLine {
    let mut fragments = vec![
        gutter(label, NORMAL_GUTTER),
        span("  ", Role::Text),
        span(format!("{}  {value}", format_percent(percent)), role),
    ];
    fragments.extend(state);
    with_flex(
        row(role, fragments),
        1,
        Flex::Meter {
            permille: permille(percent),
            role,
        },
        NORMAL_METER,
    )
}
/// One row of Full's metric table. Every row has the same fixed widths so the
/// flexible meter (or the header caption) lines up across the band.
fn table_row(
    label: &str,
    name: &str,
    percent: f64,
    role: Role,
    detail: &str,
    full: bool,
) -> CardLine {
    let value = format!(
        "{:>TABLE_PERCENT$}  {:<TABLE_DETAIL$}",
        format_percent(percent),
        detail
    );
    let state = if full {
        span(format!("{:<TABLE_STATE$}", "  ▲ full"), Role::Warning)
    } else {
        span(" ".repeat(TABLE_STATE), Role::Text)
    };
    // Stacked form: the state word sits before capacity so it survives fitting.
    let mut compact = vec![gutter(label, FULL_GUTTER)];
    if !name.is_empty() {
        compact.push(span(
            format!("{} ", fit_middle(name, TABLE_NAME - 1)),
            Role::Text,
        ));
    }
    compact.push(span(format_percent(percent), role));
    if full {
        compact.push(span(" ▲ full", Role::Warning));
    }
    if !detail.is_empty() {
        compact.push(span(format!(" · {detail}"), role));
    }
    let line = with_flex(
        row(
            role,
            vec![
                gutter(label, FULL_GUTTER),
                span(
                    format!("{:<TABLE_NAME$}", fit_middle(name, TABLE_NAME - 1)),
                    Role::Text,
                ),
                span("  ", Role::Text),
                span(value, role),
                state,
            ],
        ),
        2,
        Flex::Meter {
            permille: permille(percent),
            role,
        },
        FULL_METER,
    );
    with_compact(line, compact)
}
fn table_header(name: &str, detail: &str) -> CardLine {
    let line = with_flex(
        row(
            Role::Secondary,
            vec![
                gutter("", FULL_GUTTER),
                span(format!("{name:<TABLE_NAME$}"), Role::Secondary),
                span(
                    format!(
                        "  {:>TABLE_PERCENT$}  {detail:<TABLE_DETAIL$}{}",
                        "%",
                        " ".repeat(TABLE_STATE)
                    ),
                    Role::Secondary,
                ),
            ],
        ),
        2,
        Flex::Caption("usage"),
        FULL_METER,
    );
    with_compact(line, Vec::new())
}
/// A 24-hour ruler row and the hour axis beneath it. Both rows carry identical
/// fixed widths, so the axis labels sit under the ruler's quarter ticks.
fn ruler_rows(
    lead: Vec<CardSpan>,
    percent: f64,
    tail: Vec<CardSpan>,
    bounds: (usize, usize),
) -> [CardLine; 2] {
    let lead_width = lead.iter().map(|span| visible_width(&span.text)).sum();
    let tail_width = tail.iter().map(|span| visible_width(&span.text)).sum();
    let at = lead.len();
    let mut fragments = lead;
    fragments.extend(tail);
    [
        with_flex(
            row(Role::Time, fragments),
            at,
            Flex::Ruler {
                permille: permille(percent),
            },
            bounds,
        ),
        with_flex(
            row(
                Role::Secondary,
                vec![
                    span(" ".repeat(lead_width), Role::Text),
                    span(" ".repeat(tail_width), Role::Text),
                ],
            ),
            1,
            Flex::RulerAxis,
            bounds,
        ),
    ]
}

/// Build semantic rows directly from Snapshot. This is the frozen interface for
/// the one-shot compositor and TUI owners: title + identity + ordered sections.
pub fn card_body(snapshot: &Snapshot, full: bool) -> CardBody {
    let identity = if full {
        full_identity(snapshot)
    } else {
        normal_identity(snapshot)
    };
    let mut sections = Vec::new();
    if full {
        if let Some(left) = system_details(snapshot) {
            sections.push(CardSection {
                name: "system & health",
                role: Role::System(SystemRole::Health),
                left,
                right: Vec::new(),
            });
        }
        if let Some(left) = active_context(snapshot) {
            sections.push(CardSection {
                name: "active context",
                role: Role::Context,
                left,
                right: Vec::new(),
            });
        }
        if let Some(left) = project_telemetry(snapshot) {
            sections.push(CardSection {
                name: "project telemetry",
                role: Role::Primary,
                left,
                right: Vec::new(),
            });
        }
    }
    CardBody {
        title: vec![span("hoshino", Role::Primary)],
        identity_layout_height: if full { 6 } else { identity.len() },
        identity,
        sections,
    }
}

/// Normal: a compact 6–9-row instrument rail.
fn normal_identity(snapshot: &Snapshot) -> Vec<CardLine> {
    let mut identity = Vec::new();
    let mut place = Vec::new();
    if let Some(path) = workspace(snapshot) {
        place.push(span(shortest_path(path), Role::Primary));
    }
    if let Some(git) = snapshot.git.as_ref() {
        if !place.is_empty() {
            place.push(span(" on ", Role::Secondary));
        }
        place.push(span(git_value(git), Role::Git));
    }
    let runtime = runtime_value(snapshot, false).map(|(value, role)| vec![span(value, role)]);
    match (place.is_empty(), runtime) {
        (false, runtime) => identity.push(with_trailing(
            row(Role::Primary, place),
            runtime.unwrap_or_default(),
        )),
        (true, Some(runtime)) => identity.push(row(Role::Primary, runtime)),
        (true, None) => {}
    }

    let mut machine = Vec::new();
    if let Some(os) = snapshot.system.os.as_ref() {
        machine.push(span(format_os(os), Role::Primary));
    }
    if let Some(cpu) = snapshot.system.cpu.as_ref().and_then(cpu_value) {
        if !machine.is_empty() {
            machine.push(span(" · ", Role::Secondary));
        }
        machine.push(span(cpu, Role::System(SystemRole::Cpu)));
    }
    let uptime = snapshot.system.uptime.map(|uptime| {
        vec![
            span("up ", Role::Secondary),
            span(
                format_uptime(uptime, false),
                Role::System(SystemRole::Health),
            ),
        ]
    });
    match (machine.is_empty(), uptime) {
        (false, uptime) => identity.push(with_trailing(
            row(Role::System(SystemRole::Health), machine),
            uptime.unwrap_or_default(),
        )),
        (true, Some(uptime)) => identity.push(row(Role::System(SystemRole::Health), uptime)),
        (true, None) => {}
    }

    if let Some(memory) = snapshot.system.memory.as_ref() {
        let role = Role::System(SystemRole::Memory);
        let value = format!(
            "{}/{}",
            format_bytes(memory.used_bytes),
            format_bytes(memory.total_bytes)
        );
        identity.push(match percent_of(memory.used_bytes, memory.total_bytes) {
            Some(percent) => meter_row("mem", percent, role, &value, None),
            None => row(role, vec![gutter("mem", NORMAL_GUTTER), span(value, role)]),
        });
    }
    if let Some(disk) = displayed_disks(snapshot).first() {
        let role = Role::System(SystemRole::Disk);
        let mut value = format!(
            "{}/{}",
            format_bytes(disk.used_bytes),
            format_bytes(disk.total_bytes)
        );
        if disk.mount != "/" {
            value.push_str(&format!(" · {}", disk.mount));
        }
        identity.push(match percent_of(disk.used_bytes, disk.total_bytes) {
            Some(percent) => meter_row(
                "disk",
                percent,
                role,
                &value,
                (percent >= DISK_FULL_PERCENT).then(|| span("  ▲ full", Role::Warning)),
            ),
            None => row(role, vec![gutter("disk", NORMAL_GUTTER), span(value, role)]),
        });
    }

    let time = &snapshot.time;
    match time.day_progress_percent.filter(|value| value.is_finite()) {
        Some(day) => {
            let lead = vec![match &time.local {
                Some(local) => span(format!("{local} "), Role::Time),
                None => gutter("day", NORMAL_GUTTER),
            }];
            let mut tail = format_percent(day);
            if let Some(date) = &time.date {
                tail.push_str(&format!(" · {date}"));
            }
            identity.extend(ruler_rows(
                lead,
                day,
                vec![span("  ", Role::Text), span(tail, Role::Time)],
                NORMAL_RULER,
            ));
        }
        None => {
            if let Some(clock) = time_value(snapshot) {
                identity.push(row(
                    Role::Time,
                    vec![gutter("time", NORMAL_GUTTER), span(clock, Role::Time)],
                ));
            }
        }
    }

    if let Some(project) = snapshot.project.as_ref()
        && let Some(summary) = project_summary(
            project.coverage.as_ref(),
            project.loc.as_ref().map(|loc| (loc.total, loc.truncated)),
        )
    {
        identity.push(summary);
    }
    if let Some(diagnostic) = snapshot
        .diagnostics
        .iter()
        .max_by_key(|diagnostic| match diagnostic.severity {
            DiagnosticSeverity::Error => 2,
            DiagnosticSeverity::Warning => 1,
            DiagnosticSeverity::Info => 0,
        })
    {
        identity.push(labelled(
            "DIAG",
            format!(
                "{} · {}",
                diagnostic.message,
                severity_name(diagnostic.severity)
            ),
            diagnostic_role(diagnostic.severity),
        ));
    }
    // Keep Normal identity as a compact 6–9-row rail when the facts exist.
    identity.truncate(9);
    identity
}

/// Full: a compact workspace/Git/runtime header above the section bands.
fn full_identity(snapshot: &Snapshot) -> Vec<CardLine> {
    let mut identity = Vec::new();
    let runtime = runtime_value(snapshot, true).map(|(value, role)| vec![span(value, role)]);
    match (snapshot.git.as_ref(), runtime) {
        (Some(git), runtime) => {
            let mut fragments = vec![gutter("git", FULL_GUTTER), span(git_value(git), Role::Git)];
            if let Some(upstream) = &git.upstream {
                fragments.push(span(" → ", Role::Secondary));
                fragments.push(span(upstream, Role::Git));
            }
            identity.push(with_trailing(
                row(Role::Git, fragments),
                runtime.unwrap_or_default(),
            ));
        }
        (None, Some(runtime)) => identity.push(row(Role::Primary, runtime)),
        (None, None) => {}
    }
    let mut place = workspace(snapshot).map(shortest_path);
    if let Some(os) = snapshot.system.os.as_ref() {
        place = Some(match place {
            Some(path) => format!("{path} · {}", format_os(os)),
            None => format_os(os),
        });
    }
    if let Some(place) = place {
        identity.push(row(
            Role::Primary,
            vec![gutter("dir", FULL_GUTTER), span(place, Role::Primary)],
        ));
    }
    identity
}

fn workspace(snapshot: &Snapshot) -> Option<&str> {
    snapshot
        .context
        .worktree
        .as_deref()
        .or(snapshot.context.directory.as_deref())
}
/// `master · dirty · ~1` — head, textual state, then nonzero counts.
fn git_value(git: &Git) -> String {
    let state = if git.conflicts > 0 {
        "conflict"
    } else if git.dirty || git.staged > 0 || git.unstaged > 0 {
        "dirty"
    } else {
        "clean"
    };
    let mut counts = Vec::new();
    if git.staged > 0 {
        counts.push(format!("+{}", git.staged));
    }
    if git.unstaged > 0 {
        counts.push(format!("~{}", git.unstaged));
    }
    if git.conflicts > 0 {
        counts.push(format!("!{}", git.conflicts));
    }
    if let Some(ahead) = git.ahead.filter(|count| *count > 0) {
        counts.push(format!("↑{ahead}"));
    }
    let suffix = if counts.is_empty() {
        String::new()
    } else {
        format!(" · {}", counts.join(" "))
    };
    format!("{} · {state}{suffix}", git_head(git))
}
/// The primary language's runtime. Normal keeps only the version token; Full
/// keeps the complete toolchain version with its build provenance.
fn runtime_value(snapshot: &Snapshot, full: bool) -> Option<(String, Role)> {
    let project = snapshot.project.as_ref()?;
    let language = project.primary_language.as_deref()?;
    let toolchain = project
        .toolchains
        .iter()
        .find(|tool| tool.language.eq_ignore_ascii_case(language));
    let role = toolchain
        .map(|tool| Role::Runtime(runtime_role(&tool.language, &tool.runtime)))
        .unwrap_or_else(|| Role::Runtime(runtime_role(language, language)));
    let value = toolchain.map_or_else(
        || language.to_owned(),
        |tool| {
            let version = toolchain_version(tool);
            let version = if full {
                version
            } else {
                version.split_whitespace().next().unwrap_or(version)
            };
            format!("{} {version}", tool.runtime)
        },
    );
    Some((value, role))
}
/// `Apple M4 Pro @ 4512 MHz · 12.5%` from whichever CPU facts exist.
fn cpu_value(cpu: &crate::model::Cpu) -> Option<String> {
    let mut value = match (&cpu.label, cpu.frequency_mhz) {
        (Some(label), Some(frequency)) => format!("{label} @ {frequency} MHz"),
        (Some(label), None) => label.clone(),
        (None, Some(frequency)) => format!("{frequency} MHz"),
        (None, None) => String::new(),
    };
    if let Some(percent) = cpu.utilization_percent.filter(|value| value.is_finite()) {
        if !value.is_empty() {
            value.push_str(" · ");
        }
        value.push_str(&format_percent(percent));
    }
    (!value.is_empty()).then_some(value)
}
/// Disks in collection order, skipping volumes whose capacity matches one
/// already shown and whose usage would print identically (APFS system and data
/// volumes share one container, so their byte counts differ only by timing).
fn displayed_disks(snapshot: &Snapshot) -> Vec<&Disk> {
    let mut shown: Vec<&Disk> = Vec::new();
    for disk in &snapshot.system.disks {
        if !shown.iter().any(|seen| {
            seen.total_bytes == disk.total_bytes
                && format_bytes(seen.used_bytes) == format_bytes(disk.used_bytes)
        }) {
            shown.push(disk);
        }
    }
    shown
}
fn time_value(snapshot: &Snapshot) -> Option<String> {
    let time = &snapshot.time;
    let mut parts = Vec::new();
    if let Some(value) = &time.local {
        parts.push(value.clone());
    }
    if let Some(value) = &time.date {
        parts.push(value.clone());
    }
    if let Some(value) = &time.timezone_name {
        parts.push(value.clone());
    } else if let Some(value) = time.timezone_offset_seconds {
        parts.push(format_offset(value));
    }
    (!parts.is_empty()).then(|| parts.join(" · "))
}
fn project_summary(coverage: Option<&Coverage>, loc: Option<(u64, bool)>) -> Option<CardLine> {
    let mut parts = Vec::new();
    if let Some((total, partial)) = loc {
        parts.push(format!(
            "{} LOC{}",
            format_number(total),
            if partial { " · partial" } else { "" }
        ));
    }
    if let Some(coverage) = coverage
        && let Some(percent) = coverage.line_percent.filter(|value| value.is_finite())
    {
        parts.push(format!(
            "coverage {}{}",
            format_percent(percent),
            if coverage.stale == Freshness::Stale {
                " · stale"
            } else if coverage.stale == Freshness::Unknown {
                " · freshness unknown"
            } else {
                ""
            }
        ));
    }
    (!parts.is_empty()).then(|| {
        labelled(
            "project",
            parts.join(" · "),
            coverage.map(coverage_role).unwrap_or(Role::Secondary),
        )
    })
}
fn system_details(snapshot: &Snapshot) -> Option<Vec<CardLine>> {
    let mut rows = Vec::new();
    let cpu = snapshot
        .system
        .cpu
        .as_ref()
        .and_then(cpu_value)
        .map(|value| {
            vec![
                gutter("cpu", FULL_GUTTER),
                span(value, Role::System(SystemRole::Cpu)),
            ]
        });
    let uptime = snapshot.system.uptime.map(|uptime| {
        vec![
            span("uptime ", Role::Secondary),
            span(
                format_uptime(uptime, false),
                Role::System(SystemRole::Health),
            ),
        ]
    });
    match (cpu, uptime) {
        (Some(cpu), uptime) => rows.push(with_trailing(
            row(Role::System(SystemRole::Cpu), cpu),
            uptime.unwrap_or_default(),
        )),
        (None, Some(uptime)) => rows.push(row(Role::System(SystemRole::Health), uptime)),
        (None, None) => {}
    }
    if let Some(memory) = snapshot.system.memory.as_ref() {
        let role = Role::System(SystemRole::Memory);
        let detail = format!(
            "{} / {}",
            format_bytes(memory.used_bytes),
            format_bytes(memory.total_bytes)
        );
        rows.push(match percent_of(memory.used_bytes, memory.total_bytes) {
            Some(percent) => table_row("mem", "", percent, role, &detail, false),
            None => row(role, vec![gutter("mem", FULL_GUTTER), span(detail, role)]),
        });
    }
    let disks = displayed_disks(snapshot);
    if !disks.is_empty() {
        rows.push(table_header("mount", "used / size"));
    }
    for (index, disk) in disks.into_iter().enumerate() {
        let role = Role::System(SystemRole::Disk);
        let label = if index == 0 { "disk" } else { "" };
        let detail = format!(
            "{} / {}",
            format_bytes(disk.used_bytes),
            format_bytes(disk.total_bytes)
        );
        rows.push(match percent_of(disk.used_bytes, disk.total_bytes) {
            Some(percent) => table_row(
                label,
                &disk.mount,
                percent,
                role,
                &detail,
                percent >= DISK_FULL_PERCENT,
            ),
            None => row(
                Role::Text,
                vec![
                    gutter(label, FULL_GUTTER),
                    span(
                        format!("{:<TABLE_NAME$}", fit_middle(&disk.mount, TABLE_NAME - 1)),
                        Role::Text,
                    ),
                    span(detail, role),
                ],
            ),
        });
    }
    (!rows.is_empty()).then_some(rows)
}
fn active_context(snapshot: &Snapshot) -> Option<Vec<CardLine>> {
    let mut rows = Vec::new();
    if let Some(clock) = time_value(snapshot) {
        rows.push(row(
            Role::Time,
            vec![gutter("time", FULL_GUTTER), span(clock, Role::Time)],
        ));
    }
    if let Some(day) = snapshot
        .time
        .day_progress_percent
        .filter(|value| value.is_finite())
    {
        rows.extend(ruler_rows(
            vec![gutter("day", FULL_GUTTER)],
            day,
            vec![
                span("  ", Role::Text),
                span(format_percent(day), Role::Time),
            ],
            FULL_RULER,
        ));
    }
    if let Some(git) = snapshot.git.as_ref() {
        let mut fragments = vec![gutter("git", FULL_GUTTER)];
        let mut counts = vec![
            ("staged", git.staged.to_string()),
            ("unstaged", git.unstaged.to_string()),
            ("conflicts", git.conflicts.to_string()),
        ];
        if let Some(ahead) = git.ahead.filter(|count| *count > 0) {
            counts.push(("ahead", ahead.to_string()));
        }
        if let GitHead::Detached { commit } = &git.head {
            counts.push(("detached at", commit.clone()));
        }
        for (index, (name, value)) in counts.into_iter().enumerate() {
            let gap = if index == 0 { "" } else { "    " };
            fragments.push(span(format!("{gap}{name} "), Role::Secondary));
            fragments.push(span(value, Role::Git));
        }
        rows.push(row(Role::Git, fragments));
    }
    for diagnostic in &snapshot.diagnostics {
        let role = diagnostic_role(diagnostic.severity);
        rows.push(row(
            role,
            vec![
                gutter("diag", FULL_GUTTER),
                span(
                    format!(
                        "{} {} · {}",
                        diagnostic.code,
                        severity_name(diagnostic.severity),
                        diagnostic.message
                    ),
                    role,
                ),
            ],
        ));
    }
    (!rows.is_empty()).then_some(rows)
}
fn project_telemetry(snapshot: &Snapshot) -> Option<Vec<CardLine>> {
    let project = snapshot.project.as_ref()?;
    let mut rows = Vec::new();
    if let Some(loc) = &project.loc {
        rows.push(row(
            Role::Primary,
            vec![
                gutter("loc", FULL_GUTTER),
                span(format_number(loc.total), Role::Primary),
                span(" lines", Role::Secondary),
                span(
                    if loc.truncated { " · partial" } else { "" },
                    Role::Secondary,
                ),
            ],
        ));
        let mut languages = loc
            .by_language
            .iter()
            .filter(|language| language.lines > 0)
            .map(|language| {
                (
                    language,
                    Role::Runtime(runtime_role(&language.language, &language.language)),
                )
            })
            .collect::<Vec<_>>();
        languages.sort_by_key(|(language, _)| std::cmp::Reverse(language.lines));
        if !languages.is_empty() {
            rows.push(with_flex(
                row(Role::Primary, vec![gutter("", FULL_GUTTER)]),
                1,
                Flex::Stack(
                    languages
                        .iter()
                        .map(|(language, role)| (language.lines, *role))
                        .collect(),
                ),
                STACK,
            ));
        }
        for (language, role) in languages {
            let share = percent_of(language.lines, loc.total)
                .map(|percent| format!(" · {}", format_percent(percent)))
                .unwrap_or_default();
            rows.push(row(
                role,
                vec![
                    gutter("", FULL_GUTTER),
                    span("■ ", role),
                    span(
                        format!("{} {}", language.language, format_number(language.lines)),
                        role,
                    ),
                    span(" lines", Role::Text),
                    span(share, Role::Secondary),
                ],
            ));
        }
    }
    if let Some(coverage) = &project.coverage {
        let role = coverage_role(coverage);
        let mut first = true;
        for (name, value) in [
            ("line", coverage.line_percent),
            ("branch", coverage.branch_percent),
            ("function", coverage.function_percent),
        ] {
            if let Some(value) = value.filter(|value| value.is_finite()) {
                let label = if first { "cov" } else { "" };
                first = false;
                rows.push(table_row(label, name, value, role, "", false));
            }
        }
        let mut provenance = Vec::new();
        if let Some(format) = coverage.format {
            provenance.push(coverage_format_name(format).to_owned());
        }
        if let Some(path) = &coverage.report_path {
            provenance.push(path.clone());
        }
        if let Some(source) = &coverage.source {
            provenance.push(format!("source {source}"));
        }
        if let Some(modified) = coverage.report_modified {
            provenance.push(format!("modified Unix {}", modified.value));
        }
        let mut fragments = vec![gutter(if first { "cov" } else { "" }, FULL_GUTTER)];
        if !provenance.is_empty() {
            fragments.push(span(provenance.join(" · "), Role::Text));
            fragments.push(span(" · ", Role::Secondary));
        }
        fragments.push(span(freshness_name(coverage.stale), role));
        rows.push(row(role, fragments));
    }
    (!rows.is_empty()).then_some(rows)
}

/// Resolve a semantic line at a concrete cell width: expand its flexible
/// element, right-align trailing support, then fit every fragment. Narrow
/// widths drop trailing support first, then the flexible element, and only
/// then shorten text, so a meter's value always outlives the meter itself.
pub fn layout_line(line: &CardLine, width: usize) -> Vec<CardSpan> {
    let trailing = spans_width(&line.trailing);
    let (cells, keep_trailing) = flex_cells(line, width);
    if cells == 0
        && let Some(compact) = line.flex.as_ref().and_then(|slot| slot.compact.as_ref())
    {
        return fit_fragments(compact, width);
    }
    let mut spans = line.fragments.clone();
    if let Some(slot) = &line.flex
        && cells > 0
    {
        let at = slot.at.min(spans.len());
        spans.splice(at..at, flex_spans(&slot.kind, cells));
    }
    let used = spans_width(&spans);
    if keep_trailing && used + TRAILING_GAP + trailing <= width {
        spans.push(span(" ".repeat(width - used - trailing), Role::Text));
        spans.extend(line.trailing.iter().cloned());
    }
    fit_fragments(&spans, width)
}
/// True when the line has no form at `width`; callers skip it instead of
/// painting an empty row.
pub fn omitted_at(line: &CardLine, width: usize) -> bool {
    line.flex
        .as_ref()
        .and_then(|slot| slot.compact.as_ref())
        .is_some_and(|compact| compact.is_empty() && flex_cells(line, width).0 == 0)
}
/// The flexible element's cell count and whether trailing support survives.
fn flex_cells(line: &CardLine, width: usize) -> (usize, bool) {
    let keep_trailing = !line.trailing.is_empty();
    let Some(slot) = &line.flex else {
        return (0, keep_trailing);
    };
    let fixed = spans_width(&line.fragments);
    let reserve = if keep_trailing {
        spans_width(&line.trailing) + TRAILING_GAP
    } else {
        0
    };
    let room = width.saturating_sub(fixed + reserve);
    if room >= slot.min {
        return (room.min(slot.max), keep_trailing);
    }
    let room = width.saturating_sub(fixed);
    (
        if room >= slot.min {
            room.min(slot.max)
        } else {
            0
        },
        false,
    )
}
fn spans_width(spans: &[CardSpan]) -> usize {
    spans.iter().map(|span| visible_width(&span.text)).sum()
}
fn flex_spans(kind: &Flex, cells: usize) -> Vec<CardSpan> {
    let mut spans = Vec::new();
    match kind {
        Flex::Meter { permille, role } => {
            let mut filled = usize::from(*permille) * cells / 1000;
            if usize::from(*permille) * cells % 1000 >= 500 {
                filled += 1;
            }
            if *permille > 0 {
                filled = filled.max(1);
            }
            let filled = filled.min(cells);
            if filled > 0 {
                spans.push(gradient("━".repeat(filled), *role));
            }
            if cells > filled {
                spans.push(span("╌".repeat(cells - filled), Role::Secondary));
            }
        }
        Flex::Ruler { permille } => {
            let span_cells = ruler_span(cells);
            let position = (usize::from(*permille) * span_cells + 500) / 1000;
            if position > 0 {
                spans.push(gradient("━".repeat(position), Role::Progress));
            }
            spans.push(span("●", Role::Progress));
            let quarter = span_cells / 4;
            let track = (position + 1..=span_cells)
                .map(|index| {
                    if index == span_cells {
                        '┤'
                    } else if index % quarter == 0 {
                        '┼'
                    } else {
                        '─'
                    }
                })
                .collect::<String>();
            spans.push(span(
                format!("{track}{}", " ".repeat(cells - span_cells - 1)),
                Role::Secondary,
            ));
        }
        Flex::RulerAxis => {
            let span_cells = ruler_span(cells);
            let mut axis = vec![' '; cells];
            for (index, label) in [
                (0, "00"),
                (span_cells / 4, "06"),
                (span_cells / 2, "12"),
                (span_cells * 3 / 4, "18"),
                (span_cells - 1, "24"),
            ] {
                for (offset, character) in label.chars().enumerate() {
                    axis[index + offset] = character;
                }
            }
            spans.push(span(axis.into_iter().collect::<String>(), Role::Secondary));
        }
        Flex::Stack(parts) => {
            let total = parts.iter().map(|(value, _)| *value).sum::<u64>().max(1);
            let mut cumulative = 0u64;
            let mut drawn = 0usize;
            for (value, role) in parts {
                cumulative += value;
                let edge = ((u128::from(cumulative) * cells as u128 + u128::from(total) / 2)
                    / u128::from(total)) as usize;
                let edge = edge.min(cells);
                if edge > drawn {
                    spans.push(span("█".repeat(edge - drawn), *role));
                    drawn = edge;
                }
            }
        }
        Flex::Caption(caption) => {
            spans.push(span(
                format!("{:<cells$}", fit_middle(caption, cells)),
                Role::Secondary,
            ));
        }
    }
    spans
}
/// The ruler's tick span: the largest multiple of four that, with its end
/// cap, fits in `cells`. Quarter ticks then land on exact hour positions.
fn ruler_span(cells: usize) -> usize {
    cells.saturating_sub(1) / 4 * 4
}

pub fn card_spans(line: &CardLine) -> Vec<CardSpan> {
    line.fragments.clone()
}
pub fn expanded_card_spans(line: &CardLine, colors: Palette) -> Vec<ExpandedChunk> {
    line.fragments
        .iter()
        .flat_map(|fragment| expand_span(&fragment.text, fragment.role, fragment.style, colors))
        .collect()
}
pub fn render_text(snapshot: &Snapshot, options: TextOptions) -> String {
    render_card(snapshot, options)
        .iter()
        .map(|line| serialize_line(line, options.color, palette(options.theme)))
        .collect::<Vec<_>>()
        .join("\n")
}
pub fn render(snapshot: &Snapshot, options: RenderOptions) -> String {
    render_text(snapshot, options)
}
fn serialize_line(line: &CardLine, color: bool, colors: Palette) -> String {
    expanded_card_spans(line, colors)
        .into_iter()
        .map(|chunk| {
            if color {
                format!(
                    "{}{}\x1b[0m",
                    colors.ansi_foreground_rgb(chunk.color),
                    chunk.text
                )
            } else {
                chunk.text
            }
        })
        .collect()
}

/// Flatten the semantic body for the current surfaces. Every returned line is
/// already laid out at its cell width; this adapter contains no label-based inference.
pub fn render_card(snapshot: &Snapshot, options: TextOptions) -> Vec<CardLine> {
    let available = options.width.unwrap_or(40).max(1);
    let cap = if options.full { 120 } else { 104 };
    let width = available.min(cap);
    let body = card_body(snapshot, options.full);
    if options.embedded || width < 20 {
        let mut rows = body.identity;
        for section in body.sections {
            rows.push(section_rail(section.name, width));
            rows.extend(section_rows(&section, width, width >= 80));
        }
        return rows
            .iter()
            .map(|line| row(line.role, layout_line(line, width)))
            .collect();
    }
    let inner = width - 2;
    let content = inner - 2;
    let title = fit_fragments(&body.title, inner.saturating_sub(2));
    let title_width = spans_width(&title);
    let mut top = vec![span("╭─", Role::Border)];
    top.extend(title);
    top.push(span(
        format!(" {}", "─".repeat(inner.saturating_sub(title_width + 2))),
        Role::Border,
    ));
    top.push(span("╮", Role::Border));
    let mut output = vec![row(Role::Border, top)];
    output.extend(body.identity.iter().map(|line| frame_line(line, content)));
    for section in body.sections {
        output.push(connected_section_rail(section.name, inner));
        output.extend(
            section_rows(&section, content, width >= 80)
                .iter()
                .map(|line| frame_line(line, content)),
        );
    }
    output.push(row(
        Role::Border,
        vec![
            span("╰", Role::Border),
            span("─".repeat(inner), Role::Border),
            span("╯", Role::Border),
        ],
    ));
    output
}
/// A structural band divider: `─ name ───`, solid pink like the frame.
pub fn section_rail(name: &'static str, width: usize) -> CardLine {
    let title = fit_middle(name, width.saturating_sub(3));
    let fill = width.saturating_sub(visible_width(&title) + 3);
    row(
        Role::Border,
        vec![
            span("─ ", Role::Border),
            span(title, Role::Primary),
            span(format!(" {}", "─".repeat(fill)), Role::Border),
        ],
    )
}

/// Layout consumes the semantic columns directly. At 80 cells paired columns
/// sit side by side; below that each column stacks without relabeling its facts.
/// Rows with no form at `width` (a table header too narrow for its columns)
/// are left out rather than painted blank.
pub fn section_rows(section: &CardSection, width: usize, wide: bool) -> Vec<CardLine> {
    let shown = |line: &&CardLine| !omitted_at(line, width);
    if !wide {
        return section
            .left
            .iter()
            .chain(&section.right)
            .filter(shown)
            .cloned()
            .collect();
    }
    let left_width = width.saturating_sub(3) / 2;
    let right_width = width.saturating_sub(left_width + 3);
    let paired = section.left.len().min(section.right.len());
    let mut rows = (0..paired)
        .map(|index| {
            let mut fragments = layout_line(&section.left[index], left_width);
            let used = spans_width(&fragments);
            fragments.push(span(" ".repeat(left_width - used + 3), Role::Text));
            fragments.extend(layout_line(&section.right[index], right_width));
            row(section.role, fragments)
        })
        .collect::<Vec<_>>();
    rows.extend(section.left[paired..].iter().filter(shown).cloned());
    rows.extend(section.right[paired..].iter().filter(shown).cloned());
    rows
}
fn connected_section_rail(name: &'static str, inner: usize) -> CardLine {
    let rail = section_rail(name, inner);
    let mut fragments = vec![span("├", Role::Border)];
    fragments.extend(rail.fragments);
    fragments.push(span("┤", Role::Border));
    row(Role::Border, fragments)
}
fn fit_fragments(fragments: &[CardSpan], width: usize) -> Vec<CardSpan> {
    let mut used = 0;
    let mut output = Vec::new();
    for fragment in fragments {
        if used >= width {
            break;
        }
        let text = fit_middle(&fragment.text, width - used);
        used += visible_width(&text);
        output.push(CardSpan {
            text,
            ..fragment.clone()
        });
    }
    output
}
/// `│ content │` with one cell of padding on each side of the content.
fn frame_line(line: &CardLine, content: usize) -> CardLine {
    let laid = layout_line(line, content);
    let used = spans_width(&laid);
    let mut fragments = vec![span("│ ", Role::Border)];
    fragments.extend(laid);
    fragments.push(span(" ".repeat(content - used), Role::Text));
    fragments.push(span(" │", Role::Border));
    row(line.role, fragments)
}
fn visible_width(value: &str) -> usize {
    ratatui::text::Line::from(value).width()
}
fn fit_middle(value: &str, width: usize) -> String {
    if visible_width(value) <= width {
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
    let prefix = take_cells(value.chars(), left);
    let suffix = take_cells(value.chars().rev(), right)
        .chars()
        .rev()
        .collect::<String>();
    format!("{prefix}…{suffix}")
}

fn take_cells(characters: impl Iterator<Item = char>, limit: usize) -> String {
    let mut text = String::new();
    for character in characters {
        let candidate = format!("{text}{character}");
        if visible_width(&candidate) > limit {
            break;
        }
        text.push(character);
    }
    text
}
fn shortest_path(path: &str) -> String {
    let path = sanitize(path);
    let parts: Vec<_> = path.split('/').filter(|part| !part.is_empty()).collect();
    if parts.len() > 2 {
        format!("…/{}/{}", parts[parts.len() - 2], parts[parts.len() - 1])
    } else {
        path
    }
}
fn sanitize(value: &str) -> String {
    value
        .chars()
        .map(|character| {
            if character.is_control() {
                ' '
            } else {
                character
            }
        })
        .collect()
}
fn format_os(os: &crate::model::OperatingSystem) -> String {
    format!(
        "{}{}",
        os.name,
        os.version
            .as_ref()
            .map(|version| format!(" {version}"))
            .unwrap_or_default()
    )
}
fn git_head(git: &Git) -> String {
    match &git.head {
        GitHead::Branch { name } => name.clone(),
        GitHead::Detached { commit } => format!("detached@{commit}"),
        GitHead::Unborn => "unborn".into(),
    }
}
fn toolchain_version(tool: &crate::model::Toolchain) -> &str {
    tool.version
        .as_deref()
        .and_then(|version| {
            let without_runtime = strip_version_prefix(version, &tool.runtime).unwrap_or(version);
            strip_version_prefix(without_runtime, &tool.language)
                .or((without_runtime != version).then_some(without_runtime))
        })
        .filter(|version| !version.is_empty())
        .or(tool.version.as_deref())
        .unwrap_or("unknown")
}

fn strip_version_prefix<'a>(version: &'a str, prefix: &str) -> Option<&'a str> {
    let head = version.get(..prefix.len())?;
    let tail = version.get(prefix.len()..)?;
    (head.eq_ignore_ascii_case(prefix) && tail.chars().next().is_some_and(char::is_whitespace))
        .then_some(tail.trim_start())
}
fn coverage_role(coverage: &Coverage) -> Role {
    match coverage.stale {
        Freshness::Current => Role::Coverage,
        Freshness::Stale => Role::Warning,
        Freshness::Unknown => Role::Secondary,
    }
}
fn diagnostic_role(severity: DiagnosticSeverity) -> Role {
    match severity {
        DiagnosticSeverity::Info => Role::Secondary,
        DiagnosticSeverity::Warning => Role::Warning,
        DiagnosticSeverity::Error => Role::Error,
    }
}
fn severity_name(severity: DiagnosticSeverity) -> &'static str {
    match severity {
        DiagnosticSeverity::Info => "info",
        DiagnosticSeverity::Warning => "warning",
        DiagnosticSeverity::Error => "error",
    }
}
fn coverage_format_name(format: CoverageFormat) -> &'static str {
    match format {
        CoverageFormat::Lcov => "lcov",
        CoverageFormat::CoberturaXml => "cobertura-xml",
        CoverageFormat::JacocoXml => "jacoco-xml",
        CoverageFormat::GoCoverprofile => "go-coverprofile",
        CoverageFormat::IstanbulSummary => "istanbul-summary",
    }
}
fn freshness_name(value: Freshness) -> &'static str {
    match value {
        Freshness::Current => "current",
        Freshness::Stale => "stale",
        Freshness::Unknown => "unknown",
    }
}
fn format_percent(value: f64) -> String {
    format!("{value:.1}%")
}
fn percent_of(numerator: u64, denominator: u64) -> Option<f64> {
    (denominator > 0).then(|| numerator as f64 / denominator as f64 * 100.0)
}
fn format_number(value: u64) -> String {
    value.to_string()
}
fn format_bytes(bytes: u64) -> String {
    const UNITS: [&str; 5] = ["B", "KiB", "MiB", "GiB", "TiB"];
    if bytes < 1024 {
        return format!("{bytes} B");
    }
    let mut value = bytes as f64;
    let mut unit = 0;
    while value >= 1024.0 && unit < UNITS.len() - 1 {
        value /= 1024.0;
        unit += 1;
    }
    format!("{value:.1} {}", UNITS[unit])
}
fn format_uptime(seconds: Seconds, verbose: bool) -> String {
    if verbose {
        return format!("{} seconds", seconds.value);
    }
    let days = seconds.value / 86_400;
    let hours = seconds.value % 86_400 / 3_600;
    let minutes = seconds.value % 3_600 / 60;
    if days > 0 {
        format!("{days}d {hours}h")
    } else if hours > 0 {
        format!("{hours}h {minutes}m")
    } else {
        format!("{minutes}m")
    }
}
fn format_offset(seconds: i32) -> String {
    let seconds = i64::from(seconds);
    let sign = if seconds < 0 { '-' } else { '+' };
    let absolute = seconds.abs();
    format!("UTC{sign}{:02}:{:02}", absolute / 3_600, absolute / 60 % 60)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{
        Context, CountUnit, Cpu, Diagnostic, Disk, LanguageLoc, Loc, LocalTime, Memory,
        OperatingSystem, Project, SecondsUnit, SnapshotMode, System, Toolchain,
    };
    use crate::render::theme::RuntimeRole;
    fn fixture(git: bool) -> Snapshot {
        let mut snapshot = Snapshot::empty(SnapshotMode::OneShot);
        snapshot.context = Context {
            worktree: git.then(|| "/work/hoshino".into()),
            directory: Some("/work/hoshino/src".into()),
        };
        snapshot.system = System {
            os: Some(OperatingSystem {
                name: "macOS".into(),
                version: Some("26".into()),
                architecture: None,
            }),
            cpu: Some(Cpu {
                label: Some("Apple CPU".into()),
                utilization_percent: Some(12.5),
                frequency_mhz: Some(3200),
            }),
            memory: Some(Memory {
                used_bytes: 8 << 30,
                total_bytes: 16 << 30,
            }),
            disks: vec![Disk {
                mount: "/".into(),
                used_bytes: 40,
                total_bytes: 100,
            }],
            uptime: Some(Seconds {
                value: 90_061,
                unit: SecondsUnit::Seconds,
            }),
        };
        snapshot.time = LocalTime {
            local: Some("12:34:56".into()),
            date: Some("2026-09-16".into()),
            timezone_name: Some("America/Los_Angeles".into()),
            timezone_offset_seconds: Some(-25_200),
            day_progress_percent: Some(52.5),
        };
        if git {
            snapshot.git = Some(Git {
                head: GitHead::Branch {
                    name: "main".into(),
                },
                dirty: true,
                staged: 2,
                unstaged: 3,
                conflicts: 1,
                ahead: Some(4),
                upstream: Some("origin/main".into()),
            });
            snapshot.project = Some(Project {
                primary_language: Some("Rust".into()),
                toolchains: vec![
                    Toolchain {
                        language: "Rust".into(),
                        runtime: "rustc".into(),
                        version: Some("1.98.1".into()),
                    },
                    Toolchain {
                        language: "Python".into(),
                        runtime: "python3".into(),
                        version: Some("3.13".into()),
                    },
                ],
                loc: Some(Loc {
                    total: 1234,
                    unit: CountUnit::Lines,
                    truncated: false,
                    by_language: vec![
                        LanguageLoc {
                            language: "Rust".into(),
                            lines: 1000,
                            unit: CountUnit::Lines,
                        },
                        LanguageLoc {
                            language: "Python".into(),
                            lines: 234,
                            unit: CountUnit::Lines,
                        },
                    ],
                }),
                coverage: Some(Coverage {
                    line_percent: Some(87.5),
                    branch_percent: Some(62.0),
                    function_percent: Some(91.0),
                    source: Some("fixture".into()),
                    format: Some(CoverageFormat::Lcov),
                    report_path: Some("coverage/lcov.info".into()),
                    report_modified: Some(Seconds {
                        value: 1_700_000_000,
                        unit: SecondsUnit::Seconds,
                    }),
                    stale: Freshness::Current,
                }),
            });
        }
        snapshot.diagnostics = vec![Diagnostic {
            severity: DiagnosticSeverity::Warning,
            code: "coverage-stale".into(),
            message: "report needs refresh".into(),
            subject: None,
        }];
        snapshot
    }
    fn options(full: bool, width: usize) -> TextOptions {
        TextOptions {
            full,
            width: Some(width),
            ..TextOptions::default()
        }
    }
    fn spans_of(full: bool, width: usize) -> Vec<CardSpan> {
        render_card(&fixture(true), options(full, width))
            .iter()
            .flat_map(card_spans)
            .collect()
    }
    fn find<'a>(lines: &'a [CardLine], prefix: &str) -> &'a CardLine {
        lines
            .iter()
            .find(|line| line.text.starts_with(prefix))
            .unwrap_or_else(|| panic!("no line starting with {prefix:?}"))
    }

    #[test]
    fn normal_is_a_compact_instrument_rail() {
        let body = card_body(&fixture(true), false);
        assert!((6..=9).contains(&body.identity.len()));
        assert!(body.sections.is_empty());
        let text = render_text(&fixture(true), options(false, 104));
        for fact in [
            "/work/hoshino on main · conflict",
            "rustc 1.98.1",
            "macOS 26 · Apple CPU @ 3200 MHz · 12.5%",
            "up 1d 1h",
            "mem  ",
            "disk ",
            "12:34:56 ",
            "52.5% · 2026-09-16",
            "project: 1234 LOC",
        ] {
            assert!(text.contains(fact), "{fact:?} missing from\n{text}");
        }
        assert!(text.lines().all(|line| visible_width(line) == 104));
    }

    #[test]
    fn sparse_non_git_omits_unavailable_rows() {
        let text = render_text(&fixture(false), options(false, 40));
        assert!(!text.contains("main"));
        assert!(!text.contains("—"));
        assert!(!text.contains("coverage"));
        assert!(!text.contains("project:"));
    }

    #[test]
    fn full_has_ordered_nonempty_semantic_sections() {
        let body = card_body(&fixture(true), true);
        assert_eq!(
            body.sections
                .iter()
                .map(|section| section.name)
                .collect::<Vec<_>>(),
            ["system & health", "active context", "project telemetry"]
        );
        let text = render_text(&fixture(true), options(true, 120));
        assert_eq!(text.matches("source fixture").count(), 1);
        assert_eq!(text.matches("coverage/lcov.info").count(), 1);
        assert!(
            !text
                .lines()
                .any(|line| line.trim_matches(['│', ' ']).is_empty())
        );
    }

    #[test]
    fn full_keeps_ram_capacity_in_one_place() {
        let output = render_text(&fixture(true), options(true, 120));
        assert_eq!(output.matches("8.0 GiB / 16.0 GiB").count(), 1);
        assert!(!output.contains("RAM"));
    }

    #[test]
    fn python_runtime_strips_a_case_insensitive_language_version_prefix() {
        let mut snapshot = fixture(true);
        {
            let project = snapshot.project.as_mut().unwrap();
            project.primary_language = Some("Python".into());
            project.toolchains[1].version = Some("Python 3.14.6".into());
        }
        let output = render_text(&snapshot, options(false, 104));
        assert!(output.contains("python3 3.14.6"));
        assert!(!output.contains("python3 Python"));
        assert_eq!(
            toolchain_version(&snapshot.project.as_ref().unwrap().toolchains[0]),
            "1.98.1"
        );
    }

    #[test]
    fn normal_runtime_is_short_and_full_keeps_build_provenance() {
        let mut snapshot = fixture(true);
        snapshot.project.as_mut().unwrap().toolchains[0].version =
            Some("rustc 1.98.1 (48a229cea 2026-09-01)".into());
        let normal = render_text(&snapshot, options(false, 104));
        assert!(normal.contains("rustc 1.98.1"));
        assert!(!normal.contains("48a229cea"));
        let full = render_text(&snapshot, options(true, 120));
        assert!(full.contains("rustc 1.98.1 (48a229cea 2026-09-01)"));
    }

    #[test]
    fn coverage_report_modified_is_explicit_unix_provenance_not_an_age() {
        let output = render_text(&fixture(true), options(true, 120));
        assert!(output.contains("modified Unix 1700000000"));
        assert!(!output.contains("1700000000 seconds"));
        assert!(!output.contains("report age"));
    }

    #[test]
    fn gradients_only_fill_instrument_cells() {
        for full in [false, true] {
            let spans = spans_of(full, 200);
            let gradients = spans
                .iter()
                .filter(|span| span.style == SpanStyle::Gradient)
                .collect::<Vec<_>>();
            assert!(!gradients.is_empty());
            assert!(gradients.iter().all(|span| {
                span.text.chars().all(|character| character == '━')
                    && matches!(
                        span.role,
                        Role::System(_) | Role::Coverage | Role::Warning | Role::Progress
                    )
            }));
        }
    }

    #[test]
    fn ansi_and_plain_share_fragments_and_width_caps() {
        for full in [false, true] {
            let plain = render_text(&fixture(true), options(full, 200));
            let card = render_card(&fixture(true), options(full, 200));
            assert!(
                card.iter()
                    .all(|line| visible_width(&line.text) == if full { 120 } else { 104 })
            );
            let ansi = render_text(
                &fixture(true),
                TextOptions {
                    color: true,
                    ..options(full, 200)
                },
            );
            assert!(ansi.matches("\x1b[0m").count() > 0);
            assert_eq!(plain.lines().count(), ansi.lines().count());
        }
    }

    #[test]
    fn narrow_is_deterministic_unframed() {
        for full in [false, true] {
            for width in 1..20 {
                let output = render_text(&fixture(true), options(full, width));
                assert!(output.lines().all(|line| visible_width(line) <= width));
                assert!(!output.contains('╭'));
            }
        }
    }

    #[test]
    fn conflict_stale_python_and_long_values_remain_textual() {
        let mut snapshot = fixture(true);
        snapshot.git.as_mut().unwrap().head = GitHead::Branch {
            name: format!("feature/{}-tail", "very-long-".repeat(20)),
        };
        snapshot.project.as_mut().unwrap().primary_language = Some("Python".into());
        snapshot
            .project
            .as_mut()
            .unwrap()
            .coverage
            .as_mut()
            .unwrap()
            .stale = Freshness::Stale;
        let output = render_text(&snapshot, options(true, 80));
        assert!(output.contains("conflict"));
        assert!(output.contains("stale"));
        assert!(output.contains("Python"));
        assert!(output.contains("tail"));
    }

    #[test]
    fn every_width_is_cell_bounded_and_small_widths_are_unframed() {
        for full in [false, true] {
            for width in [1, 2, 19, 20, 21, 30, 40, 60, 80, 104, 120, 160] {
                let output = render_text(&fixture(true), options(full, width));
                assert!(
                    output.lines().all(|line| visible_width(line) <= width),
                    "{width}:\n{output}"
                );
                if width < 20 {
                    assert!(!output.contains('╭'));
                } else {
                    let cap = if full { 120 } else { 104 };
                    assert!(
                        output
                            .lines()
                            .all(|line| visible_width(line) == width.min(cap))
                    );
                }
            }
        }
    }

    #[test]
    fn wide_and_combining_values_obey_terminal_cell_boundaries() {
        assert_eq!(visible_width("界"), 2);
        assert_eq!(visible_width("e\u{301}"), 1);
        let value = "prefix-界e\u{301}-distinguishing-suffix";
        for width in 1..=16 {
            let fitted = fit_middle(value, width);
            assert!(visible_width(&fitted) <= width, "{width}: {fitted:?}");
        }
        assert!(fit_middle(value, 16).contains("suffix"));
        let mut snapshot = fixture(false);
        snapshot.context.directory = Some(format!("/prefix/{value}"));
        for width in [20, 21, 40] {
            assert!(
                render_text(&snapshot, options(false, width))
                    .lines()
                    .all(|line| visible_width(line) == width)
            );
        }
    }

    #[test]
    fn border_spans_and_framed_geometry_are_independent_of_values() {
        let card = render_card(&fixture(true), options(false, 104));
        assert!(card.iter().all(|line| visible_width(&line.text) == 104));
        let spans: Vec<_> = card.iter().flat_map(card_spans).collect();
        assert!(
            spans
                .iter()
                .filter(|span| span
                    .text
                    .chars()
                    .any(|c| matches!(c, '╭' | '╮' | '╰' | '╯' | '│' | '├')))
                .all(|span| span.role == Role::Border)
        );
        assert!(
            spans
                .iter()
                .any(|span| span.text.contains("main") && span.role == Role::Git)
        );
        let lines = render_text(&fixture(true), options(false, 104));
        assert!(
            lines
                .lines()
                .skip(1)
                .take(card.len() - 2)
                .all(|line| { line.starts_with("│ ") && line.ends_with(" │") }),
            "content keeps one cell of padding inside the frame"
        );
    }

    #[test]
    fn extreme_non_git_path_keeps_a_closed_frame() {
        let mut snapshot = fixture(false);
        snapshot.context.directory = Some(format!("/{}tail", "very-long-directory/".repeat(20)));
        let output = render_text(&snapshot, options(false, 80));
        let lines: Vec<_> = output.lines().collect();
        assert!(
            lines
                .first()
                .is_some_and(|line| line.starts_with("╭─hoshino ") && line.ends_with('╮'))
        );
        assert!(
            lines
                .last()
                .is_some_and(|line| line.starts_with('╰') && line.ends_with('╯'))
        );
        assert!(lines.iter().all(|line| visible_width(line) == 80));
    }

    #[test]
    fn full_sections_are_framed_and_normal_and_full_caps_differ() {
        for (full, cap) in [(false, 104), (true, 120)] {
            let output = render_text(&fixture(true), options(full, 160));
            let lines: Vec<_> = output.lines().collect();
            assert!(lines.iter().all(|line| visible_width(line) == cap));
            assert!(lines[1..lines.len() - 1].iter().all(|line| {
                (line.starts_with('│') && line.ends_with('│'))
                    || (full && line.starts_with("├─ ") && line.ends_with("─┤"))
            }));
            if full {
                for section in ["system & health", "active context", "project telemetry"] {
                    assert!(output.contains(&format!("├─ {section} ─")));
                }
            }
        }
    }

    #[test]
    fn git_clean_dirty_and_conflict_have_distinct_textual_states() {
        let mut clean = fixture(true);
        let git = clean.git.as_mut().unwrap();
        git.dirty = false;
        git.staged = 0;
        git.unstaged = 0;
        git.conflicts = 0;
        assert!(render_text(&clean, options(false, 104)).contains("main · clean"));
        clean.git.as_mut().unwrap().dirty = true;
        assert!(render_text(&clean, options(false, 104)).contains("main · dirty"));
        clean.git.as_mut().unwrap().conflicts = 1;
        assert!(render_text(&clean, options(false, 104)).contains("main · conflict"));
    }

    #[test]
    fn whole_values_take_their_roles_and_labels_stay_secondary() {
        let body = card_body(&fixture(true), false);
        let place = &body.identity[0];
        assert_eq!(place.fragments[0].role, Role::Primary);
        assert_eq!(place.fragments[1].role, Role::Secondary);
        assert!(
            place.fragments[2].text == "main · conflict · +2 ~3 !1 ↑4"
                && place.fragments[2].role == Role::Git
        );
        assert!(matches!(place.trailing[0].role, Role::Runtime(_)));
        let machine = &body.identity[1];
        assert!(machine.fragments.iter().any(|span| {
            span.text == "Apple CPU @ 3200 MHz · 12.5%"
                && span.role == Role::System(SystemRole::Cpu)
        }));
        assert_eq!(machine.trailing[1].role, Role::System(SystemRole::Health));
        let memory = find(&body.identity, "mem");
        assert_eq!(memory.fragments[0].role, Role::Secondary);
        assert!(memory.fragments.iter().any(|span| {
            span.text == "50.0%  8.0 GiB/16.0 GiB" && span.role == Role::System(SystemRole::Memory)
        }));
        assert!(matches!(
            memory.flex,
            Some(FlexSlot {
                kind: Flex::Meter {
                    role: Role::System(SystemRole::Memory),
                    ..
                },
                ..
            })
        ));
        let disk = find(&body.identity, "disk");
        assert!(disk.fragments.iter().any(|span| {
            span.text.starts_with("40.0%") && span.role == Role::System(SystemRole::Disk)
        }));
        assert!(
            body.identity
                .iter()
                .flat_map(|line| &line.fragments)
                .all(|span| span.style == SpanStyle::Solid)
        );
    }

    #[test]
    fn rails_are_solid_pink_without_brackets() {
        let spans = spans_of(true, 120);
        for header in ["system & health", "active context", "project telemetry"] {
            assert!(spans.iter().any(|span| span.text == header
                && span.role == Role::Primary
                && span.style == SpanStyle::Solid));
        }
        assert!(!spans.iter().any(|span| span.text.contains("[ ")));
        assert!(
            spans
                .iter()
                .filter(|span| span.text.starts_with("─ "))
                .all(|span| span.role == Role::Border)
        );
    }

    #[test]
    fn time_is_lavender_and_plain_output_is_control_free() {
        let plain = render_text(&fixture(true), options(false, 104));
        assert!(!plain.contains('\x1b'));
        let colors = palette(Theme::DuskDarker);
        let body = card_body(&fixture(true), false);
        let ruler = find(&body.identity, "12:34:56");
        assert!(
            expanded_card_spans(ruler, colors)
                .iter()
                .any(|chunk| chunk.text.contains("12:34:56") && chunk.color == colors.lavender)
        );
        let full = card_body(&fixture(true), true);
        let time = find(&full.sections[1].left, "time");
        assert!(time.fragments.iter().any(|span| {
            span.text == "12:34:56 · 2026-09-16 · America/Los_Angeles" && span.role == Role::Time
        }));
        let colored = render_text(
            &fixture(true),
            TextOptions {
                color: true,
                ..options(false, 104)
            },
        );
        assert!(colored.contains(&colors.ansi_foreground(Role::Time)));
    }

    #[test]
    fn full_diagnostics_are_inside_the_frame_and_unavailable_facts_are_omitted() {
        let full = render_text(&fixture(true), options(true, 120));
        assert!(full.contains("diag  coverage-stale warning · report needs refresh"));
        assert!(
            full.lines()
                .filter(|line| line.contains("diag  "))
                .all(|line| line.starts_with('│') && line.ends_with('│'))
        );
        let sparse = render_text(&Snapshot::empty(SnapshotMode::OneShot), options(true, 80));
        assert!(!sparse.contains("coverage"));
        assert!(!sparse.contains("unknown"));
    }

    #[test]
    fn full_owns_a_compact_header() {
        let body = card_body(&fixture(true), true);
        assert_eq!(body.identity.len(), 2);
        assert_eq!(body.identity_layout_height, 6);
        let git = &body.identity[0];
        assert!(git.text.starts_with("git   main · conflict"));
        assert!(git.text.contains(" → origin/main"));
        assert!(git.trailing.iter().any(|span| span.text == "rustc 1.98.1"));
        assert_eq!(body.identity[1].text, "dir   /work/hoshino · macOS 26");
        assert!(body.identity.iter().all(|line| !line.text.contains("GiB")));
    }

    #[test]
    fn trailing_support_drops_before_the_instrument_and_text_last() {
        let line = with_trailing(
            with_flex(
                row(
                    Role::Text,
                    vec![span("ab", Role::Text), span("cd", Role::Text)],
                ),
                1,
                Flex::Meter {
                    permille: 500,
                    role: Role::System(SystemRole::Cpu),
                },
                (4, 10),
            ),
            vec![span("tail", Role::Secondary)],
        );
        let text = |width| {
            layout_line(&line, width)
                .iter()
                .map(|span| span.text.clone())
                .collect::<String>()
        };
        assert_eq!(
            text(30),
            format!("ab{}cd{}tail", "━━━━━╌╌╌╌╌", " ".repeat(12))
        );
        // 2 + 2 fixed, 4-cell meter would need 12 with trailing: trailing goes first.
        assert_eq!(text(11), "ab━━━━╌╌╌cd");
        assert_eq!(text(7), "abcd");
        assert_eq!(visible_width(&text(3)), 3);
    }

    #[test]
    fn ruler_quarters_align_with_the_axis_labels() {
        let body = card_body(&fixture(true), true);
        let active = &body.sections[1].left;
        let ruler_index = active
            .iter()
            .position(|line| {
                matches!(
                    line.flex,
                    Some(FlexSlot {
                        kind: Flex::Ruler { .. },
                        ..
                    })
                )
            })
            .unwrap();
        for width in [40, 80, 116] {
            let ruler = layout_line(&active[ruler_index], width)
                .iter()
                .map(|span| span.text.clone())
                .collect::<String>();
            let axis = layout_line(&active[ruler_index + 1], width)
                .iter()
                .map(|span| span.text.clone())
                .collect::<String>();
            assert_eq!(visible_width(&ruler), visible_width(&axis));
            let ruler: Vec<char> = ruler.chars().collect();
            let axis: Vec<char> = axis.chars().collect();
            let end = ruler.iter().position(|c| *c == '┤').unwrap();
            let start = FULL_GUTTER;
            let quarter = (end - start) / 4;
            for (tick, label) in [(1, '0'), (2, '1'), (3, '1')] {
                let column = start + tick * quarter;
                assert!(matches!(ruler[column], '┼' | '●' | '━'), "{width} {tick}");
                assert_eq!(axis[column], label, "{width} {tick}");
            }
            assert_eq!(&axis[end - 1..=end], &['2', '4']);
            // 52.5% of the day puts the marker just past the 12:00 tick.
            let marker = ruler.iter().position(|c| *c == '●').unwrap();
            assert!(marker >= start + 2 * quarter && marker <= start + 2 * quarter + 2);
        }
    }

    #[test]
    fn disk_table_aligns_skips_mirrored_volumes_and_flags_full_disks() {
        let mut snapshot = fixture(true);
        snapshot.system.disks = vec![
            Disk {
                mount: "/".into(),
                used_bytes: 40 << 30,
                total_bytes: 100 << 30,
            },
            // The APFS data volume shares the container; its byte count drifts
            // slightly between reads but prints identically.
            Disk {
                mount: "/System/Volumes/Data".into(),
                used_bytes: (40 << 30) + 4096,
                total_bytes: 100 << 30,
            },
            Disk {
                mount: "/Volumes/OpenCode 2.0.16-arm64".into(),
                used_bytes: 996,
                total_bytes: 1000,
            },
        ];
        let body = card_body(&snapshot, true);
        let system = &body.sections[0].left;
        assert!(
            !system
                .iter()
                .any(|line| line.text.contains("/System/Volumes/Data"))
        );
        let full = system
            .iter()
            .find(|line| line.text.contains("/Volumes/O…") && line.text.contains("arm64"))
            .unwrap();
        assert!(
            full.fragments
                .iter()
                .any(|span| span.text.trim() == "▲ full" && span.role == Role::Warning)
        );
        let root = system
            .iter()
            .find(|line| line.text.starts_with("disk  / "))
            .unwrap();
        assert!(!root.text.contains("full"));
        let header = system
            .iter()
            .find(|line| line.text.contains("mount"))
            .unwrap();
        for width in [80, 116] {
            let columns = |line: &CardLine| {
                let text = layout_line(line, width)
                    .iter()
                    .map(|span| span.text.clone())
                    .collect::<String>();
                (
                    visible_width(&text),
                    text.find('%').map(|index| visible_width(&text[..index])),
                )
            };
            let (header_width, header_percent) = columns(header);
            let (root_width, root_percent) = columns(root);
            let (full_width, full_percent) = columns(full);
            assert_eq!(header_width, root_width);
            assert_eq!(root_width, full_width);
            assert_eq!(header_percent, root_percent);
            assert_eq!(root_percent, full_percent);
        }
        let narrow = section_rows(&body.sections[0], 60, false);
        assert!(!narrow.iter().any(|line| line.text.contains("mount")));
        let compact = layout_line(
            narrow
                .iter()
                .find(|line| line.text.starts_with("disk  / "))
                .unwrap(),
            60,
        );
        assert_eq!(
            compact
                .iter()
                .map(|span| span.text.as_str())
                .collect::<String>(),
            "disk  / 40.0% · 40.0 GiB / 100.0 GiB"
        );
        assert!(
            section_rows(&body.sections[0], 116, true)
                .iter()
                .any(|line| line.text.contains("mount"))
        );
        let normal = card_body(&snapshot, false);
        assert!(find(&normal.identity, "disk").text.contains("40.0%"));
    }

    #[test]
    fn language_stack_fills_its_width_in_language_roles() {
        let body = card_body(&fixture(true), true);
        let project = &body.sections[2].left;
        let stack = project
            .iter()
            .find(|line| {
                matches!(
                    line.flex,
                    Some(FlexSlot {
                        kind: Flex::Stack(_),
                        ..
                    })
                )
            })
            .unwrap();
        let spans = layout_line(stack, 60);
        let runs = spans
            .iter()
            .filter(|span| span.text.starts_with('█'))
            .collect::<Vec<_>>();
        assert_eq!(runs.len(), 2);
        assert_eq!(runs[0].role, Role::Runtime(RuntimeRole::Rust));
        assert_eq!(runs[1].role, Role::Runtime(RuntimeRole::Python));
        assert_eq!(
            runs.iter()
                .map(|span| visible_width(&span.text))
                .sum::<usize>(),
            60 - FULL_GUTTER
        );
        assert_eq!(visible_width(&runs[0].text), 44);
        let legend = find(project, "      ■ Rust");
        assert_eq!(legend.text, "      ■ Rust 1000 lines · 81.0%");
    }

    #[test]
    fn full_git_row_keeps_zero_counts() {
        let mut snapshot = fixture(true);
        let git = snapshot.git.as_mut().unwrap();
        git.dirty = false;
        git.staged = 0;
        git.unstaged = 0;
        git.conflicts = 0;
        git.ahead = None;
        let body = card_body(&snapshot, true);
        let git = find(&body.sections[1].left, "git");
        assert_eq!(git.text, "git   staged 0    unstaged 0    conflicts 0");
        assert!(
            git.fragments
                .iter()
                .filter(|span| span.text == "0")
                .all(|span| span.role == Role::Git)
        );
    }

    #[test]
    fn coverage_rows_follow_freshness_roles() {
        let mut snapshot = fixture(true);
        for (freshness, role) in [
            (Freshness::Current, Role::Coverage),
            (Freshness::Stale, Role::Warning),
            (Freshness::Unknown, Role::Secondary),
        ] {
            snapshot
                .project
                .as_mut()
                .unwrap()
                .coverage
                .as_mut()
                .unwrap()
                .stale = freshness;
            let body = card_body(&snapshot, true);
            let line = find(&body.sections[2].left, "cov   line");
            assert_eq!(line.role, role);
            assert!(matches!(
                &line.flex,
                Some(FlexSlot { kind: Flex::Meter { role: meter, .. }, .. }) if *meter == role
            ));
            let provenance = body.sections[2].left.last().unwrap();
            assert_eq!(provenance.fragments.last().unwrap().role, role);
            assert_eq!(
                provenance.fragments.last().unwrap().text,
                freshness_name(freshness)
            );
        }
    }
}
