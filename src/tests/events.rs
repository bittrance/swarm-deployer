use serde_json::json;

fn message_event() -> String {
    json!({
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
    .to_string()
}

#[test]
fn test_parse_ecr_event() {
    let event = crate::events::parse_ecr_event(&message_event()).unwrap();
    assert_eq!(event.account_id, "123456789012");
    assert_eq!(event.repository_name, "bittrance/ze-image");
    assert_eq!(event.image_digest, "sha256:1234");
    assert_eq!(event.image_tag, "latest");
}

#[test]
fn test_extract_event_image() {
    let event = crate::events::parse_ecr_event(&message_event()).unwrap();
    assert_eq!(
        "123456789012.dkr.ecr.rp-north-1.amazonaws.com/bittrance/ze-image:latest",
        event.image()
    );
}
