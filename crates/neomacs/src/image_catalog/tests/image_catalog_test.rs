use super::*;
use neomacs_display_protocol::image_diagnostic::ImageDiagnostic;
use neomacs_display_runtime::render_thread::ImageRenderState;
use neovm_core::emacs_core::Context;
use neovm_core::emacs_core::Value;
use neovm_core::emacs_core::image::image_load_identity;
use neovm_core::emacs_core::image_catalog::ImageSizeLimit;
use neovm_core::emacs_core::image_catalog::{
    AxisSize, ImageColorContext, ImageDataSource, ImageDefaultScale, ImageScaleEnvironment,
    ImageScalePolicy, ImageSizeSpec, ImageSpecIdentity,
};
use neovm_core::emacs_core::value::list_to_vec;
use std::sync::Arc;
use std::time::Instant;

thread_local! {
    static IMAGE_SPEC_TEST_CONTEXT: Context = Context::new();
}

use neomacs_display_protocol::ImageAnimationPolicy;

fn file_request(path: &str) -> ImageResolveRequest {
    let spec = IMAGE_SPEC_TEST_CONTEXT.with(|_| {
        Value::list(vec![
            Value::symbol("image"),
            Value::keyword(":type"),
            Value::symbol("png"),
            Value::keyword(":file"),
            Value::string(path),
        ])
    });
    let items = list_to_vec(&spec).expect("test image spec is a list");
    ImageResolveRequest {
        spec: ImageSpecIdentity::from_lisp_spec(&spec).expect("test image spec"),
        identity: image_load_identity(&spec, &items),
        source: ImageResolveSource::File(ImageFileName::from_utf8(path)),
        size: ImageSizeSpec::new(AxisSize::AtMost(24), AxisSize::AtMost(24)),
        rotation: Default::default(),
        colors: ImageColorContext::default(),
        mask: Default::default(),
        animation: ImageAnimationPolicy::disabled(),
        frame: Default::default(),
        realization: Default::default(),
    }
}

/// These tests exercise scheduling, not the size bound: every lookup here
/// states GNU's "no explicit limit" arm, and the bound itself is asserted by
/// `the_load_command_carries_the_looking_frames_max_image_size`.
fn lookup(catalog: &AsyncImageCatalog, request: ImageResolveRequest) -> ImageLookup {
    catalog.lookup(request, ImageSizeLimit::UNLIMITED)
}

fn classify(file: &str) -> (ImageResolveRequest, Option<ImageFileRequest>) {
    let (cmd_tx, _cmd_rx) = neomacs_display_runtime::thread_comm::command_channel(64);
    let metadata = Arc::new(ImageRenderState::default());
    let catalog = AsyncImageCatalog::new(cmd_tx, None, metadata, None);
    catalog.classify_request(file_request(file))
}

#[test]
fn relative_file_is_classified_for_off_thread_search() {
    // The #242 fix: a bare relative `:file` must be searched against
    // data-directory/images off-thread, not opened verbatim from the cwd.
    let (request, resolution) = classify("splash.svg");
    assert!(matches!(
        &request.source,
        ImageResolveSource::File(p) if p.as_utf8_str() == Some("splash.svg")
    ));
    let resolution = resolution.expect("file source is classified");
    assert!(matches!(resolution, ImageFileRequest::Search { .. }));
    assert!(resolution.needs_off_thread());
}

#[test]
fn absolute_file_is_resolved_inline_and_keys_on_itself() {
    let (request, resolution) = classify("/abs/icon.png");
    assert!(matches!(
        &request.source,
        ImageResolveSource::File(p) if p.as_utf8_str() == Some("/abs/icon.png")
    ));
    let resolution = resolution.expect("file source is classified");
    assert!(matches!(resolution, ImageFileRequest::Direct(_)));
    assert!(!resolution.needs_off_thread());
}

#[test]
fn named_user_file_is_deferred_off_thread() {
    // `~user` may consult NSS/LDAP; keep resolution off the evaluator thread.
    let (request, resolution) = classify("~some-user/x.png");
    assert!(matches!(
        &request.source,
        ImageResolveSource::File(p) if p.as_utf8_str() == Some("~some-user/x.png")
    ));
    let resolution = resolution.expect("file source is classified");
    assert!(matches!(resolution, ImageFileRequest::ExpandHome(_)));
    assert!(resolution.needs_off_thread());
}

#[test]
fn pending_slot_and_decode_command_share_one_resolved_realization() {
    let (cmd_tx, cmd_rx) = neomacs_display_runtime::thread_comm::command_channel(64);
    let metadata = Arc::new(ImageRenderState::default());
    let catalog = AsyncImageCatalog::new(cmd_tx, None, metadata, None);
    let mut request = file_request("/tmp/icon.svg");
    // Neither axis pinned: the placeholder falls back to the realization.
    request.size = ImageSizeSpec::new(AxisSize::Native, AxisSize::AtMost(24));
    request.realization = ImageScaleEnvironment::new(7.2, 1.75, ImageDefaultScale::Auto)
        .resolve(ImageScalePolicy::Default);

    let placement = lookup(&catalog, request).placement();

    assert_eq!(placement.width(), 18);
    assert_eq!(placement.height(), 18);
    assert!(matches!(
        cmd_rx.try_recv().expect("image load command"),
        RenderCommand::Asset(AssetCommand::ImageLoadFile {
            realization,
            ..
        }) if (realization.layout_scale() - (1.3 / 1.75)).abs() < 0.0001
            && (realization.device_scale() - 1.75).abs() < f32::EPSILON
    ));
}

#[test]
fn invalidate_all_requeues_every_entry_under_its_existing_id() {
    let (cmd_tx, cmd_rx) = neomacs_display_runtime::thread_comm::command_channel(64);
    let metadata = Arc::new(ImageRenderState::default());
    let catalog = AsyncImageCatalog::new(cmd_tx, None, metadata, None);

    let first = lookup(&catalog, file_request("/tmp/one.png"))
        .placement()
        .image_id();
    let second = lookup(&catalog, file_request("/tmp/two.png"))
        .placement()
        .image_id();
    // Drain the two initial load commands.
    assert!(cmd_rx.try_recv().is_ok());
    assert!(cmd_rx.try_recv().is_ok());

    catalog.invalidate_all();

    let mut requeued_ids = Vec::new();
    while let Ok(command) = cmd_rx.try_recv() {
        match command {
            RenderCommand::Asset(AssetCommand::ImageLoadFile { load, .. }) => {
                requeued_ids.push(load.image());
            }
            other => panic!("unexpected command re-queued: {other:?}"),
        }
    }
    requeued_ids.sort_unstable();
    let mut expected = vec![first, second];
    expected.sort_unstable();
    assert_eq!(requeued_ids, expected, "same ids, one command per entry");

    // The entries survive: a later lookup reuses the id, no new load.
    let again = lookup(&catalog, file_request("/tmp/one.png"))
        .placement()
        .image_id();
    assert_eq!(again, first);
    assert!(cmd_rx.try_recv().is_err());
}

#[test]
fn invalidating_dependency_retires_old_identity_and_next_lookup_reloads() {
    let (cmd_tx, cmd_rx) = neomacs_display_runtime::thread_comm::command_channel(64);
    let metadata = Arc::new(ImageRenderState::default());
    let catalog = AsyncImageCatalog::new(cmd_tx, None, metadata, None);
    let request = file_request("/tmp/watched.svg");

    let first = lookup(&catalog, request.clone()).placement().image_id();
    assert!(matches!(
        cmd_rx.try_recv().expect("initial image load"),
        RenderCommand::Asset(AssetCommand::ImageLoadFile { load, .. })
            if load.image() == first
    ));

    catalog.invalidate(ImageInvalidation::Dependency(request.source.clone()));
    assert!(matches!(
        cmd_rx.try_recv().expect("old image identity retired"),
        RenderCommand::Asset(AssetCommand::ImageRetire { image }) if image == first
    ));

    let second = lookup(&catalog, request).placement().image_id();
    assert_ne!(first, second);
    assert!(matches!(
        cmd_rx.try_recv().expect("replacement image load"),
        RenderCommand::Asset(AssetCommand::ImageLoadFile { load, .. })
            if load.image() == second
    ));
}

#[test]
fn invalidating_spec_preserves_other_spec_that_uses_same_dependency() {
    let (cmd_tx, cmd_rx) = neomacs_display_runtime::thread_comm::command_channel(64);
    let metadata = Arc::new(ImageRenderState::default());
    let catalog = AsyncImageCatalog::new(cmd_tx, None, metadata, None);
    let first = file_request("/tmp/multi-page.png");
    let mut second = first.clone();
    let second_spec = Value::list(vec![
        Value::symbol("image"),
        Value::keyword(":type"),
        Value::symbol("png"),
        Value::keyword(":file"),
        Value::string("/tmp/multi-page.png"),
        Value::keyword(":index"),
        Value::fixnum(1),
    ]);
    second.spec = ImageSpecIdentity::from_lisp_spec(&second_spec).expect("second test image spec");

    let first_id = lookup(&catalog, first.clone()).placement().image_id();
    let second_id = lookup(&catalog, second.clone()).placement().image_id();
    assert_ne!(first_id, second_id);
    assert!(cmd_rx.try_recv().is_ok());
    assert!(cmd_rx.try_recv().is_ok());

    catalog.invalidate(ImageInvalidation::Spec {
        spec: first.spec.clone(),
    });
    assert!(matches!(
        cmd_rx.try_recv().expect("only exact spec identity freed"),
        RenderCommand::Asset(AssetCommand::ImageRetire { image }) if image == first_id
    ));
    assert!(cmd_rx.try_recv().is_err());

    assert_eq!(
        lookup(&catalog, second).placement().image_id(),
        second_id,
        "the other spec keeps its renderer identity"
    );
    assert!(cmd_rx.try_recv().is_err());

    let replacement_id = lookup(&catalog, first).placement().image_id();
    assert_ne!(replacement_id, first_id);
    assert!(matches!(
        cmd_rx.try_recv().expect("exact spec is decoded again"),
        RenderCommand::Asset(AssetCommand::ImageLoadFile { load, .. })
            if load.image() == replacement_id
    ));
}

#[test]
fn renderer_reconciliation_upgrades_pending_to_ready_geometry() {
    use neomacs_display_runtime::render_thread::ImageDecodeTerminal;
    use neovm_core::emacs_core::image_catalog::{ImageLookup, ResolvedImageMetadata};

    let (cmd_tx, _cmd_rx) = neomacs_display_runtime::thread_comm::command_channel(64);
    let metadata = Arc::new(ImageRenderState::default());
    let catalog = AsyncImageCatalog::new(cmd_tx, None, Arc::clone(&metadata), None);
    let request = file_request("/tmp/promote.png");

    let ImageLookup::Pending(pending) = lookup(&catalog, request.clone()) else {
        panic!("expected pending");
    };
    let id = pending.placement().image_id();
    let load = pending.load();
    // Placeholder from AtMost(24) pins.
    assert_eq!(pending.placement().width(), 24);

    metadata.publish_terminal(
        load,
        ImageDecodeTerminal::Ready(ResolvedImageMetadata::layout_is_image_pixels(
            120,
            80,
            0,
            false,
            Default::default(),
        )),
    );

    catalog.reconcile_renderer_state(ImageStateEvent::DecodeCompleted(load));
    let ImageLookup::Ready(ready) = lookup(&catalog, request) else {
        panic!("promote must leave Ready geometry for rebuild");
    };
    assert_eq!(ready.metadata.layout.dimensions(), (120, 80));
    assert_eq!(ready.image_id(), id);
}

#[test]
fn renderer_eviction_requeues_ready_image_under_its_stable_id() {
    use neomacs_display_runtime::render_thread::ImageDecodeTerminal;
    use neovm_core::emacs_core::image_catalog::{ImageLookup, ResolvedImageMetadata};

    let (cmd_tx, cmd_rx) = neomacs_display_runtime::thread_comm::command_channel(64);
    let metadata = Arc::new(ImageRenderState::default());
    let catalog = AsyncImageCatalog::new(cmd_tx, None, Arc::clone(&metadata), None);
    let request = file_request("/tmp/room-avatar.png");

    let ImageLookup::Pending(pending) = lookup(&catalog, request.clone()) else {
        panic!("new avatar should begin pending");
    };
    let id = pending.placement().image_id();
    let first_load = pending.load();
    assert!(matches!(
        cmd_rx.try_recv().expect("initial avatar load"),
        RenderCommand::Asset(AssetCommand::ImageLoadFile { load, .. })
            if load == first_load
    ));

    metadata.publish_terminal(
        first_load,
        ImageDecodeTerminal::Ready(ResolvedImageMetadata::layout_is_image_pixels(
            48,
            48,
            0,
            false,
            Default::default(),
        )),
    );
    catalog.reconcile_renderer_state(ImageStateEvent::DecodeCompleted(first_load));
    assert!(matches!(
        lookup(&catalog, request.clone()),
        ImageLookup::Ready(_)
    ));

    // The renderer's LRU dropped the texture. Its lifecycle notification
    // removes residency metadata before asking the catalog to reconcile.
    metadata.remove_terminal(first_load);
    catalog.reconcile_renderer_state(ImageStateEvent::Evicted(id));

    let ImageLookup::Pending(reloading) = lookup(&catalog, request) else {
        panic!("evicted avatar should remain pending until its reload completes");
    };
    assert!(matches!(
        cmd_rx.try_recv().expect("evicted avatar reload"),
        RenderCommand::Asset(AssetCommand::ImageLoadFile { load, .. })
            if load.image() == id && load != first_load
    ));
    assert_eq!(reloading.placement().image_id(), id);
    assert_eq!(reloading.placement().width(), 48);
    assert_eq!(reloading.placement().height(), 48);
}

#[test]
fn eviction_after_decode_but_before_evaluator_service_does_not_strand_pending_image() {
    let (cmd_tx, cmd_rx) = neomacs_display_runtime::thread_comm::command_channel(64);
    let metadata = Arc::new(ImageRenderState::default());
    let catalog = AsyncImageCatalog::new(cmd_tx, None, metadata, None);
    let request = file_request("/tmp/large-chat-photo.png");

    let ImageLookup::Pending(first_load) = lookup(&catalog, request.clone()) else {
        panic!("new image should begin pending");
    };
    let id = first_load.placement().image_id();
    let first_token = first_load.load();
    assert!(cmd_rx.try_recv().is_ok(), "initial load was queued");

    // The renderer can publish Ready and then evict the same image in one
    // batch. By the time the evaluator services both ordered events, the
    // shared metadata map is already empty; the typed eviction reason must
    // still move Pending -> Evicted instead of leaving it pending forever.
    catalog.reconcile_renderer_state(ImageStateEvent::DecodeCompleted(first_token));
    catalog.reconcile_renderer_state(ImageStateEvent::Evicted(id));

    let ImageLookup::Pending(reload) = lookup(&catalog, request) else {
        panic!("visible evicted image should schedule another load");
    };
    assert_eq!(reload.placement().image_id(), id);
    assert!(matches!(
        cmd_rx.try_recv().expect("replacement load"),
        RenderCommand::Asset(AssetCommand::ImageLoadFile { load, .. })
            if load.image() == id && load != first_token
    ));
}

#[test]
fn stale_decode_completion_cannot_promote_a_replacement_load() {
    use neomacs_display_runtime::render_thread::ImageDecodeTerminal;
    use neovm_core::emacs_core::image_catalog::ResolvedImageMetadata;

    let (cmd_tx, cmd_rx) = neomacs_display_runtime::thread_comm::command_channel(64);
    let metadata = Arc::new(ImageRenderState::default());
    let catalog = AsyncImageCatalog::new(cmd_tx, None, Arc::clone(&metadata), None);
    let request = file_request("/tmp/replaced-avatar.png");

    let ImageLookup::Pending(first) = lookup(&catalog, request.clone()) else {
        panic!("initial image should be pending");
    };
    let first_load = first.load();
    cmd_rx.try_recv().expect("initial load command");

    catalog.reconcile_renderer_state(ImageStateEvent::Evicted(first_load.image()));
    let ImageLookup::Pending(replacement) = lookup(&catalog, request.clone()) else {
        panic!("eviction should schedule a replacement load");
    };
    let replacement_load = replacement.load();
    assert_eq!(replacement_load.image(), first_load.image());
    assert_ne!(replacement_load, first_load);
    cmd_rx.try_recv().expect("replacement load command");

    metadata.publish_terminal(
        first_load,
        ImageDecodeTerminal::Ready(ResolvedImageMetadata::layout_is_image_pixels(
            120,
            80,
            0,
            false,
            Default::default(),
        )),
    );
    catalog.reconcile_renderer_state(ImageStateEvent::DecodeCompleted(first_load));

    let ImageLookup::Pending(still_replacement) = lookup(&catalog, request) else {
        panic!("a stale completion must not promote the replacement attempt");
    };
    assert_eq!(still_replacement.load(), replacement_load);
}

/// The invariant this catalog's header probe exists for: geometry resolves
/// while the decode is still pending — no pixels, no terminal — and it is the
/// geometry the decode reports for the same image.
///
/// The renderer-side half of that equality
/// (`header_layout_equals_the_decoded_layout_for_every_probed_format` and the
/// fixture case beside it, in `neomacs-renderer-wgpu`) pins the decode's own
/// answer for this very file, so the slot asserted here is the slot the decode
/// will confirm rather than a slot that moves when the pixels land.
#[test]
fn pending_geometry_resolves_from_the_header_before_any_pixel_exists() {
    let (cmd_tx, cmd_rx) = neomacs_display_runtime::thread_comm::command_channel(64);
    let metadata = Arc::new(ImageRenderState::default());
    let (redisplay_tx, redisplay_rx) = crossbeam_channel::unbounded();
    let catalog = AsyncImageCatalog::new(
        cmd_tx,
        None,
        Arc::clone(&metadata),
        Some(RedisplayWaker::new(redisplay_tx, None)),
    );
    let fixture = neomacs_infra::workspace_root().join("test/data/image/blank-100x200.png");
    let mut request = file_request(fixture.to_str().expect("utf8 fixture path"));
    // 100x200 with only a width clamp: the decoded layout is 50x100, where the
    // pre-header placeholder reserves the clamp square 50x50 instead.
    request.size = ImageSizeSpec::new(AxisSize::AtMost(50), AxisSize::Native);

    let ImageLookup::Pending(placeholder) = lookup(&catalog, request.clone()) else {
        panic!("a new image lookup begins pending");
    };
    assert_eq!(
        (
            placeholder.placement().width(),
            placeholder.placement().height()
        ),
        (50, 50),
        "the slot starts on the request's pinned placeholder"
    );
    let RenderCommand::Asset(AssetCommand::ImageLoadFile { load, .. }) =
        cmd_rx.try_recv().expect("image load command")
    else {
        panic!("a file source loads through ImageLoadFile");
    };
    assert!(
        metadata.terminal(load).is_none(),
        "no decode terminal exists yet"
    );

    // The probe runs off-thread; wait for it without ever consulting the
    // renderer, which is what makes the resolution independent of the decode.
    let deadline = Instant::now() + Duration::from_secs(10);
    let pending = loop {
        let lookup = lookup(&catalog, request.clone());
        if lookup.placement().height() == 100 {
            break lookup;
        }
        assert!(
            Instant::now() < deadline,
            "the header probe never resolved this image's geometry"
        );
        std::thread::sleep(Duration::from_millis(5));
    };

    assert!(
        matches!(pending, ImageLookup::Pending(_)),
        "geometry must resolve while the decode is still pending"
    );
    assert!(
        metadata.terminal(load).is_none(),
        "no pixel exists when the geometry does"
    );
    assert_eq!(
        (pending.placement().width(), pending.placement().height()),
        (50, 100),
        "the slot is the decoded geometry, not the placeholder"
    );
    assert_eq!(
        pending.placement().image_id(),
        load.image(),
        "the slot keeps its identity across the refinement"
    );
    assert!(
        matches!(
            redisplay_rx.recv_timeout(Duration::from_secs(1)),
            Ok(neovm_core::keyboard::InputEvent::LayoutInvalidated)
        ),
        "resolved geometry must ask the evaluator to republish layout"
    );
}

/// A source with no readable header keeps the placeholder it always had.
#[test]
fn pending_geometry_without_a_header_keeps_the_request_placeholder() {
    let (cmd_tx, _cmd_rx) = neomacs_display_runtime::thread_comm::command_channel(64);
    let metadata = Arc::new(ImageRenderState::default());
    let catalog = AsyncImageCatalog::new(cmd_tx, None, metadata, None);
    let request = file_request("/nonexistent/neomacs/not-an-image.png");

    let ImageLookup::Pending(pending) = lookup(&catalog, request.clone()) else {
        panic!("a new image lookup begins pending");
    };
    assert_eq!(
        (pending.placement().width(), pending.placement().height()),
        (24, 24)
    );

    // Let any probe for the unreadable path land before re-checking.
    std::thread::sleep(Duration::from_millis(200));
    let ImageLookup::Pending(unchanged) = lookup(&catalog, request) else {
        panic!("an unreadable image stays pending until it fails");
    };
    assert_eq!(
        (
            unchanged.placement().width(),
            unchanged.placement().height()
        ),
        (24, 24),
        "no header means no refinement"
    );
}

/// The bound a lookup is made under travels with the load command.
///
/// The catalog cannot apply `max-image-size` itself — it never sees the
/// encoded header — and the renderer refuses at the header read it already
/// performs, which is GNU's own point in the load (`check_image_size`,
/// `src/image.c:1811`). What the catalog owes is the value.
#[test]
fn the_load_command_carries_the_looking_frames_max_image_size() {
    let (cmd_tx, cmd_rx) = neomacs_display_runtime::thread_comm::command_channel(64);
    let metadata = Arc::new(ImageRenderState::default());
    let catalog = AsyncImageCatalog::new(cmd_tx, None, metadata, None);
    let limit = ImageSizeLimit::from_axis_pixels(64);

    catalog.lookup(file_request("/tmp/huge.png"), limit);

    assert!(matches!(
        cmd_rx.try_recv().expect("image load command"),
        RenderCommand::Asset(AssetCommand::ImageLoadFile { limit: carried, .. }) if carried == limit
    ));
}

/// A later lookup under a different bound must carry *that* bound, not the one
/// it was first scheduled with: the frame that is asking is the frame whose
/// `max-image-size` applies.
#[test]
fn each_lookup_carries_the_bound_it_was_made_under() {
    let (cmd_tx, cmd_rx) = neomacs_display_runtime::thread_comm::command_channel(64);
    let metadata = Arc::new(ImageRenderState::default());
    let catalog = AsyncImageCatalog::new(cmd_tx, None, metadata, None);
    let request = file_request("/tmp/reloaded.png");

    catalog.lookup(request.clone(), ImageSizeLimit::from_axis_pixels(64));
    catalog.invalidate(ImageInvalidation::Spec {
        spec: request.spec.clone(),
    });
    catalog.lookup(request, ImageSizeLimit::from_axis_pixels(4096));

    // The first load, the retirement of its identity, then the reload.
    assert!(matches!(
        cmd_rx.try_recv().expect("first load"),
        RenderCommand::Asset(AssetCommand::ImageLoadFile { limit, .. })
            if limit == ImageSizeLimit::from_axis_pixels(64)
    ));
    assert!(matches!(
        cmd_rx.try_recv().expect("retirement"),
        RenderCommand::Asset(AssetCommand::ImageRetire { .. })
    ));
    assert!(matches!(
        cmd_rx.try_recv().expect("reload"),
        RenderCommand::Asset(AssetCommand::ImageLoadFile { limit, .. })
            if limit == ImageSizeLimit::from_axis_pixels(4096)
    ));
}

/// The device-loss re-queue has no frame in hand, so it re-checks against the
/// bound the redisplay that built the entries resolved — never against "no
/// limit", which is the one bound a re-queue must not load under.
#[test]
fn a_device_loss_requeue_carries_the_limit_it_last_saw() {
    let (cmd_tx, cmd_rx) = neomacs_display_runtime::thread_comm::command_channel(64);
    let metadata = Arc::new(ImageRenderState::default());
    let catalog = AsyncImageCatalog::new(cmd_tx, None, metadata, None);
    let limit = ImageSizeLimit::from_axis_pixels(7);

    catalog.lookup(file_request("/tmp/before-loss.png"), limit);
    cmd_rx.try_recv().expect("initial load");
    catalog.invalidate_all();

    assert!(matches!(
        cmd_rx.try_recv().expect("re-queued load"),
        RenderCommand::Asset(AssetCommand::ImageLoadFile { limit: carried, .. }) if carried == limit
    ));
}

/// What the evaluator sees when the renderer refuses a load: the same failed
/// state a decoder failure produces, carrying GNU's diagnostic.
///
/// The refusal travels as `ImageCacheEvent::Failed` (asserted where the command
/// is dispatched, in `neomacs-display-runtime`), which publishes this terminal.
/// Redisplay keeps the placeholder slot; a synchronous query surfaces the
/// message.
#[test]
fn a_refused_load_is_a_failed_lookup_carrying_gnus_diagnostic() {
    use neovm_core::emacs_core::image_catalog::OversizedImage;

    let (cmd_tx, _cmd_rx) = neomacs_display_runtime::thread_comm::command_channel(64);
    let metadata = Arc::new(ImageRenderState::default());
    let catalog = AsyncImageCatalog::new(cmd_tx, None, Arc::clone(&metadata), None);
    let request = file_request("/tmp/too-large.png");

    let ImageLookup::Pending(pending) = catalog.lookup(request.clone(), ImageSizeLimit::UNLIMITED)
    else {
        panic!("a new image lookup begins pending");
    };
    let load = pending.load();
    let slot = pending.placement();

    metadata.publish_terminal(
        load,
        ImageDecodeTerminal::Failed(ImageDiagnostic::InvalidSize),
    );

    let ImageLookup::Failed(failed) = catalog.lookup(request, ImageSizeLimit::UNLIMITED) else {
        panic!("a refused load must not stay pending");
    };
    assert_eq!(failed.load(), load);
    assert_eq!(failed.error, ImageDiagnostic::InvalidSize);
    assert_eq!(failed.error.message(), OversizedImage::MESSAGE);
    assert_eq!(
        failed.placement().dimensions(),
        slot.dimensions(),
        "the reserved slot survives the refusal, so the frame does not move"
    );
}

/// A failure arriving while a synchronous caller waits must be reported just
/// like a failure already observed by ordinary lookup. Each explicit semantic
/// query reports it once, independently of display-side negative caching.
#[test]
fn synchronous_wait_reports_a_new_decode_failure_and_each_cached_query() {
    let (cmd_tx, _cmd_rx) = neomacs_display_runtime::thread_comm::command_channel(64);
    let catalog = AsyncImageCatalog::new(cmd_tx, None, Arc::new(ImageRenderState::default()), None);
    let request = file_request("/nonexistent/neomacs/synchronous-failure.png");
    let error = catalog
        .resolve_sync(request.clone(), ImageSizeLimit::UNLIMITED)
        .unwrap_err();
    assert!(error.contains("Cannot find image file"));
    assert_eq!(catalog.take_pending_diagnostics(), vec![error.clone()]);
    assert_eq!(
        catalog.resolve_sync(request, ImageSizeLimit::UNLIMITED),
        Err(error.clone())
    );
    assert_eq!(catalog.take_pending_diagnostics(), vec![error]);
}

#[test]
fn semantic_rgba_retention_is_bounded_without_gpu_eviction() {
    let (cmd_tx, cmd_rx) = neomacs_display_runtime::thread_comm::command_channel(1);
    let catalog = AsyncImageCatalog::new(cmd_tx, None, Arc::new(ImageRenderState::default()), None);
    let fixture = neomacs_infra::workspace_root().join("test/data/image/blank-100x200.png");
    let mut first = None;
    for index in 0..850 {
        let mut request = file_request(&format!("retention-{index}.png"));
        request.source =
            ImageResolveSource::File(ImageFileName::from_utf8(fixture.to_str().unwrap()));
        request.size = ImageSizeSpec::default();
        let ready = catalog
            .resolve_sync(request.clone(), ImageSizeLimit::UNLIMITED)
            .unwrap()
            .unwrap();
        if index == 0 {
            first = Some((request, ready));
        }
    }
    let bytes = catalog.semantic_queries.cached_size_bytes();
    println!("850 complete 100x200 RGBA realizations: retained CPU bytes={bytes}");
    assert!(
        cmd_rx.try_recv().is_err(),
        "no GPU work or eviction enforces this budget"
    );
    assert!(
        bytes <= 64 * 1024 * 1024,
        "retained CPU RGBA exceeds independent budget: {bytes}"
    );
    let (request, ready) = first.unwrap();
    assert_eq!(
        catalog
            .resolve_sync(request, ImageSizeLimit::from_axis_pixels(1))
            .unwrap()
            .unwrap(),
        ready
    );
    assert!(catalog.invalidate(ImageInvalidation::All).changed());
    assert_eq!(catalog.semantic_queries.cached_size_bytes(), 0);
}

#[test]
fn semantic_rgba_fifo_preserves_answers_errors_and_encoded_fallback() {
    let (cmd_tx, cmd_rx) = neomacs_display_runtime::thread_comm::command_channel(64);
    let mut catalog =
        AsyncImageCatalog::new(cmd_tx, None, Arc::new(ImageRenderState::default()), None);
    catalog.semantic_queries = semantic_queries::SemanticQueries::with_pixel_budget(80_000);
    let fixture = neomacs_infra::workspace_root().join("test/data/image/blank-100x200.png");
    let requests: Vec<_> = (0..3)
        .map(|index| {
            let mut request = file_request(&format!("fifo-{index}.png"));
            request.source =
                ImageResolveSource::File(ImageFileName::from_utf8(fixture.to_str().unwrap()));
            request.size = ImageSizeSpec::default();
            request
        })
        .collect();
    let first = catalog
        .resolve_sync(requests[0].clone(), ImageSizeLimit::UNLIMITED)
        .unwrap()
        .unwrap();
    assert!(catalog.semantic_queries.pixels(&requests[0]).is_some());
    let second = catalog
        .resolve_sync(requests[1].clone(), ImageSizeLimit::UNLIMITED)
        .unwrap()
        .unwrap();
    assert_eq!(catalog.semantic_queries.cached_size_bytes(), 80_000);
    assert!(catalog.semantic_queries.pixels(&requests[0]).is_none());
    assert!(catalog.semantic_queries.pixels(&requests[1]).is_some());
    assert_eq!(
        catalog
            .resolve_sync(requests[0].clone(), ImageSizeLimit::from_axis_pixels(1))
            .unwrap()
            .unwrap(),
        first
    );
    assert!(
        catalog.semantic_queries.pixels(&requests[0]).is_none(),
        "warm semantic answers do not re-decode evicted RGBA"
    );
    catalog.lookup(requests[0].clone(), ImageSizeLimit::from_axis_pixels(1));
    assert!(
        matches!(cmd_rx.try_recv().unwrap(), RenderCommand::Asset(AssetCommand::ImageLoadFile { limit, .. }) if limit == ImageSizeLimit::UNLIMITED)
    );
    // Complete but oversized pixels bypass retention without discarding the FIFO.
    let mut oversized = requests[2].clone();
    oversized.size = ImageSizeSpec::new(AxisSize::Exact(200), AxisSize::Exact(400));
    assert!(
        catalog
            .resolve_sync(oversized.clone(), ImageSizeLimit::UNLIMITED)
            .is_ok()
    );
    assert!(catalog.semantic_queries.pixels(&oversized).is_none());
    assert!(catalog.semantic_queries.pixels(&requests[1]).is_some());
    // Cache failures without allowing later bounds to re-admit them.
    assert!(
        catalog
            .resolve_sync(requests[2].clone(), ImageSizeLimit::from_axis_pixels(1))
            .is_err()
    );
    catalog.take_pending_diagnostics();
    assert_eq!(
        catalog.resolve_sync(requests[2].clone(), ImageSizeLimit::UNLIMITED),
        Err(ImageDiagnostic::InvalidSize.message())
    );
    assert_eq!(
        catalog.take_pending_diagnostics(),
        vec![ImageDiagnostic::InvalidSize.message()]
    );
    catalog.invalidate_all();
    assert_eq!(
        catalog
            .resolve_sync(requests[1].clone(), ImageSizeLimit::from_axis_pixels(1))
            .unwrap()
            .unwrap(),
        second
    );
    assert_eq!(catalog.semantic_queries.cached_size_bytes(), 80_000);
    assert!(
        catalog
            .invalidate(ImageInvalidation::Spec {
                spec: requests[1].spec.clone()
            })
            .changed()
    );
    assert_eq!(catalog.semantic_queries.cached_size_bytes(), 0);
}

#[test]
fn semantic_decode_completes_without_renderer_service_or_drawable_ready() {
    let (cmd_tx, _cmd_rx) = neomacs_display_runtime::thread_comm::command_channel(1);
    let metadata = Arc::new(ImageRenderState::default());
    let catalog = AsyncImageCatalog::new(cmd_tx, None, Arc::clone(&metadata), None);
    let fixture = neomacs_infra::workspace_root().join("test/data/image/blank-100x200.png");
    let mut request = file_request(fixture.to_str().unwrap());
    request.size = ImageSizeSpec::default();
    let start = Instant::now();
    let ready = catalog
        .resolve_sync(request.clone(), ImageSizeLimit::UNLIMITED)
        .expect("full CPU decode must not wait for renderer service")
        .unwrap();
    assert_eq!(ready.metadata.layout.dimensions(), (100, 200));
    assert!(start.elapsed() < Duration::from_millis(900));
    assert!(
        matches!(lookup(&catalog, request), ImageLookup::Pending(_)),
        "semantic completion must not publish drawable residency"
    );
    assert!(metadata.terminal(ready.load).is_none());
}

#[test]
fn semantic_decode_cache_survives_gpu_reset_but_not_lisp_invalidation() {
    let (cmd_tx, cmd_rx) = neomacs_display_runtime::thread_comm::command_channel(1);
    let catalog = AsyncImageCatalog::new(cmd_tx, None, Arc::new(ImageRenderState::default()), None);
    let fixture = neomacs_infra::workspace_root().join("test/data/image/blank-100x200.png");
    let request = file_request(fixture.to_str().unwrap());
    let first = catalog
        .resolve_sync(request.clone(), ImageSizeLimit::UNLIMITED)
        .unwrap()
        .unwrap();
    assert!(
        cmd_rx.try_recv().is_err(),
        "semantic admission must not send renderer commands"
    );
    catalog.invalidate_all();
    let warm = catalog
        .resolve_sync(request.clone(), ImageSizeLimit::from_axis_pixels(1))
        .unwrap()
        .unwrap();
    assert_eq!(
        first, warm,
        "GPU reset and changed admission bounds do not erase decoded semantics"
    );
    assert!(
        catalog
            .invalidate(ImageInvalidation::Spec {
                spec: request.spec.clone()
            })
            .changed()
    );
    assert_eq!(
        catalog.resolve_sync(request.clone(), ImageSizeLimit::from_axis_pixels(1)),
        Err(ImageDiagnostic::InvalidSize.message())
    );
    assert!(
        catalog
            .invalidate(ImageInvalidation::Dependency(request.source.clone()))
            .changed()
    );
    let replacement = catalog
        .resolve_sync(request, ImageSizeLimit::UNLIMITED)
        .unwrap()
        .unwrap();
    assert_ne!(first.load, replacement.load);
    assert_eq!(first.metadata, replacement.metadata);
}

#[test]
fn semantic_decode_admission_is_shared_with_later_display_without_readiness() {
    let (cmd_tx, cmd_rx) = neomacs_display_runtime::thread_comm::command_channel(64);
    let catalog = AsyncImageCatalog::new(cmd_tx, None, Arc::new(ImageRenderState::default()), None);
    let fixture = neomacs_infra::workspace_root().join("test/data/image/blank-100x200.png");
    let request = file_request(fixture.to_str().unwrap());
    catalog
        .resolve_sync(request.clone(), ImageSizeLimit::UNLIMITED)
        .unwrap();
    assert!(matches!(
        catalog.lookup(request.clone(), ImageSizeLimit::from_axis_pixels(1)),
        ImageLookup::Pending(_)
    ));
    assert!(
        matches!(cmd_rx.try_recv().unwrap(), RenderCommand::Asset(AssetCommand::ImageLoadDecoded { decoded, .. }) if decoded.metadata().layout.dimensions() == (12, 24))
    );
    assert!(catalog.invalidate(ImageInvalidation::All).changed());
    while cmd_rx.try_recv().is_ok() {}
    assert!(
        catalog
            .resolve_sync(request.clone(), ImageSizeLimit::from_axis_pixels(1))
            .is_err()
    );
    let ImageLookup::Failed(failed) = catalog.lookup(request, ImageSizeLimit::UNLIMITED) else {
        panic!("negative admission must be shared too");
    };
    assert_eq!(failed.error, ImageDiagnostic::InvalidSize);
    assert!(cmd_rx.try_recv().is_err());
}

#[test]
fn semantic_decode_pending_display_uses_first_admission_in_both_bound_orders() {
    for first in [
        ImageSizeLimit::UNLIMITED,
        ImageSizeLimit::from_axis_pixels(1),
    ] {
        let later = if first == ImageSizeLimit::UNLIMITED {
            ImageSizeLimit::from_axis_pixels(1)
        } else {
            ImageSizeLimit::UNLIMITED
        };
        let (cmd_tx, _unserviced_rx) = neomacs_display_runtime::thread_comm::command_channel(64);
        let metadata = Arc::new(ImageRenderState::default());
        let catalog = AsyncImageCatalog::new(cmd_tx, None, Arc::clone(&metadata), None);
        let fixture = neomacs_infra::workspace_root().join("test/data/image/blank-100x200.png");
        let request = file_request(fixture.to_str().unwrap());
        let ImageLookup::Pending(pending) = catalog.lookup(request.clone(), first) else {
            panic!("display starts pending");
        };
        let before = catalog.resolve_sync(request.clone(), later);
        if first == ImageSizeLimit::UNLIMITED {
            let ready = before.as_ref().unwrap().as_ref().unwrap();
            metadata.publish_terminal(
                pending.load(),
                ImageDecodeTerminal::Ready(ready.metadata.clone()),
            );
            assert!(matches!(
                catalog.lookup(request.clone(), later),
                ImageLookup::Ready(_)
            ));
        } else {
            assert_eq!(before, Err(ImageDiagnostic::InvalidSize.message()));
            metadata.publish_terminal(
                pending.load(),
                ImageDecodeTerminal::Failed(ImageDiagnostic::InvalidSize),
            );
            assert!(matches!(
                catalog.lookup(request.clone(), later),
                ImageLookup::Failed(_)
            ));
        }
        assert_eq!(before, catalog.resolve_sync(request.clone(), later));
        catalog.invalidate_all();
        assert_eq!(before, catalog.resolve_sync(request, later));
    }
}

#[test]
fn semantic_decode_first_admission_survives_unobserved_completion_and_timeout() {
    // Deterministically model the interval after a semantic wait returned None:
    // first admission has been established, but no completion was observed and
    // no prepared pixels/admissions have been adopted. No sleeps or renderer.
    let (cmd_tx, cmd_rx) = neomacs_display_runtime::thread_comm::command_channel(64);
    let catalog = AsyncImageCatalog::new(cmd_tx, None, Arc::new(ImageRenderState::default()), None);
    let fixture = neomacs_infra::workspace_root().join("test/data/image/blank-100x200.png");
    let request = file_request(fixture.to_str().unwrap());
    assert_eq!(
        catalog.admission_limit(&request, ImageSizeLimit::UNLIMITED),
        ImageSizeLimit::UNLIMITED
    );
    assert!(catalog.semantic_queries.pixels(&request).is_none());
    assert!(catalog.semantic_queries.admission(&request).is_none());
    catalog.lookup(request.clone(), ImageSizeLimit::from_axis_pixels(1));
    assert!(
        matches!(cmd_rx.try_recv().unwrap(), RenderCommand::Asset(AssetCommand::ImageLoadFile { limit, .. }) if limit == ImageSizeLimit::UNLIMITED)
    );
    assert!(
        catalog
            .resolve_sync(request.clone(), ImageSizeLimit::from_axis_pixels(1))
            .is_ok()
    );
    assert!(catalog.invalidate(ImageInvalidation::All).changed());
    assert_eq!(
        catalog.admission_limit(&request, ImageSizeLimit::from_axis_pixels(1)),
        ImageSizeLimit::from_axis_pixels(1)
    );
    // Semantic-only entries also retire their admission on explicit invalidation.
    catalog
        .resolve_sync(request.clone(), ImageSizeLimit::UNLIMITED)
        .unwrap_err();
    assert!(catalog.invalidate(ImageInvalidation::All).changed());
    assert!(!catalog.admission_limits.borrow().contains_key(&request));
}

#[test]
fn semantic_decode_trait_dispatch_preserves_cache_invalidation_and_reconciliation() {
    let (cmd_tx, _cmd_rx) = neomacs_display_runtime::thread_comm::command_channel(64);
    let metadata = Arc::new(ImageRenderState::default());
    let catalog = AsyncImageCatalog::new(cmd_tx, None, Arc::clone(&metadata), None);
    let fixture = neomacs_infra::workspace_root().join("test/data/image/blank-100x200.png");
    let request = file_request(fixture.to_str().unwrap());
    let ready = catalog
        .resolve_sync(request.clone(), ImageSizeLimit::UNLIMITED)
        .unwrap()
        .unwrap();
    let boundary: &dyn ImageCatalog = &catalog;
    assert!(boundary.cached_size_bytes() > 0);
    let ImageLookup::Pending(pending) = boundary.lookup(request.clone(), ImageSizeLimit::UNLIMITED)
    else {
        panic!("no drawable residency");
    };
    metadata.publish_terminal(
        pending.load(),
        ImageDecodeTerminal::Ready(ready.metadata.clone()),
    );
    boundary.reconcile_renderer_state(ImageStateEvent::DecodeCompleted(pending.load()));
    assert!(matches!(
        catalog.entries.borrow().get(&request),
        Some(CatalogEntry::Resident(_))
    ));
    assert!(
        boundary
            .invalidate_animation(ImageAnimationInvalidation::All)
            .changed()
    );
    assert!(boundary.invalidate(ImageInvalidation::All).changed());
    assert_eq!(boundary.cached_size_bytes(), 0);
    let replacement = catalog
        .resolve_sync(request, ImageSizeLimit::UNLIMITED)
        .unwrap()
        .unwrap();
    assert_ne!(ready.load, replacement.load);
}

#[test]
fn semantic_decode_production_queries_complete_pixel_and_animation_semantics() {
    use neomacs_display_protocol::{ImageFrameIndex, ImageMaskKind};
    let (cmd_tx, unserviced_rx) = neomacs_display_runtime::thread_comm::command_channel(64);
    let catalog = AsyncImageCatalog::new(cmd_tx, None, Arc::new(ImageRenderState::default()), None);
    // Actual encoded PNG streams: opaque, binary-transparent, continuous alpha.
    let streams = [
        (
            vec![
                137, 80, 78, 71, 13, 10, 26, 10, 0, 0, 0, 13, 73, 72, 68, 82, 0, 0, 0, 2, 0, 0, 0,
                2, 8, 6, 0, 0, 0, 114, 182, 13, 36, 0, 0, 0, 17, 73, 68, 65, 84, 120, 156, 99, 16,
                50, 9, 251, 15, 194, 12, 48, 6, 0, 52, 12, 6, 109, 214, 210, 193, 5, 0, 0, 0, 0,
                73, 69, 78, 68, 174, 66, 96, 130,
            ],
            ImageMaskKind::None,
            false,
        ),
        (
            vec![
                137, 80, 78, 71, 13, 10, 26, 10, 0, 0, 0, 13, 73, 72, 68, 82, 0, 0, 0, 2, 0, 0, 0,
                2, 8, 6, 0, 0, 0, 114, 182, 13, 36, 0, 0, 0, 17, 73, 68, 65, 84, 120, 156, 99, 16,
                50, 9, 99, 0, 97, 6, 24, 3, 0, 22, 42, 2, 113, 226, 237, 10, 156, 0, 0, 0, 0, 73,
                69, 78, 68, 174, 66, 96, 130,
            ],
            ImageMaskKind::AlphaChannel,
            true,
        ),
        (
            vec![
                137, 80, 78, 71, 13, 10, 26, 10, 0, 0, 0, 13, 73, 72, 68, 82, 0, 0, 0, 2, 0, 0, 0,
                2, 8, 6, 0, 0, 0, 114, 182, 13, 36, 0, 0, 0, 17, 73, 68, 65, 84, 120, 156, 99, 16,
                50, 9, 107, 0, 97, 6, 24, 3, 0, 37, 42, 4, 113, 236, 234, 27, 16, 0, 0, 0, 0, 73,
                69, 78, 68, 174, 66, 96, 130,
            ],
            ImageMaskKind::AlphaChannel,
            false,
        ),
    ];
    for (index, (bytes, mask, transparent)) in streams.into_iter().enumerate() {
        let mut request = file_request(&format!("data-pixel-{index}.png"));
        request.source = ImageResolveSource::Data(ImageDataSource::Isolated(
            neomacs_display_protocol::image::EncodedBytes::new(bytes),
        ));
        request.size = ImageSizeSpec::default();
        let result = catalog
            .resolve_sync(request, ImageSizeLimit::UNLIMITED)
            .unwrap()
            .unwrap();
        assert_eq!(result.metadata.layout.dimensions(), (2, 2));
        assert_eq!(result.metadata.background, 0x123456);
        assert_eq!(result.metadata.mask, mask);
        assert_eq!(result.metadata.background_transparent, transparent);
    }
    // Real two-frame 1x1 GIF, selected green frame delayed 40 milliseconds.
    let mut request = file_request("data-animation.gif");
    request.source = ImageResolveSource::Data(ImageDataSource::Isolated(
        neomacs_display_protocol::image::EncodedBytes::new(vec![
            71, 73, 70, 56, 57, 97, 1, 0, 1, 0, 128, 0, 0, 255, 0, 0, 0, 255, 0, 33, 249, 4, 0, 2,
            0, 0, 0, 44, 0, 0, 0, 0, 1, 0, 1, 0, 0, 2, 2, 68, 1, 0, 33, 249, 4, 0, 4, 0, 0, 0, 44,
            0, 0, 0, 0, 1, 0, 1, 0, 0, 2, 2, 76, 1, 0, 59,
        ]),
    ));
    request.size = ImageSizeSpec::default();
    request.frame = ImageFrameIndex::new(1);
    let ready = catalog
        .resolve_sync(request.clone(), ImageSizeLimit::UNLIMITED)
        .unwrap()
        .unwrap();
    assert_eq!(ready.metadata.background, 0x00ff00);
    assert_eq!(ready.metadata.embedded.frame_count(), Some(2));
    assert_eq!(
        ready.metadata.embedded.frame_delay(),
        Some(neomacs_display_protocol::ImageFrameDelay::milliseconds(40, 1).unwrap())
    );
    request.frame = ImageFrameIndex::new(2);
    assert!(
        catalog
            .resolve_sync(request, ImageSizeLimit::UNLIMITED)
            .is_err()
    );
    let fixture = neomacs_infra::workspace_root().join("test/data/image/blank-100x200.png");
    let mut malformed = file_request("data-truncated.png");
    malformed.source = ImageResolveSource::Data(ImageDataSource::Isolated(
        neomacs_display_protocol::image::EncodedBytes::new(
            std::fs::read(fixture).unwrap()[..33].to_vec(),
        ),
    ));
    assert!(
        catalog
            .resolve_sync(malformed, ImageSizeLimit::UNLIMITED)
            .is_err(),
        "complete IHDR must not count as semantic completion"
    );
    assert!(
        unserviced_rx.try_recv().is_err(),
        "queries require no renderer admission or service"
    );
}

#[test]
fn semantic_decode_does_not_require_even_a_connected_renderer() {
    let (cmd_tx, cmd_rx) = neomacs_display_runtime::thread_comm::command_channel(1);
    drop(cmd_rx);
    let catalog = AsyncImageCatalog::new(cmd_tx, None, Arc::new(ImageRenderState::default()), None);
    let fixture = neomacs_infra::workspace_root().join("test/data/image/blank-100x200.png");
    assert!(
        catalog
            .resolve_sync(
                file_request(fixture.to_str().unwrap()),
                ImageSizeLimit::UNLIMITED
            )
            .is_ok()
    );
}

#[test]
fn semantic_decode_failure_without_renderer_service_reports_diagnostic() {
    let (cmd_tx, _cmd_rx) = neomacs_display_runtime::thread_comm::command_channel(1);
    let catalog = AsyncImageCatalog::new(cmd_tx, None, Arc::new(ImageRenderState::default()), None);
    let fixture = neomacs_infra::workspace_root().join("test/data/image/blank-100x200.png");
    let request = file_request(fixture.to_str().unwrap());
    assert_eq!(
        catalog.resolve_sync(request.clone(), ImageSizeLimit::from_axis_pixels(1)),
        Err(ImageDiagnostic::InvalidSize.message())
    );
    assert_eq!(
        catalog.take_pending_diagnostics(),
        vec![ImageDiagnostic::InvalidSize.message()]
    );
    assert_eq!(
        catalog.resolve_sync(request, ImageSizeLimit::UNLIMITED),
        Err(ImageDiagnostic::InvalidSize.message()),
        "cached failures remain cached until flush"
    );
    assert_eq!(
        catalog.take_pending_diagnostics(),
        vec![ImageDiagnostic::InvalidSize.message()]
    );
}

/// Production-boundary measurement, not a GNU operation benchmark. Keeping
/// the real command receiver alive without consuming commands models an owner
/// that has stopped servicing work. Header geometry must not be mistaken for
/// complete semantic metadata (mask/background/animation still need decoding).
#[cfg(target_os = "linux")]
#[test]
#[ignore = "explicit image metadata boundary profile; one-second stalls"]
fn profile_synchronous_metadata_with_unserviced_render_owner() {
    for repetition in 0..5 {
        let (cmd_tx, cmd_rx) = neomacs_display_runtime::thread_comm::command_channel(64);
        let metadata = Arc::new(ImageRenderState::default());
        let catalog = AsyncImageCatalog::new(cmd_tx, None, Arc::clone(&metadata), None);
        let fixture = neomacs_infra::workspace_root().join("test/data/image/blank-100x200.png");
        let mut request = file_request(fixture.to_str().expect("utf8 fixture"));
        request.size = ImageSizeSpec::new(AxisSize::AtMost(50), AxisSize::Native);
        let ImageLookup::Pending(pending) = lookup(&catalog, request.clone()) else {
            panic!("renderer has not serviced the load");
        };
        let header_deadline = Instant::now() + Duration::from_secs(10);
        while lookup(&catalog, request.clone()).placement().dimensions() != (50, 100) {
            assert!(
                Instant::now() < header_deadline,
                "header probe did not finish"
            );
            std::thread::sleep(Duration::from_millis(1));
        }
        let sched_before = std::fs::read_to_string("/proc/thread-self/schedstat").unwrap();
        let start = Instant::now();
        let result = catalog.resolve_sync(request.clone(), ImageSizeLimit::UNLIMITED);
        let wall_ns = start.elapsed().as_nanos();
        let sched_after = std::fs::read_to_string("/proc/thread-self/schedstat").unwrap();
        let parse = |text: &str| {
            text.split_whitespace()
                .take(2)
                .map(|value| value.parse::<u64>().unwrap())
                .collect::<Vec<_>>()
        };
        let before = parse(&sched_before);
        let after = parse(&sched_after);
        let ready = result.as_ref().unwrap().as_ref().unwrap();
        assert_eq!(ready.metadata.layout.dimensions(), (50, 100));
        let warm_start = Instant::now();
        let warm = catalog.resolve_sync(request, ImageSizeLimit::UNLIMITED);
        let warm_ns = warm_start.elapsed().as_nanos();
        assert!(warm.is_ok());
        assert!(metadata.terminal(pending.load()).is_none());
        assert!(
            matches!(cmd_rx.try_recv(), Ok(RenderCommand::Asset(AssetCommand::ImageLoadFile { load, .. })) if load == pending.load())
        );
        println!(
            "R021_METADATA_PROFILE rep={repetition} cold_ns={wall_ns} warm_ns={warm_ns} cpu_ns={} runnable_ns={} header=50x100 residency=absent result={result:?}",
            after[0] - before[0],
            after[1] - before[1]
        );
    }
}
