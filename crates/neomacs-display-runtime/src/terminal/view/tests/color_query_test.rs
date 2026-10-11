use super::*;

fn palette() -> NeoTermPalette {
    let mut palette =
        NeoTermPalette::fallback(Color::from_pixel(0x123456), Color::from_pixel(0x789abc));
    palette.cursor = Color::from_pixel(0xabcdef);
    for slot in 0..16 {
        palette.ansi_foreground[slot] = Color::from_pixel(0x203040 + slot as u32);
        palette.ansi_background[slot] = Color::from_pixel(0xff0000);
    }
    palette
}

fn parser(proxy: &NeomacsEventProxy) -> Crosswords<NeomacsEventProxy> {
    Crosswords::new(
        CrosswordsSize::new(80, 24),
        CursorShape::Block,
        proxy.clone(),
        WindowId::from(0),
        0,
        SCROLLBACK_HISTORY,
    )
}

#[test]
fn color_queries_preserve_bel_and_st_and_default_channels() {
    let proxy = NeomacsEventProxy::new(TerminalId::new(98).unwrap());
    proxy.set_palette(palette());
    let mut term = parser(&proxy);
    Processor::default().advance(&mut term, b"\x1b]10;?\x07\x1b]11;?\x1b\\\x1b]12;?\x07");
    assert_eq!(proxy.take_pending_writes(),
        b"\x1b]10;rgb:1212/3434/5656\x07\x1b]11;rgb:7878/9a9a/bcbc\x1b\\\x1b]12;rgb:abab/cdcd/efef\x07");
}

#[test]
fn osc4_uses_foreground_slots_and_xterm_extended_palette() {
    let proxy = NeomacsEventProxy::new(TerminalId::new(98).unwrap());
    proxy.set_palette(palette());
    let mut term = parser(&proxy);
    let mut processor = Processor::default();
    let mut expected = Vec::new();
    for slot in 0..16 {
        processor.advance(&mut term, format!("\x1b]4;{slot};?\x07").as_bytes());
        let blue = 0x40 + slot;
        expected.extend(format!("\x1b]4;{slot};rgb:2020/3030/{blue:02x}{blue:02x}\x07").bytes());
    }
    processor.advance(&mut term, b"\x1b]4;16;?\x1b\\\x1b]4;255;?\x07");
    expected
        .extend_from_slice(b"\x1b]4;16;rgb:0000/0000/0000\x1b\\\x1b]4;255;rgb:eeee/eeee/eeee\x07");
    assert_eq!(proxy.take_pending_writes(), expected);
}

#[test]
fn palette_updates_reach_existing_processor_and_proxy_clones() {
    let proxy = NeomacsEventProxy::new(TerminalId::new(98).unwrap());
    proxy.set_palette(palette());
    let mut term = parser(&proxy);
    let mut processor = Processor::default();
    processor.advance(&mut term, b"\x1b]10;?\x07");
    proxy.take_pending_writes();
    let mut updated = palette();
    updated.foreground = Color::from_pixel(0x654321);
    updated.ansi_foreground[1] = Color::from_pixel(0x998877);
    proxy.clone().set_palette(updated);
    processor.advance(&mut term, b"\x1b]10;?\x07\x1b]4;1;?\x1b\\");
    assert_eq!(
        proxy.take_pending_writes(),
        b"\x1b]10;rgb:6565/4343/2121\x07\x1b]4;1;rgb:9999/8888/7777\x1b\\"
    );
}

#[test]
fn invalid_color_indices_do_not_reply() {
    let proxy = NeomacsEventProxy::new(TerminalId::new(98).unwrap());
    let mut term = parser(&proxy);
    Processor::default().advance(
        &mut term,
        b"\x1b]4;256;?\x07\x1b]4;999999;?\x1b\\\x1b]4;-1;?\x07",
    );
    assert!(proxy.take_pending_writes().is_empty());
    for index in [259, usize::MAX] {
        proxy.send_event(
            RioEvent::ColorRequest(0, index, Arc::new(|_| panic!("invalid index formatted"))),
            WindowId::from(0),
        );
    }
    assert!(proxy.take_pending_writes().is_empty());
}

#[cfg(unix)]
#[test]
fn color_queries_use_exact_pty_reply_transport_in_fifo_order() {
    use std::os::fd::AsRawFd;
    use std::os::unix::net::UnixStream;
    let mut proxy = NeomacsEventProxy::new(TerminalId::new(98).unwrap());
    proxy.set_palette(palette());
    let transport = Arc::new(transport::Transport::new());
    proxy.exact_transport = Some(transport.clone());
    let mut term = parser(&proxy);
    Processor::default().advance(&mut term, b"\x1b[5n\x1b]10;?\x1b\\\x1b[5n");
    assert!(proxy.take_pending_writes().is_empty());
    let expected = b"\x1b[0n\x1b]10;rgb:1212/3434/5656\x1b\\\x1b[0n";
    let (writer, mut reader) = UnixStream::pair().unwrap();
    transport.pump(writer.as_raw_fd());
    let mut actual = vec![0; expected.len()];
    reader.read_exact(&mut actual).unwrap();
    assert_eq!(actual, expected);
}
