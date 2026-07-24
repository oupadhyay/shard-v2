use serde::{Deserialize, Serialize};

// --- Open-Meteo Geocoding API Structures ---
#[derive(Serialize, Deserialize, Debug, Clone)]
struct GeocodingResult {
    id: Option<f64>,
    name: Option<String>,
    latitude: Option<f32>,
    longitude: Option<f32>,
    country: Option<String>,
    admin1: Option<String>,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
struct GeocodingResponse {
    results: Option<Vec<GeocodingResult>>,
    generationtime_ms: Option<f32>,
}

// --- Open-Meteo Weather API Structures ---
#[derive(Serialize, Deserialize, Debug, Clone)]
struct WeatherCurrentUnits {
    temperature_2m: Option<String>,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
struct WeatherCurrentData {
    time: Option<String>,
    interval: Option<i32>,
    temperature_2m: Option<f32>,
    weather_code: Option<i32>,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
struct WeatherDailyData {
    time: Option<Vec<String>>,
    weather_code: Option<Vec<i32>>,
    temperature_2m_max: Option<Vec<f32>>,
    temperature_2m_min: Option<Vec<f32>>,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
struct WeatherResponse {
    latitude: Option<f32>,
    longitude: Option<f32>,
    generationtime_ms: Option<f32>,
    utc_offset_seconds: Option<i32>,
    timezone: Option<String>,
    timezone_abbreviation: Option<String>,
    elevation: Option<f32>,
    current_units: Option<WeatherCurrentUnits>,
    current: Option<WeatherCurrentData>,
    daily: Option<WeatherDailyData>,
}

fn sanitize_log_str(s: &str) -> String {
    s.chars()
        .map(|c| if c == '\n' || c == '\r' { ' ' } else { c })
        .filter(|c| c.is_alphanumeric() || *c == ' ' || *c == ',' || *c == '.' || *c == '-')
        .collect()
}

pub async fn perform_weather_lookup(
    client: &reqwest::Client,
    location: &str,
) -> Result<String, String> {
    let geo_url = "https://geocoding-api.open-meteo.com/v1/search";
    let geo_params = [
        ("name", location),
        ("count", "1"),
        ("language", "en"),
        ("format", "json"),
    ];

    log::info!(
        "Performing Geocoding lookup for: {}",
        sanitize_log_str(location)
    );

    let geo_resp = client
        .get(geo_url)
        .query(&geo_params)
        .send()
        .await
        .map_err(|e| format!("Geocoding network error: {}", e))?;

    if !geo_resp.status().is_success() {
        return Err(format!("Geocoding API error: {}", geo_resp.status()));
    }

    let geo_data: GeocodingResponse = geo_resp
        .json()
        .await
        .map_err(|e| format!("Geocoding JSON parse error: {}", e))?;

    let location_data = match geo_data.results.as_ref().and_then(|r| r.first()) {
        Some(data) => data,
        None => {
            log::info!("No location found");
            return Err("Location not found".to_string());
        }
    };

    let lat = match location_data.latitude {
        Some(l) => l,
        None => return Err("Missing latitude".to_string()),
    };
    let lon = match location_data.longitude {
        Some(l) => l,
        None => return Err("Missing longitude".to_string()),
    };

    let name = match &location_data.name {
        Some(n) => sanitize_log_str(n),
        None => String::new(),
    };
    let country = match &location_data.country {
        Some(c) => sanitize_log_str(c),
        None => String::new(),
    };
    let location_display = format!("{}, {}", name, country);

    // 2. Weather
    let weather_url = "https://api.open-meteo.com/v1/forecast";
    let weather_params = [
        ("latitude", lat.to_string()),
        ("longitude", lon.to_string()),
        ("current", "temperature_2m,weather_code".to_string()),
        (
            "daily",
            "weather_code,temperature_2m_max,temperature_2m_min".to_string(),
        ),
        ("timezone", "auto".to_string()),
    ];

    log::info!("Performing Weather lookup for a sanitized location");

    let weather_resp = client
        .get(weather_url)
        .query(&weather_params)
        .send()
        .await
        .map_err(|e| format!("Weather network error: {}", e))?;

    if !weather_resp.status().is_success() {
        return Err(format!("Weather API error: {}", weather_resp.status()));
    }

    let weather_data: WeatherResponse = weather_resp
        .json()
        .await
        .map_err(|e| format!("Weather JSON parse error: {}", e))?;

    let mut result_json = serde_json::json!({
        "location": location_display,
    });

    if let Some(current) = weather_data.current {
        if let Some(temp) = current.temperature_2m {
            let unit = weather_data
                .current_units
                .and_then(|u| u.temperature_2m)
                .unwrap_or_else(|| "C".to_string());
            result_json["current"] = serde_json::json!({
                "temperature": temp,
                "unit": unit,
                "weather_code": current.weather_code.unwrap_or(0)
            });
        }
    }

    if let Some(daily) = weather_data.daily {
        if let (Some(times), Some(codes), Some(maxes), Some(mins)) = (
            daily.time,
            daily.weather_code,
            daily.temperature_2m_max,
            daily.temperature_2m_min,
        ) {
            let mut forecast = Vec::new();
            for i in 0..times.len().min(7) {
                if let (Some(t), Some(c), Some(max), Some(min)) =
                    (times.get(i), codes.get(i), maxes.get(i), mins.get(i))
                {
                    forecast.push(serde_json::json!({
                        "date": t,
                        "weather_code": c,
                        "max_temp": max,
                        "min_temp": min
                    }));
                }
            }
            result_json["forecast"] = serde_json::Value::Array(forecast);
        }
    }

    Ok(result_json.to_string())
}
