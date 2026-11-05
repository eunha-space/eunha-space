//! The language a customer reads the portal and their email in.
//!
//! An account's language is `users.locale`. It starts as what their browser
//! asks for (`Accept-Language`), or the language the sign-up page was showing,
//! and they can change it on their account page. Emails are written in it, and
//! sent under the service's name in it: 은하공간 in Korean, eunha space in
//! English.

use axum::{
    Json,
    extract::State,
    http::{HeaderMap, StatusCode, header},
};
use serde::{Deserialize, Serialize};

use crate::{ApiError, CustomerEmailConfig, Shared, customer};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Locale {
    En,
    Ko,
}

impl Locale {
    pub(crate) fn code(self) -> &'static str {
        match self {
            Locale::En => "en",
            Locale::Ko => "ko",
        }
    }

    /// A language tag this portal is written in: `ko`, `ko-KR`, `en-US`.
    pub(crate) fn parse(tag: &str) -> Option<Self> {
        let primary = tag.trim().split(['-', '_']).next()?.to_ascii_lowercase();
        match primary.as_str() {
            "ko" => Some(Locale::Ko),
            "en" => Some(Locale::En),
            _ => None,
        }
    }

    /// The browser's most preferred language this portal is written in,
    /// English when it prefers none of them.
    pub(crate) fn from_accept_language(headers: &HeaderMap) -> Self {
        let Some(value) = headers
            .get(header::ACCEPT_LANGUAGE)
            .and_then(|value| value.to_str().ok())
        else {
            return Locale::En;
        };
        let mut ranges: Vec<(f32, usize, &str)> = value
            .split(',')
            .take(32)
            .enumerate()
            .filter_map(|(index, range)| {
                let mut parts = range.split(';');
                let tag = parts.next()?.trim();
                let quality = parts
                    .find_map(|parameter| parameter.trim().strip_prefix("q="))
                    .map_or(Some(1.0), |q| q.trim().parse::<f32>().ok())?;
                (quality > 0.0).then_some((quality, index, tag))
            })
            .collect();
        // Highest quality first; equal ones in the order the browser gave.
        ranges.sort_by(|a, b| b.0.total_cmp(&a.0).then(a.1.cmp(&b.1)));
        ranges
            .into_iter()
            .find_map(|(_, _, tag)| Locale::parse(tag))
            .unwrap_or(Locale::En)
    }

    /// What the service is called in this language.
    pub(crate) fn service_name(self) -> &'static str {
        match self {
            Locale::En => "eunha space",
            Locale::Ko => "은하공간",
        }
    }
}

/// The configured sender's address under the service's name in `locale`.
pub(crate) fn sender(mail: &CustomerEmailConfig, locale: Locale) -> lettre::message::Mailbox {
    lettre::message::Mailbox::new(
        Some(locale.service_name().to_owned()),
        mail.from.email.clone(),
    )
}

#[derive(Serialize)]
pub(crate) struct LocaleView {
    locale: &'static str,
    /// Whether this is the signed-in account's own setting rather than what
    /// the browser asks for.
    saved: bool,
}

/// The language to show: the signed-in account's, or what the browser asks for.
pub(crate) async fn get(
    State(s): State<Shared>,
    headers: HeaderMap,
) -> Result<Json<LocaleView>, ApiError> {
    if let Ok((user_id, _)) = customer(&headers, &s.db).await {
        let saved = sqlx::query_scalar::<_, Option<String>>("SELECT locale FROM users WHERE id=$1")
            .bind(user_id)
            .fetch_one(&s.db)
            .await?
            .and_then(|code| Locale::parse(&code));
        if let Some(locale) = saved {
            return Ok(Json(LocaleView {
                locale: locale.code(),
                saved: true,
            }));
        }
    }
    Ok(Json(LocaleView {
        locale: Locale::from_accept_language(&headers).code(),
        saved: false,
    }))
}

#[derive(Deserialize)]
pub(crate) struct LocaleChoice {
    locale: String,
}

/// Save the signed-in account's language.
pub(crate) async fn set(
    State(s): State<Shared>,
    headers: HeaderMap,
    Json(body): Json<LocaleChoice>,
) -> Result<StatusCode, ApiError> {
    let (user_id, _) = customer(&headers, &s.db).await?;
    let locale = Locale::parse(&body.locale)
        .ok_or_else(|| ApiError(StatusCode::BAD_REQUEST, "Choose English or Korean.".into()))?;
    // An administrator signed in as the user reads the site in their own
    // language without choosing the user's.
    if crate::masquerade::admin_of(&headers, &s.db)
        .await?
        .is_some()
    {
        return Ok(StatusCode::NO_CONTENT);
    }
    sqlx::query("UPDATE users SET locale=$2 WHERE id=$1")
        .bind(user_id)
        .bind(locale.code())
        .execute(&s.db)
        .await?;
    Ok(StatusCode::NO_CONTENT)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn headers(value: &str) -> HeaderMap {
        let mut headers = HeaderMap::new();
        headers.insert(header::ACCEPT_LANGUAGE, value.parse().unwrap());
        headers
    }

    #[test]
    fn the_browsers_most_preferred_supported_language_wins() {
        assert_eq!(
            Locale::from_accept_language(&headers("ko-KR,ko;q=0.9,en-US;q=0.8")),
            Locale::Ko
        );
        assert_eq!(
            Locale::from_accept_language(&headers("en-US,en;q=0.9,ko;q=0.8")),
            Locale::En
        );
        assert_eq!(
            Locale::from_accept_language(&headers("ja,ko;q=0.7,en;q=0.5")),
            Locale::Ko
        );
        assert_eq!(
            Locale::from_accept_language(&headers("en;q=0.4, ko;q=0.6")),
            Locale::Ko
        );
        assert_eq!(
            Locale::from_accept_language(&headers("ko;q=0, en")),
            Locale::En
        );
        assert_eq!(Locale::from_accept_language(&headers("fr, de")), Locale::En);
        assert_eq!(Locale::from_accept_language(&HeaderMap::new()), Locale::En);
    }

    #[test]
    fn tags_are_read_by_their_primary_language() {
        assert_eq!(Locale::parse("ko_KR"), Some(Locale::Ko));
        assert_eq!(Locale::parse("EN-gb"), Some(Locale::En));
        assert_eq!(Locale::parse("kok"), None);
        assert_eq!(Locale::parse(""), None);
    }
}
