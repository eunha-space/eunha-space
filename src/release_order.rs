//! Which of two eunha releases is newer, by eunha's commit graph.
//!
//! A release is a commit on eunha's `main`, and a newer one is a descendant
//! of an older one. Build times only stand in for that: an old commit built
//! for the first time after a newer one would look newer. The graph has no
//! such case. With CI builds the control plane has no checkout and asks
//! GitHub; otherwise it asks the central checkout it builds from. Two commits'
//! order never changes, so every answer is kept in `eunha_release_order`
//! and asked for once.

use std::process::Stdio;

use sqlx::PgPool;

use crate::{central_paths, ci_release, is_git_sha};

/// Where `head` stands against `base`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Order {
    /// The same commit.
    Identical,
    /// `head` descends from `base`: it is the newer release.
    Ahead,
    /// `base` descends from `head`: it is the older release.
    Behind,
    /// Neither descends from the other, which commits on one `main` never do
    /// unless it was rewritten.
    Diverged,
}

impl Order {
    fn as_str(self) -> &'static str {
        match self {
            Order::Identical => "identical",
            Order::Ahead => "ahead",
            Order::Behind => "behind",
            Order::Diverged => "diverged",
        }
    }

    fn parse(value: &str) -> Option<Self> {
        match value {
            "identical" => Some(Order::Identical),
            "ahead" => Some(Order::Ahead),
            "behind" => Some(Order::Behind),
            "diverged" => Some(Order::Diverged),
            _ => None,
        }
    }

    /// `head` is the release or newer.
    pub(crate) fn at_least(self) -> bool {
        matches!(self, Order::Identical | Order::Ahead)
    }

    fn reversed(self) -> Self {
        match self {
            Order::Ahead => Order::Behind,
            Order::Behind => Order::Ahead,
            order => order,
        }
    }
}

/// Where release `head` stands against release `base`. The error is fixed
/// text for the console.
pub(crate) async fn compare(db: &PgPool, base: &str, head: &str) -> Result<Order, String> {
    if !is_git_sha(base) || !is_git_sha(head) {
        return Err("A release is not a commit.".into());
    }
    if base == head {
        return Ok(Order::Identical);
    }
    if let Some(order) = cached(db, base, head).await {
        return Ok(order);
    }
    let order = match ci_release::Source::from_env()? {
        Some(ci) => Order::parse(&ci.compare(base, head).await?)
            .ok_or_else(|| "GitHub compared the releases in a way it never does.".to_string())?,
        None => local(base, head).await?,
    };
    // Best effort: a lost answer is only asked for again.
    let _ = sqlx::query(
        "INSERT INTO eunha_release_order(base,head,status) VALUES($1,$2,$3) ON CONFLICT DO NOTHING",
    )
    .bind(base)
    .bind(head)
    .bind(order.as_str())
    .execute(db)
    .await;
    Ok(order)
}

/// A kept answer, in either direction.
async fn cached(db: &PgPool, base: &str, head: &str) -> Option<Order> {
    let row = sqlx::query_as::<_, (String, bool)>(
        "SELECT status,base=$1 FROM eunha_release_order WHERE (base=$1 AND head=$2) OR (base=$2 AND head=$1) LIMIT 1",
    )
    .bind(base)
    .bind(head)
    .fetch_optional(db)
    .await
    .ok()??;
    let order = Order::parse(&row.0)?;
    Some(if row.1 { order } else { order.reversed() })
}

/// Ask the central checkout, which has every commit it ever built.
async fn local(base: &str, head: &str) -> Result<Order, String> {
    let (_, source) = central_paths()?;
    let is_ancestor = |ancestor: &str, descendant: &str| {
        let mut command = tokio::process::Command::new("/usr/bin/git");
        command
            .arg("-C")
            .arg(&source)
            .args(["merge-base", "--is-ancestor", ancestor, descendant])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        async move {
            match command.status().await.map(|status| status.code()) {
                Ok(Some(0)) => Ok(true),
                Ok(Some(1)) => Ok(false),
                _ => Err("The central Eunha checkout does not have both releases.".to_string()),
            }
        }
    };
    if is_ancestor(base, head).await? {
        return Ok(Order::Ahead);
    }
    if is_ancestor(head, base).await? {
        return Ok(Order::Behind);
    }
    Ok(Order::Diverged)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_answer_read_the_other_way_round_is_reversed() {
        for order in [
            Order::Identical,
            Order::Ahead,
            Order::Behind,
            Order::Diverged,
        ] {
            assert_eq!(Order::parse(order.as_str()), Some(order));
            assert_eq!(order.reversed().reversed(), order);
        }
        assert_eq!(Order::Ahead.reversed(), Order::Behind);
        assert_eq!(Order::Diverged.reversed(), Order::Diverged);
        assert_eq!(Order::parse("unknown"), None);
    }
}
