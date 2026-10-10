//! Feature-gated native surface retirement/retry regression.
//! Reads acquired swapchain textures and presents both retained windows.
use super::{RenderApp, SharedImageRenderState};
use crate::thread_comm::RenderComms;
use std::{
    os::unix::net::UnixStream,
    sync::{Arc, Mutex},
};
use winit::{
    application::ApplicationHandler,
    event::WindowEvent,
    event_loop::{ActiveEventLoop, EventLoop},
    platform::wayland::{EventLoopBuilderExtWayland, WaylandConnection},
    window::WindowId,
};

thread_local! {
    static FAIL_INIT: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
    static FAIL_SECONDARY: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}
// One-shot native fault injection. Compiled only with gui-test-hooks on Linux.
pub(super) fn fail_init_once() -> bool {
    FAIL_INIT.with(|flag| flag.replace(false))
}
pub(super) fn fail_secondary_once() -> bool {
    FAIL_SECONDARY.with(|flag| flag.replace(false))
}

pub struct NativeImageProbe<'a> {
    app: &'a mut RenderApp,
    owner: &'a dyn ActiveEventLoop,
}
impl NativeImageProbe<'_> {
    pub fn reset(&mut self) {
        // A real second native window exercises the same retirement boundary.
        self.app.frame_windows.request_create(
            0x23,
            400,
            300,
            "recovery secondary".into(),
            neovm_core::window::GuiFrameGeometryHints {
                base_width: 0,
                base_height: 0,
                min_width: 1,
                min_height: 1,
                width_inc: 1,
                height_inc: 1,
            },
        );
        let gpu = self.app.gpu.as_ref().unwrap();
        self.app.frame_windows.process_creates(
            self.owner,
            &self.app.comms,
            &mut self.app.window_icon,
            &gpu.instance,
            &gpu.device,
            &gpu.adapter,
        );
        assert_eq!(self.app.frame_windows.count(), 2);
        let identities: Vec<_> = self
            .app
            .frame_windows
            .windows
            .values()
            .map(|ws| ws.window().unwrap().id())
            .collect();
        self.present_color(wgpu::Color::RED, [0, 0, 255, 255]);
        let mode = std::env::var("NEOMACS_TEST_RECOVERY_RETRY").unwrap_or_default();
        if mode == "primary" {
            FAIL_INIT.with(|flag| flag.set(true));
        }
        if mode == "secondary" {
            FAIL_SECONDARY.with(|flag| flag.set(true));
        }
        self.app.recover_from_device_loss(self.owner);
        if mode == "primary" {
            assert!(
                self.app.gpu.is_none(),
                "injected primary initialization failure"
            );
            assert!(
                self.app.frame_windows.windows.values().all(|ws| ws
                    .lifecycle
                    .native()
                    .unwrap()
                    .surface
                    .is_none())
            );
        }
        if mode == "secondary" {
            assert!(
                self.app
                    .frame_windows
                    .get(0x23)
                    .unwrap()
                    .lifecycle
                    .native()
                    .unwrap()
                    .surface
                    .is_none(),
                "injected secondary initialization failure"
            );
        }
        if !mode.is_empty() {
            self.app.handle_about_to_wait(self.owner);
            assert!(
                self.app.gpu.is_some(),
                "failed recovery must retry through production event loop"
            );
            assert!(
                self.app.frame_windows.windows.values().all(|ws| ws
                    .lifecycle
                    .native()
                    .unwrap()
                    .surface
                    .is_some()),
                "all retained surfaces must recover on retry"
            );
            eprintln!("R023 RECOVERY_RETRY_PASS mode={mode}");
        }
        let mut restored: Vec<_> = self
            .app
            .frame_windows
            .windows
            .values()
            .map(|ws| ws.window().unwrap().id())
            .collect();
        let mut identities = identities;
        identities.sort();
        restored.sort();
        assert_eq!(restored, identities, "native windows remain identical");
        assert!(
            self.app.renderer.is_some(),
            "native GPU recovery must succeed"
        );
        self.present_color(wgpu::Color::GREEN, [0, 255, 0, 255]);
    }
    // Observe an acquired swapchain image, not the media cache; submit and
    // actually present it on the retained native window before and after reset.
    fn present_color(&mut self, color: wgpu::Color, expected_bgra: [u8; 4]) {
        let gpu = self.app.gpu.as_ref().unwrap();
        for ws in self.app.frame_windows.windows.values_mut() {
            let super::frame_windows::FrameLifecycle::Active { native, .. } = &mut ws.lifecycle
            else {
                panic!("native window required");
            };
            assert_eq!(
                native.surface_config.format,
                wgpu::TextureFormat::Bgra8UnormSrgb
            );
            let surface = native.surface.as_ref().expect("recovered surface");
            assert!(
                surface
                    .get_capabilities(&gpu.adapter)
                    .usages
                    .contains(wgpu::TextureUsages::COPY_SRC)
            );
            native.surface_config.usage |= wgpu::TextureUsages::COPY_SRC;
            surface.configure(&gpu.device, &native.surface_config);
            let output = match surface.get_current_texture() {
                wgpu::CurrentSurfaceTexture::Success(output)
                | wgpu::CurrentSurfaceTexture::Suboptimal(output) => output,
                other => panic!("native recovery presentation acquire: {other:?}"),
            };
            let view = output.texture.create_view(&Default::default());
            let buffer = gpu.device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("recovery swapchain readback"),
                size: 256,
                usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
                mapped_at_creation: false,
            });
            let mut encoder = gpu.device.create_command_encoder(&Default::default());
            {
                let _pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                    label: Some("recovery native color"),
                    color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                        view: &view,
                        resolve_target: None,
                        depth_slice: None,
                        ops: wgpu::Operations {
                            load: wgpu::LoadOp::Clear(color),
                            store: wgpu::StoreOp::Store,
                        },
                    })],
                    ..Default::default()
                });
            }
            encoder.copy_texture_to_buffer(
                wgpu::TexelCopyTextureInfo {
                    texture: &output.texture,
                    mip_level: 0,
                    origin: wgpu::Origin3d::ZERO,
                    aspect: wgpu::TextureAspect::All,
                },
                wgpu::TexelCopyBufferInfo {
                    buffer: &buffer,
                    layout: wgpu::TexelCopyBufferLayout {
                        offset: 0,
                        bytes_per_row: Some(256),
                        rows_per_image: Some(1),
                    },
                },
                wgpu::Extent3d {
                    width: 1,
                    height: 1,
                    depth_or_array_layers: 1,
                },
            );
            gpu.queue.submit(Some(encoder.finish()));
            let (tx, rx) = crossbeam_channel::bounded(1);
            buffer
                .slice(..)
                .map_async(wgpu::MapMode::Read, move |result| {
                    tx.send(result).unwrap();
                });
            gpu.device
                .poll(wgpu::PollType::Wait {
                    submission_index: None,
                    timeout: Some(std::time::Duration::from_secs(5)),
                })
                .unwrap();
            rx.recv_timeout(std::time::Duration::from_secs(5))
                .unwrap()
                .unwrap();
            let mapped = buffer.slice(..).get_mapped_range().unwrap();
            assert_eq!(
                &mapped[..4],
                &expected_bgra,
                "actual acquired swapchain color"
            );
            drop(mapped);
            buffer.unmap();
            native.window.pre_present_notify();
            gpu.queue.present(output);
            eprintln!(
                "R023 SWAPCHAIN_READBACK_PRESENT bgra={expected_bgra:?} window={:?}",
                native.window.id()
            );
        }
    }
}

/// Caller must select a disposable compositor, never the desktop's socket.
/// The callback runs after genuine native surface/device/renderer creation.
pub fn with_native_image_probe(
    socket: &str,
    render: RenderComms,
    metadata: SharedImageRenderState,
    probe: impl FnOnce(&mut NativeImageProbe<'_>) + 'static,
) {
    let connection = WaylandConnection::from_socket(UnixStream::connect(socket).unwrap()).unwrap();
    let mut builder = EventLoop::builder();
    builder
        .with_any_thread(true)
        .with_wayland_connection(connection);
    let event_loop = builder.build().unwrap();
    let mut app = RenderApp::new(
        render,
        800,
        600,
        "native image probe".into(),
        metadata,
        Arc::new((Mutex::new(Vec::new()), std::sync::Condvar::new())),
        true,
        #[cfg(feature = "neo-term")]
        crate::terminal::new_shared_terminals(),
    );
    app.comms.keep_alive_without_frames = true;
    struct Probe<F> {
        app: RenderApp,
        callback: Option<F>,
        done: Arc<std::sync::atomic::AtomicBool>,
    }
    impl<F: FnOnce(&mut NativeImageProbe<'_>)> ApplicationHandler for Probe<F> {
        fn window_event(&mut self, _: &dyn ActiveEventLoop, _: WindowId, _: WindowEvent) {}
        fn can_create_surfaces(&mut self, owner: &dyn ActiveEventLoop) {
            let window = Arc::from(owner.create_window(Default::default()).unwrap());
            self.app.init_wgpu(owner, window);
            assert!(self.app.renderer.is_some(), "real native renderer required");
            eprintln!(
                "native image adapter: {:?}",
                self.app.gpu.as_ref().unwrap().adapter.get_info()
            );
            (self.callback.take().unwrap())(&mut NativeImageProbe {
                app: &mut self.app,
                owner,
            });
            self.app.handle_exiting();
            self.done.store(true, std::sync::atomic::Ordering::Release);
            owner.exit();
        }
    }
    let completed = Arc::new(std::sync::atomic::AtomicBool::new(false));
    event_loop
        .run_app(Probe {
            app,
            callback: Some(probe),
            done: completed.clone(),
        })
        .unwrap();
    assert!(completed.load(std::sync::atomic::Ordering::Acquire));
}

#[cfg(test)]
#[test]
#[ignore = "requires private NEOMACS_TEST_WAYLAND_SOCKET and real GPU"]
fn native_recovery_retained_windows_swapchain_pixels() {
    let socket = std::env::var("NEOMACS_TEST_WAYLAND_SOCKET").expect("private compositor required");
    let (emacs, render) = crate::thread_comm::ThreadComms::new().split();
    with_native_image_probe(
        &socket,
        render,
        Arc::new(super::ImageRenderState::default()),
        move |probe| {
            probe.reset();
            assert!(
                emacs
                    .input_rx
                    .try_iter()
                    .any(|event| matches!(event, crate::thread_comm::InputEvent::DisplayReset))
            );
        },
    );
}
