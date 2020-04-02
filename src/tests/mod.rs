use bollard::service::{
    ObjectVersion, Service, ServiceEndpoint, ServiceSpec, TaskSpec, TaskSpecContainerSpec,
};
use chrono::{TimeZone, Utc};
use serde_json::json;
use std::collections::HashMap;

fn message_event() -> serde_json::Map<String, serde_json::Value> {
    let body = json!({
        "version": "0",
        "id": "9baf3833-b73f-1107-0234-3206ab430914",
        "detail-type": "ECR Image Action",
        "source": "aws.ecr",
        "account": "123456789012",
        "time": "2020-03-30T09:56:58Z",
        "region": "rp-north-1",
        "resources":[],
        "detail":{
            "action-type": "PUSH",
            "result": "SUCCESS",
            "repository-name": "bittrance/ze-image",
            "image-digest": "sha256:1234",
            "image-tag": "latest"
        }
    })
    .to_string();
    crate::parse_ecr_event(&body)
}

fn service_spec(label: Option<String>, image: Option<String>) -> Service<String> {
    let mut service_labels = HashMap::new();
    if let Some(l) = label {
        service_labels.insert(crate::STACK_IMAGE_LABEL.to_owned(), l);
    }
    Service {
        id: "foo".to_owned(),
        version: ObjectVersion { index: 1 },
        created_at: Utc.ymd(1970, 1, 1).and_hms_milli(0, 0, 1, 0),
        updated_at: Utc.ymd(1970, 1, 1).and_hms_milli(0, 0, 1, 0),
        spec: ServiceSpec {
            name: "ze-service".to_owned(),
            labels: service_labels,
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

#[test]
fn test_extract_event_image() {
    let event = message_event();
    let image = crate::extract_event_image(&event);
    assert_eq!(
        Some("123456789012.dkr.ecr.rp-north-1.amazonaws.com/bittrance/ze-image:latest".to_owned()),
        image
    );
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
    let service = service_spec(Some("bittrance/ze-image:latest".to_owned()), None);
    let image = crate::extract_service_image(&service);
    assert_eq!(Some("bittrance/ze-image:latest".to_owned()), image);
}

#[test]
fn test_extract_service_image_from_container_spec_with_label_with_sha() {
    let service = service_spec(
        Some("bittrance/ze-image:latest@sha512:1234".to_owned()),
        None,
    );
    let image = crate::extract_service_image(&service);
    assert_ne!(Some("bittrance/ze-image:latest".to_owned()), image);
}

#[test]
fn test_extract_service_image_from_container_with_nothing() {
    let service = service_spec(None, None);
    let image = crate::extract_service_image(&service);
    assert_eq!(None, image);
}
