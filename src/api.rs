use crate::Result;

pub struct MyboxApiClient;

impl MyboxApiClient {
    pub fn new() -> Self {
        Self
    }

    pub fn health_check(&self) -> Result<()> {
        Ok(())
    }
}
