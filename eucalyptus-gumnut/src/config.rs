use std::{
    borrow::Cow,
    error::Error,
    path::{Path, PathBuf},
    sync::LazyLock,
};

use serde::Deserialize;

use crate::item;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Config {
    pub hide_delay: f64,
    pub item: ItemConfig,
}

static XDG_CONFIG_HOME: LazyLock<Cow<'static, Path>> = LazyLock::new(|| {
    if let Some(xdg_config_home) = std::env::var_os("XDG_CONFIG_HOME") {
        Cow::Owned(PathBuf::from(xdg_config_home))
    } else if let Some(home) = std::env::home_dir() {
        Cow::Owned(home.join(".config"))
    } else {
        Cow::Borrowed(Path::new("~/.config"))
    }
});

pub static DEFAULT_PATH: LazyLock<PathBuf> =
    LazyLock::new(|| XDG_CONFIG_HOME.join("eucalyptus-gumnut/eucalyptus-gumnut.toml"));

impl Config {
    pub fn load() -> Result<Self, Box<dyn Error>> {
        let config_content = std::fs::read(DEFAULT_PATH.as_path())?;
        Ok(toml::from_slice(&config_content)?)
    }
}

impl Default for Config {
    fn default() -> Self {
        Self {
            hide_delay: 3.0,
            item: ItemConfig::default(),
        }
    }
}

#[derive(Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ItemConfig {
    pub power_profile: item::power_profile::Config,
    // pub backlight: BacklightConfig,
}
