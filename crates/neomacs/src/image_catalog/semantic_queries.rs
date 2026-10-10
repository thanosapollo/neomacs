//! Complete semantic image queries, with no renderer submission capability.
//!
//! These queries share the production decoder implementation, not its residency
//! lifecycle. A GPU reset/eviction cannot erase a completed CPU answer; a Lisp
//! cache invalidation retires its token before a late worker can publish.
use super::*;
use neomacs_display_runtime::render_thread::{
    SemanticImageDecoded, SemanticImageDecoder, SemanticImageRequest,
    SemanticImageSequenceReservation, SemanticImageSource, SvgResourceContext,
};
use neovm_core::emacs_core::image_catalog::{ImageDataSource, ResolvedImageMetadata};
use std::sync::Condvar;

type DecodeResult =
    Result<(ResolvedImageMetadata, Option<Arc<SemanticImageDecoded>>), ImageDiagnostic>;

// Retain complete answers/admission errors until explicit Lisp invalidation,
// but keep only a bounded FIFO of optional prepared RGBA. This is a CPU budget,
// independent of renderer residency and the decoder's sequence-cache budget.
const PREPARED_RGBA_BUDGET: u64 = 64 * 1024 * 1024;

#[derive(Default)]
struct CompletedLoads {
    results: HashMap<ImageLoadToken, Option<DecodeResult>>,
    pixels: std::collections::VecDeque<ImageLoadToken>,
    pixel_bytes: u64,
}

struct Completion {
    loads: Mutex<CompletedLoads>,
    changed: Condvar,
    pixel_budget: u64,
}

impl Default for Completion {
    fn default() -> Self {
        Self::new(PREPARED_RGBA_BUDGET)
    }
}

impl Completion {
    fn new(pixel_budget: u64) -> Self {
        Self {
            loads: Mutex::new(CompletedLoads::default()),
            changed: Condvar::new(),
            pixel_budget,
        }
    }

    fn begin(&self, load: ImageLoadToken) {
        self.loads
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .results
            .insert(load, None);
    }

    fn publish(&self, load: ImageLoadToken, mut result: DecodeResult) {
        let mut loads = self.loads.lock().unwrap_or_else(|e| e.into_inner());
        // Retired or already completed tokens cannot add bytes or evict others.
        if !matches!(loads.results.get(&load), Some(None)) {
            return;
        }
        if let Ok((_, pixels)) = &mut result {
            if let Some(decoded) = pixels.as_ref() {
                let bytes = decoded.size_bytes() as u64;
                if bytes > self.pixel_budget {
                    // Oversized realizations still answer complete semantics.
                    // Do not evict useful retained images for an uncacheable one.
                    *pixels = None;
                } else {
                    while loads.pixel_bytes > self.pixel_budget - bytes {
                        let oldest = loads
                            .pixels
                            .pop_front()
                            .expect("accounted pixels have a token");
                        if let Some(Some(Ok((_, pixels)))) = loads.results.get_mut(&oldest) {
                            let decoded = pixels.take().expect("FIFO token owns prepared pixels");
                            loads.pixel_bytes -= decoded.size_bytes() as u64;
                        }
                    }
                    loads.pixel_bytes += bytes;
                    loads.pixels.push_back(load);
                }
            }
        }
        loads.results.insert(load, Some(result));
        self.changed.notify_all();
    }

    fn retire(&self, load: ImageLoadToken) {
        let mut loads = self.loads.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(Some(Ok((_, Some(decoded))))) = loads.results.remove(&load) {
            loads.pixel_bytes -= decoded.size_bytes() as u64;
            loads.pixels.retain(|token| *token != load);
        }
        self.changed.notify_all();
    }

    fn pixels(&self, load: ImageLoadToken) -> Option<Arc<SemanticImageDecoded>> {
        // Drawable lookup must not wait for a decoder publishing/evicting pixels.
        // Contention takes the existing encoded-source fallback instead.
        let loads = match self.loads.try_lock() {
            Ok(loads) => loads,
            Err(std::sync::TryLockError::WouldBlock) => return None,
            Err(std::sync::TryLockError::Poisoned(error)) => error.into_inner(),
        };
        match loads.results.get(&load) {
            Some(Some(Ok((_, pixels)))) => pixels.clone(),
            _ => None,
        }
    }

    fn pixel_bytes(&self) -> u64 {
        self.loads
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .pixel_bytes
    }

    fn wait(&self, load: ImageLoadToken, timeout: Duration) -> Option<DecodeResult> {
        let deadline = std::time::Instant::now() + timeout;
        let mut loads = self.loads.lock().unwrap_or_else(|e| e.into_inner());
        loop {
            match loads.results.get(&load)? {
                Some(result) => return Some(result.clone()),
                None => {}
            }
            let remaining = deadline.checked_duration_since(std::time::Instant::now())?;
            let (guard, _) = self
                .changed
                .wait_timeout(loads, remaining)
                .unwrap_or_else(|e| e.into_inner());
            loads = guard;
        }
    }
}

#[derive(Clone)]
struct SemanticEntry {
    pending: PendingImage,
    // Only complete successes are copied into evaluator-owned storage. This
    // holds metadata, never prepared RGBA, and dies with the existing entry.
    ready: Option<ReadyImage>,
}

impl SemanticEntry {
    fn load(&self) -> ImageLoadToken {
        self.pending.load()
    }
}

#[derive(Default)]
pub(super) struct SemanticQueries {
    entries: RefCell<HashMap<ImageResolveRequest, SemanticEntry>>,
    limits: RefCell<HashMap<ImageResolveRequest, ImageSizeLimit>>,
    admissions: RefCell<HashMap<ImageResolveRequest, Result<ImageSizeLimit, ImageDiagnostic>>>,
    completion: Arc<Completion>,
    decoder: Arc<SemanticImageDecoder>,
}

impl SemanticQueries {
    #[cfg(test)]
    pub(super) fn with_pixel_budget(bytes: u64) -> Self {
        Self {
            completion: Arc::new(Completion::new(bytes)),
            ..Self::default()
        }
    }

    pub(super) fn resolve(
        &self,
        catalog: &AsyncImageCatalog,
        request: ImageResolveRequest,
        limit: ImageSizeLimit,
    ) -> Result<Option<ReadyImage>, String> {
        let (request, resolution) = catalog.classify_request(request);
        if let Some(ready) = self
            .entries
            .borrow()
            .get(&request)
            .and_then(|entry| entry.ready.clone())
        {
            return Ok(Some(ready));
        }
        // Admission belongs to the first realization attempt, even while its
        // renderer terminal is unserviced. A query must not re-admit that same
        // identity under a later dynamically changed max-image-size.
        let limit = catalog.admission_limit(&request, limit);
        let pending = {
            let mut entries = self.entries.borrow_mut();
            entries
                .entry(request.clone())
                .or_insert_with(|| {
                    let load = catalog.next_load(next_host_image_id());
                    let pending = PendingImage::new(load, placeholder_image_extent(&request));
                    self.completion.begin(load);
                    self.limits.borrow_mut().insert(request.clone(), limit);
                    // GNU keeps already-loaded realizations when max-image-size
                    // changes. Reuse an evaluator-observed complete decode, never
                    // a renderer lock/header/band or a scheduling failure.
                    let known =
                        catalog
                            .entries
                            .borrow()
                            .get(&request)
                            .and_then(|entry| match entry {
                                CatalogEntry::Resident(ready) => {
                                    Some(Ok((ready.metadata.clone(), None)))
                                }
                                CatalogEntry::Failed(failed)
                                    if failed.error != ImageDiagnostic::NotDrawable =>
                                {
                                    Some(Err(failed.error.clone()))
                                }
                                _ => None,
                            });
                    if let Some(result) = known {
                        if result.is_ok() {
                            self.limits
                                .borrow_mut()
                                .insert(request.clone(), ImageSizeLimit::UNLIMITED);
                        }
                        self.completion.publish(load, result);
                        return SemanticEntry {
                            pending,
                            ready: None,
                        };
                    }
                    let sequence = catalog.sequence_id(&request.source);
                    let reservation = self.decoder.reserve_sequence(sequence);
                    let job = Job {
                        _reservation: reservation,
                        request: request.clone(),
                        resolution,
                        limit,
                        sequence,
                        load,
                        completion: Arc::clone(&self.completion),
                        decoder: Arc::clone(&self.decoder),
                    };
                    if sender().send(job).is_err() {
                        self.completion
                            .publish(load, Err(ImageDiagnostic::NotDrawable));
                    }
                    SemanticEntry {
                        pending,
                        ready: None,
                    }
                })
                .pending
                .clone()
        };
        let Some(result) = self.completion.wait(pending.load(), Duration::from_secs(1)) else {
            return Err(format!(
                "Timed out waiting for image decode (id {})",
                pending.load().image()
            ));
        };
        let admission = result
            .as_ref()
            .map(|_| self.limits.borrow()[&request])
            .map_err(Clone::clone);
        self.admissions
            .borrow_mut()
            .insert(request.clone(), admission);
        match result {
            Ok((metadata, _)) => {
                let ready = ReadyImage {
                    load: pending.load(),
                    metadata,
                };
                self.entries
                    .borrow_mut()
                    .get_mut(&request)
                    .expect("semantic entry survives synchronous completion")
                    .ready = Some(ready.clone());
                Ok(Some(ready))
            }
            Err(error) => {
                let failed = pending.failed(error);
                catalog.record_failure_always(&failed);
                Err(failed.error.message())
            }
        }
    }

    pub(super) fn invalidate(
        &self,
        target: &ImageInvalidation,
        catalog: &AsyncImageCatalog,
    ) -> bool {
        let mut entries = self.entries.borrow_mut();
        let mut changed = false;
        entries.retain(|request, pending| {
            let matches = match target {
                ImageInvalidation::Spec { spec } => request.spec == *spec,
                ImageInvalidation::Dependency(source) => request.source == *source,
                ImageInvalidation::All => true,
            };
            if matches {
                self.completion.retire(pending.load());
                catalog.admission_limits.borrow_mut().remove(request);
                self.limits.borrow_mut().remove(request);
                self.admissions.borrow_mut().remove(request);
                catalog
                    .reported_failures
                    .borrow_mut()
                    .remove(&pending.load().image());
                changed = true;
            }
            !matches
        });
        changed
    }

    pub(super) fn admission(
        &self,
        request: &ImageResolveRequest,
    ) -> Option<Result<ImageSizeLimit, ImageDiagnostic>> {
        self.admissions.borrow().get(request).cloned()
    }

    pub(super) fn pixels(
        &self,
        request: &ImageResolveRequest,
    ) -> Option<Arc<SemanticImageDecoded>> {
        let load = self.entries.borrow().get(request)?.load();
        self.completion.pixels(load)
    }

    pub(super) fn retire_sequence(&self, retirement: ImageSequenceRetirement) {
        self.decoder.retire(retirement);
    }

    pub(super) fn cached_size_bytes(&self) -> u64 {
        u64::try_from(self.decoder.cached_size_bytes())
            .unwrap_or(u64::MAX)
            .saturating_add(self.completion.pixel_bytes())
    }
}

struct Job {
    _reservation: SemanticImageSequenceReservation,
    request: ImageResolveRequest,
    resolution: Option<ImageFileRequest>,
    limit: ImageSizeLimit,
    sequence: ImageSequenceId,
    load: ImageLoadToken,
    completion: Arc<Completion>,
    decoder: Arc<SemanticImageDecoder>,
}

fn sender() -> &'static crossbeam_channel::Sender<Job> {
    static SENDER: OnceLock<crossbeam_channel::Sender<Job>> = OnceLock::new();
    SENDER.get_or_init(|| {
        let (tx, rx) = crossbeam_channel::unbounded::<Job>();
        for index in 0..4 {
            let rx = rx.clone();
            std::thread::Builder::new()
                .name(format!("neomacs-image-semantic-{index}"))
                .spawn(move || {
                    while let Ok(job) = rx.recv() {
                        run(job);
                    }
                })
                .expect("semantic image worker");
        }
        tx
    })
}

fn run(job: Job) {
    let request = job.request;
    let source = match request.source {
        ImageResolveSource::File(path) => SemanticImageSource::File(
            job.resolution
                .as_ref()
                .and_then(ImageFileRequest::resolve)
                .unwrap_or_else(|| path.as_utf8_str().unwrap_or_default().to_owned()),
        ),
        ImageResolveSource::Data(ImageDataSource::Isolated(data)) => SemanticImageSource::Data {
            data,
            resources: SvgResourceContext::Isolated,
        },
        ImageResolveSource::Data(ImageDataSource::WithBaseUri { data, base_uri }) => {
            SemanticImageSource::Data {
                data,
                resources: SvgResourceContext::BaseUri(
                    base_uri.as_utf8_str().unwrap_or_default().to_owned(),
                ),
            }
        }
    };
    let result = job
        .decoder
        .decode_pixels(SemanticImageRequest {
            source,
            size: request.size,
            rotation: request.rotation,
            realization: request.realization,
            colors: request.colors,
            mask: request.mask,
            animation: request.animation,
            frame: request.frame,
            sequence: job.sequence,
            limit: job.limit,
            identity: request.identity,
        })
        .map(|decoded| {
            let metadata = decoded.metadata();
            (
                ResolvedImageMetadata {
                    layout: metadata.layout,
                    reported: metadata.reported,
                    background: metadata.background,
                    background_transparent: metadata.background_transparent,
                    mask: metadata.mask,
                    embedded: metadata.embedded.clone(),
                },
                Some(Arc::new(decoded)),
            )
        });
    job.completion.publish(job.load, result);
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn warm_semantics_do_not_reenter_admission_or_worker_state() {
        let (tx, _rx) = neomacs_display_runtime::thread_comm::command_channel(1);
        let catalog = AsyncImageCatalog::new(
            tx,
            None,
            Arc::new(neomacs_display_runtime::render_thread::ImageRenderState::default()),
            None,
        );
        let fixture = neomacs_infra::workspace_root().join("test/data/image/blank-100x200.png");
        let request = super::super::tests::file_request(fixture.to_str().unwrap());
        let expected = catalog
            .resolve_sync(request.clone(), ImageSizeLimit::UNLIMITED)
            .unwrap();
        // A completed success is evaluator-owned. Dynamic admission and decoder
        // publication cannot affect that answer until explicit invalidation.
        let _admission = catalog.admission_limits.borrow_mut();
        let _limits = catalog.semantic_queries.limits.borrow_mut();
        let _admissions = catalog.semantic_queries.admissions.borrow_mut();
        let _worker = catalog.semantic_queries.completion.loads.lock().unwrap();
        assert_eq!(
            catalog
                .resolve_sync(request, ImageSizeLimit::from_axis_pixels(1))
                .unwrap(),
            expected
        );
    }

    #[test]
    #[ignore = "explicit warm catalog component timing"]
    fn profile_warm_catalog_components() {
        use std::hint::black_box;
        let (tx, _rx) = neomacs_display_runtime::thread_comm::command_channel(1);
        let catalog = AsyncImageCatalog::new(
            tx,
            None,
            Arc::new(neomacs_display_runtime::render_thread::ImageRenderState::default()),
            None,
        );
        let fixture = neomacs_infra::workspace_root().join("test/data/image/blank-100x200.png");
        let request = super::super::tests::file_request(fixture.to_str().unwrap());
        let ready = catalog
            .resolve_sync(request.clone(), ImageSizeLimit::UNLIMITED)
            .unwrap()
            .unwrap();
        for block in 0..7 {
            let n = 20000;
            let start = std::time::Instant::now();
            for _ in 0..n {
                black_box(catalog.classify_request(black_box(request.clone())));
            }
            println!(
                "R025_NATIVE block={block} component=classify n={n} ns={}",
                start.elapsed().as_nanos()
            );
            let start = std::time::Instant::now();
            for _ in 0..n {
                black_box(
                    catalog
                        .semantic_queries
                        .completion
                        .wait(ready.load, Duration::from_secs(1)),
                );
            }
            println!(
                "R025_NATIVE block={block} component=completion n={n} ns={}",
                start.elapsed().as_nanos()
            );
            let (classified, _) = catalog.classify_request(request.clone());
            let start = std::time::Instant::now();
            for _ in 0..n {
                black_box(
                    catalog.admission_limit(black_box(&classified), ImageSizeLimit::UNLIMITED),
                );
            }
            println!(
                "R025_NATIVE block={block} component=admission n={n} ns={}",
                start.elapsed().as_nanos()
            );
            let start = std::time::Instant::now();
            for _ in 0..n {
                black_box(
                    catalog
                        .semantic_queries
                        .entries
                        .borrow()
                        .get(black_box(&classified))
                        .cloned(),
                );
            }
            println!(
                "R025_NATIVE block={block} component=entry n={n} ns={}",
                start.elapsed().as_nanos()
            );
            let start = std::time::Instant::now();
            for _ in 0..n {
                black_box(
                    catalog
                        .semantic_queries
                        .admissions
                        .borrow_mut()
                        .insert(classified.clone(), Ok(ImageSizeLimit::UNLIMITED)),
                );
            }
            println!(
                "R025_NATIVE block={block} component=admission_write n={n} ns={}",
                start.elapsed().as_nanos()
            );
            let start = std::time::Instant::now();
            for _ in 0..n {
                black_box(
                    catalog
                        .resolve_sync(black_box(request.clone()), ImageSizeLimit::UNLIMITED)
                        .unwrap(),
                );
            }
            println!(
                "R025_NATIVE block={block} component=resolve n={n} ns={}",
                start.elapsed().as_nanos()
            );
        }
    }

    #[test]
    fn prepared_pixels_do_not_wait_for_worker_mutex() {
        let completion = Arc::new(Completion::default());
        let load = ImageLoadToken::new(next_host_image_id(), ImageLoadAttempt::new(1).unwrap());
        completion.begin(load);
        let guard = completion.loads.lock().unwrap();
        let (tx, rx) = std::sync::mpsc::channel();
        let worker_completion = Arc::clone(&completion);
        let worker = std::thread::spawn(move || {
            tx.send(worker_completion.pixels(load)).unwrap();
        });
        let result = rx.recv_timeout(Duration::from_millis(100));
        drop(guard);
        worker.join().unwrap();
        assert!(
            matches!(result, Ok(None)),
            "drawable lookup waited for worker mutex"
        );
    }

    #[test]
    fn retired_epoch_cannot_publish_after_replacement() {
        let completion = Completion::default();
        let image = next_host_image_id();
        let old = ImageLoadToken::new(image, ImageLoadAttempt::new(1).unwrap());
        let new = ImageLoadToken::new(image, ImageLoadAttempt::new(2).unwrap());
        completion.begin(old);
        completion.retire(old);
        completion.begin(new);
        completion.publish(old, Err(ImageDiagnostic::InvalidSize));
        assert!(completion.wait(old, Duration::ZERO).is_none());
        assert!(completion.wait(new, Duration::ZERO).is_none());
        completion.publish(new, Err(ImageDiagnostic::NotDrawable));
        assert!(matches!(
            completion.wait(new, Duration::ZERO),
            Some(Err(ImageDiagnostic::NotDrawable))
        ));
    }
}
