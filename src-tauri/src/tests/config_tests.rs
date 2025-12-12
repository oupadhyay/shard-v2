
use crate::config::AppConfig;

#[test]
fn test_default_config_research_mode() {
    let config = AppConfig::default();
    assert_eq!(config.research_mode, Some(false));
}

#[test]
fn test_config_serialization() {
    let config = AppConfig {
        research_mode: Some(true),
        ..AppConfig::default()
    };

    let serialized = toml::to_string(&config).unwrap();
    assert!(serialized.contains("research_mode = true"));

    let deserialized: AppConfig = toml::from_str(&serialized).unwrap();
    assert_eq!(deserialized.research_mode, Some(true));
}
