use anyhow::{Context, Result};
use colored::Colorize;
use reqwest;
use serde::{Deserialize, Serialize};
use std::time::Duration;
use tokio::time::sleep;
use tracing::info;

#[derive(Debug, Serialize, Deserialize)]
struct DeviceCodeResponse {
    device_code: String,
    user_code: String,
    verification_uri: String,
    expires_in: u64,
    interval: u64,
}

#[derive(Debug, Serialize, Deserialize)]
struct TokenResponse {
    access_token: String,
    user_id: String,
    user_email: String,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "status")]
enum PollResponse {
    #[serde(rename = "pending")]
    Pending,

    #[serde(rename = "complete")]
    Complete {
        access_token: String,
        user_id: String,
        user_email: String,
    },

    #[serde(rename = "expired")]
    Expired,

    #[serde(rename = "denied")]
    Denied,
}

pub async fn device_flow_login(backend_url: &str) -> Result<(String, String, String)> {
    let client = reqwest::Client::new();

    // Step 1: Request device code
    let auth_base = backend_url
        .replace("ws://", "http://")
        .replace("wss://", "https://");
    let device_code_url = format!("{}/auth/device/code", auth_base);

    info!("Requesting device code from {}", device_code_url);

    let response: DeviceCodeResponse = client
        .post(&device_code_url)
        .send()
        .await
        .context("Failed to request device code")?
        .json()
        .await
        .context("Failed to parse device code response")?;

    // Step 2: Display instructions to user
    println!();
    println!(
        "{}",
        "╔═══════════════════════════════════════════════════════╗".bright_blue()
    );
    println!(
        "{}",
        "║           🔐 Authentication Required                 ║".bright_blue()
    );
    println!(
        "{}",
        "╚═══════════════════════════════════════════════════════╝".bright_blue()
    );
    println!();
    println!("  To authenticate this machine, please visit:");
    println!();
    println!("    {}", response.verification_uri.bright_green().bold());
    println!();
    println!("  And enter the code:");
    println!();
    println!(
        "    {}",
        response.user_code.bright_yellow().bold().underline()
    );
    println!();
    println!("  {} Waiting for authentication...", "⏳".bright_cyan());
    println!();

    // Step 3: Poll for completion
    let poll_url = format!("{}/auth/device/poll", auth_base);
    let interval = Duration::from_secs(response.interval.max(5));
    let expires_at = std::time::Instant::now() + Duration::from_secs(response.expires_in);

    loop {
        if std::time::Instant::now() > expires_at {
            anyhow::bail!("Authentication timed out");
        }

        sleep(interval).await;

        let poll_response: PollResponse = client
            .post(&poll_url)
            .json(&serde_json::json!({
                "device_code": response.device_code
            }))
            .send()
            .await
            .context("Failed to poll for authentication")?
            .json()
            .await
            .context("Failed to parse poll response")?;

        match poll_response {
            PollResponse::Pending => {
                // Still waiting
                continue;
            }
            PollResponse::Complete {
                access_token,
                user_id,
                user_email,
            } => {
                println!();
                println!("  {} Authentication successful!", "✓".bright_green());
                println!("  Logged in as: {}", user_email.bright_cyan());
                println!();
                return Ok((access_token, user_id, user_email));
            }
            PollResponse::Expired => {
                anyhow::bail!("Authentication code expired");
            }
            PollResponse::Denied => {
                anyhow::bail!("Authentication was denied");
            }
        }
    }
}
