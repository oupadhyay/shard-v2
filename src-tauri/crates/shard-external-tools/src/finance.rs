use time::Duration;
use yahoo_finance_api as yfa;

pub async fn perform_finance_lookup(ticker: &str) -> Result<String, String> {
    let sanitized_ticker = ticker.replace(['\n', '\r'], " ");
    log::info!("Performing Finance lookup for: {}", sanitized_ticker);

    let provider = yfa::YahooConnector::new()
        .map_err(|e| format!("Failed to create Yahoo Connector: {}", e))?;

    // Get the latest quotes and 1 month history
    let end = time::OffsetDateTime::now_utc();
    let start = end - Duration::days(30);

    let hist_response = provider
        .get_quote_history(ticker, start, end)
        .await
        .map_err(|e| format!("Yahoo Finance API error: {}", e))?;

    let quotes = hist_response
        .quotes()
        .map_err(|e| format!("No quote data found: {}", e))?;

    if quotes.is_empty() {
        return Err("No stock data found for ticker".to_string());
    }

    let latest = quotes.last().unwrap();
    let current_price = latest.close;

    // Calculate percent change from last known close (i.e. if quotes has at least 2 entries)
    let percent_change = if quotes.len() >= 2 {
        let prev_close = quotes[quotes.len() - 2].close;
        if prev_close != 0.0 {
            ((current_price - prev_close) / prev_close) * 100.0
        } else {
            0.0
        }
    } else {
        0.0
    };

    let history: Vec<serde_json::Value> = quotes
        .iter()
        .map(|q| {
            serde_json::json!({
                "timestamp": q.timestamp,
                "close": q.close
            })
        })
        .collect();

    let result_json = serde_json::json!({
        "symbol": ticker.to_uppercase(),
        "current_price": current_price,
        "percent_change": percent_change,
        "volume": latest.volume,
        "history": history
    });

    Ok(result_json.to_string())
}
