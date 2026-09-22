use crate::Result;
use serde::Deserialize;

const STORAGE_URL: &str = "https://open-api.mybox.naver.com/v1/drive/storage";
const DRIVE_URL: &str = "https://open-api.mybox.naver.com/v1/drive";

#[derive(Clone, Debug, Deserialize, PartialEq, Eq)]
pub struct RemoteEntry {
    #[serde(rename = "resourceId")]
    pub resource_id: String,
    pub name: String,
    pub size: u64,
    #[serde(rename = "type")]
    pub kind: String,
}

impl RemoteEntry {
    pub fn is_directory(&self) -> bool {
        self.kind == "folder"
    }
}

pub trait RemoteDrive: Send + Sync + 'static {
    fn list_root(&self) -> Result<Vec<RemoteEntry>>;
    fn list_children(&self, folder_id: &str) -> Result<Vec<RemoteEntry>>;
    fn download_file(&self, file_id: &str) -> Result<Vec<u8>>;
}

pub struct MyboxApiClient {
    client: reqwest::blocking::Client,
    access_token: String,
}

impl MyboxApiClient {
    pub fn new(access_token: impl Into<String>) -> Self {
        Self {
            client: reqwest::blocking::Client::new(),
            access_token: access_token.into(),
        }
    }

    pub fn health_check(&self) -> Result<()> {
        let response = self
            .client
            .get(STORAGE_URL)
            .bearer_auth(&self.access_token)
            .send()?;

        if !response.status().is_success() {
            return Err(format!("MYBOX token validation failed: {}", response.status()).into());
        }

        Ok(())
    }

    fn get_json<T: serde::de::DeserializeOwned>(&self, url: &str) -> Result<T> {
        let response = self
            .client
            .get(url)
            .bearer_auth(&self.access_token)
            .send()?;
        if !response.status().is_success() {
            return Err(format!("MYBOX API request failed: {}", response.status()).into());
        }
        Ok(response.json()?)
    }

    fn list_resources(&self, url: reqwest::Url) -> Result<Vec<RemoteEntry>> {
        let mut resources = Vec::new();
        let mut cursor: Option<String> = None;

        loop {
            let mut page_url = url.clone();
            {
                let mut query = page_url.query_pairs_mut();
                query.append_pair("count", "1000");
                if let Some(cursor) = &cursor {
                    query.append_pair("cursor", cursor);
                }
            }

            let page: ResourceList = self.get_json(page_url.as_str())?;
            resources.extend(page.resources);
            cursor = page.response_meta_data.next_cursor;
            if cursor.is_none() {
                break;
            }
        }

        Ok(resources)
    }
}

impl RemoteDrive for MyboxApiClient {
    fn list_root(&self) -> Result<Vec<RemoteEntry>> {
        self.list_resources(reqwest::Url::parse(&format!("{DRIVE_URL}/resources"))?)
    }

    fn list_children(&self, folder_id: &str) -> Result<Vec<RemoteEntry>> {
        self.list_resources(reqwest::Url::parse(&format!(
            "{DRIVE_URL}/folders/{folder_id}/resources"
        ))?)
    }

    fn download_file(&self, file_id: &str) -> Result<Vec<u8>> {
        let download: DownloadResponse =
            self.get_json(&format!("{DRIVE_URL}/files/{file_id}/download"))?;
        let response = self.client.get(download.download_url).send()?;
        if !response.status().is_success() {
            return Err(format!("MYBOX file download failed: {}", response.status()).into());
        }
        Ok(response.bytes()?.to_vec())
    }
}

#[derive(Deserialize)]
struct ResourceList {
    resources: Vec<RemoteEntry>,
    #[serde(rename = "responseMetaData")]
    response_meta_data: ResponseMetadata,
}

#[derive(Deserialize)]
struct ResponseMetadata {
    #[serde(rename = "nextCursor")]
    next_cursor: Option<String>,
}

#[derive(Deserialize)]
struct DownloadResponse {
    #[serde(rename = "downloadUrl")]
    download_url: String,
}
