//! The `imagegen` tool: images made and edited with OpenAI's Images 2.6
//! (`gpt-image-2.6-sunburst`) through the user's **ChatGPT Codex sign-in**,
//! whatever provider the conversation itself runs on (`docs/imagegen.md`).
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

/// The image model every request names unless [`IMAGEGEN_MODEL_ENV`] names
/// another: Images 2.6's Sunburst, the model for detailed work that keeps an
/// edit consistent with what it edits. Its fast sibling is
/// `gpt-image-2.6-flare` (`docs/imagegen.md`).
pub const IMAGE_MODEL: &str = "gpt-image-2.6-sunburst";

/// The model a request is sent again with when the backend refuses
/// [`IMAGE_MODEL`] by name: the one images were made with before it, so a
/// backend that does not take the newest yet still makes the image
/// (`docs/imagegen.md` *The model*).
pub const FALLBACK_IMAGE_MODEL: &str = "gpt-image-2";

/// Names another image model for the run, never saved — `gpt-image-2.6-flare`
/// for speed, or an older model. A model named here is the one asked for:
/// its refusal is the answer, with no fallback.
pub const IMAGEGEN_MODEL_ENV: &str = "ALTER_ZERO_IMAGEGEN_MODEL";

/// The model a session's requests name: `env`, the variable's value, when it
/// says anything, else [`IMAGE_MODEL`].
#[must_use]
pub fn image_model(env: Option<&str>) -> String {
    env.map(str::trim)
        .filter(|model| !model.is_empty())
        .unwrap_or(IMAGE_MODEL)
        .to_string()
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
                "num_last_images_to_include": {
                    "type": "integer",
                    "minimum": 1,
                    "maximum": MAX_REFERENCED_IMAGES,
                    "description": "How many of the newest images in the conversation to edit, 1 to 5; never with `referenced_image_paths`."
                }
            },
            "required": ["prompt"],
            "additionalProperties": false
        }),
    )
}

/// The tool's description: what it does and its sharp edges, short — every
/// word rides every request that offers it, and the craft (when to draw, how
/// to word a prompt, where a picture belongs, which images to name) is the
/// `imagegen` skill's, listed whenever the tool is offered
/// (`docs/imagegen.md`).
const IMAGEGEN_DESCRIPTION: &str = "\
Generate an image from a prompt, or edit images, with ChatGPT's image model \
through the user's ChatGPT sign-in. Each call makes one image (it can take \
minutes), saves it, shows it to the user and returns its path. Generate \
directly, without asking for confirmation. For an edit or a style reference, \
list the images in `referenced_image_paths`, looking at a local one with `read` \
first; for conversation images with no file, set `num_last_images_to_include` \
instead. Set `transparent_background` for a cutout.";

/// A call's arguments, checked.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ImagegenArgs {
    /// What to make, or what to change.
    pub prompt: String,
    /// Whether the image should have a transparent background.
    pub transparent_background: bool,
    /// The images to edit or take as references, as the model wrote them.
    pub referenced_image_paths: Vec<String>,
    /// How many of the newest images in the conversation to take instead
    /// ([`ConversationImages`]): the way to name a picture that has no file,
    /// like an MCP tool's.
    pub num_last_images_to_include: Option<usize>,
}

impl ImagegenArgs {
    /// Parse a call's raw arguments. An argument the tool does not take is
    /// refused, naming the ones it does — a model that sends `size` learns at
    /// once that it does nothing (Codex's `deny_unknown_fields`).
    ///
    /// # Errors
    /// Unparseable or unknown arguments, a blank prompt, more than
    /// [`MAX_REFERENCED_IMAGES`] references, a count of the newest images
    /// outside 1 to [`MAX_REFERENCED_IMAGES`], or a count beside paths —
    /// each a message the model reads.
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
            num_last_images_to_include: Option<i64>,
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
        // One way of naming the images or the other, never both; an empty
        // list of paths names none.
        let num_last_images_to_include = match raw.num_last_images_to_include {
            None => None,
            Some(_) if !referenced_image_paths.is_empty() => {
                return Err(
                    "provide only one of `referenced_image_paths` or `num_last_images_to_include`"
                        .to_string(),
                );
            }
            Some(count) => Some(
                usize::try_from(count)
                    .ok()
                    .filter(|count| (1..=MAX_REFERENCED_IMAGES).contains(count))
                    .ok_or_else(|| {
                        format!(
                            "`num_last_images_to_include` must be between 1 and \
                             {MAX_REFERENCED_IMAGES}"
                        )
                    })?,
            ),
        };
        Ok(Self {
            prompt,
            transparent_background: raw.transparent_background.unwrap_or(false),
            referenced_image_paths,
            num_last_images_to_include,
        })
    }
}

/// The pictures the conversation holds as of the round a call answers: the
/// image parts of the request the model wrote its calls against, oldest
/// first — a paste, an image `read`, an image made earlier, an MCP tool's
/// result — each as the conversation's own copy, never the file it came from
/// (`docs/imagegen.md` *Images already in the conversation*). Shared by
/// handle: each round records the request it sends, and an `imagegen` call's
/// `num_last_images_to_include` takes the newest from it ([`Self::newest`]).
/// The copies are the messages' own encodings, shared rather than duplicated
/// (`docs/memory.md`).
#[derive(Debug, Clone, Default)]
pub struct ConversationImages(Arc<Mutex<Vec<AttachmentUrl>>>);

impl ConversationImages {
    /// Record the images `messages` carry, in order and whoever sent them,
    /// in place of the last record.
    pub fn record(&self, messages: &[crate::llm::ChatMessage]) {
        let images = messages
            .iter()
            .filter_map(|message| match &message.content {
                crate::llm::MessageContent::Parts(parts) => Some(parts),
                crate::llm::MessageContent::Text(_) => None,
            })
            .flatten()
            .filter_map(|part| match part {
                crate::llm::ContentPart::ImageUrl { image_url } => Some(image_url.url.clone()),
                crate::llm::ContentPart::Text { .. } => None,
            })
            .collect();
        if let Ok(mut slot) = self.0.lock() {
            *slot = images;
        }
    }

    /// The newest `count` of the recorded images, oldest first, the same
    /// picture twice counting twice (`newest_images`).
    ///
    /// # Errors
    /// Fewer than `count` recorded.
    pub fn newest(&self, count: usize) -> Result<Vec<AttachmentUrl>, String> {
        match self.0.lock() {
            Ok(images) => newest_images(&images, count),
            Err(_) => newest_images(&[], count),
        }
    }
}

/// The newest `count` of `images`, in the order the conversation holds them,
/// the same picture twice counting twice: a picture with no file has no
/// stable name to tell a repeat by.
///
/// # Errors
/// Fewer than `count` images: the call fails and nothing is sent, never a
/// request with fewer pictures than the model meant.
fn newest_images(images: &[AttachmentUrl], count: usize) -> Result<Vec<AttachmentUrl>, String> {
    let available = images.len();
    if available < count {
        return Err(format!(
            "requested the last {count} conversation images, but only {available} were available"
        ));
    }
    Ok(images[available - count..].to_vec())
}

/// One request to the Images endpoints, owned so it can be streamed
/// ([`crate::llm::body::BodySource`]): the reference images ride as the
/// session's shared `data:` URL encodings, borrowed rather than copied.
#[derive(Clone)]
pub(crate) struct ImageRequest {
    pub prompt: String,
    pub transparent_background: bool,
    pub images: Vec<AttachmentUrl>,
    /// The image model ([`image_model`]).
    pub model: String,
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
        // Codex's fields, in its order: the backend chooses the size and the
        // quality, and `background` is never `auto` — opaque unless a cutout
        // was asked for (`docs/imagegen.md`).
        #[derive(serde::Serialize)]
        struct Body<'a> {
            prompt: &'a str,
            #[serde(skip_serializing_if = "Vec::is_empty")]
            images: Vec<Reference<'a>>,
            background: &'static str,
            model: &'a str,
            quality: &'static str,
            size: &'static str,
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
            model: &self.model,
            quality: "auto",
            size: "auto",
        };
        serde_json::to_writer(out, &body)
    }
}

/// The head every result of this tool opens with — what tells a generated
/// image's cell from any other output ([`generated_image`]).
const GENERATED_HEAD: &str = "Generated image ";

/// The one fact line a generated image's cell shows and the model reads:
/// `Generated image (PNG, 1536x1024, 2.1 MB) saved to /…/1.png`. The path is
/// last, so a path full of digits can never be read as the size.
#[must_use]
pub fn format_generated(label: &str, size: (u32, u32), bytes: u64, path: &str) -> String {
    let bytes = usize::try_from(bytes).unwrap_or(usize::MAX);
    format!(
        "{GENERATED_HEAD}({label}, {}x{}, {}) saved to {path}",
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

/// What the model reads for a generated image: the fact line, then Codex's
/// note on where generated images live and what to do with them, then what
/// it can see of the picture.
#[must_use]
pub fn model_result(fact: &str, dir: &str, shown: Shown) -> String {
    let mut text = format!(
        "{fact}\n\
         Generated images are saved to {dir} by default. If you need the image at another \
         path, copy it there and leave the original in place unless the user explicitly asks \
         you to delete it.\n\
         The user already sees the image in the conversation, so there is no need to show it \
         again in your reply as a Markdown image or a file link."
    );
    match shown {
        Shown::Attached(None) => {}
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
    format!("image generation failed: {}", http_words(status, &message))
}

/// A refusal in a few words: its status, and the error's own message when it
/// has one.
fn http_words(status: u16, message: &str) -> String {
    if message.is_empty() {
        format!("HTTP {status}")
    } else {
        format!("HTTP {status}: {message}")
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
    /// The image model every request names ([`image_model`]).
    pub model: String,
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
    /// The backend refused [`IMAGE_MODEL`] by name — `status` and `body` —
    /// and [`FALLBACK_IMAGE_MODEL`], asked instead, failed too: `then`.
    FellBack {
        status: u16,
        body: String,
        then: Box<ImagegenError>,
    },
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
            // What stopped the image first, then why the older model was asked.
            Self::FellBack { status, body, then } => format!(
                "{}\n{FALLBACK_IMAGE_MODEL} was asked after the image backend refused \
                 {IMAGE_MODEL}: {}",
                then.text(now),
                http_words(*status, &error_message(body))
            ),
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

/// The most a generated image may weigh on disk: the largest Images 2.6
/// output, a 4K still, is a few tens of megabytes.
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
    let path = crate::clipboard::keep_numbered(staging, dir, ext).map_err(ImagegenError::Save)?;
    Ok(Saved {
        bytes: split.decoded,
        path,
        label,
        size,
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

/// How an image was made where that is not what the call asked for — what
/// the model's note and the cell's row say about it (`docs/imagegen.md` *The
/// model*). The default is an image made as asked.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) struct Made {
    /// Made with [`FALLBACK_IMAGE_MODEL`]: the backend refused
    /// [`IMAGE_MODEL`] by name.
    pub fell_back: bool,
}

impl Made {
    /// What the model reads about it after the rest of its note, when there
    /// is anything to read.
    fn note(&self) -> Option<String> {
        self.fell_back.then(|| {
            format!(
                "This image was made with {FALLBACK_IMAGE_MODEL}: the image backend did not \
                 take {IMAGE_MODEL}."
            )
        })
    }

    /// The cell's dim row about it: which model made the image, when it is
    /// not the one asked for.
    fn cell_note(&self) -> Option<String> {
        self.fell_back
            .then(|| format!("Made with {FALLBACK_IMAGE_MODEL} (Images 2.6 not available)"))
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

/// Make the image `asked` asks for, and, when the backend refuses
/// [`IMAGE_MODEL`] by name ([`refuses_model`]), once more with
/// [`FALLBACK_IMAGE_MODEL`], handing back how it was made
/// (`docs/imagegen.md` *The model*). Any other failure stands, since another
/// model would be refused the same way, and so does the refusal of a model
/// `ALTER_ZERO_IMAGEGEN_MODEL` named — that one is the run's choice. Nothing
/// more is sent once `cancel` is set — an Esc while the refusal came back
/// would otherwise be billed an image nobody waits for — and an older model
/// failing too fails with both answers ([`ImagegenError::FellBack`]).
fn with_model_fallback<T>(
    asked: &ImageRequest,
    cancel: &CancelToken,
    mut attempt: impl FnMut(&ImageRequest) -> Result<T, ImagegenError>,
) -> Result<(T, Made), ImagegenError> {
    match attempt(asked) {
        Err(ImagegenError::Refused { status, body })
            if asked.model == IMAGE_MODEL
                && refuses_model(status, &body, IMAGE_MODEL)
                && !cancel.is_cancelled() =>
        {
            let older = ImageRequest {
                model: FALLBACK_IMAGE_MODEL.to_string(),
                ..asked.clone()
            };
            match attempt(&older) {
                Ok(image) => Ok((image, Made { fell_back: true })),
                Err(then) => Err(ImagegenError::FellBack {
                    status,
                    body,
                    then: Box::new(then),
                }),
            }
        }
        other => other.map(|image| (image, Made::default())),
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
/// [`MAX_REFERENCE_BYTES`]. A GIF goes up as its first frame
/// ([`reference_url`]). A relative path resolves against the working
/// directory, the file tools' rule.
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

/// The `data:` URL a referenced image goes up as. A GIF never goes up as
/// itself: it goes up as its first frame, the picture an edit edits. Any
/// other image goes up as the session's own encoding of it.
fn reference_url(path: &Path) -> Option<AttachmentUrl> {
    if crate::images::is_gif(path) {
        return crate::images::gif_first_frame_data_url(path);
    }
    crate::images::attachment_data_url(path)
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
/// Codex's note for the model, and the picture attached when there is one to
/// attach — or, for a model that cannot see, the mark that has the agent
/// loop send the note the replay sends in its place.
fn finished(saved: &Saved, dir: &Path, attachment: Attachment, made: &Made) -> ToolOutcome {
    let fact = format_generated(
        saved.label,
        saved.size,
        saved.bytes,
        &saved.path.to_string_lossy(),
    );
    let shown = match &attachment {
        Attachment::Attached(fitted) => Shown::Attached(fitted.sent),
        Attachment::Withheld => Shown::Withheld,
        Attachment::Unavailable => Shown::Unavailable,
    };
    let mut model = model_result(&fact, &dir.to_string_lossy(), shown);
    if let Some(note) = made.note() {
        model.push('\n');
        model.push_str(&note);
    }
    let mut outcome = ToolOutcome::ok(fact).with_context(model);
    if let Some(note) = made.cell_note() {
        outcome = outcome.with_note(note);
    }
    match attachment {
        Attachment::Attached(fitted) => outcome.with_image(fitted.url),
        Attachment::Withheld => outcome.with_image_withheld(),
        Attachment::Unavailable => outcome,
    }
}

/// Run one `imagegen` call (`docs/imagegen.md` *The call*): check the
/// arguments and the images they name — files, or the newest of the
/// conversation's `images` — then mint, send and save on a worker thread the
/// turn abandons on Esc, and hand back the fact line, the model's note and
/// the picture. `vision` is the model's image support — a model known not to
/// see gets the file and no attachment.
pub(crate) fn run_imagegen(
    arguments: &str,
    setup: &ImagegenSetup,
    vision: Option<bool>,
    images: &ConversationImages,
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
    // The conversation's own copies, as the model saw them; never both these
    // and files (`ImagegenArgs::parse`).
    let newest = match args
        .num_last_images_to_include
        .map(|count| images.newest(count))
    {
        None => Vec::new(),
        Some(Ok(newest)) => newest,
        Some(Err(error)) => return ToolOutcome::error(error),
    };
    let Some(cfg) = setup.signin.get() else {
        return ToolOutcome::error(crate::llm::auth::NOT_SIGNED_IN);
    };
    let dir = setup.dir.clone();
    let model = setup.model.clone();
    let worker_dir = dir.clone();
    let worker_cancel = cancel.clone();
    let result = crate::llm::off_thread(
        move || -> Result<(Saved, Attachment, Made), String> {
            let target = resolve_target(&cfg)?;
            let mut images = references
                .iter()
                .map(|path| {
                    reference_url(path).ok_or_else(|| {
                        format!("could not read the referenced image {}", path.display())
                    })
                })
                .collect::<Result<Vec<_>, _>>()?;
            images.extend(newest);
            let request = ImageRequest {
                prompt: args.prompt,
                transparent_background: args.transparent_background,
                images,
                model,
            };
            // A backend that refuses the newest model by name is asked once
            // more with the older one; each model's request has its retries.
            let (saved, made) = with_model_fallback(&request, &worker_cancel, |request| {
                with_retries(RETRY_PAUSE, &worker_cancel, || {
                    request_image(&target, request, &worker_dir, &worker_cancel)
                })
            })
            .map_err(|error| error.text(unix_now()))?;
            // Encoded here, on the worker the turn abandons on Esc: a picture
            // past the cap is decoded and shrunk.
            let attachment = Attachment::for_saved(&saved.path, vision);
            Ok((saved, attachment, made))
        },
        cancel,
    );
    match result {
        None => unanswered(cancel),
        Some(Err(error)) => ToolOutcome::error(error),
        Some(Ok((saved, attachment, made))) => {
            let shown = saved.path.to_string_lossy().into_owned();
            // The cell this call resolves draws the picture from the file
            // (`docs/images.md`).
            crate::images::remember_size(&shown, saved.size);
            finished(&saved, &dir, attachment, &made)
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
        let props = &params["properties"];
        assert_eq!(props["prompt"]["type"], "string");
        assert_eq!(props["transparent_background"]["type"], "boolean");
        assert_eq!(props["referenced_image_paths"]["type"], "array");
        assert_eq!(props["referenced_image_paths"]["items"]["type"], "string");
        // The count of the newest images in the conversation, its 1 to 5
        // stated in the schema.
        let count = &props["num_last_images_to_include"];
        assert_eq!(
            (&count["type"], &count["minimum"], &count["maximum"]),
            (&json!("integer"), &json!(1), &json!(5))
        );
        // No size, quality, count, model, output format or GIF: the backend
        // chooses the size and the quality, a call makes one still, and the
        // model is the session's (docs/imagegen.md).
        let names: Vec<&str> = props
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
                "num_last_images_to_include"
            ]
        );
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
        assert!(words <= 80, "{words} words: {description}");
        assert!(description.contains("path"), "names what it returns");
        assert!(
            description.contains("`referenced_image_paths`"),
            "{description}"
        );
        assert!(
            description.contains("`transparent_background`"),
            "{description}"
        );
        // This app looks at a local image with `read`; `view_image` and code
        // mode are another client's.
        assert!(description.contains("`read`"), "{description}");
        assert!(
            description.contains("`num_last_images_to_include`"),
            "{description}"
        );
        for absent in ["view_image", "@exec"] {
            assert!(!description.contains(absent), "{absent}: {description}");
        }
        // It makes stills: GIF output is not offered, so nothing promises one.
        assert!(!description.contains("GIF"), "{description}");
        // Every parameter is short (CLAUDE.md's tool prose rule).
        let props = &spec["function"]["parameters"]["properties"];
        for (name, param) in props.as_object().unwrap() {
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
                num_last_images_to_include: None,
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
                num_last_images_to_include: None,
            }
        );
    }

    #[test]
    fn a_null_optional_argument_reads_as_its_default() {
        let args = ImagegenArgs::parse(
            r#"{"prompt": "x", "transparent_background": null, "referenced_image_paths": null}"#,
        )
        .unwrap();
        assert!(!args.transparent_background);
        assert!(args.referenced_image_paths.is_empty());
    }

    #[test]
    fn an_argument_the_tool_does_not_take_is_refused_by_name() {
        let error = ImagegenArgs::parse(r#"{"prompt": "x", "n": 2}"#).unwrap_err();
        assert!(error.contains("`n`"), "{error}");
        assert!(error.contains("`referenced_image_paths`"), "{error}");
        assert!(!error.contains("`gif`"), "{error}");
        // GIF output is not offered (docs/imagegen.md *Tried live*): a call
        // asking for one is refused before anything is sent.
        let error =
            ImagegenArgs::parse(r#"{"prompt": "x", "gif": {"frame_count": 4}}"#).unwrap_err();
        assert!(error.contains("unknown field `gif`"), "{error}");
        // Nor a size or a quality: the backend chooses both.
        for (raw, field) in [
            (r#"{"prompt": "x", "size": "1536x1024"}"#, "`size`"),
            (r#"{"prompt": "x", "quality": "high"}"#, "`quality`"),
        ] {
            let error = ImagegenArgs::parse(raw).unwrap_err();
            assert!(error.contains(&format!("unknown field {field}")), "{error}");
            assert!(!error.contains("`size`, `quality`"), "{error}");
        }
    }

    #[test]
    fn the_model_is_images_2_6s_unless_the_environment_names_another() {
        assert_eq!(IMAGE_MODEL, "gpt-image-2.6-sunburst");
        assert_eq!(image_model(None), IMAGE_MODEL);
        assert_eq!(image_model(Some("  ")), IMAGE_MODEL);
        assert_eq!(
            image_model(Some(" gpt-image-2.6-flare ")),
            "gpt-image-2.6-flare"
        );
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

    #[test]
    fn the_newest_images_are_counted_from_one_to_five_and_never_beside_paths() {
        let count = |raw: &str| {
            ImagegenArgs::parse(&format!(
                r#"{{"prompt": "x", "num_last_images_to_include": {raw}}}"#
            ))
            .map(|args| args.num_last_images_to_include)
        };
        assert_eq!(count("1"), Ok(Some(1)));
        assert_eq!(count("5"), Ok(Some(5)));
        assert_eq!(count("null"), Ok(None));
        let args = ImagegenArgs::parse(r#"{"prompt": "x"}"#).unwrap();
        assert_eq!(args.num_last_images_to_include, None);
        for raw in ["0", "6", "-1"] {
            assert_eq!(
                count(raw),
                Err("`num_last_images_to_include` must be between 1 and 5".to_string()),
                "{raw}"
            );
        }
        // No whole number is no count at all.
        for raw in ["1.5", r#""2""#] {
            assert!(count(raw).is_err(), "{raw}");
        }
        // One way of naming the images or the other, never both; an empty
        // list of paths names none.
        let both = ImagegenArgs::parse(
            r#"{"prompt": "x", "referenced_image_paths": ["/a.png"],
                "num_last_images_to_include": 1}"#,
        );
        assert_eq!(
            both,
            Err(
                "provide only one of `referenced_image_paths` or `num_last_images_to_include`"
                    .to_string()
            )
        );
        let args = ImagegenArgs::parse(
            r#"{"prompt": "x", "referenced_image_paths": [], "num_last_images_to_include": 2}"#,
        )
        .unwrap();
        assert_eq!(args.num_last_images_to_include, Some(2));
    }

    /// A picture as a `data:` URL whose payload is its name.
    fn picture(name: &str) -> AttachmentUrl {
        AttachmentUrl::from(format!("data:image/png;base64,{name}"))
    }

    #[test]
    fn the_conversations_images_are_its_image_parts_in_order_whoever_sent_them() {
        use crate::llm::{ChatMessage, ContentPart};
        let messages = vec![
            ChatMessage::system("You are a helpful assistant."),
            ChatMessage::with_parts(
                "user",
                vec![
                    ContentPart::text("draw on this [Image #1: /p/1.png]"),
                    ContentPart::image(picture("pasted")),
                ],
            ),
            ChatMessage::assistant("Reading it."),
            ChatMessage::user("no picture here"),
            ChatMessage::with_parts(
                "user",
                vec![
                    ContentPart::text("[image] attached"),
                    ContentPart::image(picture("read")),
                    ContentPart::image(picture("mcp")),
                ],
            ),
        ];
        let images = ConversationImages::default();
        assert_eq!(images.newest(1), Err(too_few(1, 0)));
        images.record(&messages);
        assert_eq!(
            images.newest(3),
            Ok(vec![picture("pasted"), picture("read"), picture("mcp")])
        );
        // Each round records the request it answers, replacing the last.
        images.record(&messages[..3]);
        assert_eq!(images.newest(1), Ok(vec![picture("pasted")]));
        // Every clone is the same record: the round sets it, the call reads it.
        let held = images.clone();
        images.record(&messages);
        assert_eq!(held.newest(1), Ok(vec![picture("mcp")]));
    }

    /// What a count past the conversation's images is refused with.
    fn too_few(count: usize, available: usize) -> String {
        format!(
            "requested the last {count} conversation images, but only {available} were available"
        )
    }

    #[test]
    fn the_newest_images_go_up_oldest_first_and_too_few_is_an_error() {
        let images = [picture("a"), picture("b"), picture("c")];
        assert_eq!(
            newest_images(&images, 2),
            Ok(vec![picture("b"), picture("c")])
        );
        assert_eq!(newest_images(&images, 3), Ok(images.to_vec()));
        // The same picture twice counts twice: it has no name to tell a
        // repeat by.
        let twice = [picture("a"), picture("a")];
        assert_eq!(newest_images(&twice, 2), Ok(twice.to_vec()));
        // Never fewer than asked for: the call fails and nothing is sent.
        assert_eq!(newest_images(&images, 4), Err(too_few(4, 3)));
        assert_eq!(newest_images(&[], 1), Err(too_few(1, 0)));
    }

    #[test]
    fn a_new_image_is_a_generation_with_codexs_fields() {
        // The backend chooses the size and the quality.
        let request = generation("paint a blue whale");
        assert_eq!(request.endpoint(), "images/generations");
        assert_eq!(
            body(&request),
            json!({"prompt": "paint a blue whale", "background": "opaque",
                   "model": "gpt-image-2.6-sunburst", "quality": "auto", "size": "auto"})
        );
        // The model is the session's: `ALTER_ZERO_IMAGEGEN_MODEL` can name
        // another.
        let request = ImageRequest {
            model: "gpt-image-2.6-flare".to_string(),
            ..generation("paint a blue whale")
        };
        assert_eq!(body(&request)["model"], "gpt-image-2.6-flare");
    }

    #[test]
    fn an_edit_carries_its_images_as_data_urls() {
        let request = ImageRequest {
            transparent_background: true,
            images: vec![AttachmentUrl::from(
                "data:image/png;base64,Zm9v".to_string(),
            )],
            ..generation("add a red hat")
        };
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
    fn the_fact_line_round_trips_its_path_and_size() {
        let path = "/home/me/.alter-zero/generated_images/17638x12/1.png";
        let line = format_generated("PNG", (1536, 1024), 2_200_000, path);
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
        let line = format_generated("PNG", (1536, 1024), 2_200_000, "/g/1.png");
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
        let line = format_generated("PNG", (8, 8), 70, "/tmp/a.png");
        let output = format!("{line}\nmore text");
        assert_eq!(
            generated_image(&output),
            Some(("/tmp/a.png".to_string(), (8, 8)))
        );
    }

    #[test]
    fn the_model_reads_the_fact_and_codexs_note() {
        let fact = format_generated("PNG", (1024, 1024), 1_500_000, "/g/1.png");
        let text = model_result(&fact, "/g", Shown::Attached(None));
        assert!(text.starts_with(&fact), "{text}");
        assert!(text.contains("Generated images are saved to /g"), "{text}");
        assert!(text.contains("copy it"), "{text}");
        assert!(text.contains("leave the original in place"), "{text}");
        assert!(text.contains("already sees the image"), "{text}");
        assert!(!text.contains("cannot see"), "{text}");
    }

    #[test]
    fn a_model_that_cannot_see_is_told_the_picture_was_not_attached() {
        let fact = format_generated("PNG", (1024, 1024), 1_500_000, "/g/1.png");
        let text = model_result(&fact, "/g", Shown::Withheld);
        assert!(text.contains("cannot see images"), "{text}");
    }

    #[test]
    fn a_picture_sent_smaller_tells_the_model_what_it_is_looking_at() {
        let fact = format_generated("PNG", (3840, 2160), 20_000_000, "/g/1.png");
        let text = model_result(&fact, "/g", Shown::Attached(Some((1536, 864))));
        assert!(text.contains("1536x864"), "{text}");
        assert!(text.contains("full size"), "the file is untouched: {text}");
        assert!(!text.contains("cannot see"), "{text}");
    }

    #[test]
    fn a_picture_that_could_not_be_attached_never_blames_the_model() {
        let fact = format_generated("PNG", (1024, 1024), 1_500_000, "/g/1.png");
        let text = model_result(&fact, "/g", Shown::Unavailable);
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
        ImageRequest {
            prompt: prompt.to_string(),
            transparent_background: false,
            images: Vec::new(),
            model: IMAGE_MODEL.to_string(),
        }
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
        let request = ImageRequest {
            transparent_background: true,
            images: vec![AttachmentUrl::from(
                "data:image/png;base64,Zm9v".to_string(),
            )],
            ..generation("add a red hat")
        };
        request_image(&target(&base), &request, dir.path(), &CancelToken::new()).unwrap();
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
    fn only_a_refusal_of_the_model_itself_reads_as_one() {
        let refused = |status, body: &str| refuses_model(status, body, IMAGE_MODEL);
        // The error's `param` says which field it is about, when it says.
        assert!(refused(
            400,
            r#"{"error": {"param": "model", "message": "Invalid value."}}"#
        ));
        assert!(!refused(
            400,
            r#"{"error": {"param": "size", "message": "gpt-image-2.6-sunburst takes no 512x512"}}"#
        ));
        // Else a code naming an unknown model, else its words naming the model.
        for code in ["model_not_found", "invalid_model", "unsupported_model"] {
            let body = format!(r#"{{"error": {{"code": "{code}"}}}}"#);
            assert!(refused(404, &body), "{code}");
        }
        assert!(refused(
            400,
            r#"{"error": {"message": "The model gpt-image-2.6-sunburst does not exist."}}"#
        ));
        assert!(refused(
            422,
            r#"{"detail": "unknown model gpt-image-2.6-sunburst"}"#
        ));
        assert!(!refused(400, r#"{"error": {"message": "Invalid size."}}"#));
        // Not a refusal of the request at all: a limit, the sign-in, the server.
        for status in [401, 403, 429, 500] {
            let body = r#"{"error": {"param": "model", "message": "gpt-image-2.6-sunburst"}}"#;
            assert!(!refused(status, body), "{status}");
        }
    }

    /// A refusal of [`IMAGE_MODEL`] by name.
    fn model_refused() -> ImagegenError {
        ImagegenError::Refused {
            status: 400,
            body: r#"{"error": {"param": "model", "message": "Unknown model."}}"#.to_string(),
        }
    }

    #[test]
    fn a_backend_that_refuses_the_model_makes_the_image_with_the_older_one() {
        let cancel = CancelToken::new();
        let asked = std::cell::RefCell::new(Vec::new());
        let made = with_model_fallback(&generation("a fox"), &cancel, |request| {
            asked.borrow_mut().push(request.model.clone());
            if request.model == IMAGE_MODEL {
                Err(model_refused())
            } else {
                Ok(7)
            }
        });
        assert_eq!(made.unwrap(), (7, Made { fell_back: true }));
        assert_eq!(*asked.borrow(), [IMAGE_MODEL, FALLBACK_IMAGE_MODEL]);
        // A model that takes it makes it, once.
        asked.borrow_mut().clear();
        let made = with_model_fallback(&generation("a fox"), &cancel, |request| {
            asked.borrow_mut().push(request.model.clone());
            Ok(1)
        });
        assert_eq!(made.unwrap(), (1, Made::default()));
        assert_eq!(*asked.borrow(), [IMAGE_MODEL]);
    }

    #[test]
    fn any_other_refusal_or_a_model_the_run_named_is_never_sent_again() {
        // Another model would be refused the same way.
        let attempts = std::cell::Cell::new(0);
        let made: Result<((), _), _> =
            with_model_fallback(&generation("a fox"), &CancelToken::new(), |_| {
                attempts.set(attempts.get() + 1);
                Err(ImagegenError::Refused {
                    status: 400,
                    body: r#"{"error": {"param": "size"}}"#.to_string(),
                })
            });
        assert!(made.is_err());
        assert_eq!(attempts.get(), 1);
        // A model `ALTER_ZERO_IMAGEGEN_MODEL` named is the one asked for:
        // its refusal is the answer.
        let named = ImageRequest {
            model: "gpt-image-2.6-flare".to_string(),
            ..generation("a fox")
        };
        attempts.set(0);
        let made: Result<((), _), _> = with_model_fallback(&named, &CancelToken::new(), |_| {
            attempts.set(attempts.get() + 1);
            Err(model_refused())
        });
        assert!(made.is_err());
        assert_eq!(attempts.get(), 1);
    }

    #[test]
    fn an_esc_while_the_refusal_comes_back_sends_nothing_more() {
        // The older model's request would make, and bill, an image nobody
        // waits for any more.
        let cancel = CancelToken::new();
        let attempts = std::cell::Cell::new(0);
        let made: Result<((), _), _> = with_model_fallback(&generation("a fox"), &cancel, |_| {
            attempts.set(attempts.get() + 1);
            cancel.cancel();
            Err(model_refused())
        });
        assert!(matches!(
            made,
            Err(ImagegenError::Refused { status: 400, .. })
        ));
        assert_eq!(attempts.get(), 1);
    }

    #[test]
    fn when_the_older_model_fails_too_both_answers_are_told() {
        let made: Result<((), _), _> =
            with_model_fallback(&generation("a fox"), &CancelToken::new(), |request| {
                if request.model == IMAGE_MODEL {
                    Err(model_refused())
                } else {
                    Err(ImagegenError::Refused {
                        status: 400,
                        body: r#"{"error": {"param": "size", "message": "Invalid size."}}"#
                            .to_string(),
                    })
                }
            });
        // What stopped the image first, then why the older model was asked.
        assert_eq!(
            made.unwrap_err().text(0),
            "image generation failed: HTTP 400: Invalid size.\n\
             gpt-image-2 was asked after the image backend refused gpt-image-2.6-sunburst: \
             HTTP 400: Unknown model."
        );
        // A refusal with no words is its status.
        let bare = ImagegenError::FellBack {
            status: 404,
            body: String::new(),
            then: Box::new(ImagegenError::NoImage),
        };
        assert_eq!(
            bare.text(0),
            "image generation returned no image data\n\
             gpt-image-2 was asked after the image backend refused gpt-image-2.6-sunburst: \
             HTTP 404"
        );
        assert!(!bare.retryable(), "each model's request had its retries");
    }

    #[test]
    fn a_refused_model_is_asked_again_over_the_wire_with_the_older_one() {
        let refusal = r#"{"error": {"message": "The model gpt-image-2.6-sunburst does not exist.",
                          "type": "invalid_request_error", "param": "model"}}"#;
        let png = tiny_png();
        let (base, server) = serve(vec![(400, refusal.to_string()), (200, image_answer(&png))]);
        let dir = tempfile::tempdir().unwrap();
        let cancel = CancelToken::new();
        let (saved, made) =
            with_model_fallback(&generation("paint a blue whale"), &cancel, |request| {
                request_image(&target(&base), request, dir.path(), &cancel)
            })
            .unwrap();
        assert_eq!(saved.path, dir.path().join("1.png"));
        assert_eq!(made, Made { fell_back: true });
        let bodies: Vec<Value> = server
            .join()
            .unwrap()
            .iter()
            .map(|received| serde_json::from_slice(&received.body).unwrap())
            .collect();
        let models: Vec<&Value> = bodies.iter().map(|body| &body["model"]).collect();
        assert_eq!(models, [&json!(IMAGE_MODEL), &json!(FALLBACK_IMAGE_MODEL)]);
        // The same request otherwise: the older model is asked for what the
        // newest was.
        assert_eq!(bodies[0]["prompt"], bodies[1]["prompt"]);
    }

    #[test]
    fn the_model_is_told_which_model_made_its_image() {
        let dir = Path::new("/g");
        let fitted = || crate::images::FittedAttachment {
            url: AttachmentUrl::from("data:image/png;base64,Zm9v".to_string()),
            sent: None,
        };
        let note = |made: &Made| {
            let outcome = finished(&saved(dir), dir, Attachment::Attached(fitted()), made);
            // The cell says only what was made.
            assert_eq!(
                outcome.output,
                "Generated image (PNG, 1024x1536, 1.4 MB) saved to /g/1.png"
            );
            outcome.context.unwrap()
        };
        let context = note(&Made::default());
        assert!(!context.contains(FALLBACK_IMAGE_MODEL), "{context}");
        let context = note(&Made { fell_back: true });
        assert!(
            context.ends_with(
                "This image was made with gpt-image-2: the image backend did not take \
                 gpt-image-2.6-sunburst."
            ),
            "{context}"
        );
    }

    #[test]
    fn the_cell_says_when_the_older_model_made_the_image() {
        let dir = Path::new("/g");
        let note = |made: &Made| {
            finished(&saved(dir), dir, Attachment::Withheld, made)
                .note
                .clone()
        };
        assert_eq!(
            note(&Made { fell_back: true }).as_deref(),
            Some("Made with gpt-image-2 (Images 2.6 not available)")
        );
        // Made as asked: the cell has nothing to add.
        assert_eq!(note(&Made::default()), None);
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
        // A GIF is taken too: it goes up as a PNG of its first frame
        // (docs/imagegen.md).
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
            bytes: 1_500_000,
        }
    }

    /// A GIF of `colours.len()` 4×4 frames, one colour each.
    fn tiny_gif(colours: &[[u8; 3]]) -> Vec<u8> {
        let mut out = Vec::new();
        {
            let mut encoder = image::codecs::gif::GifEncoder::new(&mut out);
            for [r, g, b] in colours {
                let frame = image::RgbaImage::from_pixel(4, 4, image::Rgba([*r, *g, *b, 255]));
                encoder.encode_frame(image::Frame::new(frame)).unwrap();
            }
        }
        out
    }

    /// The picture a `data:` URL carries.
    fn data_url_picture(url: &AttachmentUrl) -> image::RgbaImage {
        let (head, b64) = url.split_once(";base64,").expect("a data: URL");
        assert_eq!(head, "data:image/png");
        let mut bytes = Vec::new();
        let mut decoder = scan::Base64Decoder::new();
        decoder.push_slice(b64.as_bytes(), &mut bytes).unwrap();
        decoder.finish(&mut bytes).unwrap();
        image::load_from_memory_with_format(&bytes, image::ImageFormat::Png)
            .unwrap()
            .to_rgba8()
    }

    #[test]
    fn a_gif_reference_goes_up_as_its_first_frame() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("loop.gif");
        std::fs::write(&path, tiny_gif(&[[255, 0, 0], [0, 255, 0], [0, 0, 255]])).unwrap();
        // An edit edits one picture: the first frame, never the GIF itself.
        let first = data_url_picture(&reference_url(&path).unwrap());
        assert_eq!(first.dimensions(), (4, 4));
        assert_eq!(first.get_pixel(2, 2).0, [255, 0, 0, 255]);
        // Any other picture goes up as the session's own encoding of it.
        let png = dir.path().join("a.png");
        std::fs::write(&png, tiny_png()).unwrap();
        let as_it_is = crate::images::attachment_data_url(&png).unwrap();
        assert_eq!(reference_url(&png).as_deref(), Some(as_it_is.as_str()));
    }

    #[test]
    fn a_saved_image_finishes_with_its_fact_line_the_note_and_the_picture() {
        let dir = Path::new("/g");
        let url = AttachmentUrl::from("data:image/png;base64,Zm9v".to_string());
        let fitted = crate::images::FittedAttachment {
            url: url.clone(),
            sent: None,
        };
        let outcome = finished(
            &saved(dir),
            dir,
            Attachment::Attached(fitted),
            &Made::default(),
        );
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
        let outcome = finished(&saved(dir), dir, Attachment::Withheld, &Made::default());
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
            model: IMAGE_MODEL.to_string(),
        }
    }

    #[test]
    fn a_call_with_bad_arguments_is_refused_before_anything_runs() {
        let outcome = run_imagegen(
            r#"{"prompt": "x", "size": "4k"}"#,
            &setup(),
            None,
            &ConversationImages::default(),
            &CancelToken::new(),
        );
        assert!(!outcome.ok);
        assert!(outcome.output.contains("`size`"), "{}", outcome.output);
    }

    #[test]
    fn a_call_with_nobody_signed_in_says_how_to_sign_in() {
        let outcome = run_imagegen(
            r#"{"prompt": "x"}"#,
            &setup(),
            None,
            &ConversationImages::default(),
            &CancelToken::new(),
        );
        assert!(!outcome.ok);
        assert_eq!(outcome.output, crate::llm::auth::NOT_SIGNED_IN);
    }

    #[test]
    fn a_count_past_the_conversations_images_is_refused_before_anything_runs() {
        let images = ConversationImages::default();
        let run = |images: &ConversationImages| {
            run_imagegen(
                r#"{"prompt": "x", "num_last_images_to_include": 2}"#,
                &setup(),
                None,
                images,
                &CancelToken::new(),
            )
        };
        let outcome = run(&images);
        assert!(!outcome.ok);
        assert_eq!(outcome.output, too_few(2, 0));
        // With the pictures there, the call goes on to the sign-in.
        images.record(&[crate::llm::ChatMessage::with_parts(
            "user",
            vec![
                crate::llm::ContentPart::image(picture("a")),
                crate::llm::ContentPart::image(picture("b")),
            ],
        )]);
        assert_eq!(run(&images).output, crate::llm::auth::NOT_SIGNED_IN);
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
        // A bare `detail`, as the backend answers some refusals.
        let text = failure_text(400, r#"{"detail":"Invalid size."}"#, 0);
        assert_eq!(text, "image generation failed: HTTP 400: Invalid size.");
        let text = failure_text(502, "", 0);
        assert_eq!(text, "image generation failed: HTTP 502");
    }
}
