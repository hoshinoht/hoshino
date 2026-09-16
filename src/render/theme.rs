use crate::config::Theme;

/// Semantic roles used by the one-shot card.  A role describes why a value is
/// highlighted, not where it happens to be placed in the card.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Role {
    Border,
    Primary,
    Git,
    Time,
    Context,
    Coverage,
    Progress,
    Runtime(RuntimeRole),
    System(SystemRole),
    Secondary,
    Warning,
    Error,
    Text,
}

/// Presentation treatment for a semantic card span.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SpanStyle {
    Solid,
    Gradient,
}

/// System-health accents are semantic data roles, distinct from runtimes.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SystemRole {
    Health,
    Cpu,
    Memory,
    Disk,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RuntimeRole {
    Rust,
    Bun,
    Python,
    Node,
    Go,
    Other,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Rgb {
    pub red: u8,
    pub green: u8,
    pub blue: u8,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ExpandedChunk {
    pub text: String,
    pub color: Rgb,
}

impl Rgb {
    pub const fn new(red: u8, green: u8, blue: u8) -> Self {
        Self { red, green, blue }
    }

    pub const fn hex(self) -> u32 {
        ((self.red as u32) << 16) | ((self.green as u32) << 8) | self.blue as u32
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Palette {
    pub theme: Theme,
    pub pink: Rgb,
    pub red: Rgb,
    pub peach: Rgb,
    pub yellow: Rgb,
    pub green: Rgb,
    pub sky: Rgb,
    pub blue: Rgb,
    pub lavender: Rgb,
    pub text: Rgb,
    pub overlay: Rgb,
    pub surface: Rgb,
}

impl Palette {
    pub const fn for_theme(theme: Theme) -> Self {
        match theme {
            Theme::Dusk => Self {
                theme,
                pink: Rgb::new(0xF3, 0xBD, 0xCA),
                red: Rgb::new(0xE2, 0x78, 0x78),
                peach: Rgb::new(0xDD, 0xA0, 0x5C),
                yellow: Rgb::new(0xDE, 0xC4, 0x7C),
                green: Rgb::new(0x82, 0xC8, 0xA0),
                sky: Rgb::new(0x75, 0xD6, 0xF6),
                blue: Rgb::new(0x68, 0xAC, 0xE0),
                lavender: Rgb::new(0xB0, 0xBC, 0xE8),
                text: Rgb::new(0xF3, 0xF5, 0xFC),
                overlay: Rgb::new(0x7E, 0x82, 0x8F),
                surface: Rgb::new(0x2F, 0x32, 0x38),
            },
            Theme::DuskDarker => Self {
                theme,
                pink: Rgb::new(0xFF, 0xB8, 0xD1),
                red: Rgb::new(0xFF, 0x8F, 0x9A),
                peach: Rgb::new(0xDD, 0xA0, 0x5C),
                yellow: Rgb::new(0xF4, 0xDA, 0x86),
                green: Rgb::new(0x9B, 0xE6, 0xB5),
                sky: Rgb::new(0x8B, 0xD3, 0xFF),
                blue: Rgb::new(0x7B, 0xC1, 0xF2),
                lavender: Rgb::new(0xB0, 0xBC, 0xE8),
                text: Rgb::new(0xFF, 0xFF, 0xFF),
                overlay: Rgb::new(0x85, 0x8B, 0x9C),
                surface: Rgb::new(0x20, 0x24, 0x2E),
            },
        }
    }

    pub const fn color(self, role: Role) -> Rgb {
        match role {
            Role::Border => self.pink,
            Role::Primary => self.pink,
            Role::Git => self.sky,
            Role::Time => self.lavender,
            Role::Context => self.peach,
            Role::Coverage => self.green,
            Role::Progress => self.blue,
            Role::Runtime(runtime) => match runtime {
                RuntimeRole::Rust | RuntimeRole::Bun => self.peach,
                RuntimeRole::Python => self.yellow,
                RuntimeRole::Node => self.green,
                RuntimeRole::Go => self.blue,
                RuntimeRole::Other => self.overlay,
            },
            Role::System(system) => match system {
                SystemRole::Health | SystemRole::Memory => self.green,
                SystemRole::Cpu => self.blue,
                SystemRole::Disk => self.peach,
            },
            Role::Secondary => self.overlay,
            Role::Warning => self.yellow,
            Role::Error => self.red,
            Role::Text => self.text,
        }
    }

    pub fn ansi_foreground(self, role: Role) -> String {
        self.ansi_foreground_rgb(self.color(role))
    }

    pub fn ansi_foreground_rgb(self, color: Rgb) -> String {
        format!("\x1b[38;2;{};{};{}m", color.red, color.green, color.blue)
    }
}

const GRADIENT_START_PERCENT: usize = 35;

/// Expand one semantic span into deterministic text/color chunks shared by
/// ANSI and Ratatui serializers. Gradient spans begin at a 35% integer blend
/// from the palette surface and finish at the exact semantic target.
pub fn expand_span(
    text: &str,
    role: Role,
    style: SpanStyle,
    colors: Palette,
) -> Vec<ExpandedChunk> {
    if text.is_empty() {
        return Vec::new();
    }
    let target = colors.color(role);
    if style == SpanStyle::Solid {
        return vec![ExpandedChunk {
            text: text.to_owned(),
            color: target,
        }];
    }

    let glyphs = text.chars().count();
    let chunks = glyphs.min(5);
    if chunks == 1 {
        // Preserve one-glyph geometry rather than emitting an empty shade chunk.
        return vec![ExpandedChunk {
            text: text.to_owned(),
            color: target,
        }];
    }

    let start = blend(colors.surface, target, GRADIENT_START_PERCENT, 100);
    let base_size = glyphs / chunks;
    let extra = glyphs % chunks;
    let characters = text.chars().collect::<Vec<_>>();
    let mut offset = 0;
    (0..chunks)
        .map(|index| {
            let size = base_size + usize::from(index < extra);
            let end = offset + size;
            let chunk_text = characters[offset..end].iter().collect::<String>();
            let color = if index + 1 == chunks {
                target
            } else {
                blend(start, target, index, chunks - 1)
            };
            offset = end;
            ExpandedChunk {
                text: chunk_text,
                color,
            }
        })
        .collect()
}

fn blend(start: Rgb, end: Rgb, numerator: usize, denominator: usize) -> Rgb {
    Rgb::new(
        blend_channel(start.red, end.red, numerator, denominator),
        blend_channel(start.green, end.green, numerator, denominator),
        blend_channel(start.blue, end.blue, numerator, denominator),
    )
}

fn blend_channel(start: u8, end: u8, numerator: usize, denominator: usize) -> u8 {
    let denominator = u32::try_from(denominator).expect("gradient denominator fits u32");
    let numerator = u32::try_from(numerator).expect("gradient numerator fits u32");
    let remaining = denominator
        .checked_sub(numerator)
        .expect("gradient numerator is bounded by denominator");
    let weighted_start = u32::from(start)
        .checked_mul(remaining)
        .expect("gradient channel interpolation cannot overflow");
    let weighted_end = u32::from(end)
        .checked_mul(numerator)
        .expect("gradient channel interpolation cannot overflow");
    let weighted = weighted_start
        .checked_add(weighted_end)
        .expect("gradient channel interpolation cannot overflow");
    u8::try_from(
        weighted
            .checked_div(denominator)
            .expect("gradient denominator is nonzero"),
    )
    .expect("gradient channel remains within u8")
}

pub const fn palette(theme: Theme) -> Palette {
    Palette::for_theme(theme)
}

/// Pick the semantic runtime accent for a model toolchain row.
pub fn runtime_role(language: &str, runtime: &str) -> RuntimeRole {
    let runtime = runtime.to_ascii_lowercase();
    let language = language.to_ascii_lowercase();
    if runtime.contains("bun") {
        RuntimeRole::Bun
    } else if runtime.contains("node") {
        RuntimeRole::Node
    } else if runtime.contains("python") || language == "python" {
        RuntimeRole::Python
    } else if runtime == "go" || runtime.starts_with("go ") || language == "go" {
        RuntimeRole::Go
    } else if runtime.contains("rust") || runtime == "cargo" || language == "rust" {
        RuntimeRole::Rust
    } else {
        RuntimeRole::Other
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn palettes_keep_the_repository_hex_values() {
        let dusk = palette(Theme::Dusk);
        assert_eq!(dusk.pink.hex(), 0xF3BDCA);
        assert_eq!(dusk.sky.hex(), 0x75D6F6);
        assert_eq!(dusk.lavender.hex(), 0xB0BCE8);
        assert_eq!(dusk.red.hex(), 0xE27878);

        let darker = palette(Theme::DuskDarker);
        assert_eq!(darker.pink.hex(), 0xFFB8D1);
        assert_eq!(darker.sky.hex(), 0x8BD3FF);
        assert_eq!(darker.lavender.hex(), 0xB0BCE8);
        assert_eq!(darker.red.hex(), 0xFF8F9A);
    }

    #[test]
    fn role_mapping_preserves_dusk_semantics() {
        let colors = palette(Theme::DuskDarker);
        assert_eq!(colors.color(Role::Primary), colors.pink);
        assert_eq!(colors.color(Role::Border), colors.pink);
        assert_eq!(colors.color(Role::Git), colors.sky);
        assert_eq!(colors.color(Role::Time), colors.lavender);
        assert_eq!(colors.color(Role::Context), colors.peach);
        assert_eq!(colors.color(Role::Coverage), colors.green);
        assert_eq!(colors.color(Role::Progress), colors.blue);
        assert_eq!(colors.color(Role::Runtime(RuntimeRole::Rust)), colors.peach);
        assert_eq!(colors.color(Role::Runtime(RuntimeRole::Bun)), colors.peach);
        assert_eq!(
            colors.color(Role::Runtime(RuntimeRole::Python)),
            colors.yellow
        );
        assert_eq!(colors.color(Role::Runtime(RuntimeRole::Node)), colors.green);
        assert_eq!(colors.color(Role::Runtime(RuntimeRole::Go)), colors.blue);
        assert_eq!(colors.color(Role::System(SystemRole::Health)), colors.green);
        assert_eq!(colors.color(Role::System(SystemRole::Cpu)), colors.blue);
        assert_eq!(colors.color(Role::System(SystemRole::Memory)), colors.green);
        assert_eq!(colors.color(Role::System(SystemRole::Disk)), colors.peach);
        assert_eq!(colors.color(Role::Secondary), colors.overlay);
        assert_eq!(colors.color(Role::Warning), colors.yellow);
        assert_eq!(colors.color(Role::Error), colors.red);
    }

    #[test]
    fn structural_and_time_roles_are_explicit_in_both_palettes() {
        for theme in [Theme::Dusk, Theme::DuskDarker] {
            let colors = palette(theme);
            assert_eq!(colors.color(Role::Border), colors.pink);
            assert_eq!(colors.color(Role::Time), colors.lavender);
            assert_eq!(colors.color(Role::Context), colors.peach);
            assert_eq!(colors.color(Role::Coverage), colors.green);
        }
    }

    #[test]
    fn gradient_start_uses_checked_integer_surface_blend() {
        let colors = palette(Theme::DuskDarker);
        assert_eq!(
            blend(colors.surface, colors.blue, 35, 100),
            Rgb::new(63, 90, 114)
        );
    }

    #[test]
    fn gradient_expansion_preserves_text_and_bounded_endpoints() {
        let colors = palette(Theme::DuskDarker);
        let target = colors.color(Role::Git);
        for length in [1, 2, 3, 10] {
            let text = "x".repeat(length);
            let chunks = expand_span(&text, Role::Git, SpanStyle::Gradient, colors);
            assert!(chunks.len() <= 5);
            assert_eq!(
                chunks
                    .iter()
                    .map(|chunk| chunk.text.as_str())
                    .collect::<String>(),
                text
            );
            assert_eq!(chunks.last().map(|chunk| chunk.color), Some(target));
            if length > 1 {
                assert_eq!(
                    chunks.first().map(|chunk| chunk.color),
                    Some(blend(colors.surface, target, 35, 100))
                );
            }
        }
    }

    #[test]
    fn runtime_detection_is_not_position_dependent() {
        assert_eq!(runtime_role("TypeScript", "bun"), RuntimeRole::Bun);
        assert_eq!(runtime_role("JavaScript", "node"), RuntimeRole::Node);
        assert_eq!(runtime_role("Rust", "rustc"), RuntimeRole::Rust);
        assert_eq!(runtime_role("Python", "python3"), RuntimeRole::Python);
        assert_eq!(runtime_role("Go", "go"), RuntimeRole::Go);
    }
}
