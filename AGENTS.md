# Hoshino project guidance

This repository is a standalone Cargo project. Keep its builds, installation,
and generated artifacts local to this project; it is not a Stow package and it
does not own shared repository tasks. Preserve these rules if the repository is
represented by a Git submodule.

<!-- recall:lessons:begin -->
## Rendering and safety contract

- Treat Normal as a compact 6–9-row identity/status rail. Full is deliberately
  distinct: a compact workspace/Git/runtime header followed by responsive
  `system & health`, `active context`, and `project telemetry` section bands
  with explicit semantic columns at wide widths and stacked labeled rows when
  narrow. Live must present the same mode-specific hierarchy. Omit unavailable
  and redundant facts rather than printing schema-shaped runs of placeholders.
- Keep the logo and every semantic row inside one outer frame. One-shot owns
  one frame; live reuses its existing Ratatui `Block` and must not add a nested
  frame. Total width is capped at 104 columns for Normal and 120 for Full.
  Image, divider, and gutter form one responsive region and disappear together
  when geometry is insufficient; text-only output has no empty image gutter.
- Preserve Dusk's semantic roles: solid pink for the frame, identity, and
  structural dividers; sky for Git; tool-specific runtime accents; blue for
  CPU; green for RAM; peach for disk; lavender for Time; muted overlays for
  secondary labels; and red for failures. Labels and state words must retain
  meaning without color. Apply accents to key values and states rather than
  long supporting strings; capacity, dates, provenance, punctuation, and other
  support stay neutral. Only filled progress-bar cells may use a gradient.
- Reference material may inform hierarchy, density, and composition, but never
  copy its mascot, icons, palette dots, exact layout, source, tests, or assets.
- Keep one-shot output query-free and append-safe. Ratatui's inline viewport is
  forbidden. The only one-shot graphics path is bounded Kitty direct transfer
  plus Unicode-placeholder rows: no cursor movement or save/restore, raw mode,
  alternate screen, stdin reads, or immediate deletion. Live alone owns
  terminal state, animation, and targeted cleanup.
- Keep arbitrary project, coverage, and image reads behind the shared atomic
  regular-file opener. Metadata checks followed by plain `File::open` are not
  safe against FIFO or symlink swaps; Unix implicit reads require nonblocking,
  close-on-exec, no-follow opens followed by descriptor validation.
- The bundled logo and its separate rights terms are not covered by the MIT
  software license. Do not change logo bytes, source precedence, attribution,
  or rights language as part of renderer work.
<!-- recall:lessons:end -->
