use chrono::prelude::*;
use serde::Deserialize;
use serde_json as json;

#[derive(Deserialize, Debug, Clone)]
pub struct Response {
    pub data: Data,
}

#[derive(Deserialize, Debug, Clone)]
pub struct Data {
    pub layout: Layout,
}

#[derive(Deserialize, Debug, Clone)]
pub struct Layout {
    #[serde(rename = "hashId")]
    pub hash_id: String,
    #[serde(default)]
    pub parent: Option<Box<Layout>>,

    #[serde(default)]
    pub privacy: bool,
    #[serde(default)]
    pub geometry: Option<String>,
    #[serde(default)]
    pub tags: Vec<String>,
    #[serde(default)]
    pub title: Option<String>,
    pub user: Option<User>,
    #[serde(rename = "isDefault", default)]
    pub is_default: bool,
    #[serde(default)]
    pub revision: Option<Revision>,
    #[serde(rename = "lastRevisionCompiled", default)]
    pub last_revision_compiled: bool,
    #[serde(rename = "isLatestRevision", default)]
    pub is_latest_revision: bool,
}

#[derive(Deserialize, Debug, Clone)]
pub struct User {
    #[serde(rename = "hashId")]
    pub hash_id: String,

    #[serde(default)]
    pub annotation: bool,
    #[serde(rename = "annotationPublic", default)]
    pub annotation_public: bool,
    pub name: String,
    #[serde(rename = "pictureUrl", default)]
    pub picture_url: Option<String>,
}

#[derive(Deserialize, Debug, Clone)]
pub struct Revision {
    #[serde(rename = "hashId")]
    pub hash_id: String,
    pub md5: String,
    #[serde(rename = "altMd5")]
    pub alt_md5: String,

    pub alternates: Option<String>,
    #[serde(rename = "createdAt")]
    pub created_at: chrono::DateTime<Utc>,
    #[serde(default)]
    pub navigators: Option<Vec<json::Value>>,
    pub model: String,
    pub title: Option<String>,

    #[serde(rename = "qmkVersion")]
    pub qmk_version: String,
    #[serde(rename = "qmkUptodate")]
    pub qmk_uptodate: bool,

    #[serde(rename = "hasDeletedLayers")]
    pub has_deleted_layers: bool,

    pub combos: Option<Vec<json::Value>>,
    pub tour: json::Value,

    #[serde(rename = "mcuAlternateRevisionHash", default)]
    pub mcu_alternate_revision_hash: Option<String>,
    #[serde(rename = "mcuAlternateLayoutHash", default)]
    pub mcu_alternate_layout_hash: Option<String>,

    pub config: Config,
    pub swatch: Swatch,
    pub layers: Vec<Layer>,
}

#[derive(Deserialize, Debug, Clone)]
pub struct Swatch {
    pub colors: Vec<String>,
}

#[derive(Deserialize, Debug, Clone)]
pub struct Config {
    pub uk: bool,
    #[serde(rename = "audioClick")]
    pub audio_click: bool,
    #[serde(rename = "rgbBriStep")]
    pub rgb_bri_step: i32,
    #[serde(rename = "audioDisable")]
    pub audio_disable: bool,
    #[serde(rename = "capsLockStatus")]
    pub capslock_status: bool,
    #[serde(rename = "enableNavigator")]
    pub enable_navigator: bool,
    #[serde(rename = "autoshiftTimeout")]
    pub autoshift_timeout: i32,
    #[serde(rename = "disabledAnimations")]
    pub disabled_animations: Vec<String>,
    #[serde(rename = "enableDynamicMacros")]
    pub enable_dynamic_macros: bool,
}

#[derive(Deserialize, Debug, Clone)]
pub struct Layer {
    #[serde(rename = "hashId")]
    pub hash_id: String,
    #[serde(rename = "prevHashId")]
    pub prev_hash_id: Option<String>,

    pub automouse: bool,
    #[serde(rename = "builtIn")]
    pub builtin: json::Value,
    pub position: i32,
    pub title: Option<String>,
    pub color: Option<String>,
    pub keys: Vec<Key>,
}

#[derive(Deserialize, Debug, Clone)]
pub struct Key {
    #[serde(default)]
    pub about: Option<String>,
    #[serde(rename = "glowColor", default)]
    pub glow_color: Option<String>,
    #[serde(rename = "lockGlowColor", default)]
    pub lock_glow_color: json::Value,
    #[serde(rename = "customLabel", default)]
    pub custom_label: Option<String>,
    #[serde(rename = "aboutPosition", default)]
    pub about_position: Option<String>,
    #[serde(default)]
    pub emoji: Option<String>,

    #[serde(rename = "tappingTerm", default)]
    pub tapping_term: json::Value,
    #[serde(default)]
    pub tap: Option<Mode>,
    #[serde(default)]
    pub hold: Option<Mode>,
    #[serde(rename = "tapHold", default)]
    pub tap_hold: Option<Mode>,
    #[serde(rename = "doubleTap", default)]
    pub double_tap: Option<Mode>,
}

#[derive(Deserialize, Debug, Clone)]
pub struct Mode {
    pub code: String,
    #[serde(default)]
    pub color: Option<String>,
    #[serde(default)]
    pub layer: Option<i32>,
    #[serde(default)]
    pub r#macro: Option<String>,
    #[serde(default)]
    pub modifier: Option<json::Value>,
    #[serde(default)]
    pub modifiers: Option<Vec<json::Value>>,
    #[serde(default)]
    pub description: Option<String>,
}

const GRAPHQL_QUERY: &str = "query getLayout($hashId: String!, $revisionId: String!, $geometry: String) {\n  layout(hashId: $hashId, geometry: $geometry, revisionId: $revisionId) {\n    ...LayoutData\n    __typename\n  }\n}\n\nfragment LayoutData on Layout {\n  privacy\n  geometry\n  hashId\n  parent {\n    hashId\n    __typename\n  }\n  tags {\n    id\n    hashId\n    name\n    __typename\n  }\n  title\n  user {\n    annotation\n    annotationPublic\n    name\n    hashId\n    pictureUrl\n    __typename\n  }\n  isDefault\n  revision {\n    ...RevisionData\n    __typename\n  }\n  lastRevisionCompiled\n  isLatestRevision\n  __typename\n}\n\nfragment RevisionData on Revision {\n  alternates {\n    hashId\n    model\n    __typename\n  }\n  createdAt\n  hashId\n  navigators\n  model\n  title\n  config\n  swatch\n  qmkVersion\n  qmkUptodate\n  hasDeletedLayers\n  md5\n  altMd5\n  combos {\n    keyIndices\n    layerIdx\n    name\n    trigger\n    __typename\n  }\n  tour {\n    ...TourData\n    __typename\n  }\n  layers {\n    automouse\n    builtIn\n    hashId\n    keys\n    position\n    title\n    color\n    prevHashId\n    __typename\n  }\n  mcuAlternateRevisionHash\n  mcuAlternateLayoutHash\n  __typename\n}\n\nfragment TourData on Tour {\n  hashId\n  url\n  steps: tourSteps {\n    hashId\n    intro\n    outro\n    position\n    content\n    keyIndex\n    comboIndex\n    layer {\n      hashId\n      position\n      __typename\n    }\n    __typename\n  }\n  __typename\n}";

pub async fn fetch(
    hash_id: &str,
    revision_id: &str,
    geometry: &str,
) -> Result<Response, reqwest::Error> {
    let client = reqwest::Client::new();
    let body = json::json!({
        "operationName": "getLayout",
        "variables": {
            "hashId": hash_id,
            "geometry": geometry,
            "revisionId": revision_id,
        },
        "query": GRAPHQL_QUERY,
    });

    client
        .post("https://oryx.zsa.io/graphql")
        .header("content-type", "application/json")
        .header("Origin", "https://configure.zsa.io")
        .header("Referer", "https://configure.zsa.io/")
        .json(&body)
        .send()
        .await?
        .json::<Response>()
        .await
}
