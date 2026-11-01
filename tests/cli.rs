use std::{
    io::Write,
    process::{Command, Output, Stdio},
};

use serde_json::{Value, json};
use wiremock::{
    Mock, MockServer, ResponseTemplate,
    matchers::{body_json, body_partial_json, method, path, query_param},
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
async fn recipe_list_and_search_accept_query_flags() {
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
            &["recipe", command, "--query", "Soup", "--page-size", "10"],
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
    let output = cli(&server.uri(), &["recipe", "get", "7"], None).await;
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(output.stderr.is_empty());
    let recipe: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(recipe["id"], 7);
    let output = cli(
        &server.uri(),
        &["recipe", "get", "7", "--output", "text"],
        None,
    )
    .await;
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
        &["recipe", "create", "--file", "-"],
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
        &[
            "recipe",
            "create",
            "--name",
            "Soup",
            "--instruction",
            "Simmer.",
        ],
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
async fn recipe_update_accepts_an_id_and_stdin_patch() {
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
        &["recipe", "update", "7", "--file", "-"],
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
    let output = cli(
        &server.uri(),
        &["--output", "json", "recipe", "get", "999"],
        None,
    )
    .await;
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
    let invalid = cli(
        &server.uri(),
        &["recipe", "create", "--file", "-"],
        Some("not json"),
    )
    .await;
    assert_eq!(invalid.status.code(), Some(1));
    let delete = cli(&server.uri(), &["recipe", "delete", "7"], None).await;
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
    let output = cli(&server.uri(), &["recipe", "delete", "7", "--yes"], None).await;
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

#[tokio::test]
async fn resource_commands_create_get_patch_and_delete_the_correct_endpoints() {
    let server = MockServer::start().await;
    for (resource, endpoint, body, patch, label) in [
        (
            "food",
            "food",
            json!({"name":"Flour","plural_name":"Flours"}),
            json!({"plural_name":"Flour","supermarket_category":null}),
            "Flour",
        ),
        (
            "tag",
            "keyword",
            json!({"name":"Dinner","description":"Evening meals"}),
            json!({"description":"Quick dinners"}),
            "Dinner",
        ),
        (
            "ingredient",
            "ingredient",
            json!({"food":{"id":3,"name":"Flour"},"unit":{"id":1,"name":"g"},
                "amount":"125.50","note":"sifted"}),
            json!({"amount":"250.00","unit":null}),
            "125.50 g Flour sifted",
        ),
    ] {
        let mut response = body.clone();
        response["id"] = json!(7);
        response["server_field"] = json!("retained");
        Mock::given(method("POST"))
            .and(path(format!("/api/{endpoint}/")))
            .and(body_json(&body))
            .respond_with(ResponseTemplate::new(201).set_body_json(&response))
            .expect(1)
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path(format!("/api/{endpoint}/7/")))
            .respond_with(ResponseTemplate::new(200).set_body_json(&response))
            .expect(1)
            .mount(&server)
            .await;
        Mock::given(method("PATCH"))
            .and(path(format!("/api/{endpoint}/7/")))
            .and(body_json(&patch))
            .respond_with(ResponseTemplate::new(200).set_body_json(&response))
            .expect(1)
            .mount(&server)
            .await;
        Mock::given(method("DELETE"))
            .and(path(format!("/api/{endpoint}/7/")))
            .respond_with(ResponseTemplate::new(204))
            .expect(1)
            .mount(&server)
            .await;
        for (args, input) in [
            (vec![resource, "create", "--file", "-"], body.to_string()),
            (
                vec![resource, "update", "7", "--file", "-"],
                patch.to_string(),
            ),
        ] {
            let output = cli(&server.uri(), &args, Some(&input)).await;
            assert!(
                output.status.success(),
                "{resource}: {}",
                String::from_utf8_lossy(&output.stderr)
            );
            assert_eq!(
                serde_json::from_slice::<Value>(&output.stdout).unwrap(),
                response
            );
        }
        let output = cli(
            &server.uri(),
            &[resource, "get", "7", "--output", "text"],
            None,
        )
        .await;
        assert!(output.status.success());
        assert!(String::from_utf8(output.stdout).unwrap().contains(label));
        let output = cli(&server.uri(), &[resource, "delete", "7", "--yes"], None).await;
        assert!(output.status.success());
        assert_eq!(
            serde_json::from_slice::<Value>(&output.stdout).unwrap(),
            json!({"deleted":true,"id":7})
        );
        server.verify().await;
        server.reset().await;
    }
}

#[tokio::test]
async fn resource_lists_search_filter_and_follow_pagination() {
    let server = MockServer::start().await;
    for (resource, endpoint, filters, entry) in [
        (
            "food",
            "food",
            vec!["--query", "Flour & salt"],
            json!({"id":3,"name":"Flour"}),
        ),
        (
            "tag",
            "keyword",
            vec!["--query", "Dinner"],
            json!({"id":3,"name":"Dinner"}),
        ),
        (
            "ingredient",
            "ingredient",
            vec!["--food", "3", "--unit", "1"],
            json!({"id":3,"amount":"0.5","food":{"name":"Flour"},"unit":{"name":"g"}}),
        ),
    ] {
        let mut mock = Mock::given(method("GET"))
            .and(path(format!("/api/{endpoint}/")))
            .and(query_param("page", "2"))
            .and(query_param("page_size", "1"));
        for pair in filters.chunks_exact(2) {
            mock = mock.and(query_param(pair[0].trim_start_matches('-'), pair[1]));
        }
        mock.respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "count":3,"next":format!("/api/{endpoint}/?page=3"),"previous":"?page=1",
            "results":[entry]
        })))
        .expect(3)
        .mount(&server)
        .await;
        Mock::given(method("GET"))
            .and(path(format!("/api/{endpoint}/")))
            .and(query_param("page", "3"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "count":3,"next":null,"previous":"?page=2","results":[entry]
            })))
            .expect(1)
            .mount(&server)
            .await;
        let mut args = vec![resource, "list", "--page", "2", "--page-size", "1"];
        args.extend(filters);
        let output = cli(&server.uri(), &args, None).await;
        assert!(output.status.success());
        let page: Value = serde_json::from_slice(&output.stdout).unwrap();
        assert_eq!(page["count"], 3);
        assert_eq!(page["results"], json!([entry]));
        args[1] = "search";
        args.extend(["--output", "text"]);
        let output = cli(&server.uri(), &args, None).await;
        assert!(output.status.success());
        assert!(
            String::from_utf8(output.stdout)
                .unwrap()
                .contains("3 total; more pages: yes")
        );
        args.truncate(args.len() - 2);
        args.push("--all");
        let output = cli(&server.uri(), &args, None).await;
        assert!(output.status.success());
        assert_eq!(
            serde_json::from_slice::<Value>(&output.stdout).unwrap(),
            json!([entry, entry])
        );
        server.verify().await;
        server.reset().await;
    }
}

#[tokio::test]
async fn resource_invalid_input_and_unconfirmed_deletes_make_no_requests() {
    let server = MockServer::start().await;
    for resource in ["food", "ingredient", "tag"] {
        for input in ["not json", "[]", "null"] {
            let output = cli(
                &server.uri(),
                &[resource, "create", "--file", "-"],
                Some(input),
            )
            .await;
            assert_eq!(output.status.code(), Some(1));
            assert!(output.stdout.is_empty());
        }
        let output = cli(&server.uri(), &[resource, "delete", "7"], None).await;
        assert_eq!(output.status.code(), Some(2));
    }
    for resource in ["food", "tag"] {
        for input in [r#"{}"#, r#"{"name":"  "}"#, r#"{"name":null}"#] {
            let output = cli(
                &server.uri(),
                &[resource, "create", "--file", "-"],
                Some(input),
            )
            .await;
            assert_eq!(output.status.code(), Some(1));
        }
    }
    for args in [
        vec!["ingredient", "list", "--query", "Flour"],
        vec!["food", "list", "--food", "1"],
        vec!["tag", "list", "--unit", "1"],
    ] {
        let output = cli(&server.uri(), &args, None).await;
        assert_eq!(output.status.code(), Some(1));
    }
    assert!(server.received_requests().await.unwrap().is_empty());
}

#[tokio::test]
async fn food_merge_uses_native_endpoint_and_preserves_explicit_direction() {
    let server = MockServer::start().await;
    let response = json!({"msg":"Foods merged successfully", "server_field":"retained"});
    for (source, target) in [("7", "3"), ("3", "7")] {
        Mock::given(method("PUT"))
            .and(path(format!("/api/food/{source}/merge/{target}/")))
            .and(body_json(json!({})))
            .respond_with(ResponseTemplate::new(200).set_body_json(&response))
            .expect(2)
            .mount(&server)
            .await;
        let output = cli(
            &server.uri(),
            &["food", "merge", source, target, "--yes"],
            None,
        )
        .await;
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert!(output.stderr.is_empty());
        assert_eq!(
            serde_json::from_slice::<Value>(&output.stdout).unwrap(),
            response
        );
        let output = cli(
            &server.uri(),
            &[
                "foods", "merge", source, target, "--yes", "--output", "text",
            ],
            None,
        )
        .await;
        assert!(output.status.success());
        assert_eq!(
            String::from_utf8(output.stdout).unwrap(),
            format!("Merged food {source} into {target}.\n")
        );
    }
    assert_eq!(server.received_requests().await.unwrap().len(), 4);
}

#[tokio::test]
async fn invalid_and_unconfirmed_food_merges_make_no_requests() {
    let server = MockServer::start().await;
    for args in [
        vec!["food", "merge", "7", "3"],
        vec!["food", "merge", "7", "--yes"],
        vec!["food", "merge", "invalid", "3", "--yes"],
        vec!["ingredient", "merge", "7", "3", "--yes"],
        vec!["tag", "merge", "7", "3", "--yes"],
    ] {
        let output = cli(&server.uri(), &args, None).await;
        assert_eq!(output.status.code(), Some(2));
        assert!(output.stdout.is_empty());
    }
    let output = cli(&server.uri(), &["food", "merge", "7", "7", "--yes"], None).await;
    assert_eq!(output.status.code(), Some(1));
    assert!(output.stdout.is_empty());
    let error: Value = serde_json::from_slice(&output.stderr).unwrap();
    assert_eq!(error["error"]["kind"], "input");
    assert!(server.received_requests().await.unwrap().is_empty());
}

#[tokio::test]
async fn food_merge_reports_server_failure_without_success_output() {
    let server = MockServer::start().await;
    let details = json!({"error":true,"msg":"No food with id 999 exists"});
    Mock::given(method("PUT"))
        .and(path("/api/food/7/merge/999/"))
        .respond_with(ResponseTemplate::new(404).set_body_json(&details))
        .expect(1)
        .mount(&server)
        .await;
    let output = cli(&server.uri(), &["food", "merge", "7", "999", "--yes"], None).await;
    assert_eq!(output.status.code(), Some(1));
    assert!(output.stdout.is_empty());
    let error: Value = serde_json::from_slice(&output.stderr).unwrap();
    assert_eq!(error["error"]["kind"], "api");
    assert_eq!(error["error"]["status"], 404);
    assert_eq!(error["error"]["details"], details);
    assert_eq!(server.received_requests().await.unwrap().len(), 1);
}
