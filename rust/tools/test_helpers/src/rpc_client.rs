use anyhow::{bail, Result};
use reqwest::Url;
use serde_json::{json, Value};
use std::time::Duration;

pub struct RpcClient {
    url: Url,
    client: reqwest::Client,
}

impl RpcClient {
    pub fn new(url: Url) -> Self {
        Self {
            url,
            client: reqwest::ClientBuilder::new()
                .timeout(Duration::from_secs(5))
                .build()
                .unwrap(),
        }
    }

    async fn rpc_request(&self, request: &serde_json::Value) -> Result<serde_json::Value> {
        let result = self
            .client
            .post(self.url.clone())
            .json(request)
            .send()
            .await?
            .error_for_status()?
            .json::<serde_json::Value>()
            .await?;

        if let Some(error) = result.get("error") {
            bail!("node returned error: {}", error);
        }

        Ok(result)
    }

    pub async fn account_balance(
        &self,
        destination: &str,
        only_confirmed: Option<&str>,
    ) -> Result<Value> {
        let request = json!({
            "action": "account_balance",
            "account": destination,
            "only_confirmed": only_confirmed.unwrap_or("true"),
        });
        Ok(self.rpc_request(&request).await?)
    }

    pub async fn account_create(&self, wallet: &str, index: Option<&str>) -> Result<Value> {
        let request = if index.is_some() {
            json!({
                "action": "account_create",
                "wallet": wallet,
                "index": index.unwrap(),
            })
        } else {
            json!({
                "action": "account_create",
                "wallet": wallet,
            })
        };

        Ok(self.rpc_request(&request).await?)
    }
}
