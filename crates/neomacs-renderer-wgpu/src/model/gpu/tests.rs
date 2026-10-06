use super::*;
use crate::model::obj::parse_obj;
use crate::model::scene::remaining_limits;

/// A headless device, or `None` to skip; `NEOMACS_MODEL_REQUIRE_EVIDENCE=1`
/// makes a missing adapter a failure instead of a silent pass.
fn try_device() -> Option<(wgpu::Device, wgpu::Queue)> {
    let device = open_device();
    assert!(
        device.is_some() || std::env::var_os("NEOMACS_MODEL_REQUIRE_EVIDENCE").is_none(),
        "GPU evidence required but no headless wgpu adapter"
    );
    device
}

fn open_device() -> Option<(wgpu::Device, wgpu::Queue)> {
    open_device_with(wgpu::Limits::default())
}

fn open_device_with(required_limits: wgpu::Limits) -> Option<(wgpu::Device, wgpu::Queue)> {
    let instance =
        wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle_from_env());
    let adapter =
        pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions::default()))
            .ok()?;
    pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor {
        label: Some("model-gpu-test"),
        required_features: wgpu::Features::empty(),
        required_limits,
        memory_hints: Default::default(),
        experimental_features: wgpu::ExperimentalFeatures::disabled(),
        trace: wgpu::Trace::Off,
    }))
    .ok()
}

/// Test-only readback of the resolved texture as tightly packed RGBA8.
fn read_rgba(device: &wgpu::Device, queue: &wgpu::Queue, gpu: &ModelGpu) -> Vec<u8> {
    let (w, h) = (gpu.width_px, gpu.height_px);
    let row = (w * 4).div_ceil(256) * 256;
    let buffer = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("model readback"),
        size: (row * h) as u64,
        usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
        mapped_at_creation: false,
    });
    let mut encoder = device.create_command_encoder(&Default::default());
    encoder.copy_texture_to_buffer(
        wgpu::TexelCopyTextureInfo {
            texture: &gpu.texture,
            mip_level: 0,
            origin: wgpu::Origin3d::ZERO,
            aspect: wgpu::TextureAspect::All,
        },
        wgpu::TexelCopyBufferInfo {
            buffer: &buffer,
            layout: wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(row),
                rows_per_image: Some(h),
            },
        },
        wgpu::Extent3d {
            width: w,
            height: h,
            depth_or_array_layers: 1,
        },
    );
    queue.submit(Some(encoder.finish()));
    buffer
        .slice(..)
        .map_async(wgpu::MapMode::Read, |r| r.unwrap());
    device
        .poll(wgpu::PollType::Wait {
            submission_index: None,
            timeout: Some(std::time::Duration::from_secs(10)),
        })
        .unwrap();
    let mapped = buffer.slice(..).get_mapped_range().unwrap();
    let mut out = Vec::with_capacity((w * h * 4) as usize);
    for y in 0..h {
        let start = (y * row) as usize;
        out.extend_from_slice(&mapped[start..start + (w * 4) as usize]);
    }
    out
}

fn pixel(rgba: &[u8], w: u32, x: u32, y: u32) -> [u8; 4] {
    let i = ((y * w + x) * 4) as usize;
    [rgba[i], rgba[i + 1], rgba[i + 2], rgba[i + 3]]
}

#[test]
fn rotation_is_ry_times_rx() {
    let r = ModelView {
        yaw: 90.0,
        pitch: 0.0,
        zoom: 1.0,
    }
    .rotation();
    // Ry(90) maps +x to -z and +z to +x.
    let apply = |v: [f64; 3]| [0, 1, 2].map(|i| r[i][0] * v[0] + r[i][1] * v[1] + r[i][2] * v[2]);
    let x = apply([1.0, 0.0, 0.0]);
    assert!((x[2] + 1.0).abs() < 1e-12 && x[0].abs() < 1e-12);
    let r = ModelView {
        yaw: 0.0,
        pitch: 90.0,
        zoom: 1.0,
    }
    .rotation();
    // Rx(90) maps +y to +z.
    let y = [0, 1, 2].map(|i| r[i][1]);
    assert!((y[2] - 1.0).abs() < 1e-12);
}

#[test]
fn clip_scale_fits_the_shorter_axis() {
    let v = ModelView::default();
    assert_eq!(clip_scale(&v, 100, 100), [0.8, 0.8]);
    assert_eq!(clip_scale(&v, 200, 100), [0.4, 0.8]);
    assert_eq!(clip_scale(&v, 100, 200), [0.8, 0.4]);
}

/// Regression (Sol slice review MUST 3): clamping each edge on its own
/// turned a 2048x512 model at scale 2 into a 2048x1024 texture, so a
/// circle composited into the 4:1 slot came out twice as wide.
#[test]
fn target_size_keeps_the_logical_aspect_when_capped() {
    assert_eq!(model_target_px(2048, 512, 2.0, 2048), (2048, 512));
    assert_eq!(model_target_px(512, 2048, 2.0, 2048), (512, 2048));
    assert_eq!(model_target_px(520, 520, 1.5, 2048), (780, 780));
    assert_eq!(model_target_px(300, 200, 1.0, 2048), (300, 200));
    assert_eq!(
        model_target_px(1, 10_000, 1.0, 2048),
        (1, 2048),
        "never zero"
    );
    assert_eq!(
        model_target_px(100, 50, f32::NAN, 2048),
        (100, 50),
        "bad scale is 1"
    );
    for (w, h, s) in [
        (2048u32, 512u32, 2.0f32),
        (1500, 700, 3.0),
        (4000, 3000, 1.25),
    ] {
        let (pw, ph) = model_target_px(w, h, s, 2048);
        let (logical, physical) = (w as f64 / h as f64, pw as f64 / ph as f64);
        assert!(pw <= 2048 && ph <= 2048);
        assert!(
            (physical / logical - 1.0).abs() < 0.01,
            "{w}x{h}@{s} -> {pw}x{ph}"
        );
    }
}

/// Targets beyond the device's texture limit are an error, never a wgpu
/// validation panic.
#[test]
fn oversized_target_is_an_error() {
    let Some((device, queue)) = try_device() else {
        eprintln!("skipping: no headless wgpu adapter");
        return;
    };
    let mesh = parse_obj(
        b"v 0 0 0\nv 1 0 0\nv 0 1 0\nf 1 2 3\n",
        remaining_limits(&[]),
    )
    .unwrap();
    let scene = ModelScene::new(vec![mesh]).unwrap();
    let over = device.limits().max_texture_dimension_2d + 1;
    assert!(ModelGpu::new(&device, &queue, &scene, over, 4).is_err());
    assert!(ModelGpu::new(&device, &queue, &scene, 4, 0).is_err());
}

/// Regression (Sol slice review r2 MUST 1): a failed GPU allocation must
/// come back as an error, never unwind.  A destroyed device fails every
/// allocation; `create_buffer_init` then panicked mapping the invalid
/// buffer, inside the error scopes that were meant to catch it.
#[test]
fn failed_allocation_is_an_error_not_a_panic() {
    let Some((device, queue)) = try_device() else {
        eprintln!("skipping: no headless wgpu adapter");
        return;
    };
    let mesh = parse_obj(
        b"v 0 0 0\nv 1 0 0\nv 0 1 0\nf 1 2 3\n",
        remaining_limits(&[]),
    )
    .unwrap();
    let scene = ModelScene::new(vec![mesh]).unwrap();
    device.destroy();
    let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let gpu = ModelGpu::new(&device, &queue, &scene, 16, 16)?;
        gpu.render(
            &device,
            &queue,
            &ModelView::default(),
            None,
            [0.0, 0.0, 0.0, 1.0],
        )
    }));
    assert!(
        outcome.is_ok(),
        "allocation failure unwound instead of returning Err"
    );
}

/// An adapter without vertex-stage storage buffers (WebGL2-level limits)
/// rejects the mesh pass as an error, and the device stays usable.
#[test]
fn missing_vertex_storage_is_an_error_and_the_device_survives() {
    let Some((device, queue)) = open_device_with(wgpu::Limits::downlevel_webgl2_defaults()) else {
        eprintln!("skipping: no headless wgpu adapter");
        return;
    };
    let mesh = parse_obj(
        b"v 0 0 0\nv 1 0 0\nv 0 1 0\nf 1 2 3\n",
        remaining_limits(&[]),
    )
    .unwrap();
    let scene = ModelScene::new(vec![mesh]).unwrap();
    let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        ModelGpu::new(&device, &queue, &scene, 16, 16).map(|_| ())
    }));
    assert!(
        matches!(outcome, Ok(Err(_))),
        "no vertex storage must be Err (unwound: {}, ok: {})",
        outcome.is_err(),
        matches!(outcome, Ok(Ok(())))
    );
    // The device still serves ordinary work after the rejected scope.
    let buffer = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("after"),
        size: 16,
        usage: wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    });
    queue.write_buffer(&buffer, 0, &[0u8; 16]);
    queue.submit(None);
}

#[test]
fn view_validation_matches_gnosis() {
    assert!(
        ModelView {
            yaw: 0.0,
            pitch: -90.0,
            zoom: 1.1
        }
        .validated()
        .is_ok()
    );
    assert!(
        ModelView {
            yaw: f64::NAN,
            pitch: 0.0,
            zoom: 1.0
        }
        .validated()
        .is_err()
    );
    assert!(
        ModelView {
            yaw: 0.0,
            pitch: 0.0,
            zoom: 0.2
        }
        .validated()
        .is_err()
    );
    assert!(
        ModelView {
            yaw: 2e6,
            pitch: 0.0,
            zoom: 1.0
        }
        .validated()
        .is_err()
    );
}

#[test]
fn nearer_triangle_wins_and_background_clears() {
    let Some((device, queue)) = try_device() else {
        eprintln!("skipping: no headless wgpu adapter");
        return;
    };
    // Two overlapping triangles: the near one (z = +1, towards the viewer)
    // is mesh 0 and drawn first, the far one (z = -1) is mesh 1 and drawn
    // last, so only the depth test keeps the near one on top; the selected
    // far mesh must not show.
    let far = parse_obj(
        b"v -1 -1 -1\nv 1 -1 -1\nv 0 1 -1\nf 1 2 3\n",
        remaining_limits(&[]),
    )
    .unwrap();
    let near = parse_obj(
        b"v -1 -1 1\nv 1 -1 1\nv 0 1 1\nf 1 2 3\n",
        remaining_limits(&[]),
    )
    .unwrap();
    let scene = ModelScene::new(vec![near, far]).unwrap();
    let gpu = ModelGpu::new(&device, &queue, &scene, 64, 64).unwrap();
    gpu.render(
        &device,
        &queue,
        &ModelView::default(),
        Some(1),
        [0.0, 0.0, 1.0, 1.0],
    )
    .unwrap();
    let rgba = read_rgba(&device, &queue, &gpu);
    let centre = pixel(&rgba, 64, 32, 34);
    // Facing normal (0,0,1): light = 0.28 + 0.72 * |n.l| with
    // l = normalize(-0.4,0.6,1) -> 0.28 + 0.72 * 0.8111 = 0.864.
    // bone red 0.92 * 0.864 = 0.795 -> 203 after sRGB encode of its decode.
    assert!(
        (centre[0] as i32 - 203).abs() <= 2,
        "near (unselected) bone triangle on top: {centre:?}"
    );
    assert!(centre[2] < 160, "not the blue background: {centre:?}");
    assert_eq!(
        pixel(&rgba, 64, 0, 0),
        [0, 0, 255, 255],
        "corner is background"
    );
}

/// The background is an Emacs (sRGB) colour like every face colour, so the
/// texture holds its bytes unchanged.  Regression: the slice GUI proof showed
/// `[0.02 0.02 0.03]` as srgb(39,39,48) because the clear colour was taken
/// as linear and encoded a second time.
#[test]
fn background_is_srgb_and_lands_as_its_own_bytes() {
    let Some((device, queue)) = try_device() else {
        eprintln!("skipping: no headless wgpu adapter");
        return;
    };
    let mesh = parse_obj(
        b"v 0 0 0\nv 1 0 0\nv 0 1 0\nf 1 2 3\n",
        remaining_limits(&[]),
    )
    .unwrap();
    let scene = ModelScene::new(vec![mesh]).unwrap();
    let gpu = ModelGpu::new(&device, &queue, &scene, 16, 16).unwrap();
    gpu.render(
        &device,
        &queue,
        &ModelView::default(),
        None,
        [0.02, 0.02, 0.03, 1.0],
    )
    .unwrap();
    let rgba = read_rgba(&device, &queue, &gpu);
    let corner = pixel(&rgba, 16, 0, 0);
    for (got, want) in corner.iter().zip([5u8, 5, 8, 255]) {
        assert!(
            (*got as i32 - want as i32).abs() <= 1,
            "corner {corner:?} != sRGB bytes [5, 5, 8, 255]"
        );
    }
}

/// Render the real Gnosis scene to a PNG for visual evidence, when present.
/// `NEOMACS_MODEL_PNG_SCENE` is a directory of OBJ files (rendered in name
/// order) and `NEOMACS_MODEL_PNG_OUT` the PNG to write.
#[test]
fn render_real_scene_png() {
    let (Ok(dir), Ok(out)) = (
        std::env::var("NEOMACS_MODEL_PNG_SCENE"),
        std::env::var("NEOMACS_MODEL_PNG_OUT"),
    ) else {
        assert!(
            std::env::var_os("NEOMACS_MODEL_REQUIRE_EVIDENCE").is_none(),
            "render evidence required but NEOMACS_MODEL_PNG_* unset"
        );
        eprintln!("skipping: no real scene");
        return;
    };
    let Some((device, queue)) = try_device() else {
        panic!("no wgpu adapter for the real-scene render");
    };
    let mut paths: Vec<_> = std::fs::read_dir(&dir)
        .unwrap()
        .map(|e| e.unwrap().path())
        .filter(|p| p.extension().is_some_and(|e| e == "obj"))
        .collect();
    paths.sort();
    let never = || false;
    let mut meshes = Vec::new();
    for path in &paths {
        let mesh =
            crate::model::read_obj_file(path, None, remaining_limits(&meshes), &never).unwrap();
        meshes.push(mesh);
    }
    let scene = ModelScene::new(meshes).unwrap();
    let gpu = ModelGpu::new(&device, &queue, &scene, 768, 768).unwrap();
    let view = ModelView {
        yaw: 0.0,
        pitch: -90.0,
        zoom: 1.1,
    };
    gpu.render(&device, &queue, &view, Some(1), [0.0, 0.0, 0.0, 1.0])
        .unwrap();
    let rgba = read_rgba(&device, &queue, &gpu);
    image::save_buffer(&out, &rgba, 768, 768, image::ColorType::Rgba8).unwrap();
    eprintln!(
        "wrote {out}: {} meshes, {} triangles, {} GPU bytes",
        scene.meshes.len(),
        scene.triangle_count(),
        gpu.bytes
    );
}
