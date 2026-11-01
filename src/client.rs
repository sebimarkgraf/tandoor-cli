use std::{collections::HashSet, fmt, time::Duration};

use reqwest::{
    Method, Url,
    header::{ACCEPT, AUTHORIZATION, HeaderMap, HeaderValue},
};
use serde::{Serialize, de::DeserializeOwned};
use serde_json::Value;

use crate::{Error, NewRecipe, Page, Recipe, RecipePatch, RecipeQuery, Result};

/// Credentials are redacted from Debug output. Use Session for current allauth sessions,
/// Bearer for OAuth API tokens, or LegacyToken for older DRF token installations.
#[derive(Clone)]
pub enum Auth {
    Bearer(String),
    Session(String),
    LegacyToken(String),
}

impl fmt::Debug for Auth {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Bearer(_) => "Bearer([REDACTED])",
            Self::Session(_) => "Session([REDACTED])",
            Self::LegacyToken(_) => "LegacyToken([REDACTED])",
        })
    }
}

/// Reusable async HTTP client. Does not read environment variables or perform CLI I/O.
#[derive(Clone, Debug)]
pub struct Client {
    http: reqwest::Client,
    api_url: Url,
}

impl Client {
    /// Accepts an instance root or an API root, including a deployment subpath.
    pub fn new(base_url: &str, auth: Auth) -> Result<Self> {
        Self::with_timeout(base_url, auth, Duration::from_secs(30))
    }

    pub fn with_timeout(base_url: &str, auth: Auth, timeout: Duration) -> Result<Self> {
        let mut api_url = Url::parse(base_url)
            .map_err(|_| Error::Config("base URL must be an absolute HTTP(S) URL".into()))?;
        if !matches!(api_url.scheme(), "http" | "https")
            || api_url.host_str().is_none()
            || !api_url.username().is_empty()
            || api_url.password().is_some()
            || api_url.query().is_some()
            || api_url.fragment().is_some()
        {
            return Err(Error::Config(
                "base URL must use HTTP(S), without credentials, query, or fragment".into(),
            ));
        }
        if timeout.is_zero() {
            return Err(Error::Config("timeout must be greater than zero".into()));
        }
        let path = api_url.path().trim_end_matches('/');
        let path = if path.ends_with("/api") {
            format!("{path}/")
        } else {
            format!("{path}/api/")
        };
        api_url.set_path(&path);

        let (header, credential, raw_token) = match &auth {
            Auth::Bearer(token) => (AUTHORIZATION, format!("Bearer {token}"), token),
            Auth::Session(token) => (
                reqwest::header::HeaderName::from_static("x-session-token"),
                token.clone(),
                token,
            ),
            Auth::LegacyToken(token) => (AUTHORIZATION, format!("Token {token}"), token),
        };
        if raw_token.trim().is_empty() {
            return Err(Error::Config(
                "authentication token must not be empty".into(),
            ));
        }
        let mut value = HeaderValue::from_str(&credential).map_err(|_| {
            Error::Config("authentication token contains invalid header characters".into())
        })?;
        value.set_sensitive(true);
        let mut headers = HeaderMap::new();
        headers.insert(header, value);
        headers.insert(ACCEPT, HeaderValue::from_static("application/json"));
        let http = reqwest::Client::builder()
            .default_headers(headers)
            .user_agent(concat!("tandoor-cli/", env!("CARGO_PKG_VERSION")))
            .timeout(timeout)
            // Authentication must not be forwarded by redirects, nor writes silently redirected.
            .redirect(reqwest::redirect::Policy::none())
            .build()?;
        Ok(Self { http, api_url })
    }

    pub fn api_url(&self) -> &Url {
        &self.api_url
    }

    /// Access endpoints beyond the typed recipe methods. `path` is relative to `/api/`.
    /// Supply query parameters separately; empty responses deserialize as JSON null.
    pub async fn request<T: DeserializeOwned>(
        &self,
        method: Method,
        path: &str,
        query: &[(String, String)],
        body: Option<&Value>,
    ) -> Result<T> {
        if path.starts_with('/') || path.contains(['?', '#', '\\']) {
            return Err(Error::Config(
                "API path must be relative, with query parameters supplied separately".into(),
            ));
        }
        let url = self
            .api_url
            .join(path)
            .map_err(|_| Error::Config("invalid API path".into()))?;
        self.validate_url(&url)?;
        let mut request = self.http.request(method, url).query(query);
        if let Some(body) = body {
            request = request.json(body);
        }
        Self::decode(request.send().await?).await
    }

    fn validate_url(&self, url: &Url) -> Result<()> {
        if url.origin() != self.api_url.origin()
            || !url.path().starts_with(self.api_url.path())
            || !url.username().is_empty()
            || url.password().is_some()
            || url.fragment().is_some()
        {
            return Err(Error::Config(
                "request URL must stay within the configured API root".into(),
            ));
        }
        Ok(())
    }

    async fn decode<T: DeserializeOwned>(response: reqwest::Response) -> Result<T> {
        let status = response.status();
        let bytes = response.bytes().await?;
        if !status.is_success() {
            let body = serde_json::from_slice(&bytes).unwrap_or_else(|_| {
                Value::String(String::from_utf8_lossy(&bytes).chars().take(8192).collect())
            });
            return Err(Error::Api {
                status: status.as_u16(),
                body,
            });
        }
        Ok(serde_json::from_slice(if bytes.is_empty() {
            b"null"
        } else {
            &bytes
        })?)
    }

    pub async fn list_recipes(&self, query: &RecipeQuery) -> Result<Page<Recipe>> {
        if query.page == Some(0) || query.page_size.is_some_and(|size| size == 0 || size > 100) {
            return Err(Error::Config(
                "page must be positive and page_size must be between 1 and 100".into(),
            ));
        }
        let query = serde_json::to_value(query)?;
        let pairs: Vec<_> = query
            .as_object()
            .expect("RecipeQuery is an object")
            .iter()
            .map(|(key, value)| {
                (
                    key.clone(),
                    value
                        .as_str()
                        .map(str::to_owned)
                        .unwrap_or_else(|| value.to_string()),
                )
            })
            .collect();
        self.request(Method::GET, "recipe/", &pairs, None).await
    }

    /// Fetch the next page while preventing credentials from leaving the configured API root.
    pub async fn next_page<T: DeserializeOwned>(&self, page: &Page<T>) -> Result<Option<Page<T>>> {
        let Some(next) = &page.next else {
            return Ok(None);
        };
        let url = self
            .api_url
            .join(next)
            .map_err(|_| Error::Pagination("invalid next URL".into()))?;
        self.validate_url(&url)?;
        Ok(Some(Self::decode(self.http.get(url).send().await?).await?))
    }

    /// Collect pages starting at `query.page` (the first page when omitted).
    pub async fn list_all_recipes(&self, query: &RecipeQuery) -> Result<Vec<Recipe>> {
        let mut page = self.list_recipes(query).await?;
        let mut results = Vec::new();
        let mut visited = HashSet::new();
        loop {
            results.append(&mut page.results);
            let Some(next) = &page.next else {
                break;
            };
            if !visited.insert(next.clone()) {
                return Err(Error::Pagination(
                    "server returned a repeated next URL".into(),
                ));
            }
            page = self.next_page(&page).await?.expect("next URL is present");
        }
        Ok(results)
    }

    pub async fn get_recipe(&self, id: u64) -> Result<Recipe> {
        self.request(Method::GET, &format!("recipe/{id}/"), &[], None)
            .await
    }

    pub async fn create_recipe(&self, recipe: &NewRecipe) -> Result<Recipe> {
        self.write_recipe(Method::POST, "recipe/", recipe).await
    }

    /// PATCH changes only supplied fields. Tandoor controls nested-array replacement behavior.
    pub async fn update_recipe(&self, id: u64, patch: &RecipePatch) -> Result<Recipe> {
        self.write_recipe(Method::PATCH, &format!("recipe/{id}/"), patch)
            .await
    }

    async fn write_recipe(
        &self,
        method: Method,
        path: &str,
        body: &impl Serialize,
    ) -> Result<Recipe> {
        self.request(method, path, &[], Some(&serde_json::to_value(body)?))
            .await
    }

    pub async fn delete_recipe(&self, id: u64) -> Result<()> {
        let _: Value = self
            .request(Method::DELETE, &format!("recipe/{id}/"), &[], None)
            .await?;
        Ok(())
    }
}
