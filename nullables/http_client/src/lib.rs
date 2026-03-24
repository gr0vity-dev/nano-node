use anyhow::anyhow;
use rsnano_output_tracker::{OutputListenerMt, OutputTrackerMt};
use serde::{Serialize, de::DeserializeOwned};
use std::{collections::HashMap, sync::Arc};

pub use reqwest::{IntoUrl, Method, StatusCode, Url};
use tokio::task::yield_now;

pub struct HttpClient {
    strategy: HttpClientStrategy,
    request_listener: OutputListenerMt<TrackedRequest>,
}

impl HttpClient {
    pub fn new() -> Self {
        Self::new_with_strategy(HttpClientStrategy::Real(reqwest::Client::new()))
    }

    pub fn new_null() -> Self {
        Self::new_with_strategy(HttpClientStrategy::Nulled(HttpClientStub::with_response(
            ConfiguredHttpResponse::Json(JsonResponse::default()),
        )))
    }

    fn new_with_strategy(strategy: HttpClientStrategy) -> Self {
        Self {
            strategy,
            request_listener: OutputListenerMt::new(),
        }
    }

    pub fn null_builder() -> NulledHttpClientBuilder {
        NulledHttpClientBuilder {
            responses: HashMap::new(),
        }
    }

    pub async fn post_json<U: IntoUrl, T: Serialize + ?Sized>(
        &self,
        url: U,
        json: &T,
    ) -> anyhow::Result<Response> {
        let url = url.into_url()?;
        if self.request_listener.is_tracked() {
            self.request_listener.emit(TrackedRequest {
                url: url.clone(),
                method: Method::POST,
                json: serde_json::to_value(json)?,
            });
        }

        match &self.strategy {
            HttpClientStrategy::Real(client) => {
                Ok(client.post(url).json(json).send().await?.into())
            }
            HttpClientStrategy::Nulled(client) => client.get_response(Method::POST, url).await,
        }
    }

    pub fn track_requests(&self) -> Arc<OutputTrackerMt<TrackedRequest>> {
        self.request_listener.track()
    }
}

impl Default for HttpClient {
    fn default() -> Self {
        Self::new()
    }
}

enum HttpClientStrategy {
    Real(reqwest::Client),
    Nulled(HttpClientStub),
}

pub struct NulledHttpClientBuilder {
    responses: HashMap<(Url, Method), ConfiguredHttpResponse>,
}

impl NulledHttpClientBuilder {
    pub fn respond(self, response: JsonResponse) -> HttpClient {
        Self::create_client_with_response(ConfiguredHttpResponse::Json(response))
    }

    pub fn respond_url(
        mut self,
        method: Method,
        url: impl IntoUrl,
        response: ConfiguredHttpResponse,
    ) -> Self {
        self.responses
            .insert((url.into_url().unwrap(), method), response);
        self
    }

    pub fn fail_with(self, error_message: impl Into<String>) -> HttpClient {
        Self::create_client_with_response(ConfiguredHttpResponse::Error(error_message.into()))
    }

    pub fn finish(self) -> HttpClient {
        Self::create_client(HttpClientStub {
            responses: self.responses,
            ..Default::default()
        })
    }

    pub fn halt(self) -> HttpClient {
        Self::create_client_with_response(ConfiguredHttpResponse::Halt)
    }

    fn create_client_with_response(response: ConfiguredHttpResponse) -> HttpClient {
        Self::create_client(HttpClientStub::with_response(response))
    }

    fn create_client(stub: HttpClientStub) -> HttpClient {
        HttpClient::new_with_strategy(HttpClientStrategy::Nulled(stub))
    }
}

#[derive(Default)]
struct HttpClientStub {
    the_only_response: Option<ConfiguredHttpResponse>,
    responses: HashMap<(Url, Method), ConfiguredHttpResponse>,
}

impl HttpClientStub {
    fn with_response(response: ConfiguredHttpResponse) -> Self {
        Self {
            the_only_response: Some(response),
            ..Default::default()
        }
    }

    async fn get_response(&self, method: Method, url: Url) -> anyhow::Result<Response> {
        let response = if let Some(r) = &self.the_only_response {
            r
        } else {
            self.responses
                .get(&(url.clone(), method.clone()))
                .ok_or_else(|| anyhow!("no response configured for {} {}", method, url))?
        };

        match response {
            ConfiguredHttpResponse::Json(i) => Ok(i.clone().into()),
            ConfiguredHttpResponse::Error(e) => Err(anyhow!("{}", e)),
            ConfiguredHttpResponse::Halt => loop {
                yield_now().await;
            },
        }
    }
}

#[derive(Clone)]
pub struct TrackedRequest {
    pub url: Url,
    pub method: Method,
    pub json: serde_json::Value,
}

pub struct Response {
    strategy: ResponseStrategy,
}

impl Response {
    pub fn status(&self) -> StatusCode {
        match &self.strategy {
            ResponseStrategy::Real(resp) => resp.status(),
            ResponseStrategy::Nulled(resp) => resp.status,
        }
    }

    pub fn error_for_status(self) -> anyhow::Result<Self> {
        match self.strategy {
            ResponseStrategy::Real(resp) => Ok(Self {
                strategy: ResponseStrategy::Real(resp.error_for_status()?),
            }),
            ResponseStrategy::Nulled(resp) => {
                let status = resp.status;
                if status.is_client_error() || status.is_server_error() {
                    Err(anyhow!("Nulled HTTP client error ({})", status))
                } else {
                    Ok(Self {
                        strategy: ResponseStrategy::Nulled(resp),
                    })
                }
            }
        }
    }

    pub async fn json<T: DeserializeOwned>(self) -> anyhow::Result<T> {
        match self.strategy {
            ResponseStrategy::Real(resp) => Ok(resp.json().await?),
            ResponseStrategy::Nulled(resp) => resp.json(),
        }
    }
}

enum ResponseStrategy {
    Real(reqwest::Response),
    Nulled(JsonResponse),
}

impl From<reqwest::Response> for Response {
    fn from(value: reqwest::Response) -> Self {
        Self {
            strategy: ResponseStrategy::Real(value),
        }
    }
}

pub enum ConfiguredHttpResponse {
    Json(JsonResponse),
    Error(String),
    Halt,
}

#[derive(Clone)]
pub struct JsonResponse {
    status: StatusCode,
    body: serde_json::Value,
}

impl JsonResponse {
    pub fn new(status: StatusCode, json: impl Serialize) -> Self {
        Self {
            status,
            body: serde_json::to_value(json).unwrap(),
        }
    }
    pub fn json<T: DeserializeOwned>(&self) -> anyhow::Result<T> {
        let deserialized = serde_json::from_value(self.body.clone())?;
        Ok(deserialized)
    }
}

impl Default for JsonResponse {
    fn default() -> Self {
        Self {
            status: StatusCode::OK,
            body: serde_json::Value::Null,
        }
    }
}

impl From<JsonResponse> for Response {
    fn from(value: JsonResponse) -> Self {
        Self {
            strategy: ResponseStrategy::Nulled(value),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use reqwest::StatusCode;

    #[tokio::test]
    async fn make_real_request() {
        let server = test_http_server::start().await;

        let client = HttpClient::new();
        let result = client
            .post_json(server.url(), &vec!["hello"])
            .await
            .unwrap();
        assert_eq!(result.status(), StatusCode::OK);
        let response = result.json::<Vec<String>>().await.unwrap();
        assert_eq!(response, vec!["hello".to_string(), "world".to_string()]);
    }

    #[tokio::test]
    async fn track_requests() {
        let client = HttpClient::new_null();
        let tracker = client.track_requests();
        let target_url: Url = "http://127.0.0.1:42/foobar".parse().unwrap();
        let data = vec![1, 2, 3];

        client.post_json(target_url.clone(), &data).await.unwrap();

        let requests = tracker.output();
        assert_eq!(requests.len(), 1);
        assert_eq!(requests[0].url, target_url);
        assert_eq!(requests[0].method, Method::POST);
        assert_eq!(requests[0].json, serde_json::to_value(&data).unwrap());
    }

    #[tokio::test]
    async fn error_for_status() {
        let server = test_http_server::start().await;

        let client = HttpClient::new();
        let url: Url = format!("{}/not-found", server.url()).parse().unwrap();
        let result = client.post_json(url.clone(), &vec!["hello"]).await.unwrap();
        assert_eq!(result.status(), StatusCode::NOT_FOUND);
        match result.error_for_status() {
            Ok(_) => panic!("should return error!"),
            Err(e) => {
                assert_eq!(
                    e.to_string(),
                    format!("HTTP status client error (404 Not Found) for url ({url})")
                );
            }
        }
    }

    mod nullability {
        use super::*;
        use std::time::Duration;
        use tokio::time::timeout;

        #[tokio::test]
        async fn can_be_nulled() {
            let client = HttpClient::new_null();
            let response = client.post_json(test_url(), "foobar").await.unwrap();
            assert_eq!(response.status(), StatusCode::OK);
        }

        #[tokio::test]
        async fn return_configured_json_response() {
            let client = HttpClient::null_builder()
                .respond(JsonResponse::new(StatusCode::OK, vec![1, 2, 3]));

            let response = client.post_json(test_url(), "").await.unwrap();
            assert_eq!(response.json::<Vec<i32>>().await.unwrap(), vec![1, 2, 3]);
        }

        #[tokio::test]
        async fn error_for_status() {
            let client = HttpClient::null_builder()
                .respond(JsonResponse::new(StatusCode::NOT_FOUND, "not found"));

            let response = client.post_json(test_url(), "").await.unwrap();
            match response.error_for_status() {
                Ok(_) => panic!("should return error"),
                Err(e) => {
                    assert_eq!(e.to_string(), "Nulled HTTP client error (404 Not Found)");
                }
            }
        }

        #[tokio::test]
        async fn failing_null() {
            let client = HttpClient::null_builder().fail_with("my error");
            let Err(error) = client.post_json(test_url(), "").await else {
                panic!("did not fail!")
            };
            assert_eq!(error.to_string(), "my error")
        }

        #[tokio::test]
        async fn halts() {
            let client = HttpClient::null_builder().halt();
            let result = timeout(Duration::ZERO, client.post_json(test_url(), "")).await;
            assert!(matches!(result, Err(_)));
        }

        #[tokio::test]
        async fn halts_only_for_specified_url() {
            let client = HttpClient::null_builder()
                .respond_url(Method::POST, test_url(), ConfiguredHttpResponse::Halt)
                .finish();
            let result = timeout(Duration::ZERO, client.post_json(test_url(), "")).await;
            assert!(matches!(result, Err(_)));
        }

        fn test_url() -> Url {
            "http://127.0.0.1:42".parse().unwrap()
        }
    }

    mod test_http_server {
        use axum::{Json, Router, routing::post};
        use tokio::{net::TcpListener, sync::oneshot};
        use tokio_util::sync::CancellationToken;

        use crate::Url;

        pub(crate) struct TestServer {
            url: Url,
            cancel_token: CancellationToken,
        }

        impl TestServer {
            pub(crate) fn url(&self) -> Url {
                self.url.clone()
            }
        }

        impl Drop for TestServer {
            fn drop(&mut self) {
                self.cancel_token.cancel();
            }
        }

        pub(crate) async fn start() -> TestServer {
            let listener = TcpListener::bind(("127.0.0.1", 0)).await.unwrap();
            let addr = listener.local_addr().unwrap();
            let server = TestServer {
                url: format!("http://{addr}").parse().unwrap(),
                cancel_token: CancellationToken::new(),
            };
            let cancel_token = server.cancel_token.clone();
            let (tx_ready, rx_ready) = oneshot::channel::<()>();

            tokio::spawn(async move { run_server(listener, cancel_token, tx_ready).await });

            rx_ready.await.unwrap();

            server
        }

        async fn run_server(
            listener: TcpListener,
            cancel_token: CancellationToken,
            tx_ready: oneshot::Sender<()>,
        ) {
            tx_ready.send(()).unwrap();
            tokio::select! {
                _ = serve(listener) => { },
                _ = cancel_token.cancelled() => { }
            }
        }

        async fn serve(tcp_listener: TcpListener) {
            let app = Router::new().route("/", post(root));
            axum::serve(tcp_listener, app).await.unwrap()
        }

        async fn root(Json(mut payload): Json<Vec<String>>) -> Json<Vec<String>> {
            payload.push("world".to_string());
            Json(payload)
        }
    }
}
