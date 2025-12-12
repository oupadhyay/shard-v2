use yahoo_finance_api as yfa;
use time::OffsetDateTime;
use log;


pub async fn perform_finance_lookup(ticker: &str) -> Result<String, String> {
    log::info!("Performing Finance lookup for: {}", ticker);

    let provider = yfa::YahooConnector::new()
        .map_err(|e| format!("Failed to create Yahoo Connector: {}", e))?;

    // Get the latest quotes
    let response = provider
        .get_latest_quotes(ticker, "1d")
        .await
        .map_err(|e| format!("Yahoo Finance API error: {}", e))?;

    let quote = response.last_quote().map_err(|e| format!("No quote data found: {}", e))?;

    let price = quote.close;
    let time = OffsetDateTime::from_unix_timestamp(quote.timestamp as i64)
        .map_err(|_| "Invalid timestamp")?;

    // Try to get more info if possible, but for now basic price
    // We could format this nicely

    let result = format!(
        "Stock: {}\nPrice: ${:.2}\nTime: {}\nVolume: {}",
        ticker.to_uppercase(),
        price,
        time,
        quote.volume
    );

    Ok(result)
}
