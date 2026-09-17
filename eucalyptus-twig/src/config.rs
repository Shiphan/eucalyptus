use std::{env, error::Error, fs, path::PathBuf};

use iced_core::theme::Base;
use serde::Deserialize;

use crate::widget::{WidgetConfig, WidgetKind};

#[derive(Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Config {
    #[serde(deserialize_with = "deserialize_theme")]
    pub theme: iced_core::Theme,
    pub left: Box<[WidgetKindGroup]>,
    pub middle: Box<[WidgetKindGroup]>,
    pub right: Box<[WidgetKindGroup]>,
    pub widget: WidgetConfig,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            theme: iced_core::Theme::KanagawaWave,
            left: Box::new([
                // WidgetKind::PowerMenu.into(),
                WidgetKind::Power.into(),
                WidgetKind::Clock.into(),
                // WidgetKind::Display.into(),
            ]),
            middle: Box::new([
                // WidgetKind::Workspaces.into()
            ]),
            right: Box::new([
                // WidgetKind::Volume.into(),
                // WidgetKind::Network.into(),
                [WidgetKind::Network, WidgetKind::Bluetooth].into(),
                // WidgetKind::PowerProfile.into(),
            ]),
            widget: WidgetConfig::default(),
        }
    }
}

impl Config {
    pub fn load() -> Result<Self, Box<dyn Error>> {
        let path = if let Some(config_home) = env::var_os("XDG_CONFIG_HOME")
            && !config_home.is_empty()
        {
            [
                config_home.as_os_str(),
                "eucalyptus-twig/eucalyptus-twig.toml".as_ref(),
            ]
            .iter()
            .collect::<PathBuf>()
        } else if let Some(home_dir) = env::home_dir() {
            tracing::warn!("XDG_CONFIG_HOME is not set or is empty, default to $HOME/.config");
            [
                home_dir.as_os_str(),
                ".config/eucalyptus-twig/eucalyptus-twig.toml".as_ref(),
            ]
            .iter()
            .collect()
        } else {
            return Err("Failed to get home directory".into());
        };
        let config_content = fs::read(path)?;
        Ok(toml::from_slice(&config_content)?)
    }
}

fn deserialize_theme<'de, D>(deserializer: D) -> Result<iced_core::Theme, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let s: &str = serde::Deserialize::deserialize(deserializer)?;
    iced_core::Theme::ALL
        .iter()
        .find(|theme| s == theme.name())
        .cloned()
        .ok_or_else(|| {
            serde::de::Error::custom(format_args!(
                "unknown theme `{}`, expected one of {:?}",
                s,
                iced_core::Theme::ALL
            ))
        })
}

#[derive(Deserialize)]
#[serde(untagged)]
pub enum WidgetKindGroup {
    One(WidgetKind),
    Array(Box<[WidgetKind]>),
}

impl<'a> IntoIterator for &'a WidgetKindGroup {
    type Item = &'a WidgetKind;

    type IntoIter = std::slice::Iter<'a, WidgetKind>;

    fn into_iter(self) -> Self::IntoIter {
        match self {
            WidgetKindGroup::One(widget) => std::slice::from_ref(widget).iter(),
            WidgetKindGroup::Array(widgets) => widgets.iter(),
        }
    }
}

impl From<WidgetKind> for WidgetKindGroup {
    fn from(value: WidgetKind) -> Self {
        Self::One(value)
    }
}

impl<const N: usize> From<[WidgetKind; N]> for WidgetKindGroup {
    fn from(value: [WidgetKind; N]) -> Self {
        Self::Array(Box::new(value))
    }
}

impl Into<Box<[WidgetKind]>> for WidgetKindGroup {
    fn into(self) -> Box<[WidgetKind]> {
        match self {
            Self::One(widget_kind) => Box::new([widget_kind]),
            Self::Array(widget_kinds) => widget_kinds,
        }
    }
}
