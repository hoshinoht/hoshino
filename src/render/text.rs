use crate::{
    config::Theme,
    model::{
        Coverage, CoverageFormat, DiagnosticSeverity, Freshness, Git, GitHead, Seconds, Snapshot,
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
/// callers; it is derived from `fragments` and is never reparsed for styling.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CardLine {
    pub text: String,
    pub role: Role,
    pub fragments: Vec<CardSpan>,
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

fn span(text: impl Into<String>, role: Role) -> CardSpan {
    CardSpan {
        text: sanitize(&text.into()),
        role,
        style: SpanStyle::Solid,
    }
}
fn progress(text: impl Into<String>) -> CardSpan {
    CardSpan {
        text: text.into(),
        role: Role::Progress,
        style: SpanStyle::Gradient,
    }
}
fn metric_progress(text: impl Into<String>, role: Role) -> CardSpan {
    CardSpan {
        text: text.into(),
        role,
        style: SpanStyle::Gradient,
    }
}
fn row(role: Role, fragments: Vec<CardSpan>) -> CardLine {
    let text = fragments
        .iter()
        .map(|fragment| fragment.text.as_str())
        .collect();
    CardLine {
        text,
        role,
        fragments,
    }
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
fn labelled_progress(label: &str, value: &str, percent: f64) -> CardLine {
    row(
        Role::Time,
        vec![
            span(format!("{label}: "), Role::Secondary),
            span(format!("{value} "), Role::Time),
            span("[", Role::Time),
            progress("█".repeat(progress_cells(percent, 8))),
            span("░".repeat(8 - progress_cells(percent, 8)), Role::Time),
            span("]", Role::Time),
        ],
    )
}
fn metric_progress_row(
    label: &str,
    percent: f64,
    role: Role,
    metric: impl Into<String>,
    supporting: impl Into<String>,
) -> CardLine {
    let filled = progress_cells(percent, 8);
    row(
        role,
        vec![
            span(format!("{label}: "), Role::Secondary),
            span("[", Role::Secondary),
            metric_progress("█".repeat(filled), role),
            span("░".repeat(8 - filled), Role::Secondary),
            span("]", Role::Secondary),
            span(format!(" {}", metric.into()), role),
            span(supporting, Role::Text),
        ],
    )
}

/// Build semantic rows directly from Snapshot. This is the frozen interface for
/// the one-shot compositor and TUI owners: title + identity + ordered sections.
pub fn card_body(snapshot: &Snapshot, full: bool) -> CardBody {
    let mut identity = Vec::new();
    let workspace = snapshot
        .context
        .worktree
        .as_deref()
        .or(snapshot.context.directory.as_deref());
    if let Some(path) = workspace {
        let mut value = shortest_path(path);
        if let Some(os) = snapshot.system.os.as_ref() {
            value.push_str(&format!(" · {}", format_os(os)));
        }
        identity.push(labelled("workspace", value, Role::Primary));
    }
    if let Some(git) = snapshot.git.as_ref() {
        identity.push(if full {
            git_header(git)
        } else {
            git_summary(git)
        });
    }
    if let Some(project) = snapshot.project.as_ref()
        && let Some(language) = project.primary_language.as_deref()
    {
        let toolchain = project
            .toolchains
            .iter()
            .find(|tool| tool.language.eq_ignore_ascii_case(language));
        let role = toolchain
            .map(|tool| Role::Runtime(runtime_role(&tool.language, &tool.runtime)))
            .unwrap_or_else(|| Role::Runtime(runtime_role(language, language)));
        let value = toolchain.map_or_else(
            || language.to_owned(),
            |tool| format!("{} {}", tool.runtime, toolchain_version(tool)),
        );
        identity.push(labelled("runtime", value, role));
    }
    if !full {
        if let Some(health) = health_summary(snapshot) {
            identity.push(health);
        }
        if let Some(uptime) = snapshot.system.uptime {
            identity.push(labelled(
                "uptime",
                format_uptime(uptime, false),
                Role::System(SystemRole::Health),
            ));
        }
        if let Some(clock) = time_summary(snapshot) {
            identity.push(clock);
        }
        if let Some(day) = snapshot
            .time
            .day_progress_percent
            .filter(|value| value.is_finite())
        {
            identity.push(labelled_progress("day", &format_percent(day), day));
        }
        if let Some(project) = snapshot.project.as_ref()
            && let Some(summary) = project_summary(
                project.coverage.as_ref(),
                project.loc.as_ref().map(|loc| (loc.total, loc.truncated)),
                true,
            )
        {
            identity.push(summary);
        }
    }
    if !full && !snapshot.diagnostics.is_empty() {
        let diagnostic = snapshot
            .diagnostics
            .iter()
            .max_by_key(|diagnostic| match diagnostic.severity {
                DiagnosticSeverity::Error => 2,
                DiagnosticSeverity::Warning => 1,
                DiagnosticSeverity::Info => 0,
            })
            .expect("nonempty");
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
    if !full {
        identity.truncate(9);
    }

    let mut sections = Vec::new();
    if full {
        if let Some((left, right)) = system_details(snapshot) {
            sections.push(CardSection {
                name: "system & health",
                role: Role::System(SystemRole::Health),
                left,
                right,
            });
        }
        if let Some((left, right)) = active_context(snapshot) {
            sections.push(CardSection {
                name: "active context",
                role: Role::Context,
                left,
                right,
            });
        }
        if let Some((left, right)) = project_telemetry(snapshot) {
            sections.push(CardSection {
                name: "project telemetry",
                role: Role::Primary,
                left,
                right,
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

fn git_summary(git: &Git) -> CardLine {
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
    labelled(
        "git",
        format!("{} · {state}{suffix}", git_head(git)),
        Role::Git,
    )
}
fn git_header(git: &Git) -> CardLine {
    let state = if git.conflicts > 0 {
        "conflict"
    } else if git.dirty || git.staged > 0 || git.unstaged > 0 {
        "dirty"
    } else {
        "clean"
    };
    labelled("git", format!("{} · {state}", git_head(git)), Role::Git)
}
fn health_summary(snapshot: &Snapshot) -> Option<CardLine> {
    let mut metrics = Vec::new();
    if let Some(cpu) = snapshot.system.cpu.as_ref() {
        let value = cpu
            .utilization_percent
            .filter(|value| value.is_finite())
            .map(format_percent)
            .or_else(|| cpu.label.clone());
        if let Some(value) = value {
            metrics.push(("CPU", value, Role::System(SystemRole::Cpu)));
        }
    }
    if let Some(memory) = snapshot.system.memory.as_ref() {
        metrics.push((
            "RAM",
            format!(
                "{}/{}",
                format_bytes(memory.used_bytes),
                format_bytes(memory.total_bytes)
            ),
            Role::System(SystemRole::Memory),
        ));
    }
    if let Some(disk) = snapshot.system.disks.first() {
        metrics.push((
            "disk",
            format!(
                "{} {}",
                disk.mount,
                disk_percent(disk.used_bytes, disk.total_bytes)
            ),
            Role::System(SystemRole::Disk),
        ));
    }
    (!metrics.is_empty()).then(|| {
        let mut fragments = vec![span("health: ", Role::Secondary)];
        for (index, (label, value, role)) in metrics.into_iter().enumerate() {
            if index > 0 {
                fragments.push(span(" · ", Role::Secondary));
            }
            fragments.push(span(format!("{label} "), Role::Secondary));
            fragments.push(span(value, role));
        }
        row(Role::System(SystemRole::Health), fragments)
    })
}
fn time_summary(snapshot: &Snapshot) -> Option<CardLine> {
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
    (!parts.is_empty()).then(|| labelled("time", parts.join(" · "), Role::Time))
}
fn detailed_time_summary(snapshot: &Snapshot) -> Option<CardLine> {
    let time = &snapshot.time;
    let mut fragments = vec![span("time: ", Role::Secondary)];
    if let Some(local) = &time.local {
        fragments.push(span(local, Role::Time));
    }
    if let Some(date) = &time.date {
        if fragments.len() > 1 {
            fragments.push(span(" · ", Role::Secondary));
        }
        fragments.push(span(date, Role::Text));
    }
    let timezone = time
        .timezone_name
        .clone()
        .or_else(|| time.timezone_offset_seconds.map(format_offset));
    if let Some(timezone) = timezone {
        if fragments.len() > 1 {
            fragments.push(span(" · ", Role::Secondary));
        }
        fragments.push(span(timezone, Role::Text));
    }
    (fragments.len() > 1).then(|| row(Role::Time, fragments))
}
fn git_changes(git: &Git) -> CardLine {
    let mut fragments = vec![span("git changes: ", Role::Secondary)];
    for (index, (name, count)) in [
        ("staged", git.staged),
        ("unstaged", git.unstaged),
        ("conflicts", git.conflicts),
    ]
    .into_iter()
    .enumerate()
    {
        if index > 0 {
            fragments.push(span(" · ", Role::Secondary));
        }
        fragments.push(span(format!("{name} "), Role::Secondary));
        fragments.push(span(count.to_string(), Role::Git));
    }
    row(Role::Git, fragments)
}
fn project_summary(
    coverage: Option<&Coverage>,
    loc: Option<(u64, bool)>,
    show_coverage: bool,
) -> Option<CardLine> {
    let mut parts = Vec::new();
    if let Some((total, partial)) = loc {
        parts.push(format!(
            "{} LOC{}",
            format_number(total),
            if partial { " · partial" } else { "" }
        ));
    }
    if show_coverage
        && let Some(coverage) = coverage
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
fn system_details(snapshot: &Snapshot) -> Option<(Vec<CardLine>, Vec<CardLine>)> {
    let mut left = Vec::new();
    let mut right = Vec::new();
    if let Some(cpu) = snapshot.system.cpu.as_ref() {
        if let Some(percent) = cpu.utilization_percent.filter(|value| value.is_finite()) {
            let suffix = match cpu.frequency_mhz {
                // Frequency is the durable CPU fact; utilization remains readable
                // support so CPU blue does not turn into a full-width value run.
                Some(frequency) => (
                    format!("{frequency} MHz"),
                    format!(" · {}", format_percent(percent)),
                ),
                None => (format_percent(percent), String::new()),
            };
            left.push(metric_progress_row(
                "CPU",
                percent,
                Role::System(SystemRole::Cpu),
                suffix.0,
                suffix.1,
            ));
        } else if let Some(frequency) = cpu.frequency_mhz {
            left.push(labelled(
                "CPU frequency",
                format!("{frequency} MHz"),
                Role::System(SystemRole::Cpu),
            ));
        }
    }
    for (index, disk) in snapshot.system.disks.iter().enumerate() {
        let value = format!(
            "used {} / {}",
            format_bytes(disk.used_bytes),
            format_bytes(disk.total_bytes)
        );
        if index == 0 {
            if let Some(percent) = percent_of(disk.used_bytes, disk.total_bytes) {
                left.push(metric_progress_row(
                    &format!("disk {}", disk.mount),
                    percent,
                    Role::System(SystemRole::Disk),
                    disk_percent(disk.used_bytes, disk.total_bytes),
                    format!(" · {value}"),
                ));
            } else {
                left.push(labelled(&format!("disk {}", disk.mount), value, Role::Text));
            }
        } else {
            left.push(labelled(&format!("disk {}", disk.mount), value, Role::Text));
        }
    }
    if let Some(memory) = snapshot.system.memory.as_ref() {
        if let Some(percent) = percent_of(memory.used_bytes, memory.total_bytes) {
            right.push(metric_progress_row(
                "RAM",
                percent,
                Role::System(SystemRole::Memory),
                format_percent(percent),
                format!(
                    " · {} / {}",
                    format_bytes(memory.used_bytes),
                    format_bytes(memory.total_bytes)
                ),
            ));
        } else {
            right.push(labelled(
                "RAM",
                format!(
                    "used {} / {}",
                    format_bytes(memory.used_bytes),
                    format_bytes(memory.total_bytes)
                ),
                Role::System(SystemRole::Memory),
            ));
        }
    }
    if let Some(uptime) = snapshot.system.uptime {
        right.push(labelled(
            "uptime",
            format_uptime(uptime, false),
            Role::System(SystemRole::Health),
        ));
    }
    (!left.is_empty() || !right.is_empty()).then_some((left, right))
}
fn active_context(snapshot: &Snapshot) -> Option<(Vec<CardLine>, Vec<CardLine>)> {
    let mut left = Vec::new();
    let mut right = Vec::new();
    if let Some(clock) = detailed_time_summary(snapshot) {
        left.push(clock);
    }
    if let Some(day) = snapshot
        .time
        .day_progress_percent
        .filter(|value| value.is_finite())
    {
        left.push(metric_progress_row(
            "day",
            day,
            Role::Time,
            format_percent(day),
            String::new(),
        ));
    }
    if let Some(git) = snapshot.git.as_ref() {
        right.push(git_changes(git));
        if let Some(ahead) = git.ahead.filter(|count| *count > 0) {
            right.push(labelled("ahead", ahead.to_string(), Role::Git));
        }
        if let Some(upstream) = &git.upstream {
            right.push(labelled("upstream", upstream, Role::Text));
        }
        if let GitHead::Detached { commit } = &git.head {
            right.push(row(
                Role::Git,
                vec![
                    span("git state: detached commit ", Role::Secondary),
                    span(commit, Role::Git),
                ],
            ));
        }
    }
    for diagnostic in &snapshot.diagnostics {
        right.push(labelled(
            "DIAG",
            format!(
                "{} {} · {}",
                diagnostic.code,
                severity_name(diagnostic.severity),
                diagnostic.message
            ),
            diagnostic_role(diagnostic.severity),
        ));
    }
    (!left.is_empty() || !right.is_empty()).then_some((left, right))
}
fn project_telemetry(snapshot: &Snapshot) -> Option<(Vec<CardLine>, Vec<CardLine>)> {
    let project = snapshot.project.as_ref()?;
    let mut left = Vec::new();
    let mut right = Vec::new();
    if let Some(loc) = &project.loc {
        left.push(row(
            Role::Primary,
            vec![
                span("LOC: ", Role::Secondary),
                span(format_number(loc.total), Role::Primary),
                span(
                    if loc.truncated { " · partial" } else { "" },
                    Role::Secondary,
                ),
            ],
        ));
        for language in &loc.by_language {
            let share = percent_of(language.lines, loc.total)
                .filter(|_| language.lines > 0)
                .map(|percent| format!(" · {}", format_percent(percent)))
                .unwrap_or_default();
            let role = Role::Runtime(runtime_role(&language.language, &language.language));
            left.push(row(
                role,
                vec![
                    span(format!("LOC {}: ", language.language), Role::Secondary),
                    span(format_number(language.lines), role),
                    span(" lines", Role::Text),
                    span(share, Role::Secondary),
                ],
            ));
        }
    }
    if let Some(coverage) = &project.coverage {
        let role = coverage_role(coverage);
        for (label, value) in [
            ("line coverage", coverage.line_percent),
            ("branch coverage", coverage.branch_percent),
            ("function coverage", coverage.function_percent),
        ] {
            if let Some(value) = value.filter(|value| value.is_finite()) {
                right.push(metric_progress_row(
                    label,
                    value,
                    role,
                    format_percent(value),
                    String::new(),
                ));
            }
        }
        if let Some(source) = &coverage.source {
            right.push(labelled("coverage source", source, Role::Text));
        }
        if let Some(format) = coverage.format {
            right.push(labelled(
                "coverage format",
                coverage_format_name(format),
                Role::Text,
            ));
        }
        if let Some(path) = &coverage.report_path {
            right.push(labelled("coverage path", path, Role::Text));
        }
        if let Some(modified) = coverage.report_modified {
            right.push(labelled(
                "report modified",
                format!("Unix {}", modified.value),
                Role::Text,
            ));
        }
        right.push(labelled(
            "coverage freshness",
            freshness_name(coverage.stale),
            role,
        ));
    }
    (!left.is_empty() || !right.is_empty()).then_some((left, right))
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

/// Flatten the semantic body for the current surfaces. A future image compositor
/// consumes `card_body` directly; this adapter contains no label-based inference.
pub fn render_card(snapshot: &Snapshot, options: TextOptions) -> Vec<CardLine> {
    let available = options.width.unwrap_or(40).max(1);
    let cap = if options.full { 120 } else { 104 };
    let width = available.min(cap);
    let body = card_body(snapshot, options.full);
    let mut rows = body.identity;
    let mut rails = Vec::new();
    for section in body.sections {
        rails.push((rows.len(), section.name, section.role));
        rows.push(section_rail(
            section.name,
            section.role,
            width.saturating_sub(2),
        ));
        rows.extend(section_rows(&section, width.saturating_sub(2), width >= 80));
    }
    if options.embedded || width < 20 {
        return rows.into_iter().map(|line| fit_line(line, width)).collect();
    }
    let title = format!(
        "[ hoshino // {} ]",
        snapshot
            .context
            .worktree
            .as_deref()
            .or(snapshot.context.directory.as_deref())
            .map(shortest_path)
            .unwrap_or_else(|| "local status".into())
    );
    let inner = width - 2;
    let title = fit_middle(&title, inner.saturating_sub(1));
    let title_width = visible_width(&title);
    let mut output = vec![row(
        Role::Border,
        vec![
            span("╭", Role::Border),
            span("─", Role::Border),
            span(title, Role::Primary),
            span(
                "─".repeat(inner.saturating_sub(title_width.saturating_add(1))),
                Role::Border,
            ),
            span("╮", Role::Border),
        ],
    )];
    output.extend(rows.into_iter().enumerate().map(|(index, line)| {
        rails
            .iter()
            .find(|(rail, _, _)| *rail == index)
            .map_or_else(
                || frame_line(line, inner),
                |(_, name, role)| connected_section_rail(name, *role, inner),
            )
    }));
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
pub fn section_rail(name: &'static str, role: Role, width: usize) -> CardLine {
    let title = fit_middle(&format!("[ {name} ]"), width.saturating_sub(1));
    let prefix = "─";
    let suffix = width.saturating_sub(visible_width(prefix) + visible_width(&title));
    row(
        role,
        vec![
            span(prefix, role),
            span(title, role),
            span("─".repeat(suffix), role),
        ],
    )
}

/// Layout consumes the semantic columns directly. At 80 cells the columns are
/// deterministic; below that each column stacks without relabeling its facts.
pub fn section_rows(section: &CardSection, width: usize, wide: bool) -> Vec<CardLine> {
    if !wide {
        let mut rows = section
            .left
            .iter()
            .chain(&section.right)
            .cloned()
            .collect::<Vec<_>>();
        if !rows.is_empty() {
            rows.push(row(Role::Text, vec![]));
        }
        return rows;
    }
    let left_width = width.saturating_sub(3) / 2;
    let right_width = width.saturating_sub(left_width + 3);
    let paired = section.left.len().min(section.right.len());
    let mut rows = (0..paired)
        .map(|index| {
            let mut fragments = section
                .left
                .get(index)
                .map(|line| fit_fragments(&line.fragments, left_width))
                .expect("paired left row");
            fragments.push(span("   ", Role::Text));
            fragments.extend(
                section
                    .right
                    .get(index)
                    .map(|line| fit_fragments(&line.fragments, right_width))
                    .expect("paired right row"),
            );
            row(section.role, fragments)
        })
        .collect::<Vec<_>>();
    rows.extend(section.left[paired..].iter().cloned());
    rows.extend(section.right[paired..].iter().cloned());
    if !rows.is_empty() {
        rows.push(row(Role::Text, vec![]));
    }
    rows
}
fn connected_section_rail(name: &'static str, role: Role, inner: usize) -> CardLine {
    let rail = section_rail(name, role, inner);
    let mut fragments = vec![span("├", Role::Border)];
    fragments.extend(rail.fragments);
    fragments.push(span("┤", Role::Border));
    row(role, fragments)
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
fn frame_line(line: CardLine, inner: usize) -> CardLine {
    let mut fragments = vec![span("│", Role::Border)];
    let mut used = 0;
    for fragment in line.fragments {
        if used >= inner {
            break;
        }
        let text = fit_middle(&fragment.text, inner - used);
        used += visible_width(&text);
        fragments.push(CardSpan { text, ..fragment });
    }
    if used < inner {
        fragments.push(span(" ".repeat(inner - used), Role::Text));
    }
    fragments.push(span("│", Role::Border));
    row(line.role, fragments)
}
fn fit_line(line: CardLine, width: usize) -> CardLine {
    let mut fragments = Vec::new();
    let mut used = 0;
    for fragment in line.fragments {
        if used >= width {
            break;
        }
        let text = fit_middle(&fragment.text, width - used);
        used += visible_width(&text);
        fragments.push(CardSpan { text, ..fragment });
    }
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
fn progress_cells(percent: f64, cells: usize) -> usize {
    ((percent.clamp(0.0, 100.0) / 100.0) * cells as f64).round() as usize
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
fn disk_percent(used: u64, total: u64) -> String {
    if total == 0 {
        "unknown".into()
    } else {
        format_percent(used as f64 / total as f64 * 100.0)
    }
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
    #[test]
    fn normal_is_a_compact_identity_rail() {
        let body = card_body(&fixture(true), false);
        assert!((6..=9).contains(&body.identity.len()));
        assert!(body.sections.is_empty());
        let text = render_text(&fixture(true), options(false, 104));
        for label in [
            "workspace:",
            "git:",
            "runtime:",
            "health:",
            "uptime:",
            "time:",
            "day:",
            "project:",
        ] {
            assert!(text.contains(label));
        }
    }
    #[test]
    fn sparse_non_git_omits_unavailable_rows() {
        let text = render_text(&fixture(false), options(false, 40));
        assert!(!text.contains("git:"));
        assert!(!text.contains("—"));
        assert!(!text.contains("coverage:"));
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
        assert_eq!(text.matches("coverage source:").count(), 1);
        assert_eq!(text.matches("coverage freshness:").count(), 1);
    }

    #[test]
    fn full_keeps_ram_capacity_in_the_identity_rail_only() {
        let output = render_text(&fixture(true), options(true, 120));
        assert_eq!(output.matches("8.0 GiB").count(), 1);
        assert_eq!(output.matches("16.0 GiB").count(), 1);
        assert!(!output.contains("memory: used"));
    }

    #[test]
    fn python_runtime_strips_a_case_insensitive_language_version_prefix() {
        let mut snapshot = fixture(true);
        {
            let project = snapshot.project.as_mut().unwrap();
            project.primary_language = Some("Python".into());
            project.toolchains[1].version = Some("Python 3.14.6".into());
        }
        let output = render_text(&snapshot, options(false, 80));
        assert!(output.contains("runtime: python3 3.14.6"));
        assert!(!output.contains("python3 Python"));
        assert_eq!(
            toolchain_version(&snapshot.project.as_ref().unwrap().toolchains[0]),
            "1.98.1"
        );
    }

    #[test]
    fn coverage_report_modified_is_explicit_unix_provenance_not_an_age() {
        let output = render_text(&fixture(true), options(true, 120));
        assert!(output.contains("report modified: Unix 1700000000"));
        assert!(!output.contains("report modified: 1700000000 seconds"));
        assert!(!output.contains("report age"));
    }
    #[test]
    fn fragments_are_authoritative_and_gradients_only_fill_progress() {
        let card = render_card(&fixture(true), options(false, 104));
        let spans: Vec<_> = card.iter().flat_map(card_spans).collect();
        assert!(
            spans
                .iter()
                .any(|span| span.text == "git: " && span.role == Role::Secondary)
        );
        assert!(
            spans
                .iter()
                .filter(|span| span.style == SpanStyle::Gradient)
                .all(|span| span.role == Role::Progress
                    && span.text.chars().all(|character| character == '█'))
        );
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
        for width in 1..20 {
            let output = render_text(&fixture(true), options(true, width));
            assert!(output.lines().all(|line| visible_width(line) <= width));
            assert!(!output.contains('╭'));
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
        for width in [1, 2, 19, 20, 21, 40, 80, 104, 120, 160] {
            let output = render_text(&fixture(true), options(true, width));
            assert!(output.lines().all(|line| visible_width(line) <= width));
            if width < 20 {
                assert!(!output.contains('╭'));
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
                    .all(|c| matches!(c, '╭' | '╮' | '╰' | '╯' | '│' | '─')))
                .all(|span| span.role == Role::Border)
        );
        assert!(
            spans
                .iter()
                .any(|span| span.text.contains("main") && span.role == Role::Git)
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
                .is_some_and(|line| line.starts_with('╭') && line.ends_with('╮'))
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
                    || (full && line.starts_with('├') && line.ends_with('┤'))
            }));
            if full {
                for section in [
                    "[ system & health ]",
                    "[ active context ]",
                    "[ project telemetry ]",
                ] {
                    assert!(output.contains(section));
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
        assert!(render_text(&clean, options(false, 104)).contains("clean"));
        clean.git.as_mut().unwrap().dirty = true;
        assert!(render_text(&clean, options(false, 104)).contains("dirty"));
        clean.git.as_mut().unwrap().conflicts = 1;
        assert!(render_text(&clean, options(false, 104)).contains("conflict"));
    }

    #[test]
    fn combined_health_row_keeps_cpu_ram_and_disk_value_roles() {
        let health = card_body(&fixture(true), false)
            .identity
            .into_iter()
            .find(|line| line.text.starts_with("health:"))
            .unwrap();
        assert!(
            health
                .fragments
                .iter()
                .any(|span| span.text == "12.5%" && span.role == Role::System(SystemRole::Cpu))
        );
        assert!(
            health
                .fragments
                .iter()
                .any(|span| span.text.contains("8.0 GiB")
                    && span.role == Role::System(SystemRole::Memory))
        );
        assert!(
            health
                .fragments
                .iter()
                .any(|span| span.text.contains("40.0%")
                    && span.role == Role::System(SystemRole::Disk))
        );
        assert!(
            health
                .fragments
                .iter()
                .filter(|span| span.text == " · ")
                .all(|span| span.role == Role::Secondary)
        );
    }

    #[test]
    fn headers_are_solid_and_metric_fills_target_their_roles() {
        let spans: Vec<_> = render_card(&fixture(true), options(true, 120))
            .iter()
            .flat_map(card_spans)
            .collect();
        for header in [
            "[ system & health ]",
            "[ active context ]",
            "[ project telemetry ]",
        ] {
            assert!(
                spans
                    .iter()
                    .any(|span| span.text == header && span.style == SpanStyle::Solid)
            );
        }
        assert!(
            spans
                .iter()
                .filter(|span| span.style == SpanStyle::Gradient)
                .all(|span| span.text.chars().all(|character| character == '█')
                    && matches!(span.role, Role::System(_) | Role::Time | Role::Coverage))
        );
    }

    #[test]
    fn time_is_lavender_and_plain_output_is_control_free() {
        let plain = render_text(&fixture(true), options(false, 104));
        assert!(!plain.contains('\x1b'));
        let colors = palette(Theme::DuskDarker);
        let time = card_body(&fixture(true), false)
            .identity
            .into_iter()
            .find(|line| line.text.starts_with("time:"))
            .unwrap();
        assert!(
            expanded_card_spans(&time, colors)
                .iter()
                .any(|chunk| chunk.text.contains("12:34:56") && chunk.color == colors.lavender)
        );
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
        assert!(full.contains("DIAG: coverage-stale"));
        assert!(
            full.lines()
                .filter(|line| line.contains("DIAG:"))
                .all(|line| line.starts_with('│') && line.ends_with('│'))
        );
        let sparse = render_text(&Snapshot::empty(SnapshotMode::OneShot), options(true, 80));
        assert!(!sparse.contains("coverage"));
        assert!(!sparse.contains("unknown"));
    }

    #[test]
    fn full_owns_compact_identity_and_explicit_wide_columns() {
        let body = card_body(&fixture(true), true);
        assert_eq!(body.identity.len(), 3);
        assert_eq!(body.identity_layout_height, 6);
        assert!(
            body.identity
                .iter()
                .all(|line| !line.text.starts_with("health:"))
        );
        let system = &body.sections[0];
        assert!(system.left.iter().any(|line| line.text.starts_with("CPU:")));
        assert!(
            system
                .right
                .iter()
                .any(|line| line.text.starts_with("RAM:"))
        );
        let wide = section_rows(system, 118, true);
        assert!(
            wide[..system.left.len().min(system.right.len())]
                .iter()
                .all(|line| line.text.contains("   "))
        );
        assert!(wide.iter().all(|line| !line.text.contains('│')));
        let narrow = section_rows(system, 38, false);
        assert_eq!(narrow.len(), system.left.len() + system.right.len() + 1);
        assert!(
            narrow[..system.left.len()]
                .iter()
                .all(|line| !line.text.contains('│'))
        );
        let overflow = CardSection {
            name: "overflow",
            role: Role::Text,
            left: vec![labelled("left", "one", Role::Text)],
            right: vec![
                labelled("right", "one", Role::Text),
                labelled("right", "two", Role::Text),
            ],
        };
        let wide = section_rows(&overflow, 80, true);
        assert_eq!(wide[1].text, "right: two");
        assert!(wide.last().is_some_and(|line| line.text.is_empty()));
    }

    #[test]
    fn full_bars_precede_details_and_clean_git_keeps_zero_counts() {
        let mut snapshot = fixture(true);
        let git = snapshot.git.as_mut().unwrap();
        git.dirty = false;
        git.staged = 0;
        git.unstaged = 0;
        git.conflicts = 0;
        let body = card_body(&snapshot, true);
        let cpu = &body.sections[0].left[0];
        assert!(cpu.text.find('[').unwrap() < cpu.text.find("12.5%").unwrap());
        assert!(cpu.fragments.iter().any(|span| {
            span.style == SpanStyle::Gradient && span.role == Role::System(SystemRole::Cpu)
        }));
        let active = &body.sections[1];
        assert!(
            active
                .right
                .iter()
                .any(|line| { line.text == "git changes: staged 0 · unstaged 0 · conflicts 0" })
        );
    }

    #[test]
    fn full_section_and_coverage_roles_are_semantic() {
        let mut snapshot = fixture(true);
        let body = card_body(&snapshot, true);
        assert_eq!(body.sections[1].role, Role::Context);
        assert!(
            body.sections[1]
                .left
                .iter()
                .any(|line| line.role == Role::Time)
        );
        assert!(
            body.sections[1]
                .right
                .iter()
                .any(|line| line.role == Role::Git)
        );
        assert_eq!(body.sections[2].role, Role::Primary);
        assert_eq!(body.sections[2].left[0].role, Role::Primary);
        let coverage = body.sections[2]
            .right
            .iter()
            .find(|line| line.text.starts_with("coverage source:"))
            .unwrap();
        assert_eq!(coverage.fragments[0].role, Role::Secondary);
        assert_eq!(coverage.fragments[1].role, Role::Text);

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
            let coverage = body.sections[2]
                .right
                .iter()
                .find(|line| line.text.starts_with("line coverage:"))
                .unwrap();
            assert_eq!(coverage.role, role);
            assert!(
                coverage
                    .fragments
                    .iter()
                    .any(|span| { span.style == SpanStyle::Gradient && span.role == role })
            );
        }
    }

    #[test]
    fn full_uses_restrained_metric_spans_and_normal_roles_stay_frozen() {
        let full = card_body(&fixture(true), true);
        let system = &full.sections[0];
        let cpu = &system.left[0];
        assert!(cpu.fragments.iter().any(|span| {
            span.text.contains("3200 MHz") && span.role == Role::System(SystemRole::Cpu)
        }));
        assert!(
            cpu.fragments
                .iter()
                .any(|span| span.text == " · 12.5%" && span.role == Role::Text)
        );
        for line in [&system.left[1], &system.right[0]] {
            assert_eq!(line.fragments[0].role, Role::Secondary);
            assert_eq!(line.fragments[1].role, Role::Secondary);
            assert_eq!(line.fragments[3].role, Role::Secondary);
            assert!(
                line.fragments
                    .iter()
                    .any(|span| span.style == SpanStyle::Gradient)
            );
            assert!(line.fragments.iter().any(|span| span.role == Role::Text));
        }

        let active = &full.sections[1];
        let time = &active.left[0];
        assert!(
            time.fragments
                .iter()
                .any(|span| span.text == "12:34:56" && span.role == Role::Time)
        );
        assert!(
            time.fragments
                .iter()
                .any(|span| { span.text == "2026-09-16" && span.role == Role::Text })
        );
        let changes = active
            .right
            .iter()
            .find(|line| line.text.starts_with("git changes:"))
            .unwrap();
        assert!(
            changes
                .fragments
                .iter()
                .any(|span| span.text == "2" && span.role == Role::Git)
        );
        assert!(
            changes
                .fragments
                .iter()
                .filter(|span| span.text != "2" && span.text != "3" && span.text != "1")
                .all(|span| span.role == Role::Secondary)
        );

        let project = &full.sections[2];
        assert_eq!(project.left[0].fragments[0].role, Role::Secondary);
        assert_eq!(project.left[0].fragments[1].role, Role::Primary);
        assert_eq!(
            project.left[1].fragments[1].role,
            Role::Runtime(RuntimeRole::Rust)
        );
        assert_eq!(project.left[1].fragments[3].role, Role::Secondary);
        let source = project
            .right
            .iter()
            .find(|line| line.text.starts_with("coverage source:"))
            .unwrap();
        assert_eq!(source.fragments[0].role, Role::Secondary);
        assert_eq!(source.fragments[1].role, Role::Text);

        let normal = card_body(&fixture(true), false);
        let normal_time = normal
            .identity
            .iter()
            .find(|line| line.text.starts_with("time:"))
            .unwrap();
        assert!(
            normal_time
                .fragments
                .iter()
                .all(|span| span.role == Role::Secondary || span.role == Role::Time)
        );
        assert!(
            normal_time
                .fragments
                .iter()
                .any(|span| span.text.contains("2026-09-16") && span.role == Role::Time)
        );
    }
}
