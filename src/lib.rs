//! Async SDK for Tandoor Recipes, independent of the command-line interface.
//!
//! ```no_run
//! use tandoor::{Auth, Client, RecipeQuery, NewRecipe};
//!
//! # async fn example() -> Result<(), Box<dyn std::error::Error>> {
//! let client = Client::new("https://recipes.example.com", Auth::Bearer("secret".into()))?;
//! let page = client.list_recipes(&RecipeQuery {
//!     query: Some("pasta".into()),
//!     ..Default::default()
//! }).await?;
//! for recipe in page.results {
//!     println!("{}: {}", recipe.id, recipe.name);
//! }
//! let created = client.create_recipe(&NewRecipe::new("Soup")).await?;
//! # Ok(())
//! # }
//! ```

mod client;
mod error;
mod models;

pub use client::{Auth, Client};
pub use error::{Error, Result};
pub use models::*;
pub use reqwest::Method;
