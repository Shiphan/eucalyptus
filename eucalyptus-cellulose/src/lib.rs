#![allow(unused)] // TODO: temporarily allow unused

use std::{
    collections::{HashMap, HashSet},
    fmt::Debug,
    num::NonZero,
    ops::Mul,
    ptr::NonNull,
    time::{Duration, Instant},
};

use futures::{
    SinkExt,
    StreamExt,
    channel::mpsc::{self, TryRecvError, UnboundedReceiver, UnboundedSender},
    stream::BoxStream,
};
use iced_core::{Font, Pixels, Point, Size, Theme};
use iced_futures::Subscription;
use iced_runtime::{UserInterface, user_interface};
use iced_wgpu::{Renderer, wgpu};
use raw_window_handle::{WaylandDisplayHandle, WaylandWindowHandle};
use smithay_client_toolkit::{
    compositor::{CompositorHandler, CompositorState, FrameCallbackData},
    output::{OutputHandler, OutputState},
    reexports::{
        calloop::EventLoop,
        calloop_wayland_source::WaylandSource,
        client::{
            Connection,
            Dispatch,
            Proxy,
            QueueHandle,
            globals::{BindError, registry_queue_init},
            protocol::{
                wl_keyboard::WlKeyboard,
                wl_output::{Transform, WlOutput},
                wl_pointer::WlPointer,
                wl_seat::WlSeat,
                wl_surface::WlSurface,
            },
        },
        protocols::wp::{
            fractional_scale::v1::client::{
                wp_fractional_scale_manager_v1::WpFractionalScaleManagerV1,
                wp_fractional_scale_v1::{self, WpFractionalScaleV1},
            },
            viewporter::client::{wp_viewport::WpViewport, wp_viewporter::WpViewporter},
        },
    },
    registry::{ProvidesRegistryState, RegistryState},
    seat::{
        self,
        Capability,
        SeatHandler,
        SeatState,
        keyboard::KeyboardHandler,
        pointer::{
            AxisScroll,
            BTN_BACK,
            BTN_FORWARD,
            BTN_LEFT,
            BTN_MIDDLE,
            BTN_RIGHT,
            PointerEvent,
            PointerEventKind,
            PointerHandler,
        },
    },
    shell::{
        WaylandSurface,
        wlr_layer::{self, LayerShell, LayerShellHandler, LayerSurface},
        xdg::{
            XdgShell,
            window::{Window, WindowConfigure, WindowHandler},
        },
    },
};

use crate::{
    action::{Action, WindowSettings},
    task::{Task, TaskExt},
};

pub mod action;
pub mod task;

const HEIGHT: u32 = 40;

pub type Element<'a, Message, Theme = iced_core::Theme, Renderer = iced_wgpu::Renderer> =
    iced_core::Element<'a, Message, Theme, Renderer>;

pub struct Application<'a, State, Message>
where
    Message: Send + 'static,
    State: self::State,
{
    state: State,
    boot_task: Task<Message>,
    wayland_event_loop: EventLoop<'a, WaylandClient>,
    wayland_client: WaylandClient,
    wayland_event_rx: UnboundedReceiver<WaylandEvent>,
    wgpu_instance: wgpu::Instance,
    wgpu_adapter: wgpu::Adapter,
    wgpu_device: wgpu::Device,
    wgpu_engine: iced_wgpu::Engine,
    surfaces: HashMap<WlSurface, SurfaceState>,
    clipboard: Clipboard,
    // pending_event: Vec<iced_core::Event>,
    iced_futures_runtime: iced_futures::Runtime<
        iced_futures::backend::default::Executor,
        UnboundedSender<Action<Message>>,
        Action<Message>,
    >,
    future_message_rx: UnboundedReceiver<Action<Message>>,
    window_settings_for_every_output: Vec<(WindowSettings, HashSet<WlOutput>)>,
}

impl<State, Message> Application<'_, State, Message>
where
    Message: Send + 'static,
    State: self::State<Message = Message>,
{
    pub fn new(state: State, boot_task: Task<Message>) -> Result<Self, Box<dyn std::error::Error>> {
        let (wayland_event_tx, wayland_event_rx) = mpsc::unbounded();
        let (wayland_client, wayland_event_loop) = WaylandClient::new(wayland_event_tx)?;

        let wgpu_instance = wgpu::Instance::new(&wgpu::InstanceDescriptor::from_env_or_default());

        let wgpu_adapter = futures::executor::block_on(
            wgpu_instance.request_adapter(&wgpu::RequestAdapterOptions::default()),
        )?;
        let (wgpu_device, wgpu_queue) = futures::executor::block_on(
            wgpu_adapter.request_device(&wgpu::wgt::DeviceDescriptor::default()),
        )?;

        let wgpu_engine = iced_wgpu::Engine::new(
            &wgpu_adapter,
            wgpu_device.clone(),
            wgpu_queue,
            wgpu::TextureFormat::Bgra8UnormSrgb,
            Some(iced_graphics::Antialiasing::MSAAx4),
            iced_graphics::Shell::headless(),
        );

        let (future_message_tx, future_message_rx) = mpsc::unbounded();
        let iced_futures_runtime = iced_futures::Runtime::new(
            iced_futures::backend::default::Executor::new()?,
            future_message_tx,
        );

        Ok(Self {
            state,
            boot_task,
            wayland_event_loop,
            wayland_client,
            wayland_event_rx,
            wgpu_instance,
            wgpu_adapter,
            wgpu_device,
            wgpu_engine,
            surfaces: HashMap::new(),
            clipboard: Clipboard,
            // pending_event: vec![],
            iced_futures_runtime,
            future_message_rx,
            window_settings_for_every_output: vec![],
        })
    }
    pub fn run(mut self) -> Result<(), Box<dyn std::error::Error>> {
        self.iced_futures_runtime
            .run(std::mem::replace(&mut self.boot_task, Task::none()));

        self.iced_futures_runtime
            .track(iced_futures::subscription::into_recipes(
                self.iced_futures_runtime
                    .enter(|| self.state.subscription().into())
                    .map(Action::Output),
            ));

        // TODO: Make it only wake up when needed
        loop {
            self.wayland_event_loop
                .dispatch(Some(Duration::from_millis(10)), &mut self.wayland_client)?;

            match self.wayland_event_rx.try_recv() {
                Ok(event) => {
                    self.on_wayland_event(event)?;
                }
                Err(TryRecvError::Empty) => (),
                Err(TryRecvError::Closed) => {
                    tracing::warn!("channel `wayland_event_rx` closed");
                }
            }

            match self.future_message_rx.try_recv() {
                Ok(Action::Output(message)) => {
                    let task = self.state.update(message).into();
                    self.iced_futures_runtime.run(task);
                }
                Ok(Action::OpenWindow(settings)) => {
                    if settings.open_on_every_output {
                        let outputs = self
                            .wayland_client
                            .output_state
                            .outputs()
                            .collect::<HashSet<_>>();
                        for output in outputs.iter() {
                            self.open_new_layer_surface(Some(output), &settings)?;
                        }
                        self.window_settings_for_every_output
                            .push((settings, outputs));
                    } else {
                        self.open_new_layer_surface(None, &settings)?;
                    }
                }
                Ok(Action::CloseAllWindow) => {
                    for surface in self.surfaces.keys() {
                        surface.destroy();
                    }
                }
                Ok(Action::Exit) => {
                    return Ok(());
                }
                Err(TryRecvError::Empty) => (),
                Err(TryRecvError::Closed) => {
                    tracing::warn!("channel `wayland_event_rx` closed");
                }
            }
        }
    }
    fn open_new_layer_surface(
        &mut self,
        wl_output: Option<&WlOutput>,
        settings: &WindowSettings,
    ) -> Result<(), Box<dyn std::error::Error>> {
        let layer_surface = self.wayland_client.create_layer_surface(
            wlr_layer::Layer::Top,
            settings.namespace.clone(),
            wl_output,
            None,
            NonZero::new(HEIGHT),
            Some(settings.anchor),
            settings.exclusive_zone.then_some(HEIGHT as _),
        )?;
        let wp_viewport = self.wayland_client.get_viewport(layer_surface.wl_surface());

        let wgpu_surface = {
            let raw_display_handle = WaylandDisplayHandle::new(
                NonNull::new(self.wayland_client.connection.backend().display_ptr() as _)
                    .ok_or("wayland display pointer is null")?,
            )
            .into();
            let raw_window_handle = WaylandWindowHandle::new(
                NonNull::new(layer_surface.wl_surface().id().as_ptr() as _)
                    .ok_or("wl_surface is null")?,
            )
            .into();

            unsafe {
                self.wgpu_instance
                    .create_surface_unsafe(wgpu::SurfaceTargetUnsafe::RawHandle {
                        raw_display_handle,
                        raw_window_handle,
                    })
            }?
        };

        let renderer = Renderer::new(self.wgpu_engine.clone(), Font::default(), Pixels(16.0));

        self.surfaces.insert(
            layer_surface.wl_surface().clone(),
            SurfaceState {
                wl_surface: layer_surface.wl_surface().clone(),
                layer_surface,
                wp_viewport,
                size: None,
                scale: SurfaceScale::BufferScale(1),
                buffer_size: None,
                wgpu_surface,
                renderer,
                user_interface_cache: user_interface::Cache::new(),
                view_port: None,
                cursor: iced_core::mouse::Cursor::Unavailable,
                pending_event: vec![],
                exclusive_zone: settings.exclusive_zone,
            },
        );

        Ok(())
    }
    fn draw(&mut self, surface: &WlSurface) {
        let Some((Some(width), Some(height))) =
            self.wayland_client.surfaces.get(surface).map(|x| x.size())
        else {
            return;
        };
        let Some(it) = self.surfaces.get_mut(surface) else {
            return;
        };
        let surface_texture = match it.wgpu_surface.get_current_texture() {
            Ok(x) => x,
            Err(e) => {
                tracing::warn!(error = %e, "wgpu_surface.get_current_texture() failed");
                it.wl_surface.frame(
                    &self.wayland_client.queue_handle,
                    FrameCallbackData(it.wl_surface.clone()),
                );
                it.wl_surface.commit();
                return;
            }
        };
        let texture_view = surface_texture
            .texture
            .create_view(&wgpu::TextureViewDescriptor::default());

        let mut user_interface = UserInterface::build(
            self.state.view(),
            Size::new(width.get() as _, height.get() as _),
            std::mem::take(&mut it.user_interface_cache),
            &mut it.renderer,
        );

        let mut messages = vec![];
        let (user_interface_state, _event_statuses) = user_interface.update(
            std::mem::take(&mut it.pending_event).as_slice(),
            it.cursor,
            &mut it.renderer,
            &mut self.clipboard,
            &mut messages,
        );

        // TODO: this will make the window fit content size, but might worth more refinement on the solution
        #[derive(Debug)]
        struct InspectBounds(Option<Size<u32>>);

        impl iced_core::widget::Operation for InspectBounds {
            fn traverse(
                &mut self,
                _operate: &mut dyn FnMut(&mut dyn iced_core::widget::Operation<()>),
            ) {
            }
            fn container(&mut self, _id: Option<&iced_widget::Id>, bounds: iced_core::Rectangle) {
                let size = bounds.size();
                self.0 = Some(Size::new(size.width as _, size.height as _));
            }
        }

        let mut inspect_bounds = InspectBounds(None);

        user_interface.operate(&it.renderer, &mut inspect_bounds);

        if let Some(bounds) = inspect_bounds.0
            && it
                .size
                .is_none_or(|(_width, height)| height.get() != bounds.height)
        {
            tracing::info!(?inspect_bounds, "bounds");
            it.layer_surface.set_size(0, bounds.height);
            if it.exclusive_zone {
                it.layer_surface.set_exclusive_zone(bounds.height as _);
            }
            if let (Some(width), Some(height)) =
                (NonZero::new(bounds.width), NonZero::new(bounds.height))
            {
                it.size = Some((width, height));
            }
        }
        // FIXME: call on_resize??

        user_interface.draw(
            &mut it.renderer,
            &Theme::KanagawaWave,
            &iced_core::renderer::Style::default(),
            it.cursor,
        );
        it.user_interface_cache = user_interface.into_cache();

        it.renderer.present(
            None,
            surface_texture.texture.format(),
            &texture_view,
            &iced_graphics::Viewport::with_physical_size(
                Size::new(width.get() * it.scale, height.get() * it.scale),
                it.scale.into(),
            ),
        );

        surface_texture.present();

        it.wl_surface.frame(
            &self.wayland_client.queue_handle,
            FrameCallbackData(it.wl_surface.clone()),
        );
        it.wl_surface.commit();

        for message in messages {
            let task = self.state.update(message).into();
            self.iced_futures_runtime.run(task);
        }

        self.iced_futures_runtime
            .track(iced_futures::subscription::into_recipes(
                self.iced_futures_runtime
                    .enter(|| self.state.subscription().into())
                    .map(Action::Output),
            ));
    }
    fn on_wayland_event(&mut self, event: WaylandEvent) -> Result<(), Box<dyn std::error::Error>> {
        match event {
            WaylandEvent::NewOutput(output) => {
                let settings = self
                    .window_settings_for_every_output
                    .iter_mut()
                    .filter_map(|(window_settings, outputs)| {
                        if window_settings.open_on_every_output && outputs.insert(output.clone()) {
                            Some(window_settings.clone())
                        } else {
                            None
                        }
                    })
                    .collect::<Vec<_>>();

                for settings in settings {
                    if settings.open_on_every_output {
                        self.open_new_layer_surface(Some(&output), &settings)?;
                    }
                }
            }
            WaylandEvent::OutputDestroyed(output) => {
                for (_, outputs) in &mut self.window_settings_for_every_output {
                    outputs.remove(&output);
                }
            }
            WaylandEvent::SurfaceConfiure {
                wl_surface,
                width,
                height,
            } => {
                if let Some(it) = self.surfaces.get_mut(&wl_surface) {
                    it.size = Some((width, height));

                    it.buffer_size = Some((width.get() * it.scale, height.get() * it.scale));
                    it.pending_event.push(iced_core::Event::Window(
                        iced_core::window::Event::Resized(Size::new(
                            width.get() as _,
                            height.get() as _,
                        )),
                    ));
                    it.on_resize(&self.wgpu_adapter, &self.wgpu_device);
                }
                self.draw(&wl_surface);
            }
            WaylandEvent::SurfaceFrame(surface) => {
                if let Some(it) = self.surfaces.get_mut(&surface) {
                    it.pending_event.push(iced_core::Event::Window(
                        iced_core::window::Event::RedrawRequested(Instant::now()),
                    ));
                }
                self.draw(&surface);
            }
            WaylandEvent::SurfaceClose(surface) => {
                self.surfaces.remove(&surface);
            }
            WaylandEvent::SurfaceScale { wl_surface, scale } => {
                if let Some(it) = self.surfaces.get_mut(&wl_surface) {
                    it.scale = scale;

                    if let Some((width, height)) = it.size {
                        it.buffer_size = Some((width.get() * scale, height.get() * scale));
                        it.on_resize(&self.wgpu_adapter, &self.wgpu_device);
                    };
                }
                // self.draw(&wl_surface);
            }
            WaylandEvent::PointerEvents(events) => {
                for event in events {
                    if let Some(it) = self.surfaces.get_mut(&event.surface) {
                        let position = Point::new(event.position.0 as _, event.position.1 as _);
                        match event.kind {
                            PointerEventKind::Enter { .. } | PointerEventKind::Motion { .. } => {
                                it.cursor = iced_core::mouse::Cursor::Available(position);
                            }
                            _ => (),
                        }

                        let button_from_u32 = |button| match button {
                            BTN_LEFT => iced_core::mouse::Button::Left,
                            BTN_RIGHT => iced_core::mouse::Button::Right,
                            BTN_MIDDLE => iced_core::mouse::Button::Middle,
                            BTN_BACK => iced_core::mouse::Button::Back,
                            BTN_FORWARD => iced_core::mouse::Button::Forward,
                            button => iced_core::mouse::Button::Other(button as _),
                        };
                        let mouse_event = match event.kind {
                            PointerEventKind::Enter { .. } => {
                                iced_core::mouse::Event::CursorEntered
                            }
                            PointerEventKind::Leave { .. } => iced_core::mouse::Event::CursorLeft,
                            PointerEventKind::Motion { .. } => {
                                iced_core::mouse::Event::CursorMoved { position }
                            }
                            PointerEventKind::Press { button, .. } => {
                                iced_core::mouse::Event::ButtonPressed(button_from_u32(button))
                            }
                            PointerEventKind::Release { button, .. } => {
                                iced_core::mouse::Event::ButtonReleased(button_from_u32(button))
                            }
                            PointerEventKind::Axis {
                                horizontal:
                                    AxisScroll {
                                        value120: horizontal_value120,
                                        ..
                                    },
                                vertical:
                                    AxisScroll {
                                        value120: vertical_value120,
                                        ..
                                    },
                                ..
                            } => iced_core::mouse::Event::WheelScrolled {
                                delta: iced_core::mouse::ScrollDelta::Lines {
                                    x: horizontal_value120 as f32 / 120.0,
                                    y: vertical_value120 as f32 / 120.0,
                                },
                            },
                        };
                        it.pending_event.push(iced_core::Event::Mouse(mouse_event));
                    }
                }
            }
        }

        Ok(())
    }
}

pub trait State: Sized {
    type Message: Send + 'static;

    fn update(&mut self, message: Self::Message) -> impl Into<Task<Self::Message>>;

    fn view(&self) -> impl Into<Element<'_, Self::Message>>;

    fn subscription(&self) -> impl Into<Subscription<Self::Message>> {
        Subscription::none()
    }
}

pub trait BootFn<State, Message> {
    fn boot(&self) -> (State, Task<Message>);
}

impl<T, State, Message, BootResult> BootFn<State, Message> for T
where
    T: Fn() -> BootResult,
    BootResult: Into<(State, Task<Message>)>,
{
    fn boot(&self) -> (State, Task<Message>) {
        self().into()
    }
}

pub trait UpdateFn<'a, State, Message> {
    fn update(&self, state: &'a mut State, message: Message) -> Task<Message>;
}

impl<'a, T, State, Message, UpdateResult> UpdateFn<'a, State, Message> for T
where
    T: Fn(&'a mut State, Message) -> UpdateResult,
    State: 'a,
    UpdateResult: Into<Task<Message>>,
{
    fn update(&self, state: &'a mut State, message: Message) -> Task<Message> {
        self(state, message).into()
    }
}

pub trait ViewFn<'a, State, Message> {
    fn view(&self, state: &'a State) -> Element<'a, Message>;
}

impl<'a, T, State, Message, ViewResult> ViewFn<'a, State, Message> for T
where
    T: Fn(&'a State) -> ViewResult,
    State: 'a,
    ViewResult: Into<Element<'a, Message>>,
{
    fn view(&self, state: &'a State) -> Element<'a, Message> {
        self(state).into()
    }
}

pub trait SubscriptionFn<'a, State, Message> {
    fn subscription(&self, state: &'a State) -> Subscription<Message>;
}

impl<'a, T, State, Message, SubscriptionResult> SubscriptionFn<'a, State, Message> for T
where
    T: Fn(&'a State) -> SubscriptionResult,
    State: 'a,
    SubscriptionResult: Into<Subscription<Message>>,
{
    fn subscription(&self, state: &'a State) -> Subscription<Message> {
        self(state).into()
    }
}

struct SurfaceState {
    wl_surface: WlSurface,
    layer_surface: LayerSurface,
    wp_viewport: WpViewport,
    size: Option<(NonZero<u32>, NonZero<u32>)>,
    scale: SurfaceScale,
    buffer_size: Option<(u32, u32)>,
    wgpu_surface: wgpu::Surface<'static>,
    renderer: iced_wgpu::Renderer,
    user_interface_cache: user_interface::Cache,
    view_port: Option<iced_graphics::Viewport>,
    cursor: iced_core::mouse::Cursor,
    pending_event: Vec<iced_core::Event>,
    exclusive_zone: bool,
}

impl SurfaceState {
    fn on_resize(&mut self, wgpu_adapter: &wgpu::Adapter, wgpu_device: &wgpu::Device) {
        if let Some((width, height)) = self.size {
            match self.scale {
                SurfaceScale::FractionalScale(scale) => {
                    self.wl_surface.set_buffer_scale(1);
                    self.wp_viewport
                        .set_destination(width.get() as _, height.get() as _);
                }
                SurfaceScale::BufferScale(scale) => {
                    self.wl_surface.set_buffer_scale(scale);
                    self.wp_viewport.set_destination(-1, -1);
                }
            }
        };
        if let Some((buffer_width, buffer_height)) = self.buffer_size {
            self.pending_event
                .push(iced_core::Event::Window(iced_core::window::Event::Resized(
                    Size::new(buffer_width as _, buffer_height as _),
                )));
            self.view_port = Some(iced_graphics::Viewport::with_physical_size(
                Size::new(buffer_width, buffer_height),
                self.scale.into(),
            ));

            let mut wgpu_surface_configuration = self
                .wgpu_surface
                .get_default_config(wgpu_adapter, buffer_width, buffer_height)
                .unwrap(); // TODO: remove unwrap

            wgpu_surface_configuration.alpha_mode = wgpu::CompositeAlphaMode::PreMultiplied;

            self.wgpu_surface
                .configure(wgpu_device, &wgpu_surface_configuration);
        };
    }
}

struct Clipboard;

impl iced_core::Clipboard for Clipboard {
    fn read(&self, kind: iced_core::clipboard::Kind) -> Option<String> {
        None
    }

    fn write(&mut self, kind: iced_core::clipboard::Kind, contents: String) {}
}

struct WaylandClient {
    event_tx: UnboundedSender<WaylandEvent>,
    connection: Connection,
    queue_handle: QueueHandle<Self>,
    registry_state: RegistryState,
    output_state: OutputState,
    seat_state: SeatState,
    fractional_scale_manager: Option<WpFractionalScaleManagerV1>,
    viewporter: WpViewporter,
    pointer: Option<WlPointer>,
    compositor_state: CompositorState,
    xdg_shell: XdgShell,
    layer_shell: LayerShell,
    surfaces: HashMap<WlSurface, Surface>,
}

impl WaylandClient {
    pub fn new<'a>(
        event_tx: UnboundedSender<WaylandEvent>,
    ) -> Result<(Self, EventLoop<'a, Self>), Box<dyn std::error::Error>> {
        let connection = Connection::connect_to_env()?;

        let (globals, event_queue) = registry_queue_init::<Self>(&connection)?;
        let qh = event_queue.handle();
        let event_loop = EventLoop::try_new()?;
        let loop_handle = event_loop.handle();
        WaylandSource::new(connection.clone(), event_queue).insert(loop_handle)?;

        let compositor_state = CompositorState::bind(&globals, &qh)?;
        let registry_state = RegistryState::new(&globals);
        let output_state = OutputState::new(&globals, &qh);
        let seat_state = SeatState::new(&globals, &qh);
        let xdg_shell = XdgShell::bind(&globals, &qh)?;
        let layer_shell = LayerShell::bind(&globals, &qh)?;
        let fractional_scale_manager = globals
            .bind::<WpFractionalScaleManagerV1, _, _>(&qh, 1..=1, ())
            .ok();
        let viewporter = globals.bind::<WpViewporter, _, _>(&qh, 1..=1, ())?;

        Ok((
            Self {
                event_tx,
                connection,
                queue_handle: qh,
                registry_state,
                output_state,
                seat_state,
                compositor_state,
                fractional_scale_manager,
                viewporter,
                pointer: None,
                xdg_shell,
                layer_shell,
                surfaces: HashMap::new(),
            },
            event_loop,
        ))
    }
    fn create_layer_surface(
        &mut self,
        layer: wlr_layer::Layer,
        namespace: Option<impl Into<String>>,
        output: Option<&WlOutput>,
        width: Option<NonZero<u32>>,
        height: Option<NonZero<u32>>,
        anchor: Option<wlr_layer::Anchor>,
        exclusive_zone: Option<i32>,
    ) -> Result<LayerSurface, Box<dyn std::error::Error>> {
        let wl_surface = self.compositor_state.create_surface(&self.queue_handle);
        let fractional_scale = self
            .fractional_scale_manager
            .as_ref()
            .map(|x| x.get_fractional_scale(&wl_surface, &self.queue_handle, wl_surface.clone()));
        let layer_surface = self.layer_shell.create_layer_surface(
            &self.queue_handle,
            wl_surface.clone(),
            layer,
            namespace,
            output,
        );
        layer_surface.set_size(
            width.map_or(0, NonZero::get),
            height.map_or(0, NonZero::get),
        );
        if let Some(anchor) = anchor {
            layer_surface.set_anchor(anchor);
        }
        if let Some(zone) = exclusive_zone {
            layer_surface.set_exclusive_zone(zone);
        }
        layer_surface.commit();

        self.surfaces
            .insert(
                wl_surface.clone(),
                Surface::LayerSurface(LayerSurfaceInfo::new(layer_surface.clone())),
            )
            .map_or(Ok(()), |surface| {
                Err("A new WlSurface should not already be in surfaces")
            })?;

        Ok(layer_surface)
    }
    fn get_viewport(&mut self, surface: &WlSurface) -> WpViewport {
        self.viewporter
            .get_viewport(&surface, &self.queue_handle, surface.clone())
    }
}

enum Surface {
    Window(WindowInfo),
    LayerSurface(LayerSurfaceInfo),
}

impl Surface {
    fn size(&self) -> (Option<NonZero<u32>>, Option<NonZero<u32>>) {
        match self {
            Self::Window(WindowInfo { width, height, .. }) => (*width, *height),
            Self::LayerSurface(LayerSurfaceInfo { width, height, .. }) => (*width, *height),
        }
    }
    fn commit(&self) {
        match self {
            Self::Window(WindowInfo { window, .. }) => {
                window.commit();
            }
            Self::LayerSurface(LayerSurfaceInfo { layer_surface, .. }) => {
                layer_surface.commit();
            }
        }
    }
}

struct WindowInfo {
    window: Window,
    width: Option<NonZero<u32>>,
    height: Option<NonZero<u32>>,
    started: bool,
}

impl WindowInfo {
    fn new(window: Window) -> Self {
        Self {
            window,
            width: None,
            height: None,
            started: false,
        }
    }
}

struct LayerSurfaceInfo {
    layer_surface: LayerSurface,
    width: Option<NonZero<u32>>,
    height: Option<NonZero<u32>>,
    started: bool,
}

impl LayerSurfaceInfo {
    fn new(layer_surface: LayerSurface) -> Self {
        Self {
            layer_surface,
            width: None,
            height: None,
            started: false,
        }
    }
}

enum WaylandEvent {
    NewOutput(WlOutput),
    OutputDestroyed(WlOutput),
    SurfaceConfiure {
        wl_surface: WlSurface,
        width: NonZero<u32>,
        height: NonZero<u32>,
    },
    SurfaceFrame(WlSurface),
    SurfaceClose(WlSurface),
    SurfaceScale {
        wl_surface: WlSurface,
        scale: SurfaceScale,
    },
    PointerEvents(Vec<PointerEvent>),
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum SurfaceScale {
    FractionalScale(u32),
    BufferScale(i32),
}

impl Mul<SurfaceScale> for u32 {
    type Output = Self;

    fn mul(self, rhs: SurfaceScale) -> Self::Output {
        match rhs {
            SurfaceScale::FractionalScale(scale) => (self as f64 * (scale as f64 / 120.0)) as _,
            SurfaceScale::BufferScale(scale) => self * scale as u32,
        }
    }
}

impl Mul<SurfaceScale> for i32 {
    type Output = Self;

    fn mul(self, rhs: SurfaceScale) -> Self::Output {
        match rhs {
            SurfaceScale::FractionalScale(scale) => (self as f64 * (scale as f64 / 120.0)) as _,
            SurfaceScale::BufferScale(scale) => self * scale,
        }
    }
}

impl Into<f64> for SurfaceScale {
    fn into(self) -> f64 {
        match self {
            SurfaceScale::FractionalScale(scale) => scale as f64 / 120.0,
            SurfaceScale::BufferScale(scale) => scale as _,
        }
    }
}

impl Into<f32> for SurfaceScale {
    fn into(self) -> f32 {
        match self {
            SurfaceScale::FractionalScale(scale) => scale as f32 / 120.0,
            SurfaceScale::BufferScale(scale) => scale as _,
        }
    }
}

smithay_client_toolkit::delegate_registry!(WaylandClient);
smithay_client_toolkit::delegate_dispatch2!(WaylandClient);

impl ProvidesRegistryState for WaylandClient {
    fn registry(&mut self) -> &mut RegistryState {
        &mut self.registry_state
    }
    smithay_client_toolkit::registry_handlers![OutputState, SeatState];
}

impl OutputHandler for WaylandClient {
    fn output_state(&mut self) -> &mut OutputState {
        &mut self.output_state
    }

    fn new_output(&mut self, _conn: &Connection, _qh: &QueueHandle<Self>, output: WlOutput) {
        let _ = futures::executor::block_on(self.event_tx.send(WaylandEvent::NewOutput(output)));
    }

    fn update_output(&mut self, _conn: &Connection, _qh: &QueueHandle<Self>, _output: WlOutput) {}

    fn output_destroyed(&mut self, _conn: &Connection, _qh: &QueueHandle<Self>, output: WlOutput) {
        let _ =
            futures::executor::block_on(self.event_tx.send(WaylandEvent::OutputDestroyed(output)));
    }
}

impl CompositorHandler for WaylandClient {
    fn scale_factor_changed(
        &mut self,
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
        surface: &WlSurface,
        new_factor: i32,
    ) {
        let _ = futures::executor::block_on(self.event_tx.send(WaylandEvent::SurfaceScale {
            wl_surface: surface.clone(),
            scale: SurfaceScale::BufferScale(new_factor),
        }));
    }

    fn transform_changed(
        &mut self,
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
        _surface: &WlSurface,
        _new_transform: Transform,
    ) {
    }

    fn frame(
        &mut self,
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
        surface: &WlSurface,
        _time: u32,
    ) {
        let _ = futures::executor::block_on(
            self.event_tx
                .send(WaylandEvent::SurfaceFrame(surface.clone())),
        );
    }

    fn surface_enter(
        &mut self,
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
        _surface: &WlSurface,
        _output: &WlOutput,
    ) {
    }

    fn surface_leave(
        &mut self,
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
        _surface: &WlSurface,
        _output: &WlOutput,
    ) {
    }
}

impl WindowHandler for WaylandClient {
    fn request_close(&mut self, _conn: &Connection, _qh: &QueueHandle<Self>, window: &Window) {
        let _ = futures::executor::block_on(
            self.event_tx
                .send(WaylandEvent::SurfaceClose(window.wl_surface().clone())),
        );
    }

    fn configure(
        &mut self,
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
        window: &Window,
        configure: WindowConfigure,
        _serial: u32,
    ) {
        let Some(Surface::Window(window_info)) = self.surfaces.get_mut(window.wl_surface()) else {
            return;
        };

        window_info.width = configure.new_size.0;
        window_info.height = configure.new_size.1;

        if let (Some(width), Some(height)) = configure.new_size {
            let _ =
                futures::executor::block_on(self.event_tx.send(WaylandEvent::SurfaceConfiure {
                    wl_surface: window.wl_surface().clone(),
                    width,
                    height,
                }));
        }
    }
}

impl LayerShellHandler for WaylandClient {
    fn closed(
        &mut self,
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
        layer: &wlr_layer::LayerSurface,
    ) {
        let _ = futures::executor::block_on(
            self.event_tx
                .send(WaylandEvent::SurfaceClose(layer.wl_surface().clone())),
        );
    }

    fn configure(
        &mut self,
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
        layer: &wlr_layer::LayerSurface,
        configure: wlr_layer::LayerSurfaceConfigure,
        _serial: u32,
    ) {
        let Some(Surface::LayerSurface(layer_surface_info)) =
            self.surfaces.get_mut(layer.wl_surface())
        else {
            return;
        };

        let width = NonZero::new(configure.new_size.0);
        let height = NonZero::new(configure.new_size.1);

        layer_surface_info.width = width;
        layer_surface_info.height = height;

        if let (Some(width), Some(height)) = (width, height) {
            let _ =
                futures::executor::block_on(self.event_tx.send(WaylandEvent::SurfaceConfiure {
                    wl_surface: layer.wl_surface().clone(),
                    width,
                    height,
                }));
        }
    }
}

impl Dispatch<WpFractionalScaleManagerV1, ()> for WaylandClient {
    fn event(
        state: &mut Self,
        proxy: &WpFractionalScaleManagerV1,
        event: <WpFractionalScaleManagerV1 as Proxy>::Event,
        data: &(),
        conn: &Connection,
        qhandle: &QueueHandle<Self>,
    ) {
        match event {
            _ => (),
        }
    }
}

impl Dispatch<WpFractionalScaleV1, WlSurface> for WaylandClient {
    fn event(
        state: &mut Self,
        proxy: &WpFractionalScaleV1,
        event: <WpFractionalScaleV1 as Proxy>::Event,
        data: &WlSurface,
        conn: &Connection,
        qhandle: &QueueHandle<Self>,
    ) {
        match event {
            wp_fractional_scale_v1::Event::PreferredScale { scale } => {
                let _ =
                    futures::executor::block_on(state.event_tx.send(WaylandEvent::SurfaceScale {
                        wl_surface: data.clone(),
                        scale: SurfaceScale::FractionalScale(scale),
                    }));
            }
            _ => (),
        }
    }
}

impl Dispatch<WpViewporter, ()> for WaylandClient {
    fn event(
        state: &mut Self,
        proxy: &WpViewporter,
        event: <WpViewporter as Proxy>::Event,
        data: &(),
        conn: &Connection,
        qhandle: &QueueHandle<Self>,
    ) {
        match event {
            _ => (),
        }
    }
}

impl Dispatch<WpViewport, WlSurface> for WaylandClient {
    fn event(
        state: &mut Self,
        proxy: &WpViewport,
        event: <WpViewport as Proxy>::Event,
        data: &WlSurface,
        conn: &Connection,
        qhandle: &QueueHandle<Self>,
    ) {
        match event {
            _ => (),
        }
    }
}

impl SeatHandler for WaylandClient {
    fn seat_state(&mut self) -> &mut SeatState {
        &mut self.seat_state
    }

    fn new_seat(&mut self, _conn: &Connection, _qh: &QueueHandle<Self>, _seat: WlSeat) {}

    fn new_capability(
        &mut self,
        _conn: &Connection,
        qh: &QueueHandle<Self>,
        seat: WlSeat,
        capability: Capability,
    ) {
        if capability == Capability::Pointer && self.pointer.is_none() {
            self.pointer = self.seat_state.get_pointer(qh, &seat).ok();
        }
    }

    fn remove_capability(
        &mut self,
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
        _seat: WlSeat,
        capability: Capability,
    ) {
        if capability == Capability::Pointer
            && let Some(pointer) = self.pointer.take()
        {
            pointer.release();
        }
    }

    fn remove_seat(&mut self, _conn: &Connection, _qh: &QueueHandle<Self>, _seat: WlSeat) {}
}

impl PointerHandler for WaylandClient {
    fn pointer_frame(
        &mut self,
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
        _pointer: &WlPointer,
        events: &[PointerEvent],
    ) {
        let _ = futures::executor::block_on(
            self.event_tx
                .send(WaylandEvent::PointerEvents(events.to_vec())),
        );
    }
}

impl KeyboardHandler for WaylandClient {
    fn enter(
        &mut self,
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
        _keyboard: &WlKeyboard,
        _surface: &WlSurface,
        _serial: u32,
        _raw: &[u32],
        _keysyms: &[seat::keyboard::Keysym],
    ) {
    }

    fn leave(
        &mut self,
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
        _keyboard: &WlKeyboard,
        _surface: &WlSurface,
        _serial: u32,
    ) {
    }

    fn press_key(
        &mut self,
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
        _keyboard: &WlKeyboard,
        _serial: u32,
        _event: seat::keyboard::KeyEvent,
    ) {
    }

    fn repeat_key(
        &mut self,
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
        _keyboard: &WlKeyboard,
        _serial: u32,
        _event: seat::keyboard::KeyEvent,
    ) {
    }

    fn release_key(
        &mut self,
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
        _keyboard: &WlKeyboard,
        _serial: u32,
        _event: seat::keyboard::KeyEvent,
    ) {
    }

    fn update_modifiers(
        &mut self,
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
        _keyboard: &WlKeyboard,
        _serial: u32,
        _modifiers: seat::keyboard::Modifiers,
        _raw_modifiers: seat::keyboard::RawModifiers,
        _layout: u32,
    ) {
    }
}
