//! Bounded image decoding and terminal-independent protocol decisions.

use std::{
    io::{BufReader, Cursor, Read},
    num::NonZeroU32,
    path::{Path, PathBuf},
    sync::atomic::{AtomicU32, Ordering},
};

use image::{
    AnimationDecoder, ImageFormat, ImageReader, RgbaImage,
    codecs::{gif::GifDecoder, png::PngDecoder, webp::WebPDecoder},
};

use crate::{
    cli::Protocol,
    config::ImageFit,
    limits::Limits,
    regular_file::{OpenRegularError, SymlinkPolicy, open_regular},
};

static NEXT_IMAGE_ID: AtomicU32 = AtomicU32::new(1);

pub const BUNDLED_LOGO_BYTES: &[u8] =
    include_bytes!(concat!(env!("CARGO_MANIFEST_DIR"), "/assets/logo.png"));

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RenderMode {
    OneShot,
    Live,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TerminalKind {
    Ghostty,
    Iterm2,
    Other,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct CellPixels {
    pub width: u32,
    pub height: u32,
}

impl CellPixels {
    pub fn new(width: u32, height: u32) -> Result<Self, ImageError> {
        if width == 0 || height == 0 {
            return Err(ImageError::InvalidGeometry("cell pixels must be nonzero"));
        }
        Ok(Self { width, height })
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct TerminalCapabilities {
    pub stdin_tty: bool,
    pub stdout_tty: bool,
    pub terminal: TerminalKind,
    pub cell_pixels: Option<CellPixels>,
    pub is_tmux: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SelectedProtocol {
    None,
    Text,
    Halfblocks,
    Kitty,
    Iterm2,
    Sixel,
    BoundedQueryRequired,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ProtocolDecision {
    pub protocol: SelectedProtocol,
    pub is_tmux: bool,
}

pub fn select_protocol(
    mode: RenderMode,
    requested: Protocol,
    caps: TerminalCapabilities,
) -> Result<ProtocolDecision, ImageError> {
    if !caps.stdout_tty {
        return Ok(ProtocolDecision {
            protocol: SelectedProtocol::None,
            is_tmux: caps.is_tmux,
        });
    }
    if mode == RenderMode::Live && !caps.stdin_tty {
        return Err(ImageError::LiveRequiresTtys);
    }
    let selected = match (mode, requested) {
        (_, Protocol::None) => SelectedProtocol::None,
        (_, Protocol::Halfblocks) => SelectedProtocol::Halfblocks,
        (RenderMode::Live, Protocol::Auto) => SelectedProtocol::BoundedQueryRequired,
        (RenderMode::Live, Protocol::Kitty) => SelectedProtocol::Kitty,
        (RenderMode::Live, Protocol::Iterm2) => SelectedProtocol::Iterm2,
        (RenderMode::Live, Protocol::Sixel) => SelectedProtocol::Sixel,
        (RenderMode::OneShot, Protocol::Auto) => {
            match (caps.terminal, caps.cell_pixels, caps.is_tmux) {
                (TerminalKind::Ghostty, Some(_), false) => SelectedProtocol::Kitty,
                _ => SelectedProtocol::Text,
            }
        }
        (RenderMode::OneShot, Protocol::Kitty) => {
            if caps.cell_pixels.is_none() {
                return Err(ImageError::MissingGeometry {
                    protocol: Protocol::Kitty,
                });
            }
            SelectedProtocol::Kitty
        }
        (RenderMode::OneShot, Protocol::Iterm2) => SelectedProtocol::Iterm2,
        (RenderMode::OneShot, Protocol::Sixel) => SelectedProtocol::Sixel,
    };
    Ok(ProtocolDecision {
        protocol: selected,
        is_tmux: caps.is_tmux,
    })
}

#[derive(Debug)]
pub enum ImageError {
    Io { path: PathBuf, message: String },
    UnsupportedFormat,
    Decode(String),
    Limit(&'static str),
    MissingGeometry { protocol: Protocol },
    InvalidGeometry(&'static str),
    LiveRequiresTtys,
}

impl std::fmt::Display for ImageError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Io { path, message } => {
                write!(f, "cannot read image {}: {message}", path.display())
            }
            Self::UnsupportedFormat => f.write_str("image format must be PNG, JPEG, GIF, or WebP"),
            Self::Decode(message) => write!(f, "cannot decode image: {message}"),
            Self::Limit(limit) => write!(f, "image exceeds configured {limit} limit"),
            Self::MissingGeometry { protocol } => write!(
                f,
                "{protocol:?} images require configured cell pixel geometry"
            ),
            Self::InvalidGeometry(message) => f.write_str(message),
            Self::LiveRequiresTtys => {
                f.write_str("live image display requires both stdin and stdout to be TTYs")
            }
        }
    }
}

impl std::error::Error for ImageError {}

#[derive(Clone, Debug)]
pub struct DecodedFrame {
    pub image: RgbaImage,
    pub delay_ms: u64,
}

#[derive(Clone, Debug)]
pub struct DecodedImage {
    pub format: ImageFormat,
    pub first_frame: RgbaImage,
    pub frames: Vec<DecodedFrame>,
}

pub fn load_image(path: &Path, limits: &Limits) -> Result<DecodedImage, ImageError> {
    let file = open_regular(path, SymlinkPolicy::Follow).map_err(|error| ImageError::Io {
        path: path.to_path_buf(),
        message: match error {
            OpenRegularError::NotRegular => "image path is not a regular file".into(),
            OpenRegularError::Io(error) => error.to_string(),
            #[cfg(not(any(unix, windows)))]
            OpenRegularError::NonblockingUnavailable => {
                "platform cannot safely open arbitrary paths without blocking".into()
            }
        },
    })?;
    let read_limit = limits
        .image_max_file_bytes
        .checked_add(1)
        .ok_or(ImageError::Limit("file bytes"))?;
    let capacity =
        usize::try_from(read_limit.min(65_536)).map_err(|_| ImageError::Limit("file bytes"))?;
    let mut bytes = Vec::with_capacity(capacity);
    file.take(read_limit)
        .read_to_end(&mut bytes)
        .map_err(|error| ImageError::Io {
            path: path.to_path_buf(),
            message: error.to_string(),
        })?;
    if u64::try_from(bytes.len()).map_err(|_| ImageError::Limit("file bytes"))?
        > limits.image_max_file_bytes
    {
        return Err(ImageError::Limit("file bytes"));
    }
    load_image_bytes(&bytes, limits)
}

pub fn load_bundled_logo(limits: &Limits) -> Result<DecodedImage, ImageError> {
    load_image_bytes(BUNDLED_LOGO_BYTES, limits)
}

pub fn load_image_bytes(bytes: &[u8], limits: &Limits) -> Result<DecodedImage, ImageError> {
    if u64::try_from(bytes.len()).map_err(|_| ImageError::Limit("file bytes"))?
        > limits.image_max_file_bytes
    {
        return Err(ImageError::Limit("file bytes"));
    }
    let format = image::guess_format(bytes).map_err(|_| ImageError::UnsupportedFormat)?;
    if !matches!(
        format,
        ImageFormat::Png | ImageFormat::Jpeg | ImageFormat::Gif | ImageFormat::WebP
    ) {
        return Err(ImageError::UnsupportedFormat);
    }
    let reader = ImageReader::with_format(Cursor::new(bytes), format);
    let (width, height) = reader.into_dimensions().map_err(decode_error)?;
    validate_dimensions(width, height, limits)?;
    let frames = match format {
        ImageFormat::Gif => decode_animation(
            GifDecoder::new(BufReader::new(Cursor::new(bytes))).map_err(decode_error)?,
            limits,
        )?,
        ImageFormat::WebP => decode_animation(
            WebPDecoder::new(BufReader::new(Cursor::new(bytes))).map_err(decode_error)?,
            limits,
        )?,
        ImageFormat::Png => {
            let decoder =
                PngDecoder::new(BufReader::new(Cursor::new(bytes))).map_err(decode_error)?;
            if decoder.is_apng().map_err(decode_error)? {
                decode_animation(decoder.apng().map_err(decode_error)?, limits)?
            } else {
                Vec::new()
            }
        }
        ImageFormat::Jpeg => Vec::new(),
        _ => unreachable!(),
    };
    if frames.is_empty() {
        let image = image::load_from_memory_with_format(bytes, format)
            .map_err(decode_error)?
            .to_rgba8();
        validate_frame(&image, 0, limits)?;
        return Ok(DecodedImage {
            format,
            first_frame: image,
            frames: Vec::new(),
        });
    }
    let first_frame = frames[0].image.clone();
    let retained_bytes = frames
        .iter()
        .try_fold(frame_bytes(&first_frame)?, |total, frame| {
            total
                .checked_add(frame_bytes(&frame.image)?)
                .ok_or(ImageError::Limit("decoded bytes"))
        })?;
    if retained_bytes > limits.image_max_decoded_bytes {
        return Err(ImageError::Limit("decoded bytes"));
    }
    Ok(DecodedImage {
        format,
        first_frame,
        frames,
    })
}

fn decode_animation<'a, D: AnimationDecoder<'a>>(
    decoder: D,
    limits: &Limits,
) -> Result<Vec<DecodedFrame>, ImageError> {
    let mut total = 0_u64;
    let mut result = Vec::new();
    for frame in decoder.into_frames() {
        if u32::try_from(result.len()).map_err(|_| ImageError::Limit("frames"))?
            >= limits.image_max_frames
        {
            return Err(ImageError::Limit("frames"));
        }
        let frame = frame.map_err(decode_error)?;
        let delay = clamp_delay(frame.delay().numer_denom_ms(), limits);
        let image = frame.into_buffer();
        validate_dimensions(image.width(), image.height(), limits)?;
        let bytes = frame_bytes(&image)?;
        total = total
            .checked_add(bytes)
            .ok_or(ImageError::Limit("decoded bytes"))?;
        if total > limits.image_max_decoded_bytes {
            return Err(ImageError::Limit("decoded bytes"));
        }
        result.push(DecodedFrame {
            image,
            delay_ms: delay,
        });
    }
    Ok(result)
}

fn validate_dimensions(width: u32, height: u32, limits: &Limits) -> Result<(), ImageError> {
    if width == 0
        || height == 0
        || width > limits.image_max_dimension_px
        || height > limits.image_max_dimension_px
    {
        return Err(ImageError::Limit("dimensions"));
    }
    let bytes = u64::from(width)
        .checked_mul(u64::from(height))
        .and_then(|pixels| pixels.checked_mul(4))
        .ok_or(ImageError::Limit("decoded bytes"))?;
    if bytes > limits.image_max_decoded_bytes {
        return Err(ImageError::Limit("decoded bytes"));
    }
    Ok(())
}

fn validate_frame(image: &RgbaImage, total: u64, limits: &Limits) -> Result<(), ImageError> {
    validate_dimensions(image.width(), image.height(), limits)?;
    if total
        .checked_add(frame_bytes(image)?)
        .ok_or(ImageError::Limit("decoded bytes"))?
        > limits.image_max_decoded_bytes
    {
        return Err(ImageError::Limit("decoded bytes"));
    }
    Ok(())
}

fn frame_bytes(image: &RgbaImage) -> Result<u64, ImageError> {
    u64::from(image.width())
        .checked_mul(u64::from(image.height()))
        .and_then(|pixels| pixels.checked_mul(4))
        .ok_or(ImageError::Limit("decoded bytes"))
}

fn decode_error(error: image::ImageError) -> ImageError {
    ImageError::Decode(error.to_string())
}

pub fn clamp_delay((numerator, denominator): (u32, u32), limits: &Limits) -> u64 {
    let raw = if denominator == 0 {
        0
    } else {
        u64::from(numerator) / u64::from(denominator)
    };
    raw.clamp(limits.frame_min_delay_ms, limits.frame_max_delay_ms)
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PixelRect {
    pub width: u32,
    pub height: u32,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Crop {
    pub x: u32,
    pub y: u32,
    pub width: u32,
    pub height: u32,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ResizePlan {
    pub target: PixelRect,
    pub resized: PixelRect,
    pub crop: Option<Crop>,
}

pub fn resize_plan(
    source: PixelRect,
    cells: PixelRect,
    cell_pixels: CellPixels,
    fit: ImageFit,
) -> Result<ResizePlan, ImageError> {
    if source.width == 0 || source.height == 0 || cells.width == 0 || cells.height == 0 {
        return Err(ImageError::InvalidGeometry(
            "image and cell rectangles must be nonzero",
        ));
    }
    let target = PixelRect {
        width: cells
            .width
            .checked_mul(cell_pixels.width)
            .ok_or(ImageError::InvalidGeometry("pixel width overflows"))?,
        height: cells
            .height
            .checked_mul(cell_pixels.height)
            .ok_or(ImageError::InvalidGeometry("pixel height overflows"))?,
    };
    let lhs = u64::from(target.width)
        .checked_mul(u64::from(source.height))
        .ok_or(ImageError::InvalidGeometry("aspect ratio overflows"))?;
    let rhs = u64::from(target.height)
        .checked_mul(u64::from(source.width))
        .ok_or(ImageError::InvalidGeometry("aspect ratio overflows"))?;
    let scale_width = matches!(fit, ImageFit::Contain) == (lhs <= rhs);
    let (width, height) = if scale_width {
        (
            target.width,
            scaled(source.height, target.width, source.width)?,
        )
    } else {
        (
            scaled(source.width, target.height, source.height)?,
            target.height,
        )
    };
    let resized = PixelRect { width, height };
    let crop = (fit == ImageFit::Cover).then(|| Crop {
        x: width.saturating_sub(target.width) / 2,
        y: height.saturating_sub(target.height) / 2,
        width: target.width,
        height: target.height,
    });
    Ok(ResizePlan {
        target,
        resized,
        crop,
    })
}

fn scaled(value: u32, numerator: u32, denominator: u32) -> Result<u32, ImageError> {
    let product = u64::from(value)
        .checked_mul(u64::from(numerator))
        .ok_or(ImageError::InvalidGeometry("resize overflows"))?;
    let rounded = product
        .checked_add(u64::from(denominator) - 1)
        .ok_or(ImageError::InvalidGeometry("resize overflows"))?
        / u64::from(denominator);
    u32::try_from(rounded).map_err(|_| ImageError::InvalidGeometry("resize overflows"))
}

#[derive(Clone, Debug)]
pub struct ImageSession {
    pub id: NonZeroU32,
    pub decision: ProtocolDecision,
}

impl ImageSession {
    pub fn new(decision: ProtocolDecision) -> Self {
        let id = loop {
            let value = NEXT_IMAGE_ID.fetch_add(1, Ordering::Relaxed);
            if let Some(id) = NonZeroU32::new(value) {
                break id;
            }
        };
        Self { id, decision }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::{ExtendedColorType, ImageEncoder, Rgba};
    #[cfg(unix)]
    use std::{
        fs::{create_dir_all, remove_dir_all, write},
        os::unix::fs::symlink,
        process::Command,
    };

    fn limits() -> Limits {
        Limits {
            image_max_file_bytes: 1_000_000,
            image_max_dimension_px: 100,
            image_max_decoded_bytes: 1_000_000,
            image_max_frames: 3,
            frame_min_delay_ms: 20,
            frame_max_delay_ms: 1_000,
            ..Limits::default()
        }
    }
    fn caps(
        stdout_tty: bool,
        stdin_tty: bool,
        terminal: TerminalKind,
        geometry: bool,
    ) -> TerminalCapabilities {
        TerminalCapabilities {
            stdout_tty,
            stdin_tty,
            terminal,
            cell_pixels: geometry.then(|| CellPixels::new(8, 16).unwrap()),
            is_tmux: true,
        }
    }

    #[test]
    fn selection_matrix_never_probes_non_ttys() {
        for requested in [
            Protocol::Auto,
            Protocol::Kitty,
            Protocol::Iterm2,
            Protocol::Sixel,
            Protocol::Halfblocks,
        ] {
            assert_eq!(
                select_protocol(
                    RenderMode::OneShot,
                    requested,
                    caps(false, false, TerminalKind::Ghostty, true)
                )
                .unwrap()
                .protocol,
                SelectedProtocol::None
            );
        }
        assert_eq!(
            select_protocol(
                RenderMode::OneShot,
                Protocol::Auto,
                TerminalCapabilities {
                    is_tmux: false,
                    ..caps(true, true, TerminalKind::Ghostty, true)
                }
            )
            .unwrap()
            .protocol,
            SelectedProtocol::Kitty
        );
        assert_eq!(
            select_protocol(
                RenderMode::OneShot,
                Protocol::Auto,
                caps(true, true, TerminalKind::Iterm2, true)
            )
            .unwrap()
            .protocol,
            SelectedProtocol::Text
        );
        assert_eq!(
            select_protocol(
                RenderMode::OneShot,
                Protocol::Auto,
                caps(true, true, TerminalKind::Other, true)
            )
            .unwrap()
            .protocol,
            SelectedProtocol::Text
        );
        assert_eq!(
            select_protocol(
                RenderMode::OneShot,
                Protocol::Halfblocks,
                caps(true, true, TerminalKind::Other, false)
            )
            .unwrap()
            .protocol,
            SelectedProtocol::Halfblocks
        );
        assert!(matches!(
            select_protocol(
                RenderMode::OneShot,
                Protocol::Kitty,
                caps(true, true, TerminalKind::Other, false)
            ),
            Err(ImageError::MissingGeometry { .. })
        ));
        assert_eq!(
            select_protocol(
                RenderMode::Live,
                Protocol::Auto,
                caps(true, true, TerminalKind::Other, false)
            )
            .unwrap()
            .protocol,
            SelectedProtocol::BoundedQueryRequired
        );
        assert!(matches!(
            select_protocol(
                RenderMode::Live,
                Protocol::Kitty,
                caps(true, false, TerminalKind::Other, false)
            ),
            Err(ImageError::LiveRequiresTtys)
        ));
    }

    fn static_bytes(format: ImageFormat) -> Vec<u8> {
        let image = RgbaImage::from_pixel(2, 3, Rgba([1, 2, 3, 255]));
        let mut bytes = Vec::new();
        match format {
            ImageFormat::Png => image::codecs::png::PngEncoder::new(&mut bytes)
                .write_image(image.as_raw(), 2, 3, ExtendedColorType::Rgba8)
                .unwrap(),
            ImageFormat::Jpeg => {
                let rgb = image::RgbImage::from_pixel(2, 3, image::Rgb([1, 2, 3]));
                image::codecs::jpeg::JpegEncoder::new(&mut bytes)
                    .write_image(rgb.as_raw(), 2, 3, ExtendedColorType::Rgb8)
                    .unwrap()
            }
            ImageFormat::WebP => image::codecs::webp::WebPEncoder::new_lossless(&mut bytes)
                .write_image(image.as_raw(), 2, 3, ExtendedColorType::Rgba8)
                .unwrap(),
            _ => unreachable!(),
        };
        bytes
    }

    #[cfg(unix)]
    #[test]
    fn load_image_rejects_non_regular_paths_and_supports_regular_symlinks() {
        let root = std::env::temp_dir().join(format!("hoshino-image-{}", std::process::id()));
        let _ = remove_dir_all(&root);
        create_dir_all(&root).unwrap();
        let fifo = root.join("image.fifo");
        let Ok(status) = Command::new("mkfifo").arg(&fifo).status() else {
            remove_dir_all(&root).unwrap();
            return;
        };
        assert!(status.success());
        let fifo_link = root.join("fifo-link");
        symlink("image.fifo", &fifo_link).unwrap();
        for path in [&fifo, &fifo_link, &root] {
            assert!(matches!(
                load_image(path, &limits()),
                Err(ImageError::Io { message, .. }) if message.contains("not a regular file")
            ));
        }

        let image = root.join("image.png");
        write(&image, static_bytes(ImageFormat::Png)).unwrap();
        let image_link = root.join("image-link.png");
        symlink("image.png", &image_link).unwrap();
        assert_eq!(
            load_image(&image_link, &limits())
                .unwrap()
                .first_frame
                .dimensions(),
            (2, 3)
        );
        remove_dir_all(&root).unwrap();
    }

    #[test]
    fn decodes_supported_static_images_and_bounded_gif_frames() {
        for format in [ImageFormat::Png, ImageFormat::Jpeg, ImageFormat::WebP] {
            let decoded = load_image_bytes(&static_bytes(format), &limits()).unwrap();
            assert_eq!(decoded.first_frame.dimensions(), (2, 3));
        }
        let frames = [
            image::Frame::from_parts(
                RgbaImage::from_pixel(2, 2, Rgba([1, 0, 0, 255])),
                0,
                0,
                image::Delay::from_numer_denom_ms(0, 1),
            ),
            image::Frame::from_parts(
                RgbaImage::from_pixel(2, 2, Rgba([0, 1, 0, 255])),
                0,
                0,
                image::Delay::from_numer_denom_ms(9_999, 1),
            ),
        ];
        let mut bytes = Vec::new();
        image::codecs::gif::GifEncoder::new(&mut bytes)
            .encode_frames(frames)
            .unwrap();
        let decoded = load_image_bytes(&bytes, &limits()).unwrap();
        assert_eq!(decoded.frames.len(), 2);
        assert_eq!(decoded.frames[0].delay_ms, 20);
        assert_eq!(decoded.frames[1].delay_ms, 1_000);
        let mut frame_limit = limits();
        frame_limit.image_max_frames = 1;
        assert!(matches!(
            load_image_bytes(&bytes, &frame_limit),
            Err(ImageError::Limit("frames"))
        ));
    }

    #[test]
    fn bundled_logo_decodes_with_the_shared_bounds() {
        let decoded = load_bundled_logo(&Limits::default()).unwrap();
        assert_eq!(decoded.format, ImageFormat::Png);
        assert_eq!(decoded.first_frame.dimensions(), (640, 640));
        assert!(decoded.frames.is_empty());

        let file_limit = Limits {
            image_max_file_bytes: 1,
            ..Limits::default()
        };
        assert!(matches!(
            load_bundled_logo(&file_limit),
            Err(ImageError::Limit("file bytes"))
        ));

        let dimension_limit = Limits {
            image_max_dimension_px: 639,
            ..Limits::default()
        };
        assert!(matches!(
            load_bundled_logo(&dimension_limit),
            Err(ImageError::Limit("dimensions"))
        ));

        let decoded_limit = Limits {
            image_max_decoded_bytes: 640 * 640 * 4 - 1,
            ..Limits::default()
        };
        assert!(matches!(
            load_bundled_logo(&decoded_limit),
            Err(ImageError::Limit("decoded bytes"))
        ));
    }

    #[test]
    fn malformed_and_limits_are_errors() {
        assert!(matches!(
            load_image_bytes(b"not an image", &limits()),
            Err(ImageError::UnsupportedFormat)
        ));
        let truncated = &static_bytes(ImageFormat::Png)[..10];
        assert!(matches!(
            load_image_bytes(truncated, &limits()),
            Err(ImageError::Decode(_))
        ));
        let mut file_limit = limits();
        file_limit.image_max_file_bytes = 1;
        assert!(matches!(
            load_image_bytes(&static_bytes(ImageFormat::Png), &file_limit),
            Err(ImageError::Limit("file bytes"))
        ));
        let mut dimension_limit = limits();
        dimension_limit.image_max_dimension_px = 1;
        assert!(matches!(
            load_image_bytes(&static_bytes(ImageFormat::Png), &dimension_limit),
            Err(ImageError::Limit("dimensions"))
        ));
        let mut decoded_limit = limits();
        decoded_limit.image_max_decoded_bytes = 20;
        assert!(matches!(
            load_image_bytes(&static_bytes(ImageFormat::Png), &decoded_limit),
            Err(ImageError::Limit("decoded bytes"))
        ));
    }

    #[test]
    fn resize_and_session_are_checked() {
        let cell = CellPixels::new(8, 16).unwrap();
        let contain = resize_plan(
            PixelRect {
                width: 400,
                height: 100,
            },
            PixelRect {
                width: 10,
                height: 10,
            },
            cell,
            ImageFit::Contain,
        )
        .unwrap();
        assert_eq!(
            contain.resized,
            PixelRect {
                width: 80,
                height: 20
            }
        );
        assert_eq!(contain.crop, None);
        let cover = resize_plan(
            PixelRect {
                width: 400,
                height: 100,
            },
            PixelRect {
                width: 10,
                height: 10,
            },
            cell,
            ImageFit::Cover,
        )
        .unwrap();
        assert_eq!(
            cover.resized,
            PixelRect {
                width: 640,
                height: 160
            }
        );
        assert_eq!(cover.crop.unwrap().width, 80);
        assert!(
            resize_plan(
                PixelRect {
                    width: 1,
                    height: 1
                },
                PixelRect {
                    width: u32::MAX,
                    height: 1
                },
                CellPixels::new(2, 1).unwrap(),
                ImageFit::Contain
            )
            .is_err()
        );
        let decision = ProtocolDecision {
            protocol: SelectedProtocol::Kitty,
            is_tmux: true,
        };
        let session = ImageSession::new(decision);
        assert_ne!(session.id.get(), 0);
        assert_eq!(session.decision, decision);
    }
}
