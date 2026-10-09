use super::*;
use std::io::BufReader;

fn assert_readback_configuration(supported: bool, first: bool, continuous: u32) {
    let capabilities = wgpu::TextureUsages::RENDER_ATTACHMENT
        | if supported {
            wgpu::TextureUsages::COPY_SRC
        } else {
            wgpu::TextureUsages::empty()
        };
    let mut pending = first;
    let mut remaining = continuous;
    let usage = surface_usage_for_debug_readback(capabilities, &mut pending, &mut remaining);
    let enabled = supported && (first || continuous > 0);
    assert_eq!(
        usage,
        wgpu::TextureUsages::RENDER_ATTACHMENT
            | if enabled {
                wgpu::TextureUsages::COPY_SRC
            } else {
                wgpu::TextureUsages::empty()
            }
    );
    assert_eq!(pending, supported && first);
    assert_eq!(remaining, if supported { continuous } else { 0 });
}

#[test]
fn unsupported_first_frame_readback_is_disarmed() {
    assert_readback_configuration(false, true, 0);
}

#[test]
fn unsupported_continuous_readback_is_disarmed() {
    assert_readback_configuration(false, false, 3);
}

#[test]
fn unsupported_combined_readback_is_disarmed() {
    assert_readback_configuration(false, true, 3);
}

#[test]
fn supported_first_frame_readback_retains_request_and_copy_src() {
    assert_readback_configuration(true, true, 0);
}

#[test]
fn supported_continuous_readback_retains_budget_and_copy_src() {
    assert_readback_configuration(true, false, 3);
}

#[test]
fn supported_combined_readback_retains_both_requests_and_copy_src() {
    assert_readback_configuration(true, true, 3);
}

#[test]
fn idle_surface_does_not_request_copy_src() {
    assert_readback_configuration(false, false, 0);
    assert_readback_configuration(true, false, 0);
}

#[test]
fn published_readback_remains_complete_while_next_capture_replaces_it() {
    let root = neomacs_infra::crate_root!().join("../../tmp");
    std::fs::create_dir_all(&root).unwrap();
    let directory = tempfile::tempdir_in(root).unwrap();
    let path = directory.path().join("readback.png");
    let publish = |color: [u8; 4]| {
        write_surface_readback_png(
            &path,
            &color.repeat(4),
            8,
            wgpu::TextureFormat::Rgba8Unorm,
            2,
            2,
        )
        .unwrap();
    };
    publish([255, 0, 0, 255]);
    // The reader may be the evaluator's copy-file, concurrently opening the
    // last capture while the render thread starts its next PNG. An opened
    // capture must remain immutable instead of being truncated underneath it.
    let previous = std::fs::File::open(&path).unwrap();
    publish([0, 0, 255, 255]);
    let previous = image::ImageReader::new(BufReader::new(previous))
        .with_guessed_format()
        .unwrap()
        .decode()
        .unwrap()
        .to_rgba8();
    assert_eq!(previous.get_pixel(0, 0).0, [255, 0, 0, 255]);
    assert_eq!(
        image::open(&path).unwrap().to_rgba8().get_pixel(0, 0).0,
        [0, 0, 255, 255]
    );
}
