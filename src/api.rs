use crate::Result;
use serde::{Deserialize, Serialize};
use std::io::Read;

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
    fn download_file(&self, file_id: &str, offset: u64, size: u32) -> Result<Vec<u8>>;
    fn upload_file(
        &self,
        parent_id: Option<&str>,
        name: &str,
        data: Vec<u8>,
        overwrite: bool,
    ) -> Result<RemoteEntry>;
    fn delete_file(&self, file_id: &str) -> Result<()>;
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

    fn download_file(&self, file_id: &str, offset: u64, size: u32) -> Result<Vec<u8>> {
        if size == 0 {
            return Ok(Vec::new());
        }
        let download: DownloadResponse =
            self.get_json(&format!("{DRIVE_URL}/files/{file_id}/download"))?;
        let mut response = self
            .client
            .get(download.download_url)
            .header(
                reqwest::header::RANGE,
                format!("bytes={offset}-{}", offset + size as u64 - 1),
            )
            .send()?;
        #[cfg(debug_assertions)]
        tracing::debug!(
            %file_id,
            offset,
            size,
            status = %response.status(),
            content_type = ?response.headers().get(reqwest::header::CONTENT_TYPE),
            content_length = ?response.headers().get(reqwest::header::CONTENT_LENGTH),
            content_range = ?response.headers().get(reqwest::header::CONTENT_RANGE),
            content_encoding = ?response.headers().get(reqwest::header::CONTENT_ENCODING),
            transfer_encoding = ?response.headers().get(reqwest::header::TRANSFER_ENCODING),
            "MYBOX download response"
        );
        if !response.status().is_success() {
            return Err(format!("MYBOX file download failed: {}", response.status()).into());
        }
        if response.status() == reqwest::StatusCode::PARTIAL_CONTENT {
            let content_range = response
                .headers()
                .get(reqwest::header::CONTENT_RANGE)
                .ok_or("MYBOX partial download missing Content-Range")?
                .to_str()?;
            let start = content_range
                .strip_prefix("bytes ")
                .and_then(|range| range.split_once('-'))
                .and_then(|(start, _)| start.parse::<u64>().ok());
            if start != Some(offset) {
                return Err("MYBOX partial download has unexpected start offset".into());
            }
        } else if response.status() == reqwest::StatusCode::OK && offset > 0 {
            let skipped = std::io::copy(&mut response.by_ref().take(offset), &mut std::io::sink())?;
            if skipped < offset {
                return Ok(Vec::new());
            }
        } else if response.status() != reqwest::StatusCode::OK {
            return Err(format!(
                "MYBOX file download returned unexpected status: {}",
                response.status()
            )
            .into());
        }
        let mut bytes = Vec::with_capacity(size as usize);
        response
            .take(size as u64)
            .read_to_end(&mut bytes)
            .inspect_err(|_error| {
                #[cfg(debug_assertions)]
                {
                    let error = _error;
                    let mut source = std::error::Error::source(error);
                    while let Some(cause) = source {
                        tracing::debug!(%file_id, %cause, "MYBOX download body error cause");
                        source = cause.source();
                    }
                }
            })?;
        Ok(bytes)
    }

    fn upload_file(
        &self,
        parent_id: Option<&str>,
        name: &str,
        data: Vec<u8>,
        overwrite: bool,
    ) -> Result<RemoteEntry> {
        let request = UploadRequest {
            file_name: name,
            file_size: data.len() as u64,
            parent_id,
            is_overwrite: overwrite,
        };
        let response = self
            .client
            .post(format!("{DRIVE_URL}/files"))
            .bearer_auth(&self.access_token)
            .json(&request)
            .send()?;
        if !response.status().is_success() {
            return Err(format!("MYBOX upload URL request failed: {}", response.status()).into());
        }
        let upload: UploadResponse = response.json()?;
        let part = reqwest::blocking::multipart::Part::bytes(data).file_name(name.to_owned());
        let form = reqwest::blocking::multipart::Form::new().part("Filedata", part);
        let response = self
            .client
            .post(upload.upload_url)
            .multipart(form)
            .send()
            .map_err(reqwest::Error::without_url)?;
        if !response.status().is_success() {
            return Err(format!("MYBOX file upload failed: {}", response.status()).into());
        }
        let entries = match parent_id {
            Some(parent_id) => self.list_children(parent_id)?,
            None => self.list_root()?,
        };
        entries
            .into_iter()
            .find(|entry| entry.name == name && !entry.is_directory())
            .ok_or_else(|| "uploaded file not found in parent directory".into())
    }

    fn delete_file(&self, file_id: &str) -> Result<()> {
        let response = self
            .client
            .delete(format!("{DRIVE_URL}/resources/{file_id}"))
            .bearer_auth(&self.access_token)
            .send()?;
        if !response.status().is_success() {
            return Err(format!("MYBOX file deletion failed: {}", response.status()).into());
        }
        Ok(())
    }
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct UploadRequest<'a> {
    file_name: &'a str,
    file_size: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    parent_id: Option<&'a str>,
    is_overwrite: bool,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct UploadResponse {
    upload_url: String,
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
