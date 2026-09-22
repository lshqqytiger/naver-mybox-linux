use crate::Result;

const STORAGE_URL: &str = "https://open-api.mybox.naver.com/v1/drive/storage";

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
}
