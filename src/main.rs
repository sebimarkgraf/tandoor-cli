use std::{
    io::{self, IsTerminal, Read, Write},
    path::PathBuf,
    process::ExitCode,
    time::Duration,
};

use clap::{Args, Parser, Subcommand, ValueEnum};
use serde::Serialize;
use serde_json::{Value, json};
use tandoor::{Auth, Client, Error, Method, NewRecipe, Recipe, RecipePatch, RecipeQuery, Step};

mod resources;
use resources::{Resource, ResourceCommand};

#[derive(Parser)]
#[command(
    name = "tandoor",
    version,
    about = "Tandoor Recipes for humans and agents",
    after_help = "Output defaults to text in a terminal and JSON when piped.\nUse --output json for predictable agent output. Credentials are never saved."
)]
struct Cli {
    /// Instance URL or API root, including any deployment subpath
    #[arg(long, env = "TANDOOR_URL", global = true)]
    url: Option<String>,
    /// Authentication credential (prefer the environment variable)
    #[arg(long, env = "TANDOOR_TOKEN", hide_env_values = true, global = true)]
    token: Option<String>,
    /// Bearer API token, allauth session token, or legacy DRF token
    #[arg(long, env = "TANDOOR_AUTH", default_value = "bearer", global = true)]
    auth: AuthKind,
    #[arg(long, default_value = "auto", global = true)]
    output: Output,
    /// HTTP timeout in seconds
    #[arg(long, default_value_t = 30, value_parser = clap::value_parser!(u64).range(1..), global = true)]
    timeout: u64,
    #[command(subcommand)]
    command: Command,
}

#[derive(Clone, Copy, ValueEnum)]
enum AuthKind {
    Bearer,
    Session,
    LegacyToken,
}

#[derive(Clone, Copy, ValueEnum)]
enum Output {
    Auto,
    Json,
    Text,
}

#[derive(Subcommand)]
enum Command {
    /// Manage recipes
    Recipe {
        #[command(subcommand)]
        command: RecipeCommand,
    },
    /// Manage reusable foods in the ingredient catalog
    #[command(alias = "foods")]
    Food {
        #[command(subcommand)]
        command: ResourceCommand,
    },
    /// Manage recipe ingredient rows (food, unit, amount, and note)
    #[command(alias = "ingredients")]
    Ingredient {
        #[command(subcommand)]
        command: ResourceCommand,
    },
    /// Manage tags (Tandoor keywords)
    #[command(aliases = ["tags", "keyword"])]
    Tag {
        #[command(subcommand)]
        command: ResourceCommand,
    },
    /// Call any endpoint relative to the API root (e.g. keyword/)
    Api {
        path: String,
        #[arg(short, long, default_value = "GET", value_parser = ["GET", "POST", "PUT", "PATCH", "DELETE"])]
        method: String,
        /// Repeatable query parameter, as key=value
        #[arg(short, long, value_parser = parse_pair)]
        query: Vec<(String, String)>,
        /// JSON body file; use - to read stdin
        #[arg(short, long)]
        file: Option<PathBuf>,
        /// Required for API requests that write data
        #[arg(long)]
        yes: bool,
    },
}

#[derive(Subcommand)]
enum RecipeCommand {
    /// List a page of recipes or search by text
    #[command(alias = "search")]
    List {
        #[arg(short, long)]
        query: Option<String>,
        #[arg(long, default_value_t = 1, value_parser = clap::value_parser!(u32).range(1..))]
        page: u32,
        #[arg(long, default_value_t = 25, value_parser = clap::value_parser!(u32).range(1..=100))]
        page_size: u32,
        /// Server sort key, e.g. name or -created_at
        #[arg(long, allow_hyphen_values = true)]
        sort: Option<String>,
        /// Collect all remaining pages; JSON output is an array
        #[arg(long)]
        all: bool,
    },
    /// Fetch a complete recipe, including steps and ingredients
    Get { id: u64 },
    /// Create from flags or a JSON file/stdin
    Create(CreateArgs),
    /// Apply a partial JSON object; omitted fields remain unchanged
    Update {
        id: u64,
        /// JSON patch file; use - to read stdin
        #[arg(short, long)]
        file: PathBuf,
    },
    /// Permanently delete a recipe (requires --yes)
    Delete {
        id: u64,
        #[arg(long, required = true)]
        yes: bool,
    },
}

#[derive(Args)]
struct CreateArgs {
    #[arg(long, required_unless_present = "file", conflicts_with = "file")]
    name: Option<String>,
    #[arg(long, conflicts_with = "file")]
    description: Option<String>,
    #[arg(long, value_parser = clap::value_parser!(u32).range(1..), conflicts_with = "file")]
    servings: Option<u32>,
    /// Repeat for each step; ingredients can be supplied with --file
    #[arg(long, conflicts_with = "file")]
    instruction: Vec<String>,
    /// JSON recipe file; use - to read stdin
    #[arg(short, long)]
    file: Option<PathBuf>,
}

#[derive(Serialize)]
struct CliError {
    kind: &'static str,
    message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    status: Option<u16>,
    #[serde(skip_serializing_if = "Option::is_none")]
    details: Option<Value>,
}

impl CliError {
    fn input(message: impl Into<String>) -> Self {
        Self {
            kind: "input",
            message: message.into(),
            status: None,
            details: None,
        }
    }
}

impl From<Error> for CliError {
    fn from(error: Error) -> Self {
        let (kind, status, details) = match &error {
            Error::Config(_) => ("configuration", None, None),
            Error::Transport(_) => ("transport", None, None),
            Error::Api { status, body } => ("api", Some(*status), Some(body.clone())),
            Error::Json(_) => ("json", None, None),
            Error::Pagination(_) => ("pagination", None, None),
        };
        Self {
            kind,
            message: error.to_string(),
            status,
            details,
        }
    }
}

impl From<serde_json::Error> for CliError {
    fn from(error: serde_json::Error) -> Self {
        Self::input(error.to_string())
    }
}

struct CommandOutput {
    json: Value,
    text: String,
}

#[tokio::main]
async fn main() -> ExitCode {
    let cli = Cli::parse();
    let use_json = matches!(cli.output, Output::Json)
        || matches!(cli.output, Output::Auto) && !io::stdout().is_terminal();
    match run(&cli).await {
        Ok(output) => {
            let output = if use_json {
                output.json.to_string()
            } else {
                output.text
            };
            match writeln!(io::stdout().lock(), "{output}") {
                Ok(()) => ExitCode::SUCCESS,
                Err(error) if error.kind() == io::ErrorKind::BrokenPipe => ExitCode::SUCCESS,
                Err(error) => {
                    eprintln!("failed to write output: {error}");
                    ExitCode::FAILURE
                }
            }
        }
        Err(error) => {
            if use_json {
                eprintln!("{}", json!({"error": error}));
            } else {
                eprintln!("error: {}", error.message);
            }
            ExitCode::FAILURE
        }
    }
}

async fn run(cli: &Cli) -> Result<CommandOutput, CliError> {
    let url = cli
        .url
        .as_deref()
        .ok_or_else(|| CliError::input("set TANDOOR_URL or pass --url"))?;
    let token = cli
        .token
        .clone()
        .ok_or_else(|| CliError::input("set TANDOOR_TOKEN or pass --token"))?;
    let auth = match cli.auth {
        AuthKind::Bearer => Auth::Bearer(token),
        AuthKind::Session => Auth::Session(token),
        AuthKind::LegacyToken => Auth::LegacyToken(token),
    };
    let client = Client::with_timeout(url, auth, Duration::from_secs(cli.timeout))?;
    match &cli.command {
        Command::Food { command } => resources::run(&client, Resource::Food, command).await,
        Command::Ingredient { command } => {
            resources::run(&client, Resource::Ingredient, command).await
        }
        Command::Tag { command } => resources::run(&client, Resource::Tag, command).await,
        Command::Recipe { command } => match command {
            RecipeCommand::List {
                query,
                page,
                page_size,
                sort,
                all,
            } => {
                let query = RecipeQuery {
                    query: query.clone(),
                    page: Some(*page),
                    page_size: Some(*page_size),
                    sort_order: sort.clone(),
                };
                if *all {
                    let recipes = client.list_all_recipes(&query).await?;
                    let text = render_list(&recipes);
                    Ok(CommandOutput {
                        json: serde_json::to_value(recipes)?,
                        text,
                    })
                } else {
                    let page = client.list_recipes(&query).await?;
                    let mut text = render_list(&page.results);
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
            RecipeCommand::Get { id } => recipe_output(client.get_recipe(*id).await?),
            RecipeCommand::Create(args) => {
                let recipe = if let Some(file) = &args.file {
                    serde_json::from_value::<NewRecipe>(read_json(file)?)?
                } else {
                    let mut recipe =
                        NewRecipe::new(args.name.as_deref().expect("clap requires name or file"));
                    recipe.description = args.description.clone();
                    recipe.servings = args.servings.unwrap_or(1);
                    recipe.steps = args
                        .instruction
                        .iter()
                        .enumerate()
                        .map(|(order, instruction)| Step {
                            instruction: instruction.clone(),
                            order: order as u32,
                            ..Default::default()
                        })
                        .collect();
                    recipe
                };
                if recipe.name.trim().is_empty() || recipe.servings == 0 {
                    return Err(CliError::input(
                        "recipe name must not be empty and servings must be positive",
                    ));
                }
                recipe_output(client.create_recipe(&recipe).await?)
            }
            RecipeCommand::Update { id, file } => {
                let patch: RecipePatch = serde_json::from_value(read_json(file)?)?;
                recipe_output(client.update_recipe(*id, &patch).await?)
            }
            RecipeCommand::Delete { id, .. } => {
                client.delete_recipe(*id).await?;
                Ok(CommandOutput {
                    json: json!({"deleted": true, "id": id}),
                    text: format!("Deleted recipe {id}."),
                })
            }
        },
        Command::Api {
            path,
            method,
            query,
            file,
            yes,
        } => {
            if method != "GET" && !yes {
                return Err(CliError::input("API writes require --yes"));
            }
            if method == "GET" && file.is_some() {
                return Err(CliError::input("GET requests do not accept --file"));
            }
            let body = file.as_ref().map(read_json).transpose()?;
            let method =
                Method::from_bytes(method.as_bytes()).expect("clap validates HTTP methods");
            let value: Value = client.request(method, path, query, body.as_ref()).await?;
            let text = serde_json::to_string_pretty(&value)?;
            Ok(CommandOutput { json: value, text })
        }
    }
}

fn parse_pair(value: &str) -> Result<(String, String), String> {
    let (key, value) = value.split_once('=').ok_or("expected key=value")?;
    if key.is_empty() {
        return Err("query key must not be empty".into());
    }
    Ok((key.into(), value.into()))
}

fn read_json(path: &PathBuf) -> Result<Value, CliError> {
    let mut input = String::new();
    if path.as_os_str() == "-" {
        io::stdin()
            .read_to_string(&mut input)
            .map_err(|error| CliError::input(format!("cannot read stdin: {error}")))?;
    } else {
        input = std::fs::read_to_string(path)
            .map_err(|error| CliError::input(format!("cannot read {}: {error}", path.display())))?;
    }
    Ok(serde_json::from_str(&input)?)
}

fn recipe_output(recipe: Recipe) -> Result<CommandOutput, CliError> {
    let mut text = format!("{} (#{})", recipe.name, recipe.id);
    if let Some(description) = &recipe.description {
        text.push_str(&format!("\n{description}"));
    }
    if let Some(servings) = recipe.servings {
        text.push_str(&format!("\nServings: {servings}"));
    }
    if let Some(time) = recipe.working_time {
        text.push_str(&format!("\nWorking time: {time} min"));
    }
    if let Some(time) = recipe.waiting_time {
        text.push_str(&format!("\nWaiting time: {time} min"));
    }
    let keywords: Vec<_> = recipe
        .keywords
        .iter()
        .filter_map(|keyword| keyword.name.as_deref().or(keyword.label.as_deref()))
        .collect();
    if !keywords.is_empty() {
        text.push_str(&format!("\nKeywords: {}", keywords.join(", ")));
    }
    if let Some(steps) = &recipe.steps {
        for (index, step) in steps.iter().enumerate() {
            text.push_str(&format!("\n\n{}. {}", index + 1, step.name));
            for ingredient in &step.ingredients {
                text.push_str(&format!("\n  - {}", render_ingredient(ingredient)));
            }
            if !step.instruction.is_empty() {
                text.push_str(&format!("\n{}", step.instruction));
            }
        }
    }
    Ok(CommandOutput {
        json: serde_json::to_value(recipe)?,
        text,
    })
}

fn render_ingredient(ingredient: &tandoor::Ingredient) -> String {
    let mut parts = Vec::new();
    if !ingredient.no_amount && !ingredient.is_header {
        if let Some(amount) = &ingredient.amount {
            parts.push(amount.to_string());
        }
        if let Some(unit) = &ingredient.unit {
            parts.push(unit.to_string());
        }
    }
    if let Some(food) = &ingredient.food {
        parts.push(food.to_string());
    }
    if !ingredient.note.is_empty() {
        parts.push(ingredient.note.clone());
    }
    parts.join(" ")
}

fn render_list(recipes: &[Recipe]) -> String {
    if recipes.is_empty() {
        return "No recipes found.".into();
    }
    recipes
        .iter()
        .map(|recipe| format!("{}\t{}", recipe.id, recipe.name))
        .collect::<Vec<_>>()
        .join("\n")
}
