use base64;
use bollard::errors::Error as BollardError;
use bollard::service::{ListServicesOptions, Service, ServiceSpec, UpdateServiceOptions};
use bollard::{auth::DockerCredentials, Docker};
use log::{debug, info, warn};
use rusoto_core::Region;
use rusoto_core::RusotoError;
use rusoto_ecr::{Ecr, EcrClient, GetAuthorizationTokenError, GetAuthorizationTokenRequest};
use rusoto_sqs::{
    DeleteMessageError, DeleteMessageRequest, GetQueueUrlError, GetQueueUrlRequest, Message,
    ReceiveMessageError, ReceiveMessageRequest, Sqs, SqsClient,
};
use snafu::{ResultExt, Snafu};
use std::collections::HashMap;
use std::str::FromStr;
use stderrlog;
use structopt::StructOpt;
use tokio::runtime::Runtime;

mod events;
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
    #[snafu(display(
        "Could not retrieve authentication token for accounts {:?}: {}",
        registry_ids,
        source
    ))]
    AuthToken {
        registry_ids: Vec<String>,
        source: RusotoError<GetAuthorizationTokenError>,
    },
}

type Result<T, E = SeedyError> = std::result::Result<T, E>;

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

fn docker_credentials_from_auth_token(auth_token: String) -> DockerCredentials {
    let decoded = String::from_utf8(
        base64::decode(&auth_token)
            .unwrap_or_else(|_| panic!("Failed base64 decode from ECR: {}", &auth_token)),
    )
    .unwrap_or_else(|_| panic!("Failed base64 decode from ECR: {}", &auth_token));
    let parts: Vec<&str> = decoded.splitn(2, ':').collect();
    DockerCredentials {
        username: Some(parts[0].to_owned()),
        password: Some(parts[1].to_owned()),
        ..Default::default()
    }
}

fn ecr_auth_for_event(ecr: &EcrClient, event: &events::Event) -> Result<Option<DockerCredentials>> {
    let req = GetAuthorizationTokenRequest {
        registry_ids: Some(vec![event.account_id.clone()]),
    };
    let auth_token = ecr
        .get_authorization_token(req)
        .sync()
        .with_context(|| AuthToken {
            registry_ids: vec![event.account_id.clone()],
        })?
        .authorization_data
        .and_then(|mut auths| {
            auths
                .get_mut(0)
                .map(|auth| auth.authorization_token.take().unwrap())
                .map(docker_credentials_from_auth_token)
        });
    Ok(auth_token)
}

fn update_spec(service: &Service<String>, event: &events::Event) -> ServiceSpec<String> {
    let mut spec = service.spec.clone();
    spec.task_template.force_update = Some(service.version.index as isize);
    spec.task_template
        .container_spec
        .as_mut()
        .and_then(|mut spec| {
            spec.image = Some(format!("{}@{}", event.image(), event.image_digest));
            Some(spec)
        });
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
        if let Some(event) = events::parse_ecr_event(event_str) {
            if let Some(service) = services_by_image.get(&event.image()) {
                let event_region = Region::from_str(&event.region).unwrap();
                let ecr = EcrClient::new(event_region);
                let auth_token = ecr_auth_for_event(&ecr, &event)?;
                let updated_spec = update_spec(&service, &event);
                let options = UpdateServiceOptions {
                    version: service.version.index,
                    ..Default::default()
                };
                rt.block_on(docker.update_service(&service.id, updated_spec, options, auth_token))
                    .with_context(|| UpdatingService {
                        service_id: service.id.clone(),
                    })?;
                info!(
                    "Updated service {} with image {}",
                    &service.id,
                    &event.image()
                );
            } else {
                debug!("No service matching image {}", &event.image());
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
