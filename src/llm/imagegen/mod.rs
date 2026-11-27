//! The `imagegen` tool: images — stills and looping GIFs — made and edited
//! with OpenAI's newest image model, GPT-Image-2.6 Sunburst, through the
//! user's **ChatGPT Codex sign-in**, whatever provider the conversation
//! itself runs on (`docs/imagegen.md`).
//!
//! Ported from OpenAI Codex's built-in `image_gen.imagegen`, which is a
//! client-side function rather than the hosted Responses tool: the model
//! calls it, and the call POSTs JSON to the Codex backend's Images endpoints
//! with the same ChatGPT credential the chat requests use. The answer's image
//! is streamed to disk (`scan`), the cell shows one fact line naming the
//! file, and the picture rides the model's context like an image `read`.
//!
//! The pure half — the spec, the arguments, the request bodies, the fact
//! line, the wording of a failure — is unit-tested here; the HTTP call and
//! the file are the boundary half at the bottom.

pub(crate) mod scan;

use std::ops::RangeInclusive;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde::Deserialize;
use serde_json::{Value, json};

use crate::images::AttachmentUrl;
use crate::llm::tools::ToolOutcome;
use crate::stream::CancelToken;

/// The tool's wire name — Codex's own function name, one lowercase word like
/// every tool here, so its display name lowercases back to it
/// (`context::wire_tool_name`).
pub const IMAGEGEN_TOOL_NAME: &str = "imagegen";

/// The cell header's name: `● ImageGen(a red fox at dawn)`.
pub const IMAGEGEN_TOOL_DISPLAY: &str = "ImageGen";

/// The image model a request names: OpenAI's newest, GPT-Image-2.6
/// Sunburst — the one of the pair made for finished work and consistent
/// edits, and the Images API's own default for an edit (`docs/imagegen.md`
/// *The model*). The tool takes no model; the call's `quality` is the speed
/// control.
pub const IMAGE_MODEL: &str = "gpt-image-2.6-sunburst";

/// The model a request is sent again with when the backend refuses
/// [`IMAGE_MODEL`] by name: the one images were made with before it, so a
/// backend that does not take the newest yet still makes the image
/// (`docs/imagegen.md` *The model*).
pub const FALLBACK_IMAGE_MODEL: &str = "gpt-image-2";

/// What both edges of an exact `size` are multiples of.
const SIZE_STEP: u32 = 16;
/// The longest edge an exact `size` may have.
const MAX_SIZE_EDGE: u32 = 3840;
/// How many times its short edge an exact `size`'s long edge may be.
const MAX_SIZE_RATIO: u32 = 3;
/// The most pixels an exact `size` may have: 3840x2160's.
const MAX_SIZE_PIXELS: u64 = 3840 * 2160;
/// The frames a GIF may have.
const GIF_FRAME_COUNTS: RangeInclusive<u32> = 2..=16;
/// How long each frame of a GIF may show, in milliseconds.
const GIF_FRAME_DURATIONS_MS: RangeInclusive<u32> = 20..=2000;

/// The pixel size a call asks for (`docs/imagegen.md` *The tool*).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ImageSize {
    /// The backend chooses from the prompt — the default.
    Auto,
    /// Exactly this many pixels wide and high.
    Exact(u32, u32),
}

impl ImageSize {
    /// What the request's `size` says: `auto`, or `1536x1024`.
    #[must_use]
    pub fn wire(self) -> String {
        match self {
            Self::Auto => "auto".to_string(),
            Self::Exact(width, height) => format!("{width}x{height}"),
        }
    }

    /// Read a call's `size`: `auto` (in any case, or nothing at all), or
    /// WIDTHxHEIGHT as the backend takes it — both edges multiples of 16, the
    /// longer at most 3840 and at most three times the shorter, and no more
    /// pixels than 3840x2160 has. Refused here, naming the rule, rather than
    /// after a request the backend would refuse the same way.
    fn parse(raw: &str) -> Result<Self, String> {
        let text = raw.trim();
        if text.is_empty() || text.eq_ignore_ascii_case("auto") {
            return Ok(Self::Auto);
        }
        let shapeless =
            || format!("`size` is `auto` or WIDTHxHEIGHT, such as `1536x1024` — not `{text}`");
        let (width, height) = text.split_once(['x', 'X']).ok_or_else(shapeless)?;
        let edge = |edge: &str| edge.trim().parse::<u32>().ok().filter(|edge| *edge > 0);
        let (Some(width), Some(height)) = (edge(width), edge(height)) else {
            return Err(shapeless());
        };
        if width % SIZE_STEP != 0 || height % SIZE_STEP != 0 {
            // The nearest size on the grid, offered when it passes the rest.
            let on_grid = |edge: u32| {
                (edge.saturating_add(SIZE_STEP / 2) / SIZE_STEP * SIZE_STEP).max(SIZE_STEP)
            };
            let nearest = format!("{}x{}", on_grid(width), on_grid(height));
            let offer = if Self::parse(&nearest).is_ok() {
                format!(" — try {nearest}")
            } else {
                String::new()
            };
            return Err(format!(
                "`size` {width}x{height}: both edges must be multiples of {SIZE_STEP}{offer}"
            ));
        }
        let (long, short) = (width.max(height), width.min(height));
        if long > MAX_SIZE_EDGE {
            return Err(format!(
                "`size` {width}x{height}: an edge may be at most {MAX_SIZE_EDGE} pixels"
            ));
        }
        if long > short * MAX_SIZE_RATIO {
            return Err(format!(
                "`size` {width}x{height}: the long edge may be at most {MAX_SIZE_RATIO} times                  the short one"
            ));
        }
        if u64::from(width) * u64::from(height) > MAX_SIZE_PIXELS {
            return Err(format!(
                "`size` {width}x{height}: at most 8,294,400 pixels, as 3840x2160 has"
            ));
        }
        Ok(Self::Exact(width, height))
    }
}

/// How much care the image is rendered with (`docs/imagegen.md` *The tool*).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ImageQuality {
    /// The backend chooses — the default.
    Auto,
    /// The fastest: drafts, thumbnails, quick tries.
    Low,
    /// Between the two.
    Medium,
    /// The most detail: dense text, final assets.
    High,
}

impl ImageQuality {
    /// Every quality, in the order the spec lists them.
    const ALL: [Self; 4] = [Self::Auto, Self::Low, Self::Medium, Self::High];

    /// What the request's `quality` says.
    #[must_use]
    pub fn wire(self) -> &'static str {
        match self {
            Self::Auto => "auto",
            Self::Low => "low",
            Self::Medium => "medium",
            Self::High => "high",
        }
    }

    /// Read a call's `quality`, in any case; nothing at all is `auto`.
    fn parse(raw: &str) -> Result<Self, String> {
        let text = raw.trim();
        if text.is_empty() {
            return Ok(Self::Auto);
        }
        Self::ALL
            .into_iter()
            .find(|quality| quality.wire().eq_ignore_ascii_case(text))
            .ok_or_else(|| {
                format!("`quality` is one of `auto`, `low`, `medium` or `high` — not `{text}`")
            })
    }
}

/// A looping GIF instead of a still: how many frames, and how long each
/// shows — the backend's own defaults (9 frames, 200 ms each) for what is
/// left out (`docs/imagegen.md` *A GIF*).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct GifOptions {
    /// Frames in the loop, 2 to 16.
    pub frame_count: Option<u32>,
    /// How long each frame shows, 20 to 2000 milliseconds.
    pub frame_duration_ms: Option<u32>,
}

impl GifOptions {
    /// Read a call's `gif`: an object of the two numbers (`{}` for the
    /// defaults), or the bare `true` or `false` a model may send instead;
    /// `null` is no GIF. A field it does not take does nothing, so it is
    /// refused by name, as an unknown argument is.
    fn parse(value: Option<Value>) -> Result<Option<Self>, String> {
        let fields = match value {
            None | Some(Value::Null | Value::Bool(false)) => return Ok(None),
            Some(Value::Bool(true)) => return Ok(Some(Self::default())),
            Some(Value::Object(fields)) => fields,
            Some(other) => {
                return Err(format!(
                    "`gif` is an object — `{{}}` for the defaults — not {other}"
                ));
            }
        };
        let mut gif = Self::default();
        for (name, value) in &fields {
            match name.as_str() {
                "frame_count" => gif.frame_count = gif_number(name, value, GIF_FRAME_COUNTS)?,
                "frame_duration_ms" => {
                    gif.frame_duration_ms = gif_number(name, value, GIF_FRAME_DURATIONS_MS)?;
                }
                _ => {
                    return Err(format!(
                        "`gif` takes `frame_count` and `frame_duration_ms`, not `{name}`"
                    ));
                }
            }
        }
        Ok(Some(gif))
    }
}

/// One of a GIF's numbers: a whole number in `range`, or `null` for the
/// backend's default.
fn gif_number(
    name: &str,
    value: &Value,
    range: RangeInclusive<u32>,
) -> Result<Option<u32>, String> {
    if value.is_null() {
        return Ok(None);
    }
    value
        .as_u64()
        .and_then(|number| u32::try_from(number).ok())
        .filter(|number| range.contains(number))
        .map(Some)
        .ok_or_else(|| {
            format!(
                "`gif.{name}` must be a whole number from {} to {}, not {value}",
                range.start(),
                range.end()
            )
        })
}

/// Whether a call's arguments ask for a GIF — what the running cell names
/// while it is made (`Generating GIF…`).
#[must_use]
pub fn asks_for_gif(arguments: &str) -> bool {
    ImagegenArgs::parse(arguments).is_ok_and(|args| args.gif.is_some())
}

/// How many images one call may edit or take as references.
pub const MAX_REFERENCED_IMAGES: usize = 5;

/// The Images API's limit on one input image.
pub const MAX_REFERENCE_BYTES: u64 = 50 * 1024 * 1024;

/// Seeds the `/settings` **Image generation** row for the run, never saved:
/// falsy starts the session with the tool and its skill withdrawn.
pub const IMAGEGEN_ENV: &str = "ALTER_ZERO_IMAGEGEN";

/// Is `name` this tool's wire name?
#[must_use]
pub fn is_imagegen_tool(name: &str) -> bool {
    name == IMAGEGEN_TOOL_NAME
}

/// The tool's Chat Completions spec (`docs/imagegen.md` *The tool*).
#[must_use]
pub fn imagegen_spec() -> Value {
    crate::llm::tools::function_spec(
        IMAGEGEN_TOOL_NAME,
        IMAGEGEN_DESCRIPTION,
        json!({
            "type": "object",
            "properties": {
                "prompt": {
                    "type": "string",
                    "description": "What to generate, or the change to make to the referenced images."
                },
                "transparent_background": {
                    "type": "boolean",
                    "description": "Whether the output should have a transparent background. Defaults to false."
                },
                "referenced_image_paths": {
                    "type": "array",
                    "items": {"type": "string"},
                    "description": "Absolute paths of up to 5 local images to edit or to use as references."
                },
                "size": {
                    "type": "string",
                    "description": "`auto` (the default), or WIDTHxHEIGHT such as `1536x1024`: multiples of 16, up to 3840, at most 3:1."
                },
                "quality": {
                    "type": "string",
                    "enum": ImageQuality::ALL.map(ImageQuality::wire),
                    "description": "`auto` (the default); `low` is fastest, for drafts; `high` for dense text and final assets."
                },
                "gif": {
                    "type": "object",
                    "description": "Make a looping GIF of 2 to 16 frames instead of a still; `{}` takes the defaults.",
                    "properties": {
                        "frame_count": {
                            "type": "integer",
                            "minimum": GIF_FRAME_COUNTS.start(),
                            "maximum": GIF_FRAME_COUNTS.end(),
                            "description": "Frames in the loop, 2 to 16. Defaults to 9."
                        },
                        "frame_duration_ms": {
                            "type": "integer",
                            "minimum": GIF_FRAME_DURATIONS_MS.start(),
                            "maximum": GIF_FRAME_DURATIONS_MS.end(),
                            "description": "How long each frame shows, in milliseconds, 20 to 2000. Defaults to 200."
                        }
                    },
                    "additionalProperties": false
                }
            },
            "required": ["prompt"],
            "additionalProperties": false
        }),
    )
}

/// The tool's description: what it does and its sharp edges, short — every
/// word rides every request that offers it, and the craft (when to draw, how
/// to word a prompt, where a picture belongs, how to animate) is the
/// `imagegen` skill's, listed whenever the tool is offered
/// (`docs/imagegen.md`).
const IMAGEGEN_DESCRIPTION: &str = "\
Generate an image from a prompt, or edit images, with ChatGPT's image model \
through the user's ChatGPT sign-in. Each call makes one image (it can take a \
minute or two), saves it, shows it to the user and returns its path. Generate \
directly, without asking for confirmation. For an edit or a style reference, \
list the images in `referenced_image_paths`, looking at a local one with `read` \
first; set `transparent_background` for a cutout. Set `gif` for a short looping \
animation; to change one, reference a spritesheet of its frames, not the GIF \
itself.";

/// A call's arguments, checked.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ImagegenArgs {
    /// What to make, or what to change.
    pub prompt: String,
    /// Whether the image should have a transparent background.
    pub transparent_background: bool,
    /// The images to edit or take as references, as the model wrote them.
    pub referenced_image_paths: Vec<String>,
    /// The pixel size asked for.
    pub size: ImageSize,
    /// The rendering quality asked for.
    pub quality: ImageQuality,
    /// A looping GIF instead of a still, when asked for.
    pub gif: Option<GifOptions>,
}

impl ImagegenArgs {
    /// Parse a call's raw arguments. An argument the tool does not take is
    /// refused, naming the ones it does — a model that sends a count learns
    /// at once that it does nothing.
    ///
    /// # Errors
    /// Unparseable or unknown arguments, a blank prompt, more than
    /// [`MAX_REFERENCED_IMAGES`] references, or a `size`, `quality` or `gif`
    /// the backend would refuse — each a message the model reads.
    pub fn parse(arguments: &str) -> Result<Self, String> {
        #[derive(Deserialize)]
        #[serde(deny_unknown_fields)]
        struct Raw {
            #[serde(default)]
            prompt: Option<String>,
            #[serde(default)]
            transparent_background: Option<bool>,
            #[serde(default)]
            referenced_image_paths: Option<Vec<String>>,
            #[serde(default)]
            size: Option<String>,
            #[serde(default)]
            quality: Option<String>,
            #[serde(default)]
            gif: Option<Value>,
        }
        let raw: Raw = crate::llm::tools::parse_args(arguments)?;
        let prompt = raw.prompt.unwrap_or_default().trim().to_string();
        if prompt.is_empty() {
            return Err("the prompt is empty — say what to generate or what to change".to_string());
        }
        let referenced_image_paths = raw.referenced_image_paths.unwrap_or_default();
        if referenced_image_paths.len() > MAX_REFERENCED_IMAGES {
            return Err(format!(
                "`referenced_image_paths` takes at most {MAX_REFERENCED_IMAGES} images, not {}",
                referenced_image_paths.len()
            ));
        }
        Ok(Self {
            prompt,
            transparent_background: raw.transparent_background.unwrap_or(false),
            referenced_image_paths,
            size: ImageSize::parse(raw.size.as_deref().unwrap_or_default())?,
            quality: ImageQuality::parse(raw.quality.as_deref().unwrap_or_default())?,
            gif: GifOptions::parse(raw.gif)?,
        })
    }
}

/// One request to the Images endpoints, owned so it can be streamed
/// ([`crate::llm::body::BodySource`]): the reference images ride as the
/// session's shared `data:` URL encodings, borrowed rather than copied.
#[derive(Clone)]
pub(crate) struct ImageRequest {
    pub prompt: String,
    pub transparent_background: bool,
    pub images: Vec<AttachmentUrl>,
    /// [`IMAGE_MODEL`], or [`FALLBACK_IMAGE_MODEL`] once the backend refused it.
    pub model: &'static str,
    pub size: ImageSize,
    pub quality: ImageQuality,
    pub gif: Option<GifOptions>,
}

impl ImageRequest {
    /// The path under the backend's base: `images/generations` for a new
    /// image, `images/edits` when there is anything to edit.
    pub(crate) fn endpoint(&self) -> &'static str {
        if self.images.is_empty() {
            "images/generations"
        } else {
            "images/edits"
        }
    }
}

impl crate::llm::body::BodySource for ImageRequest {
    fn write_to(&self, out: &mut dyn std::io::Write) -> serde_json::Result<()> {
        #[derive(serde::Serialize)]
        struct Reference<'a> {
            image_url: &'a AttachmentUrl,
        }
        // The Images API's fields: `background` is never `auto`, and a GIF
        // is its format with the frames beside it as siblings, each left out
        // for the backend's default (`docs/imagegen.md`).
        #[derive(serde::Serialize)]
        struct Body<'a> {
            prompt: &'a str,
            #[serde(skip_serializing_if = "Vec::is_empty")]
            images: Vec<Reference<'a>>,
            background: &'static str,
            model: &'static str,
            quality: &'static str,
            size: String,
            #[serde(skip_serializing_if = "Option::is_none")]
            output_format: Option<&'static str>,
            #[serde(skip_serializing_if = "Option::is_none")]
            frame_count: Option<u32>,
            #[serde(skip_serializing_if = "Option::is_none")]
            frame_duration_ms: Option<u32>,
        }
        let body = Body {
            prompt: &self.prompt,
            images: self
                .images
                .iter()
                .map(|image_url| Reference { image_url })
                .collect(),
            background: if self.transparent_background {
                "transparent"
            } else {
                "opaque"
            },
            model: self.model,
            quality: self.quality.wire(),
            size: self.size.wire(),
            output_format: self.gif.map(|_| "gif"),
            frame_count: self.gif.and_then(|gif| gif.frame_count),
            frame_duration_ms: self.gif.and_then(|gif| gif.frame_duration_ms),
        };
        serde_json::to_writer(out, &body)
    }
}

/// The head every result of this tool opens with — what tells a generated
/// image's cell from any other output ([`generated_image`]).
const GENERATED_HEAD: &str = "Generated image ";

/// The one fact line a generated image's cell shows and the model reads:
/// `Generated image (PNG, 1536x1024, 2.1 MB) saved to /…/1.png`, an
/// animation's frames after its size — `(GIF, 480x270, 9 frames, 1.1 MB)`.
/// The path is last, so a path full of digits can never be read as the
/// size, and the size is always the second fact ([`generated_image`]).
#[must_use]
pub fn format_generated(
    label: &str,
    size: (u32, u32),
    frames: Option<u32>,
    bytes: u64,
    path: &str,
) -> String {
    let bytes = usize::try_from(bytes).unwrap_or(usize::MAX);
    let frames = frames
        .filter(|frames| *frames > 1)
        .map(|frames| format!(", {frames} frames"))
        .unwrap_or_default();
    format!(
        "{GENERATED_HEAD}({label}, {}x{}{frames}, {}) saved to {path}",
        size.0,
        size.1,
        crate::llm::tools::human_size(bytes)
    )
}

/// What separates a fact line's facts from its file.
const SAVED_TO: &str = ") saved to ";

/// The file and pixel size a [`format_generated`] line names — what the
/// inline picture and the later turns' attachment are rebuilt from.
#[must_use]
pub fn generated_image(output: &str) -> Option<(String, (u32, u32))> {
    let line = output.lines().next()?;
    let facts = line.strip_prefix(GENERATED_HEAD)?.strip_prefix('(')?;
    let (facts, path) = facts.split_once(SAVED_TO)?;
    if path.trim().is_empty() {
        return None;
    }
    // `PNG, 1536x1024, 2.1 MB`: the size is the second fact.
    let dimensions = facts.split(", ").nth(1)?;
    let (width, height) = dimensions.split_once('x')?;
    let (width, height): (u32, u32) = (width.parse().ok()?, height.parse().ok()?);
    (width > 0 && height > 0).then(|| (path.to_string(), (width, height)))
}

/// The facts a [`format_generated`] line opens with —
/// `Generated image (PNG, 1536x1024, 2.1 MB)` — which the cell shows on a
/// row of its own, the file under it (`docs/imagegen.md`).
#[must_use]
pub fn generated_facts(output: &str) -> Option<&str> {
    generated_image(output)?;
    let line = output.lines().next()?;
    let at = line.find(SAVED_TO)?;
    Some(&line[..=at])
}

/// How a generated picture reaches the model, as [`model_result`] words it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Shown {
    /// Attached — as it is, or as a copy shrunk to this size because the
    /// picture itself is past the attachment cap.
    Attached(Option<(u32, u32)>),
    /// Kept back from a model that cannot see images.
    Withheld,
    /// Not attachable: the saved file could not be read back, or no copy fit.
    Unavailable,
}

/// What the model reads for a generated image: the fact line, then the
/// note on where generated images live and what to do with them, then what
/// it can see of the picture — for an `animated` one, the still of its first
/// frame (`docs/images.md` *A GIF goes up as a still*) — and, when the
/// backend refused [`IMAGE_MODEL`], the `model` that made it.
#[must_use]
pub fn model_result(fact: &str, dir: &str, shown: Shown, animated: bool, model: &str) -> String {
    let mut text = format!(
        "{fact}\n\
         Generated images are saved to {dir} by default. If you need the image at another \
         path, copy it there and leave the original in place unless the user explicitly asks \
         you to delete it.\n\
         The user already sees the image in the conversation, so there is no need to show it \
         again in your reply as a Markdown image or a file link."
    );
    match shown {
        Shown::Attached(None) if animated => text.push_str(
            "\nThe picture attached for you is the GIF's first frame, as a still; the file \
             itself is animated.",
        ),
        Shown::Attached(None) => {}
        Shown::Attached(Some((width, height))) if animated => text.push_str(&format!(
            "\nThe picture attached for you is the GIF's first frame, as a still shrunk to \
             {width}x{height} to fit the attachment limit; the file itself is animated and \
             full size."
        )),
        Shown::Attached(Some((width, height))) => text.push_str(&format!(
            "\nThe picture attached for you is a copy shrunk to {width}x{height} to fit the \
             attachment limit; the file itself is full size."
        )),
        Shown::Withheld => text.push_str(
            "\nThis model cannot see images, so the picture is not attached: describe it \
             from the prompt rather than from looking at it.",
        ),
        Shown::Unavailable => text.push_str(
            "\nThe picture could not be attached for you to look at; the file itself is \
             complete.",
        ),
    }
    if model != IMAGE_MODEL {
        text.push_str(&format!(
            "\nThis image was made with {model}: the image backend did not take {IMAGE_MODEL}."
        ));
    }
    text
}

/// The words for a refused request, as the model reads them. `now` is the
/// Unix time, for a usage limit's reset.
#[must_use]
pub(crate) fn failure_text(status: u16, body: &str, now: i64) -> String {
    if status == 429
        && let Some(limit) = usage_limit(body, now)
    {
        return limit.describe(now);
    }
    let message = error_message(body);
    if matches!(status, 401 | 403)
        && let Some(advice) = crate::llm::chatgpt::auth_advice(status, body)
    {
        // The advice says what to do; the endpoint's own words say why,
        // which for a 403 is more than the plan.
        return if message.is_empty() {
            advice
        } else {
            format!("{advice} ChatGPT said: {message}")
        };
    }
    if message.is_empty() {
        format!("image generation failed: HTTP {status}")
    } else {
        format!("image generation failed: HTTP {status}: {message}")
    }
}

/// A 429 that names the plan's image limit, and when it lifts.
struct UsageLimit {
    /// The Unix time it resets, when the body says.
    resets_at: Option<i64>,
}

impl UsageLimit {
    fn describe(&self, now: i64) -> String {
        let base = "The ChatGPT plan's image generation limit is reached";
        match self.resets_at.map(|at| at - now).filter(|left| *left > 0) {
            Some(left) => {
                // To the minute, rounded up: a reset is a wait, not a clock.
                let minutes = u64::try_from(left).unwrap_or(0).div_ceil(60).max(1);
                format!(
                    "{base}; it resets in {}.",
                    crate::app::format_timeout(minutes * 60_000)
                )
            }
            None => format!("{base}."),
        }
    }
}

/// The usage limit a 429's body names — Codex's `usage_limit_reached`, with
/// `resets_at` (Unix seconds) or `resets_in_seconds` (from `now`).
fn usage_limit(body: &str, now: i64) -> Option<UsageLimit> {
    #[derive(Deserialize)]
    struct Envelope {
        error: Option<Limit>,
    }
    #[derive(Deserialize)]
    struct Limit {
        #[serde(rename = "type", default)]
        kind: Option<String>,
        #[serde(default)]
        resets_at: Option<i64>,
        #[serde(default)]
        resets_in_seconds: Option<i64>,
    }
    let error = serde_json::from_str::<Envelope>(body).ok()?.error?;
    if error.kind.as_deref() != Some("usage_limit_reached") {
        return None;
    }
    Some(UsageLimit {
        resets_at: error
            .resets_at
            .or_else(|| error.resets_in_seconds.map(|seconds| now + seconds)),
    })
}

/// A refusal body's message: `error.message` (or a bare `error` string, or
/// `detail`) when the body is JSON saying one, else the body itself on one
/// line, cut short.
fn error_message(body: &str) -> String {
    #[derive(Deserialize)]
    #[serde(untagged)]
    enum ErrorField {
        Text(String),
        Object {
            #[serde(default)]
            message: Option<String>,
        },
    }
    #[derive(Deserialize)]
    struct Envelope {
        #[serde(default)]
        error: Option<ErrorField>,
        #[serde(default)]
        detail: Option<String>,
    }
    let said = serde_json::from_str::<Envelope>(body)
        .ok()
        .and_then(|envelope| match envelope.error {
            Some(ErrorField::Text(text)) => Some(text),
            Some(ErrorField::Object { message }) => message,
            None => envelope.detail,
        });
    let text = said.unwrap_or_else(|| body.to_string());
    let flat = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if flat.chars().count() > 300 {
        let cut: String = flat.chars().take(300).collect();
        format!("{cut}…")
    } else {
        flat
    }
}

// ===========================================================================
// The boundary: the sign-in, the request, the file
// ===========================================================================

/// The ChatGPT Codex sign-in the Images endpoints authenticate with, shared
/// by handle: the boundary keeps it current — at startup, after a `/login`
/// save, after a provider reload — and every call reads it fresh, so a
/// sign-in made mid-session reaches the next call without a rebuild.
/// `None` while nobody is signed in.
#[derive(Debug, Clone, Default)]
pub struct ChatGptSignin(Arc<Mutex<Option<crate::llm::ModelConfig>>>);

impl ChatGptSignin {
    /// Replace the sign-in (`None`: signed out).
    pub fn set(&self, cfg: Option<crate::llm::ModelConfig>) {
        if let Ok(mut slot) = self.0.lock() {
            *slot = cfg;
        }
    }

    /// The sign-in as it is now.
    #[must_use]
    pub fn get(&self) -> Option<crate::llm::ModelConfig> {
        self.0.lock().ok().and_then(|slot| slot.clone())
    }
}

/// What the tool needs from the session: the sign-in, and where generated
/// images are saved (`~/.alter-zero/generated_images/<session>`).
#[derive(Debug, Clone)]
pub struct ImagegenSetup {
    /// The ChatGPT Codex sign-in, shared with the boundary.
    pub signin: ChatGptSignin,
    /// The folder this session's images are saved into, created on the
    /// first one.
    pub dir: PathBuf,
    /// The seeded `imagegen` skill's folder, when there is one: a subagent
    /// whose type may not call the tool is not briefed on it either — the
    /// tool and the listing travel together (`docs/subagents.md`).
    pub skill: Option<PathBuf>,
}

/// Where one request goes and what it carries — the base URL and every
/// header, `authorization` included. Resolved from the sign-in
/// ([`resolve_target`]) apart from the request itself, so the HTTP half is
/// tested against a loopback stand-in with no credential at all.
#[derive(Debug, Clone)]
pub(crate) struct Target {
    pub base: String,
    pub headers: Vec<(String, String)>,
}

/// What a finished request saved.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Saved {
    /// The file, numbered in the session's folder.
    pub path: PathBuf,
    /// The format, read from the file's own content (`PNG`).
    pub label: &'static str,
    /// The pixel size, from the file's header.
    pub size: (u32, u32),
    /// A GIF's frames ([`crate::images::gif_frame_count`]); `None` for a
    /// still.
    pub frames: Option<u32>,
    /// The file's size in bytes.
    pub bytes: u64,
}

/// Why a request made no image.
#[derive(Debug)]
pub(crate) enum ImagegenError {
    /// The backend answered with a non-2xx status.
    Refused { status: u16, body: String },
    /// The request never got an answer.
    Transport(crate::llm::LlmError),
    /// The answer could not be read into an image.
    Scan(scan::ScanError),
    /// The answer held no image.
    NoImage,
    /// The image could not be kept.
    Save(String),
}

impl ImagegenError {
    /// The model's words for it. `now` is the Unix time, for a usage limit.
    fn text(&self, now: i64) -> String {
        match self {
            Self::Refused { status, body } => failure_text(*status, body, now),
            Self::Transport(error) => format!("image generation failed: {error}"),
            Self::Scan(error) => format!("image generation failed: {error}"),
            Self::NoImage => "image generation returned no image data".to_string(),
            Self::Save(reason) => format!("the generated image could not be saved: {reason}"),
        }
    }

    /// Whether the same request may be sent again: nothing reached the
    /// backend, or the backend failed without answering. A usage limit, a
    /// refusal of the request and an answer that came are final — the image
    /// may already be billed.
    fn retryable(&self) -> bool {
        match self {
            Self::Transport(crate::llm::LlmError::Unreachable { .. }) => true,
            Self::Refused { status, .. } => matches!(status, 500 | 502 | 503 | 504),
            _ => false,
        }
    }
}

/// The most a generated image may weigh on disk: the largest
/// `gpt-image-2` output is a few tens of megabytes.
const GENERATED_IMAGE_MAX_BYTES: u64 = 128 * 1024 * 1024;

/// The most of an answer read at all — the image's base64 and the rest.
const RESPONSE_MAX_BYTES: u64 = 256 * 1024 * 1024;

/// The most of a refusal's body kept to be worded.
const REFUSAL_BODY_MAX_BYTES: u64 = 64 * 1024;

/// How long one request may take, start to end. A generation takes tens of
/// seconds to a few minutes; Esc does not wait on this — the call runs on a
/// worker the turn abandons ([`crate::llm::off_thread`]).
const REQUEST_DEADLINE: Duration = Duration::from_secs(600);

/// How many times a request that may be retried is sent again, and the
/// first pause before it (doubled each time).
const RETRIES: u32 = 2;
const RETRY_PAUSE: Duration = Duration::from_secs(2);

/// Send `request` to `target` and save the first image of the answer into
/// `dir` under its next number. One attempt; [`with_retries`] wraps it.
pub(crate) fn request_image(
    target: &Target,
    request: &ImageRequest,
    dir: &Path,
    cancel: &CancelToken,
) -> Result<Saved, ImagegenError> {
    use std::io::Read;
    let client = crate::llm::http_client(crate::llm::openai::NET_OP_TIMEOUT)
        .map_err(ImagegenError::Transport)?;
    let url = format!(
        "{}/{}",
        target.base.trim_end_matches('/'),
        request.endpoint()
    );
    let mut builder = client
        .post(&url)
        .timeout(REQUEST_DEADLINE)
        .header("content-type", "application/json")
        .header("accept", "application/json");
    for (name, value) in &target.headers {
        builder = builder.header(name, value);
    }
    // A new image's body is a few hundred bytes, sent whole so a refused
    // connection is told apart from a failed upload; an edit's carries its
    // pictures and is streamed from the shared encodings, never built whole
    // (`docs/memory.md`).
    builder = if request.images.is_empty() {
        let mut body = Vec::new();
        crate::llm::body::BodySource::write_to(request, &mut body)
            .map_err(|e| ImagegenError::Save(format!("could not encode the request: {e}")))?;
        builder.body(body)
    } else {
        let (reader, len) = crate::llm::body::streamed_request(request.clone())
            .map_err(ImagegenError::Transport)?;
        builder.body(reqwest::blocking::Body::sized(reader, len))
    };
    let mut response = builder
        .send()
        .map_err(|e| ImagegenError::Transport(crate::llm::LlmError::transport(&e)))?;
    let status = response.status().as_u16();
    if !response.status().is_success() {
        let mut body = String::new();
        let _ = (&mut response)
            .take(REFUSAL_BODY_MAX_BYTES)
            .read_to_string(&mut body);
        return Err(ImagegenError::Refused { status, body });
    }
    save_answer(&mut response.take(RESPONSE_MAX_BYTES), dir, cancel)
}

/// Stream an answer's first image into a staging file in `dir`, check that
/// it is a picture, and give it the folder's next number with the extension
/// its content calls for. A refusal or a cancel drops the staging file.
fn save_answer(
    answer: &mut dyn std::io::Read,
    dir: &Path,
    cancel: &CancelToken,
) -> Result<Saved, ImagegenError> {
    use std::io::Write;
    let staging = crate::clipboard::staging_file(dir, "part").map_err(ImagegenError::Save)?;
    let split = {
        let mut out = crate::clipboard::CappedWriter::new(
            std::io::BufWriter::with_capacity(64 * 1024, staging.as_file()),
            GENERATED_IMAGE_MAX_BYTES,
        );
        let split = scan::split_response(answer, &mut out, &|| cancel.is_cancelled())
            .map_err(ImagegenError::Scan)?;
        out.flush()
            .map_err(|e| ImagegenError::Save(format!("could not write the image: {e}")))?;
        split
    };
    if split.images == 0 || split.decoded == 0 {
        return Err(ImagegenError::NoImage);
    }
    let sniffed = sniff(staging.path())
        .map_err(|e| ImagegenError::Save(format!("could not read the image back: {e}")))?;
    let Some((format, (label, ext))) =
        sniffed.and_then(|format| Some((format, format_names(format)?)))
    else {
        return Err(ImagegenError::Save(
            "the answer's image is not a PNG, JPEG, WebP or GIF".to_string(),
        ));
    };
    // Reopened by path: the staging handle sits at the end of what it wrote.
    let mut reader = image::ImageReader::open(staging.path())
        .map_err(|e| ImagegenError::Save(format!("could not read the image back: {e}")))?;
    reader.set_format(format);
    let size = reader
        .into_dimensions()
        .map_err(|e| ImagegenError::Save(format!("the image did not parse: {e}")))?;
    // An animation's frames, counted off its blocks without a pixel decoded.
    let frames = (format == image::ImageFormat::Gif)
        .then(|| std::fs::File::open(staging.path()).ok())
        .flatten()
        .and_then(crate::images::gif_frame_count);
    let path = crate::clipboard::keep_numbered(staging, dir, ext).map_err(ImagegenError::Save)?;
    Ok(Saved {
        bytes: split.decoded,
        path,
        label,
        size,
        frames,
    })
}

/// Save an Images API answer's first image into `dir` under its next number
/// — the streamed path a request's answer takes, never holding the image or
/// its base64 whole. Public for the memory gate
/// (`tests/image_generation_memory.rs`), which has no backend to stream from.
///
/// # Errors
/// An answer with no image, image data that is not base64 or not a picture,
/// or a folder that cannot be written — the reason as text.
pub fn save_generated_answer(
    dir: &Path,
    answer: &mut dyn std::io::Read,
) -> Result<PathBuf, String> {
    save_answer(answer, dir, &CancelToken::new())
        .map(|saved| saved.path)
        .map_err(|error| error.text(0))
}

/// The format the file at `path` is, by its **content** alone — its first
/// bytes' signature, never its name: `ImageReader::with_guessed_format`
/// keeps the extension's guess when the content matches nothing, which
/// would take a text file named `.png` for a picture.
fn sniff(path: &Path) -> std::io::Result<Option<image::ImageFormat>> {
    use std::io::Read;
    let mut head = Vec::with_capacity(64);
    std::fs::File::open(path)?.take(64).read_to_end(&mut head)?;
    Ok(image::guess_format(&head).ok())
}

/// The label a fact line names a format by, and the extension its file
/// takes — `None` for anything else.
fn format_names(format: image::ImageFormat) -> Option<(&'static str, &'static str)> {
    match format {
        image::ImageFormat::Png => Some(("PNG", "png")),
        image::ImageFormat::Jpeg => Some(("JPEG", "jpg")),
        image::ImageFormat::WebP => Some(("WebP", "webp")),
        image::ImageFormat::Gif => Some(("GIF", "gif")),
        _ => None,
    }
}

/// Run `attempt` until it succeeds, fails for good, or has been retried
/// [`RETRIES`] times, pausing `pause` (doubled each time) between tries and
/// giving up at once on a cancel.
fn with_retries<T>(
    pause: Duration,
    cancel: &CancelToken,
    mut attempt: impl FnMut() -> Result<T, ImagegenError>,
) -> Result<T, ImagegenError> {
    let mut wait = pause;
    let mut tries = 0;
    loop {
        match attempt() {
            Err(error) if error.retryable() && tries < RETRIES => {
                tries += 1;
                crate::llm::retry::sleep_cancellable(wait, cancel);
                if cancel.is_cancelled() {
                    return Err(error);
                }
                wait *= 2;
            }
            other => return other,
        }
    }
}

/// Whether a refusal says the backend does not take `model` itself — the one
/// refusal worth asking again with [`FALLBACK_IMAGE_MODEL`]: a bad request
/// answered at once, so nothing was made or billed. The error's `param` says
/// so when there is one (`model`, or the other field it is about instead),
/// else a code naming an unknown model, else the error's words naming the
/// model.
fn refuses_model(status: u16, body: &str, model: &str) -> bool {
    #[derive(Deserialize, Default)]
    struct Fields {
        #[serde(default)]
        param: Option<String>,
        #[serde(default)]
        code: Option<String>,
    }
    #[derive(Deserialize)]
    #[serde(untagged)]
    enum ErrorField {
        Fields(Fields),
        Other(serde::de::IgnoredAny),
    }
    #[derive(Deserialize)]
    struct Envelope {
        #[serde(default)]
        error: Option<ErrorField>,
    }
    if !matches!(status, 400 | 404 | 422) {
        return false;
    }
    let fields = serde_json::from_str::<Envelope>(body)
        .ok()
        .and_then(|envelope| match envelope.error {
            Some(ErrorField::Fields(fields)) => Some(fields),
            _ => None,
        })
        .unwrap_or_default();
    if let Some(param) = fields.param.filter(|param| !param.is_empty()) {
        return param == "model";
    }
    if fields.code.as_deref().is_some_and(|code| {
        matches!(
            code,
            "model_not_found" | "invalid_model" | "unsupported_model"
        )
    }) {
        return true;
    }
    error_message(body).contains(model)
}

/// Make the image with [`IMAGE_MODEL`] and, when the backend refuses that
/// model by name (`refuses_model`), once more with [`FALLBACK_IMAGE_MODEL`],
/// handing back the model that made it (`docs/imagegen.md` *The model*). Any
/// other failure stands: another model would be refused the same way.
fn with_model_fallback<T>(
    mut attempt: impl FnMut(&'static str) -> Result<T, ImagegenError>,
) -> Result<(T, &'static str), ImagegenError> {
    match attempt(IMAGE_MODEL) {
        Ok(made) => Ok((made, IMAGE_MODEL)),
        Err(ImagegenError::Refused { status, body })
            if refuses_model(status, &body, IMAGE_MODEL) =>
        {
            attempt(FALLBACK_IMAGE_MODEL).map(|made| (made, FALLBACK_IMAGE_MODEL))
        }
        Err(error) => Err(error),
    }
}

/// Where the sign-in's requests go and the headers they carry — the bearer
/// minted from the stored refresh token through the cache every ChatGPT
/// request shares (`docs/chatgpt.md`), the provider's own headers, the
/// account the backend routes on, and the two Codex sends with every image
/// request. A Free-plan account is refused here, without a request: Codex
/// does not offer the tool there.
///
/// # Errors
/// No sign-in, a refused mint, or a Free plan — each a sentence the model
/// reads.
fn resolve_target(cfg: &crate::llm::ModelConfig) -> Result<Target, String> {
    if cfg.auth != crate::llm::AuthScheme::ChatGptCodex {
        return Err(crate::llm::auth::NOT_SIGNED_IN.to_string());
    }
    let Some(refresh) = cfg.api_key.as_deref().filter(|k| !k.is_empty()) else {
        return Err(crate::llm::auth::NOT_SIGNED_IN.to_string());
    };
    let access = crate::llm::chatgpt::authorize(refresh).map_err(|e| e.to_string())?;
    if access.plan.as_deref() == Some("free") {
        return Err(FREE_PLAN.to_string());
    }
    // A cache hit: the mint above just filled it.
    let auth = crate::llm::auth::request_auth(cfg).map_err(|e| e.to_string())?;
    let Some(bearer) = auth.bearer else {
        return Err(crate::llm::auth::NOT_SIGNED_IN.to_string());
    };
    let mut headers = vec![("authorization".to_string(), format!("Bearer {bearer}"))];
    headers.extend(cfg.extra_headers.iter().cloned());
    headers.extend(auth.headers);
    headers.push((
        "x-codex-image-turn-id".to_string(),
        crate::llm::cloud_code::uuid_v4(),
    ));
    Ok(Target {
        base: auth.base.unwrap_or_else(|| cfg.api_base.clone()),
        headers,
    })
}

/// The refusal for a call this session does not offer the tool for.
pub const UNAVAILABLE: &str = "image generation is not available here — it needs the ChatGPT \
     Codex sign-in: run /login and choose Sign in with an account";

/// The refusal for an account on the Free plan.
const FREE_PLAN: &str =
    "image generation needs a paid ChatGPT plan, and this account is on the Free plan";

/// Check the images a call names and resolve each to an absolute path: a
/// PNG, JPEG, WebP or GIF judged by its content, under the API's
/// [`MAX_REFERENCE_BYTES`] — a GIF goes up as a still of its first frame, as
/// every GIF sent anywhere does (`docs/images.md` *A GIF goes up as a
/// still*). A relative path resolves against the working directory, the
/// file tools' rule.
///
/// # Errors
/// The first image that is missing, not a file, too large, or not one of
/// the four formats — named, so the model can fix the call.
pub(crate) fn resolve_references(paths: &[String]) -> Result<Vec<PathBuf>, String> {
    paths
        .iter()
        .map(|raw| {
            let path = std::path::absolute(raw)
                .map_err(|e| format!("could not resolve the image path {raw}: {e}"))?;
            let shown = path.display();
            let meta = std::fs::metadata(&path)
                .map_err(|e| format!("could not read the referenced image {shown}: {e}"))?;
            if !meta.is_file() {
                return Err(format!("the referenced image {shown} is not a file"));
            }
            if meta.len() > MAX_REFERENCE_BYTES {
                return Err(format!(
                    "the referenced image {shown} is larger than the 50 MB an image may be"
                ));
            }
            let format = sniff(&path)
                .map_err(|e| format!("could not read the referenced image {shown}: {e}"))?;
            match format {
                Some(
                    image::ImageFormat::Png
                    | image::ImageFormat::Jpeg
                    | image::ImageFormat::WebP
                    | image::ImageFormat::Gif,
                ) => Ok(path),
                _ => Err(format!(
                    "the referenced image {shown} is not a PNG, JPEG, WebP or GIF image"
                )),
            }
        })
        .collect()
}

/// What a saved image hands the model to look at.
enum Attachment {
    /// The picture, or the copy held to the attachment cap sent in its place.
    Attached(crate::images::FittedAttachment),
    /// Nothing: the model cannot see images.
    Withheld,
    /// Nothing: the file could not be read back, or no copy fit.
    Unavailable,
}

impl Attachment {
    /// The attachment for the image saved at `path` — held to the limit an
    /// image `read` is held to, since a provider refuses a picture past it and
    /// the conversation re-sends every picture it holds on every turn
    /// (`images::fitted_attachment_data_url`). `vision` is the model's image
    /// support: a model known not to see gets none.
    fn for_saved(path: &Path, vision: Option<bool>) -> Self {
        if vision == Some(false) {
            return Self::Withheld;
        }
        crate::images::fitted_attachment_data_url(path, crate::llm::tools::READ_IMAGE_MAX_BYTES)
            .map_or(Self::Unavailable, Self::Attached)
    }
}

/// The outcome for a saved image: the fact line on the cell, the fact and
/// the note for the model — which says what it is looking at of an
/// animation, and which model made the image when it was not
/// [`IMAGE_MODEL`] — and the picture attached when there is one to attach,
/// or, for a model that cannot see, the mark that has the agent loop send
/// the note the replay sends in its place.
fn finished(saved: &Saved, dir: &Path, attachment: Attachment, model: &str) -> ToolOutcome {
    let fact = format_generated(
        saved.label,
        saved.size,
        saved.frames,
        saved.bytes,
        &saved.path.to_string_lossy(),
    );
    let shown = match &attachment {
        Attachment::Attached(fitted) => Shown::Attached(fitted.sent),
        Attachment::Withheld => Shown::Withheld,
        Attachment::Unavailable => Shown::Unavailable,
    };
    let animated = saved.frames.is_some_and(|frames| frames > 1);
    let note = model_result(&fact, &dir.to_string_lossy(), shown, animated, model);
    let outcome = ToolOutcome::ok(fact).with_context(note);
    match attachment {
        Attachment::Attached(fitted) => outcome.with_image(fitted.url),
        Attachment::Withheld => outcome.with_image_withheld(),
        Attachment::Unavailable => outcome,
    }
}

/// Run one `imagegen` call (`docs/imagegen.md` *The call*): check the
/// arguments and the images they name, then mint, send and save on a worker
/// thread the turn abandons on Esc, and hand back the fact line, the model's
/// note and the picture. `vision` is the model's image support — a model
/// known not to see gets the file and no attachment.
pub(crate) fn run_imagegen(
    arguments: &str,
    setup: &ImagegenSetup,
    vision: Option<bool>,
    cancel: &CancelToken,
) -> ToolOutcome {
    let args = match ImagegenArgs::parse(arguments) {
        Ok(args) => args,
        Err(error) => return ToolOutcome::error(error),
    };
    let references = match resolve_references(&args.referenced_image_paths) {
        Ok(references) => references,
        Err(error) => return ToolOutcome::error(error),
    };
    let Some(cfg) = setup.signin.get() else {
        return ToolOutcome::error(crate::llm::auth::NOT_SIGNED_IN);
    };
    let dir = setup.dir.clone();
    let worker_dir = dir.clone();
    let worker_cancel = cancel.clone();
    let result = crate::llm::off_thread(
        move || -> Result<(Saved, Attachment, &'static str), String> {
            let target = resolve_target(&cfg)?;
            let images = references
                .iter()
                .map(|path| {
                    crate::images::attachment_data_url(path).ok_or_else(|| {
                        format!("could not read the referenced image {}", path.display())
                    })
                })
                .collect::<Result<Vec<_>, _>>()?;
            let request = ImageRequest {
                prompt: args.prompt,
                transparent_background: args.transparent_background,
                images,
                model: IMAGE_MODEL,
                size: args.size,
                quality: args.quality,
                gif: args.gif,
            };
            let (saved, model) = with_model_fallback(|model| {
                let asked = ImageRequest {
                    model,
                    ..request.clone()
                };
                with_retries(RETRY_PAUSE, &worker_cancel, || {
                    request_image(&target, &asked, &worker_dir, &worker_cancel)
                })
            })
            .map_err(|error| error.text(unix_now()))?;
            // Encoded here, on the worker the turn abandons on Esc: a picture
            // past the cap is decoded and shrunk.
            let attachment = Attachment::for_saved(&saved.path, vision);
            Ok((saved, attachment, model))
        },
        cancel,
    );
    match result {
        None => unanswered(cancel),
        Some(Err(error)) => ToolOutcome::error(error),
        Some(Ok((saved, attachment, model))) => {
            let shown = saved.path.to_string_lossy().into_owned();
            // The cell this call resolves draws the picture from the file
            // (`docs/images.md`).
            crate::images::remember_size(&shown, saved.size);
            finished(&saved, &dir, attachment, model)
        }
    }
}

/// The answer to a call whose worker ended without one: an interrupt when
/// Esc ended it, else a failure — a worker that died is not anything the
/// user did.
fn unanswered(cancel: &CancelToken) -> ToolOutcome {
    if cancel.is_cancelled() {
        ToolOutcome::error(crate::app::INTERRUPT_TOOL_OUTPUT)
            .with_error_kind(crate::stream::ToolErrorKind::Interrupted)
    } else {
        ToolOutcome::error(STOPPED_UNEXPECTEDLY)
    }
}

/// What a call whose worker died answers.
const STOPPED_UNEXPECTEDLY: &str = "image generation failed: the request stopped unexpectedly";

/// Seconds since the Unix epoch, for a usage limit's reset.
fn unix_now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| i64::try_from(d.as_secs()).unwrap_or(i64::MAX))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::llm::body::BodySource;

    fn body(request: &ImageRequest) -> Value {
        let mut out = Vec::new();
        request.write_to(&mut out).unwrap();
        serde_json::from_slice(&out).unwrap()
    }

    #[test]
    fn the_spec_is_a_function_with_a_closed_object_schema() {
        let spec = imagegen_spec();
        assert_eq!(spec["type"], "function");
        assert_eq!(spec["function"]["name"], IMAGEGEN_TOOL_NAME);
        let params = &spec["function"]["parameters"];
        assert_eq!(params["type"], "object");
        assert_eq!(params["required"], json!(["prompt"]));
        assert_eq!(params["additionalProperties"], false);
        assert_eq!(params["properties"]["prompt"]["type"], "string");
        assert_eq!(
            params["properties"]["transparent_background"]["type"],
            "boolean"
        );
        assert_eq!(
            params["properties"]["referenced_image_paths"]["type"],
            "array"
        );
        assert_eq!(
            params["properties"]["referenced_image_paths"]["items"]["type"],
            "string"
        );
        let names: Vec<&str> = params["properties"]
            .as_object()
            .unwrap()
            .keys()
            .map(String::as_str)
            .collect();
        assert_eq!(
            names,
            [
                "prompt",
                "transparent_background",
                "referenced_image_paths",
                "size",
                "quality",
                "gif"
            ]
        );
        // No model and no count: one image per call, the model this tool
        // names (docs/imagegen.md).
        let properties = &params["properties"];
        assert_eq!(properties["size"]["type"], "string");
        assert_eq!(
            properties["quality"]["enum"],
            json!(["auto", "low", "medium", "high"])
        );
        let gif = &properties["gif"];
        assert_eq!(gif["type"], "object");
        assert_eq!(gif["additionalProperties"], false);
        assert_eq!(gif["properties"]["frame_count"]["type"], "integer");
        assert_eq!(gif["properties"]["frame_count"]["minimum"], 2);
        assert_eq!(gif["properties"]["frame_count"]["maximum"], 16);
        assert_eq!(gif["properties"]["frame_duration_ms"]["type"], "integer");
        assert_eq!(gif["properties"]["frame_duration_ms"]["minimum"], 20);
        assert_eq!(gif["properties"]["frame_duration_ms"]["maximum"], 2000);
        for field in ["frame_count", "frame_duration_ms"] {
            let text = gif["properties"][field]["description"].as_str().unwrap();
            assert!(text.ends_with('.') && text.len() < 110, "{field}: {text}");
        }
    }

    #[test]
    fn the_description_says_what_the_tool_is_for_and_names_this_apps_tools() {
        let spec = imagegen_spec();
        let description = spec["function"]["description"].as_str().unwrap();
        assert!(description.contains("ChatGPT"), "{description}");
        // Every word rides every request that offers the tool; the craft —
        // when to draw, how to word a prompt — is the skill's (CLAUDE.md's
        // tool prose rule, docs/imagegen.md).
        let words = description.split_whitespace().count();
        assert!(words <= 100, "{words} words: {description}");
        assert!(description.contains("path"), "names what it returns");
        assert!(
            description.contains("`referenced_image_paths`"),
            "{description}"
        );
        assert!(
            description.contains("`transparent_background`"),
            "{description}"
        );
        // A GIF is offered, with its one sharp edge: an animation is
        // changed through a spritesheet of its frames, never by editing the
        // GIF itself (docs/imagegen.md).
        assert!(description.contains("`gif`"), "{description}");
        assert!(description.contains("spritesheet"), "{description}");
        // This app looks at a local image with `read`; another client's
        // `view_image`, code mode and image counts are not here.
        assert!(description.contains("`read`"), "{description}");
        for absent in ["view_image", "num_last_images_to_include", "@exec"] {
            assert!(!description.contains(absent), "{absent}: {description}");
        }
        // Every parameter is one short sentence (CLAUDE.md's tool prose rule).
        for (name, param) in spec["function"]["parameters"]["properties"]
            .as_object()
            .unwrap()
        {
            let text = param["description"].as_str().unwrap();
            assert!(text.ends_with('.'), "{name}: {text}");
            assert!(text.len() < 110, "{name}: {text}");
        }
    }

    #[test]
    fn arguments_parse_with_their_defaults() {
        assert_eq!(
            ImagegenArgs::parse(r#"{"prompt": "a red fox at dawn"}"#).unwrap(),
            ImagegenArgs {
                prompt: "a red fox at dawn".to_string(),
                transparent_background: false,
                referenced_image_paths: Vec::new(),
                size: ImageSize::Auto,
                quality: ImageQuality::Auto,
                gif: None,
            }
        );
        assert_eq!(
            ImagegenArgs::parse(
                r#"{"prompt": "add a hat", "transparent_background": true,
                    "referenced_image_paths": ["/tmp/fox.png"]}"#
            )
            .unwrap(),
            ImagegenArgs {
                prompt: "add a hat".to_string(),
                transparent_background: true,
                referenced_image_paths: vec!["/tmp/fox.png".to_string()],
                size: ImageSize::Auto,
                quality: ImageQuality::Auto,
                gif: None,
            }
        );
    }

    #[test]
    fn a_null_optional_argument_reads_as_its_default() {
        let args = ImagegenArgs::parse(
            r#"{"prompt": "x", "transparent_background": null, "referenced_image_paths": null,
                "size": null, "quality": null, "gif": null}"#,
        )
        .unwrap();
        assert!(!args.transparent_background);
        assert!(args.referenced_image_paths.is_empty());
        assert_eq!(args.size, ImageSize::Auto);
        assert_eq!(args.quality, ImageQuality::Auto);
        assert_eq!(args.gif, None);
    }

    fn parse_size(size: &str) -> Result<ImageSize, String> {
        ImagegenArgs::parse(&json!({"prompt": "x", "size": size}).to_string()).map(|a| a.size)
    }

    #[test]
    fn size_is_auto_or_width_by_height() {
        for auto in ["auto", "AUTO", " auto ", ""] {
            assert_eq!(parse_size(auto), Ok(ImageSize::Auto), "{auto:?}");
        }
        assert_eq!(parse_size("1536x1024"), Ok(ImageSize::Exact(1536, 1024)));
        assert_eq!(parse_size("1024X1536"), Ok(ImageSize::Exact(1024, 1536)));
        assert_eq!(
            parse_size(" 2048 x 1152 "),
            Ok(ImageSize::Exact(2048, 1152))
        );
        assert_eq!(parse_size("3840x2160"), Ok(ImageSize::Exact(3840, 2160)));
        assert_eq!(parse_size("2160x3840"), Ok(ImageSize::Exact(2160, 3840)));
        assert_eq!(ImageSize::Exact(1536, 1024).wire(), "1536x1024");
        assert_eq!(ImageSize::Auto.wire(), "auto");
    }

    #[test]
    fn a_size_the_backend_would_refuse_is_refused_with_its_rule() {
        // Refused before a minute is spent waiting on the same answer.
        for shapeless in [
            "big",
            "1536",
            "1536x",
            "x1024",
            "0x512",
            "1536x1024x2",
            "-16x16",
        ] {
            let error = parse_size(shapeless).unwrap_err();
            assert!(error.contains("`size`"), "{shapeless}: {error}");
            assert!(error.contains("WIDTHxHEIGHT"), "{shapeless}: {error}");
        }
        // Edges off the 16-pixel grid: the nearest size on it is offered.
        let error = parse_size("1920x1080").unwrap_err();
        assert!(error.contains("multiples of 16"), "{error}");
        assert!(error.contains("1920x1088"), "{error}");
        let error = parse_size("4096x2048").unwrap_err();
        assert!(error.contains("3840"), "{error}");
        let error = parse_size("3840x1024").unwrap_err();
        assert!(error.contains("3 times"), "{error}");
        // Within every edge rule, past the pixel count 3840x2160 has.
        let error = parse_size("2896x2880").unwrap_err();
        assert!(error.contains("8,294,400"), "{error}");
    }

    fn parse_quality(quality: &str) -> Result<ImageQuality, String> {
        ImagegenArgs::parse(&json!({"prompt": "x", "quality": quality}).to_string())
            .map(|a| a.quality)
    }

    #[test]
    fn quality_is_one_of_four() {
        assert_eq!(parse_quality("auto"), Ok(ImageQuality::Auto));
        assert_eq!(parse_quality(""), Ok(ImageQuality::Auto));
        assert_eq!(parse_quality("low"), Ok(ImageQuality::Low));
        assert_eq!(parse_quality("MEDIUM"), Ok(ImageQuality::Medium));
        assert_eq!(parse_quality(" high "), Ok(ImageQuality::High));
        let error = parse_quality("ultra").unwrap_err();
        assert!(error.contains("`quality`"), "{error}");
        for value in ["`auto`", "`low`", "`medium`", "`high`"] {
            assert!(error.contains(value), "{value}: {error}");
        }
        assert_eq!(ImageQuality::Medium.wire(), "medium");
    }

    fn parse_gif(gif: Value) -> Result<Option<GifOptions>, String> {
        ImagegenArgs::parse(&json!({"prompt": "x", "gif": gif}).to_string()).map(|a| a.gif)
    }

    #[test]
    fn a_gif_takes_its_frames_and_how_long_each_shows() {
        assert_eq!(parse_gif(json!({})), Ok(Some(GifOptions::default())));
        assert_eq!(
            parse_gif(json!({"frame_count": 4, "frame_duration_ms": 120})),
            Ok(Some(GifOptions {
                frame_count: Some(4),
                frame_duration_ms: Some(120),
            }))
        );
        assert_eq!(
            parse_gif(json!({"frame_count": 16, "frame_duration_ms": null})),
            Ok(Some(GifOptions {
                frame_count: Some(16),
                frame_duration_ms: None,
            }))
        );
        // A model that says yes or no to a GIF is understood.
        assert_eq!(parse_gif(json!(true)), Ok(Some(GifOptions::default())));
        assert_eq!(parse_gif(json!(false)), Ok(None));
        assert_eq!(parse_gif(Value::Null), Ok(None));
    }

    #[test]
    fn a_gif_the_backend_would_refuse_is_refused_by_its_field() {
        for frames in [json!(1), json!(17), json!("4"), json!(2.5), json!(-3)] {
            let error = parse_gif(json!({"frame_count": frames})).unwrap_err();
            assert!(error.contains("`gif.frame_count`"), "{frames}: {error}");
            assert!(error.contains("2 to 16"), "{frames}: {error}");
        }
        for ms in [json!(10), json!(2001)] {
            let error = parse_gif(json!({"frame_duration_ms": ms})).unwrap_err();
            assert!(error.contains("`gif.frame_duration_ms`"), "{ms}: {error}");
            assert!(error.contains("20 to 2000"), "{ms}: {error}");
        }
        // A field it does not take does nothing, so it is refused by name.
        let error = parse_gif(json!({"loop": 0})).unwrap_err();
        assert!(error.contains("`loop`"), "{error}");
        assert!(error.contains("`frame_count`"), "{error}");
        let error = parse_gif(json!("yes")).unwrap_err();
        assert!(error.contains("`gif`"), "{error}");
    }

    #[test]
    fn an_argument_the_tool_does_not_take_is_refused_by_name() {
        // A model that passes a count learns at once that it does nothing.
        let error = ImagegenArgs::parse(r#"{"prompt": "x", "n": 2}"#).unwrap_err();
        assert!(error.contains("`n`"), "{error}");
        assert!(error.contains("`referenced_image_paths`"), "{error}");
        assert!(error.contains("`gif`"), "{error}");
    }

    #[test]
    fn a_blank_prompt_is_refused() {
        for args in [r#"{"prompt": "  "}"#, r"{}", ""] {
            let error = ImagegenArgs::parse(args).unwrap_err();
            assert!(error.contains("prompt"), "{args}: {error}");
        }
    }

    #[test]
    fn more_references_than_the_limit_are_refused() {
        let paths: Vec<String> = (0..=MAX_REFERENCED_IMAGES)
            .map(|i| format!("\"/tmp/{i}.png\""))
            .collect();
        let args = format!(
            r#"{{"prompt": "x", "referenced_image_paths": [{}]}}"#,
            paths.join(",")
        );
        let error = ImagegenArgs::parse(&args).unwrap_err();
        assert!(
            error.contains(&MAX_REFERENCED_IMAGES.to_string()),
            "{error}"
        );
    }

    /// A request for `prompt` with every choice at its default.
    fn request(
        prompt: &str,
        transparent_background: bool,
        images: Vec<AttachmentUrl>,
    ) -> ImageRequest {
        ImageRequest {
            prompt: prompt.to_string(),
            transparent_background,
            images,
            model: IMAGE_MODEL,
            size: ImageSize::Auto,
            quality: ImageQuality::Auto,
            gif: None,
        }
    }

    #[test]
    fn a_new_image_is_a_generation_with_the_newest_model() {
        let request = request("paint a blue whale", false, Vec::new());
        assert_eq!(request.endpoint(), "images/generations");
        assert_eq!(
            body(&request),
            json!({"prompt": "paint a blue whale", "background": "opaque",
                   "model": "gpt-image-2.6-sunburst", "quality": "auto", "size": "auto"})
        );
    }

    #[test]
    fn an_edit_carries_its_images_as_data_urls() {
        let request = request(
            "add a red hat",
            true,
            vec![AttachmentUrl::from(
                "data:image/png;base64,Zm9v".to_string(),
            )],
        );
        assert_eq!(request.endpoint(), "images/edits");
        assert_eq!(
            body(&request),
            json!({"prompt": "add a red hat",
                   "images": [{"image_url": "data:image/png;base64,Zm9v"}],
                   "background": "transparent", "model": "gpt-image-2.6-sunburst",
                   "quality": "auto", "size": "auto"})
        );
    }

    #[test]
    fn a_size_and_a_quality_ride_the_body_as_asked() {
        let mut asked = request("a poster", false, Vec::new());
        asked.size = ImageSize::Exact(1024, 1536);
        asked.quality = ImageQuality::High;
        let sent = body(&asked);
        assert_eq!(sent["size"], "1024x1536");
        assert_eq!(sent["quality"], "high");
    }

    #[test]
    fn a_gif_is_asked_for_as_its_format_beside_its_frames() {
        // The Images API takes the format and its controls as siblings.
        let mut animated = request("a waving mascot", true, Vec::new());
        animated.gif = Some(GifOptions {
            frame_count: Some(4),
            frame_duration_ms: Some(120),
        });
        assert_eq!(
            body(&animated),
            json!({"prompt": "a waving mascot", "background": "transparent",
                   "model": "gpt-image-2.6-sunburst", "quality": "auto", "size": "auto",
                   "output_format": "gif", "frame_count": 4, "frame_duration_ms": 120})
        );
        // `{}`: the backend's own defaults, so nothing but the format.
        animated.gif = Some(GifOptions::default());
        let sent = body(&animated);
        assert_eq!(sent["output_format"], "gif");
        assert!(sent.get("frame_count").is_none(), "{sent}");
        assert!(sent.get("frame_duration_ms").is_none(), "{sent}");
        // A still names no format at all: PNG is the default.
        assert!(
            body(&request("x", false, Vec::new()))
                .get("output_format")
                .is_none()
        );
    }

    #[test]
    fn only_a_refusal_of_the_model_itself_reads_as_one() {
        let named = r#"{"error": {"message": "Invalid value: 'gpt-image-2.6-sunburst'.", "type": "invalid_request_error", "param": "model", "code": "invalid_value"}}"#;
        assert!(refuses_model(400, named, IMAGE_MODEL));
        let unknown =
            r#"{"error": {"message": "The model does not exist.", "code": "model_not_found"}}"#;
        assert!(refuses_model(404, unknown, IMAGE_MODEL));
        // A body that only says which model, in words.
        let worded = r#"{"detail": "Unsupported model gpt-image-2.6-sunburst"}"#;
        assert!(refuses_model(400, worded, IMAGE_MODEL));
        assert!(refuses_model(
            422,
            "model gpt-image-2.6-sunburst is not available",
            IMAGE_MODEL
        ));
        // Another field refused, even one whose message names the model.
        let size = r#"{"error": {"message": "gpt-image-2.6-sunburst: 512x512 is too small", "param": "size"}}"#;
        assert!(!refuses_model(400, size, IMAGE_MODEL));
        assert!(!refuses_model(
            400,
            r#"{"error": {"message": "bad prompt"}}"#,
            IMAGE_MODEL
        ));
        // A limit, the plan, a backend failing: never the model's fault.
        assert!(!refuses_model(429, worded, IMAGE_MODEL));
        assert!(!refuses_model(403, worded, IMAGE_MODEL));
        assert!(!refuses_model(500, worded, IMAGE_MODEL));
    }

    #[test]
    fn a_model_the_backend_refuses_is_asked_for_again_as_the_one_before_it() {
        let refused = || ImagegenError::Refused {
            status: 400,
            body: r#"{"error": {"param": "model", "message": "unknown model"}}"#.to_string(),
        };
        let asked = std::cell::RefCell::new(Vec::new());
        let made = with_model_fallback(|model| {
            asked.borrow_mut().push(model);
            if model == IMAGE_MODEL {
                Err(refused())
            } else {
                Ok(7)
            }
        });
        assert_eq!(made.unwrap(), (7, FALLBACK_IMAGE_MODEL));
        assert_eq!(*asked.borrow(), [IMAGE_MODEL, FALLBACK_IMAGE_MODEL]);
        // Made with the newest: asked once.
        asked.borrow_mut().clear();
        let made = with_model_fallback(|model| {
            asked.borrow_mut().push(model);
            Ok(1)
        });
        assert_eq!(made.unwrap(), (1, IMAGE_MODEL));
        assert_eq!(*asked.borrow(), [IMAGE_MODEL]);
        // Any other refusal stands: asking another model would not change it.
        asked.borrow_mut().clear();
        let made: Result<((), _), _> = with_model_fallback(|model| {
            asked.borrow_mut().push(model);
            Err(ImagegenError::Refused {
                status: 400,
                body: r#"{"error": {"param": "size", "message": "too small"}}"#.to_string(),
            })
        });
        assert!(made.is_err());
        assert_eq!(*asked.borrow(), [IMAGE_MODEL]);
    }

    #[test]
    fn an_animations_fact_line_names_its_frames() {
        let line = format_generated("GIF", (480, 270), Some(9), 1_200_000, "/g/1.gif");
        assert_eq!(
            line,
            "Generated image (GIF, 480x270, 9 frames, 1.1 MB) saved to /g/1.gif"
        );
        assert_eq!(
            generated_image(&line),
            Some(("/g/1.gif".to_string(), (480, 270)))
        );
        assert_eq!(
            generated_facts(&line),
            Some("Generated image (GIF, 480x270, 9 frames, 1.1 MB)")
        );
        // One frame is no animation.
        assert_eq!(
            format_generated("GIF", (8, 8), Some(1), 70, "/g/2.gif"),
            "Generated image (GIF, 8x8, 70 B) saved to /g/2.gif"
        );
    }

    #[test]
    fn the_fact_line_round_trips_its_path_and_size() {
        let path = "/home/me/.alter-zero/generated_images/17638x12/1.png";
        let line = format_generated("PNG", (1536, 1024), None, 2_200_000, path);
        assert_eq!(
            line,
            "Generated image (PNG, 1536x1024, 2.1 MB) saved to \
             /home/me/.alter-zero/generated_images/17638x12/1.png"
        );
        // The path's own `17638x12` is never mistaken for the size.
        assert_eq!(
            generated_image(&line),
            Some((path.to_string(), (1536, 1024)))
        );
    }

    #[test]
    fn the_facts_are_the_fact_line_up_to_its_file() {
        let line = format_generated("PNG", (1536, 1024), None, 2_200_000, "/g/1.png");
        assert_eq!(
            generated_facts(&line),
            Some("Generated image (PNG, 1536x1024, 2.1 MB)")
        );
        assert_eq!(generated_facts("Read image (PNG, 1x1, 1 B)"), None);
    }

    #[test]
    fn only_a_generated_images_fact_line_parses() {
        assert_eq!(generated_image("Read image (PNG, 512x512, 17 KB)"), None);
        assert_eq!(generated_image("image generation failed: HTTP 500"), None);
        assert_eq!(
            generated_image("Generated image (PNG, 0x5, 1 KB) saved to /x"),
            None
        );
        assert_eq!(
            generated_image("Generated image (PNG, 5x5, 1 KB) saved to "),
            None
        );
        // The first line is the fact line; the model's note under it is not
        // part of the path.
        let line = format_generated("PNG", (8, 8), None, 70, "/tmp/a.png");
        let output = format!("{line}\nmore text");
        assert_eq!(
            generated_image(&output),
            Some(("/tmp/a.png".to_string(), (8, 8)))
        );
    }

    #[test]
    fn the_model_reads_the_fact_and_the_note() {
        let fact = format_generated("PNG", (1024, 1024), None, 1_500_000, "/g/1.png");
        let text = model_result(&fact, "/g", Shown::Attached(None), false, IMAGE_MODEL);
        assert!(text.starts_with(&fact), "{text}");
        assert!(text.contains("Generated images are saved to /g"), "{text}");
        assert!(text.contains("copy it"), "{text}");
        assert!(text.contains("leave the original in place"), "{text}");
        assert!(text.contains("already sees the image"), "{text}");
        assert!(!text.contains("cannot see"), "{text}");
        assert!(!text.contains("first frame"), "{text}");
        assert!(!text.contains(FALLBACK_IMAGE_MODEL), "{text}");
    }

    #[test]
    fn a_model_looking_at_an_animation_is_told_it_sees_its_first_frame() {
        let fact = format_generated("GIF", (480, 270), Some(9), 1_200_000, "/g/1.gif");
        let text = model_result(&fact, "/g", Shown::Attached(None), true, IMAGE_MODEL);
        assert!(text.contains("first frame"), "{text}");
        assert!(text.contains("animated"), "{text}");
        let text = model_result(
            &fact,
            "/g",
            Shown::Attached(Some((240, 135))),
            true,
            IMAGE_MODEL,
        );
        assert!(text.contains("first frame"), "{text}");
        assert!(text.contains("240x135"), "{text}");
    }

    #[test]
    fn an_image_made_with_the_model_before_says_so() {
        let fact = format_generated("PNG", (1024, 1024), None, 1_500_000, "/g/1.png");
        let text = model_result(
            &fact,
            "/g",
            Shown::Attached(None),
            false,
            FALLBACK_IMAGE_MODEL,
        );
        assert!(text.contains(FALLBACK_IMAGE_MODEL), "{text}");
        assert!(text.contains(IMAGE_MODEL), "names the one refused: {text}");
    }

    #[test]
    fn a_model_that_cannot_see_is_told_the_picture_was_not_attached() {
        let fact = format_generated("PNG", (1024, 1024), None, 1_500_000, "/g/1.png");
        let text = model_result(&fact, "/g", Shown::Withheld, false, IMAGE_MODEL);
        assert!(text.contains("cannot see images"), "{text}");
    }

    #[test]
    fn a_picture_sent_smaller_tells_the_model_what_it_is_looking_at() {
        let fact = format_generated("PNG", (3840, 2160), None, 20_000_000, "/g/1.png");
        let text = model_result(
            &fact,
            "/g",
            Shown::Attached(Some((1536, 864))),
            false,
            IMAGE_MODEL,
        );
        assert!(text.contains("1536x864"), "{text}");
        assert!(text.contains("full size"), "the file is untouched: {text}");
        assert!(!text.contains("cannot see"), "{text}");
    }

    #[test]
    fn a_picture_that_could_not_be_attached_never_blames_the_model() {
        let fact = format_generated("PNG", (1024, 1024), None, 1_500_000, "/g/1.png");
        let text = model_result(&fact, "/g", Shown::Unavailable, false, IMAGE_MODEL);
        assert!(!text.contains("cannot see images"), "{text}");
        assert!(text.contains("could not be attached"), "{text}");
    }

    #[test]
    fn a_refusal_keeps_what_the_endpoint_said_beside_the_advice() {
        // A 403 has more causes than the plan: the endpoint's own words are
        // what says which (docs/imagegen.md).
        let body = r#"{"error": {"message": "Image generation is disabled for this workspace."}}"#;
        let text = failure_text(403, body, 0);
        assert!(text.contains("disabled for this workspace"), "{text}");
        assert!(text.contains("OpenAI refused"), "the advice stays: {text}");
    }

    #[test]
    fn a_refused_sign_in_is_worded_by_the_chatgpt_module() {
        let text = failure_text(401, r#"{"error": {"code": "refresh_token_expired"}}"#, 0);
        assert!(text.contains("/login"), "{text}");
        let text = failure_text(403, "{}", 0);
        assert!(text.contains("plan"), "{text}");
    }

    #[test]
    fn a_usage_limit_says_when_it_resets() {
        let body = r#"{"error": {"type": "usage_limit_reached", "message": "You've hit your limit",
                       "resets_at": 1778840173, "plan_type": "plus"}}"#;
        let text = failure_text(429, body, 1_778_832_973);
        assert!(text.contains("limit"), "{text}");
        assert!(text.contains("2h"), "{text}");
        // A limit with no reset time still says it is a limit.
        let text = failure_text(429, r#"{"error": {"type": "usage_limit_reached"}}"#, 0);
        assert!(text.contains("limit"), "{text}");
        assert!(!text.contains("resets"), "{text}");
    }

    // ----- the boundary half, against a loopback stand-in -----

    /// One request the stand-in received: its request line, its headers
    /// (lowercased) and its body.
    struct Received {
        line: String,
        headers: String,
        body: Vec<u8>,
    }

    /// A stand-in for the Codex backend answering one request per entry of
    /// `answers`, in order, and handing back each request it was sent — the
    /// body read to its `content-length`, since an edit's is streamed.
    fn serve(answers: Vec<(u16, String)>) -> (String, std::thread::JoinHandle<Vec<Received>>) {
        use std::io::{Read, Write};
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let base = format!(
            "http://{}/backend-api/codex",
            listener.local_addr().unwrap()
        );
        let server = std::thread::spawn(move || {
            let mut received = Vec::new();
            for (status, body) in answers {
                let (mut stream, _) = listener.accept().unwrap();
                let mut request = Vec::new();
                let mut chunk = [0u8; 4096];
                let head_end = loop {
                    if let Some(at) = request.windows(4).position(|w| w == b"\r\n\r\n") {
                        break at + 4;
                    }
                    let n = stream.read(&mut chunk).unwrap();
                    assert!(n > 0, "the request ended inside its headers");
                    request.extend_from_slice(&chunk[..n]);
                };
                let head = String::from_utf8_lossy(&request[..head_end]).to_ascii_lowercase();
                let length: usize = head
                    .lines()
                    .find_map(|line| line.strip_prefix("content-length:"))
                    .map_or(0, |n| n.trim().parse().unwrap());
                let mut sent = request[head_end..].to_vec();
                while sent.len() < length {
                    let n = stream.read(&mut chunk).unwrap();
                    assert!(n > 0, "the request ended inside its body");
                    sent.extend_from_slice(&chunk[..n]);
                }
                let reply = format!(
                    "HTTP/1.1 {status} X\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
                    body.len()
                );
                stream.write_all(reply.as_bytes()).unwrap();
                let (line, headers) = head.split_once("\r\n").unwrap();
                received.push(Received {
                    line: line.to_string(),
                    headers: headers.to_string(),
                    body: sent,
                });
            }
            received
        });
        (base, server)
    }

    fn tiny_png() -> Vec<u8> {
        let mut out = std::io::Cursor::new(Vec::new());
        image::RgbaImage::from_fn(3, 2, |x, y| {
            image::Rgba([x as u8 * 40, y as u8 * 90, 7, 255])
        })
        .write_to(&mut out, image::ImageFormat::Png)
        .unwrap();
        out.into_inner()
    }

    fn image_answer(bytes: &[u8]) -> String {
        format!(
            r#"{{"created": 1, "data": [{{"b64_json": "{}", "revised_prompt": "a blue whale"}}], "size": "1024x1024", "output_format": "png"}}"#,
            crate::clipboard::base64_encode(bytes)
        )
    }

    fn target(base: &str) -> Target {
        Target {
            base: base.to_string(),
            headers: vec![
                ("authorization".to_string(), "Bearer t-1".to_string()),
                ("chatgpt-account-id".to_string(), "acct-1".to_string()),
            ],
        }
    }

    fn generation(prompt: &str) -> ImageRequest {
        request(prompt, false, Vec::new())
    }

    #[test]
    fn a_new_image_is_posted_to_the_generations_endpoint_and_saved() {
        let png = tiny_png();
        let (base, server) = serve(vec![(200, image_answer(&png))]);
        let dir = tempfile::tempdir().unwrap();
        let saved = request_image(
            &target(&base),
            &generation("paint a blue whale"),
            dir.path(),
            &CancelToken::new(),
        )
        .unwrap();
        assert_eq!(saved.path, dir.path().join("1.png"));
        assert_eq!(std::fs::read(&saved.path).unwrap(), png);
        assert_eq!((saved.label, saved.size), ("PNG", (3, 2)));
        assert_eq!(saved.bytes, png.len() as u64);
        let request = &server.join().unwrap()[0];
        assert!(
            request
                .line
                .starts_with("post /backend-api/codex/images/generations "),
            "{}",
            request.line
        );
        assert!(
            request.headers.contains("authorization: bearer t-1"),
            "{}",
            request.headers
        );
        assert!(
            request.headers.contains("chatgpt-account-id: acct-1"),
            "{}",
            request.headers
        );
        assert!(
            request.headers.contains("content-type: application/json"),
            "{}",
            request.headers
        );
        let body: Value = serde_json::from_slice(&request.body).unwrap();
        assert_eq!(body["prompt"], "paint a blue whale");
        assert_eq!(body["model"], IMAGE_MODEL);
        // Nothing but the image is left in the folder: no staging file.
        let names: Vec<_> = std::fs::read_dir(dir.path())
            .unwrap()
            .map(|e| e.unwrap().file_name())
            .collect();
        assert_eq!(names, ["1.png"]);
    }

    #[test]
    fn an_edit_streams_its_images_to_the_edits_endpoint() {
        let png = tiny_png();
        let (base, server) = serve(vec![(200, image_answer(&png))]);
        let dir = tempfile::tempdir().unwrap();
        let edit = request(
            "add a red hat",
            true,
            vec![AttachmentUrl::from(
                "data:image/png;base64,Zm9v".to_string(),
            )],
        );
        request_image(&target(&base), &edit, dir.path(), &CancelToken::new()).unwrap();
        let received = &server.join().unwrap()[0];
        assert!(
            received
                .line
                .starts_with("post /backend-api/codex/images/edits "),
            "{}",
            received.line
        );
        let body: Value = serde_json::from_slice(&received.body).unwrap();
        assert_eq!(body["images"][0]["image_url"], "data:image/png;base64,Zm9v");
        assert_eq!(body["background"], "transparent");
    }

    /// A 1×1 GIF of two frames, red then blue.
    const TWO_FRAME_GIF: [u8; 107] = [
        0x47, 0x49, 0x46, 0x38, 0x39, 0x61, 0x01, 0x00, 0x01, 0x00, 0x81, 0x00, 0x00, 0xff, 0x00,
        0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x21, 0xff, 0x0b, 0x4e, 0x45,
        0x54, 0x53, 0x43, 0x41, 0x50, 0x45, 0x32, 0x2e, 0x30, 0x03, 0x01, 0x00, 0x00, 0x00, 0x21,
        0xf9, 0x04, 0x00, 0x14, 0x00, 0x00, 0x00, 0x2c, 0x00, 0x00, 0x00, 0x00, 0x01, 0x00, 0x01,
        0x00, 0x00, 0x08, 0x04, 0x00, 0x01, 0x04, 0x04, 0x00, 0x21, 0xf9, 0x04, 0x01, 0x14, 0x00,
        0x01, 0x00, 0x2c, 0x00, 0x00, 0x00, 0x00, 0x01, 0x00, 0x01, 0x00, 0x81, 0x00, 0x00, 0xff,
        0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x08, 0x04, 0x00, 0x01, 0x04, 0x04,
        0x00, 0x3b,
    ];

    #[test]
    fn a_gif_answer_is_saved_as_a_gif_with_its_frames_counted() {
        let (base, server) = serve(vec![(200, image_answer(&TWO_FRAME_GIF))]);
        let dir = tempfile::tempdir().unwrap();
        let mut animated = generation("a pixel turning blue");
        animated.gif = Some(GifOptions::default());
        let saved =
            request_image(&target(&base), &animated, dir.path(), &CancelToken::new()).unwrap();
        server.join().unwrap();
        assert_eq!(saved.path, dir.path().join("1.gif"));
        assert_eq!(
            (saved.label, saved.size, saved.frames),
            ("GIF", (1, 1), Some(2))
        );
        assert_eq!(std::fs::read(&saved.path).unwrap(), TWO_FRAME_GIF);
    }

    #[test]
    fn a_backend_that_refuses_the_newest_model_makes_the_image_with_the_one_before() {
        let png = tiny_png();
        let refusal = r#"{"error": {"message": "Invalid value: 'gpt-image-2.6-sunburst'.", "param": "model"}}"#;
        let (base, server) = serve(vec![(400, refusal.to_string()), (200, image_answer(&png))]);
        let dir = tempfile::tempdir().unwrap();
        let cancel = CancelToken::new();
        let base_request = generation("a lighthouse");
        let (saved, model) = with_model_fallback(|model| {
            let asked = ImageRequest {
                model,
                ..base_request.clone()
            };
            request_image(&target(&base), &asked, dir.path(), &cancel)
        })
        .unwrap();
        assert_eq!(model, FALLBACK_IMAGE_MODEL);
        assert_eq!(saved.path, dir.path().join("1.png"));
        let received = server.join().unwrap();
        let models: Vec<Value> = received
            .iter()
            .map(|r| serde_json::from_slice::<Value>(&r.body).unwrap()["model"].clone())
            .collect();
        assert_eq!(models, [json!(IMAGE_MODEL), json!(FALLBACK_IMAGE_MODEL)]);
    }

    #[test]
    fn each_image_takes_the_folders_next_number() {
        let png = tiny_png();
        let (base, server) = serve(vec![(200, image_answer(&png)), (200, image_answer(&png))]);
        let dir = tempfile::tempdir().unwrap();
        let cancel = CancelToken::new();
        let first = request_image(&target(&base), &generation("a"), dir.path(), &cancel).unwrap();
        let second = request_image(&target(&base), &generation("b"), dir.path(), &cancel).unwrap();
        server.join().unwrap();
        assert_eq!(first.path, dir.path().join("1.png"));
        assert_eq!(second.path, dir.path().join("2.png"));
    }

    #[test]
    fn a_refusal_keeps_its_status_and_body() {
        let limit = r#"{"error": {"type": "usage_limit_reached"}}"#.to_string();
        let (base, server) = serve(vec![(429, limit.clone())]);
        let dir = tempfile::tempdir().unwrap();
        let error = request_image(
            &target(&base),
            &generation("a"),
            dir.path(),
            &CancelToken::new(),
        )
        .unwrap_err();
        server.join().unwrap();
        match error {
            ImagegenError::Refused { status, body } => assert_eq!((status, body), (429, limit)),
            other => panic!("{other:?}"),
        }
        assert_eq!(std::fs::read_dir(dir.path()).map_or(0, Iterator::count), 0);
    }

    #[test]
    fn an_answer_without_an_image_saves_nothing() {
        let (base, server) = serve(vec![(200, r#"{"created": 1, "data": []}"#.to_string())]);
        let dir = tempfile::tempdir().unwrap();
        let error = request_image(
            &target(&base),
            &generation("a"),
            dir.path(),
            &CancelToken::new(),
        )
        .unwrap_err();
        server.join().unwrap();
        assert!(matches!(error, ImagegenError::NoImage), "{error:?}");
        assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 0);
    }

    #[test]
    fn an_image_that_is_not_a_picture_saves_nothing() {
        let (base, server) = serve(vec![(200, image_answer(b"certainly not a picture"))]);
        let dir = tempfile::tempdir().unwrap();
        let error = request_image(
            &target(&base),
            &generation("a"),
            dir.path(),
            &CancelToken::new(),
        )
        .unwrap_err();
        server.join().unwrap();
        assert!(matches!(error, ImagegenError::Save(_)), "{error:?}");
        assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 0);
    }

    #[test]
    fn an_answer_saved_without_a_request_lands_in_the_folder() {
        let png = tiny_png();
        let dir = tempfile::tempdir().unwrap();
        let answer = image_answer(&png);
        let path = save_generated_answer(dir.path(), &mut answer.as_bytes()).unwrap();
        assert_eq!(path, dir.path().join("1.png"));
        assert_eq!(std::fs::read(path).unwrap(), png);
        let error = save_generated_answer(dir.path(), &mut &b"{}"[..]).unwrap_err();
        assert_eq!(error, "image generation returned no image data");
    }

    #[test]
    fn a_failure_that_may_be_retried_is_retried_and_one_that_may_not_is_not() {
        let cancel = CancelToken::new();
        let attempts = std::cell::Cell::new(0);
        let unreachable = || {
            ImagegenError::Transport(crate::llm::LlmError::Unreachable {
                host: "chatgpt.com".to_string(),
                message: "refused".to_string(),
            })
        };
        // Nothing was sent: retried, and the third try's answer stands.
        let result = with_retries(Duration::ZERO, &cancel, || {
            attempts.set(attempts.get() + 1);
            if attempts.get() < 3 {
                Err(unreachable())
            } else {
                Ok(7)
            }
        });
        assert_eq!((result.unwrap(), attempts.get()), (7, 3));
        // A backend that keeps failing is tried RETRIES more times, no more.
        attempts.set(0);
        let result: Result<(), _> = with_retries(Duration::ZERO, &cancel, || {
            attempts.set(attempts.get() + 1);
            Err(ImagegenError::Refused {
                status: 503,
                body: String::new(),
            })
        });
        assert!(result.is_err());
        assert_eq!(attempts.get(), 1 + RETRIES);
        // A usage limit, a bad request and an answer that came are final.
        for error in [
            ImagegenError::Refused {
                status: 429,
                body: String::new(),
            },
            ImagegenError::Refused {
                status: 400,
                body: String::new(),
            },
            ImagegenError::NoImage,
        ] {
            attempts.set(0);
            let mut once = Some(error);
            let result: Result<(), _> = with_retries(Duration::ZERO, &cancel, || {
                attempts.set(attempts.get() + 1);
                Err(once.take().unwrap())
            });
            assert!(result.is_err());
            assert_eq!(attempts.get(), 1);
        }
    }

    #[test]
    fn a_cancel_ends_the_retries() {
        let cancel = CancelToken::new();
        cancel.cancel();
        let attempts = std::cell::Cell::new(0);
        let result: Result<(), _> = with_retries(Duration::from_secs(60), &cancel, || {
            attempts.set(attempts.get() + 1);
            Err(ImagegenError::Refused {
                status: 503,
                body: String::new(),
            })
        });
        assert!(result.is_err());
        assert_eq!(attempts.get(), 1, "no second try after Esc");
    }

    #[test]
    fn referenced_images_resolve_to_absolute_paths_and_are_checked() {
        let dir = tempfile::tempdir().unwrap();
        let png = dir.path().join("fox.png");
        std::fs::write(&png, tiny_png()).unwrap();
        let gif = dir.path().join("anim.gif");
        image::RgbaImage::new(2, 2).save(&gif).unwrap();
        let text = dir.path().join("notes.png");
        std::fs::write(&text, "not an image").unwrap();
        let show = |path: &Path| path.to_string_lossy().into_owned();

        assert_eq!(
            resolve_references(&[show(&png)]).unwrap(),
            std::slice::from_ref(&png)
        );
        // A GIF is taken: the encoder sends its first frame as a still
        // (docs/images.md "A GIF goes up as a still").
        assert_eq!(
            resolve_references(&[show(&gif)]).unwrap(),
            std::slice::from_ref(&gif)
        );
        let error = resolve_references(&[show(&text)]).unwrap_err();
        assert!(error.contains("not a PNG, JPEG, WebP or GIF"), "{error}");
        let error = resolve_references(&[show(dir.path())]).unwrap_err();
        assert!(error.contains("not a file"), "{error}");
        let error = resolve_references(&[show(&dir.path().join("gone.png"))]).unwrap_err();
        assert!(error.contains("gone.png"), "{error}");
        // A relative path resolves against the working directory.
        let resolved = resolve_references(&["fox.png".to_string()]).unwrap_err();
        let cwd = std::env::current_dir().unwrap();
        assert!(resolved.contains(&show(&cwd.join("fox.png"))), "{resolved}");
    }

    #[test]
    fn a_reference_past_the_apis_limit_is_refused() {
        let dir = tempfile::tempdir().unwrap();
        let big = dir.path().join("big.png");
        let file = std::fs::File::create(&big).unwrap();
        // Sparse: past the limit without writing fifty megabytes.
        file.set_len(MAX_REFERENCE_BYTES + 1).unwrap();
        let error = resolve_references(&[big.to_string_lossy().into_owned()]).unwrap_err();
        assert!(error.contains("50 MB"), "{error}");
    }

    fn saved(dir: &Path) -> Saved {
        Saved {
            path: dir.join("1.png"),
            label: "PNG",
            size: (1024, 1536),
            frames: None,
            bytes: 1_500_000,
        }
    }

    #[test]
    fn a_saved_image_finishes_with_its_fact_line_the_note_and_the_picture() {
        let dir = Path::new("/g");
        let url = AttachmentUrl::from("data:image/png;base64,Zm9v".to_string());
        let fitted = crate::images::FittedAttachment {
            url: url.clone(),
            sent: None,
        };
        let outcome = finished(&saved(dir), dir, Attachment::Attached(fitted), IMAGE_MODEL);
        assert!(outcome.ok);
        assert_eq!(
            outcome.output,
            "Generated image (PNG, 1024x1536, 1.4 MB) saved to /g/1.png"
        );
        let context = outcome.context.as_deref().unwrap();
        assert!(context.starts_with(&outcome.output), "{context}");
        assert!(context.contains("saved to /g by default"), "{context}");
        assert_eq!(outcome.image, Some(url));
    }

    #[test]
    fn a_model_that_cannot_see_gets_the_file_and_no_picture() {
        let dir = Path::new("/g");
        let outcome = finished(&saved(dir), dir, Attachment::Withheld, IMAGE_MODEL);
        assert!(outcome.ok);
        assert_eq!(outcome.image, None);
        assert!(outcome.image_withheld, "the loop sends the replay's note");
        assert!(
            outcome
                .context
                .as_deref()
                .unwrap()
                .contains("cannot see images")
        );
    }

    #[test]
    fn a_saved_animation_finishes_with_its_frames_and_the_still_it_shows() {
        let dir = Path::new("/g");
        let animation = Saved {
            path: dir.join("1.gif"),
            label: "GIF",
            size: (480, 270),
            frames: Some(9),
            bytes: 1_200_000,
        };
        let fitted = crate::images::FittedAttachment {
            url: AttachmentUrl::from("data:image/png;base64,Zm9v".to_string()),
            sent: None,
        };
        let outcome = finished(&animation, dir, Attachment::Attached(fitted), IMAGE_MODEL);
        assert_eq!(
            outcome.output,
            "Generated image (GIF, 480x270, 9 frames, 1.1 MB) saved to /g/1.gif"
        );
        assert!(
            outcome.context.as_deref().unwrap().contains("first frame"),
            "{:?}",
            outcome.context
        );
    }

    #[test]
    fn a_call_asking_for_a_gif_is_known_by_its_arguments() {
        assert!(asks_for_gif(r#"{"prompt": "x", "gif": {}}"#));
        assert!(asks_for_gif(
            r#"{"prompt": "x", "gif": {"frame_count": 4}}"#
        ));
        assert!(!asks_for_gif(r#"{"prompt": "x"}"#));
        assert!(!asks_for_gif(r#"{"prompt": "x", "gif": null}"#));
        assert!(!asks_for_gif("not json"));
    }

    #[test]
    fn only_a_cancel_reads_as_an_interrupt() {
        let cancel = CancelToken::new();
        let died = unanswered(&cancel);
        assert!(!died.ok);
        assert_eq!(died.output, STOPPED_UNEXPECTEDLY);
        assert_eq!(died.error_kind, None, "the tool's own failure");
        cancel.cancel();
        let stopped = unanswered(&cancel);
        assert_eq!(
            stopped.error_kind,
            Some(crate::stream::ToolErrorKind::Interrupted)
        );
    }

    #[test]
    fn the_sign_in_handle_is_shared_by_every_clone() {
        let signin = ChatGptSignin::default();
        let held = signin.clone();
        assert!(held.get().is_none());
        let mut cfg = crate::llm::ModelConfig::fallback();
        cfg.api_key = Some("rt-1".to_string());
        signin.set(Some(cfg));
        assert_eq!(
            held.get().and_then(|cfg| cfg.api_key).as_deref(),
            Some("rt-1")
        );
        signin.set(None);
        assert!(held.get().is_none());
    }

    fn setup() -> ImagegenSetup {
        ImagegenSetup {
            signin: ChatGptSignin::default(),
            dir: PathBuf::from("/nowhere"),
            skill: None,
        }
    }

    #[test]
    fn a_call_with_bad_arguments_is_refused_before_anything_runs() {
        let outcome = run_imagegen(
            r#"{"prompt": "x", "size": "4k"}"#,
            &setup(),
            None,
            &CancelToken::new(),
        );
        assert!(!outcome.ok);
        assert!(outcome.output.contains("`size`"), "{}", outcome.output);
    }

    #[test]
    fn a_call_with_nobody_signed_in_says_how_to_sign_in() {
        let outcome = run_imagegen(r#"{"prompt": "x"}"#, &setup(), None, &CancelToken::new());
        assert!(!outcome.ok);
        assert_eq!(outcome.output, crate::llm::auth::NOT_SIGNED_IN);
    }

    #[test]
    fn a_sign_in_of_another_kind_is_no_chatgpt_sign_in() {
        let mut cfg = crate::llm::ModelConfig::fallback();
        cfg.api_key = Some("sk-not-chatgpt".to_string());
        assert_eq!(
            resolve_target(&cfg).unwrap_err(),
            crate::llm::auth::NOT_SIGNED_IN
        );
    }

    #[test]
    fn any_other_refusal_names_its_status_and_the_servers_message() {
        let text = failure_text(500, r#"{"error": {"message": "image backend failed"}}"#, 0);
        assert_eq!(
            text,
            "image generation failed: HTTP 500: image backend failed"
        );
        let text = failure_text(400, "plain text\nbody", 0);
        assert_eq!(text, "image generation failed: HTTP 400: plain text body");
        let text = failure_text(502, "", 0);
        assert_eq!(text, "image generation failed: HTTP 502");
    }
}
