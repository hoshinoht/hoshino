# Hoshino project guidance

This repository is a standalone Cargo project. Keep its builds, installation,
and generated artifacts local to this project; it is not a Stow package and it
does not own shared repository tasks. Preserve these rules if the repository is
represented by a Git submodule.

<!-- recall:lessons:begin -->
## Rendering and safety contract

- Treat Normal as a compact 6–9-row instrument rail: workspace and Git with the
  runtime right-aligned, OS/CPU with uptime, memory and disk meters, and a
  24-hour day ruler with hour labels beneath it. Full is deliberately distinct:
  a compact workspace/Git/runtime header followed by `system & health` (a
  metric table of memory and disks), `active context` (time, the day ruler, Git
  counts), and `project telemetry` (a stacked language bar with per-language
  legend rows, coverage meters, and provenance) bands. Tables keep explicit
  columns at wide widths; narrow widths drop the table header and stack each
  row's labeled facts. Live must present the same mode-specific hierarchy. Omit
  unavailable and redundant facts (including volumes that print identical
  usage) rather than printing schema-shaped runs of placeholders or spacer rows.
- Width-dependent elements (meters, the ruler and its axis, the language bar)
  stay symbolic in `CardLine` and are resolved only by `text::layout_line`,
  which every surface (plain text, one-shot, live) must use. Narrow widths drop
  right-aligned support first, then the instrument, and only then shorten text.
- Keep the logo and every semantic row inside one outer frame. One-shot owns
  one frame; live reuses its existing Ratatui `Block` and must not add a nested
  frame. Total width is capped at 104 columns for Normal and 120 for Full.
  Content keeps one cell of padding inside the frame and after the image
  divider. Band rails are solid pink `├─ name ───┤` without brackets.
  Image, divider, and gutter form one responsive region and disappear together
  when geometry is insufficient; text-only output has no empty image gutter.
- Preserve Dusk's semantic roles: solid pink for the frame, identity, and
  structural dividers; sky for Git; tool-specific runtime accents; blue for
  CPU and for the day ruler's elapsed cells and marker; green for RAM and
  uptime; peach for disk; lavender for Time; muted overlays for labels,
  separators, and instrument tracks; yellow `▲ full` for a disk at 95% or more;
  and red for failures. Labels and state words must retain meaning without
  color. Labels stay overlay while each value takes its role color as a whole
  (the full time string, runtime with build provenance, memory with capacity);
  nothing is bold. Only filled meter and ruler cells may use a gradient.
- Reference material may inform hierarchy, density, and composition, but never
  copy its mascot, icons, palette dots, exact layout, source, tests, or assets.
- Keep one-shot output query-free and append-safe. Ratatui's inline viewport is
  forbidden. The only one-shot graphics path is bounded Kitty direct transfer
  plus Unicode-placeholder rows: no cursor movement or save/restore, raw mode,
  alternate screen, stdin reads, or immediate deletion. Live alone owns
  terminal state, animation, and targeted cleanup. Live motion is currently
  limited to the refresh spinner; ticking clocks or eased bars need approval.
- Keep arbitrary project, coverage, and image reads behind the shared atomic
  regular-file opener. Metadata checks followed by plain `File::open` are not
  safe against FIFO or symlink swaps; Unix implicit reads require nonblocking,
  close-on-exec, no-follow opens followed by descriptor validation.
- The bundled logo and its separate rights terms are not covered by the MIT
  software license. Do not change logo bytes, source precedence, attribution,
  or rights language as part of renderer work.
<!-- recall:lessons:end -->
