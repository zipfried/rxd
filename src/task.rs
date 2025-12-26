use std::path::PathBuf;
use std::str::FromStr;
use std::sync::Arc;

use chrono::{DateTime, FixedOffset, Local};
use futures::stream;
use futures::stream::StreamExt;
use reqwest::Client;
use reqwest::header::{AUTHORIZATION, COOKIE, REFERER, USER_AGENT};
use reqwest::header::{HeaderMap, HeaderName, HeaderValue};
use serde::Deserialize;
use serde_json::{Value, json};
use tokio::fs;
use tokio::io::AsyncWriteExt;
use tracing::{error, info, instrument, trace, warn};

const DEFAULT_USER_AGENT: &str = "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/114.0.0.0 Safari/537.36";
const DEFAULT_AUTHORIZATION: &str = "Bearer AAAAAAAAAAAAAAAAAAAAANRILgAAAAAAnNwIzUejRCOuH5E6I8xnZz4puTs%3D1Zv7ttfk8LF81IUq16cHjhLTvJu4FA33AGWWjCpTnA";

#[derive(Debug, Deserialize)]
pub struct Config {
    pub auth_token: String,
    pub ct0: String,
    #[serde(default = "default_concurrent_downloads")]
    pub concurrent_downloads: usize,
    pub tasks: Vec<TaskConfig>,
}

fn default_concurrent_downloads() -> usize {
    4
}

#[derive(Debug, Deserialize)]
pub struct TaskConfig {
    pub screen_name: String,
    #[serde(default)]
    pub save_path: Option<String>,
}

#[derive(Debug, Clone)]
struct User {
    screen_name: String,
    name: String,
    rest_id: String,
    media_count: u64,
}

#[derive(Debug, Clone)]
struct MediaItem {
    url: String,
    media_type: MediaType,
    timestamp: DateTime<FixedOffset>,
}

#[derive(Debug, Clone)]
enum MediaType {
    Image,
    Video,
}

pub struct Task {
    client: Client,
    user: User,
    save_path: PathBuf,
    concurrent_downloads: usize,
}

impl Task {
    #[instrument(skip_all)]
    pub async fn new(
        screen_name: &str,
        auth_token: &str,
        ct0: &str,
        concurrent_downloads: usize,
        save_path: Option<&str>,
    ) -> Result<Self, Box<dyn std::error::Error>> {
        let client = build_client(screen_name, auth_token, ct0)?;
        let user = fetch_user_info(&client, screen_name).await?;

        let save_path = if let Some(custom_path) = save_path {
            PathBuf::from(custom_path)
        } else {
            PathBuf::from("downloads").join(&user.screen_name)
        };
        fs::create_dir_all(&save_path).await?;

        info!(
            "task created for @{} ({}) - {} media tweets",
            user.screen_name, user.name, user.media_count
        );

        Ok(Self {
            client,
            user,
            save_path,
            concurrent_downloads,
        })
    }

    #[instrument(skip_all)]
    pub async fn execute(self: Arc<Self>) -> Result<(), Box<dyn std::error::Error>> {
        info!(
            "starting download with {} concurrent downloads",
            self.concurrent_downloads
        );

        let mut cursor: Option<String> = None;
        let mut total_downloaded = 0u64;
        let mut page = 0u32;

        loop {
            page += 1;
            info!("fetching page {}", page);

            let (media_items, next_cursor) = self.fetch_user_media(cursor.as_deref()).await?;

            if media_items.is_empty() {
                info!("no more media items found");
                break;
            }

            info!("found {} media items on page {}", media_items.len(), page);

            let self_clone = Arc::clone(&self);
            let results: Vec<_> = stream::iter(media_items.iter())
                .map(|item| {
                    let task = Arc::clone(&self_clone);
                    let local_dt = item.timestamp.with_timezone(&Local);
                    let date_str = local_dt.format("%Y-%m-%d").to_string();
                    async move {
                        match task.download_media(item, &date_str).await {
                            Ok(path) => {
                                info!("downloaded: {}", path.display());
                                Ok(())
                            }
                            Err(e) => {
                                warn!("failed to download {}: {}", item.url, e);
                                Err(e)
                            }
                        }
                    }
                })
                .buffer_unordered(self.concurrent_downloads)
                .collect()
                .await;

            let successful = results.iter().filter(|r| r.is_ok()).count();
            total_downloaded += successful as u64;

            match next_cursor {
                Some(c) => cursor = Some(c),
                None => {
                    info!("No more pages");
                    break;
                }
            }
        }

        info!(
            "download complete for @{}: {} items downloaded",
            self.user.screen_name, total_downloaded
        );
        Ok(())
    }

    #[instrument(skip_all)]
    async fn fetch_user_media(
        &self,
        cursor: Option<&str>,
    ) -> Result<(Vec<MediaItem>, Option<String>), Box<dyn std::error::Error>> {
        let variables = if let Some(c) = cursor {
            json!({
                "userId": self.user.rest_id,
                "count": 100,
                "cursor": c,
                "includePromotedContent": false,
                "withClientEventToken": false,
                "withBirdwatchNotes": false,
                "withVoice": true,
                "withV2Timeline": true
            })
        } else {
            json!({
                "userId": self.user.rest_id,
                "count": 100,
                "includePromotedContent": false,
                "withClientEventToken": false,
                "withBirdwatchNotes": false,
                "withVoice": true,
                "withV2Timeline": true
            })
        };

        let features = json!({
            "responsive_web_graphql_exclude_directive_enabled": true,
            "verified_phone_label_enabled": false,
            "creator_subscriptions_tweet_preview_api_enabled": true,
            "responsive_web_graphql_timeline_navigation_enabled": true,
            "responsive_web_graphql_skip_user_profile_image_extensions_enabled": false,
            "tweetypie_unmention_optimization_enabled": true,
            "responsive_web_edit_tweet_api_enabled": true,
            "graphql_is_translatable_rweb_tweet_is_translatable_enabled": true,
            "view_counts_everywhere_api_enabled": true,
            "longform_notetweets_consumption_enabled": true,
            "responsive_web_twitter_article_tweet_consumption_enabled": false,
            "tweet_awards_web_tipping_enabled": false,
            "freedom_of_speech_not_reach_fetch_enabled": true,
            "standardized_nudges_misinfo": true,
            "tweet_with_visibility_results_prefer_gql_limited_actions_policy_enabled": true,
            "longform_notetweets_rich_text_read_enabled": true,
            "longform_notetweets_inline_media_enabled": true,
            "responsive_web_media_download_video_enabled": false,
            "responsive_web_enhance_cards_enabled": false
        });

        let response = self
            .client
            .get("https://twitter.com/i/api/graphql/Le6KlbilFmSu-5VltFND-Q/UserMedia")
            .query(&[
                ("variables", serde_json::to_string(&variables)?),
                ("features", serde_json::to_string(&features)?),
            ])
            .send()
            .await?;

        let status = response.status();
        trace!("UserMedia response status: {}", status);

        if !status.is_success() {
            let body = response.text().await?;
            error!("UserMedia API error: {}", body);
            return Err(format!("API error: {}", status).into());
        }

        let body = response.text().await?;
        let raw: Value = serde_json::from_str(&body)?;

        let (media_items, next_cursor) = parse_user_media_response(&raw)?;

        Ok((media_items, next_cursor))
    }

    #[instrument(skip_all)]
    async fn download_media(
        &self,
        item: &MediaItem,
        date_str: &str,
    ) -> Result<PathBuf, Box<dyn std::error::Error>> {
        let download_url = match item.media_type {
            MediaType::Image => format!("{}?name=orig", item.url),
            MediaType::Video => item.url.clone(),
        };

        let ext = match item.media_type {
            MediaType::Image => item.url.rsplit('.').next().unwrap_or("jpg"),
            MediaType::Video => "mp4",
        };

        let media_id = item
            .url
            .rsplit('/')
            .next()
            .and_then(|s| s.split('.').next())
            .unwrap_or("unknown");

        let filename = format!("{}-{}.{}", date_str, media_id, ext);
        let filepath = self.save_path.join(&filename);

        if filepath.exists() {
            info!("file already exists, skipping: {}", filepath.display());
            return Ok(filepath);
        }

        let response = self.client.get(&download_url).send().await?;

        if !response.status().is_success() {
            return Err(format!("download failed: {}", response.status()).into());
        }

        let bytes = response.bytes().await?;

        let mut file = fs::File::create(&filepath).await?;
        file.write_all(&bytes).await?;

        Ok(filepath)
    }
}

#[instrument(skip_all)]
fn build_client(
    screen_name: &str,
    auth_token: &str,
    ct0: &str,
) -> Result<Client, Box<dyn std::error::Error>> {
    let mut headers = HeaderMap::new();
    headers.insert(USER_AGENT, HeaderValue::from_static(DEFAULT_USER_AGENT));
    headers.insert(
        AUTHORIZATION,
        HeaderValue::from_static(DEFAULT_AUTHORIZATION),
    );
    headers.insert(
        COOKIE,
        HeaderValue::from_str(&format!("auth_token={auth_token}; ct0={ct0};"))?,
    );
    headers.insert(
        HeaderName::from_str("x-csrf-token")?,
        HeaderValue::from_str(ct0)?,
    );
    headers.insert(
        REFERER,
        HeaderValue::from_str(&format!("https://twitter.com/{screen_name}"))?,
    );

    let client = Client::builder().default_headers(headers).build()?;
    Ok(client)
}

#[instrument(skip_all)]
async fn fetch_user_info(
    client: &Client,
    screen_name: &str,
) -> Result<User, Box<dyn std::error::Error>> {
    let variables = json!({
        "screen_name": screen_name,
        "withSafetyModeUserFields": false,
    });

    let features = json!({
        "hidden_profile_likes_enabled": false,
        "hidden_profile_subscriptions_enabled": false,
        "responsive_web_graphql_exclude_directive_enabled": true,
        "verified_phone_label_enabled": false,
        "subscriptions_verification_info_verified_since_enabled": true,
        "highlights_tweets_tab_ui_enabled": true,
        "creator_subscriptions_tweet_preview_api_enabled": true,
        "responsive_web_graphql_skip_user_profile_image_extensions_enabled": false,
        "responsive_web_graphql_timeline_navigation_enabled": true,
    });

    let field_toggles = json!({
        "withAuxiliaryUserLabels": false,
    });

    let response = client
        .get("https://twitter.com/i/api/graphql/xc8f1g7BYqr6VTzTbvNlGw/UserByScreenName")
        .query(&[
            ("variables", serde_json::to_string(&variables)?),
            ("features", serde_json::to_string(&features)?),
            ("fieldToggles", serde_json::to_string(&field_toggles)?),
        ])
        .send()
        .await?;

    let status = response.status();
    trace!("UserByScreenName response status: {}", status);

    if !status.is_success() {
        let body = response.text().await?;
        error!("UserByScreenName API error: {}", body);
        return Err(format!("API error: {}", status).into());
    }

    let body = response.text().await?;
    let raw: Value = serde_json::from_str(&body)?;

    let result = raw
        .pointer("/data/user/result")
        .ok_or("Failed to find user result")?;
    let legacy = result.get("legacy").ok_or("Failed to find legacy")?;

    let rest_id = result
        .get("rest_id")
        .and_then(|v| v.as_str())
        .ok_or("Failed to get rest_id")?
        .to_string();
    let name = legacy
        .get("name")
        .and_then(|v| v.as_str())
        .ok_or("Failed to get name")?
        .to_string();
    let media_count = legacy
        .get("media_count")
        .and_then(|v| v.as_u64())
        .ok_or("Failed to get media_count")?;

    Ok(User {
        screen_name: screen_name.to_string(),
        name,
        rest_id,
        media_count,
    })
}

#[instrument(skip_all)]
fn parse_user_media_response(
    raw: &Value,
) -> Result<(Vec<MediaItem>, Option<String>), Box<dyn std::error::Error>> {
    let mut media_items = Vec::new();
    let mut next_cursor: Option<String> = None;

    let instructions = raw
        .pointer("/data/user/result/timeline_v2/timeline/instructions")
        .and_then(|v| v.as_array())
        .ok_or("Failed to find instructions")?;

    for instruction in instructions {
        if let Some(module_items) = instruction.get("moduleItems").and_then(|v| v.as_array()) {
            for item in module_items {
                if let Some(media) = extract_media_from_item(item) {
                    media_items.extend(media);
                }
            }
        }

        if let Some(entries) = instruction.get("entries").and_then(|v| v.as_array()) {
            for entry in entries {
                let entry_id = entry.get("entryId").and_then(|v| v.as_str()).unwrap_or("");

                if entry_id.contains("cursor-bottom")
                    && let Some(cursor_value) =
                        entry.pointer("/content/value").and_then(|v| v.as_str())
                {
                    next_cursor = Some(cursor_value.to_string());
                }

                if let Some(items) = entry.pointer("/content/items").and_then(|v| v.as_array()) {
                    for item in items {
                        if let Some(media) = extract_media_from_item(item) {
                            media_items.extend(media);
                        }
                    }
                }
            }
        }
    }

    Ok((media_items, next_cursor))
}

#[instrument(skip_all)]
fn extract_media_from_item(item: &Value) -> Option<Vec<MediaItem>> {
    let mut results = Vec::new();

    let result = item.pointer("/item/itemContent/tweet_results/result")?;

    let legacy = result
        .get("legacy")
        .or_else(|| result.pointer("/tweet/legacy"))?;

    let timestamp = legacy
        .get("created_at")
        .and_then(|v| v.as_str())
        .and_then(|s| {
            // Wed Mar 12 18:47:51 +0000 2025
            //  %a  %b %d %H:%M:%S    %z   %Y
            DateTime::parse_from_str(s, "%a %b %d %H:%M:%S %z %Y").ok()
        })
        .unwrap_or(DateTime::<FixedOffset>::default());

    let media_array = legacy
        .pointer("/extended_entities/media")
        .and_then(|v| v.as_array())?;

    for media in media_array {
        let media_type_str = media
            .get("type")
            .and_then(|v| v.as_str())
            .unwrap_or("photo");

        match media_type_str {
            "photo" => {
                if let Some(url) = media.get("media_url_https").and_then(|v| v.as_str()) {
                    results.push(MediaItem {
                        url: url.to_string(),
                        media_type: MediaType::Image,
                        timestamp,
                    });
                }
            }
            "video" | "animated_gif" => {
                if let Some(variants) = media
                    .pointer("/video_info/variants")
                    .and_then(|v| v.as_array())
                {
                    let best_video = variants
                        .iter()
                        .filter(|v| {
                            v.get("content_type")
                                .and_then(|t| t.as_str())
                                .map(|t| t.contains("mp4"))
                                .unwrap_or(false)
                        })
                        .max_by_key(|v| v.get("bitrate").and_then(|b| b.as_u64()).unwrap_or(0));

                    if let Some(video) = best_video
                        && let Some(url) = video.get("url").and_then(|v| v.as_str())
                    {
                        results.push(MediaItem {
                            url: url.to_string(),
                            media_type: MediaType::Video,
                            timestamp,
                        });
                    }
                }
            }
            _ => {}
        }
    }

    if results.is_empty() {
        None
    } else {
        Some(results)
    }
}
