use std::{
    env,
    path::{Path, PathBuf},
    sync::LazyLock,
    time::Duration,
};

use eucalyptus_cellulose::{
    Element,
    action::WindowSettings,
    task::{Task, TaskExt},
};
use futures::{FutureExt, Sink, SinkExt, StreamExt, channel::mpsc};
use iced_futures::Subscription;
use smithay_client_toolkit::shell::wlr_layer;
use tokio::{io::AsyncReadExt, net::UnixListener};

use crate::{
    config::Config,
    item::{self, Item, ItemKind, power_profile::PowerProfile},
};

pub static SOCKET_PATH: LazyLock<PathBuf> = LazyLock::new(|| {
    let socket_path = Path::new("eucalyptus-gumnut/daemon.sock");
    match env::var_os("XDG_RUNTIME_DIR") {
        Some(runtime_dir) => Path::new(&runtime_dir).join(socket_path),
        None => Path::new("/run/user")
            .join(nix::unistd::getuid().to_string())
            .join(socket_path),
    }
});

pub fn start() {
    let config = match Config::load() {
        Ok(x) => {
            tracing::info!("Load config from {:?}", crate::config::DEFAULT_PATH);
            x
        }
        Err(e) => {
            tracing::error!(error = %e, "Failed to load config, fallback to default");
            Config::default()
        }
    };

    let (state, task) = State::new(config);
    eucalyptus_cellulose::Application::new(
        state,
        task,
        eucalyptus_cellulose::Settings {
            theme: iced_core::Theme::KanagawaWave,
        },
    )
    .unwrap()
    .run()
    .unwrap();
}

const WIDTH: f32 = 600.0;
const HEIGHT: f32 = 60.0;

pub struct State {
    hide_delay: Duration,
    item: Option<ItemKind>,
    power_profile: PowerProfile,
    // backlight: Entity<Backlight>,
}

impl State {
    pub fn new(config: Config) -> (Self, Task<Message>) {
        let (power_profile, task) = PowerProfile::new(&config.item.power_profile);
        (
            Self {
                hide_delay: Duration::from_secs_f64(config.hide_delay),
                item: None,
                power_profile,
            },
            task.map_output(ItemMessage::PowerProfile)
                .map_output(Message::ItemMessage),
        )
    }
}

impl eucalyptus_cellulose::State for State {
    type Message = Message;

    fn update(&mut self, message: Self::Message) -> impl Into<Task<Self::Message>> {
        match message {
            Message::Show(item_kind) => {
                self.item = Some(item_kind);
                Task::none()
            }
            Message::OpenWindow => async {
                eucalyptus_cellulose::action::Action::OpenWindow(WindowSettings {
                    open_on_every_output: false,
                    layer: wlr_layer::Layer::Overlay,
                    namespace: Some("eucalyptus-gumnut".to_owned()),
                    anchor: wlr_layer::Anchor::BOTTOM
                        | wlr_layer::Anchor::LEFT
                        | wlr_layer::Anchor::RIGHT,
                    exclusive_zone: false,
                })
            }
            .into_stream()
            .boxed(),
            Message::Close => async { eucalyptus_cellulose::action::Action::CloseAllWindow }
                .into_stream()
                .boxed(),
            Message::Stop => {
                tracing::warn!("calling iced_runtime::exit()");
                async { eucalyptus_cellulose::action::Action::Exit }
                    .into_stream()
                    .boxed()
            }
            Message::ItemMessage(ItemMessage::PowerProfile(message)) => self
                .power_profile
                .update(message)
                .into()
                .map_output(ItemMessage::PowerProfile)
                .map_output(Message::ItemMessage),
        }
    }

    fn view(&self) -> impl Into<Element<'_, Self::Message>> {
        iced_widget::container(match self.item {
            Some(ItemKind::PowerProfile) => self
                .power_profile
                .view()
                .map(ItemMessage::PowerProfile)
                .map(Message::ItemMessage),
            Some(_) => todo!(),
            None => iced_widget::text("don't know what item to show").into(),
        })
        .style(|theme| iced_widget::container::Style {
            text_color: Some(theme.extended_palette().background.base.text),
            background: Some(theme.extended_palette().background.base.color.into()),
            border: iced_core::border::rounded(6),
            ..Default::default()
        })
    }

    fn subscription(&self) -> impl Into<Subscription<Self::Message>> {
        Subscription::batch([
            self.power_profile
                .subscription()
                .into()
                .map(ItemMessage::PowerProfile)
                .map(Message::ItemMessage),
            Subscription::run_with(self.hide_delay, |hide_delay| {
                let (tx, rx) = mpsc::unbounded();
                futures::stream_select!(
                    Box::pin(
                        daemon(tx, *hide_delay)
                            .into_stream()
                            .filter_map(async |_| None)
                    ),
                    rx,
                )
            }),
        ])
    }
}

pub enum Message {
    Show(ItemKind),
    OpenWindow,
    Close,
    Stop,
    ItemMessage(ItemMessage),
}

pub enum ItemMessage {
    PowerProfile(item::power_profile::Message),
}

async fn daemon<Tx>(mut tx: Tx, hide_delay: Duration)
where
    Tx: Sink<Message> + Unpin,
{
    tracing::info!("try to bind {SOCKET_PATH:?}");
    if let Some(parent) = SOCKET_PATH.parent() {
        tokio::fs::create_dir_all(parent).await.unwrap();
    }
    // FIXME: when the daemon dies, we need to clean up socket so that next one can use the same path
    // also need to handle when another daemon is running and the socket path is used by it (maybe need a "ping")
    let _ = tokio::fs::remove_file(SOCKET_PATH.as_path())
        .await
        .inspect_err(|e| tracing::warn!(%e, "Failed to remove old socket"));
    let listener = UnixListener::bind(SOCKET_PATH.as_path()).unwrap();
    tracing::info!("start listening at {SOCKET_PATH:?}");

    loop {
        let item_kind = loop {
            if let Action::Show(item_kind) = next_action(&listener).await {
                break item_kind;
            }
        };
        let _ = tx
            .send_all(&mut futures::stream::iter(
                [Message::OpenWindow, Message::Show(item_kind)].map(Result::Ok),
            ))
            .await;

        loop {
            futures::select! {
                action = next_action(&listener).fuse() => match action {
                    Action::Show(item_kind) => {
                        let _ = tx.send(Message::Show(item_kind)).await;
                    }
                    Action::Close => {
                        break;
                    }
                    Action::Stop => {
                        let _ = tx.send(Message::Stop).await;
                    }
                },
                _ = tokio::time::sleep(hide_delay).fuse() => break,
            }
        }
        let _ = tx.send(Message::Close).await;
    }
}

enum Action {
    Show(ItemKind),
    Close,
    Stop,
}

async fn next_action(listener: &UnixListener) -> Action {
    loop {
        match listener.accept().await {
            Ok((mut stream, _)) => {
                let mut buf = String::new();
                stream.read_to_string(&mut buf).await.unwrap();
                match buf.as_str() {
                    "show/power_profile" => {
                        tracing::info!("get show/power_profile");
                        break Action::Show(ItemKind::PowerProfile);
                    }
                    "show/backlight" => {
                        tracing::info!("get show/power_profile");
                        break Action::Show(ItemKind::Backlight);
                    }
                    "stop" => {
                        tracing::info!("get stop");
                        break Action::Stop;
                    }
                    message => {
                        tracing::warn!(message, "Unknown message from socket");
                    }
                }
            }
            Err(e) => {
                tracing::error!(%e, "Failed to accept connection from socket");
            }
        }
    }
}
