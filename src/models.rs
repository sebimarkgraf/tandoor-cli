use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// Unmodeled fields are retained for compatibility with different server versions.
pub type ExtraFields = BTreeMap<String, Value>;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Page<T> {
    pub count: u64,
    pub next: Option<String>,
    pub previous: Option<String>,
    pub results: Vec<T>,
}

/// Recipe lists contain overviews; `steps` is present on detail responses.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Recipe {
    pub id: u64,
    pub name: String,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub servings: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub servings_text: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub working_time: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub waiting_time: Option<u32>,
    #[serde(default)]
    pub keywords: Vec<Keyword>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub steps: Option<Vec<Step>>,
    #[serde(flatten)]
    pub extra: ExtraFields,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Keyword {
    pub id: u64,
    // Overview responses use `label`; detail responses also include `name`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
    #[serde(flatten)]
    pub extra: ExtraFields,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Step {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub id: Option<u64>,
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub instruction: String,
    #[serde(default)]
    pub ingredients: Vec<Ingredient>,
    #[serde(default)]
    pub order: u32,
    #[serde(flatten)]
    pub extra: ExtraFields,
}

/// Tandoor accepts decimal amounts as numbers or strings.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum Decimal {
    Number(serde_json::Number),
    String(String),
}

impl std::fmt::Display for Decimal {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Number(number) => write!(f, "{number}"),
            Self::String(string) => f.write_str(string),
        }
    }
}

/// A food or unit can be supplied as an existing ID or a nested object.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum NamedReference {
    Id(u64),
    Object {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        id: Option<u64>,
        name: String,
        #[serde(flatten)]
        extra: ExtraFields,
    },
}

impl std::fmt::Display for NamedReference {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Id(id) => write!(f, "#{id}"),
            Self::Object { name, .. } => f.write_str(name),
        }
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Ingredient {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub id: Option<u64>,
    #[serde(default)]
    pub food: Option<NamedReference>,
    #[serde(default)]
    pub unit: Option<NamedReference>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub amount: Option<Decimal>,
    #[serde(default)]
    pub note: String,
    #[serde(default)]
    pub order: u32,
    #[serde(default)]
    pub no_amount: bool,
    #[serde(default)]
    pub is_header: bool,
    #[serde(flatten)]
    pub extra: ExtraFields,
}

/// Creation is separate from responses so read-only fields are not sent back.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NewRecipe {
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(default = "one")]
    pub servings: u32,
    #[serde(default = "yes")]
    pub internal: bool,
    #[serde(default)]
    pub steps: Vec<Step>,
    #[serde(flatten)]
    pub extra: ExtraFields,
}

fn one() -> u32 {
    1
}
fn yes() -> bool {
    true
}

impl NewRecipe {
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            description: None,
            servings: 1,
            internal: true,
            steps: Vec::new(),
            extra: ExtraFields::new(),
        }
    }
}

/// Omitted fields are unchanged. Use `extra` for other fields, including JSON null.
/// Nested arrays follow Tandoor's replacement semantics; include existing IDs when editing.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct RecipePatch {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub servings: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub steps: Option<Vec<Step>>,
    #[serde(flatten)]
    pub extra: ExtraFields,
}

#[derive(Debug, Clone, Default, Serialize)]
pub struct RecipeQuery {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub query: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub page: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub page_size: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sort_order: Option<String>,
}
