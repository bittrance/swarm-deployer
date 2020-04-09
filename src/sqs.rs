use crate::{AckingMessage, Opt, PollingMessage, Result, SqsUrl};
use rusoto_sqs::{DeleteMessageRequest, GetQueueUrlRequest, Message, ReceiveMessageRequest, Sqs};
use snafu::ResultExt;

async fn resolve_queue_url(sqs: &dyn Sqs, opt: &Opt) -> Result<String> {
    let req = GetQueueUrlRequest {
        queue_name: opt.queue_name.clone(),
        ..Default::default()
    };
    let queue_url = sqs
        .get_queue_url(req)
        .await
        .with_context(|| SqsUrl {
            queue_name: opt.queue_name.clone(),
        })?
        .queue_url
        .unwrap();
    Ok(queue_url)
}

pub async fn poll_messages(sqs: &dyn Sqs, opt: &Opt) -> Result<Vec<Message>> {
    let queue_url = resolve_queue_url(sqs, opt).await?;
    let request = ReceiveMessageRequest {
        queue_url: queue_url.clone(),
        wait_time_seconds: Some(20),
        ..Default::default()
    };
    let messages = sqs
        .receive_message(request)
        .await
        .with_context(|| PollingMessage {
            queue_url: queue_url.clone(),
        })?
        .messages
        .unwrap_or_else(Vec::new);
    Ok(messages)
}

pub async fn delete_message(sqs: &dyn Sqs, message: &Message, opt: &Opt) -> Result<()> {
    let queue_url = resolve_queue_url(sqs, opt).await?;
    let receipt_handle = message.receipt_handle.as_ref().expect("No handle");
    let req = DeleteMessageRequest {
        queue_url: queue_url.clone(),
        receipt_handle: receipt_handle.clone(),
    };
    sqs.delete_message(req)
        .await
        .with_context(|| AckingMessage {
            queue_url: queue_url.clone(),
            receipt_handle,
        })?;
    Ok(())
}
