use base64;
use bollard::errors::Error as BollardError;
use bollard::service::{ListServicesOptions, Service, ServiceSpec, UpdateServiceOptions};
use bollard::{auth::DockerCredentials, Docker};
use futures::future::FutureExt;
use log::{debug, info, warn};
use rusoto_core::Region;
use rusoto_core::RusotoError;
use rusoto_ecr::{Ecr, EcrClient, GetAuthorizationTokenError, GetAuthorizationTokenRequest};
use rusoto_sqs::{DeleteMessageError, GetQueueUrlError, Message, ReceiveMessageError, SqsClient};
use snafu::{ensure, ResultExt, Snafu};
use std::collections::HashMap;
use std::str::FromStr;
use stderrlog;
use structopt::StructOpt;

mod events;
mod sqs;
#[cfg(test)]
mod tests;

const STACK_IMAGE_LABEL: &str = "com.docker.stack.image";

#[derive(StructOpt, Debug)]
#[structopt()]
pub struct Opt {
    /// Update only labelled services (default is to consider all services)
    #[structopt(long = "filter-label", parse(try_from_str = split_label))]
    filter_label: Option<(String, String)>,
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
pub enum SeedyError {
    #[snafu(display("Filter label {} expected to be on format key=value", label))]
    LabelFilterError { label: String },
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

fn split_label(input: &str) -> Result<(String, String)> {
    let parts: Vec<&str> = input.splitn(2, '=').collect();
    ensure!(
        parts.len() == 2,
        LabelFilterError {
            label: input.to_owned()
        }
    );
    Ok((parts[0].to_owned(), parts[1].to_owned()))
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

async fn ecr_auth_for_event(
    ecr: &EcrClient,
    event: &events::Event,
) -> Result<Option<DockerCredentials>> {
    let req = GetAuthorizationTokenRequest {
        registry_ids: Some(vec![event.account_id.clone()]),
    };
    ecr.get_authorization_token(req)
        .map(|res| {
            res.map(|res| {
                res.authorization_data.and_then(|mut auths| {
                    auths
                        .get_mut(0)
                        .map(|auth| auth.authorization_token.take().unwrap())
                        .map(docker_credentials_from_auth_token)
                })
            })
            .with_context(|| AuthToken {
                registry_ids: vec![event.account_id.clone()],
            })
        })
        .await
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

async fn process_one(
    message: &Message,
    services_by_image: &HashMap<String, Service<String>>,
    docker: &Docker,
) -> Result<()> {
    debug!("Processing message {:?}", message);
    if let Some(event_str) = &message.body {
        if let Some(event) = events::parse_ecr_event(event_str) {
            if let Some(service) = services_by_image.get(&event.image()) {
                let event_region = Region::from_str(&event.region).unwrap();
                let ecr = EcrClient::new(event_region);
                let auth_token = ecr_auth_for_event(&ecr, &event).await?;
                let updated_spec = update_spec(&service, &event);
                let options = UpdateServiceOptions {
                    version: service.version.index,
                    ..Default::default()
                };
                docker
                    .update_service(&service.id, updated_spec, options, auth_token)
                    .map(|res| {
                        res.with_context(|| UpdatingService {
                            service_id: service.id.clone(),
                        })
                    })
                    .await?;
                info!(
                    "Updated service {} with image {}, {}",
                    &service.id,
                    &event.image(),
                    &event.image_digest
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

async fn candidate_services(docker: &Docker) -> Result<Vec<Service<String>>> {
    docker
        .list_services::<ListServicesOptions<String>, _>(None)
        .map(|res| res.with_context(|| ServiceListing))
        .await
}

fn build_service_index(
    services: Vec<Service<String>>,
    opt: &Opt,
) -> HashMap<String, Service<String>> {
    services
        .into_iter()
        .filter(|service| match &opt.filter_label {
            Some((key, value)) => service
                .spec
                .labels
                .get(key)
                .filter(|v| *v == value)
                .is_some(),
            None => true,
        })
        .map(|service| (extract_service_image(&service).unwrap(), service))
        .collect()
}

#[tokio::main]
async fn main() -> Result<()> {
    let opt = Opt::from_args();
    stderrlog::new()
        .module(module_path!())
        .quiet(opt.quiet)
        .verbosity(opt.verbose)
        .timestamp(stderrlog::Timestamp::Second)
        .init()
        .unwrap();

    let docker = Docker::connect_with_local_defaults().with_context(|| DockerInstantiation)?;
    let sqs = SqsClient::new(Region::default());
    warn!("Listening for ECR events on {}", &opt.queue_name);
    loop {
        let messages = sqs::poll_messages(&sqs, &opt).await?;
        // TODO: Messages may be empty
        let services = candidate_services(&docker).await?;
        let services_by_image = build_service_index(services, &opt);
        for message in messages.iter() {
            process_one(message, &services_by_image, &docker).await?;
            sqs::delete_message(&sqs, &message, &opt).await?;
        }
    }
}
