use std::time::Duration;

use crate::stripe::{Client, Invoice, InvoiceStatus};

/// Retries a failed charge with exponential backoff.
pub async fn settle(client: &Client, invoice: &Invoice) -> anyhow::Result<()> {
    let mut delay = Duration::from_millis(250);
    for attempt in 1..=5 {
        match client.charge(invoice).await {
            Ok(receipt) => {
                tracing::info!(attempt, id = %receipt.id, "invoice settled");
                return Ok(());
            }
            Err(e) if e.is_retryable() => {
                tracing::warn!(attempt, "charge failed: {e}; retrying in {delay:?}");
                tokio::time::sleep(delay).await;
                delay *= 2;
            }
            Err(e) => return Err(e.into()),
        }
    }
    client.mark(invoice, InvoiceStatus::PastDue).await?;
    Ok(())
}
