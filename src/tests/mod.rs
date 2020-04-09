use base64;
use bollard::service::{
    ObjectVersion, Service, ServiceEndpoint, ServiceSpec, TaskSpec, TaskSpecContainerSpec,
};
use chrono::{TimeZone, Utc};
use std::collections::HashMap;
use structopt::StructOpt;

#[cfg(test)]
mod events;

fn message_event() -> crate::events::Event {
    crate::events::Event {
        account_id: String::from("123456789012"),
        region: String::from("rp-north-1"),
        repository_name: String::from("bittrance/ze-image"),
        image_tag: String::from("latest"),
        image_digest: String::from("sha256:1234"),
    }
}

fn service_spec(
    service_labels: Option<HashMap<String, String>>,
    image: Option<String>,
) -> Service<String> {
    Service {
        id: "foo".to_owned(),
        version: ObjectVersion { index: 1 },
        created_at: Utc.ymd(1970, 1, 1).and_hms_milli(0, 0, 1, 0),
        updated_at: Utc.ymd(1970, 1, 1).and_hms_milli(0, 0, 1, 0),
        spec: ServiceSpec {
            name: "ze-service".to_owned(),
            labels: service_labels.unwrap_or(HashMap::new()),
            task_template: TaskSpec {
                container_spec: Some(TaskSpecContainerSpec {
                    image: image,
                    ..Default::default()
                }),
                ..Default::default()
            },
            ..Default::default()
        },
        endpoint: ServiceEndpoint {
            ..Default::default()
        },
        update_status: None,
    }
}

fn stack_label(image: &str) -> Option<HashMap<String, String>> {
    let mut service_labels = HashMap::new();
    service_labels.insert(crate::STACK_IMAGE_LABEL.to_owned(), image.to_owned());
    Some(service_labels)
}

fn filter_label(key: &str, value: &str) -> Option<HashMap<String, String>> {
    let mut service_labels = HashMap::new();
    service_labels.insert(key.to_owned(), value.to_owned());
    Some(service_labels)
}

#[test]
fn test_extract_service_image_from_container_spec_without_sha() {
    let service = service_spec(None, Some("bittrance/ze-image:latest".to_owned()));
    let image = crate::extract_service_image(&service);
    assert_eq!(Some("bittrance/ze-image:latest".to_owned()), image);
}

#[test]
fn test_extract_service_image_from_container_spec_with_sha() {
    let service = service_spec(
        None,
        Some("bittrance/ze-image:latest@sha512:12341243".to_owned()),
    );
    let image = crate::extract_service_image(&service);
    assert_eq!(Some("bittrance/ze-image:latest".to_owned()), image);
}

#[test]
fn test_extract_service_image_from_container_spec_with_label() {
    let service = service_spec(stack_label("bittrance/ze-image:latest"), None);
    let image = crate::extract_service_image(&service);
    assert_eq!(Some("bittrance/ze-image:latest".to_owned()), image);
}

#[test]
fn test_extract_service_image_from_container_spec_with_label_with_sha() {
    let service = service_spec(stack_label("bittrance/ze-image:latest@sha512:1234"), None);
    let image = crate::extract_service_image(&service);
    assert_ne!(Some("bittrance/ze-image:latest".to_owned()), image);
}

#[test]
fn test_extract_service_image_from_container_with_nothing() {
    let service = service_spec(None, None);
    let image = crate::extract_service_image(&service);
    assert_eq!(None, image);
}

#[test]
fn test_docker_credentials_from_auth_token() {
    let encoded = base64::encode("foo:bar");
    let credentials = crate::docker_credentials_from_auth_token(encoded);
    assert_eq!(Some("foo".to_owned()), credentials.username);
    assert_eq!(Some("bar".to_owned()), credentials.password);
}

#[test]
fn test_update_spec_adds_digest() {
    let service = service_spec(
        None,
        Some(
            "123456789012.dkr.ecr.rp-north-1.amazonaws.com/bittrance/ze-image:latest@sha512:5678"
                .to_owned(),
        ),
    );
    let updated_spec = crate::update_spec(&service, &message_event());
    assert_eq!(
        Some(
            "123456789012.dkr.ecr.rp-north-1.amazonaws.com/bittrance/ze-image:latest@sha256:1234"
                .to_owned()
        ),
        updated_spec
            .task_template
            .container_spec
            .and_then(|spec| spec.image)
    );
}

#[test]
fn test_build_service_index() {
    let service = service_spec(None, Some("bittrance/ze-image:latest".to_owned()));
    let opt = crate::Opt::from_iter(vec!["ze-bin", "--queue", "some-queue"].iter());
    let index = crate::build_service_index(vec![service], &opt);
    assert_eq!(1, index.len());
}

#[test]
fn test_build_service_index_with_label_filter_includes() {
    let service = service_spec(
        filter_label("some", "label"),
        Some("bittrance/ze-image:latest".to_owned()),
    );
    let opt = crate::Opt::from_iter(
        vec![
            "ze-bin",
            "--queue",
            "some-queue",
            "--filter-label",
            "some=label",
        ]
        .iter(),
    );
    let index = crate::build_service_index(vec![service], &opt);
    assert_eq!(1, index.len());
}

#[test]
fn test_build_service_index_with_label_filter_excludes() {
    let service = service_spec(
        filter_label("some", "label"),
        Some("bittrance/ze-image:latest".to_owned()),
    );
    let opt = crate::Opt::from_iter(
        vec![
            "ze-bin",
            "--queue",
            "some-queue",
            "--filter-label",
            "some=other",
        ]
        .iter(),
    );
    let index = crate::build_service_index(vec![service], &opt);
    assert_eq!(0, index.len());
}
