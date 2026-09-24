# hoshino

`hoshino` is a standalone Rust terminal card for the current directory. It
collects project/Git facts when the directory is in a Git worktree, and always
keeps the machine and local-time portion useful. It is deliberately project
local: installation uses Cargo, does not add this directory to GNU Stow or a
global task registry, and never edits shell files or creates a config file on
its own.

Hoshino bundles `assets/logo.png` as its default image source. A user-supplied
path can override it, and `--no-image` disables image output. The bundled
artwork is expressly excluded from the MIT License; see
[`assets/NOTICE.md`](assets/NOTICE.md) for its separate rights notice.

## Quick start

From the repository root, install with Cargo:

```bash
cargo install --path . --locked
```

Then run a one-shot card:

```bash
hoshino
hoshino --full
hoshino --json
```

The supported/tested Rust version is **1.98.1**, owned by rustup. Rust remains
separate from the repository's other runtime managers. If a Homebrew `rustc`
on `PATH` is broken or shadows the rustup compiler, use the rustup-owned tools
explicitly:

```bash
RUSTC="$(rustup which rustc)" RUSTDOC="$(rustup which rustdoc)" \
  rustup run stable cargo install --path . --locked
```

Check the compiler before installing when needed:

```bash
rustup run stable rustc --version
```

The local uninstall is:

```bash
cargo uninstall hoshino
```

Cargo's normal install prefix owns the installed binary. No global config is
written by installation or display.

## Commands and modes

The public command shape is:

```text
hoshino [--full] [--json] [--config PATH] [--color auto|always|never] [--image PATH|--no-image] [--protocol auto|kitty|iterm2|sixel|halfblocks|none]
hoshino live [--full]
hoshino hook <bash|zsh|fish|nu|powershell>
```

The display options are global, so they may also be placed with `live` as
shown by `hoshino live --help`. `--json` is for one-shot output;
`hoshino live --json` is rejected. Hook generation takes only the shell name and rejects
display options. The hidden internal hook-context switch is not a public
interface.

Useful discovery commands:

```bash
hoshino --help
hoshino --version
hoshino live --help
hoshino hook --help
```

### One-shot output

`hoshino` takes one bounded snapshot and exits. It does not start a daemon,
background monitor, or project test command.

In a Git worktree, the concise card includes:

- directory and operating-system identity;
- primary project language and its detected runtime version;
- total lines of code and aggregate line coverage;
- Git head, clean/dirty state, staged, unstaged, conflict, and ahead facts;
- CPU, memory, disks, uptime, clock, date, and day progress.

Normal output presents those available facts as one compact 6–9-row instrument
rail inside a single frame: workspace and Git with the runtime right-aligned,
OS and CPU with uptime, memory and disk meters, and a 24-hour day ruler with
hour labels beneath it. Its total width is capped at 104 columns, including the
frame, optional inset image, divider, and text; meters and the ruler shrink with
the available width, and right-aligned support is dropped first when space runs
out.

Outside a Git worktree, project and Git sections are omitted entirely. The
card is still a system-and-time readout. A user-invoked non-Git `hoshino`
therefore remains useful, while the shell hook is silent there.

`--full` switches to a compact workspace/Git/runtime header followed by
nonempty `system & health`, `active context`, and `project telemetry` bands
inside the same frame. System & health is a metric table (memory and one row
per disk, with volumes that print identical usage shown once and `▲ full` at
95% or more); active context carries the timezone, the 24-hour day ruler, and
Git counts; project telemetry shows a stacked language bar with one legend row
per language, coverage meters, and coverage format/path/source/mtime and
freshness. At wide widths the table keeps explicit columns; narrow terminals
drop its header and stack each row's labeled facts. Diagnostics carry their
codes, and unavailable fields are omitted rather than repeated. It is capped at
120 total columns and can take longer than the concise path.

`--json` emits one versioned JSON document (`schema_version: 1`) for one-shot
automation. Units are explicit in the model, missing metrics are `null` or
omitted according to the field, and collection problems are structured in
`diagnostics`. JSON is not a terminal-rendering format and contains no ANSI
or graphics control sequences; it does not decode an image.

When stdout is not a TTY, hoshino always uses compact plain text unless
`--json` was selected. It does not inspect stdin, probe terminal capabilities,
read terminal geometry, emit ANSI escapes, emit image/graphics bytes, or decode
an image on a pipe. This remains true with `--color always` and an image
configured. On a text TTY, `--color auto|always|never` controls color;
`NO_COLOR` disables `auto`, while `always` and `never` are explicit choices.

Unavailable optional metrics are omitted or compactly marked once rather than
fabricated as zero or expanded into placeholder rows. A failed optional
collector is reported as a diagnostic and does not turn an otherwise usable
card into a fake precise result.

### Live output

Live mode is explicit and interactive:

```bash
hoshino live
hoshino live --full
```

It requires both stdin and stdout to be TTYs. It refreshes on the configured
cadence, responds to terminal resize, and exits with `q`, `Esc`, or `Ctrl-C`.
Only live mode takes raw/alternate-screen ownership; hoshino restores the
cursor, alternate screen, and raw mode on normal exit and handled errors. Live
retains the selected terminal protocols and animation behavior, and uses the
bundled logo by default. It uses the same 104/120-column hierarchy inside one
left-compact Ratatui frame. On short terminals it drops a top image before
content; if the semantic rows still do not fit, it shows an explicit
`resize for details` fallback (and `full details hidden` in Full mode) instead
of silently clipping rows.

## Shell hooks

Hooks are opt-in source text. Generating or sourcing one does not edit a shell
startup file automatically. The generated code is synchronous, starts no
background process, does not parse Git in shell code, and preserves the
existing prompt handlers. It has an install guard and remembers the last
directory, so repeated prompt events in one directory do not print duplicate
cards. The application decides whether the current directory is a worktree;
the generated hook emits one compact text card there and no card outside Git.
Hook generation emits source text only. Its internal hook-context invocation is
also text-only: it emits no graphics and does not decode the bundled logo.

The command must be on `PATH` in the shell that sources the snippet.

### Bash

```bash
source <(hoshino hook bash)
```

The Bash snippet appends to either the scalar or array form of
`PROMPT_COMMAND` without replacing earlier entries.

### Zsh

```zsh
source <(hoshino hook zsh)
```

The Zsh snippet uses `add-zsh-hook` for `precmd` and `chpwd`, leaving existing
handlers registered.

### Fish

```fish
hoshino hook fish | source
```

The Fish snippet registers named handlers for `fish_prompt` and the `PWD`
variable. It does not replace the `fish_prompt` function.

### Nushell

Nushell sources a file path. Use a disposable path (not a startup file), then
remove it after sourcing:

```nu
hoshino hook nu | save --force /path/to/temporary/hoshino-hook.nu
source /path/to/temporary/hoshino-hook.nu
rm /path/to/temporary/hoshino-hook.nu
```

The generated Nushell code appends to both `hooks.pre_prompt` and the `PWD`
entry in `hooks.env_change`, preserving an existing function or list in each
slot.

### PowerShell

Dot-source a disposable script rather than changing a profile file:

```powershell
$hook = Join-Path $env:TEMP 'hoshino-hook.ps1'
hoshino hook powershell | Set-Content -LiteralPath $hook
. $hook
Remove-Item -LiteralPath $hook
```

The PowerShell snippet saves the existing `prompt` script block, runs the
worktree-aware hoshino emit step, and returns the prior prompt result.

## Configuration

Hoshino reads one TOML file and never writes it during display.

| Platform | Default path |
|---|---|
| Linux/macOS with `XDG_CONFIG_HOME` | `$XDG_CONFIG_HOME/hoshino/config.toml` |
| Linux/macOS without it | `~/.config/hoshino/config.toml` |
| Windows with `APPDATA` | `%APPDATA%\\hoshino\\config.toml` |
| Windows without it | `%USERPROFILE%\\AppData\\Roaming\\hoshino\\config.toml` |

`--config PATH` selects an explicit file. A missing default file uses built-in
defaults; a missing, unreadable, or malformed explicitly selected file is a
fatal configuration error. A malformed file at the default path is also
reported rather than silently ignored. CLI values override the selected file,
and file values override built-ins. Image source precedence is
`--no-image > --image > config image.path > bundled`. In particular,
`--no-image` clears a configured image path; it does not reset unrelated image
settings.

Relative image paths are interpreted from the current working directory.
Relative coverage paths are interpreted from the Git worktree root. The
coverage path is only considered while in a worktree.

The complete annotated example is also in
[`config.example.toml`](config.example.toml). It uses the actual built-in
defaults, leaves `image.path` commented so the bundled logo remains active, and
leaves the optional coverage path commented out. Uncomment `image.path` to
override the bundled logo:

```toml
# Copy this file to the hoshino config path only when you want persistent
# settings. A missing default config is valid, and hoshino never creates one.

theme = "dusk-darker"
color = "auto"

# Optional existing report; relative to the Git worktree root.
# coverage_report_path = "coverage/lcov.info"

[image]
# User-supplied override; relative to the current working directory.
# path = "/path/to/your/image.png"
protocol = "auto"
width_cells = 24
max_height_rows = 12
fit = "contain"
position = "left"
# Configure both together when terminal-derived geometry is not suitable.
# cell_width_px = 8
# cell_height_px = 16

[limits]
subprocess_timeout_ms = 500
subprocess_output_bytes = 65536
scan_max_files = 10000
scan_max_file_bytes = 1048576
scan_max_read_bytes = 8388608
scan_elapsed_ms = 250
coverage_max_bytes = 8388608
image_max_file_bytes = 16777216
image_max_dimension_px = 4096
image_max_decoded_bytes = 67108864
image_max_frames = 120
live_refresh_ms = 1000
frame_min_delay_ms = 20
frame_max_delay_ms = 10000
```

The limits are intentionally conservative and each has a hard ceiling. Values
outside the allowed range are rejected:

| Setting | Built-in default | Accepted range |
|---|---:|---:|
| `subprocess_timeout_ms` | 500 | 1..=10000 |
| `subprocess_output_bytes` | 65536 | 1..=1048576 |
| `scan_max_files` | 10000 | 1..=100000 |
| `scan_max_file_bytes` | 1048576 | 1..=16777216 |
| `scan_max_read_bytes` | 8388608 | 1..=134217728 |
| `scan_elapsed_ms` | 250 | 1..=10000 |
| `coverage_max_bytes` | 8388608 | 1..=67108864 |
| `image_max_file_bytes` | 16777216 | 1..=67108864 |
| `image_max_dimension_px` | 4096 | 1..=16384 |
| `image_max_decoded_bytes` | 67108864 | 1..=536870912 |
| `image_max_frames` | 120 | 1..=1000 |
| `live_refresh_ms` | 1000 | 100..=60000 |
| `frame_min_delay_ms` | 20 | 1..=1000 |
| `frame_max_delay_ms` | 10000 | 1..=60000 |

`frame_min_delay_ms` must not exceed `frame_max_delay_ms`.

## Images and terminal protocols

The bundled `assets/logo.png` is the default source. Source precedence is
`--no-image > --image > config image.path > bundled`. User-supplied overrides
support PNG, JPEG, GIF, and WebP; APNG is supported through the PNG decoder.
All image sources use the file-size, dimension, retained decoded-byte, frame,
and animation-delay limits above. Delays are clamped to the configured minimum
and maximum.

Users are responsible for owning or having permission to display every image
they supply. Do not assume that a path is licensed merely because hoshino can
read it. The bundled logo has separate rights terms in
[`assets/NOTICE.md`](assets/NOTICE.md) and is not covered by the MIT License.

### Selection and fallback

`--protocol` or `image.protocol` selects `auto`, `kitty`, `iterm2`, `sixel`,
`halfblocks`, or `none`:

- On a non-TTY, every choice becomes text-only. Hoshino does not probe or
  write graphics to a pipe, or decode the bundled image. JSON output and
  generated/internal hook output are likewise text-only and do not decode an
  image. `none` deliberately selects silent text without a warning.
- One-shot `auto` selects query-free Kitty direct transfer only for direct
  Ghostty with non-query cell geometry and no tmux. Unknown terminals, iTerm2,
  tmux, or missing geometry use silent text. The geometry comes from the
  configured `cell_width_px`/`cell_height_px` pair when present, or from
  terminal/window dimensions without a capability or cursor-query round trip.
- Explicit one-shot Kitty uses the same query-free direct transfer and Unicode
  placeholder rows. It supports the reviewed tmux passthrough wrapper; when
  using Kitty through tmux, enable it in tmux with
  `set -g allow-passthrough on`. One-shot writes no stdin/query,
  cursor-movement, raw-mode, or alternate-screen controls. The virtual image
  remains with scrollback, with only a transient hint and no immediate delete.
- Explicit one-shot iTerm2, Sixel, and Halfblocks requests warn and fall back
  to the full text card. They do not render one-shot graphics.
- One-shot widths below 20 use bounded unframed text with no graphics. Framing
  begins at width 20. A left inset image requires at least 43 total columns; a
  top inset image requires at least 34. Below those image thresholds, image,
  divider, and gutter disappear together while the text expands inside the
  single frame. `position = "left"` or `"top"` selects where the image is inset
  within that frame for one-shot and live layouts.
- An explicit CLI image-path or decode failure is fatal. A config-selected or
  bundled image failure warns and falls back to text. An explicit CLI Kitty
  geometry failure is fatal; the corresponding config failure warns and falls
  back to text.
- Live mode requires both TTYs, retains the existing protocol choices and
  animation behavior, and uses the bundled logo by default. Kitty can animate
  decoded GIF/WebP/APNG frames; selected non-Kitty backends retain the first
  frame. Live owns raw mode, the alternate screen, and the cursor; after live
  Kitty rendering it performs targeted deletion of its own image. `q`, `Esc`,
  and `Ctrl-C` remain available regardless of image selection.

`fit = "contain"` preserves the complete image within the requested live cell
rectangle. `fit = "cover"` fills that rectangle and center-crops the excess.
Live layout computes semantic height first and drops the image when the
available area cannot retain the required text area.

## Collection boundaries and diagnostics

Hoshino is read-only with respect to the project. It does not run project
tests, invoke coverage tooling, make network requests, or generate coverage
reports.

### Project and toolchains

The finite language set is Rust, Python, TypeScript, JavaScript, and Go. Git
markers and source extensions determine the primary language using nearest
manifest precedence, counted source lines for ties, and a fixed language order.
Bun is a runtime marker, not a sixth language. The supported direct probes are:

| Language | Probe |
|---|---|
| Rust | `rustc --version` |
| Python | `python3 --version`, then `python --version` |
| TypeScript/JavaScript with Bun marker | `bun --version` |
| TypeScript/JavaScript without Bun marker | `node --version` |
| Go | `go version` |

Probes use direct executable/argument vectors, closed stdin, bounded stdout and
stderr, a deadline, and child termination on timeout. Missing, nonzero,
malformed, over-output, and timed-out probes become diagnostics; they do not
execute arbitrary project content.

The LOC scan honors ignore files, does not follow symlinks, prunes VCS,
vendor, build, distribution, dependency, and virtual-environment directories,
skips binary files, and enforces file-count, per-file, aggregate-read, and
elapsed-time budgets. A partial scan is marked with a visible diagnostic and
must not be read as a complete total. Normal mode collects the primary
language; `--full` includes all detected finite languages and `Other` text
files.

### Coverage

Coverage is detected from existing files only; hoshino never runs a coverage
command or writes a report. It parses LCOV, Cobertura XML, JaCoCo XML, Go
coverprofiles, and Istanbul `coverage-summary.json`.

An explicit `coverage_report_path` is tried first. Without one, candidates are
checked in this order:

```text
coverage/lcov.info
lcov.info
coverage/coverage-summary.json
coverage-summary.json
coverage.xml
cobertura.xml
coverage/cobertura-coverage.xml
target/site/jacoco/jacoco.xml
coverage.out
cover.out
```

Malformed, unreadable, oversized, or otherwise invalid candidates are skipped
with diagnostics so a later valid candidate can be used. A report older than
the newest counted source is marked `stale`; if the source scan was truncated,
freshness is `unknown`. Normal output shows aggregate line coverage. `--full`
adds format, path, mtime, freshness, and available branch/function details.

### System, time, and unavailable data

CPU utilization/frequency, memory, mounted disks, uptime, local clock/date,
timezone, offset, and day progress are reported when the platform provides
them. Values that cannot be obtained are unavailable (`null` in JSON and `—`
in text), never guessed or substituted with zero. The crate contains separate
macOS, Linux, and Windows adapters; adapter presence is not a claim that every
platform has been runtime-certified.

## Performance

The concise, text-only one-shot path is suitable for a prompt hook. On
2026-09-16, 20 warm release-binary invocations in a representative temporary
Rust worktree had a 74.797 ms median and 76.700 ms maximum, passing the target
of a median no greater than 300 ms and no invocation over 1 second. `--full`,
live mode, image decoding, and terminal graphics have different costs and are
not covered by that measurement.

## Validation status and limitations

The current implementation has been checked with rustup stable 1.98.1 using:

- formatting;
- 152 unit tests plus 7 hook integration tests;
- Rust documentation tests; and
- all-target, all-feature Clippy with warnings denied.

The local runtime environment is macOS. Bash, Zsh, and Fish hook syntax and
local behavior are validated there. Nushell and PowerShell generators receive
static checks but are locally runtime-unverified when those executables are
absent. Linux and Windows adapters exist and are tested at the source/unit
level where possible, but Linux/Windows runtime behavior is unverified from
this macOS environment. Do not read this document as remote-platform or
remote-shell certification.

Graphics selection, decoder bounds, non-TTY safety, and live cleanup are
covered by automated tests and PTY byte-level coverage. Normal and Full were
visually accepted in actual Ghostty 1.3.1 on macOS; live GUI behavior remains
covered by Ratatui frames and PTY smoke rather than a retained GUI capture. A
real terminal can still differ in font metrics, cell geometry, multiplexer
behavior, or backend support; explicit text and `--protocol none` remain the
fallback choices.

## Troubleshooting

- **`hoshino: command not found`:** ensure Cargo's install `bin` directory is
  on `PATH`, or invoke the binary from the Cargo install prefix.
- **Rust compiler or `dyld` failure:** check `rustup run stable rustc
  --version` and use the explicit `RUSTC`/`RUSTDOC` rustup install command in
  [Quick start](#quick-start) when a Homebrew compiler is shadowing rustup.
- **No colors or image in a pipe:** this is intentional. Non-TTY output is
  compact and control-free, and does not decode an image; use a real TTY for
  terminal graphics.
- **Image falls back to text:** check the path, supported format, configured
  limits, terminal identification, and cell-pixel geometry. Pair both
  geometry settings for explicit Kitty; use `--protocol none` for deliberate
  text output. One-shot iTerm2, Sixel, and Halfblocks requests intentionally
  warn and fall back to text.
- **Explicit image/config errors:** an attempted `--image` path or decode
  failure and explicit Kitty geometry failures are actionable nonzero errors.
  Config-selected and bundled image failures warn with a text fallback. A
  missing default config is okay, but a missing or malformed `--config PATH` is
  intentionally fatal.
- **Live exits immediately or refuses to start:** live needs both stdin and
  stdout attached to a TTY. Run it directly in a terminal, then use `q`,
  `Esc`, or `Ctrl-C` to exit.
- **Hooks print nothing:** outside a Git worktree that is expected. Inside a
  worktree, check that `hoshino` is on the sourcing shell's `PATH`; source the
  generated snippet again only after starting a fresh shell if its install
  guard was already set.
- **Coverage is absent or stale:** hoshino only reads existing bounded reports;
  it never creates one. Check the supported candidate path, configured path,
  report size, and report mtime relative to source files.

## License and inspiration

Hoshino is released under the MIT License; see [`LICENSE`](LICENSE). The
category-level idea of a terminal project card acknowledges the public
description of [mew](https://github.com/programmersd21/mew) as category
inspiration only. Hoshino has no copied mew code, layout, prompt, decorative
ASCII, or screenshot. The bundled logo is excluded from MIT; see
[`assets/NOTICE.md`](assets/NOTICE.md).
