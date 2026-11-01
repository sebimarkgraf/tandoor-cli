use std::{
    io::Write,
    process::{Command, Output, Stdio},
};

use serde_json::{Value, json};
use wiremock::{
    Mock, MockServer, ResponseTemplate,
    matchers::{body_partial_json, method, path, query_param},
};

async fn cli(url: &str, args: &[&str], input: Option<&str>) -> Output {
    let url = url.to_owned();
    let args: Vec<String> = args.iter().map(|s| (*s).to_owned()).collect();
    let input = input.map(str::to_owned);
    tokio::task::spawn_blocking(move || {
        let mut child = Command::new(env!("CARGO_BIN_EXE_tandoor"))
            .env_remove("TANDOOR_URL")
            .env_remove("TANDOOR_TOKEN")
            .env_remove("TANDOOR_AUTH")
            .args(["--url", &url, "--token", "secret"])
            .args(args)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        if let Some(input) = input {
            child
                .stdin
                .take()
                .unwrap()
                .write_all(input.as_bytes())
                .unwrap();
        }
        child.wait_with_output().unwrap()
    })
    .await
    .unwrap()
}

#[tokio::test]
async fn list_and_search_accept_top_level_query_flags() {
    let server = MockServer::start().await;
    let page = json!({
        "count":1,"next":null,"previous":null,"results":[{"id":7,"name":"Soup"}]
    });
    Mock::given(method("GET"))
        .and(path("/api/recipe/"))
        .and(query_param("query", "Soup"))
        .and(query_param("page_size", "10"))
        .respond_with(ResponseTemplate::new(200).set_body_json(&page))
        .expect(2)
        .mount(&server)
        .await;
    for command in ["list", "search"] {
        let output = cli(
            &server.uri(),
            &[command, "--query", "Soup", "--page-size", "10"],
            None,
        )
        .await;
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        let page: Value = serde_json::from_slice(&output.stdout).unwrap();
        assert_eq!(page["count"], 1);
        assert_eq!(page["results"][0]["id"], 7);
        assert_eq!(page["results"][0]["name"], "Soup");
    }
}

#[tokio::test]
async fn piped_output_is_json_and_text_output_is_readable() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/api/recipe/7/"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "id":7,"name":"Soup","description":"A warm meal","servings":2,
            "steps":[{"name":"Cook","instruction":"Simmer.","ingredients":[{
                "amount":"0.5","food":{"id":3,"name":"Salt"},"unit":{"id":1,"name":"tsp"}
            }]}]
        })))
        .expect(2)
        .mount(&server)
        .await;
    let output = cli(&server.uri(), &["get", "7"], None).await;
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(output.stderr.is_empty());
    let recipe: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(recipe["id"], 7);
    let output = cli(&server.uri(), &["get", "7", "--output", "text"], None).await;
    assert!(output.status.success());
    let text = String::from_utf8(output.stdout).unwrap();
    assert!(text.contains("Soup (#7)"));
    assert!(text.contains("0.5 tsp Salt"));
    assert!(text.contains("Simmer."));
}

#[tokio::test]
async fn create_accepts_stdin_and_simple_flags() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/api/recipe/"))
        .and(body_partial_json(
            json!({"name":"Soup","internal":true,"steps":[{
                "instruction":"Simmer.","ingredients":[],"order":0
            }]}),
        ))
        .respond_with(
            ResponseTemplate::new(201).set_body_json(json!({"id":7,"name":"Soup","steps":[]})),
        )
        .expect(2)
        .mount(&server)
        .await;
    let output = cli(
        &server.uri(),
        &["create", "--file", "-"],
        Some(r#"{"name":"Soup","steps":[{"instruction":"Simmer."}]}"#),
    )
    .await;
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let output = cli(
        &server.uri(),
        &["create", "--name", "Soup", "--instruction", "Simmer."],
        None,
    )
    .await;
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
}

#[tokio::test]
async fn update_accepts_a_top_level_id_and_stdin_patch() {
    let server = MockServer::start().await;
    Mock::given(method("PATCH"))
        .and(path("/api/recipe/7/"))
        .and(body_partial_json(json!({"servings":4})))
        .respond_with(
            ResponseTemplate::new(200).set_body_json(json!({"id":7,"name":"Soup","servings":4})),
        )
        .expect(1)
        .mount(&server)
        .await;
    let output = cli(
        &server.uri(),
        &["update", "7", "--file", "-"],
        Some(r#"{"servings":4}"#),
    )
    .await;
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let recipe: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(recipe["servings"], 4);
}

#[tokio::test]
async fn runtime_errors_are_json_on_stderr_and_stdout_is_empty() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/api/recipe/999/"))
        .respond_with(ResponseTemplate::new(404).set_body_json(json!({"detail":"Not found."})))
        .expect(1)
        .mount(&server)
        .await;
    let output = cli(&server.uri(), &["--output", "json", "get", "999"], None).await;
    assert_eq!(output.status.code(), Some(1));
    assert!(output.stdout.is_empty());
    let error: Value = serde_json::from_slice(&output.stderr).unwrap();
    assert_eq!(error["error"]["kind"], "api");
    assert_eq!(error["error"]["status"], 404);
    assert_eq!(error["error"]["details"]["detail"], "Not found.");
}

#[tokio::test]
async fn invalid_json_and_unconfirmed_deletes_do_not_make_requests() {
    let server = MockServer::start().await;
    let invalid = cli(&server.uri(), &["create", "--file", "-"], Some("not json")).await;
    assert_eq!(invalid.status.code(), Some(1));
    let delete = cli(&server.uri(), &["delete", "7"], None).await;
    assert_eq!(delete.status.code(), Some(2));
    let api = cli(
        &server.uri(),
        &["api", "recipe/7/", "--method", "DELETE"],
        None,
    )
    .await;
    assert_eq!(api.status.code(), Some(1));
    assert!(server.received_requests().await.unwrap().is_empty());
}

#[tokio::test]
async fn explicit_delete_succeeds_with_machine_readable_confirmation() {
    let server = MockServer::start().await;
    Mock::given(method("DELETE"))
        .and(path("/api/recipe/7/"))
        .respond_with(ResponseTemplate::new(204))
        .expect(1)
        .mount(&server)
        .await;
    let output = cli(&server.uri(), &["delete", "7", "--yes"], None).await;
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(
        serde_json::from_slice::<Value>(&output.stdout).unwrap(),
        json!({"deleted":true,"id":7})
    );
}

#[test]
fn help_does_not_require_credentials_or_expose_environment_token() {
    let output = Command::new(env!("CARGO_BIN_EXE_tandoor"))
        .env_remove("TANDOOR_URL")
        .env("TANDOOR_TOKEN", "do-not-display")
        .arg("--help")
        .output()
        .unwrap();
    assert!(output.status.success());
    let help = String::from_utf8(output.stdout).unwrap();
    assert!(help.contains("TANDOOR_TOKEN"));
    assert!(!help.contains("do-not-display"));
}
