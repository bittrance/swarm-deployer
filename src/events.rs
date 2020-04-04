use serde_json;

pub struct Event {
    pub account_id: String,
    pub region: String,
    pub repository_name: String,
    pub image_digest: String,
    pub image_tag: String,
}

impl Event {
    pub fn image(&self) -> String {
        format!(
            "{}.dkr.ecr.{}.amazonaws.com/{}:{}",
            self.account_id, self.region, self.repository_name, self.image_tag
        )
    }
}

fn extract_string_value(
    object: &serde_json::Map<String, serde_json::Value>,
    field: &str,
) -> String {
    object
        .get(field)
        .unwrap_or_else(|| panic!("event to contain {}", field))
        .as_str()
        .unwrap_or_else(|| panic!("{} to be a string", field))
        .to_owned()
}

pub fn parse_ecr_event(event_str: &str) -> Option<Event> {
    let parsed: serde_json::Map<String, serde_json::Value> =
        serde_json::from_str(event_str).expect("event to be json");

    let detail = parsed
        .get("detail")
        .expect("event to contain detail object")
        .as_object()
        .expect("a detail object");
    if detail.get("action-type")?.as_str() == Some("PUSH")
        && detail.get("result")?.as_str() == Some("SUCCESS")
    {
        let account_id = extract_string_value(&parsed, "account");
        let region = extract_string_value(&parsed, "region");
        let repository_name = extract_string_value(detail, "repository-name");
        let image_digest = extract_string_value(detail, "image-digest");
        let image_tag = extract_string_value(detail, "image-tag");

        Some(Event {
            account_id,
            region,
            repository_name,
            image_digest,
            image_tag,
        })
    } else {
        None
    }
}
