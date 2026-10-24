use serde::Serialize;

#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct PhotoInfo {
    pub path: String,
    pub file_name: String,
    pub timestamp_ms: i64,
    pub thumb_base64: String,
    pub rating: i8,
    pub pick: bool,
    /// Its saved picture is rendered with its current edit (`diskcache::preview_current`).
    pub rendered: bool,
    /// It has an edit (RapidOne's, or a Lightroom one RapidOne reads): the "edited" icon.
    pub edited: bool,
    /// Where its look came from (`{from, name}`: a style, a preset, its XMP), for the thumbnail's
    /// sparkle or palette (`sidecar::look`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub look: Option<serde_json::Value>,
    /// Its `.rpd` has Noise on: the thumbnail's denoise icon (`sidecar::denoised`).
    pub denoised: bool,
}

#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct ScanProgress {
    pub total: usize,
    pub done: usize,
}

#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct FullPreview {
    pub path: String,
    pub image_base64: String,
    pub width: u32,
    pub height: u32,
}
