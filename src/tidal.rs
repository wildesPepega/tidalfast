//! TIDAL API client.

use std::time::Duration;
use anyhow::{anyhow, Context, Result};
use base64::Engine;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::model::{Album, Artist, CreditGroup, Lyrics, Playlist, PlaylistCreator, SearchResults, StreamInfo, Track};

const AUTH_URL: &str = "https://auth.tidal.com/v1/oauth2";
const API_URL: &str = "https://api.tidal.com/v1";
const OPENAPI_URL: &str = "https://openapi.tidal.com/v2";
const API_V2_URL: &str = "https://api.tidal.com/v2";
const CLIENT_VERSION: &str = "2026.9.15";

const PLAYBACKINFO_SUB_STATUS_RANGE: std::ops::RangeInclusive<u64> = 4000..=4999;

/// Device-code client credentials (decoded from SONE's embedded config).
pub const CLIENT_ID: &str = "fX2JxdmntZWK0ixT";
pub const CLIENT_SECRET: &str = "1Nn9AfDAjxrgJFJbKNWLeAyKGVGmINuXPPLHVXAvxAg=";

/// Centre-crop to a square, at most 1280 px, as JPEG.
fn square_jpeg(bytes: &[u8]) -> Result<Vec<u8>> {
    let img = image::load_from_memory(bytes).context("This file is not a readable image")?;
    let side = img.width().min(img.height());
    let mut square = img.crop_imm((img.width() - side) / 2, (img.height() - side) / 2, side, side);
    if side > 1280 {
        square = square.resize(1280, 1280, image::imageops::FilterType::Lanczos3);
    }
    let mut out = std::io::Cursor::new(Vec::new());
    square
        .to_rgb8()
        .write_with_encoder(image::codecs::jpeg::JpegEncoder::new_with_quality(&mut out, 90))
        .context("encoding the cover")?;
    Ok(out.into_inner())
}

fn sub_status(body: &str) -> Option<u64> {
    serde_json::from_str::<Value>(body)
        .ok()
        .and_then(|v| v.get("subStatus").cloned())
        .and_then(|s| match s.as_u64() {
            Some(n) => Some(n),
            None => s
                .as_f64()
                .filter(|f| f.is_finite() && f.fract() == 0.0 && *f >= 0.0)
                .map(|f| f as u64),
        })
}

fn is_playbackinfo_sub_status(body: &str) -> bool {
    sub_status(body).is_some_and(|s| PLAYBACKINFO_SUB_STATUS_RANGE.contains(&s))
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Tokens {
    #[serde(default)]
    pub access_token: String,
    #[serde(default)]
    pub refresh_token: String,
    #[serde(default)]
    pub expires_in: u64,
    #[serde(default)]
    pub token_type: String,
    #[serde(default)]
    pub user_id: Option<u64>,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DeviceAuth {
    pub device_code: String,
    pub user_code: String,
    pub verification_uri: String,
    #[serde(default)]
    pub verification_uri_complete: Option<String>,
    pub expires_in: u64,
    pub interval: u64,
}

pub enum Poll {
    Pending,
    SlowDown,
    Done(Box<Tokens>),
}

/// Qualities tried in this order.
const QUALITIES: [&str; 3] = ["HI_RES_LOSSLESS", "LOSSLESS", "HIGH"];

pub struct TidalClient {
    http: reqwest::Client,
    access_token: String,
    refresh_token: String,
    pub user_id: Option<u64>,
    pub country_code: String,
    /// Whether to try hi-res streams (cleared once they turn out unreadable).
    hires: bool,
}

impl TidalClient {
    pub fn new(country_code: String) -> Result<Self> {
        let http = reqwest::Client::builder()
            .user_agent("tidalfast/0.1")
            .connect_timeout(Duration::from_secs(10))
            .timeout(Duration::from_secs(60))
            .build()
            .context("building HTTP client")?;
        Ok(Self {
            http,
            access_token: String::new(),
            refresh_token: String::new(),
            user_id: None,
            country_code,
            hires: true,
        })
    }

    pub fn http(&self) -> reqwest::Client {
        self.http.clone()
    }

    pub fn set_tokens(&mut self, tokens: &Tokens) {
        self.access_token = tokens.access_token.clone();
        self.refresh_token = tokens.refresh_token.clone();
        if tokens.user_id.is_some() {
            self.user_id = tokens.user_id;
        }
    }

    pub fn has_refresh_token(&self) -> bool {
        !self.refresh_token.is_empty()
    }

    pub fn current_tokens(&self) -> Tokens {
        Tokens {
            access_token: self.access_token.clone(),
            refresh_token: self.refresh_token.clone(),
            expires_in: 0,
            token_type: "Bearer".to_string(),
            user_id: self.user_id,
        }
    }

    // ---- Authentication -------------------------------------------------

    pub async fn device_authorization(&self) -> Result<DeviceAuth> {
        let form = [
            ("client_id", CLIENT_ID),
            ("client_secret", CLIENT_SECRET),
            ("scope", "r_usr w_usr w_sub"),
        ];
        let resp = self
            .http
            .post(format!("{AUTH_URL}/device_authorization"))
            .form(&form)
            .send()
            .await?;
        let status = resp.status();
        let body = resp.text().await.unwrap_or_default();
        if !status.is_success() {
            return Err(anyhow!("device_authorization failed ({status}): {body}"));
        }
        serde_json::from_str(&body).context("parsing device_authorization")
    }

    pub async fn poll_device_token(&mut self, device_code: &str) -> Result<Poll> {
        let form = [
            ("client_id", CLIENT_ID),
            ("client_secret", CLIENT_SECRET),
            ("device_code", device_code),
            ("grant_type", "urn:ietf:params:oauth:grant-type:device_code"),
            ("scope", "r_usr w_usr w_sub"),
        ];
        let resp = self
            .http
            .post(format!("{AUTH_URL}/token"))
            .form(&form)
            .send()
            .await?;
        let status = resp.status();
        let body = resp.text().await.unwrap_or_default();

        if status.as_u16() == 400 {
            if body.contains("authorization_pending") {
                return Ok(Poll::Pending);
            }
            if body.contains("slow_down") {
                return Ok(Poll::SlowDown);
            }
        }
        if !status.is_success() {
            return Err(anyhow!("token poll failed ({status}): {body}"));
        }

        let tokens: Tokens = serde_json::from_str(&body).context("parsing tokens")?;
        self.set_tokens(&tokens);
        Ok(Poll::Done(Box::new(tokens)))
    }

    pub async fn refresh(&mut self) -> Result<Tokens> {
        if self.refresh_token.is_empty() {
            return Err(anyhow!("no refresh token"));
        }
        let form = [
            ("client_id", CLIENT_ID),
            ("client_secret", CLIENT_SECRET),
            ("refresh_token", self.refresh_token.as_str()),
            ("grant_type", "refresh_token"),
            ("scope", "r_usr w_usr w_sub"),
        ];
        let resp = self
            .http
            .post(format!("{AUTH_URL}/token"))
            .form(&form)
            .send()
            .await?;
        let status = resp.status();
        let body = resp.text().await.unwrap_or_default();
        if !status.is_success() {
            return Err(anyhow!("token refresh failed ({status}): {body}"));
        }
        let tokens: Tokens = serde_json::from_str(&body).context("parsing refreshed tokens")?;
        let old_refresh = self.refresh_token.clone();
        let old_user = self.user_id;
        self.access_token = tokens.access_token.clone();
        if !tokens.refresh_token.is_empty() {
            self.refresh_token = tokens.refresh_token.clone();
        }
        let merged = Tokens {
            refresh_token: if tokens.refresh_token.is_empty() {
                old_refresh
            } else {
                tokens.refresh_token
            },
            user_id: tokens.user_id.or(old_user),
            ..tokens
        };
        Ok(merged)
    }

    // ---- Authenticated reads -------------------------------------------

    /// GET JSON with one automatic token refresh on 401.
    async fn get_value(&mut self, url: &str, query: &[(&str, &str)]) -> Result<Value> {
        let mut refreshed = false;
        loop {
            let mut req = self.http.get(url).query(query);
            if url.contains("/v2/") {
                req = req.header("x-tidal-client-version", CLIENT_VERSION);
            }
            if !self.access_token.is_empty() {
                req = req.header("Authorization", format!("Bearer {}", self.access_token));
            }
            let resp = req.send().await?;
            let status = resp.status();
            if status == reqwest::StatusCode::UNAUTHORIZED {
                let body = resp.text().await.unwrap_or_default();
                if is_playbackinfo_sub_status(&body) {
                    return Err(anyhow!("HTTP {status} for {url}: {}", truncate(&body, 200)));
                }
                if !refreshed && self.has_refresh_token() {
                    refreshed = true;
                    self.refresh().await?;
                    continue;
                }
                return Err(anyhow!("HTTP {status} for {url}: {}", truncate(&body, 200)));
            }
            let body = resp.text().await.unwrap_or_default();
            if !status.is_success() {
                return Err(anyhow!("HTTP {status} for {url}: {}", truncate(&body, 200)));
            }
            return serde_json::from_str(&body).context("invalid JSON response");
        }
    }

    /// Authenticated write request (form body) with one token refresh on 401.
    async fn write(
        &mut self,
        method: reqwest::Method,
        url: &str,
        form: &[(&str, &str)],
        etag: Option<&str>,
    ) -> Result<String> {
        let mut refreshed = false;
        loop {
            let mut req = self
                .http
                .request(method.clone(), url)
                .header("Authorization", format!("Bearer {}", self.access_token));
            if !form.is_empty() {
                req = req.form(form);
            }
            if let Some(etag) = etag {
                req = req.header("If-None-Match", etag);
            }
            let resp = req.send().await?;
            let status = resp.status();
            let body = resp.text().await.unwrap_or_default();
            if status == reqwest::StatusCode::UNAUTHORIZED && !refreshed && self.has_refresh_token() {
                refreshed = true;
                self.refresh().await?;
                continue;
            }
            if !status.is_success() {
                return Err(anyhow!("HTTP {status}: {}", truncate(&body, 200)));
            }
            return Ok(body);
        }
    }

    pub async fn set_liked(&mut self, track_id: u64, liked: bool) -> Result<()> {
        let user_id = self.user_id.ok_or_else(|| anyhow!("not signed in"))?;
        let cc = self.country_code.clone();
        if liked {
            self.write(
                reqwest::Method::POST,
                &format!("{API_URL}/users/{user_id}/favorites/tracks"),
                &[("trackIds", &track_id.to_string()), ("countryCode", &cc)],
                None,
            )
            .await?;
        } else {
            self.write(
                reqwest::Method::DELETE,
                &format!("{API_URL}/users/{user_id}/favorites/tracks/{track_id}?countryCode={cc}"),
                &[],
                None,
            )
            .await?;
        }
        Ok(())
    }

    /// The playlist's ETag, which TIDAL requires for every edit.
    async fn playlist_etag(&mut self, uuid: &str) -> Result<String> {
        let cc = self.country_code.clone();
        let mut refreshed = false;
        loop {
            let resp = self
                .http
                .get(format!("{API_URL}/playlists/{uuid}"))
                .query(&[("countryCode", &cc)])
                .header("Authorization", format!("Bearer {}", self.access_token))
                .send()
                .await?;
            if resp.status() == reqwest::StatusCode::UNAUTHORIZED && !refreshed && self.has_refresh_token() {
                refreshed = true;
                self.refresh().await?;
                continue;
            }
            if !resp.status().is_success() {
                return Err(anyhow!("Could not read playlist ({})", resp.status()));
            }
            return resp
                .headers()
                .get("etag")
                .and_then(|v| v.to_str().ok())
                .map(str::to_string)
                .ok_or_else(|| anyhow!("no ETag from TIDAL"));
        }
    }

    /// Change a playlist's title and description.
    pub async fn update_playlist(&mut self, uuid: &str, title: &str, description: &str) -> Result<()> {
        let cc = self.country_code.clone();
        let etag = self.playlist_etag(uuid).await?;
        self.write(
            reqwest::Method::POST,
            &format!("{API_URL}/playlists/{uuid}?countryCode={cc}"),
            &[("title", title), ("description", description)],
            Some(&etag),
        )
        .await?;
        Ok(())
    }

    /// One JSON:API request to TIDAL's OpenAPI (artworks and relationships).
    async fn openapi(&mut self, method: reqwest::Method, path: &str, body: Option<&Value>) -> Result<Value> {
        let mut refreshed = false;
        loop {
            let mut req = self
                .http
                .request(method.clone(), format!("{OPENAPI_URL}{path}"))
                .header("Authorization", format!("Bearer {}", self.access_token))
                .header("x-tidal-client-version", CLIENT_VERSION)
                .query(&[("countryCode", self.country_code.as_str())]);
            if let Some(body) = body {
                req = req.header("Content-Type", "application/vnd.api+json").json(body);
            }
            let resp = req.send().await?;
            let status = resp.status();
            let text = resp.text().await.unwrap_or_default();
            if status == reqwest::StatusCode::UNAUTHORIZED && !refreshed && self.has_refresh_token() {
                refreshed = true;
                self.refresh().await?;
                continue;
            }
            if !status.is_success() {
                return Err(anyhow!("HTTP {status} for {path}: {}", truncate(&text, 300)));
            }
            // 204 answers have no body
            return Ok(serde_json::from_str(&text).unwrap_or(Value::Null));
        }
    }

    /// Upload `image` (any format the decoder knows) as the playlist's cover.
    pub async fn set_playlist_cover(&mut self, uuid: &str, image: &[u8]) -> Result<()> {
        use base64::Engine;
        let jpeg = square_jpeg(image)?;
        let digest = md5::compute(&jpeg);
        let hex = format!("{digest:x}");
        let content_md5 = base64::engine::general_purpose::STANDARD.encode(digest.0);

        // 1. announce the artwork and get an upload link
        let created = self
            .openapi(
                reqwest::Method::POST,
                "/artworks",
                Some(&serde_json::json!({
                    "data": { "type": "artworks", "attributes": {
                        "mediaType": "IMAGE",
                        "sourceFile": { "md5Hash": hex, "size": jpeg.len() }
                    } }
                })),
            )
            .await?;
        let id = created
            .pointer("/data/id")
            .and_then(Value::as_str)
            .ok_or_else(|| anyhow!("TIDAL returned no artwork ID"))?
            .to_string();
        let href = created
            .pointer("/data/attributes/sourceFile/uploadLink/href")
            .and_then(Value::as_str)
            .ok_or_else(|| anyhow!("TIDAL returned no upload link"))?
            .to_string();

        // 2. upload the bytes (no authorization here)
        let put = self
            .http
            .put(&href)
            .header("content-md5", content_md5)
            .header("Content-Type", "image/jpeg")
            .timeout(Duration::from_secs(120))
            .body(jpeg)
            .send()
            .await?;
        if !put.status().is_success() {
            return Err(anyhow!("Image upload failed ({})", put.status()));
        }

        // 3. link it as the playlist's cover. TIDAL often accepts this right
        // away; if it isn't done processing yet, wait for it and try again.
        let link = serde_json::json!({ "data": [{ "type": "artworks", "id": id }] });
        let path = format!("/playlists/{uuid}/relationships/coverArt");
        let mut ready = false;
        for _ in 0..40 {
            let art = self.openapi(reqwest::Method::GET, &format!("/artworks/{id}"), None).await?;
            let status = art
                .pointer("/data/attributes/sourceFile/status/technicalFileStatus")
                .and_then(Value::as_str)
                .unwrap_or("");
            match status {
                "OK" => {
                    ready = true;
                    break;
                }
                "" | "UPLOAD_REQUESTED" | "PROCESSING" => tokio::time::sleep(Duration::from_millis(250)).await,
                other => return Err(anyhow!("TIDAL could not process the image ({other})")),
            }
        }
        if !ready {
            return Err(anyhow!("TIDAL took too long to process the image"));
        }
        self.openapi(reqwest::Method::PATCH, &path, Some(&link)).await?;
        Ok(())
    }

    /// Remove a custom cover: TIDAL goes back to the generated one.
    pub async fn remove_playlist_cover(&mut self, uuid: &str) -> Result<()> {
        self.openapi(
            reqwest::Method::PATCH,
            &format!("/playlists/{uuid}/relationships/coverArt"),
            Some(&serde_json::json!({ "data": [] })),
        )
        .await?;
        Ok(())
    }

    pub async fn delete_playlist(&mut self, uuid: &str) -> Result<()> {
        let cc = self.country_code.clone();
        let etag = self.playlist_etag(uuid).await?;
        self.write(
            reqwest::Method::DELETE,
            &format!("{API_URL}/playlists/{uuid}?countryCode={cc}"),
            &[],
            Some(&etag),
        )
        .await?;
        Ok(())
    }

    /// Create an empty playlist and return its uuid.
    pub async fn create_playlist(&mut self, title: &str) -> Result<String> {
        let user_id = self.user_id.ok_or_else(|| anyhow!("not signed in"))?;
        let cc = self.country_code.clone();
        let body = self
            .write(
                reqwest::Method::POST,
                &format!("{API_URL}/users/{user_id}/playlists?countryCode={cc}"),
                &[("title", title), ("description", "")],
                None,
            )
            .await?;
        serde_json::from_str::<Value>(&body)
            .ok()
            .and_then(|v| v.get("uuid").and_then(Value::as_str).map(str::to_string))
            .ok_or_else(|| anyhow!("TIDAL returned no playlist ID"))
    }

    /// Append many tracks to a playlist, in order.
    pub async fn add_tracks(&mut self, uuid: &str, track_ids: &[u64]) -> Result<()> {
        let cc = self.country_code.clone();
        let mut added = 0usize;
        for chunk in track_ids.chunks(50) {
            let etag = self.playlist_etag(uuid).await?;
            let ids = chunk.iter().map(u64::to_string).collect::<Vec<_>>().join(",");
            self.write(
                reqwest::Method::POST,
                &format!("{API_URL}/playlists/{uuid}/items?countryCode={cc}"),
                &[("trackIds", &ids), ("onDupes", "SKIP"), ("toIndex", &added.to_string())],
                Some(&etag),
            )
            .await?;
            added += chunk.len();
        }
        Ok(())
    }

    /// Append a track to one of the user's playlists. Returns false if it was
    /// already in there.
    pub async fn add_to_playlist(&mut self, uuid: &str, track_id: u64) -> Result<bool> {
        let cc = self.country_code.clone();
        let etag = self.playlist_etag(uuid).await?;
        let body = self
            .write(
                reqwest::Method::POST,
                &format!("{API_URL}/playlists/{uuid}/items?countryCode={cc}"),
                &[
                    ("trackIds", &track_id.to_string()),
                    ("onDupes", "SKIP"),
                    ("toIndex", "0"),
                ],
                Some(&etag),
            )
            .await?;
        let added = serde_json::from_str::<Value>(&body)
            .ok()
            .and_then(|v| v.get("addedItemIds").and_then(Value::as_array).map(|a| !a.is_empty()))
            .unwrap_or(true);
        Ok(added)
    }

    pub async fn session_info(&mut self) -> Result<(u64, String)> {
        let value = self.get_value(&format!("{API_URL}/sessions"), &[]).await?;
        let user_id = value
            .get("userId")
            .and_then(Value::as_u64)
            .ok_or_else(|| anyhow!("no userId in session"))?;
        let country = value
            .get("countryCode")
            .and_then(Value::as_str)
            .unwrap_or(&self.country_code)
            .to_string();
        self.user_id = Some(user_id);
        self.country_code = country.clone();
        Ok((user_id, country))
    }

    /// Display name and (optional) profile picture UUID of a user.
    pub async fn user_profile(&mut self, user_id: u64) -> Result<(String, Option<String>)> {
        let cc = self.country_code.clone();
        let value = self
            .get_value(
                &format!("{API_URL}/users/{user_id}"),
                &[("countryCode", &cc)],
            )
            .await?;
        let name = value
            .get("profileName")
            .and_then(Value::as_str)
            .filter(|s| !s.is_empty())
            .or_else(|| value.get("username").and_then(Value::as_str))
            .or_else(|| value.get("firstName").and_then(Value::as_str))
            .unwrap_or("TIDAL User")
            .to_string();
        let picture = value
            .get("picture")
            .and_then(Value::as_str)
            .filter(|s| !s.is_empty())
            .map(str::to_string);
        Ok((name, picture))
    }

    /// The user's liked tracks, newest first.
    pub async fn favorite_tracks(&mut self, user_id: u64) -> Result<Vec<Track>> {
        let cc = self.country_code.clone();
        let mut tracks = Vec::new();
        let mut offset = 0u32;
        loop {
            let offset_s = offset.to_string();
            let value = self
                .get_value(
                    &format!("{API_URL}/users/{user_id}/favorites/tracks"),
                    &[
                        ("countryCode", &cc),
                        ("limit", "100"),
                        ("offset", &offset_s),
                        ("order", "DATE"),
                        ("orderDirection", "DESC"),
                    ],
                )
                .await?;
            let items = value
                .get("items")
                .and_then(Value::as_array)
                .cloned()
                .unwrap_or_default();
            let total = value
                .get("totalNumberOfItems")
                .and_then(Value::as_u64)
                .unwrap_or(items.len() as u64);
            let fetched = items.len() as u32;
            tracks.extend(items.into_iter().filter_map(parse_playlist_item));
            if fetched == 0 || tracks.len() as u64 >= total {
                break;
            }
            offset += fetched;
        }
        Ok(tracks)
    }

    pub async fn user_playlists(&mut self, user_id: u64) -> Result<Vec<Playlist>> {
        let cc = self.country_code.clone();
        let value = self
            .get_value(
                &format!("{API_URL}/users/{user_id}/playlists"),
                &[("countryCode", &cc), ("limit", "100"), ("offset", "0")],
            )
            .await?;
        let items = value
            .get("items")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        // Only playlists the user created themselves (not followed ones).
        Ok(items
            .into_iter()
            .filter_map(parse_playlist)
            .filter(|p| {
                p.creator
                    .as_ref()
                    .and_then(|c| c.id)
                    .map_or(true, |id| id == user_id || id == 0)
            })
            .collect())
    }

    pub async fn playlist_tracks(&mut self, uuid: &str) -> Result<Vec<Track>> {
        let cc = self.country_code.clone();
        let mut tracks = Vec::new();
        let mut offset = 0u32;
        let page = 100u32;
        loop {
            let offset_s = offset.to_string();
            let limit_s = page.to_string();
            let value = self
                .get_value(
                    &format!("{API_URL}/playlists/{uuid}/items"),
                    &[
                        ("countryCode", &cc),
                        ("limit", &limit_s),
                        ("offset", &offset_s),
                    ],
                )
                .await?;
            let items = value
                .get("items")
                .and_then(Value::as_array)
                .cloned()
                .unwrap_or_default();
            let total = value
                .get("totalNumberOfItems")
                .and_then(Value::as_u64)
                .unwrap_or(items.len() as u64);
            let fetched = items.len() as u32;
            tracks.extend(items.into_iter().filter_map(parse_playlist_item));
            if fetched == 0 || tracks.len() as u64 >= total {
                break;
            }
            offset += fetched;
        }
        Ok(tracks)
    }

    /// Page through a track list endpoint. `wrapped` lists nest each track in `item`.
    async fn paged_tracks(&mut self, url: &str, wrapped: bool, max: usize) -> Result<Vec<Track>> {
        let cc = self.country_code.clone();
        let mut tracks = Vec::new();
        let mut offset = 0u32;
        loop {
            let offset_s = offset.to_string();
            let value = self
                .get_value(
                    url,
                    &[("countryCode", &cc), ("limit", "100"), ("offset", &offset_s)],
                )
                .await?;
            let items = value
                .get("items")
                .and_then(Value::as_array)
                .cloned()
                .unwrap_or_default();
            let total = value
                .get("totalNumberOfItems")
                .and_then(Value::as_u64)
                .unwrap_or(items.len() as u64);
            let fetched = items.len() as u32;
            for entry in items {
                let entry = if wrapped { entry } else { serde_json::json!({ "item": entry }) };
                if let Some(track) = parse_playlist_item(entry) {
                    tracks.push(track);
                }
            }
            if fetched == 0 || tracks.len() as u64 >= total || tracks.len() >= max {
                break;
            }
            offset += fetched;
        }
        Ok(tracks)
    }

    /// Tracks of any collection: playlists, `mix:<id>`, `album:<id>`, `artist:<id>`.
    pub async fn collection_tracks(&mut self, uuid: &str) -> Result<Vec<Track>> {
        if let Some(id) = uuid.strip_prefix("mix:") {
            self.paged_tracks(&format!("{API_URL}/mixes/{id}/items"), true, 500).await
        } else if let Some(id) = uuid.strip_prefix("album:") {
            self.paged_tracks(&format!("{API_URL}/albums/{id}/items"), true, 500).await
        } else if let Some(id) = uuid.strip_prefix("artist:") {
            self.paged_tracks(&format!("{API_URL}/artists/{id}/toptracks"), false, 50).await
        } else {
            self.playlist_tracks(uuid).await
        }
    }

    pub async fn artist_info(&mut self, id: u64) -> Result<Artist> {
        let cc = self.country_code.clone();
        let value = self
            .get_value(&format!("{API_URL}/artists/{id}"), &[("countryCode", &cc)])
            .await?;
        serde_json::from_value(value).context("parsing artist")
    }

    /// An artist's releases; `filter` is `ALBUMS`, `EPSANDSINGLES` or `COMPILATIONS`.
    /// Newest first.
    /// All releases of an artist for one filter, each release once. TIDAL
    /// hands them out in pages of 100 and lists many releases several times.
    pub async fn artist_albums(&mut self, id: u64, filter: &str) -> Result<Vec<Album>> {
        let cc = self.country_code.clone();
        let mut albums: Vec<Album> = Vec::new();
        // (the compilation list of a big artist runs into the thousands: first page only)
        let pages = if filter == "COMPILATIONS" { 1 } else { 6 };
        for page in 0..pages {
            let offset = (page * 100).to_string();
            let value = self
                .get_value(
                    &format!("{API_URL}/artists/{id}/albums"),
                    &[("countryCode", &cc), ("filter", filter), ("limit", "100"), ("offset", &offset)],
                )
                .await?;
            let items = value.get("items").and_then(Value::as_array).cloned().unwrap_or_default();
            let count = items.len();
            albums.extend(items.into_iter().filter_map(|v| serde_json::from_value(v).ok()));
            let total = value.get("totalNumberOfItems").and_then(Value::as_u64).unwrap_or(0) as usize;
            if count < 100 || (page + 1) * 100 >= total {
                break;
            }
        }
        let mut albums = crate::model::dedupe_albums(albums);
        albums.sort_by(|a, b| b.release_date.cmp(&a.release_date));
        Ok(albums)
    }

    /// Lyrics for a track; empty when TIDAL has none.
    pub async fn lyrics(&mut self, id: u64) -> Result<Lyrics> {
        let cc = self.country_code.clone();
        let value = match self
            .get_value(
                &format!("{API_URL}/tracks/{id}/lyrics"),
                &[("countryCode", &cc), ("deviceType", "BROWSER"), ("locale", "en_US")],
            )
            .await
        {
            Ok(v) => v,
            Err(e) if e.to_string().contains("404") => return Ok(Lyrics::default()),
            Err(e) => return Err(e),
        };
        let plain = value.get("lyrics").and_then(Value::as_str).unwrap_or("").to_string();
        let synced = value
            .get("subtitles")
            .and_then(Value::as_str)
            .map(parse_synced_lyrics)
            .unwrap_or_default();
        Ok(Lyrics { plain, synced })
    }

    /// Songwriters, performers, producers and engineers of a track.
    pub async fn credits(&mut self, id: u64) -> Result<Vec<CreditGroup>> {
        let cc = self.country_code.clone();
        let value = match self
            .get_value(&format!("{API_URL}/tracks/{id}/credits"), &[("countryCode", &cc)])
            .await
        {
            Ok(v) => v,
            Err(e) if e.to_string().contains("404") => return Ok(Vec::new()),
            Err(e) => return Err(e),
        };
        Ok(value
            .as_array()
            .map(|groups| {
                groups
                    .iter()
                    .filter_map(|g| {
                        let role = g.get("type")?.as_str()?.to_string();
                        let people: Vec<_> = g
                            .get("contributors")?
                            .as_array()?
                            .iter()
                            .filter_map(|c| {
                                Some((c.get("name")?.as_str()?.to_string(), c.get("id").and_then(Value::as_u64)))
                            })
                            .collect();
                        (!people.is_empty()).then_some(CreditGroup { role, people })
                    })
                    .collect()
            })
            .unwrap_or_default())
    }

    pub async fn track(&mut self, id: u64) -> Result<Track> {
        let cc = self.country_code.clone();
        let value = self
            .get_value(&format!("{API_URL}/tracks/{id}"), &[("countryCode", &cc)])
            .await?;
        parse_playlist_item(serde_json::json!({ "item": value })).ok_or_else(|| anyhow!("Could not read track"))
    }

    pub async fn album(&mut self, id: u64) -> Result<Album> {
        let cc = self.country_code.clone();
        let value = self
            .get_value(&format!("{API_URL}/albums/{id}"), &[("countryCode", &cc)])
            .await?;
        serde_json::from_value(value).context("parsing album")
    }

    pub async fn playlist(&mut self, uuid: &str) -> Result<Playlist> {
        let cc = self.country_code.clone();
        let value = self
            .get_value(&format!("{API_URL}/playlists/{uuid}"), &[("countryCode", &cc)])
            .await?;
        parse_playlist(value).ok_or_else(|| anyhow!("Could not read playlist"))
    }

    /// Similar tracks chosen by TIDAL's algorithm (used for autoplay).
    pub async fn track_radio(&mut self, track_id: u64) -> Result<Vec<Track>> {
        self.paged_tracks(&format!("{API_URL}/tracks/{track_id}/radio"), false, 100).await
    }

    /// Personal mixes (Daily Discovery, New Arrivals, My Mix …) and editorial
    /// playlists, as titled sections.
    pub async fn home_sections(&mut self) -> Result<Vec<(String, Vec<Playlist>)>> {
        let cc = self.country_code.clone();
        let query = [("countryCode", cc.as_str()), ("deviceType", "BROWSER"), ("locale", "en_US")];
        let mut sections: Vec<(String, Vec<Playlist>)> = Vec::new();

        let for_you = self.get_value(&format!("{API_URL}/pages/for_you"), &query).await?;
        let mut mixes = Vec::new();
        for module in page_modules(&for_you) {
            let title = module.get("title").and_then(Value::as_str).unwrap_or("");
            if module.get("type").and_then(Value::as_str) != Some("MIX_LIST")
                || !title.to_lowercase().contains("mix")
            {
                continue;
            }
            for item in module_items(module) {
                if let Some(mix) = parse_mix(item) {
                    if !mixes.iter().any(|m: &Playlist| m.uuid == mix.uuid) {
                        mixes.push(mix);
                    }
                }
            }
        }
        if !mixes.is_empty() {
            sections.push(("Mixes for you".to_string(), mixes));
        }

        if let Ok(home) = self.get_value(&format!("{API_URL}/pages/home"), &query).await {
            for module in page_modules(&home) {
                if module.get("type").and_then(Value::as_str) != Some("PLAYLIST_LIST") {
                    continue;
                }
                let title = module.get("title").and_then(Value::as_str).unwrap_or("Playlists");
                let mut list: Vec<Playlist> = module_items(module)
                    .into_iter()
                    .filter_map(|v| parse_playlist(v.clone()))
                    .collect();
                for pl in &mut list {
                    if pl.creator.is_none() {
                        pl.creator = Some(PlaylistCreator { id: None, name: Some("TIDAL".into()) });
                    }
                }
                if !list.is_empty() {
                    sections.push((title.to_string(), list));
                }
            }
        }
        Ok(sections)
    }

    pub async fn search(&mut self, query: &str) -> Result<SearchResults> {
        let cc = self.country_code.clone();
        let value = self
            .get_value(
                &format!("{API_V2_URL}/search"),
                &[
                    ("query", query),
                    ("countryCode", &cc),
                    ("limit", "30"),
                    ("types", "ARTISTS,ALBUMS,TRACKS,PLAYLISTS"),
                    ("includeContributors", "true"),
                    ("includeUserPlaylists", "true"),
                    ("supportsUserData", "true"),
                    ("locale", "en_US"),
                    ("deviceType", "BROWSER"),
                ],
            )
            .await?;

        let section = |key: &str| -> Vec<Value> {
            value
                .get(key)
                .and_then(|s| s.get("items"))
                .and_then(Value::as_array)
                .cloned()
                .unwrap_or_default()
        };

        let mut tracks: Vec<Track> = section("tracks")
            .into_iter()
            .filter_map(|v| serde_json::from_value(v).ok())
            .collect();
        for t in &mut tracks {
            backfill_artist(t);
        }
        let mut albums: Vec<Album> = section("albums")
            .into_iter()
            .filter_map(|v| serde_json::from_value(v).ok())
            .collect();
        let artists: Vec<Artist> = section("artists")
            .into_iter()
            .filter_map(|v| serde_json::from_value(v).ok())
            .collect();
        let playlists: Vec<Playlist> = section("playlists")
            .into_iter()
            .filter_map(parse_playlist)
            .collect();

        // v2 album entries carry `artists` (plural); our Album only keeps the
        // singular fields it needs, so nothing extra to backfill here.
        for _ in &mut albums {}

        Ok(SearchResults {
            tracks,
            albums,
            artists,
            playlists,
        })
    }

    /// Resolve the stream of a track in the best quality TIDAL offers.
    /// Hi-res streams come as DASH, which the player can't read, so they are
    /// skipped (for good, once seen) in favour of lossless FLAC.
    pub async fn stream_info(&mut self, track_id: u64) -> Result<StreamInfo> {
        let mut last = anyhow!("no stream available");
        for quality in QUALITIES {
            if quality == "HI_RES_LOSSLESS" && !self.hires {
                continue;
            }
            match self.stream_info_at(track_id, quality).await {
                Ok(info) => return Ok(info),
                Err(e) => {
                    if quality == "HI_RES_LOSSLESS" && format!("{e:#}").contains("unsupported stream format") {
                        self.hires = false;
                    }
                    log::debug!("stream in {quality} unavailable: {e:#}");
                    last = e;
                }
            }
        }
        Err(last)
    }

    async fn stream_info_at(&mut self, track_id: u64, quality: &str) -> Result<StreamInfo> {
        let cc = self.country_code.clone();
        let value = self
            .get_value(
                &format!("{API_URL}/tracks/{track_id}/playbackinfopostpaywall"),
                &[
                    ("countryCode", &cc),
                    ("audioquality", quality),
                    ("playbackmode", "STREAM"),
                    ("assetpresentation", "FULL"),
                ],
            )
            .await?;

        let mime = value
            .get("manifestMimeType")
            .and_then(Value::as_str)
            .unwrap_or("");
        let manifest_b64 = value
            .get("manifest")
            .and_then(Value::as_str)
            .ok_or_else(|| anyhow!("no manifest in playback info"))?;
        let raw = base64::engine::general_purpose::STANDARD
            .decode(manifest_b64)
            .context("decoding manifest")?;
        let manifest = String::from_utf8(raw).context("manifest is not UTF-8")?;

        let (url, codec) = if mime.contains("bts") || manifest.trim_start().starts_with('{') {
            let parsed: Value = serde_json::from_str(&manifest).context("parsing BTS manifest")?;
            let url = parsed
                .get("urls")
                .and_then(Value::as_array)
                .and_then(|u| u.first())
                .and_then(Value::as_str)
                .ok_or_else(|| anyhow!("no URL in stream manifest"))?
                .to_string();
            let codec = parsed
                .get("codecs")
                .and_then(Value::as_str)
                .map(|c| c.to_uppercase());
            (url, codec)
        } else {
            return Err(anyhow!(
                "unsupported stream format ({mime})"
            ));
        };

        Ok(StreamInfo {
            url,
            codec,
            audio_quality: value
                .get("audioQuality")
                .and_then(Value::as_str)
                .map(str::to_string),
            bit_depth: value
                .get("bitDepth")
                .and_then(Value::as_u64)
                .map(|v| v as u32),
            sample_rate: value
                .get("sampleRate")
                .and_then(Value::as_u64)
                .map(|v| v as u32),
        })
    }
}

/// Parse LRC-style lines (`[mm:ss.xx] text`).
fn parse_synced_lyrics(text: &str) -> Vec<(f64, String)> {
    text.lines()
        .filter_map(|line| {
            let rest = line.trim().strip_prefix('[')?;
            let (stamp, text) = rest.split_once(']')?;
            let (min, sec) = stamp.split_once(':')?;
            let at = min.parse::<f64>().ok()? * 60.0 + sec.parse::<f64>().ok()?;
            Some((at, text.trim().to_string()))
        })
        .collect()
}

fn page_modules(page: &Value) -> Vec<&Value> {
    page.get("rows")
        .and_then(Value::as_array)
        .map(|rows| {
            rows.iter()
                .filter_map(|r| r.get("modules").and_then(Value::as_array))
                .flatten()
                .collect()
        })
        .unwrap_or_default()
}

fn module_items(module: &Value) -> Vec<&Value> {
    module
        .get("pagedList")
        .and_then(|p| p.get("items"))
        .and_then(Value::as_array)
        .map(|a| a.iter().collect())
        .unwrap_or_default()
}

/// A TIDAL-made mix, presented as a playlist with the id `mix:<id>`.
fn parse_mix(value: &Value) -> Option<Playlist> {
    let id = value.get("id")?.as_str()?;
    let mix_type = value.get("mixType").and_then(Value::as_str).unwrap_or("");
    if mix_type.contains("VIDEO") {
        return None;
    }
    let image = value
        .get("images")
        .and_then(|i| i.get("MEDIUM").or_else(|| i.get("SMALL")))
        .and_then(|i| i.get("url"))
        .and_then(Value::as_str)
        .map(str::to_string);
    Some(Playlist {
        uuid: format!("mix:{id}"),
        title: value.get("title")?.as_str()?.to_string(),
        description: value.get("subTitle").and_then(Value::as_str).map(str::to_string),
        square_image: image,
        creator: Some(PlaylistCreator { id: None, name: Some("TIDAL".into()) }),
        ..Default::default()
    })
}

fn parse_playlist(value: Value) -> Option<Playlist> {
    let mut playlist: Playlist = serde_json::from_value(value.clone()).ok()?;
    if playlist.square_image.is_none() {
        playlist.square_image = value
            .get("squareImage")
            .and_then(Value::as_str)
            .map(str::to_string);
    }
    if playlist.creator.is_none() {
        if let Some(creator) = value.get("creator") {
            playlist.creator = serde_json::from_value::<PlaylistCreator>(creator.clone()).ok();
        }
    }
    Some(playlist)
}

fn parse_playlist_item(entry: Value) -> Option<Track> {
    let mut inner = entry.get("item")?.clone();
    if inner.get("duration").map(Value::is_null).unwrap_or(true) {
        if let Some(obj) = inner.as_object_mut() {
            obj.insert("duration".to_string(), Value::from(0u32));
        }
    }
    let mut track: Track = serde_json::from_value(inner.clone()).ok()?;
    if track.image_id.is_none() {
        track.image_id = inner
            .get("imageId")
            .and_then(Value::as_str)
            .map(str::to_string);
    }
    backfill_artist(&mut track);
    Some(track)
}

fn backfill_artist(track: &mut Track) {
    if track.artist.is_none() {
        if let Some(list) = &track.artists {
            track.artist = list.first().cloned();
        }
    }
}

fn truncate(text: &str, max: usize) -> String {
    text.chars().take(max).collect()
}

#[cfg(test)]
mod live_tests {
    use super::*;

    /// Manual smoke test against the real API: `cargo test live -- --ignored --nocapture`.
    #[test]
    #[ignore]
    fn live_home_radio_collections() {
        let cfg = crate::config::Settings::load();
        let tokens = Tokens {
            access_token: cfg.access_token.clone(),
            refresh_token: cfg.refresh_token.clone(),
            expires_in: 0,
            token_type: "Bearer".into(),
            user_id: cfg.user_id,
        };
        let rt = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
        rt.block_on(async {
            let mut c = TidalClient::new(cfg.country_code.clone()).unwrap();
            c.set_tokens(&tokens);
            let sections = c.home_sections().await.unwrap();
            for (t, l) in &sections {
                println!("{t}: {} -> {:?}", l.len(), l.iter().take(3).map(|p| (&p.uuid, &p.title, p.cover_url())).collect::<Vec<_>>());
            }
            let mix = &sections[0].1[0];
            let t = c.collection_tracks(&mix.uuid).await.unwrap();
            println!("mix {} tracks, first {:?}", t.len(), t.first().map(|t| (&t.title, t.artist_name())));
            let r = c.track_radio(t[0].id).await.unwrap();
            println!("radio {} first {:?}", r.len(), r.first().map(|t| &t.title));
            let a = c.collection_tracks("album:573283").await.unwrap();
            println!("album {}", a.len());
            let ar = c.collection_tracks("artist:17275").await.unwrap();
            println!("artist {}", ar.len());
            let s = c.search("eminem").await.unwrap();
            println!("search a={} ar={} pl={} t={} first album cover {:?} artists {}", s.albums.len(), s.artists.len(), s.playlists.len(), s.tracks.len(), s.albums.first().map(|a| (a.cover_url(160), a.artist_names())), s.artists.first().map(|a| a.picture_url(160).is_some()).unwrap_or(false));
        });
    }
}

#[cfg(test)]
mod live {
    use super::*;

    /// Creates a throw-away playlist, sets description and cover, reads it back
    /// and deletes it: `cargo test live_playlist_edit -- --ignored --nocapture`.
    #[test]
    #[ignore]
    fn live_playlist_edit() {
        let cfg = crate::config::Settings::load();
        let tokens = Tokens {
            access_token: cfg.access_token.clone(),
            refresh_token: cfg.refresh_token.clone(),
            user_id: cfg.user_id,
            ..Default::default()
        };
        let rt = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
        rt.block_on(async {
            let mut c = TidalClient::new(cfg.country_code.clone()).unwrap();
            c.set_tokens(&tokens);
            let uuid = c.create_playlist("tidalfast api test (deleted right away)").await.unwrap();
            println!("created {uuid}");
            let result = async {
                let t = std::time::Instant::now();
                c.update_playlist(&uuid, "tidalfast api test (deleted right away)", "a test description").await?;
                println!("update_playlist {:?}", t.elapsed());
                let p = c.playlist(&uuid).await?;
                println!("description now: {:?}", p.description);
                let img = std::fs::read("assets/icon.png")?;
                let t = std::time::Instant::now();
                c.set_playlist_cover(&uuid, &img).await?;
                println!("set_playlist_cover {:?}", t.elapsed());
                let cc = c.country_code.clone();
                let v1 = c.get_value(&format!("{API_URL}/playlists/{uuid}"), &[("countryCode", &cc)]).await?;
                let sq = v1.get("squareImage").and_then(Value::as_str).unwrap_or("").replace('-', "/");
                let r = c.http.get(format!("https://resources.tidal.com/images/{sq}/320x320.jpg")).send().await?;
                println!("CDN {} {:?}", r.status(), r.headers().get("content-length"));
                println!("with cover: square {}", sq);
                c.remove_playlist_cover(&uuid).await?;
                let after = c.playlist(&uuid).await?;
                println!("after removal: square {:?} image {:?}", after.square_image, after.image);
                let v2 = c.openapi(reqwest::Method::GET, &format!("/playlists/{uuid}?include=coverArt"), None).await?;
                println!("V2: {}", serde_json::to_string(&v2)?);
                anyhow::Ok(())
            }
            .await;
            c.delete_playlist(&uuid).await.unwrap();
            println!("deleted {uuid}");
            result.unwrap();
        });
    }

    /// Which artist pages show the same release more than once?
    /// `cargo test live_artist_dupes -- --ignored --nocapture`.
    #[test]
    #[ignore]
    fn live_artist_dupes() {
        let cfg = crate::config::Settings::load();
        let tokens = Tokens {
            access_token: cfg.access_token.clone(),
            refresh_token: cfg.refresh_token.clone(),
            user_id: cfg.user_id,
            ..Default::default()
        };
        let rt = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
        rt.block_on(async {
            let mut c = TidalClient::new(cfg.country_code.clone()).unwrap();
            c.set_tokens(&tokens);
            for name in ["Linkin Park", "Eminem", "Taylor Swift"] {
                let found = c.search(name).await.unwrap();
                let artist = &found.artists[0];
                println!("== {} ({})", artist.name, artist.id);
                for filter in ["ALBUMS", "EPSANDSINGLES", "COMPILATIONS"] {
                    let cc = c.country_code.clone();
                    let v = c
                        .get_value(
                            &format!("{API_URL}/artists/{}/albums", artist.id),
                            &[("countryCode", &cc), ("filter", filter), ("limit", "100")],
                        )
                        .await
                        .unwrap();
                    let items = v.get("items").and_then(Value::as_array).cloned().unwrap_or_default();
                    println!("-- {filter}: {} items (totalNumberOfItems {:?})", items.len(), v.get("totalNumberOfItems"));
                    let mut seen: std::collections::HashMap<String, Vec<String>> = Default::default();
                    for a in &items {
                        let title = a.get("title").and_then(Value::as_str).unwrap_or("").to_lowercase();
                        seen.entry(title).or_default().push(format!(
                            "id={} type={} tracks={} explicit={} q={} date={}",
                            a.get("id").unwrap_or(&Value::Null),
                            a.get("type").unwrap_or(&Value::Null),
                            a.get("numberOfTracks").unwrap_or(&Value::Null),
                            a.get("explicit").unwrap_or(&Value::Null),
                            a.get("audioQuality").unwrap_or(&Value::Null),
                            a.get("releaseDate").unwrap_or(&Value::Null),
                        ));
                    }
                    for (title, list) in seen.iter().filter(|(_, l)| l.len() > 1).take(2) {
                        println!("   DUP {title}: {} entries", list.len());
                    }
                    let deduped = c.artist_albums(artist.id, filter).await.unwrap();
                    let mut titles: Vec<(String, String)> = deduped
                        .iter()
                        .map(|a| (a.title.to_lowercase().split_whitespace().collect::<Vec<_>>().join(" "), a.kind.clone().unwrap_or_default()))
                        .collect();
                    let before = titles.len();
                    titles.sort();
                    titles.dedup();
                    println!("   after dedupe: {} items for {} raw, {} distinct titles", before, items.len(), titles.len());
                    assert_eq!(before, titles.len());
                }
            }
        });
    }
}
