//! Stateless Gemini Files API transport helpers.
//!
//! Ownership split: the Gemini/provider API crate should own this resumable
//! upload/delete protocol eventually. The Shard host remains responsible for
//! deciding when to upload images, storing returned file URIs in chat history,
//! and deleting uploaded files during session cleanup.

use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct GeminiFileUri {
    #[serde(rename = "mimeType")]
    pub mime_type: String,
    #[serde(rename = "fileUri")]
    pub file_uri: String,
}

#[derive(Debug, Clone)]
pub struct GeminiFilesUploadConfig {
    pub upload_url: String,
    pub auth_token: String,
}

#[derive(Debug, Clone)]
pub struct GeminiFilesDeleteConfig {
    pub files_base_url: String,
    pub auth_token: String,
}

#[derive(Serialize)]
struct FileMetadata {
    display_name: String,
}

#[derive(Serialize)]
struct InitialUploadRequest {
    file: FileMetadata,
}

#[derive(Deserialize)]
struct UploadedFile {
    uri: String,
    #[serde(rename = "mimeType")]
    mime_type: String,
}

#[derive(Deserialize)]
struct UploadResponse {
    file: UploadedFile,
}

/// Uploads an image to the Gemini Files API using the resumable upload protocol.
///
/// Protocol steps:
/// 1. Decode base64 image to bytes.
/// 2. Send initial POST request to get a unique upload URL.
/// 3. Upload the file bytes to the upload URL.
/// 4. Parse the response to get the `fileUri`.
pub async fn upload_image_to_gemini_files_api(
    client: &reqwest::Client,
    image_base64: &str,
    mime_type: &str,
    config: &GeminiFilesUploadConfig,
) -> Result<GeminiFileUri, String> {
    use base64::{engine::general_purpose, Engine as _};

    // Step 1: Decode base64 to bytes
    let image_bytes = general_purpose::STANDARD
        .decode(image_base64)
        .map_err(|e| format!("Failed to decode base64 image: {}", e))?;
    let num_bytes = image_bytes.len();

    // Step 2: Initial POST to get upload_url
    // We generate a random display name to avoid collisions, though Gemini handles this.
    let display_name = format!("image_{}.png", uuid::Uuid::new_v4());

    let init_response = client
        .post(&config.upload_url)
        .header("X-Goog-Api-Key", &config.auth_token)
        .header("X-Goog-Upload-Protocol", "resumable")
        .header("X-Goog-Upload-Command", "start")
        .header("X-Goog-Upload-Header-Content-Length", num_bytes.to_string())
        .header("X-Goog-Upload-Header-Content-Type", mime_type)
        .header("Content-Type", "application/json")
        .json(&InitialUploadRequest {
            file: FileMetadata {
                display_name: display_name.clone(),
            },
        })
        .send()
        .await
        .map_err(|e| format!("Initial upload request failed (network error): {}", e))?;

    if !init_response.status().is_success() {
        let error_text = init_response.text().await.unwrap_or_default();
        return Err(format!(
            "Initial upload request failed (API error): {}",
            error_text
        ));
    }

    // Extract upload URL from headers
    let upload_url = init_response
        .headers()
        .get("x-goog-upload-url")
        .and_then(|v| v.to_str().ok())
        .ok_or("No 'x-goog-upload-url' header in response")?
        .to_string();

    // Step 3: Upload actual bytes
    let upload_response = client
        .post(&upload_url)
        .header("Content-Length", num_bytes.to_string())
        .header("X-Goog-Upload-Offset", "0")
        .header("X-Goog-Upload-Command", "upload, finalize")
        .body(image_bytes)
        .send()
        .await
        .map_err(|e| format!("File upload failed (network error): {}", e))?;

    if !upload_response.status().is_success() {
        let error_text = upload_response.text().await.unwrap_or_default();
        return Err(format!("File upload failed (API error): {}", error_text));
    }

    // Step 4: Parse response to get file URI
    let response_data: UploadResponse = upload_response
        .json()
        .await
        .map_err(|e| format!("Failed to parse upload response JSON: {}", e))?;

    Ok(GeminiFileUri {
        mime_type: response_data.file.mime_type,
        file_uri: response_data.file.uri,
    })
}

pub async fn delete_uploaded_gemini_file(
    client: &reqwest::Client,
    file_uri: &str,
    config: &GeminiFilesDeleteConfig,
) -> Result<(), String> {
    let file_name = file_uri
        .rsplit('/')
        .next()
        .ok_or_else(|| "Gemini file URI did not contain a file name".to_string())?;
    let delete_url = format!("{}/{}", config.files_base_url, file_name);
    let response = client
        .delete(delete_url)
        .header("X-Goog-Api-Key", &config.auth_token)
        .send()
        .await
        .map_err(|e| format!("Failed to delete Gemini file: {}", e))?;

    if response.status().is_success() {
        Ok(())
    } else {
        let status = response.status();
        let error_text = response.text().await.unwrap_or_default();
        Err(format!(
            "Failed to delete Gemini file (HTTP {}): {}",
            status, error_text
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn initial_upload_request_wire_shape_is_stable() {
        let request = InitialUploadRequest {
            file: FileMetadata {
                display_name: "image_test.png".to_string(),
            },
        };

        assert_eq!(
            serde_json::to_value(request).unwrap(),
            json!({
                "file": {
                    "display_name": "image_test.png"
                }
            })
        );
    }

    #[test]
    fn upload_response_maps_provider_fields_to_file_uri() {
        let response: UploadResponse = serde_json::from_value(json!({
            "file": {
                "uri": "https://generativelanguage.googleapis.com/v1beta/files/example",
                "mimeType": "image/png"
            }
        }))
        .unwrap();

        let file_uri = GeminiFileUri {
            mime_type: response.file.mime_type,
            file_uri: response.file.uri,
        };
        assert_eq!(
            serde_json::to_value(file_uri).unwrap(),
            json!({
                "mimeType": "image/png",
                "fileUri": "https://generativelanguage.googleapis.com/v1beta/files/example"
            })
        );
    }
}
