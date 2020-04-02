use bollard::errors::Error as BollardError;
use bollard::service::{ListServicesOptions, Service, ServiceSpec, UpdateServiceOptions};
use bollard::Docker;
use log::{debug, info, warn};
use rusoto_core::Region;
use rusoto_core::RusotoError;
use rusoto_sqs::{
    DeleteMessageError, DeleteMessageRequest, GetQueueUrlError, GetQueueUrlRequest, Message,
    ReceiveMessageError, ReceiveMessageRequest, Sqs, SqsClient,
};
use serde_json;
use snafu::{ResultExt, Snafu};
use std::collections::HashMap;
use stderrlog;
use structopt::StructOpt;
use tokio::runtime::Runtime;

#[cfg(test)]
mod tests;

const STACK_IMAGE_LABEL: &str = "com.docker.stack.image";

#[derive(StructOpt, Debug)]
#[structopt()]
struct Opt {
    /// SQS queue name to receive ECR events
    #[structopt(short = "q", long = "queue")]
    queue_name: String,
    /// Silence all output
    #[structopt(long = "quiet")]
    quiet: bool,
    /// Verbose mode (-v, -vv, -vvv, etc)
    #[structopt(short = "v", long = "verbose", parse(from_occurrences))]
    verbose: usize,
}

#[derive(Debug, Snafu)]
enum SeedyError {
    #[snafu(display("Counld not instantiate a Docker client from environment {}", source))]
    DockerInstantiation { source: BollardError },
    #[snafu(display("Failed to retrieve URL for queue {}: {}", queue_name, source))]
    SqsUrl {
        queue_name: String,
        source: RusotoError<GetQueueUrlError>,
    },
    #[snafu(display("Polling for ECR events on {} failed: {}", queue_url, source))]
    PollingMessage {
        queue_url: String,
        source: RusotoError<ReceiveMessageError>,
    },
    #[snafu(display("Could not list services: {}", source))]
    ServiceListing { source: BollardError },
    #[snafu(display("Failed to update image for service {}: {}", service_id, source))]
    UpdatingService {
        service_id: String,
        source: BollardError,
    },
    #[snafu(display(
        "Failed to ack (delete) ECR event {} from queue {}: {}",
        receipt_handle,
        queue_url,
        source
    ))]
    AckingMessage {
        receipt_handle: String,
        queue_url: String,
        source: RusotoError<DeleteMessageError>,
    },
}

type Result<T, E = SeedyError> = std::result::Result<T, E>;

fn parse_ecr_event(event_str: &str) -> serde_json::Map<String, serde_json::Value> {
    let parsed: serde_json::Value = serde_json::from_str(event_str).expect("event to be json");
    parsed.as_object().expect("event to be object").clone()
}

fn extract_event_image(body: &serde_json::Map<String, serde_json::Value>) -> Option<String> {
    let detail = body
        .get("detail")
        .expect("event to contain detail object")
        .as_object()
        .expect("a detail object");
    if detail.get("action-type")?.as_str() == Some("PUSH")
        && detail.get("result")?.as_str() == Some("SUCCESS")
    {
        let account = body.get("account").expect("").as_str()?;
        let region = body.get("region").expect("").as_str()?;
        let repository = detail.get("repository-name").expect("asdf").as_str()?;
        let tag = detail.get("image-tag").expect("").as_str()?;
        Some(format!(
            "{}.dkr.ecr.{}.amazonaws.com/{}:{}",
            account, region, repository, tag
        ))
    } else {
        None
    }
}

fn extract_service_image(service: &Service<String>) -> Option<String> {
    service
        .spec
        .labels
        .get(STACK_IMAGE_LABEL)
        .map(|image| image.to_owned())
        .or_else(|| {
            service
                .spec
                .task_template
                .container_spec
                .as_ref()
                .and_then(|spec| {
                    spec.image.clone().map(|mut image| {
                        let at_pos = image.find('@').unwrap_or(usize::max_value());
                        image.truncate(at_pos);
                        image
                    })
                })
        })
}

fn update_spec(service: &Service<String>) -> ServiceSpec<String> {
    let mut spec = service.spec.clone();
    spec.task_template.force_update = Some(service.version.index as isize);
    spec
}

fn process_one(
    message: &Message,
    services_by_image: &mut HashMap<String, Service<String>>,
    docker: &Docker,
    rt: &mut Runtime,
) -> Result<()> {
    debug!("Processing message {:?}", message);
    if let Some(event_str) = &message.body {
        let event = parse_ecr_event(event_str);
        if let Some(image) = extract_event_image(&event) {
            if let Some(service) = services_by_image.get(&image) {
                let updated_spec = update_spec(&service);
                let options = UpdateServiceOptions {
                    version: service.version.index,
                    ..Default::default()
                };
                rt.block_on(docker.update_service(&service.id, updated_spec, options, None))
                    .with_context(|| UpdatingService {
                        service_id: service.id.clone(),
                    })?;
                info!("Updated service {} with image {}", &service.id, &image);
            } else {
                debug!("No service matching image {}", &image);
            }
        } else {
            debug!("Skipping message {:?} because invalid type", &message.body);
        }
    } else {
        debug!("Encountered empty message {:?}", &message.body);
    }
    Ok(())
}

fn main() -> Result<()> {
    let opt = Opt::from_args();
    stderrlog::new()
        .module(module_path!())
        .quiet(opt.quiet)
        .verbosity(opt.verbose)
        .timestamp(stderrlog::Timestamp::Second)
        .init()
        .unwrap();

    let mut rt = Runtime::new().unwrap();
    let docker = Docker::connect_with_local_defaults().with_context(|| DockerInstantiation)?;
    let sqs = SqsClient::new(Region::default());
    warn!("Listening for ECR events on {}", &opt.queue_name);
    loop {
        let req = GetQueueUrlRequest {
            queue_name: opt.queue_name.clone(),
            ..Default::default()
        };
        let queue_url = sqs
            .get_queue_url(req)
            .sync()
            .with_context(|| SqsUrl {
                queue_name: opt.queue_name.clone(),
            })?
            .queue_url
            .unwrap();
        let request = ReceiveMessageRequest {
            queue_url: queue_url.clone(),
            wait_time_seconds: Some(20),
            ..Default::default()
        };
        let messages = sqs
            .receive_message(request)
            .sync()
            .with_context(|| PollingMessage {
                queue_url: queue_url.clone(),
            })?
            .messages;
        // TODO: Messages may be empty
        let services = rt
            .block_on(docker.list_services::<ListServicesOptions<String>, _>(None))
            .with_context(|| ServiceListing)?;
        let mut services_by_image: HashMap<String, Service<String>> = services
            .into_iter()
            .map(|service| (extract_service_image(&service).unwrap(), service))
            .collect();

        for message in messages.iter().flatten() {
            process_one(&message, &mut services_by_image, &docker, &mut rt)?;
            let receipt_handle = message.receipt_handle.as_ref().expect("No handle");
            let req = DeleteMessageRequest {
                queue_url: queue_url.clone(),
                receipt_handle: receipt_handle.clone(),
            };
            sqs.delete_message(req)
                .sync()
                .with_context(|| AckingMessage {
                    queue_url: queue_url.clone(),
                    receipt_handle,
                })?;
        }
    }
}
