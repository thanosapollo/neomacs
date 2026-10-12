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
    let (w, h) = gpu.size_px();
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
            texture: &gpu.targets.texture,
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

/// A plain draw: `view`, square-or-not logical size, no highlight.
fn draw(view: ModelView, logical: (u32, u32), background: [f32; 4]) -> ModelDraw {
    ModelDraw {
        view,
        logical,
        background,
        highlight: GpuHighlight::default(),
    }
}

fn selecting(mut d: ModelDraw, mesh: u16) -> ModelDraw {
    d.highlight.mesh = Some(mesh);
    d
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
    assert!(ModelGpu::new(&device, &queue, &scene, (over, 4)).is_err());
    assert!(ModelGpu::new(&device, &queue, &scene, (4, 0)).is_err());
}

/// Regression (Sol slice review r2 MUST 1): a GPU resource rejected at
/// creation must come back as an error, never unwind. A destroyed device
/// rejects every buffer before any driver allocation, so this exercises the
/// invalid-resource path (`create_buffer_init` panicked mapping the invalid
/// buffer), not real out-of-memory recovery.
#[test]
fn rejected_resources_are_an_error_not_a_panic() {
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
        ModelGpu::new(&device, &queue, &scene, (16, 16)).map(|_| ())
    }));
    assert!(
        matches!(outcome, Ok(Err(_))),
        "rejected creation must be Err (unwound: {})",
        outcome.is_err()
    );
}

/// Drawing and picking on a device lost after creation must not unwind.
/// wgpu reports device loss through its callback rather than error scopes,
/// so the outcome may be Ok; what matters is that the render thread lives.
#[test]
fn drawing_after_device_loss_does_not_unwind() {
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
    let mut gpu = ModelGpu::new(&device, &queue, &scene, (16, 16)).unwrap();
    device.destroy();
    let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let _ = gpu.render(
            &device,
            &queue,
            &draw(ModelView::default(), (16, 16), [0.0, 0.0, 0.0, 1.0]),
        );
        let _ = gpu.pick_start(
            &device,
            &queue,
            &PickRequest {
                view: ModelView::default(),
                logical: (16, 16),
                size_px: (16, 16),
                texel: (8, 8),
            },
        );
        let _ = gpu.pick_poll(&device);
        let _ = gpu.resize(&device, (8, 8));
    }));
    assert!(outcome.is_ok(), "device loss unwound");
}

/// An adapter without vertex-stage storage buffers rejects the mesh pass as
/// an error from inside the GPU error scope (not the size preflight: the
/// binding size limit stays nonzero), and the device stays usable.
#[test]
fn missing_vertex_storage_is_a_scoped_error_and_the_device_survives() {
    let limits = wgpu::Limits {
        max_storage_buffers_per_shader_stage: 0,
        ..wgpu::Limits::default()
    };
    let device = open_device_with(limits);
    assert!(
        device.is_some() || std::env::var_os("NEOMACS_MODEL_REQUIRE_EVIDENCE").is_none(),
        "GPU evidence required but no headless wgpu adapter"
    );
    let Some((device, queue)) = device else {
        eprintln!("skipping: no headless wgpu adapter");
        return;
    };
    assert!(device.limits().max_storage_buffer_binding_size > 0);
    let mesh = parse_obj(
        b"v 0 0 0\nv 1 0 0\nv 0 1 0\nf 1 2 3\n",
        remaining_limits(&[]),
    )
    .unwrap();
    let scene = ModelScene::new(vec![mesh]).unwrap();
    let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        ModelGpu::new(&device, &queue, &scene, (16, 16)).map(|_| ())
    }));
    match outcome {
        Ok(Err(error)) => assert!(
            error.starts_with("model GPU resources rejected"),
            "rejected by the scope, not the preflight: {error}"
        ),
        Ok(Ok(())) => panic!("no vertex storage must be Err"),
        Err(_) => panic!("no vertex storage unwound"),
    }
    // The device still completes ordinary submitted work afterwards.
    let buffer = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("after"),
        size: 16,
        usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
        mapped_at_creation: false,
    });
    queue.write_buffer(&buffer, 0, &[7u8; 16]);
    queue.submit(None);
    buffer
        .slice(..)
        .map_async(wgpu::MapMode::Read, |r| r.unwrap());
    device
        .poll(wgpu::PollType::Wait {
            submission_index: None,
            timeout: Some(std::time::Duration::from_secs(10)),
        })
        .unwrap();
    assert_eq!(
        &buffer.slice(..).get_mapped_range().unwrap()[..],
        &[7u8; 16]
    );
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
    let mut gpu = ModelGpu::new(&device, &queue, &scene, (64, 64)).unwrap();
    gpu.render(
        &device,
        &queue,
        &selecting(
            draw(ModelView::default(), (64, 64), [0.0, 0.0, 1.0, 1.0]),
            1,
        ),
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
    let mut gpu = ModelGpu::new(&device, &queue, &scene, (16, 16)).unwrap();
    gpu.render(
        &device,
        &queue,
        &draw(ModelView::default(), (16, 16), [0.02, 0.02, 0.03, 1.0]),
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
    let mut gpu = ModelGpu::new(&device, &queue, &scene, (768, 768)).unwrap();
    let view = ModelView {
        yaw: 0.0,
        pitch: -90.0,
        zoom: 1.1,
    };
    gpu.render(
        &device,
        &queue,
        &selecting(draw(view, (768, 768), [0.0, 0.0, 0.0, 1.0]), 1),
    )
    .unwrap();
    let rgba = read_rgba(&device, &queue, &gpu);
    image::save_buffer(&out, &rgba, 768, 768, image::ColorType::Rgba8).unwrap();
    eprintln!(
        "wrote {out}: {} meshes, {} triangles, {} GPU bytes",
        scene.meshes().len(),
        scene.triangle_count(),
        gpu.geometry_bytes + target_bytes(768, 768)
    );
}

/// Deferred Sol slice SHOULD: a combined yaw and pitch, checked against
/// canvas-3d's own construction (`ry @ rx` of the two 4x4 matrices,
/// render.py) multiplied out here, not against the closed form under test.
#[test]
fn combined_yaw_and_pitch_match_canvas_3d() {
    let (yaw, pitch) = (30.0f64, -50.0f64);
    let (y, p) = (yaw.to_radians(), pitch.to_radians());
    let (cy, sy, cp, sp) = (y.cos(), y.sin(), p.cos(), p.sin());
    let ry = [[cy, 0.0, sy], [0.0, 1.0, 0.0], [-sy, 0.0, cy]];
    let rx = [[1.0, 0.0, 0.0], [0.0, cp, -sp], [0.0, sp, cp]];
    let mut want = [[0.0f64; 3]; 3];
    for i in 0..3 {
        for j in 0..3 {
            want[i][j] = (0..3).map(|k| ry[i][k] * rx[k][j]).sum();
        }
    }
    let got = ModelView {
        yaw,
        pitch,
        zoom: 1.0,
    }
    .rotation();
    for i in 0..3 {
        for j in 0..3 {
            assert!((got[i][j] - want[i][j]).abs() < 1e-15, "{i},{j}");
        }
    }
}

/// Deferred Sol slice SHOULD: the camera fit follows the logical aspect,
/// so a capped or rounded physical target frames the model as its glyph.
#[test]
fn camera_fit_uses_the_logical_aspect() {
    let v = ModelView::default();
    // 2048x32 logical renders at 2048x32 (or less when capped); the fit is
    // the logical 64:1, whatever the physical rounding.
    let [sx, sy] = clip_scale(&v, 2048, 32);
    assert!((sx as f64 - 0.8 / 64.0).abs() < 1e-7 && sy == 0.8);
}

#[test]
fn byte_accounting_matches_the_design_formula() {
    let uniform = std::mem::size_of::<CameraUniform>() as u64;
    // Region marks: 4 bits per face, 64 faces in 8 words.
    assert_eq!(
        geometry_bytes(10, 64),
        160 + 768 + 1024 + 32 + 2 * uniform + 12 + 256
    );
    assert_eq!(target_bytes(10, 20), 200 * 36);
}

#[test]
fn pick_intersection_retains_f64_precision_and_camera_fit() {
    let triangle = [
        [-0.9, -0.8, 0.1234567890123],
        [0.8, -0.7, -0.2345678901234],
        [0.1, 0.9, 0.3456789012345],
    ];
    for (yaw, pitch, logical, size_px, texel) in [
        (0.0, 0.0, (200, 100), (128, 64), (64, 32)),
        (29.0, -17.0, (100, 200), (64, 128), (32, 64)),
    ] {
        let request = PickRequest {
            view: ModelView {
                yaw,
                pitch,
                zoom: 1.3,
            },
            logical,
            size_px,
            texel,
        };
        let weights = pick_weights(triangle, &request).unwrap();
        assert!(weights.iter().all(|w| *w >= 0.0));
        assert!((weights.iter().sum::<f64>() - 1.0).abs() < 1e-14);
        let point =
            [0, 1, 2].map(|axis| (0..3).map(|i| weights[i] * triangle[i][axis]).sum::<f64>());
        let rotation = request.view.rotation();
        let q = [0, 1].map(|i| (0..3).map(|j| rotation[i][j] * point[j]).sum::<f64>());
        let scale = request.view.zoom * 0.8;
        let sx = scale * (logical.1 as f64 / logical.0 as f64).min(1.0);
        let sy = scale * (logical.0 as f64 / logical.1 as f64).min(1.0);
        let want = [
            ((texel.0 as f64 + 0.5) / size_px.0 as f64 * 2.0 - 1.0) / sx,
            (1.0 - (texel.1 as f64 + 0.5) / size_px.1 as f64 * 2.0) / sy,
        ];
        assert!((q[0] - want[0]).abs() < 1e-14);
        assert!((q[1] - want[1]).abs() < 1e-14);
    }
    let request = PickRequest {
        view: ModelView::default(),
        logical: (64, 64),
        size_px: (64, 64),
        texel: (32, 32),
    };
    assert!(pick_weights([[0.0; 3]; 3], &request).is_none());
}

const QUAD: &[u8] = b"v -1 -1 0\nv 1 -1 0\nv 1 1 0\nv -1 1 0\nf 1 2 3 4\n";

fn blue_marked(px: [u8; 4]) -> bool {
    // Bone blue 0.59 * light, highlight blue 0.2 * light (light 0.864 for a
    // facing quad): about 130 against 44 after the sRGB round trip.
    px[2] < 80
}

/// Region marks paint exactly the listed faces; a point target paints a
/// disc of its radius around the barycentric point, on its own mesh.
#[test]
fn region_and_point_targets_paint_the_right_fragments() {
    let Some((device, queue)) = try_device() else {
        eprintln!("skipping: no headless wgpu adapter");
        return;
    };
    let scene = ModelScene::new(vec![parse_obj(QUAD, remaining_limits(&[])).unwrap()]).unwrap();
    let mut gpu = ModelGpu::new(&device, &queue, &scene, (64, 64)).unwrap();
    // Faces fan from vertex 0: face 0 is the lower-right half (x > y),
    // face 1 the upper-left half.
    let mut d = draw(ModelView::default(), (64, 64), [0.0, 0.0, 0.0, 1.0]);
    d.highlight.faces = Some(vec![1]);
    gpu.render(&device, &queue, &d).unwrap();
    let rgba = read_rgba(&device, &queue, &gpu);
    assert!(blue_marked(pixel(&rgba, 64, 22, 22)), "face 1 marked");
    assert!(!blue_marked(pixel(&rgba, 64, 42, 42)), "face 0 not marked");

    let centre = scene
        .point_on(0, 0, [0.5, 0.0, 0.5])
        .expect("point on face 0");
    assert_eq!(centre, [0.0, 0.0, 0.0]);
    let mut d = draw(ModelView::default(), (64, 64), [0.0, 0.0, 0.0, 1.0]);
    d.highlight.point = Some(GpuPoint {
        mesh: 0,
        centre: scene.normalise(centre),
        radius: (0.3 / scene.radius()) as f32,
    });
    gpu.render(&device, &queue, &d).unwrap();
    let rgba = read_rgba(&device, &queue, &gpu);
    assert!(blue_marked(pixel(&rgba, 64, 32, 32)), "inside the disc");
    // (18, 18) is on the quad (it spans texels 14..50) but 0.74 from the
    // centre in normalised units, beyond the 0.21 radius.
    assert!(pixel(&rgba, 64, 18, 18)[0] > 150, "on the quad");
    assert!(!blue_marked(pixel(&rgba, 64, 18, 18)), "outside the disc");
    // The same point on another mesh index paints nothing here.
    d.highlight.point = Some(GpuPoint {
        mesh: 1,
        ..d.highlight.point.unwrap()
    });
    gpu.render(&device, &queue, &d).unwrap();
    let rgba = read_rgba(&device, &queue, &gpu);
    assert!(!blue_marked(pixel(&rgba, 64, 32, 32)), "other mesh only");
}

/// The dominant channel of a lit fragment: 0 red, 1 green, 2 blue, or
/// `None` for unmarked bone (all channels well lit).
fn layer_channel(px: [u8; 4]) -> Option<usize> {
    let max = *px[..3].iter().max().unwrap();
    let low = px[..3].iter().filter(|c| **c < 40).count();
    (low == 2 && max > 150).then(|| px[..3].iter().position(|c| *c == max).unwrap())
}

/// Layers paint together, each in its colour, a later layer winning
/// where they overlap: region, whole mesh and point layers, and two
/// region layers sharing a face.
#[test]
fn layers_paint_their_colours_and_later_layers_win() {
    let Some((device, queue)) = try_device() else {
        eprintln!("skipping: no headless wgpu adapter");
        return;
    };
    let scene = ModelScene::new(vec![parse_obj(QUAD, remaining_limits(&[])).unwrap()]).unwrap();
    let mut gpu = ModelGpu::new(&device, &queue, &scene, (64, 64)).unwrap();
    let (red, green, blue) = ([1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]);
    let render = |gpu: &mut ModelGpu, layers: Vec<GpuLayer>| {
        let mut d = draw(ModelView::default(), (64, 64), [0.0, 0.0, 0.0, 1.0]);
        d.highlight.layers = layers;
        gpu.render(&device, &queue, &d).unwrap();
        let rgba = read_rgba(&device, &queue, gpu);
        // (22, 22) is face 1 (upper left), (42, 42) face 0 and (34, 34)
        // face 0 inside the point disc around the centre.
        [(22, 22), (42, 42), (34, 34)].map(|(x, y)| layer_channel(pixel(&rgba, 64, x, y)))
    };
    let point = GpuPoint {
        mesh: 0,
        centre: scene.normalise([0.0, 0.0, 0.0]),
        radius: (0.3 / scene.radius()) as f32,
    };
    // Red whole mesh first, so the green region on face 1 wins over it
    // there, and the blue point, last, wins at the centre.
    assert_eq!(
        render(
            &mut gpu,
            vec![
                GpuLayer {
                    color: red,
                    paint: GpuPaint::Mesh(0)
                },
                GpuLayer {
                    color: green,
                    paint: GpuPaint::Faces(vec![1])
                },
                GpuLayer {
                    color: blue,
                    paint: GpuPaint::Point(point)
                },
            ]
        ),
        [Some(1), Some(0), Some(2)]
    );
    // Order reversed: the whole red mesh, last, wins everywhere.
    assert_eq!(
        render(
            &mut gpu,
            vec![
                GpuLayer {
                    color: blue,
                    paint: GpuPaint::Point(point)
                },
                GpuLayer {
                    color: green,
                    paint: GpuPaint::Faces(vec![1])
                },
                GpuLayer {
                    color: red,
                    paint: GpuPaint::Mesh(0)
                },
            ]
        ),
        [Some(0), Some(0), Some(0)]
    );
    // Two region layers share face 1: the later (blue) wins there; face
    // 0 is only in the earlier (green) one.
    assert_eq!(
        render(
            &mut gpu,
            vec![
                GpuLayer {
                    color: green,
                    paint: GpuPaint::Faces(vec![0, 1])
                },
                GpuLayer {
                    color: blue,
                    paint: GpuPaint::Faces(vec![1])
                },
            ]
        ),
        [Some(2), Some(1), Some(1)]
    );
    // Same faces, other layer numbers: the marks are rewritten.
    assert_eq!(
        render(
            &mut gpu,
            vec![
                GpuLayer {
                    color: blue,
                    paint: GpuPaint::Faces(vec![1])
                },
                GpuLayer {
                    color: green,
                    paint: GpuPaint::Faces(vec![0, 1])
                },
            ]
        ),
        [Some(1), Some(1), Some(1)]
    );
    // No layers: plain bone.
    assert_eq!(render(&mut gpu, vec![]), [None, None, None]);
    // The last regions again, after none: the cached marks still apply.
    assert_eq!(
        render(
            &mut gpu,
            vec![
                GpuLayer {
                    color: blue,
                    paint: GpuPaint::Faces(vec![1])
                },
                GpuLayer {
                    color: green,
                    paint: GpuPaint::Faces(vec![0, 1])
                },
            ]
        ),
        [Some(1), Some(1), Some(1)]
    );
}

/// A strip of five unit quads side by side: ten faces, so region marks
/// span two words (faces 0-7 in word 0, faces 8-9 in word 1).
const STRIP: &[u8] = b"v -5 -1 0\nv -3 -1 0\nv -3 1 0\nv -5 1 0\n\
v -3 -1 0\nv -1 -1 0\nv -1 1 0\nv -3 1 0\n\
v -1 -1 0\nv 1 -1 0\nv 1 1 0\nv -1 1 0\n\
v 1 -1 0\nv 3 -1 0\nv 3 1 0\nv 1 1 0\n\
v 3 -1 0\nv 5 -1 0\nv 5 1 0\nv 3 1 0\n\
f 1 2 3 4\nf 5 6 7 8\nf 9 10 11 12\nf 13 14 15 16\nf 17 18 19 20\n";

/// Every layer slot and both mark words decode exactly: eight region
/// layers in eight colours (two of them midtones) paint each face the
/// colour of the last layer listing it, byte for byte the colour that
/// layer paints alone; untouched faces stay bone; a whole-mesh layer in
/// the last slot wins everywhere.
#[test]
fn eight_layers_decode_across_mark_words_with_exact_colours() {
    let Some((device, queue)) = try_device() else {
        eprintln!("skipping: no headless wgpu adapter");
        return;
    };
    let scene = ModelScene::new(vec![parse_obj(STRIP, remaining_limits(&[])).unwrap()]).unwrap();
    assert_eq!(scene.triangle_count(), 10);
    let size = (256, 256);
    let mut gpu = ModelGpu::new(&device, &queue, &scene, size).unwrap();
    let render = |gpu: &mut ModelGpu, layers: Vec<GpuLayer>| {
        let mut d = draw(ModelView::default(), size, [0.0, 0.0, 0.0, 1.0]);
        d.highlight.layers = layers;
        gpu.render(&device, &queue, &d).unwrap();
        read_rgba(&device, &queue, gpu)
    };
    let faces = |faces: &[u32], color: [f32; 3]| GpuLayer {
        color,
        paint: GpuPaint::Faces(faces.to_vec()),
    };
    let (w, _) = gpu.size_px();
    // Find an interior probe pixel of each face: the fully red pixel
    // nearest the centroid of the pixels it paints red alone.
    let probes: Vec<(u32, u32)> = (0..10)
        .map(|f| {
            let rgba = render(&mut gpu, vec![faces(&[f], [1.0, 0.0, 0.0])]);
            let red: Vec<(u32, u32)> = (0..rgba.len() as u32 / 4)
                .map(|i| (i % w, i / w))
                .filter(|&(x, y)| {
                    let p = pixel(&rgba, w, x, y);
                    p[0] > 150 && p[1] < 10 && p[2] < 10
                })
                .collect();
            assert!(red.len() > 50, "face {f} paints {} pixels", red.len());
            let n = red.len() as f64;
            let cx = red.iter().map(|p| p.0 as f64).sum::<f64>() / n;
            let cy = red.iter().map(|p| p.1 as f64).sum::<f64>() / n;
            *red.iter()
                .min_by(|a, b| {
                    let d = |p: &(u32, u32)| (p.0 as f64 - cx).powi(2) + (p.1 as f64 - cy).powi(2);
                    d(a).total_cmp(&d(b))
                })
                .unwrap()
        })
        .collect();
    let colours: [[f32; 3]; 8] = [
        [1.0, 0.0, 0.0],
        [0.0, 1.0, 0.0],
        [0.0, 0.0, 1.0],
        [1.0, 1.0, 0.0],
        [0.0, 1.0, 1.0],
        [1.0, 0.0, 1.0],
        [0.5, 0.25, 0.75],
        [0.2, 0.6, 0.4],
    ];
    // What colour `c` looks like on face `f` painted alone.
    let alone = |gpu: &mut ModelGpu, f: u32, c: [f32; 3]| {
        let (x, y) = probes[f as usize];
        pixel(&render(gpu, vec![faces(&[f], c)]), w, x, y)
    };
    let expected_by_face: Vec<[u8; 4]> = (0..10u32)
        .map(|f| match f {
            1..=6 => alone(&mut gpu, f, colours[f as usize]),
            7 | 8 => alone(&mut gpu, f, colours[7]),
            _ => alone(&mut gpu, f, colours[0]),
        })
        .collect();
    let bone = render(&mut gpu, vec![]);
    let bone: Vec<[u8; 4]> = probes.iter().map(|&(x, y)| pixel(&bone, w, x, y)).collect();
    let all: Vec<u32> = (0..10).collect();
    let mut eight = vec![faces(&all, colours[0])];
    eight.extend((1..=6).map(|f| faces(&[f], colours[f as usize])));
    // Slot 7 (mark 8) on face 7 (word 0, top nibble) and face 8 (word 1).
    eight.push(faces(&[7, 8], colours[7]));
    let rgba = render(&mut gpu, eight.clone());
    let got: Vec<[u8; 4]> = probes.iter().map(|&(x, y)| pixel(&rgba, w, x, y)).collect();
    assert_eq!(got, expected_by_face);
    // Independently of the renderer: the strip is flat, so every face has
    // one light factor, read off bone; a layer colour must then show as
    // colour x light (sRGB in, sRGB texture out), midtones included.
    let light = bone[0][0] as f32 / 255.0 / BONE[0];
    for ch in 0..3 {
        let modelled = BONE[ch] * light * 255.0;
        assert!(
            (bone[0][ch] as f32 - modelled).abs() <= 2.0,
            "bone {:?} light {light}",
            bone[0]
        );
    }
    for (f, px) in got.iter().enumerate() {
        let colour = match f {
            1..=6 => colours[f],
            7 | 8 => colours[7],
            _ => colours[0],
        };
        for ch in 0..3 {
            let modelled = colour[ch] * light * 255.0;
            assert!(
                (px[ch] as f32 - modelled).abs() <= 2.0,
                "face {f} channel {ch}: {px:?} vs {colour:?} x {light}"
            );
        }
    }
    // The midtones are distinct from bone and from each other.
    assert_ne!(got[6], got[7]);
    assert!(got.iter().zip(&bone).all(|(g, b)| g != b));
    // Slot 7 alone: only faces 7 and 8 change.
    let rgba = render(&mut gpu, vec![faces(&[7, 8], colours[7])]);
    for f in 0..10usize {
        let (x, y) = probes[f];
        let p = pixel(&rgba, w, x, y);
        if f == 7 || f == 8 {
            assert_eq!(p, expected_by_face[f], "face {f}");
        } else {
            assert_eq!(p, bone[f], "face {f}");
        }
    }
    // A whole-mesh layer in the last slot wins over every region.
    eight[7] = GpuLayer {
        color: colours[7],
        paint: GpuPaint::Mesh(0),
    };
    let rgba = render(&mut gpu, eight);
    let mesh = alone(&mut gpu, 0, colours[7]);
    let rgba_probe: Vec<[u8; 4]> = probes.iter().map(|&(x, y)| pixel(&rgba, w, x, y)).collect();
    assert!(rgba_probe.iter().all(|p| *p == mesh), "{rgba_probe:?}");
}

fn pick_now(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    gpu: &mut ModelGpu,
    request: PickRequest,
) -> RawPick {
    gpu.pick_start(device, queue, &request).unwrap();
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    loop {
        match gpu.pick_poll(device) {
            PickPoll::Done(raw) => return raw,
            PickPoll::Pending => {
                assert!(std::time::Instant::now() < deadline, "pick timed out");
                std::thread::sleep(std::time::Duration::from_millis(1));
            }
            other => panic!("pick failed: {other:?}"),
        }
    }
}

/// §5.4 pick cases: depth order, background, barycentric reconstruction
/// in original coordinates, a coplanar tie, and the same place picked at
/// two DPI sizes.
#[test]
fn picks_name_the_covering_face_and_point() {
    let Some((device, queue)) = try_device() else {
        eprintln!("skipping: no headless wgpu adapter");
        return;
    };
    let near = parse_obj(
        b"v -1 -1 1\nv 1 -1 1\nv 0 1 1\nf 1 2 3\n",
        remaining_limits(&[]),
    )
    .unwrap();
    let far = parse_obj(
        b"v -1 -1 -1\nv 1 -1 -1\nv 0 1 -1\nf 1 2 3\n",
        remaining_limits(&[]),
    )
    .unwrap();
    // The far mesh is listed first, so only the depth test can put the near
    // one (mesh 1) on top.
    let scene = ModelScene::new(vec![far, near]).unwrap();
    let mut gpu = ModelGpu::new(&device, &queue, &scene, (64, 64)).unwrap();
    let request = |size: u32, texel: (u32, u32)| PickRequest {
        view: ModelView::default(),
        logical: (64, 64),
        size_px: (size, size),
        texel,
    };
    let hit = pick_now(&device, &queue, &mut gpu, request(64, (32, 36)))
        .hit
        .expect("centre hits");
    let (mesh, face) = scene.locate(hit.0).unwrap();
    assert_eq!((mesh, face), (1, 0), "nearer mesh wins");
    let point = scene.point_on(mesh, face, hit.1).unwrap();
    // Texel centre (32.5, 36.5) of 64: NDC (0.0156, -0.1406); normalised
    // position = NDC / 0.8, original = normalised * radius (centre 0).
    let r = scene.radius();
    let want_x = (32.5 / 64.0 * 2.0 - 1.0) / 0.8 * r;
    let want_y = (1.0 - 36.5 / 64.0 * 2.0) / 0.8 * r;
    assert!((point[0] - want_x).abs() < 1e-4, "{point:?} x {want_x}");
    assert!((point[1] - want_y).abs() < 1e-4, "{point:?} y {want_y}");
    assert!(
        (point[2] - 1.0).abs() < 1e-9,
        "on the near plane: {point:?}"
    );

    assert_eq!(
        pick_now(&device, &queue, &mut gpu, request(64, (0, 0))).hit,
        None,
        "corner is background"
    );

    // Twice the texels: texel (64, 72) is the same place.
    let hit2 = pick_now(&device, &queue, &mut gpu, request(128, (64, 72)))
        .hit
        .expect("hit at 2x");
    let point2 = scene.point_on(1, 0, hit2.1).unwrap();
    let want_x2 = (64.5 / 128.0 * 2.0 - 1.0) / 0.8 * r;
    assert!((point2[0] - want_x2).abs() < 1e-4, "{point2:?} x {want_x2}");

    // Coplanar duplicates: the first drawn (lowest global face) wins.
    let a = parse_obj(QUAD, remaining_limits(&[])).unwrap();
    let b = parse_obj(QUAD, remaining_limits(&[])).unwrap();
    let tie = ModelScene::new(vec![a, b]).unwrap();
    let mut gpu = ModelGpu::new(&device, &queue, &tie, (64, 64)).unwrap();
    let hit = pick_now(&device, &queue, &mut gpu, request(64, (42, 42)))
        .hit
        .unwrap();
    assert_eq!(tie.locate(hit.0), Some((0, 0)));
}

#[test]
fn a_second_pick_waits_for_the_first_mapping() {
    let Some((device, queue)) = try_device() else {
        eprintln!("skipping: no headless wgpu adapter");
        return;
    };
    let scene = ModelScene::new(vec![parse_obj(QUAD, remaining_limits(&[])).unwrap()]).unwrap();
    let mut gpu = ModelGpu::new(&device, &queue, &scene, (32, 32)).unwrap();
    let request = PickRequest {
        view: ModelView::default(),
        logical: (32, 32),
        size_px: (32, 32),
        texel: (16, 16),
    };
    gpu.pick_start(&device, &queue, &request).unwrap();
    assert!(gpu.pick_start(&device, &queue, &request).is_err());
    let raw = pick_now_after_start(&device, &mut gpu);
    assert!(raw.hit.is_some());
    assert_eq!(gpu.pick_poll(&device), PickPoll::Idle);
}

fn pick_now_after_start(device: &wgpu::Device, gpu: &mut ModelGpu) -> RawPick {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    loop {
        match gpu.pick_poll(device) {
            PickPoll::Done(raw) => return raw,
            PickPoll::Pending => assert!(std::time::Instant::now() < deadline),
            other => panic!("{other:?}"),
        }
    }
}

#[test]
fn resize_replaces_the_targets_and_keeps_drawing() {
    let Some((device, queue)) = try_device() else {
        eprintln!("skipping: no headless wgpu adapter");
        return;
    };
    let scene = ModelScene::new(vec![parse_obj(QUAD, remaining_limits(&[])).unwrap()]).unwrap();
    let mut gpu = ModelGpu::new(&device, &queue, &scene, (32, 32)).unwrap();
    gpu.resize(&device, (48, 24)).unwrap();
    assert_eq!(gpu.size_px(), (48, 24));
    assert_eq!(
        (gpu.texture().0.width(), gpu.texture().0.height()),
        (48, 24)
    );
    gpu.render(
        &device,
        &queue,
        &draw(ModelView::default(), (48, 24), [0.0, 0.0, 0.0, 1.0]),
    )
    .unwrap();
    let rgba = read_rgba(&device, &queue, &gpu);
    assert!(pixel(&rgba, 48, 24, 12)[0] > 150, "quad drawn after resize");
    let over = device.limits().max_texture_dimension_2d + 1;
    assert!(gpu.resize(&device, (over, 4)).is_err());
    assert_eq!(
        gpu.size_px(),
        (48, 24),
        "failed resize keeps the old targets"
    );
}
