//! Regression tests that parse mm-openapi.yaml and verify the endpoints and schemas
//! that tldr depends on still exist. When the spec is updated, failing tests here
//! indicate that tldr may need updating.

use serde_yaml::Value;
use std::fs;

fn load_spec() -> Value {
    let contents = fs::read_to_string("mm-openapi.yaml").expect("failed to read mm-openapi.yaml");
    serde_yaml::from_str(&contents).expect("failed to parse mm-openapi.yaml")
}

/// Helper: check that a path+method exists in the spec
fn assert_endpoint_exists(spec: &Value, path: &str, method: &str) {
    let paths = spec.get("paths").expect("spec missing 'paths'");
    let endpoint = paths.get(path);
    assert!(
        endpoint.is_some(),
        "endpoint {path} not found in OpenAPI spec"
    );
    let endpoint = endpoint.unwrap();
    let operation = endpoint.get(method);
    assert!(
        operation.is_some(),
        "method {method} not found for endpoint {path}"
    );
}

/// Helper: check that a schema exists and has expected fields
fn assert_schema_has_fields(spec: &Value, schema_name: &str, fields: &[&str]) {
    let schemas = spec
        .get("components")
        .and_then(|c| c.get("schemas"))
        .expect("spec missing components/schemas");
    let schema = schemas.get(schema_name);
    assert!(
        schema.is_some(),
        "schema '{schema_name}' not found in OpenAPI spec"
    );
    let properties = schema
        .unwrap()
        .get("properties")
        .unwrap_or_else(|| panic!("schema '{schema_name}' has no properties"));
    for field in fields {
        assert!(
            properties.get(*field).is_some(),
            "schema '{schema_name}' missing field '{field}'"
        );
    }
}

#[test]
fn test_get_current_user_endpoint_exists() {
    // /api/v4/users/me is handled by /api/v4/users/{user_id} where "me" is a convention
    let spec = load_spec();
    assert_endpoint_exists(&spec, "/api/v4/users/{user_id}", "get");
}

#[test]
fn test_get_teams_for_user_endpoint_exists() {
    let spec = load_spec();
    assert_endpoint_exists(&spec, "/api/v4/users/{user_id}/teams", "get");
}

#[test]
fn test_get_channels_for_team_for_user_endpoint_exists() {
    let spec = load_spec();
    assert_endpoint_exists(
        &spec,
        "/api/v4/users/{user_id}/teams/{team_id}/channels",
        "get",
    );
}

#[test]
fn test_get_channel_unread_endpoint_exists() {
    let spec = load_spec();
    assert_endpoint_exists(
        &spec,
        "/api/v4/users/{user_id}/channels/{channel_id}/unread",
        "get",
    );
}

#[test]
fn test_get_posts_around_last_unread_endpoint_exists() {
    let spec = load_spec();
    assert_endpoint_exists(
        &spec,
        "/api/v4/users/{user_id}/channels/{channel_id}/posts/unread",
        "get",
    );
}

#[test]
fn test_get_posts_for_channel_endpoint_exists() {
    let spec = load_spec();
    assert_endpoint_exists(&spec, "/api/v4/channels/{channel_id}/posts", "get");
}

#[test]
fn test_get_user_endpoint_exists() {
    let spec = load_spec();
    assert_endpoint_exists(&spec, "/api/v4/users/{user_id}", "get");
}

#[test]
fn test_create_post_endpoint_exists() {
    let spec = load_spec();
    assert_endpoint_exists(&spec, "/api/v4/posts", "post");
}

#[test]
fn test_get_team_unreads_for_user_endpoint_exists() {
    let spec = load_spec();
    assert_endpoint_exists(&spec, "/api/v4/users/{user_id}/teams/unread", "get");
}

// --- Schema field assertions ---

#[test]
fn test_post_schema_has_required_fields() {
    let spec = load_spec();
    assert_schema_has_fields(
        &spec,
        "Post",
        &["id", "create_at", "user_id", "channel_id", "message"],
    );
}

#[test]
fn test_post_list_schema_has_required_fields() {
    let spec = load_spec();
    assert_schema_has_fields(&spec, "PostList", &["order", "posts"]);
}

#[test]
fn test_channel_unread_schema_has_required_fields() {
    let spec = load_spec();
    assert_schema_has_fields(&spec, "ChannelUnread", &["msg_count", "mention_count"]);
}

#[test]
fn test_user_schema_has_required_fields() {
    let spec = load_spec();
    assert_schema_has_fields(&spec, "User", &["id", "username", "email"]);
}

#[test]
fn test_team_schema_has_required_fields() {
    let spec = load_spec();
    assert_schema_has_fields(&spec, "Team", &["id", "name", "display_name"]);
}

#[test]
fn test_channel_schema_has_expected_fields() {
    let spec = load_spec();
    // Channel is per MM spec - verify key fields we use
    let schemas = spec
        .get("components")
        .and_then(|c| c.get("schemas"))
        .expect("spec missing components/schemas");
    let channel = schemas.get("Channel").expect("Channel schema not found");
    let props = channel
        .get("properties")
        .expect("Channel schema has no properties");

    for field in &["id", "team_id", "type", "name", "display_name"] {
        assert!(
            props.get(*field).is_some(),
            "Channel schema missing field '{field}'"
        );
    }
}

#[test]
fn test_team_unread_schema_has_required_fields() {
    let spec = load_spec();
    assert_schema_has_fields(
        &spec,
        "TeamUnread",
        &["team_id", "msg_count", "mention_count"],
    );
}

// --- Query parameter assertions ---

#[test]
fn test_get_posts_for_channel_has_since_param() {
    let spec = load_spec();
    let paths = spec.get("paths").unwrap();
    let endpoint = paths
        .get("/api/v4/channels/{channel_id}/posts")
        .unwrap()
        .get("get")
        .unwrap();
    let parameters = endpoint.get("parameters").unwrap().as_sequence().unwrap();

    let has_since = parameters.iter().any(|p| {
        p.get("name")
            .and_then(|n| n.as_str())
            .map(|n| n == "since")
            .unwrap_or(false)
    });

    assert!(
        has_since,
        "GET /api/v4/channels/{{channel_id}}/posts missing 'since' query parameter"
    );
}

#[test]
fn test_get_posts_around_unread_has_limit_params() {
    let spec = load_spec();
    let paths = spec.get("paths").unwrap();
    let endpoint = paths
        .get("/api/v4/users/{user_id}/channels/{channel_id}/posts/unread")
        .unwrap()
        .get("get")
        .unwrap();
    let parameters = endpoint.get("parameters").unwrap().as_sequence().unwrap();

    let param_names: Vec<&str> = parameters
        .iter()
        .filter_map(|p| p.get("name").and_then(|n| n.as_str()))
        .collect();

    assert!(
        param_names.contains(&"limit_before"),
        "missing 'limit_before' param"
    );
    assert!(
        param_names.contains(&"limit_after"),
        "missing 'limit_after' param"
    );
}
