use std::{collections::{HashMap, HashSet}, path::PathBuf};

use anyhow::{anyhow, Result};
use figment::{providers::{Format, Toml}, value::magic::RelativePathBuf, Figment};
use serde::{Deserialize, Serialize};
use sqlx::{sqlite::SqlitePoolOptions, Sqlite};

use crate::{morph::MecabConfig, vtt::CleanVttOptions};

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct Dictionary {
    pub name: Option<String>,
    pub url: String,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct DatabaseConfig {
    pub path: RelativePathBuf,
    pub max_connections: u32,
}

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
pub struct TemplateConfig {
    banner: Option<String>,
}

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
pub struct DisplayConfig {
    pub hide_tags: HashSet<String>,
}

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
pub struct ImportPluginConfig {
    pub name: String,
    pub url_patterns: Vec<String>,
    pub content_type: String,
    pub args: Vec<String>,
    pub clean_vtt: Option<CleanVttOptions>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct Config {
    pub port: u16,
    pub database: DatabaseConfig,
    pub lang: String,
    #[serde(default)]
    pub display: DisplayConfig,
    pub dictionaries: Vec<Dictionary>,
    pub mecab: MecabConfig,
    #[serde(default)]
    pub template: TemplateConfig,
    userdata: RelativePathBuf,
    #[serde(default)]
    pub import_plugins: Vec<ImportPluginConfig>,
}

impl Config {
    pub fn load(path: &str) -> Result<Self> {
        let config: Config = Figment::from(Toml::file(path)).extract()?;
        if !config.userdata.relative().is_absolute() {
            Err(anyhow!("`userdata` must resolve to an absolute path"))?;
        }
        Ok(config)
    }

    pub fn userdata(&self) -> PathBuf {
        self.userdata.relative()
    }

    pub fn word_images_path(&self) -> PathBuf {
        self.userdata().join("word_images")
    }

    pub fn book_audio_path(&self) -> PathBuf {
        self.userdata().join("book_audio")
    }
}

impl tera::Function for TemplateConfig {
    fn call(&self, _: &HashMap<String, tera::Value>) -> tera::Result<tera::Value> {
        tera::to_value(self).map_err(|e| e.into())
    }
}

impl DatabaseConfig {
    pub async fn open(&self) -> Result<sqlx::Pool<Sqlite>> {
        let db_path = self.path.relative();
        let Some(db) = db_path.to_str() else {
            return Err(anyhow!("invalid database path: {db_path:?}"));
        };
        let pool = SqlitePoolOptions::new()
            .max_connections(self.max_connections)
            .connect(db)
            .await?;
        Ok(pool)
    }
}
