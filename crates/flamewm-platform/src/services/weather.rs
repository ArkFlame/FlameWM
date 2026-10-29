//! Cached weather feed published by the shell. Stores the opaque payload and its revision;
//! it never fetches, polls or decodes the forecast.

use flamewm_api::weather::{MAX_WEATHER_FEED_BYTES, WeatherFeedSnapshot};
use flamewm_api::{FlameError, FlameResult};

#[derive(Debug, Default)]
pub struct WeatherFeedService {
    snapshot: WeatherFeedSnapshot,
}

impl WeatherFeedService {
    #[must_use]
    pub fn snapshot(&self) -> WeatherFeedSnapshot {
        self.snapshot.clone()
    }

    /// Stores `payload` and returns `true` when it differs from the cached one (the revision
    /// advances only then). An oversized payload is rejected and leaves the cache unchanged.
    pub fn publish(&mut self, payload: String) -> FlameResult<bool> {
        if payload.len() > MAX_WEATHER_FEED_BYTES {
            return Err(FlameError::invalid("weather feed payload is too large"));
        }
        if payload == self.snapshot.payload {
            return Ok(false);
        }
        self.snapshot.revision = self.snapshot.revision.wrapping_add(1);
        self.snapshot.payload = payload;
        Ok(true)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn publish_advances_revision_only_on_change() {
        let mut service = WeatherFeedService::default();
        assert_eq!(service.snapshot(), WeatherFeedSnapshot::default());
        assert_eq!(service.publish(String::new()), Ok(false));
        assert_eq!(service.publish("a".to_owned()), Ok(true));
        assert_eq!(service.publish("a".to_owned()), Ok(false));
        let snapshot = service.snapshot();
        assert_eq!(snapshot.revision, 1);
        assert_eq!(snapshot.payload, "a");
    }

    #[test]
    fn oversized_payload_is_rejected_without_touching_cache() {
        let mut service = WeatherFeedService::default();
        service.publish("kept".to_owned()).unwrap();
        let big = "x".repeat(MAX_WEATHER_FEED_BYTES + 1);
        assert!(service.publish(big).is_err());
        assert_eq!(service.snapshot().payload, "kept");
        assert_eq!(service.snapshot().revision, 1);
    }
}
