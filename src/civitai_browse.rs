//! Browsing Civitai through its public REST API (`/api/v1/models`): LoRAs
//! for the LoRAs tab and ComfyUI workflows for the Workflows tab, as
//! sorted/filtered pages, one model's details, and the list of base models
//! for the filter.
//!
//! Workflows are limited to those whose files include a `.json` workflow
//! (many ship a `.zip` instead); their details list only the `.json` files.
//!
//! Mature content: unless `include_mature` is set, models Civitai flags as
//! NSFW are dropped and only PG-rated images (`nsfwLevel` 1) are kept, on
//! top of Civitai's own `nsfw=false` filtering. With it set, every image is
//! returned and marked `mature`.

use anyhow::{anyhow, Context, Result};
use reqwest::{Client, Url};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

const API_BASE: &str = "https://civitai.com/api/v1";
/// LoRA-like model types Civitai lists separately.
const LORA_TYPES: [&str; 3] = ["LORA", "LoCon", "DoRA"];
const WORKFLOW_TYPES: [&str; 1] = ["Workflows"];
pub const SORTS: [&str; 3] = ["Most Downloaded", "Highest Rated", "Newest"];
pub const PERIODS: [&str; 5] = ["AllTime", "Year", "Month", "Week", "Day"];
const PAGE_SIZE: u32 = 24;
/// Larger pages for workflows: only those with a `.json` file are kept.
const WORKFLOW_PAGE_SIZE: u32 = 40;
const CACHE_TTL: Duration = Duration::from_secs(5 * 60);
const ENUMS_TTL: Duration = Duration::from_secs(24 * 60 * 60);
const CACHE_MAX_ENTRIES: usize = 200;
/// Civitai's `nsfwLevel` for PG content; higher levels are PG-13 and up.
const PG_LEVEL: u64 = 1;

/// What a browser lists.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Loras,
    Workflows,
}

impl Kind {
    fn types(self) -> &'static [&'static str] {
        match self {
            Kind::Loras => &LORA_TYPES,
            Kind::Workflows => &WORKFLOW_TYPES,
        }
    }

    fn page_size(self) -> u32 {
        match self {
            Kind::Loras => PAGE_SIZE,
            Kind::Workflows => WORKFLOW_PAGE_SIZE,
        }
    }

    fn noun(self) -> &'static str {
        match self {
            Kind::Loras => "LoRA",
            Kind::Workflows => "workflow",
        }
    }

    /// Whether a file of a version counts (workflows: `.json` only).
    fn keeps_file(self, name: &str) -> bool {
        match self {
            Kind::Loras => true,
            Kind::Workflows => is_json_file_name(name),
        }
    }
}

fn is_json_file_name(name: &str) -> bool {
    name.to_ascii_lowercase().ends_with(".json")
}

/// What to list.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BrowseQuery {
    #[serde(default)]
    pub sort: Option<String>,
    #[serde(default)]
    pub period: Option<String>,
    #[serde(default)]
    pub base_model: Option<String>,
    #[serde(default)]
    pub query: Option<String>,
    #[serde(default)]
    pub cursor: Option<String>,
}

/// An image or video shown for a LoRA, with URLs resized by Civitai's CDN.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct Media {
    /// A still image about 450 px wide (first frame for videos).
    pub thumb_url: String,
    /// A larger still image for the gallery.
    pub image_url: String,
    /// A playable MP4 for videos.
    pub video_url: Option<String>,
    /// Rated above PG; the UI blurs it until clicked.
    pub mature: bool,
    pub width: Option<u32>,
    pub height: Option<u32>,
}

/// One LoRA in a browse page.
#[derive(Debug, Clone, Serialize)]
pub struct LoraCard {
    pub id: u64,
    pub name: String,
    pub creator: Option<String>,
    pub base_model: Option<String>,
    pub downloads: u64,
    pub likes: u64,
    pub mature: bool,
    pub thumb: Option<Media>,
    /// The latest version's primary file name, to mark installed LoRAs.
    pub file_name: Option<String>,
    pub version_count: usize,
}

#[derive(Debug, Clone, Serialize)]
pub struct BrowsePage {
    pub items: Vec<LoraCard>,
    pub next_cursor: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct LoraFile {
    pub id: u64,
    pub name: String,
    pub size_bytes: Option<u64>,
    pub download_url: String,
    pub primary: bool,
    pub kind: Option<String>,
    pub sha256: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct LoraVersion {
    pub id: u64,
    pub name: String,
    pub base_model: Option<String>,
    pub trained_words: Vec<String>,
    pub published_at: Option<String>,
    /// Early-access or paid versions can't be downloaded without buying access.
    pub restricted: bool,
    pub files: Vec<LoraFile>,
    pub media: Vec<Media>,
}

#[derive(Debug, Clone, Serialize)]
pub struct LoraDetails {
    pub id: u64,
    pub name: String,
    pub creator: Option<String>,
    pub page_url: String,
    pub description: String,
    pub tags: Vec<String>,
    pub mature: bool,
    pub downloads: u64,
    pub likes: u64,
    pub versions: Vec<LoraVersion>,
}

// --- Civitai API shapes (only the fields used) ---

#[derive(Debug, Deserialize)]
struct ApiPage {
    #[serde(default)]
    items: Vec<ApiModel>,
    #[serde(default)]
    metadata: ApiPageMeta,
}

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ApiPageMeta {
    next_cursor: Option<serde_json::Value>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ApiModel {
    id: u64,
    #[serde(default)]
    name: String,
    #[serde(default)]
    description: Option<String>,
    #[serde(default)]
    nsfw: bool,
    #[serde(default)]
    tags: Vec<String>,
    #[serde(default)]
    creator: Option<ApiCreator>,
    #[serde(default)]
    stats: ApiStats,
    #[serde(default)]
    model_versions: Vec<ApiVersion>,
}

#[derive(Debug, Deserialize)]
struct ApiCreator {
    #[serde(default)]
    username: Option<String>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ApiStats {
    #[serde(default)]
    download_count: u64,
    #[serde(default)]
    thumbs_up_count: u64,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ApiVersion {
    id: u64,
    #[serde(default)]
    name: String,
    #[serde(default)]
    base_model: Option<String>,
    #[serde(default)]
    trained_words: Vec<String>,
    #[serde(default)]
    published_at: Option<String>,
    #[serde(default)]
    availability: Option<String>,
    #[serde(default)]
    files: Vec<ApiFile>,
    #[serde(default)]
    images: Vec<ApiImage>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ApiFile {
    id: u64,
    #[serde(default)]
    name: String,
    #[serde(default, rename = "sizeKB")]
    size_kb: Option<f64>,
    #[serde(default)]
    download_url: Option<String>,
    #[serde(default)]
    primary: Option<bool>,
    #[serde(default, rename = "type")]
    kind: Option<String>,
    #[serde(default)]
    hashes: HashMap<String, String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ApiImage {
    url: String,
    #[serde(default)]
    nsfw_level: Option<u64>,
    #[serde(default)]
    width: Option<u32>,
    #[serde(default)]
    height: Option<u32>,
    #[serde(default, rename = "type")]
    kind: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "PascalCase")]
struct ApiEnums {
    #[serde(default)]
    active_base_model: Vec<String>,
}

// --- HTTP with a small in-memory cache ---

fn client() -> &'static Client {
    static CLIENT: OnceLock<Client> = OnceLock::new();
    CLIENT.get_or_init(|| {
        Client::builder()
            .timeout(Duration::from_secs(30))
            .user_agent(concat!("ComfyPilot/", env!("CARGO_PKG_VERSION")))
            .build()
            .unwrap_or_default()
    })
}

fn cache() -> &'static Mutex<HashMap<String, (Instant, String)>> {
    static CACHE: OnceLock<Mutex<HashMap<String, (Instant, String)>>> = OnceLock::new();
    CACHE.get_or_init(|| Mutex::new(HashMap::new()))
}

/// GETs `url` and returns the body, from the cache when it's younger than `ttl`.
async fn get_cached(url: &str, ttl: Duration) -> Result<String> {
    if let Ok(cache) = cache().lock() {
        if let Some((at, body)) = cache.get(url) {
            if at.elapsed() < ttl {
                return Ok(body.clone());
            }
        }
    }
    let response = client()
        .get(url)
        .send()
        .await
        .context("Civitai could not be reached")?;
    let status = response.status();
    if !status.is_success() {
        return Err(anyhow!("Civitai returned HTTP {status}"));
    }
    let body = response.text().await.context("reading Civitai's reply")?;
    if let Ok(mut cache) = cache().lock() {
        if cache.len() >= CACHE_MAX_ENTRIES {
            cache.retain(|_, (at, _)| at.elapsed() < CACHE_TTL);
            if cache.len() >= CACHE_MAX_ENTRIES {
                cache.clear();
            }
        }
        cache.insert(url.to_string(), (Instant::now(), body.clone()));
    }
    Ok(body)
}

// --- Public API ---

/// The browse URL for `query` (sort and period fall back to Most Downloaded
/// over the past week when missing or not one of Civitai's values).
pub fn browse_url(kind: Kind, query: &BrowseQuery, include_mature: bool) -> String {
    let pick = |value: &Option<String>, allowed: &[&str], fallback: &str| -> String {
        value
            .as_deref()
            .map(str::trim)
            .filter(|v| allowed.contains(v))
            .unwrap_or(fallback)
            .to_string()
    };
    let mut url = Url::parse(&format!("{API_BASE}/models")).expect("valid base URL");
    {
        let mut params = url.query_pairs_mut();
        for model_type in kind.types() {
            params.append_pair("types", model_type);
        }
        params.append_pair("sort", &pick(&query.sort, &SORTS, SORTS[0]));
        // Civitai returns nothing for a text search limited to a period.
        let period = if non_empty(&query.query).is_some() {
            "AllTime".to_string()
        } else {
            pick(&query.period, &PERIODS, "Week")
        };
        params.append_pair("period", &period);
        params.append_pair("limit", &kind.page_size().to_string());
        params.append_pair("nsfw", if include_mature { "true" } else { "false" });
        if let Some(base) = non_empty(&query.base_model) {
            params.append_pair("baseModels", base);
        }
        if let Some(text) = non_empty(&query.query) {
            params.append_pair("query", text);
        }
        if let Some(cursor) = non_empty(&query.cursor) {
            params.append_pair("cursor", cursor);
        }
    }
    url.to_string()
}

fn non_empty(value: &Option<String>) -> Option<&str> {
    value.as_deref().map(str::trim).filter(|v| !v.is_empty())
}

/// One page of LoRAs or workflows.
pub async fn browse(kind: Kind, query: &BrowseQuery, include_mature: bool) -> Result<BrowsePage> {
    let body = get_cached(&browse_url(kind, query, include_mature), CACHE_TTL).await?;
    parse_browse_page(kind, &body, include_mature)
}

/// A LoRA's or workflow's details with all its versions (for a workflow,
/// the versions that have a `.json` file, and only those files).
pub async fn details(kind: Kind, model_id: u64, include_mature: bool) -> Result<LoraDetails> {
    let body = get_cached(&format!("{API_BASE}/models/{model_id}"), CACHE_TTL).await?;
    let model: ApiModel = serde_json::from_str(&body).context("unexpected reply from Civitai")?;
    if model.nsfw && !include_mature {
        return Err(anyhow!(
            "This {} is marked as mature content. Turn on mature content to view it.",
            kind.noun()
        ));
    }
    Ok(details_from(kind, model, include_mature))
}

/// Civitai's current base models, for the filter.
pub async fn base_models() -> Result<Vec<String>> {
    let body = get_cached(&format!("{API_BASE}/enums"), ENUMS_TTL).await?;
    let enums: ApiEnums = serde_json::from_str(&body).context("unexpected reply from Civitai")?;
    Ok(enums.active_base_model)
}

fn parse_browse_page(kind: Kind, body: &str, include_mature: bool) -> Result<BrowsePage> {
    let page: ApiPage = serde_json::from_str(body).context("unexpected reply from Civitai")?;
    let items = page
        .items
        .into_iter()
        .filter(|model| include_mature || !model.nsfw)
        .filter_map(|model| card_from(kind, model, include_mature))
        .collect();
    let next_cursor = page.metadata.next_cursor.and_then(|cursor| match cursor {
        serde_json::Value::String(s) if !s.is_empty() => Some(s),
        serde_json::Value::Number(n) => Some(n.to_string()),
        _ => None,
    });
    Ok(BrowsePage { items, next_cursor })
}

/// The card for a model; none for a workflow without a `.json` file.
fn card_from(kind: Kind, model: ApiModel, include_mature: bool) -> Option<LoraCard> {
    let latest = model.model_versions.first();
    let file_name = match kind {
        Kind::Loras => latest.and_then(primary_file).map(|f| f.name.clone()),
        Kind::Workflows => Some(
            latest?
                .files
                .iter()
                .find(|f| is_json_file_name(&f.name))?
                .name
                .clone(),
        ),
    };
    Some(LoraCard {
        id: model.id,
        name: model.name,
        creator: model.creator.and_then(|c| c.username),
        base_model: latest.and_then(|v| v.base_model.clone()),
        downloads: model.stats.download_count,
        likes: model.stats.thumbs_up_count,
        mature: model.nsfw,
        thumb: latest.and_then(|v| visible_media(&v.images, include_mature).into_iter().next()),
        file_name,
        version_count: model.model_versions.len(),
    })
}

fn details_from(kind: Kind, model: ApiModel, include_mature: bool) -> LoraDetails {
    let versions = model
        .model_versions
        .iter()
        .filter(|version| version.files.iter().any(|f| kind.keeps_file(&f.name)))
        .map(|version| LoraVersion {
            id: version.id,
            name: version.name.clone(),
            base_model: version.base_model.clone(),
            trained_words: version
                .trained_words
                .iter()
                .map(|w| w.trim().trim_end_matches(',').trim().to_string())
                .filter(|w| !w.is_empty())
                .collect(),
            published_at: version.published_at.clone(),
            restricted: version
                .availability
                .as_deref()
                .is_some_and(|a| !a.eq_ignore_ascii_case("Public")),
            files: version
                .files
                .iter()
                .filter(|file| kind.keeps_file(&file.name))
                .filter_map(|file| {
                    Some(LoraFile {
                        id: file.id,
                        name: file.name.clone(),
                        size_bytes: file.size_kb.map(|kb| (kb * 1024.0).round() as u64),
                        download_url: file.download_url.clone()?,
                        primary: file.primary.unwrap_or(false),
                        kind: file.kind.clone(),
                        sha256: file
                            .hashes
                            .iter()
                            .find(|(k, _)| k.eq_ignore_ascii_case("SHA256"))
                            .map(|(_, v)| v.to_ascii_lowercase()),
                    })
                })
                .collect(),
            media: visible_media(&version.images, include_mature),
        })
        .collect();
    LoraDetails {
        id: model.id,
        page_url: format!("https://civitai.com/models/{}", model.id),
        name: model.name,
        creator: model.creator.and_then(|c| c.username),
        description: model
            .description
            .as_deref()
            .map(html_to_text)
            .unwrap_or_default(),
        tags: model.tags,
        mature: model.nsfw,
        downloads: model.stats.download_count,
        likes: model.stats.thumbs_up_count,
        versions,
    }
}

fn primary_file(version: &ApiVersion) -> Option<&ApiFile> {
    version
        .files
        .iter()
        .find(|f| f.primary == Some(true))
        .or_else(|| {
            version
                .files
                .iter()
                .find(|f| f.kind.as_deref() == Some("Model"))
        })
        .or_else(|| version.files.first())
}

/// The images to show: PG-rated only unless mature content is on. An image
/// without a rating counts as mature.
fn visible_media(images: &[ApiImage], include_mature: bool) -> Vec<Media> {
    images
        .iter()
        .filter_map(|image| {
            let mature = image.nsfw_level.is_none_or(|level| level > PG_LEVEL);
            if mature && !include_mature {
                return None;
            }
            let is_video = image.kind.as_deref() == Some("video")
                || image.url.to_ascii_lowercase().ends_with(".mp4")
                || image.url.to_ascii_lowercase().ends_with(".webm");
            // A still: a video needs `transcode=true` (else the CDN sends the
            // whole MP4), while an image with it gets the full-size original.
            let still = if is_video {
                "anim=false,transcode=true"
            } else {
                "anim=false"
            };
            Some(Media {
                thumb_url: cdn_variant(&image.url, &format!("{still},width=450"), "jpeg")?,
                image_url: cdn_variant(&image.url, &format!("{still},width=1024"), "jpeg")?,
                video_url: if is_video {
                    cdn_variant(&image.url, "transcode=true,width=720", "mp4")
                } else {
                    None
                },
                mature,
                width: image.width,
                height: image.height,
            })
        })
        .collect()
}

/// Rewrites a Civitai CDN URL (`…/<uuid>/<options>/<name>.<ext>`) to other
/// CDN options and extension. Only image.civitai.com URLs are accepted.
fn cdn_variant(url: &str, options: &str, ext: &str) -> Option<String> {
    let parsed = Url::parse(url).ok()?;
    if parsed.scheme() != "https" || parsed.host_str() != Some("image.civitai.com") {
        return None;
    }
    let (rest, file) = url.rsplit_once('/')?;
    let (prefix, last) = rest.rsplit_once('/')?;
    // `last` is the options segment ("original=true", "width=450") when
    // present; otherwise it's the image's uuid folder.
    let base = if last.contains('=') { prefix } else { rest };
    let stem = file
        .split('?')
        .next()?
        .rsplit_once('.')
        .map_or(file, |(s, _)| s);
    Some(format!("{base}/{options}/{stem}.{ext}"))
}

/// Plain text from Civitai's HTML descriptions, keeping paragraph and list
/// breaks.
pub fn html_to_text(html: &str) -> String {
    let mut out = String::with_capacity(html.len());
    let mut rest = html;
    while let Some(start) = rest.find('<') {
        out.push_str(&rest[..start]);
        let Some(end) = rest[start..].find('>') else {
            rest = "";
            break;
        };
        let inner = &rest[start + 1..start + end];
        let closing = inner.starts_with('/');
        let tag = inner
            .trim_start_matches('/')
            .split(|c: char| c.is_whitespace() || c == '/')
            .next()
            .unwrap_or("")
            .to_ascii_lowercase();
        match tag.as_str() {
            "br" | "p" | "div" | "h1" | "h2" | "h3" | "h4" | "h5" | "h6" | "tr" => out.push('\n'),
            "li" if !closing => out.push_str("\n• "),
            _ => {}
        }
        rest = &rest[start + end + 1..];
    }
    out.push_str(rest);
    let decoded = out
        .replace("&nbsp;", " ")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&#39;", "'")
        .replace("&amp;", "&");
    // Collapse runs of blank lines and trailing spaces.
    let mut text = String::new();
    let mut blank = 0;
    for line in decoded
        .lines()
        .map(|l| l.split_whitespace().collect::<Vec<_>>().join(" "))
    {
        if line.is_empty() || line == "•" {
            blank += 1;
            continue;
        }
        if !text.is_empty() {
            text.push_str(if blank > 0 { "\n\n" } else { "\n" });
        }
        blank = 0;
        text.push_str(&line);
    }
    text
}

#[cfg(test)]
mod tests {
    use super::*;

    const PAGE: &str = r#"{
      "items": [
        {"id": 1, "name": "Safe", "nsfw": false, "tags": ["style"],
         "creator": {"username": "alice"},
         "stats": {"downloadCount": 10, "thumbsUpCount": 3},
         "modelVersions": [{"id": 11, "name": "v1", "baseModel": "Flux.1 D",
           "trainedWords": ["sks style,"], "availability": "Public",
           "files": [{"id": 111, "name": "safe.safetensors", "sizeKB": 1024,
                      "primary": true, "type": "Model",
                      "downloadUrl": "https://civitai.com/api/download/models/11",
                      "hashes": {"SHA256": "ABC"}}],
           "images": [
             {"url": "https://image.civitai.com/x/uuid1/original=true/1.jpeg", "nsfwLevel": 4, "type": "image"},
             {"url": "https://image.civitai.com/x/uuid2/original=true/2.mp4", "nsfwLevel": 1, "type": "video"}
           ]}]},
        {"id": 2, "name": "Mature", "nsfw": true, "modelVersions": []}
      ],
      "metadata": {"nextCursor": "abc|1"}
    }"#;

    #[test]
    fn safe_mode_drops_mature_models_and_images() {
        let page = parse_browse_page(Kind::Loras, PAGE, false).unwrap();
        assert_eq!(page.items.len(), 1);
        let card = &page.items[0];
        assert_eq!(card.file_name.as_deref(), Some("safe.safetensors"));
        let thumb = card.thumb.as_ref().unwrap();
        assert!(!thumb.mature);
        assert_eq!(
            thumb.thumb_url,
            "https://image.civitai.com/x/uuid2/anim=false,transcode=true,width=450/2.jpeg"
        );
        assert_eq!(
            thumb.video_url.as_deref(),
            Some("https://image.civitai.com/x/uuid2/transcode=true,width=720/2.mp4")
        );
        let page = parse_browse_page(Kind::Loras, PAGE, true).unwrap();
        let image = page.items[0].thumb.as_ref().unwrap();
        assert_eq!(
            image.thumb_url,
            "https://image.civitai.com/x/uuid1/anim=false,width=450/1.jpeg"
        );
        assert!(image.video_url.is_none());
        assert_eq!(page.next_cursor.as_deref(), Some("abc|1"));
    }

    #[test]
    fn mature_mode_keeps_everything_and_marks_it() {
        let page = parse_browse_page(Kind::Loras, PAGE, true).unwrap();
        assert_eq!(page.items.len(), 2);
        assert!(page.items[0].thumb.as_ref().unwrap().mature);
    }

    #[test]
    fn details_clean_up_words_files_and_description() {
        let mut model: ApiModel = serde_json::from_str::<ApiPage>(PAGE)
            .unwrap()
            .items
            .remove(0);
        model.description =
            Some("<p>Use <b>sks</b> &amp; enjoy</p><ul><li>one</li><li>two</li></ul>".into());
        let details = details_from(Kind::Loras, model, false);
        let version = &details.versions[0];
        assert_eq!(version.trained_words, ["sks style"]);
        assert_eq!(version.files[0].size_bytes, Some(1024 * 1024));
        assert_eq!(version.files[0].sha256.as_deref(), Some("abc"));
        assert!(!version.restricted);
        assert_eq!(version.media.len(), 1);
        assert_eq!(details.description, "Use sks & enjoy\n\n• one\n• two");
        assert_eq!(details.page_url, "https://civitai.com/models/1");
    }

    #[test]
    fn browse_urls_only_use_known_sorts() {
        let url = browse_url(
            Kind::Loras,
            &BrowseQuery {
                sort: Some("Bogus".into()),
                period: Some("Day".into()),
                base_model: Some("Flux.1 D".into()),
                query: Some("  ".into()),
                cursor: None,
            },
            false,
        );
        assert!(url.contains("sort=Most+Downloaded"));
        assert!(url.contains("period=Day"));
        assert!(url.contains("baseModels=Flux.1+D"));
        assert!(url.contains("nsfw=false"));
        assert!(!url.contains("query="));
        assert_eq!(url.matches("types=").count(), 3);

        let search = browse_url(
            Kind::Loras,
            &BrowseQuery {
                period: Some("Week".into()),
                query: Some("pixel art".into()),
                ..Default::default()
            },
            false,
        );
        assert!(search.contains("period=AllTime"));
        assert!(search.contains("query=pixel+art"));
    }

    const WORKFLOWS: &str = r#"{
      "items": [
        {"id": 5, "name": "JSON workflow", "nsfw": false,
         "modelVersions": [
           {"id": 51, "name": "v2", "files": [
             {"id": 511, "name": "preview.zip", "type": "Archive", "downloadUrl": "https://civitai.com/api/download/models/51?type=Archive"},
             {"id": 512, "name": "Flow_v2.JSON", "type": "Config", "downloadUrl": "https://civitai.com/api/download/models/51"}]},
           {"id": 52, "name": "v1 (zip only)", "files": [
             {"id": 521, "name": "flow_v1.zip", "type": "Archive", "downloadUrl": "https://civitai.com/api/download/models/52"}]}]},
        {"id": 6, "name": "Zip workflow", "nsfw": false,
         "modelVersions": [{"id": 61, "name": "v1", "files": [
           {"id": 611, "name": "flow.zip", "type": "Archive", "downloadUrl": "https://civitai.com/api/download/models/61"}]}]}
      ],
      "metadata": {}
    }"#;

    #[test]
    fn workflows_need_a_json_file() {
        let page = parse_browse_page(Kind::Workflows, WORKFLOWS, false).unwrap();
        assert_eq!(page.items.len(), 1);
        assert_eq!(page.items[0].file_name.as_deref(), Some("Flow_v2.JSON"));

        let model: ApiModel = serde_json::from_str::<ApiPage>(WORKFLOWS)
            .unwrap()
            .items
            .remove(0);
        let details = details_from(Kind::Workflows, model, false);
        assert_eq!(details.versions.len(), 1);
        let files: Vec<_> = details.versions[0].files.iter().map(|f| f.id).collect();
        assert_eq!(files, [512]);

        let url = browse_url(Kind::Workflows, &BrowseQuery::default(), false);
        assert!(url.contains("types=Workflows"));
        assert_eq!(url.matches("types=").count(), 1);
        assert!(url.contains("limit=40"));
    }

    #[test]
    fn cdn_variants_reject_other_hosts() {
        assert!(cdn_variant(
            "https://evil.example/x/original=true/1.jpeg",
            "width=1",
            "jpeg"
        )
        .is_none());
        assert_eq!(
            cdn_variant(
                "https://image.civitai.com/x/uuid/1.jpeg",
                "width=450",
                "jpeg"
            )
            .as_deref(),
            Some("https://image.civitai.com/x/uuid/width=450/1.jpeg")
        );
    }
}
