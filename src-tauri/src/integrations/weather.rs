use serde::{Deserialize, Serialize};
use reqwest;
use log;

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
}

pub async fn perform_weather_lookup(
    client: &reqwest::Client,
    location: &str,
) -> Result<Option<(f32, String, String)>, String> {
    // (temperature, unit, description/location_name)

    // 1. Geocoding
    let geo_url = "https://geocoding-api.open-meteo.com/v1/search";
    let geo_params = [("name", location), ("count", "1"), ("language", "en"), ("format", "json")];

    log::info!("Performing Geocoding lookup for: {}", location);

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
            log::info!("No location found for '{}'", location);
            return Ok(None);
        }
    };

    let lat = location_data.latitude.ok_or("Missing latitude")?;
    let lon = location_data.longitude.ok_or("Missing longitude")?;
    let name = location_data.name.clone().unwrap_or_default();
    let country = location_data.country.clone().unwrap_or_default();
    let location_display = format!("{}, {}", name, country);

    // 2. Weather
    let weather_url = "https://api.open-meteo.com/v1/forecast";
    let weather_params = [
        ("latitude", lat.to_string()),
        ("longitude", lon.to_string()),
        ("current", "temperature_2m".to_string()),
    ];

    log::info!("Performing Weather lookup for: {} ({}, {})", location_display, lat, lon);

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

    if let (Some(current), Some(units)) = (weather_data.current, weather_data.current_units) {
        if let (Some(temp), Some(unit)) = (current.temperature_2m, units.temperature_2m) {
            return Ok(Some((temp, unit, location_display)));
        }
    }

    Ok(None)
}
