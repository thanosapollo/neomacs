use std::cell::Cell;

thread_local! {
    static DEPTH: Cell<usize> = const { Cell::new(0) };
    static MAX_DEPTH: Cell<usize> = const { Cell::new(0) };
    static POINT_SCROLL_REJECTIONS: Cell<usize> = const { Cell::new(0) };
}

pub(super) struct Guard;

impl Guard {
    pub(super) fn enter() -> Self {
        DEPTH.with(|depth| {
            let next = depth.get() + 1;
            depth.set(next);
            MAX_DEPTH.with(|max| {
                if next > max.get() {
                    max.set(next);
                }
            });
        });
        Guard
    }
}

impl Drop for Guard {
    fn drop(&mut self) {
        DEPTH.with(|depth| depth.set(depth.get() - 1));
    }
}

pub(super) fn reset() {
    MAX_DEPTH.with(|max| max.set(0));
    POINT_SCROLL_REJECTIONS.with(|count| count.set(0));
}

pub(super) fn max_depth() -> usize {
    MAX_DEPTH.with(|max| max.get())
}

pub(super) fn note_point_scroll_rejection() {
    POINT_SCROLL_REJECTIONS.with(|count| count.set(count.get() + 1));
}

pub(super) fn point_scroll_rejections() -> usize {
    POINT_SCROLL_REJECTIONS.with(Cell::get)
}
