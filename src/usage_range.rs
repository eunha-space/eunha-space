//! Shared bounds for resource history queries and chart resolution.
use super::*;

pub(crate) const RETENTION_MS: i64 = 28 * 86_400_000;

#[derive(Clone, Copy, Default, Deserialize)]
pub(crate) struct Range {
    // Flattened scopes receive URL values as strings rather than the query
    // deserializer's numeric adapter. Accept both representations.
    #[serde(default, deserialize_with = "deserialize_minutes")]
    pub minutes: Option<i64>,
}
fn deserialize_minutes<'de, D: serde::Deserializer<'de>>(
    deserializer: D,
) -> Result<Option<i64>, D::Error> {
    #[derive(Deserialize)]
    #[serde(untagged)]
    enum Minutes {
        Number(i64),
        Text(String),
    }
    Option::<Minutes>::deserialize(deserializer)?
        .map(|value| match value {
            Minutes::Number(value) => Ok(value),
            Minutes::Text(value) => value.parse().map_err(serde::de::Error::custom),
        })
        .transpose()
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
    fn scoped_history_accepts_browser_query_parameters() {
        for minutes in [10, 60, 360, 1440, 10080, 40320] {
            let uri = format!(
                "/usage/history?minutes={minutes}&instance_id=00000000-0000-0000-0000-000000000001"
            )
            .parse()
            .unwrap();
            let Query(scope) = Query::<usage::AggregateScope>::try_from_uri(&uri).unwrap();
            assert_eq!(scope.range.minutes, Some(minutes));
            assert!(scope.instance_id.is_some());
            let Query(scope) = Query::<service_usage::Scope>::try_from_uri(&uri).unwrap();
            assert_eq!(scope.range.minutes, Some(minutes));
            let Query(range) = Query::<Range>::try_from_uri(&uri).unwrap();
            assert_eq!(range.minutes, Some(minutes));
        }
        let empty = "/usage/history".parse().unwrap();
        assert!(
            Query::<usage::AggregateScope>::try_from_uri(&empty)
                .unwrap()
                .range
                .minutes
                .is_none()
        );
        for value in ["abc", "1.5", "9223372036854775808"] {
            let uri = format!("/usage/history?minutes={value}").parse().unwrap();
            assert!(Query::<usage::AggregateScope>::try_from_uri(&uri).is_err());
            assert!(Query::<service_usage::Scope>::try_from_uri(&uri).is_err());
        }
    }

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
