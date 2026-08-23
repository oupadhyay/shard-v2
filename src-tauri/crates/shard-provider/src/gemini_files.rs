//! Gemini Files API transport and wire contracts.
//!
//! Image upload decisions, persisted chat-history URI ownership, retries, and
//! cleanup policy belong to the Shard host. This module only implements a
//! caller-configured resumable upload or deletion.

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
/// 2. Send an initial POST request to get a unique upload URL.
/// 3. Upload and finalize the file bytes at the returned URL.
/// 4. Parse the response into a provider file URI.
pub async fn upload_image_to_gemini_files_api(
    client: &reqwest::Client,
    image_base64: &str,
    mime_type: &str,
    config: &GeminiFilesUploadConfig,
) -> Result<GeminiFileUri, String> {
    use base64::{engine::general_purpose, Engine as _};

    let image_bytes = general_purpose::STANDARD
        .decode(image_base64)
        .map_err(|e| format!("Failed to decode base64 image: {}", e))?;
    let num_bytes = image_bytes.len();

    let extension = match mime_type {
        "image/avif" => Some("avif"),
        "image/bmp" => Some("bmp"),
        "image/gif" => Some("gif"),
        "image/heic" => Some("heic"),
        "image/heif" => Some("heif"),
        "image/jpeg" => Some("jpg"),
        "image/png" => Some("png"),
        "image/tiff" => Some("tiff"),
        "image/webp" => Some("webp"),
        _ => None,
    };
    let image_id = uuid::Uuid::new_v4();
    let display_name = match extension {
        Some(extension) => format!("image_{image_id}.{extension}"),
        None => format!("image_{image_id}"),
    };

    let init_response = client
        .post(&config.upload_url)
        .header("X-Goog-Api-Key", &config.auth_token)
        .header("X-Goog-Upload-Protocol", "resumable")
        .header("X-Goog-Upload-Command", "start")
        .header("X-Goog-Upload-Header-Content-Length", num_bytes.to_string())
        .header("X-Goog-Upload-Header-Content-Type", mime_type)
        .header("Content-Type", "application/json")
        .json(&InitialUploadRequest {
            file: FileMetadata { display_name },
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

    let upload_url = init_response
        .headers()
        .get("x-goog-upload-url")
        .and_then(|v| v.to_str().ok())
        .ok_or("No 'x-goog-upload-url' header in response")?
        .to_string();

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
        .trim_end_matches('/')
        .rsplit('/')
        .next()
        .filter(|file_name| !file_name.is_empty())
        .ok_or_else(|| "Gemini file URI did not contain a file name".to_string())?;
    let delete_url = format!(
        "{}/{}",
        config.files_base_url.trim_end_matches('/'),
        file_name
    );
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
    use wiremock::matchers::{header, method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

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

    #[tokio::test]
    async fn resumable_upload_sends_expected_requests_and_maps_response() {
        let server = MockServer::start().await;
        let upload_url = format!("{}/upload-session", server.uri());
        Mock::given(method("POST"))
            .and(path("/upload/v1beta/files"))
            .and(header("x-goog-api-key", "test-gemini"))
            .and(header("x-goog-upload-protocol", "resumable"))
            .and(header("x-goog-upload-command", "start"))
            .and(header("x-goog-upload-header-content-length", "4"))
            .and(header("x-goog-upload-header-content-type", "image/jpeg"))
            .and(header("content-type", "application/json"))
            .respond_with(ResponseTemplate::new(200).insert_header("x-goog-upload-url", upload_url))
            .expect(1)
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path("/upload-session"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "file": {
                    "uri": "https://generativelanguage.googleapis.com/v1beta/files/uploaded",
                    "mimeType": "image/jpeg"
                }
            })))
            .expect(1)
            .mount(&server)
            .await;

        let result = upload_image_to_gemini_files_api(
            &reqwest::Client::new(),
            "AQIDBA==",
            "image/jpeg",
            &GeminiFilesUploadConfig {
                upload_url: format!("{}/upload/v1beta/files", server.uri()),
                auth_token: "test-gemini".to_string(),
            },
        )
        .await
        .expect("upload should succeed");

        assert_eq!(result.mime_type, "image/jpeg");
        assert_eq!(
            result.file_uri,
            "https://generativelanguage.googleapis.com/v1beta/files/uploaded"
        );
        let requests = server.received_requests().await.unwrap();
        assert_eq!(requests.len(), 2);
        let initial_body: serde_json::Value = serde_json::from_slice(&requests[0].body).unwrap();
        let display_name = initial_body["file"]["display_name"].as_str().unwrap();
        assert!(display_name.starts_with("image_"));
        assert!(display_name.ends_with(".jpg"));
        assert_eq!(requests[1].headers["content-length"], "4");
        assert_eq!(requests[1].headers["x-goog-upload-offset"], "0");
        assert_eq!(
            requests[1].headers["x-goog-upload-command"],
            "upload, finalize"
        );
        assert_eq!(requests[1].body, vec![1, 2, 3, 4]);
    }

    #[tokio::test]
    async fn delete_uses_file_name_configured_base_url_and_api_key() {
        let server = MockServer::start().await;
        Mock::given(method("DELETE"))
            .and(path("/v1beta/files/uploaded"))
            .and(header("x-goog-api-key", "test-gemini"))
            .respond_with(ResponseTemplate::new(204))
            .expect(1)
            .mount(&server)
            .await;

        delete_uploaded_gemini_file(
            &reqwest::Client::new(),
            "https://generativelanguage.googleapis.com/v1beta/files/uploaded/",
            &GeminiFilesDeleteConfig {
                files_base_url: format!("{}/v1beta/files/", server.uri()),
                auth_token: "test-gemini".to_string(),
            },
        )
        .await
        .expect("delete should succeed");
    }
}
