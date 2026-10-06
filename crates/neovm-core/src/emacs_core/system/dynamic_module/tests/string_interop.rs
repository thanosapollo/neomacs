use super::*;

fn assert_copy(value: Value, expected: &[u8]) {
    let mut fixture = TestEnv::new();
    let env = fixture.env_ptr();
    let value = lisp_to_value(env, value);
    let copy = fixture.env.copy_string_contents.unwrap();
    let mut len = -17;
    assert!(unsafe { copy(env, value, std::ptr::null_mut(), &mut len) });
    assert_eq!(len, expected.len() as isize + 1);
    let mut buf = vec![0x55_u8; len as usize + 2];
    let mut capacity = buf.len() as isize;
    assert!(unsafe { copy(env, value, buf.as_mut_ptr().cast(), &mut capacity) });
    assert_eq!(capacity, len);
    assert_eq!(&buf[..expected.len()], expected);
    assert_eq!(buf[expected.len()], 0);
    assert_eq!(&buf[expected.len() + 1..], &[0x55, 0x55]);
    // An exact-fit buffer is sufficient; no extra byte beyond the terminator
    // may be required or written.
    let mut exact = vec![0x55_u8; len as usize];
    let mut exact_capacity = len;
    assert!(unsafe { copy(env, value, exact.as_mut_ptr().cast(), &mut exact_capacity) });
    assert_eq!(exact_capacity, len);
    assert_eq!(&exact[..expected.len()], expected);
    assert_eq!(exact[expected.len()], 0);
    assert_eq!(
        fixture.priv_.pending_non_local_exit,
        emacs_funcall_exit::Return
    );
}

#[test]
fn copy_string_contents_preserves_all_unibyte_octets_and_nul() {
    let _ctx = Context::new();
    for bytes in [
        vec![],
        vec![65, 0, 66],
        vec![128],
        vec![255],
        vec![195, 169],
        (0..=255).collect(),
    ] {
        assert_copy(
            Value::heap_string(LispString::from_unibyte(bytes.clone())),
            &bytes,
        );
    }
}

#[test]
fn copy_string_contents_encodes_multibyte_unicode() {
    let _ctx = Context::new();
    for text in ["", "ASCII\0tail", "Ελληνικά é 😀"] {
        assert_copy(Value::string(text), text.as_bytes());
    }
}

#[test]
fn copy_string_contents_rejects_multibyte_raw_octets_without_touching_outputs() {
    let _ctx = Context::new();
    let value = Value::heap_string(LispString::from_emacs_bytes(vec![0xc1, 0xbf]));
    assert!(value.as_lisp_string().unwrap().is_multibyte());
    let mut fixture = TestEnv::new();
    let env = fixture.env_ptr();
    let handle = lisp_to_value(env, value);
    let mut len = 9;
    let mut buf = [0x55_u8; 9];
    assert!(!unsafe {
        module_copy_string_contents(env, handle, buf.as_mut_ptr().cast(), &mut len)
    });
    assert_eq!(len, 9);
    assert_eq!(buf, [0x55; 9]);
    assert_eq!(
        fixture.priv_.non_local_exit_symbol,
        Value::symbol("wrong-type-argument")
    );
    assert_eq!(
        list_to_vec(&fixture.priv_.non_local_exit_data).unwrap(),
        vec![Value::symbol("unicode-string-p"), value]
    );
}

#[test]
fn copy_string_contents_short_buffer_reports_required_size_and_preserves_bytes() {
    let _ctx = Context::new();
    let mut fixture = TestEnv::new();
    let env = fixture.env_ptr();
    let value = lisp_to_value(
        env,
        Value::heap_string(LispString::from_unibyte(vec![0, 128, 255])),
    );
    for capacity in [-1, 0, 3] {
        unsafe { module_non_local_exit_clear(env) };
        let mut len = capacity;
        let mut buf = [0x55_u8; 5];
        assert!(!unsafe {
            module_copy_string_contents(env, value, buf.as_mut_ptr().cast(), &mut len)
        });
        assert_eq!(len, 4);
        assert_eq!(buf, [0x55; 5]);
        assert_eq!(
            fixture.priv_.non_local_exit_symbol,
            Value::symbol("memory-buffer-too-small")
        );
        assert_eq!(
            list_to_vec(&fixture.priv_.non_local_exit_data).unwrap(),
            vec![Value::fixnum(capacity as i64), Value::fixnum(4)]
        );
    }
}

#[test]
fn copy_string_contents_wrong_type_and_pending_exit_preserve_outputs() {
    let _ctx = Context::new();
    let mut fixture = TestEnv::new();
    let env = fixture.env_ptr();
    let wrong = lisp_to_value(env, Value::fixnum(1));
    let mut len = 19;
    assert!(!unsafe { module_copy_string_contents(env, wrong, std::ptr::null_mut(), &mut len) });
    assert_eq!(len, 19);
    assert_eq!(
        list_to_vec(&fixture.priv_.non_local_exit_data).unwrap(),
        vec![Value::symbol("stringp"), Value::fixnum(1)]
    );
    let original = fixture.priv_.non_local_exit_data;
    let valid = lisp_to_value(env, Value::string("later"));
    assert!(!unsafe { module_copy_string_contents(env, valid, std::ptr::null_mut(), &mut len) });
    assert_eq!(len, 19);
    assert_eq!(fixture.priv_.non_local_exit_data, original);
}
