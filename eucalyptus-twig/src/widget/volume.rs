use eucalyptus_cellulose::{Element, task::{Task, TaskExt}};
use futures::{FutureExt, StreamExt, channel::mpsc};
use iced_core::Font;
use iced_futures::Subscription;
use serde::Deserialize;

use crate::{
    widget::{Widget, WidgetPadding, spawn_detached_command},
};

pub struct Volume {
    config: Config,
    state: State,
}

enum State {
    Ok {
        volume: Option<f32>,
        mute: Option<bool>,
    },
    Err {
        message: String,
    },
}

impl Widget for Volume {
    type Config = Config;

    type Message = Message;

    fn new(config: &Self::Config) -> (Self, Task<Self::Message>) {
        (
            Self {
                config: config.clone(),
                state: State::Ok {
                    volume: None,
                    mute: None,
                },
            },
            Task::none(),
        )
    }

    fn update(&mut self, message: Self::Message) -> impl Into<Task<Self::Message>> {
        match (&mut self.state, message) {
            (State::Ok { volume, .. }, Message::NewVolume(v)) => {
                *volume = v;
                Task::none()
            }
            (State::Ok { mute, .. }, Message::NewMute(m)) => {
                *mute = m;
                Task::none()
            }
            (State::Ok { volume, mute }, Message::NewVolumeAndMute(v, m)) => {
                *volume = v;
                *mute = m;
                Task::none()
            }
            (_, Message::LaunchSettings) => {
                if let Some(command) = &self.config.settings_command {
                    spawn_detached_command(command, "widget.volume.settings_command").discard()
                } else {
                    Task::none()
                }
            }
            (s, Message::Error(message)) => {
                *s = State::Err { message };
                Task::none()
            }
            (State::Err { .. }, _) => Task::none(),
        }
    }

    fn view(&self) -> Element<'_, Self::Message> {
        let widget: Element<_> = match &self.state {
            State::Ok {
                mute: Some(true), ..
            } => iced_widget::text("\u{e04f}")
                .font(Font::with_name("Material Symbols Rounded"))
                .into(),
            State::Ok {
                volume: Some(volume),
                mute: Some(false),
            }
            | State::Ok {
                volume: Some(volume),
                mute: None,
            } => {
                let volume = volume.cbrt() * 100.0;
                iced_widget::row![
                    iced_widget::text(if volume <= 0.0 {
                        "\u{e04e}"
                    } else if volume < 50.0 {
                        "\u{e04d}"
                    } else {
                        "\u{e050}"
                    })
                    .font(Font::with_name("Material Symbols Rounded")),
                    iced_widget::text!("{:.0}", volume),
                ]
                .into()
            }
            State::Ok {
                volume: None,
                mute: Some(false),
            }
            | State::Ok {
                volume: None,
                mute: None,
            } => iced_widget::text("?").into(),
            State::Err { message } => iced_widget::text(message).into(),
        };
        iced_widget::mouse_area(iced_widget::container(widget).widget_padding())
            .on_press(Message::LaunchSettings)
            .into()
    }

    fn subscription(&self) -> impl Into<Subscription<Self::Message>> {
        match self.state {
            State::Ok { .. } => Subscription::run(|| {
                let (tx, rx) = mpsc::unbounded();
                futures::stream_select!(
                    Box::pin(async move {
                        if let Err(e) = tokio::task::spawn_blocking(move || eucalyptus_root::audio::task(tx)).await {
                            tracing::error!(%e, "Join error");
                        }
                    }.into_stream().filter_map(async |_| None)),
                    rx.map(|message| match message {
                        eucalyptus_root::audio::Message::NewVolume(v) => Message::NewVolume(v),
                        eucalyptus_root::audio::Message::NewMute(m) => Message::NewMute(m),
                        eucalyptus_root::audio::Message::NewVolumeAndMute(v, m) => Message::NewVolumeAndMute(v, m),
                        eucalyptus_root::audio::Message::Error(e) => Message::Error(e),
                    })
                )
            }),
            State::Err { .. } => Subscription::none(),
        }
    }
}

#[derive(Clone, Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Config {
    pub settings_command: Option<Box<[String]>>,
}

#[derive(Clone)]
pub enum Message {
    NewVolume(Option<f32>),
    NewMute(Option<bool>),
    NewVolumeAndMute(Option<f32>, Option<bool>),
    LaunchSettings,
    Error(String),
}
