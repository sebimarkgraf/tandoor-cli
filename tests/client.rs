use std::time::Duration;

use serde_json::{Value, json};
use tandoor::{Auth, Client, Error, Method, NewRecipe, Recipe, RecipePatch, RecipeQuery, Step};
use wiremock::{
    Mock, MockServer, ResponseTemplate,
    matchers::{body_json, header, method, path, query_param},
};

fn client(server: &MockServer) -> Client {
    Client::new(&server.uri(), Auth::Bearer("test-secret".into())).unwrap()
}

#[tokio::test]
async fn authentication_modes_send_the_correct_header() {
    let server = MockServer::start().await;
    for (auth, name, value) in [
        (
            Auth::Bearer("test-secret".into()),
            "authorization",
            "Bearer test-secret",
        ),
        (
            Auth::Session("test-secret".into()),
            "x-session-token",
            "test-secret",
        ),
        (
            Auth::LegacyToken("test-secret".into()),
            "authorization",
            "Token test-secret",
        ),
    ] {
        assert!(!format!("{auth:?}").contains("test-secret"));
        let client = Client::new(&server.uri(), auth).unwrap();
        assert!(!format!("{client:?}").contains("test-secret"));
        Mock::given(method("GET"))
            .and(path("/api/recipe/7/"))
            .and(header(name, value))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({"id":7,"name":"Soup"})))
            .expect(1)
            .mount(&server)
            .await;
        assert_eq!(client.get_recipe(7).await.unwrap().name, "Soup");
        server.verify().await;
        server.reset().await;
    }
}

#[test]
fn base_urls_preserve_subpaths_and_normalize_api_roots() {
    for (base, expected) in [
        ("https://example.com", "https://example.com/api/"),
        ("https://example.com/api/", "https://example.com/api/"),
        (
            "https://example.com/recipes",
            "https://example.com/recipes/api/",
        ),
        (
            "https://example.com/recipes/api",
            "https://example.com/recipes/api/",
        ),
    ] {
        assert_eq!(
            Client::new(base, Auth::Bearer("secret".into()))
                .unwrap()
                .api_url()
                .as_str(),
            expected
        );
    }
    for base in [
        "no-url",
        "file:///tmp/recipes",
        "https://user:secret@example.com",
        "https://example.com?x=y",
        "https://example.com#fragment",
    ] {
        assert!(matches!(
            Client::new(base, Auth::Bearer("secret".into())),
            Err(Error::Config(_))
        ));
    }
    for token in ["", "  ", "secret\nvalue"] {
        assert!(Client::new("https://example.com", Auth::Bearer(token.into())).is_err());
    }
    assert!(
        Client::with_timeout(
            "https://example.com",
            Auth::Bearer("secret".into()),
            Duration::ZERO
        )
        .is_err()
    );
}

#[tokio::test]
async fn search_encodes_query_and_collects_pages() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/api/recipe/"))
        .and(query_param("query", "pasta & cheese"))
        .and(query_param("page_size", "1"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "count":2, "next":format!("{}/api/recipe/?page=2", server.uri()), "previous":null,
            "results":[{"id":1,"name":"Pasta","keywords":[{"id":3,"label":"Dinner"}],"rating":4.5}]
        })))
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/api/recipe/"))
        .and(query_param("page", "2"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "count":2, "next":null,"previous":"?page=1", "results":[{"id":2,"name":"Cheese"}]
        })))
        .expect(1)
        .mount(&server)
        .await;
    let recipes = client(&server)
        .list_all_recipes(&RecipeQuery {
            query: Some("pasta & cheese".into()),
            page_size: Some(1),
            ..Default::default()
        })
        .await
        .unwrap();
    assert_eq!(recipes.len(), 2);
    assert_eq!(recipes[0].keywords[0].label.as_deref(), Some("Dinner"));
    assert_eq!(recipes[0].extra["rating"], 4.5);
    assert!(recipes[0].steps.is_none());
}

#[tokio::test]
async fn unsafe_endpoints_and_pagination_never_receive_credentials() {
    let server = MockServer::start().await;
    let other = MockServer::start().await;
    let client = client(&server);
    for endpoint in [
        format!("{}/api/recipe/", other.uri()),
        "../settings/".into(),
        "%2e%2e/settings/".into(),
        "/api/recipe/".into(),
        "recipe/?query=x".into(),
    ] {
        assert!(
            client
                .request::<Value>(Method::GET, &endpoint, &[], None)
                .await
                .is_err()
        );
    }
    let page = tandoor::Page::<Recipe> {
        count: 0,
        next: Some(format!("{}/api/recipe/", other.uri())),
        previous: None,
        results: vec![],
    };
    assert!(client.next_page(&page).await.is_err());
    let page = tandoor::Page::<Recipe> {
        next: Some("/settings/".into()),
        ..page
    };
    assert!(client.next_page(&page).await.is_err());
    assert!(server.received_requests().await.unwrap().is_empty());
    assert!(other.received_requests().await.unwrap().is_empty());
}

#[tokio::test]
async fn pagination_cycles_fail_instead_of_looping_forever() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/api/recipe/"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "count":2,"next":"/api/recipe/?page=2","previous":null,"results":[]
        })))
        .expect(2)
        .mount(&server)
        .await;
    assert!(matches!(
        client(&server)
            .list_all_recipes(&RecipeQuery::default())
            .await,
        Err(Error::Pagination(_))
    ));
}

#[tokio::test]
async fn recipe_writes_preserve_nested_inputs_and_patch_only_supplied_fields() {
    let server = MockServer::start().await;
    let recipe = NewRecipe {
        steps: vec![Step {
            instruction: "Simmer.".into(),
            ..Default::default()
        }],
        ..NewRecipe::new("Soup")
    };
    Mock::given(method("POST"))
        .and(path("/api/recipe/"))
        .and(body_json(
            json!({"name":"Soup","servings":1,"internal":true,"steps":[{
                "name":"","instruction":"Simmer.","ingredients":[],"order":0
            }]}),
        ))
        .respond_with(
            ResponseTemplate::new(201).set_body_json(json!({"id":7,"name":"Soup","steps":[]})),
        )
        .expect(1)
        .mount(&server)
        .await;
    assert_eq!(client(&server).create_recipe(&recipe).await.unwrap().id, 7);

    let patch: RecipePatch =
        serde_json::from_value(json!({"description":null,"servings":3})).unwrap();
    Mock::given(method("PATCH"))
        .and(path("/api/recipe/7/"))
        .and(body_json(json!({"description":null,"servings":3})))
        .respond_with(
            ResponseTemplate::new(200).set_body_json(json!({"id":7,"name":"Soup","servings":3})),
        )
        .expect(1)
        .mount(&server)
        .await;
    assert_eq!(
        client(&server)
            .update_recipe(7, &patch)
            .await
            .unwrap()
            .servings,
        Some(3)
    );
}

#[test]
fn nested_models_accept_decimal_strings_and_ids_without_losing_extra_fields() {
    let input = json!({"name":"Bread","keywords":[5],"steps":[{"instruction":"Mix.","ingredients":[
        {"amount":"0.1250","food":9,"unit":{"name":"g"},"original_text":"flour"}
    ]}]});
    let recipe: NewRecipe = serde_json::from_value(input).unwrap();
    let output = serde_json::to_value(recipe).unwrap();
    assert_eq!(output["keywords"], json!([5]));
    assert_eq!(output["steps"][0]["ingredients"][0]["amount"], "0.1250");
    assert_eq!(output["steps"][0]["ingredients"][0]["food"], 9);
    assert_eq!(
        output["steps"][0]["ingredients"][0]["original_text"],
        "flour"
    );
    assert_eq!(output["internal"], true);
}

#[tokio::test]
async fn empty_delete_and_structured_errors_are_handled() {
    let server = MockServer::start().await;
    Mock::given(method("DELETE"))
        .and(path("/api/recipe/7/"))
        .respond_with(ResponseTemplate::new(204))
        .expect(1)
        .mount(&server)
        .await;
    client(&server).delete_recipe(7).await.unwrap();
    Mock::given(method("GET"))
        .and(path("/api/recipe/7/"))
        .respond_with(ResponseTemplate::new(403).set_body_json(json!({"detail":"Forbidden"})))
        .expect(1)
        .mount(&server)
        .await;
    match client(&server).get_recipe(7).await.unwrap_err() {
        Error::Api { status, body } => {
            assert_eq!(status, 403);
            assert_eq!(body["detail"], "Forbidden");
        }
        error => panic!("unexpected error: {error}"),
    }
}

#[tokio::test]
async fn redirects_are_not_followed_and_non_json_errors_keep_the_status() {
    let server = MockServer::start().await;
    Mock::given(path("/api/recipe/7/"))
        .respond_with(
            ResponseTemplate::new(302)
                .insert_header("Location", "/login/")
                .set_body_string("Sign in"),
        )
        .expect(1)
        .mount(&server)
        .await;
    assert!(matches!(
        client(&server).get_recipe(7).await,
        Err(Error::Api { status: 302, .. })
    ));
    assert_eq!(server.received_requests().await.unwrap().len(), 1);
}

#[tokio::test]
async fn request_timeout_is_applied() {
    let server = MockServer::start().await;
    Mock::given(path("/api/recipe/7/"))
        .respond_with(ResponseTemplate::new(200).set_delay(Duration::from_secs(1)))
        .mount(&server)
        .await;
    let client = Client::with_timeout(
        &server.uri(),
        Auth::Bearer("secret".into()),
        Duration::from_millis(50),
    )
    .unwrap();
    assert!(
        matches!(client.get_recipe(7).await, Err(Error::Transport(error)) if error.is_timeout())
    );
}
