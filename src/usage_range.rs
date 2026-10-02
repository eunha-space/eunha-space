//! Shared bounds for resource history queries and chart resolution.
use super::*;

pub(crate) const RETENTION_MS: i64 = 28 * 86_400_000;

#[derive(Clone, Copy, Default, Deserialize)]
pub(crate) struct Range {
    pub minutes: Option<i64>,
}
impl Range {
    pub fn bounds(self, now: i64) -> Result<(i64, i64), ApiError> {
        let minutes = self.minutes.unwrap_or(10);
        if !(10..=40320).contains(&minutes) {
            return Err(ApiError(
                StatusCode::BAD_REQUEST,
                "Choose a range from 10 minutes to 4 weeks.".into(),
            ));
        }
        let duration = minutes * 60000;
        let bucket = ((duration + 3_599_999) / 3_600_000).max(1) * 10000;
        Ok((now - duration, bucket))
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn ranges_are_bounded_and_resolution_limits_points() {
        for minutes in [10, 60, 360, 1440, 10080, 40320] {
            let (start, bucket) = Range {
                minutes: Some(minutes),
            }
            .bounds(RETENTION_MS)
            .unwrap();
            assert_eq!(RETENTION_MS - start, minutes * 60000);
            assert!(minutes * 60000 / bucket <= 360);
        }
        for minutes in [-1, 0, 9, 40321, i64::MAX] {
            assert!(
                Range {
                    minutes: Some(minutes)
                }
                .bounds(0)
                .is_err()
            );
        }
    }
}
