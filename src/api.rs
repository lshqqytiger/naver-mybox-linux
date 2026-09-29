use crate::Result;
use serde::{Deserialize, Serialize};
use std::{error::Error, fmt};
use std::{
    fs::File,
    io::{Read, Seek, SeekFrom},
    sync::atomic::{AtomicU64, Ordering},
    thread,
    time::{Duration, Instant},
};

const STORAGE_URL: &str = "https://open-api.mybox.naver.com/v1/drive/storage";
const DRIVE_URL: &str = "https://open-api.mybox.naver.com/v1/drive";

#[derive(Debug)]
pub struct ApiStatus {
    pub status: reqwest::StatusCode,
    operation: &'static str,
}

impl fmt::Display for ApiStatus {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "MYBOX {} failed: {}",
            self.operation, self.status
        )
    }
}

impl Error for ApiStatus {}

pub(crate) fn status_error(
    operation: &'static str,
    status: reqwest::StatusCode,
) -> crate::Result<()> {
    if status.is_success() {
        Ok(())
    } else {
        Err(ApiStatus { status, operation }.into())
    }
}

#[derive(Clone, Debug, Deserialize, PartialEq, Eq)]
pub struct RemoteEntry {
    #[serde(rename = "resourceId")]
    pub resource_id: String,
    pub name: String,
    pub size: u64,
    #[serde(rename = "type")]
    pub kind: String,
    #[serde(rename = "modifiedAt", default)]
    pub modified_at: Option<String>,
}

impl RemoteEntry {
    pub fn is_directory(&self) -> bool {
        self.kind == "folder"
    }
}

pub trait RemoteDrive: Send + Sync + 'static {
    fn list_root(&self) -> Result<Vec<RemoteEntry>>;
    fn list_children(&self, folder_id: &str) -> Result<Vec<RemoteEntry>>;
    fn file_metadata(&self, file_id: &str) -> Result<RemoteEntry>;
    fn create_folder(&self, parent_id: Option<&str>, name: &str) -> Result<RemoteEntry>;
    fn download_file(&self, file_id: &str, offset: u64, size: u32) -> Result<Vec<u8>>;
    fn upload_file(
        &self,
        parent_id: Option<&str>,
        name: &str,
        data: &mut File,
        size: u64,
        overwrite: bool,
    ) -> Result<RemoteEntry>;
    fn delete_file(&self, file_id: &str) -> Result<()>;
    fn move_resource(&self, resource_id: &str, parent_id: Option<&str>) -> Result<()>;
    fn rename_resource(&self, resource_id: &str, name: &str) -> Result<()>;
}

pub struct MyboxApiClient {
    client: reqwest::blocking::Client,
    access_token: String,
    next_request_id: AtomicU64,
}

impl MyboxApiClient {
    pub fn new(access_token: impl Into<String>) -> Self {
        Self {
            client: reqwest::blocking::Client::builder()
                .connect_timeout(Duration::from_secs(10))
                .timeout(Duration::from_secs(300))
                .redirect(reqwest::redirect::Policy::custom(|attempt| {
                    if attempt.url().scheme() == "https"
                        && attempt.url().username().is_empty()
                        && attempt.url().password().is_none()
                        && attempt.previous().len() < 5
                    {
                        attempt.follow()
                    } else {
                        attempt.stop()
                    }
                }))
                .build()
                .expect("valid MYBOX HTTP client configuration"),
            access_token: access_token.into(),
            next_request_id: AtomicU64::new(1),
        }
    }

    pub fn health_check(&self) -> Result<()> {
        let response = self.get_response("health", || {
            self.client.get(STORAGE_URL).bearer_auth(&self.access_token)
        })?;

        status_error("token validation", response.status())?;

        Ok(())
    }

    fn get_json<T: serde::de::DeserializeOwned>(&self, url: &str) -> Result<T> {
        let response = self.get_response("metadata", || {
            self.client.get(url).bearer_auth(&self.access_token)
        })?;
        status_error("API request", response.status())?;
        Ok(response.json()?)
    }

    fn get_response(
        &self,
        operation: &'static str,
        request: impl Fn() -> reqwest::blocking::RequestBuilder,
    ) -> Result<reqwest::blocking::Response> {
        let request_id = self.next_request_id.fetch_add(1, Ordering::Relaxed);
        for attempt in 0..3 {
            let started = Instant::now();
            match request().send() {
                Ok(response) if retry_status(response.status()) && attempt < 2 => {
                    tracing::debug!(request_id, operation, attempt, elapsed_ms = started.elapsed().as_millis(), status = %response.status(), "MYBOX read request retry");
                }
                Ok(response) => {
                    tracing::debug!(request_id, operation, attempt, elapsed_ms = started.elapsed().as_millis(), status = %response.status(), "MYBOX read response");
                    return Ok(response);
                }
                Err(error) if error.is_timeout() && attempt < 2 => {
                    tracing::debug!(
                        request_id,
                        operation,
                        attempt,
                        elapsed_ms = started.elapsed().as_millis(),
                        "MYBOX read request timed out; retrying"
                    );
                }
                Err(error) => {
                    tracing::debug!(
                        request_id,
                        operation,
                        attempt,
                        elapsed_ms = started.elapsed().as_millis(),
                        "MYBOX read request failed"
                    );
                    return Err(reqwest::Error::without_url(error).into());
                }
            }
            thread::sleep(Duration::from_millis(100 * (1 << attempt)));
        }
        unreachable!()
    }

    fn send_mutation(
        &self,
        operation: &'static str,
        request: reqwest::blocking::RequestBuilder,
    ) -> Result<reqwest::blocking::Response> {
        let request_id = self.next_request_id.fetch_add(1, Ordering::Relaxed);
        let started = Instant::now();
        let response = request.send().map_err(reqwest::Error::without_url);
        tracing::debug!(request_id, operation, elapsed_ms = started.elapsed().as_millis(), status = ?response.as_ref().map(|response| response.status()), "MYBOX mutation response");
        Ok(response?)
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

    fn download_range(
        &self,
        url: &str,
        offset: u64,
        size: u32,
    ) -> Result<reqwest::blocking::Response> {
        let url = transfer_url(url)?;
        self.get_response("download", || {
            self.client.get(url.clone()).header(
                reqwest::header::RANGE,
                format!("bytes={offset}-{}", offset + size as u64 - 1),
            )
        })
    }
}

impl RemoteDrive for MyboxApiClient {
    fn list_root(&self) -> Result<Vec<RemoteEntry>> {
        self.list_resources(reqwest::Url::parse(&format!("{DRIVE_URL}/resources"))?)
    }

    fn list_children(&self, folder_id: &str) -> Result<Vec<RemoteEntry>> {
        self.list_resources(resource_url(&["folders", folder_id, "resources"])?)
    }

    fn file_metadata(&self, file_id: &str) -> Result<RemoteEntry> {
        self.get_json(resource_url(&["resources", file_id])?.as_str())
    }

    fn create_folder(&self, parent_id: Option<&str>, name: &str) -> Result<RemoteEntry> {
        let response = self.send_mutation(
            "create folder",
            self.client
                .post(format!("{DRIVE_URL}/folders"))
                .bearer_auth(&self.access_token)
                .json(&FolderRequest {
                    folder_name: name,
                    parent_id,
                }),
        )?;
        status_error("folder creation", response.status())?;
        let folder: CreatedFolder = response.json()?;
        Ok(RemoteEntry {
            resource_id: folder.resource_id,
            name: folder.name,
            size: 0,
            kind: "folder".into(),
            modified_at: None,
        })
    }

    fn download_file(&self, file_id: &str, offset: u64, size: u32) -> Result<Vec<u8>> {
        if size == 0 {
            return Ok(Vec::new());
        }
        let download: DownloadResponse =
            self.get_json(resource_url(&["files", file_id, "download"])?.as_str())?;
        let mut response = self.download_range(&download.download_url, offset, size)?;
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
        status_error("file download", response.status())?;
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
            let skipped = std::io::copy(&mut response.by_ref().take(offset), &mut std::io::sink())
                .map_err(|error| format!("MYBOX download seek failed ({:?})", error.kind()))?;
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
            .map_err(|error| format!("MYBOX download body read failed ({:?})", error.kind()))?;
        Ok(bytes)
    }

    fn upload_file(
        &self,
        parent_id: Option<&str>,
        name: &str,
        data: &mut File,
        size: u64,
        overwrite: bool,
    ) -> Result<RemoteEntry> {
        let request = UploadRequest {
            file_name: name,
            file_size: size,
            parent_id,
            is_overwrite: overwrite,
        };
        let response = self.send_mutation(
            "request upload",
            self.client
                .post(format!("{DRIVE_URL}/files"))
                .bearer_auth(&self.access_token)
                .json(&request),
        )?;
        status_error("upload URL request", response.status())?;
        let upload: UploadResponse = response.json()?;
        let upload_url = transfer_url(&upload.upload_url)?;
        data.seek(SeekFrom::Start(0))?;
        let part = reqwest::blocking::multipart::Part::reader_with_length(data.try_clone()?, size)
            .file_name(name.to_owned());
        let form = reqwest::blocking::multipart::Form::new().part("Filedata", part);
        let response =
            self.send_mutation("upload", self.client.post(upload_url).multipart(form))?;
        status_error("file upload", response.status())?;
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
        let response = self.send_mutation(
            "delete",
            self.client
                .delete(resource_url(&["resources", file_id])?)
                .bearer_auth(&self.access_token),
        )?;
        status_error("file deletion", response.status())?;
        Ok(())
    }

    fn move_resource(&self, resource_id: &str, parent_id: Option<&str>) -> Result<()> {
        let root_id;
        let parent_id = match parent_id {
            Some(parent_id) => parent_id,
            None => {
                let roots: RootParents =
                    self.get_json(&format!("{DRIVE_URL}/resources?count=1"))?;
                root_id = roots
                    .resources
                    .into_iter()
                    .next()
                    .ok_or("cannot determine root folder ID from an empty root listing")?
                    .parent_id;
                &root_id
            }
        };
        let response = self.send_mutation(
            "move",
            self.client
                .post(resource_url(&["resources", resource_id, "move"])?)
                .bearer_auth(&self.access_token)
                .json(&MoveRequest {
                    parent_id,
                    is_overwrite: false,
                }),
        )?;
        status_error("resource move", response.status())?;
        Ok(())
    }

    fn rename_resource(&self, resource_id: &str, name: &str) -> Result<()> {
        let response = self.send_mutation(
            "rename",
            self.client
                .post(resource_url(&["resources", resource_id, "rename"])?)
                .bearer_auth(&self.access_token)
                .json(&RenameRequest { name }),
        )?;
        status_error("resource rename", response.status())?;
        Ok(())
    }
}

fn transfer_url(value: &str) -> Result<reqwest::Url> {
    let url = reqwest::Url::parse(value).map_err(|_| "invalid MYBOX transfer URL")?;
    if url.scheme() != "https"
        || url.host_str().is_none()
        || !url.username().is_empty()
        || url.password().is_some()
    {
        return Err("MYBOX transfer URL must use HTTPS without embedded credentials".into());
    }
    Ok(url)
}

fn retry_status(status: reqwest::StatusCode) -> bool {
    status == reqwest::StatusCode::TOO_MANY_REQUESTS || status.is_server_error()
}

fn resource_url(parts: &[&str]) -> Result<reqwest::Url> {
    let mut url = reqwest::Url::parse(DRIVE_URL)?;
    url.path_segments_mut()
        .map_err(|_| "invalid MYBOX API base URL")?
        .extend(parts);
    Ok(url)
}

#[derive(Deserialize)]
struct RootParents {
    resources: Vec<RootParent>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct RootParent {
    parent_id: String,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct MoveRequest<'a> {
    parent_id: &'a str,
    is_overwrite: bool,
}

#[derive(Serialize)]
struct RenameRequest<'a> {
    name: &'a str,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct FolderRequest<'a> {
    folder_name: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    parent_id: Option<&'a str>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct CreatedFolder {
    resource_id: String,
    name: String,
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        io::{Read, Write},
        net::TcpListener,
    };

    #[test]
    fn failed_signed_download_does_not_expose_its_query() {
        let client = MyboxApiClient::new("unused");
        let error = client
            .download_range("https://127.0.0.1:0/download?stoken=private-marker", 0, 10)
            .err()
            .expect("unused local port should fail");
        assert!(!error.to_string().contains("private-marker"));
    }

    #[test]
    fn transfer_urls_require_https_and_hide_invalid_inputs() {
        for value in [
            "http://example.com/?secret=private-marker",
            "not-a-url-private-marker",
            "https://user:pass@example.com/",
        ] {
            let error = transfer_url(value).unwrap_err();
            assert!(!error.to_string().contains("private-marker"));
        }
        assert!(transfer_url("https://example.com/upload?sig=private-marker").is_ok());
        assert_eq!(
            resource_url(&["resources", "id/with?query"])
                .unwrap()
                .path(),
            "/v1/drive/resources/id%2Fwith%3Fquery"
        );
        assert!(retry_status(reqwest::StatusCode::TOO_MANY_REQUESTS));
        assert!(!retry_status(reqwest::StatusCode::CONFLICT));
    }

    #[test]
    fn retries_a_transient_read() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        let server = std::thread::spawn(move || {
            for status in ["503 Service Unavailable", "200 OK"] {
                let (mut stream, _) = listener.accept().unwrap();
                let mut request = [0; 1024];
                stream.read(&mut request).unwrap();
                write!(
                    stream,
                    "HTTP/1.1 {status}\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
                )
                .unwrap();
            }
        });
        let client = MyboxApiClient::new("unused");
        assert_eq!(
            client
                .get_response("test", || client.client.get(&url))
                .unwrap()
                .status(),
            reqwest::StatusCode::OK
        );
        server.join().unwrap();
    }
}
