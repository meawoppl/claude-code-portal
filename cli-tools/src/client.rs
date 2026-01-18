//! Native HTTP client implementation using reqwest

use reqwest::{Client, StatusCode};
use shared::api::{
    endpoints, ApiClientConfig, ApiError, CcProxyApi, CreateProxyTokenRequest,
    CreateProxyTokenResponse, DeviceCodeResponse, HealthResponse,
};
use shared::{DevicePollRequest, DevicePollResponse, SessionInfo, UserInfo};

/// Native API client using reqwest
pub struct NativeApiClient {
    client: Client,
    config: ApiClientConfig,
}

impl NativeApiClient {
    pub fn new(base_url: &str, token: Option<&str>) -> Self {
        let config = if let Some(t) = token {
            ApiClientConfig::new(base_url).with_token(t)
        } else {
            ApiClientConfig::new(base_url)
        };

        Self {
            client: Client::builder()
                .cookie_store(true)
                .build()
                .expect("Failed to create HTTP client"),
            config,
        }
    }

    fn add_auth(&self, req: reqwest::RequestBuilder) -> reqwest::RequestBuilder {
        if let Some(token) = &self.config.auth_token {
            req.header("Authorization", format!("Bearer {}", token))
        } else {
            req
        }
    }

    async fn handle_response<T: serde::de::DeserializeOwned>(
        &self,
        response: reqwest::Response,
    ) -> Result<T, ApiError> {
        let status = response.status();

        if status == StatusCode::UNAUTHORIZED {
            return Err(ApiError::Auth("Unauthorized".to_string()));
        }

        if status == StatusCode::NOT_FOUND {
            return Err(ApiError::NotFound("Resource not found".to_string()));
        }

        if !status.is_success() {
            let message = response
                .text()
                .await
                .unwrap_or_else(|_| "Unknown error".to_string());
            return Err(ApiError::Server {
                status: status.as_u16(),
                message,
            });
        }

        response
            .json::<T>()
            .await
            .map_err(|e| ApiError::Parse(e.to_string()))
    }
}

impl CcProxyApi for NativeApiClient {
    async fn health(&self) -> Result<HealthResponse, ApiError> {
        let url = self.config.url(endpoints::HEALTH);
        let response = self
            .client
            .get(&url)
            .send()
            .await
            .map_err(|e| ApiError::Network(e.to_string()))?;

        // Health endpoint might return plain text
        if response.status().is_success() {
            let text = response
                .text()
                .await
                .map_err(|e| ApiError::Parse(e.to_string()))?;

            // Try to parse as JSON, fallback to treating text as status
            serde_json::from_str(&text).unwrap_or(HealthResponse {
                status: text,
                version: None,
            });

            Ok(HealthResponse {
                status: "ok".to_string(),
                version: None,
            })
        } else {
            Err(ApiError::Server {
                status: response.status().as_u16(),
                message: "Health check failed".to_string(),
            })
        }
    }

    async fn get_me(&self) -> Result<UserInfo, ApiError> {
        let url = self.config.url(endpoints::AUTH_ME);
        let req = self.add_auth(self.client.get(&url));

        let response = req
            .send()
            .await
            .map_err(|e| ApiError::Network(e.to_string()))?;

        self.handle_response(response).await
    }

    async fn list_sessions(&self) -> Result<Vec<SessionInfo>, ApiError> {
        let url = self.config.url(endpoints::SESSIONS);
        let req = self.add_auth(self.client.get(&url));

        let response = req
            .send()
            .await
            .map_err(|e| ApiError::Network(e.to_string()))?;

        // Backend wraps sessions in {"sessions": [...]}
        #[derive(serde::Deserialize)]
        struct SessionsResponse {
            sessions: Vec<SessionInfo>,
        }

        let wrapper: SessionsResponse = self.handle_response(response).await?;
        Ok(wrapper.sessions)
    }

    async fn get_session(&self, id: &str) -> Result<SessionInfo, ApiError> {
        let url = self.config.url(&endpoints::session(id));
        let req = self.add_auth(self.client.get(&url));

        let response = req
            .send()
            .await
            .map_err(|e| ApiError::Network(e.to_string()))?;

        self.handle_response(response).await
    }

    async fn delete_session(&self, id: &str) -> Result<(), ApiError> {
        let url = self.config.url(&endpoints::session(id));
        let req = self.add_auth(self.client.delete(&url));

        let response = req
            .send()
            .await
            .map_err(|e| ApiError::Network(e.to_string()))?;

        if response.status().is_success() {
            Ok(())
        } else {
            let status = response.status().as_u16();
            let message = response
                .text()
                .await
                .unwrap_or_else(|_| "Unknown error".to_string());
            Err(ApiError::Server { status, message })
        }
    }

    async fn create_proxy_token(
        &self,
        req: CreateProxyTokenRequest,
    ) -> Result<CreateProxyTokenResponse, ApiError> {
        let url = self.config.url(endpoints::PROXY_TOKENS);
        let request = self.add_auth(self.client.post(&url)).json(&req);

        let response = request
            .send()
            .await
            .map_err(|e| ApiError::Network(e.to_string()))?;

        self.handle_response(response).await
    }

    async fn request_device_code(&self) -> Result<DeviceCodeResponse, ApiError> {
        let url = self.config.url(endpoints::DEVICE_CODE);

        let response = self
            .client
            .post(&url)
            .send()
            .await
            .map_err(|e| ApiError::Network(e.to_string()))?;

        self.handle_response(response).await
    }

    async fn poll_device_code(&self, device_code: &str) -> Result<DevicePollResponse, ApiError> {
        let url = self.config.url(endpoints::DEVICE_POLL);
        let req_body = DevicePollRequest {
            device_code: device_code.to_string(),
        };

        let response = self
            .client
            .post(&url)
            .json(&req_body)
            .send()
            .await
            .map_err(|e| ApiError::Network(e.to_string()))?;

        self.handle_response(response).await
    }
}
