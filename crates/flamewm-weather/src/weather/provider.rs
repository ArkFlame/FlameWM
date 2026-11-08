use super::{WeatherError, WeatherLocation, WeatherSnapshot};

pub trait HttpGet {
    fn get(&self, url: &str) -> Result<String, WeatherError>;
}

pub trait WeatherProvider {
    fn forecast(&self, location: &WeatherLocation) -> Result<WeatherSnapshot, WeatherError>;
    fn search(&self, query: &str) -> Result<Vec<WeatherLocation>, WeatherError>;
}
