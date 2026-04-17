use std::collections::HashMap;

// Mock FlashMessage and FlashLevel
#[derive(Debug, serde::Serialize)]
#[serde(rename_all = "lowercase")]
pub enum FlashLevel {
    Success,
}

#[derive(Debug, serde::Serialize)]
pub struct FlashMessage {
    pub level: FlashLevel,
    pub message: String,
}

fn main() {
    let messages = vec![
        FlashMessage { level: FlashLevel::Success, message: "Hello".to_string() }
    ];
    let payload = serde_json::json!({
        "flash": messages
    });
    println!("{}", payload.to_string());
}
