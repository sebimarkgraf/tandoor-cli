use std::path::PathBuf;

use clap::Subcommand;
use serde_json::{Value, json};
use tandoor::{Client, Method, Page};

use crate::{CliError, CommandOutput, read_json, render_ingredient};

#[derive(Subcommand)]
pub enum FoodCommand {
    #[command(flatten)]
    Resource(ResourceCommand),
    /// Merge SOURCE_ID into TARGET_ID and delete the source (requires --yes)
    ///
    /// Tandoor may discard source nutrition properties and unit conversions.
    Merge {
        source_id: u64,
        target_id: u64,
        #[arg(long, required = true)]
        yes: bool,
    },
}

#[derive(Subcommand)]
pub enum ResourceCommand {
    /// List a page of entries
    #[command(alias = "search")]
    List {
        /// Search foods or tags by name
        #[arg(short, long)]
        query: Option<String>,
        /// Filter ingredient rows by food ID
        #[arg(long)]
        food: Option<u64>,
        /// Filter ingredient rows by unit ID
        #[arg(long)]
        unit: Option<u64>,
        #[arg(long, default_value_t = 1, value_parser = clap::value_parser!(u32).range(1..))]
        page: u32,
        #[arg(long, default_value_t = 25, value_parser = clap::value_parser!(u32).range(1..=100))]
        page_size: u32,
        /// Collect all remaining pages; JSON output is an array
        #[arg(long)]
        all: bool,
    },
    /// Fetch an entry by ID
    Get { id: u64 },
    /// Create from a JSON object
    Create {
        /// JSON body file; use - to read stdin
        #[arg(short, long)]
        file: PathBuf,
    },
    /// Apply a partial JSON object; omitted fields remain unchanged
    Update {
        id: u64,
        /// JSON patch file; use - to read stdin
        #[arg(short, long)]
        file: PathBuf,
    },
    /// Permanently delete an entry (requires --yes)
    Delete {
        id: u64,
        #[arg(long, required = true)]
        yes: bool,
    },
}

#[derive(Clone, Copy)]
pub enum Resource {
    Food,
    Ingredient,
    Tag,
}

impl Resource {
    fn endpoint(self) -> &'static str {
        match self {
            Self::Food => "food/",
            Self::Ingredient => "ingredient/",
            Self::Tag => "keyword/",
        }
    }

    fn name(self) -> &'static str {
        match self {
            Self::Food => "food",
            Self::Ingredient => "ingredient",
            Self::Tag => "tag",
        }
    }

    fn label(self, value: &Value) -> Result<String, CliError> {
        if matches!(self, Self::Ingredient) {
            return Ok(render_ingredient(&serde_json::from_value(value.clone())?));
        }
        Ok(value["name"]
            .as_str()
            .or_else(|| value["label"].as_str())
            .unwrap_or_default()
            .to_owned())
    }

    fn render_list(self, values: &[Value]) -> Result<String, CliError> {
        if values.is_empty() {
            return Ok("No entries found.".into());
        }
        values
            .iter()
            .map(|value| Ok(format!("{}\t{}", value["id"], self.label(value)?)))
            .collect::<Result<Vec<_>, _>>()
            .map(|lines| lines.join("\n"))
    }

    fn output(self, value: Value) -> Result<CommandOutput, CliError> {
        let mut text = format!("{} (#{})", self.label(&value)?, value["id"]);
        if let Some(description) = value["description"].as_str() {
            text.push_str(&format!("\n{description}"));
        }
        Ok(CommandOutput { json: value, text })
    }
}

pub async fn run_food(client: &Client, command: &FoodCommand) -> Result<CommandOutput, CliError> {
    match command {
        FoodCommand::Resource(command) => run(client, Resource::Food, command).await,
        FoodCommand::Merge {
            source_id,
            target_id,
            ..
        } => {
            if source_id == target_id {
                return Err(CliError::input("source and target food IDs must differ"));
            }
            let value = client
                .request(
                    Method::PUT,
                    &format!("food/{source_id}/merge/{target_id}/"),
                    &[],
                    Some(&json!({})),
                )
                .await?;
            Ok(CommandOutput {
                json: value,
                text: format!("Merged food {source_id} into {target_id}."),
            })
        }
    }
}

pub async fn run(
    client: &Client,
    resource: Resource,
    command: &ResourceCommand,
) -> Result<CommandOutput, CliError> {
    let endpoint = resource.endpoint();
    match command {
        ResourceCommand::List {
            query,
            food,
            unit,
            page,
            page_size,
            all,
        } => {
            let is_ingredient = matches!(resource, Resource::Ingredient);
            if is_ingredient && query.is_some() {
                return Err(CliError::input(
                    "ingredient lists use --food and --unit filters, not --query",
                ));
            }
            if !is_ingredient && (food.is_some() || unit.is_some()) {
                return Err(CliError::input(
                    "--food and --unit filters are only supported for ingredient lists",
                ));
            }
            let mut pairs = vec![
                ("page".into(), page.to_string()),
                ("page_size".into(), page_size.to_string()),
            ];
            if let Some(query) = query {
                pairs.push(("query".into(), query.clone()));
            }
            if let Some(food) = food {
                pairs.push(("food".into(), food.to_string()));
            }
            if let Some(unit) = unit {
                pairs.push(("unit".into(), unit.to_string()));
            }
            let page: Page<Value> = client.request(Method::GET, endpoint, &pairs, None).await?;
            if *all {
                let values = client.collect_pages(page).await?;
                let text = resource.render_list(&values)?;
                Ok(CommandOutput {
                    json: serde_json::to_value(values)?,
                    text,
                })
            } else {
                let mut text = resource.render_list(&page.results)?;
                text.push_str(&format!(
                    "\n{} total; more pages: {}",
                    page.count,
                    if page.next.is_some() { "yes" } else { "no" }
                ));
                Ok(CommandOutput {
                    json: serde_json::to_value(page)?,
                    text,
                })
            }
        }
        ResourceCommand::Get { id } => resource.output(
            client
                .request(Method::GET, &format!("{endpoint}{id}/"), &[], None)
                .await?,
        ),
        ResourceCommand::Create { file } | ResourceCommand::Update { file, .. } => {
            let body = read_json(file)?;
            if !body.is_object() {
                return Err(CliError::input("body must be a JSON object"));
            }
            let creating = matches!(command, ResourceCommand::Create { .. });
            if !matches!(resource, Resource::Ingredient)
                && (creating || body.get("name").is_some())
                && body["name"]
                    .as_str()
                    .is_none_or(|name| name.trim().is_empty())
            {
                return Err(CliError::input("name must be a non-empty string"));
            }
            let (method, path) = match command {
                ResourceCommand::Update { id, .. } => (Method::PATCH, format!("{endpoint}{id}/")),
                _ => (Method::POST, endpoint.to_owned()),
            };
            resource.output(client.request(method, &path, &[], Some(&body)).await?)
        }
        ResourceCommand::Delete { id, .. } => {
            let _: Value = client
                .request(Method::DELETE, &format!("{endpoint}{id}/"), &[], None)
                .await?;
            Ok(CommandOutput {
                json: json!({"deleted": true, "id": id}),
                text: format!("Deleted {} {id}.", resource.name()),
            })
        }
    }
}
